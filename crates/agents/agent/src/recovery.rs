//! Agent 崩溃恢复 — CrashRecovery
//!
//! # 设计
//! - `CrashRecovery` 提供 Agent 崩溃后的自动重启能力，是 v0.38.0 的核心模块
//! - `handle_crash` 算法：Error → Recovering → 检查重启次数 → restart 或 Dead
//! - 重启策略：Recovering → Ready → Running 状态转换 + 心跳重注册（D5：不重载代码）
//! - 检查点保存/恢复委托给 `CheckpointStore` trait（D1/D8）
//!
//! # 偏差声明
//! - D1: `CheckpointStore` 为 trait（蓝图为 struct with `Box<dyn FileSystem>`），因 agent crate 保持零外部依赖
//! - D2: `handle_crash`/`restart` 接受 `now: u64` 参数（no_std 无系统时钟；蓝图使用不存在的 `crate::time::now_ms()`）
//! - D3: `lifecycle: Rc<RefCell<LifecycleManager>>`（蓝图为 `Rc<LifecycleManager>`）；`force_state` 需要 `&mut self`，需内部可变性
//! - D4: `registry` 作为构造参数直接传入（蓝图使用 `spawner.registry.borrow()`）；`AgentSpawner.registry` 为私有字段无公开访问器
//! - D5: CrashRecovery 不持有 `spawner` 字段；restart 仅执行状态转换（Recovering→Ready→Running），不重载 Agent 代码（由调用方负责重新执行）
//! - D6: `heartbeat.register(id, now)` — v0.37.0 D2 兼容；HeartbeatMonitor::register 接受 `now` 参数
//! - D8: `Checkpointable` trait 已定义但 CrashRecovery 不直接调用；调用方负责编排 save_state/restore_state
//! - D9: `handle_crash` 假定 Agent 已处于 `Error` 状态（调用方须先转换到 Error）；对非 Error 状态调用将返回 `InvalidStateTransition`
//!
//! # no_std 合规
//! 仅使用 `alloc::*` 与 `core::*`，子模块不重复 `#![cfg_attr(not(test), no_std)]`。

use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::checkpoint::CheckpointStore;
use crate::error::AgentError;
use crate::heartbeat::HeartbeatMonitor;
use crate::id::AgentId;
use crate::lifecycle::LifecycleManager;
use crate::registry::AgentRegistry;
use crate::types::AgentState;

/// 默认最大重启次数.
const DEFAULT_MAX_RESTARTS: u32 = 3;

/// Agent 崩溃恢复管理器.
///
/// 提供 Agent 崩溃后的自动重启能力，集成生命周期管理、心跳监控与检查点存储。
/// 通过 `handle_crash` 触发恢复流程，按重启次数上限决定重启或标记为 Dead。
///
/// # 字段
/// - `registry` - 共享注册表引用（D4 偏差：直接传入而非通过 spawner）
/// - `heartbeat` - 共享心跳监控器引用
/// - `lifecycle` - 共享生命周期管理器引用（D3 偏差：`Rc<RefCell<...>>`）
/// - `checkpoint_store` - 检查点存储 trait 对象（D1 偏差）
/// - `max_restarts` - 最大重启次数上限
///
/// 注：不 derive `Debug`，因 `Rc<dyn CheckpointStore>` 与 `Rc<RefCell<LifecycleManager>>`
/// 字段无法自动派生 Debug（与 `AgentSpawner` 同一约定）。
pub struct CrashRecovery {
    registry: Rc<RefCell<AgentRegistry>>,
    heartbeat: Rc<RefCell<HeartbeatMonitor>>,
    lifecycle: Rc<RefCell<LifecycleManager>>,
    checkpoint_store: Rc<dyn CheckpointStore>,
    max_restarts: u32,
}

impl CrashRecovery {
    /// 创建 CrashRecovery 实例.
    ///
    /// # 参数
    /// * `registry` - 共享注册表引用
    /// * `heartbeat` - 共享心跳监控器引用
    /// * `lifecycle` - 共享生命周期管理器引用
    /// * `checkpoint_store` - 检查点存储 trait 对象
    /// * `max_restarts` - 最大重启次数上限
    pub fn new(
        registry: Rc<RefCell<AgentRegistry>>,
        heartbeat: Rc<RefCell<HeartbeatMonitor>>,
        lifecycle: Rc<RefCell<LifecycleManager>>,
        checkpoint_store: Rc<dyn CheckpointStore>,
        max_restarts: u32,
    ) -> Self {
        CrashRecovery {
            registry,
            heartbeat,
            lifecycle,
            checkpoint_store,
            max_restarts,
        }
    }

    /// 使用默认最大重启次数创建 CrashRecovery 实例.
    ///
    /// 等价于 `new(...)` 并传入 `DEFAULT_MAX_RESTARTS`。
    ///
    /// # 参数
    /// * `registry` - 共享注册表引用
    /// * `heartbeat` - 共享心跳监控器引用
    /// * `lifecycle` - 共享生命周期管理器引用
    /// * `checkpoint_store` - 检查点存储 trait 对象
    pub fn with_defaults(
        registry: Rc<RefCell<AgentRegistry>>,
        heartbeat: Rc<RefCell<HeartbeatMonitor>>,
        lifecycle: Rc<RefCell<LifecycleManager>>,
        checkpoint_store: Rc<dyn CheckpointStore>,
    ) -> Self {
        Self::new(
            registry,
            heartbeat,
            lifecycle,
            checkpoint_store,
            DEFAULT_MAX_RESTARTS,
        )
    }

    /// 处理 Agent 崩溃，执行自动恢复流程.
    ///
    /// # 算法（D9：假定 Agent 处于 Error 状态）
    /// 1. Error → Recovering 状态转换
    /// 2. 读取 `restart_count`
    /// 3. 若 `restart_count >= max_restarts`：Recovering → Dead，返回 `MaxRestartsExceeded`
    /// 4. 否则调用 `restart(id, now)`
    ///
    /// # 参数
    /// * `id` - 崩溃的 Agent ID
    /// * `now` - 当前时间戳（D2 偏差：no_std 无系统时钟）
    ///
    /// # 错误
    /// - `InvalidStateTransition` - Agent 不在 Error 状态（D9）
    /// - `AgentNotFound` - Agent 不存在于注册表
    /// - `MaxRestartsExceeded` - 超过最大重启次数，Agent 已转为 Dead
    /// - `restart` 传播的其他错误
    pub fn handle_crash(&self, id: AgentId, now: u64) -> Result<(), AgentError> {
        // D9: Error → Recovering（调用方须先将 Agent 转为 Error）
        self.lifecycle
            .borrow()
            .transition(id, AgentState::Recovering)?;

        // 读取当前重启次数（未找到视为 0）
        let restart_count = self
            .registry
            .borrow()
            .get(id)
            .map(|d| d.restart_count)
            .unwrap_or(0);

        // 超过最大重启次数：转为 Dead 并返回错误
        if restart_count >= self.max_restarts {
            self.lifecycle.borrow().transition(id, AgentState::Dead)?;
            return Err(AgentError::MaxRestartsExceeded {
                agent_id: id,
                count: restart_count,
            });
        }

        // 执行重启
        self.restart(id, now)?;
        Ok(())
    }

    /// 重启 Agent，执行状态转换与心跳重注册.
    ///
    /// # 算法（D5：仅状态转换，不重载代码）
    /// 1. Recovering → Ready
    /// 2. Ready → Running
    /// 3. 更新描述符：`restart_count += 1`，`last_heartbeat = now`
    /// 4. 重新注册心跳（D6：register 接受 now 参数）
    ///
    /// # 参数
    /// * `id` - 要重启的 Agent ID
    /// * `now` - 当前时间戳（D2 偏差）
    ///
    /// # 错误
    /// - `InvalidStateTransition` - Agent 不在 Recovering 状态
    /// - `AgentNotFound` - Agent 不存在于注册表
    pub fn restart(&self, id: AgentId, now: u64) -> Result<(), AgentError> {
        // D5: 仅状态转换，不重载 Agent 代码
        self.lifecycle.borrow().transition(id, AgentState::Ready)?;
        self.lifecycle
            .borrow()
            .transition(id, AgentState::Running)?;

        // 更新描述符（块作用域显式释放 reg 借用，避免与 heartbeat 借用冲突）
        {
            let mut reg = self.registry.borrow_mut();
            if let Some(desc) = reg.get_mut(id) {
                desc.restart_count += 1;
                desc.last_heartbeat = now;
            }
        }

        // D6: 重新注册心跳
        self.heartbeat.borrow_mut().register(id, now);

        Ok(())
    }

    /// 从检查点存储加载 Agent 状态数据.
    ///
    /// 委托给 `CheckpointStore::load`（D1 偏差）。
    /// 调用方负责通过 `Checkpointable::restore_state` 应用字节到 Agent 实例（D8 偏差）。
    ///
    /// # 参数
    /// * `id` - Agent ID
    ///
    /// # 返回
    /// - `Ok(Some(data))` - 存在检查点数据
    /// - `Ok(None)` - 无检查点数据
    ///
    /// # 错误
    /// - `CheckpointStore::load` 传播的错误（如 `CheckpointCorrupted`）
    pub fn restore_checkpoint(&self, id: AgentId) -> Result<Option<Vec<u8>>, AgentError> {
        self.checkpoint_store.load(id)
    }

    /// 保存 Agent 状态数据到检查点存储.
    ///
    /// 委托给 `CheckpointStore::save`（D1 偏差）。
    /// 调用方负责通过 `Checkpointable::save_state` 序列化 Agent 状态为字节（D8 偏差）。
    ///
    /// # 参数
    /// * `id` - Agent ID
    /// * `data` - 检查点字节数据
    ///
    /// # 错误
    /// - `CheckpointStore::save` 传播的错误
    pub fn save_checkpoint(&self, id: AgentId, data: &[u8]) -> Result<(), AgentError> {
        self.checkpoint_store.save(id, data)
    }
}

#[cfg(test)]
mod tests {
    use alloc::rc::Rc;
    use core::cell::RefCell;

    use super::*;
    use crate::{
        AgentDescriptor, AgentId, AgentRegistry, AgentState, AgentType, HeartbeatMonitor,
        InMemoryCheckpointStore, LifecycleManager,
    };

    /// Helper: construct a full CrashRecovery environment with DEFAULT_MAX_RESTARTS.
    #[allow(clippy::type_complexity)]
    fn make_recovery() -> (
        CrashRecovery,
        Rc<RefCell<AgentRegistry>>,
        Rc<RefCell<LifecycleManager>>,
        Rc<RefCell<HeartbeatMonitor>>,
    ) {
        let reg = Rc::new(RefCell::new(AgentRegistry::new()));
        let heartbeat = Rc::new(RefCell::new(HeartbeatMonitor::with_defaults()));
        let lifecycle = Rc::new(RefCell::new(LifecycleManager::new(reg.clone())));
        let checkpoint_store: Rc<dyn CheckpointStore> = Rc::new(InMemoryCheckpointStore::new());
        let recovery = CrashRecovery::with_defaults(
            reg.clone(),
            heartbeat.clone(),
            lifecycle.clone(),
            checkpoint_store,
        );
        (recovery, reg, lifecycle, heartbeat)
    }

    /// Helper: same as make_recovery but with custom max_restarts.
    #[allow(clippy::type_complexity)]
    fn make_recovery_with_max(
        max_restarts: u32,
    ) -> (
        CrashRecovery,
        Rc<RefCell<AgentRegistry>>,
        Rc<RefCell<LifecycleManager>>,
        Rc<RefCell<HeartbeatMonitor>>,
    ) {
        let reg = Rc::new(RefCell::new(AgentRegistry::new()));
        let heartbeat = Rc::new(RefCell::new(HeartbeatMonitor::with_defaults()));
        let lifecycle = Rc::new(RefCell::new(LifecycleManager::new(reg.clone())));
        let checkpoint_store: Rc<dyn CheckpointStore> = Rc::new(InMemoryCheckpointStore::new());
        let recovery = CrashRecovery::new(
            reg.clone(),
            heartbeat.clone(),
            lifecycle.clone(),
            checkpoint_store,
            max_restarts,
        );
        (recovery, reg, lifecycle, heartbeat)
    }

    /// Helper: spawn an agent and force it into Error state.
    fn spawn_agent_at_error(
        reg: &Rc<RefCell<AgentRegistry>>,
        lifecycle: &Rc<RefCell<LifecycleManager>>,
        agent_type: AgentType,
        name: &str,
        now: u64,
    ) -> AgentId {
        let desc = AgentDescriptor::new(agent_type, name, now);
        let id = reg.borrow_mut().register(desc).unwrap();
        lifecycle
            .borrow_mut()
            .force_state(id, AgentState::Error)
            .unwrap();
        id
    }

    // Agent in Error (restart_count=0), handle_crash → Ok, state Running, restart_count=1
    #[test]
    fn test_handle_crash_first_restart() {
        let (recovery, reg, lifecycle, _) = make_recovery();
        let id = spawn_agent_at_error(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);
        assert_eq!(reg.borrow().get(id).unwrap().restart_count, 0);
        let result = recovery.handle_crash(id, 2000);
        assert!(result.is_ok());
        assert_eq!(
            lifecycle.borrow().current_state(id),
            Ok(AgentState::Running)
        );
        assert_eq!(reg.borrow().get(id).unwrap().restart_count, 1);
    }

    // Pre-set restart_count=1, handle_crash → Ok, state Running, restart_count=2
    #[test]
    fn test_handle_crash_second_restart() {
        let (recovery, reg, lifecycle, _) = make_recovery();
        let id = spawn_agent_at_error(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);
        reg.borrow_mut().get_mut(id).unwrap().restart_count = 1;
        let result = recovery.handle_crash(id, 2000);
        assert!(result.is_ok());
        assert_eq!(
            lifecycle.borrow().current_state(id),
            Ok(AgentState::Running)
        );
        assert_eq!(reg.borrow().get(id).unwrap().restart_count, 2);
    }

    // Pre-set restart_count=2, handle_crash → Ok, state Running, restart_count=3
    #[test]
    fn test_handle_crash_third_restart() {
        let (recovery, reg, lifecycle, _) = make_recovery();
        let id = spawn_agent_at_error(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);
        reg.borrow_mut().get_mut(id).unwrap().restart_count = 2;
        let result = recovery.handle_crash(id, 2000);
        assert!(result.is_ok());
        assert_eq!(
            lifecycle.borrow().current_state(id),
            Ok(AgentState::Running)
        );
        assert_eq!(reg.borrow().get(id).unwrap().restart_count, 3);
    }

    // Pre-set restart_count=3, handle_crash → Err(MaxRestartsExceeded), state becomes Dead
    #[test]
    fn test_handle_crash_exceeds_max_restarts() {
        let (recovery, reg, lifecycle, _) = make_recovery();
        let id = spawn_agent_at_error(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);
        reg.borrow_mut().get_mut(id).unwrap().restart_count = 3;
        let result = recovery.handle_crash(id, 2000);
        assert_eq!(
            result,
            Err(AgentError::MaxRestartsExceeded {
                agent_id: id,
                count: 3
            })
        );
        assert_eq!(lifecycle.borrow().current_state(id), Ok(AgentState::Dead));
    }

    // Agent in Running state (not Error), handle_crash → Err(InvalidStateTransition)
    #[test]
    fn test_handle_crash_not_in_error_state() {
        let (recovery, reg, lifecycle, _) = make_recovery();
        let desc = AgentDescriptor::new(AgentType::Energy, "agent-1", 1000);
        let id = reg.borrow_mut().register(desc).unwrap();
        lifecycle
            .borrow_mut()
            .force_state(id, AgentState::Running)
            .unwrap();
        let result = recovery.handle_crash(id, 2000);
        assert_eq!(
            result,
            Err(AgentError::InvalidStateTransition {
                from: AgentState::Running,
                to: AgentState::Recovering
            })
        );
    }

    // handle_crash on a non-existent id → Err(AgentNotFound)
    #[test]
    fn test_handle_crash_nonexistent_agent() {
        let (recovery, _, _, _) = make_recovery();
        let fake_id = AgentId::generate();
        let result = recovery.handle_crash(fake_id, 2000);
        assert_eq!(result, Err(AgentError::AgentNotFound));
    }

    // Agent in Recovering state, restart(now) → Ok, state becomes Running
    #[test]
    fn test_restart_transitions_to_running() {
        let (recovery, reg, lifecycle, _) = make_recovery();
        let id = spawn_agent_at_error(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);
        lifecycle
            .borrow_mut()
            .force_state(id, AgentState::Recovering)
            .unwrap();
        let result = recovery.restart(id, 2000);
        assert!(result.is_ok());
        assert_eq!(
            lifecycle.borrow().current_state(id),
            Ok(AgentState::Running)
        );
    }

    // restart(now) → Ok, restart_count incremented by 1
    #[test]
    fn test_restart_increments_restart_count() {
        let (recovery, reg, lifecycle, _) = make_recovery();
        let id = spawn_agent_at_error(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);
        lifecycle
            .borrow_mut()
            .force_state(id, AgentState::Recovering)
            .unwrap();
        assert_eq!(reg.borrow().get(id).unwrap().restart_count, 0);
        let result = recovery.restart(id, 2000);
        assert!(result.is_ok());
        assert_eq!(reg.borrow().get(id).unwrap().restart_count, 1);
    }

    // restart(now=12345) → Ok, last_heartbeat == 12345
    #[test]
    fn test_restart_updates_last_heartbeat() {
        let (recovery, reg, lifecycle, _) = make_recovery();
        let id = spawn_agent_at_error(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);
        lifecycle
            .borrow_mut()
            .force_state(id, AgentState::Recovering)
            .unwrap();
        let result = recovery.restart(id, 12345);
        assert!(result.is_ok());
        assert_eq!(reg.borrow().get(id).unwrap().last_heartbeat, 12345);
    }

    // After restart, heartbeat.is_healthy(id) == true
    #[test]
    fn test_restart_re_registers_heartbeat() {
        let (recovery, reg, lifecycle, heartbeat) = make_recovery();
        let id = spawn_agent_at_error(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);
        assert!(!heartbeat.borrow().is_healthy(id));
        lifecycle
            .borrow_mut()
            .force_state(id, AgentState::Recovering)
            .unwrap();
        let result = recovery.restart(id, 2000);
        assert!(result.is_ok());
        assert!(heartbeat.borrow().is_healthy(id));
    }

    // save_checkpoint(id, &[1,2,3]) → Ok, then restore_checkpoint(id) → Ok(Some([1,2,3]))
    #[test]
    fn test_save_and_restore_checkpoint() {
        let (recovery, reg, lifecycle, _) = make_recovery();
        let id = spawn_agent_at_error(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);
        let save_result = recovery.save_checkpoint(id, &[1, 2, 3]);
        assert!(save_result.is_ok());
        let restore_result = recovery.restore_checkpoint(id);
        assert!(restore_result.is_ok());
        assert_eq!(restore_result.unwrap(), Some(vec![1u8, 2, 3]));
    }

    // restore_checkpoint on unsaved id → Ok(None)
    #[test]
    fn test_restore_checkpoint_nonexistent() {
        let (recovery, reg, lifecycle, _) = make_recovery();
        let id = spawn_agent_at_error(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);
        let result = recovery.restore_checkpoint(id);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None);
    }

    // with_defaults() constructs CrashRecovery with DEFAULT_MAX_RESTARTS=3
    #[test]
    fn test_with_defaults() {
        let (recovery, reg, lifecycle, _) = make_recovery();
        let id = spawn_agent_at_error(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);

        // 1st crash: restart_count 0 → 1
        assert!(recovery.handle_crash(id, 1100).is_ok());
        assert_eq!(
            lifecycle.borrow().current_state(id),
            Ok(AgentState::Running)
        );
        assert_eq!(reg.borrow().get(id).unwrap().restart_count, 1);
        lifecycle
            .borrow_mut()
            .force_state(id, AgentState::Error)
            .unwrap();

        // 2nd crash: restart_count 1 → 2
        assert!(recovery.handle_crash(id, 1200).is_ok());
        assert_eq!(reg.borrow().get(id).unwrap().restart_count, 2);
        lifecycle
            .borrow_mut()
            .force_state(id, AgentState::Error)
            .unwrap();

        // 3rd crash: restart_count 2 → 3
        assert!(recovery.handle_crash(id, 1300).is_ok());
        assert_eq!(reg.borrow().get(id).unwrap().restart_count, 3);
        lifecycle
            .borrow_mut()
            .force_state(id, AgentState::Error)
            .unwrap();

        // 4th crash: restart_count 3 >= 3 → Dead, MaxRestartsExceeded
        let result = recovery.handle_crash(id, 1400);
        assert_eq!(
            result,
            Err(AgentError::MaxRestartsExceeded {
                agent_id: id,
                count: 3
            })
        );
        assert_eq!(lifecycle.borrow().current_state(id), Ok(AgentState::Dead));
    }

    // make_recovery_with_max(1): first handle_crash succeeds, second fails with MaxRestartsExceeded
    #[test]
    fn test_custom_max_restarts() {
        let (recovery, reg, lifecycle, _) = make_recovery_with_max(1);
        let id = spawn_agent_at_error(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);

        // First crash: restart_count 0 → 1, Running
        assert!(recovery.handle_crash(id, 1100).is_ok());
        assert_eq!(
            lifecycle.borrow().current_state(id),
            Ok(AgentState::Running)
        );
        assert_eq!(reg.borrow().get(id).unwrap().restart_count, 1);

        // Second crash: force back to Error, then handle_crash fails
        lifecycle
            .borrow_mut()
            .force_state(id, AgentState::Error)
            .unwrap();
        let result = recovery.handle_crash(id, 1200);
        assert_eq!(
            result,
            Err(AgentError::MaxRestartsExceeded {
                agent_id: id,
                count: 1
            })
        );
        assert_eq!(lifecycle.borrow().current_state(id), Ok(AgentState::Dead));
    }

    // Spawn 2 agents at Error, handle_crash each succeeds, both Running, restart_counts independent
    #[test]
    fn test_handle_crash_multiple_agents_independent() {
        let (recovery, reg, lifecycle, _) = make_recovery();
        let id1 = spawn_agent_at_error(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);
        let id2 = spawn_agent_at_error(&reg, &lifecycle, AgentType::Energy, "agent-2", 1000);

        assert!(recovery.handle_crash(id1, 2000).is_ok());
        assert!(recovery.handle_crash(id2, 2000).is_ok());

        assert_eq!(
            lifecycle.borrow().current_state(id1),
            Ok(AgentState::Running)
        );
        assert_eq!(
            lifecycle.borrow().current_state(id2),
            Ok(AgentState::Running)
        );
        assert_eq!(reg.borrow().get(id1).unwrap().restart_count, 1);
        assert_eq!(reg.borrow().get(id2).unwrap().restart_count, 1);
    }

    // Pre-set restart_count=5, max_restarts=3, handle_crash → Err(MaxRestartsExceeded { count: 5 })
    #[test]
    fn test_handle_crash_returns_max_restarts_with_correct_count() {
        let (recovery, reg, lifecycle, _) = make_recovery_with_max(3);
        let id = spawn_agent_at_error(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);
        reg.borrow_mut().get_mut(id).unwrap().restart_count = 5;
        let result = recovery.handle_crash(id, 2000);
        assert_eq!(
            result,
            Err(AgentError::MaxRestartsExceeded {
                agent_id: id,
                count: 5
            })
        );
        assert_eq!(lifecycle.borrow().current_state(id), Ok(AgentState::Dead));
    }

    // CrashRecovery::new(..., 5) constructs with max_restarts=5 (5 successful restarts, 6th fails)
    #[test]
    fn test_new_constructor_sets_max_restarts() {
        let (recovery, reg, lifecycle, _) = make_recovery_with_max(5);
        let id = spawn_agent_at_error(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);

        // 5 successful restarts
        for i in 1..=5u32 {
            assert!(
                recovery.handle_crash(id, 1000 + i as u64 * 100).is_ok(),
                "restart {} should succeed",
                i
            );
            assert_eq!(reg.borrow().get(id).unwrap().restart_count, i);
            lifecycle
                .borrow_mut()
                .force_state(id, AgentState::Error)
                .unwrap();
        }

        // 6th crash: restart_count=5 >= 5 → Dead, MaxRestartsExceeded
        let result = recovery.handle_crash(id, 2000);
        assert_eq!(
            result,
            Err(AgentError::MaxRestartsExceeded {
                agent_id: id,
                count: 5
            })
        );
        assert_eq!(lifecycle.borrow().current_state(id), Ok(AgentState::Dead));
    }
}
