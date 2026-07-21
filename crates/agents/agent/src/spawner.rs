//! Agent 启动器 — AgentSpawner / AgentFactory
//!
//! # 设计
//! - `AgentFactory` 是依赖注入点（D2 偏差），替代蓝图引用的 `crate::agents::create_agent`
//! - `AgentSpawner` 持有共享 registry / lifecycle / factory，执行 spawn 流程
//! - spawn 流程 8 步：创建描述符 → 应用覆盖 → 注册 → Created→Ready → load_code → on_init → Ready→Running → on_start
//!
//! # 偏差声明
//! - D1: `lifecycle: Rc<RefCell<LifecycleManager>>`（非蓝图的 `Rc<LifecycleManager>`），因 `force_state` 需要 `&mut self`
//! - D2: 新增 `AgentFactory` trait，蓝图 `load_code` 引用不存在的 `create_agent`
//! - D3: `spawn_blocking` 委托 `spawn`（Phase 1 单线程无异步运行时）
//! - D4: `spawn` 签名追加 `now: u64`（no_std 时间戳外部提供）
//! - D5: 错误清理用 `force_state`（Ready→Error 不在 TRANSITIONS 表中，`transition` 会拒绝）
//!
//! # no_std 合规
//! 仅使用 `alloc::*` 与 `core::*`，子模块不重复 `#![cfg_attr(not(test), no_std)]`。

use alloc::boxed::Box;
use alloc::rc::Rc;
use core::cell::RefCell;

use crate::{
    AgentConfig, AgentContext, AgentDescriptor, AgentEntry, AgentError, AgentId, AgentRegistry,
    AgentState, AgentType, LifecycleManager,
};

/// Agent 工厂 trait（object-safe，D2 偏差）.
///
/// 作为 `AgentSpawner` 的依赖注入点，替代蓝图引用的不存在的 `crate::agents::create_agent`。
/// 生产环境在启动时注册具体 factory（如 EnergyAgentFactory），测试提供 mock factory。
pub trait AgentFactory {
    /// 根据 Agent 类型与名称创建 Agent 实例.
    fn create(&self, agent_type: AgentType, name: &str) -> Result<Box<dyn AgentEntry>, AgentError>;
}

/// Agent 启动器.
///
/// 持有共享注册表、生命周期管理器与工厂引用，执行 Agent 启动流程。
pub struct AgentSpawner {
    registry: Rc<RefCell<AgentRegistry>>,
    lifecycle: Rc<RefCell<LifecycleManager>>,
    factory: Rc<dyn AgentFactory>,
}

impl AgentSpawner {
    /// 创建 AgentSpawner.
    pub fn new(
        registry: Rc<RefCell<AgentRegistry>>,
        lifecycle: Rc<RefCell<LifecycleManager>>,
        factory: Rc<dyn AgentFactory>,
    ) -> Self {
        AgentSpawner {
            registry,
            lifecycle,
            factory,
        }
    }

    /// 启动 Agent（D4 偏差：追加 `now: u64` 参数）.
    ///
    /// 执行 8 步流程：
    /// 1. 创建 `AgentDescriptor::new(config.agent_type, &config.name, now)`
    /// 2. 应用 `priority_override` / `mem_override`
    /// 3. 注册到 registry
    /// 4. `Created→Ready` 转换
    /// 5. `load_code`（委托 factory.create）— 失败时 `force_state(id, Error)` 并返回错误（D5）
    /// 6. 构造 `AgentContext`
    /// 7. `on_init` — 失败时 `force_state(id, Error)` 并返回错误（D5）
    /// 8. `Ready→Running` 转换
    /// 9. `on_start` — 失败时 `force_state(id, Error)` 并返回错误（D5）
    /// 10. 返回 `Ok(id)`
    pub fn spawn(&self, config: AgentConfig, now: u64) -> Result<AgentId, AgentError> {
        // Step 1: 创建描述符
        let mut desc = AgentDescriptor::new(config.agent_type, &config.name, now);

        // Step 2: 应用覆盖
        if let Some(p) = config.priority_override {
            desc.priority = p;
        }
        if let Some(m) = config.mem_override {
            desc.mem_quota = m;
        }

        // Step 3: 注册到 registry
        let id = self.registry.borrow_mut().register(desc)?;

        // Step 4: Created → Ready
        self.lifecycle
            .borrow()
            .transition(id, AgentState::Ready)
            .map_err(|e| {
                let _ = self
                    .lifecycle
                    .borrow_mut()
                    .force_state(id, AgentState::Error);
                e
            })?;

        // Step 5: load_code — 失败时 force_state(Error) 并返回错误（D5）
        let mut agent = self.load_code(&config).map_err(|e| {
            let _ = self
                .lifecycle
                .borrow_mut()
                .force_state(id, AgentState::Error);
            e
        })?;

        // Step 6: 构造上下文
        let mut ctx = self.init_context(id, &config);

        // Step 7: on_init — 失败时 force_state(Error) 并返回错误（D5）
        agent.on_init(&mut ctx).map_err(|e| {
            let _ = self
                .lifecycle
                .borrow_mut()
                .force_state(id, AgentState::Error);
            e
        })?;

        // Step 8: Ready → Running
        self.lifecycle
            .borrow()
            .transition(id, AgentState::Running)
            .map_err(|e| {
                let _ = self
                    .lifecycle
                    .borrow_mut()
                    .force_state(id, AgentState::Error);
                e
            })?;

        // Step 9: on_start — 失败时 force_state(Error) 并返回错误（D5）
        agent.on_start(&mut ctx).map_err(|e| {
            let _ = self
                .lifecycle
                .borrow_mut()
                .force_state(id, AgentState::Error);
            e
        })?;

        // Step 10: 返回 Ok(id)
        Ok(id)
    }

    /// 阻塞式启动 Agent（D3 偏差：Phase 1 单线程下与 `spawn` 等价）.
    pub fn spawn_blocking(&self, config: AgentConfig, now: u64) -> Result<AgentId, AgentError> {
        self.spawn(config, now)
    }

    /// 加载 Agent 代码（委托 factory.create）.
    fn load_code(&self, config: &AgentConfig) -> Result<Box<dyn AgentEntry>, AgentError> {
        self.factory.create(config.agent_type, &config.name)
    }

    /// 构造 AgentContext.
    fn init_context(&self, id: AgentId, config: &AgentConfig) -> AgentContext {
        AgentContext {
            agent_id: id,
            config: config.clone(),
            registry: self.registry.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::rc::Rc;
    use alloc::string::String;
    use core::cell::RefCell;

    use super::*;
    use crate::{AgentRegistry, LifecycleManager};

    // ---- Helper agent implementations ----

    /// Success Agent: on_init/on_start return Ok.
    struct SuccessAgent;
    impl AgentEntry for SuccessAgent {
        fn on_init(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> {
            Ok(())
        }
        fn on_start(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> {
            Ok(())
        }
        fn on_stop(&mut self, _ctx: &mut AgentContext) {}
    }

    /// FailInit Agent: on_init returns Err.
    struct FailInitAgent;
    impl AgentEntry for FailInitAgent {
        fn on_init(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> {
            Err(AgentError::InitFailed(String::from("test init failure")))
        }
        fn on_start(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> {
            Ok(())
        }
        fn on_stop(&mut self, _ctx: &mut AgentContext) {}
    }

    /// FailStart Agent: on_start returns Err.
    struct FailStartAgent;
    impl AgentEntry for FailStartAgent {
        fn on_init(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> {
            Ok(())
        }
        fn on_start(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> {
            Err(AgentError::StartFailed(String::from("test start failure")))
        }
        fn on_stop(&mut self, _ctx: &mut AgentContext) {}
    }

    // ---- Helper factory implementations ----

    /// Success factory: creates SuccessAgent.
    struct SuccessFactory;
    impl AgentFactory for SuccessFactory {
        fn create(
            &self,
            _agent_type: AgentType,
            _name: &str,
        ) -> Result<Box<dyn AgentEntry>, AgentError> {
            Ok(Box::new(SuccessAgent))
        }
    }

    /// Fail factory: load_code fails (CodeLoadFailed).
    struct FailFactory;
    impl AgentFactory for FailFactory {
        fn create(
            &self,
            _agent_type: AgentType,
            _name: &str,
        ) -> Result<Box<dyn AgentEntry>, AgentError> {
            Err(AgentError::CodeLoadFailed(String::from(
                "no agent registered",
            )))
        }
    }

    /// FailInit factory: creates FailInitAgent.
    struct FailInitFactory;
    impl AgentFactory for FailInitFactory {
        fn create(
            &self,
            _agent_type: AgentType,
            _name: &str,
        ) -> Result<Box<dyn AgentEntry>, AgentError> {
            Ok(Box::new(FailInitAgent))
        }
    }

    /// FailStart factory: creates FailStartAgent.
    struct FailStartFactory;
    impl AgentFactory for FailStartFactory {
        fn create(
            &self,
            _agent_type: AgentType,
            _name: &str,
        ) -> Result<Box<dyn AgentEntry>, AgentError> {
            Ok(Box::new(FailStartAgent))
        }
    }

    // ---- Helper functions ----

    /// Create an AgentSpawner with shared registry and lifecycle.
    fn make_spawner<F: AgentFactory + 'static>(
        factory: F,
    ) -> (
        AgentSpawner,
        Rc<RefCell<AgentRegistry>>,
        Rc<RefCell<LifecycleManager>>,
    ) {
        let reg = Rc::new(RefCell::new(AgentRegistry::new()));
        let lifecycle = Rc::new(RefCell::new(LifecycleManager::new(reg.clone())));
        let spawner = AgentSpawner::new(reg.clone(), lifecycle.clone(), Rc::new(factory));
        (spawner, reg, lifecycle)
    }

    /// Create a minimal AgentConfig.
    fn make_config(agent_type: AgentType, name: &str) -> AgentConfig {
        AgentConfig {
            agent_type,
            name: String::from(name),
            binary_path: None,
            config_path: None,
            priority_override: None,
            mem_override: None,
        }
    }

    // ---- Tests ----

    #[test]
    fn test_spawn_success() {
        let (spawner, _reg, lifecycle) = make_spawner(SuccessFactory);
        let config = make_config(AgentType::Energy, "e1");
        let result = spawner.spawn(config, 1000);
        assert!(result.is_ok());
        let id = result.unwrap();
        assert_eq!(
            lifecycle.borrow().current_state(id),
            Ok(AgentState::Running)
        );
    }

    #[test]
    fn test_spawn_returns_agent_id() {
        let (spawner, reg, _lifecycle) = make_spawner(SuccessFactory);
        let config = make_config(AgentType::Energy, "e1");
        let id = spawner.spawn(config, 1000).unwrap();
        assert!(reg.borrow().exists(id));
    }

    #[test]
    fn test_spawn_blocking_equivalent_to_spawn() {
        let (spawner, _reg, lifecycle) = make_spawner(SuccessFactory);
        let config = make_config(AgentType::Energy, "e1");
        let result = spawner.spawn_blocking(config, 1000);
        assert!(result.is_ok());
        let id = result.unwrap();
        assert_eq!(
            lifecycle.borrow().current_state(id),
            Ok(AgentState::Running)
        );
    }

    #[test]
    fn test_spawn_on_init_failure_goes_to_error() {
        let (spawner, reg, lifecycle) = make_spawner(FailInitFactory);
        let config = make_config(AgentType::Energy, "e1");
        let err = spawner.spawn(config, 1000).unwrap_err();
        assert_eq!(
            err,
            AgentError::InitFailed(String::from("test init failure"))
        );
        assert_eq!(reg.borrow().count(), 1);
        let id = reg.borrow().list_all()[0].agent_id;
        assert_eq!(lifecycle.borrow().current_state(id), Ok(AgentState::Error));
    }

    #[test]
    fn test_spawn_on_start_failure_goes_to_error() {
        let (spawner, reg, lifecycle) = make_spawner(FailStartFactory);
        let config = make_config(AgentType::Energy, "e1");
        let err = spawner.spawn(config, 1000).unwrap_err();
        assert_eq!(
            err,
            AgentError::StartFailed(String::from("test start failure"))
        );
        assert_eq!(reg.borrow().count(), 1);
        let id = reg.borrow().list_all()[0].agent_id;
        assert_eq!(lifecycle.borrow().current_state(id), Ok(AgentState::Error));
    }

    #[test]
    fn test_spawn_load_code_failure_goes_to_error() {
        let (spawner, reg, lifecycle) = make_spawner(FailFactory);
        let config = make_config(AgentType::Energy, "e1");
        let err = spawner.spawn(config, 1000).unwrap_err();
        assert_eq!(
            err,
            AgentError::CodeLoadFailed(String::from("no agent registered"))
        );
        assert_eq!(reg.borrow().count(), 1);
        let id = reg.borrow().list_all()[0].agent_id;
        assert_eq!(lifecycle.borrow().current_state(id), Ok(AgentState::Error));
    }

    #[test]
    fn test_spawn_priority_override_applied() {
        let (spawner, reg, _lifecycle) = make_spawner(SuccessFactory);
        // Device default priority is 100; override to 200 to verify it is applied.
        let mut config = make_config(AgentType::Device, "d1");
        config.priority_override = Some(200);
        let id = spawner.spawn(config, 1000).unwrap();
        let priority = reg.borrow().get(id).unwrap().priority;
        assert_eq!(priority, 200);
    }

    #[test]
    fn test_spawn_mem_override_applied() {
        let (spawner, reg, _lifecycle) = make_spawner(SuccessFactory);
        // Energy default mem_quota is 128 MB; override to 1024 to verify it is applied.
        let mut config = make_config(AgentType::Energy, "e1");
        config.mem_override = Some(1024);
        let id = spawner.spawn(config, 1000).unwrap();
        let mem_quota = reg.borrow().get(id).unwrap().mem_quota;
        assert_eq!(mem_quota, 1024);
    }

    #[test]
    fn test_spawn_multiple_agents_independent() {
        let (spawner, reg, lifecycle) = make_spawner(SuccessFactory);
        let id1 = spawner
            .spawn(make_config(AgentType::Energy, "e1"), 1000)
            .unwrap();
        let id2 = spawner
            .spawn(make_config(AgentType::Market, "m1"), 1000)
            .unwrap();
        let id3 = spawner
            .spawn(make_config(AgentType::Grid, "g1"), 1000)
            .unwrap();
        assert_eq!(reg.borrow().count(), 3);
        assert_eq!(
            lifecycle.borrow().current_state(id1),
            Ok(AgentState::Running)
        );
        assert_eq!(
            lifecycle.borrow().current_state(id2),
            Ok(AgentState::Running)
        );
        assert_eq!(
            lifecycle.borrow().current_state(id3),
            Ok(AgentState::Running)
        );
    }

    #[test]
    fn test_spawn_registers_in_registry() {
        let (spawner, reg, _lifecycle) = make_spawner(SuccessFactory);
        let config = make_config(AgentType::Energy, "e1");
        spawner.spawn(config, 1000).unwrap();
        assert_eq!(reg.borrow().count(), 1);
    }

    #[test]
    fn test_spawn_created_to_ready_to_running() {
        let (spawner, _reg, lifecycle) = make_spawner(SuccessFactory);
        let config = make_config(AgentType::Energy, "e1");
        let id = spawner.spawn(config, 1000).unwrap();
        // The internal sequence is Created -> Ready -> Running;
        // we verify the final end state is Running.
        assert_eq!(
            lifecycle.borrow().current_state(id),
            Ok(AgentState::Running)
        );
    }
}
