//! Agent 管理方法 — start/stop/suspend/resume
//!
//! # 设计
//! - `start_agent` 委托 spawner.spawn + heartbeat.register（D5：接受 now 参数）
//! - `stop_agent` 使用 force_state(Dead)（D9：Suspended→Dead 非法，必须用 force_state 绕过转换表）
//! - `suspend_agent` / `resume_agent` 使用 transition（合法转换）
//!
//! # no_std 合规
//! 仅使用 `alloc::*` 与 `core::*`，子模块不重复 `#![cfg_attr(not(test), no_std)]`。

use crate::error::AgentError;
use crate::id::AgentId;
use crate::init::AgentConfig;
use crate::types::AgentState;

impl super::SystemAgent {
    /// 启动 Agent（D5 偏差：接受 now 参数）.
    ///
    /// # 算法
    /// 1. `spawner.spawn(config, now)` — 调用 spawner 启动 Agent，返回 AgentId
    /// 2. `heartbeat.register(id, now)` — 注册心跳
    /// 3. 返回 `Ok(id)`
    ///
    /// # 参数
    /// * `config` - Agent 配置
    /// * `now` - 当前时间戳（no_std 无系统时钟，外部提供）
    ///
    /// # 错误
    /// - `spawner.spawn` 传播的错误（如 `CodeLoadFailed` / `InitFailed` / `StartFailed`）
    pub fn start_agent(&self, config: AgentConfig, now: u64) -> Result<AgentId, AgentError> {
        let id = self.spawner.spawn(config, now)?;
        self.heartbeat.borrow_mut().register(id, now);
        Ok(id)
    }

    /// 停止 Agent（D9 偏差：使用 force_state）.
    ///
    /// # 算法
    /// 1. `lifecycle.force_state(id, Dead)` — 强制转为 Dead 状态（绕过转换表，因 Suspended→Dead 非法）
    /// 2. `heartbeat.unregister(id)` — 注销心跳
    /// 3. `registry.unregister(id)` — 注销注册（返回 AgentNotFound 如果不存在）
    /// 4. 返回 `Ok(())`
    ///
    /// # 参数
    /// * `id` - 要停止的 Agent ID
    ///
    /// # 错误
    /// - `AgentNotFound` - Agent 不在注册表中
    /// - `lifecycle.force_state` 传播的其他错误
    pub fn stop_agent(&self, id: AgentId) -> Result<(), AgentError> {
        // D9: force_state 绕过转换表（Suspended→Dead 非法）
        // 注意：先 force_state，再 unregister，避免 unregister 后 force_state 找不到 agent
        self.lifecycle
            .borrow_mut()
            .force_state(id, AgentState::Dead)?;
        self.heartbeat.borrow_mut().unregister(id);
        self.registry.borrow_mut().unregister(id)?;
        Ok(())
    }

    /// 挂起 Agent.
    ///
    /// 调用 `lifecycle.transition(id, Suspended)`（合法转换：Running→Suspended）。
    ///
    /// # 参数
    /// * `id` - 要挂起的 Agent ID
    ///
    /// # 错误
    /// - `InvalidStateTransition` - Agent 不在 Running 状态
    /// - `AgentNotFound` - Agent 不存在
    pub fn suspend_agent(&self, id: AgentId) -> Result<(), AgentError> {
        self.lifecycle
            .borrow()
            .transition(id, AgentState::Suspended)?;
        Ok(())
    }

    /// 恢复 Agent.
    ///
    /// 调用 `lifecycle.transition(id, Running)`（合法转换：Suspended→Running）。
    ///
    /// # 参数
    /// * `id` - 要恢复的 Agent ID
    ///
    /// # 错误
    /// - `InvalidStateTransition` - Agent 不在 Suspended 状态
    /// - `AgentNotFound` - Agent 不存在
    pub fn resume_agent(&self, id: AgentId) -> Result<(), AgentError> {
        self.lifecycle
            .borrow()
            .transition(id, AgentState::Running)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use alloc::rc::Rc;
    use alloc::string::String;
    use core::cell::RefCell;

    use super::super::{SystemAgent, SystemConfig};
    use crate::{
        AgentConfig, AgentContext, AgentEntry, AgentError, AgentFactory, AgentId, AgentRegistry,
        AgentSpawner, AgentState, AgentType, CrashRecovery, HeartbeatMonitor,
        InMemoryCheckpointStore, LifecycleManager,
    };

    /// Stub AgentEntry for testing (does nothing).
    struct StubEntry;
    impl AgentEntry for StubEntry {
        fn on_init(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> {
            Ok(())
        }
        fn on_start(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> {
            Ok(())
        }
        fn on_stop(&mut self, _ctx: &mut AgentContext) {}
    }

    /// Mock AgentFactory that returns StubEntry.
    struct MockFactory;
    impl AgentFactory for MockFactory {
        fn create(
            &self,
            _agent_type: AgentType,
            _name: &str,
        ) -> Result<Box<dyn AgentEntry>, AgentError> {
            Ok(Box::new(StubEntry))
        }
    }

    /// Helper: create a minimal AgentConfig.
    fn make_config(name: &str) -> AgentConfig {
        AgentConfig {
            agent_type: AgentType::Energy,
            name: String::from(name),
            binary_path: None,
            config_path: None,
            priority_override: None,
            mem_override: None,
        }
    }

    /// Helper: construct SystemAgent with all dependencies.
    /// Returns (SystemAgent, registry, lifecycle, heartbeat).
    #[allow(clippy::type_complexity)]
    fn make_system_agent() -> (
        SystemAgent,
        Rc<RefCell<AgentRegistry>>,
        Rc<RefCell<LifecycleManager>>,
        Rc<RefCell<HeartbeatMonitor>>,
    ) {
        let reg = Rc::new(RefCell::new(AgentRegistry::new()));
        let heartbeat = Rc::new(RefCell::new(HeartbeatMonitor::with_defaults()));
        let lifecycle = Rc::new(RefCell::new(LifecycleManager::new(reg.clone())));
        let checkpoint_store: Rc<dyn crate::checkpoint::CheckpointStore> =
            Rc::new(InMemoryCheckpointStore::new());
        let recovery = Rc::new(CrashRecovery::with_defaults(
            reg.clone(),
            heartbeat.clone(),
            lifecycle.clone(),
            checkpoint_store,
        ));
        let factory: Rc<MockFactory> = Rc::new(MockFactory);
        let spawner = Rc::new(AgentSpawner::new(reg.clone(), lifecycle.clone(), factory));
        let sa = SystemAgent::new(
            reg.clone(),
            spawner,
            recovery,
            heartbeat.clone(),
            lifecycle.clone(),
            SystemConfig::default(),
        );
        (sa, reg, lifecycle, heartbeat)
    }

    #[test]
    fn test_start_agent_success() {
        let (sa, reg, _, heartbeat) = make_system_agent();
        let config = make_config("test-agent");
        let id = sa
            .start_agent(config, 1000)
            .expect("start_agent should succeed");
        // Verify agent is registered
        assert!(reg.borrow().get(id).is_some());
        // Verify heartbeat registered (id should be in heartbeat monitor — hard to introspect, but no panic is good)
        let _ = heartbeat;
    }

    #[test]
    fn test_stop_agent_success() {
        let (sa, reg, lifecycle, _) = make_system_agent();
        // First start an agent
        let config = make_config("test");
        let id = sa.start_agent(config, 1000).unwrap();
        assert!(reg.borrow().get(id).is_some());
        // Now stop it
        sa.stop_agent(id).expect("stop_agent should succeed");
        // Verify: agent no longer in registry
        assert!(reg.borrow().get(id).is_none());
        // Verify: state is Dead (but agent is unregistered, so current_state returns AgentNotFound)
        assert_eq!(
            lifecycle.borrow().current_state(id),
            Err(AgentError::AgentNotFound)
        );
    }

    #[test]
    fn test_stop_agent_not_found() {
        let (sa, _, _, _) = make_system_agent();
        let fake_id = AgentId::generate();
        let result = sa.stop_agent(fake_id);
        // force_state should fail with AgentNotFound
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), AgentError::AgentNotFound);
    }

    #[test]
    fn test_suspend_agent_success() {
        let (sa, reg, lifecycle, _) = make_system_agent();
        // Start agent (which goes through Created→Ready→Running)
        let config = make_config("test");
        let id = sa.start_agent(config, 1000).unwrap();
        assert!(reg.borrow().get(id).is_some());
        assert_eq!(
            lifecycle.borrow().current_state(id),
            Ok(AgentState::Running)
        );
        // Suspend
        sa.suspend_agent(id).expect("suspend should succeed");
        assert_eq!(
            lifecycle.borrow().current_state(id),
            Ok(AgentState::Suspended)
        );
    }

    #[test]
    fn test_resume_agent_success() {
        let (sa, _, lifecycle, _) = make_system_agent();
        let config = make_config("test");
        let id = sa.start_agent(config, 1000).unwrap();
        sa.suspend_agent(id).unwrap();
        sa.resume_agent(id).expect("resume should succeed");
        assert_eq!(
            lifecycle.borrow().current_state(id),
            Ok(AgentState::Running)
        );
    }

    #[test]
    fn test_suspend_resume_cycle() {
        let (sa, _, lifecycle, _) = make_system_agent();
        let config = make_config("test");
        let id = sa.start_agent(config, 1000).unwrap();
        // Multiple suspend/resume cycles
        for _ in 0..3 {
            sa.suspend_agent(id).unwrap();
            assert_eq!(
                lifecycle.borrow().current_state(id),
                Ok(AgentState::Suspended)
            );
            sa.resume_agent(id).unwrap();
            assert_eq!(
                lifecycle.borrow().current_state(id),
                Ok(AgentState::Running)
            );
        }
    }
}
