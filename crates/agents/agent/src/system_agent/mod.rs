//! SystemAgent — OS 级管理 Agent，统一管理 Agent 生命周期编排与系统资源监控.
//!
//! # 设计
//! - `SystemAgent` 是 EnerOS 的 OS 级管理 Agent，作为单步驱动的编排器
//! - 集成 registry / spawner / recovery / heartbeat / lifecycle / monitor 六大组件
//! - `tick(now)` 替代无限循环 `run`，由外部调度器按周期调用（D1 偏差）
//! - 返回 `Vec<SystemEvent>` 替代 log 记录（D6 偏差），调用方按需消费
//!
//! # 偏差声明
//! - D1: `tick(now) -> Vec<SystemEvent>` 替代蓝图的 `run()` 无限循环（no_std 无后台线程）
//! - D4: SystemAgent 显式持有 `lifecycle: Rc<RefCell<LifecycleManager>>` 字段
//!   （蓝图未显式列出，但 tick 需直接调用 force_state / transition）
//! - D6: 返回 `Vec<SystemEvent>` 替代 log（agent crate 零依赖无 logging 框架）
//! - D10: 采用 `mod.rs` + `monitor.rs` + `manager.rs` 子模块模式（非单文件）
//! - D11: 故障恢复时先 `force_state(id, Error)` 再 `handle_crash(id, now)`
//!   （CrashRecovery D9 要求 Agent 处于 Error 状态才执行恢复）
//! - D12: v0.42.0 新增 `dependency` 模块（DependencyGraph，Kahn 拓扑排序）
//! - D13: v0.42.0 新增 `recovery_orchestrator` 模块（RecoveryOrchestrator，优先级调度）
//!
//! # no_std 合规
//! 仅使用 `alloc::*` 与 `core::*`，子模块不重复 `#![cfg_attr(not(test), no_std)]`。

pub mod dependency;
pub mod manager;
pub mod monitor;
pub mod recovery_orchestrator;

use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

pub use dependency::DependencyGraph;
pub use monitor::{
    AgentResourceUsage, ResourceMonitor, ResourceSource, SystemConfig, SystemEvent, SystemStats,
};
pub use recovery_orchestrator::{priority_of, RecoveryOrchestrator, RecoveryPriority};

use crate::error::AgentError;
use crate::health::HealthStatus;
use crate::heartbeat::HeartbeatMonitor;
use crate::id::AgentId;
use crate::lifecycle::LifecycleManager;
use crate::recovery::CrashRecovery;
use crate::registry::AgentRegistry;
use crate::spawner::AgentSpawner;
use crate::types::AgentState;

/// OS 级管理 Agent — 统一管理 Agent 生命周期编排与系统资源监控.
///
/// 字段包含：
/// - `registry` - 共享注册表
/// - `spawner` - Agent 启动器
/// - `recovery` - 崩溃恢复管理器
/// - `heartbeat` - 心跳监控器
/// - `lifecycle` - 生命周期管理器（D4 偏差）
/// - `monitor` - 资源监控器
/// - `config` - 系统配置
///
/// 注：不 derive `Debug`，因含 `Rc<RefCell<...>>` 和 `Rc<dyn>` 字段
/// （与 AgentSpawner / CrashRecovery 同一约定）。
pub struct SystemAgent {
    registry: Rc<RefCell<AgentRegistry>>,
    spawner: Rc<AgentSpawner>,
    recovery: Rc<CrashRecovery>,
    heartbeat: Rc<RefCell<HeartbeatMonitor>>,
    lifecycle: Rc<RefCell<LifecycleManager>>,
    pub monitor: ResourceMonitor,
    config: SystemConfig,
}

impl SystemAgent {
    /// 创建 SystemAgent 实例.
    ///
    /// # 参数
    /// * `registry` - 共享注册表引用
    /// * `spawner` - Agent 启动器引用
    /// * `recovery` - 崩溃恢复管理器引用
    /// * `heartbeat` - 心跳监控器引用
    /// * `lifecycle` - 生命周期管理器引用（D4 偏差）
    /// * `config` - 系统配置
    pub fn new(
        registry: Rc<RefCell<AgentRegistry>>,
        spawner: Rc<AgentSpawner>,
        recovery: Rc<CrashRecovery>,
        heartbeat: Rc<RefCell<HeartbeatMonitor>>,
        lifecycle: Rc<RefCell<LifecycleManager>>,
        config: SystemConfig,
    ) -> Self {
        SystemAgent {
            registry,
            spawner,
            recovery,
            heartbeat,
            lifecycle,
            monitor: ResourceMonitor::new(),
            config,
        }
    }

    /// 单步执行（D1 偏差：单步 tick 替代无限循环 run）.
    ///
    /// 算法：
    /// 1. `monitor.poll()` 资源监控
    /// 2. `heartbeat.check(now)` 心跳检查
    /// 3. 对每个 Unhealthy Agent（D11 偏差）：
    ///    - `lifecycle.force_state(id, Error)` 强制转 Error
    ///    - `recovery.handle_crash(id, now)` 触发恢复
    ///    - 成功 → `SystemEvent::AgentRecovered`
    ///    - `MaxRestartsExceeded` → `SystemEvent::AgentRecoveryFailed`
    ///    - 其他错误 → `SystemEvent::AgentCrashed`
    /// 4. OOM 检查：若 `monitor.is_oom(threshold)` 且 `find_oom_victim()` 返回 `Some(victim)`：
    ///    - 直接调用 `lifecycle.transition(id, Suspended)`（避免与 manager.rs 的 suspend_agent
    ///      形成循环依赖，因 manager.rs 尚未实现）
    ///    - 产生 `SystemEvent::OomVictimSuspended { agent: victim }`
    /// 5. 过热检查：若 `monitor.is_overheat(threshold)`：产生 `SystemEvent::Overheat { temp }`
    /// 6. 返回 events
    ///
    /// # 参数
    /// * `now` - 当前时间戳（no_std 无系统时钟，外部提供）
    pub fn tick(&mut self, now: u64) -> Vec<SystemEvent> {
        let mut events: Vec<SystemEvent> = Vec::new();

        // 1. 资源监控
        self.monitor.poll();

        // 2. 心跳检查
        let health_results = self.heartbeat.borrow_mut().check(now);

        // 3. 故障恢复（D11：force_state Error 后 handle_crash）
        for (id, status) in health_results {
            if matches!(status, HealthStatus::Unhealthy) {
                // D11: 强制转 Error，handle_crash 要求 Agent 处于 Error 状态（D9）
                // force_state 需要 &mut self，故用 borrow_mut()
                let force_ok = self
                    .lifecycle
                    .borrow_mut()
                    .force_state(id, AgentState::Error)
                    .is_ok();
                if !force_ok {
                    events.push(SystemEvent::AgentCrashed { agent: id });
                    continue;
                }
                match self.recovery.handle_crash(id, now) {
                    Ok(()) => events.push(SystemEvent::AgentRecovered { agent: id }),
                    Err(AgentError::MaxRestartsExceeded { .. }) => {
                        events.push(SystemEvent::AgentRecoveryFailed { agent: id })
                    }
                    Err(_) => events.push(SystemEvent::AgentCrashed { agent: id }),
                }
            }
        }

        // 4. OOM 检查（D8：monitor 判阈值，SystemAgent 选 victim）
        if self.monitor.is_oom(self.config.oom_threshold_percent) {
            if let Some(victim) = self.find_oom_victim() {
                // 直接调用 lifecycle.transition（避免与 manager.rs 的 suspend_agent 形成循环依赖）
                if self
                    .lifecycle
                    .borrow()
                    .transition(victim, AgentState::Suspended)
                    .is_ok()
                {
                    events.push(SystemEvent::OomVictimSuspended { agent: victim });
                }
            }
        }

        // 5. 过热检查
        if self.monitor.is_overheat(self.config.overheat_threshold) {
            events.push(SystemEvent::Overheat {
                temp: self.monitor.temperature,
            });
        }

        events
    }

    /// 获取系统级统计.
    ///
    /// 从 registry 读取 agent_count / alive_agents / error_agents，
    /// 从 monitor 读取 cpu/mem/temp。
    pub fn get_system_stats(&self) -> SystemStats {
        let reg = self.registry.borrow();
        let all = reg.list_all();
        let mut alive = 0usize;
        let mut error_count = 0usize;
        for desc in &all {
            if desc.is_alive() {
                alive += 1;
            }
            if matches!(desc.state, AgentState::Error) {
                error_count += 1;
            }
        }
        SystemStats {
            cpu_usage: self.monitor.cpu_usage,
            mem_usage: self.monitor.mem_usage_percent(),
            temperature: self.monitor.temperature,
            agent_count: all.len(),
            alive_agents: alive,
            error_agents: error_count,
        }
    }

    /// 查找 OOM victim（D8 偏差：从 registry 选最低优先级的存活 Agent）.
    ///
    /// 遍历所有存活 Agent（`is_alive()` 返回 true），返回 priority 最低的 AgentId。
    /// 若无存活 Agent，返回 None。
    pub fn find_oom_victim(&self) -> Option<AgentId> {
        let reg = self.registry.borrow();
        let mut victim: Option<AgentId> = None;
        let mut victim_priority: u8 = u8::MAX;
        for desc in reg.list_alive() {
            if desc.priority < victim_priority {
                victim_priority = desc.priority;
                victim = Some(desc.agent_id);
            }
        }
        victim
    }

    /// 获取资源监控器引用.
    pub fn monitor(&self) -> &ResourceMonitor {
        &self.monitor
    }

    /// 获取系统配置引用.
    pub fn config(&self) -> &SystemConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use alloc::rc::Rc;
    use alloc::string::String;
    use core::cell::RefCell;

    use super::*;
    use crate::{
        AgentDescriptor, AgentRegistry, AgentState, AgentType, HeartbeatMonitor,
        InMemoryCheckpointStore, LifecycleManager,
    };

    /// Mock AgentFactory（返回 Err，测试中不调用 spawn）.
    struct MockFactory;
    impl crate::spawner::AgentFactory for MockFactory {
        fn create(
            &self,
            _agent_type: AgentType,
            _name: &str,
        ) -> Result<Box<dyn crate::init::AgentEntry>, AgentError> {
            Err(AgentError::InitFailed(String::from("mock factory")))
        }
    }

    /// Helper: construct SystemAgent with empty registry.
    fn make_system_agent() -> SystemAgent {
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
        let spawner = Rc::new(crate::spawner::AgentSpawner::new(
            reg.clone(),
            lifecycle.clone(),
            factory,
        ));
        let config = SystemConfig::default();
        SystemAgent::new(reg, spawner, recovery, heartbeat, lifecycle, config)
    }

    /// Helper: construct SystemAgent with shared deps (returned for tests needing local access).
    #[allow(clippy::type_complexity)]
    fn make_system_agent_with_deps() -> (
        SystemAgent,
        Rc<RefCell<AgentRegistry>>,
        Rc<RefCell<HeartbeatMonitor>>,
        Rc<RefCell<LifecycleManager>>,
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
        let spawner = Rc::new(crate::spawner::AgentSpawner::new(
            reg.clone(),
            lifecycle.clone(),
            factory,
        ));
        let sa = SystemAgent::new(
            reg.clone(),
            spawner,
            recovery,
            heartbeat.clone(),
            lifecycle.clone(),
            SystemConfig::default(),
        );
        (sa, reg, heartbeat, lifecycle)
    }

    /// Helper: register an Agent in the given state, return its AgentId.
    fn make_agent(
        reg: &Rc<RefCell<AgentRegistry>>,
        lifecycle: &Rc<RefCell<LifecycleManager>>,
        agent_type: AgentType,
        name: &str,
        priority: u8,
        state: AgentState,
        now: u64,
    ) -> AgentId {
        let mut desc = AgentDescriptor::new(agent_type, name, now);
        desc.priority = priority;
        let id = desc.agent_id;
        reg.borrow_mut().register(desc).unwrap();
        lifecycle.borrow_mut().force_state(id, state).unwrap();
        id
    }

    #[test]
    fn test_system_agent_new() {
        let sa = make_system_agent();
        // monitor is empty
        assert_eq!(sa.monitor().cpu_usage, 0.0);
        assert_eq!(sa.monitor().mem_total, 0);
        // config is default
        assert_eq!(sa.config().oom_threshold_percent, 0.9);
        assert_eq!(sa.config().overheat_threshold, 80.0);
        assert_eq!(sa.config().monitor_interval_ms, 1000);
    }

    #[test]
    fn test_system_agent_tick_no_events() {
        let mut sa = make_system_agent();
        let events = sa.tick(1000);
        assert!(
            events.is_empty(),
            "no events with empty system, got: {:?}",
            events
        );
    }

    #[test]
    fn test_system_agent_tick_overheat() {
        let mut sa = make_system_agent();
        sa.monitor.set_values(0.5, 50, 100, 85.0); // 85 > 80
        let events = sa.tick(1000);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, SystemEvent::Overheat { temp: 85.0 })),
            "expected Overheat event, got: {:?}",
            events
        );
    }

    #[test]
    fn test_system_agent_tick_oom_suspends_victim() {
        let (mut sa, reg, heartbeat, lifecycle) = make_system_agent_with_deps();

        // Create 2 agents: high priority 200, low priority 100
        let high = make_agent(
            &reg,
            &lifecycle,
            AgentType::Energy,
            "high",
            200,
            AgentState::Running,
            1000,
        );
        let low = make_agent(
            &reg,
            &lifecycle,
            AgentType::Device,
            "low",
            100,
            AgentState::Running,
            1000,
        );

        // Register heartbeats so check() returns Healthy (within window)
        heartbeat.borrow_mut().register(high, 1000);
        heartbeat.borrow_mut().register(low, 1000);

        // Set OOM condition: 95% mem used, threshold 0.9
        sa.monitor.set_values(0.5, 95, 100, 50.0);

        let events = sa.tick(1500); // elapsed=500, not > 1000, no unhealthy
                                    // Expect OomVictimSuspended for low (priority 100 < 200)
        assert!(
            events
                .iter()
                .any(|e| matches!(e, SystemEvent::OomVictimSuspended { agent } if *agent == low)),
            "expected OomVictimSuspended for low-priority agent, got: {:?}",
            events
        );
        // Verify low is Suspended
        assert_eq!(
            lifecycle.borrow().current_state(low),
            Ok(AgentState::Suspended)
        );
    }

    #[test]
    fn test_system_agent_get_system_stats() {
        let mut sa = make_system_agent();
        sa.monitor.set_values(0.5, 50, 100, 65.0);
        // Note: registry is empty in make_system_agent
        let stats = sa.get_system_stats();
        assert_eq!(stats.cpu_usage, 0.5);
        assert_eq!(stats.mem_usage, 0.5);
        assert_eq!(stats.temperature, 65.0);
        assert_eq!(stats.agent_count, 0);
        assert_eq!(stats.alive_agents, 0);
        assert_eq!(stats.error_agents, 0);
    }

    #[test]
    fn test_system_agent_find_oom_victim_lowest_priority() {
        // Empty registry → None
        let sa = make_system_agent();
        let victim = sa.find_oom_victim();
        assert!(victim.is_none(), "no alive agents should return None");
    }

    #[test]
    fn test_system_agent_find_oom_victim_no_alive() {
        let sa = make_system_agent();
        assert!(sa.find_oom_victim().is_none());
    }

    #[test]
    fn test_system_agent_find_oom_victim_multiple_agents() {
        let (sa, reg, _heartbeat, lifecycle) = make_system_agent_with_deps();

        // Add 3 agents with different priorities
        let _a1 = make_agent(
            &reg,
            &lifecycle,
            AgentType::Energy,
            "a1",
            200,
            AgentState::Running,
            1000,
        );
        let low = make_agent(
            &reg,
            &lifecycle,
            AgentType::Device,
            "low",
            50,
            AgentState::Running,
            1000,
        );
        let _a3 = make_agent(
            &reg,
            &lifecycle,
            AgentType::System,
            "a3",
            255,
            AgentState::Running,
            1000,
        );

        let victim = sa.find_oom_victim();
        assert_eq!(victim, Some(low), "lowest priority (50) should be victim");
    }
}
