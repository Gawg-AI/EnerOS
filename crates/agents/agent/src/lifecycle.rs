//! Agent 生命周期状态机 — LifecycleManager / LifecycleHook / LifecycleEvent
//!
//! # 设计
//! - LifecycleManager 持有 `Rc<RefCell<AgentRegistry>>` 共享注册表引用（D1 单线程偏差）
//! - 转换合法性由 `transitions::TRANSITIONS` 表驱动
//! - Hook 在 RefCell::borrow_mut() 期间调用，hook 实现不得访问 registry（D5 偏差）
//! - `force_state` 直接设置状态，不触发 hooks、不验证转换表（D2 偏差）

pub mod transitions;
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

pub use transitions::{can_transition, TRANSITIONS};

use crate::{AgentError, AgentId, AgentRegistry, AgentState};

/// 生命周期 Hook trait（object-safe）.
///
/// 当 Agent 状态发生转换时，`on_exit` 在源状态退出前调用，
/// `on_enter` 在目标状态进入后调用。
///
/// **约束**：Hook 实现不得访问 registry（在 RefCell 借用期间调用，会 panic）。
pub trait LifecycleHook {
    /// Agent 进入新状态时调用.
    fn on_enter(&self, state: AgentState, id: AgentId);
    /// Agent 退出旧状态时调用.
    fn on_exit(&self, state: AgentState, id: AgentId);
}

/// 生命周期事件.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleEvent {
    /// 状态已变更
    StateChanged {
        from: AgentState,
        to: AgentState,
        agent_id: AgentId,
    },
    /// 转换被拒绝
    TransitionRejected {
        from: AgentState,
        to: AgentState,
        reason: String,
    },
}

/// Agent 生命周期管理器.
///
/// 持有共享注册表引用与 hook 列表，提供状态转换、查询与强制设置能力。
pub struct LifecycleManager {
    registry: Rc<RefCell<AgentRegistry>>,
    hooks: Vec<Box<dyn LifecycleHook>>,
}

impl LifecycleManager {
    /// 创建 LifecycleManager.
    pub fn new(registry: Rc<RefCell<AgentRegistry>>) -> Self {
        LifecycleManager {
            registry,
            hooks: Vec::new(),
        }
    }

    /// 添加生命周期 Hook（D3 偏差：蓝图未显式声明此方法但必需）.
    pub fn add_hook(&mut self, hook: Box<dyn LifecycleHook>) {
        self.hooks.push(hook);
    }

    /// 查询状态转换是否合法.
    pub fn can_transition(&self, from: AgentState, to: AgentState) -> bool {
        transitions::can_transition(from, to)
    }

    /// 执行状态转换.
    ///
    /// - 合法转换：更新 Agent state，触发 on_exit/on_enter hooks，返回 Ok(target)
    /// - 非法转换：返回 Err(InvalidStateTransition)，状态不变，不触发 hooks
    /// - Agent 不存在：返回 Err(AgentNotFound)
    pub fn transition(&self, id: AgentId, target: AgentState) -> Result<AgentState, AgentError> {
        let mut reg = self.registry.borrow_mut();
        let desc = reg.get_mut(id).ok_or(AgentError::AgentNotFound)?;
        let from = desc.state;
        if !self.can_transition(from, target) {
            return Err(AgentError::InvalidStateTransition { from, to: target });
        }
        for hook in &self.hooks {
            hook.on_exit(from, id);
        }
        desc.state = target;
        for hook in &self.hooks {
            hook.on_enter(target, id);
        }
        Ok(target)
    }

    /// 查询 Agent 当前状态.
    pub fn current_state(&self, id: AgentId) -> Result<AgentState, AgentError> {
        let reg = self.registry.borrow();
        reg.get(id)
            .map(|d| d.state)
            .ok_or(AgentError::AgentNotFound)
    }

    /// 强制设置状态（D2 偏差：不验证转换表，不触发 hooks）.
    pub fn force_state(&mut self, id: AgentId, state: AgentState) -> Result<(), AgentError> {
        let mut reg = self.registry.borrow_mut();
        let desc = reg.get_mut(id).ok_or(AgentError::AgentNotFound)?;
        desc.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use alloc::rc::Rc;
    use core::cell::RefCell;

    use super::*;
    use crate::{AgentDescriptor, AgentId, AgentRegistry, AgentType};

    /// 创建注册表并注册一个处于指定状态的 Agent，返回共享注册表与 Agent ID.
    fn make_registry_with_agent(
        agent_type: AgentType,
        state: AgentState,
    ) -> (Rc<RefCell<AgentRegistry>>, AgentId) {
        let mut reg = AgentRegistry::new();
        let mut desc = AgentDescriptor::new(agent_type, "test-agent", 0);
        desc.state = state;
        let id = reg.register(desc).unwrap();
        (Rc::new(RefCell::new(reg)), id)
    }

    /// 测试用 Hook，记录所有 on_enter/on_exit 调用.
    ///
    /// `events` 使用 `Rc<RefCell<...>>` 以便 hook 移入 `Box<dyn LifecycleHook>` 后
    /// 仍可从外部检查记录的事件。每条记录为 `(state, id, is_enter)`.
    struct RecordingHook {
        events: Rc<RefCell<Vec<(AgentState, AgentId, bool)>>>,
    }

    impl RecordingHook {
        fn new() -> Self {
            RecordingHook {
                events: Rc::new(RefCell::new(Vec::new())),
            }
        }
    }

    impl LifecycleHook for RecordingHook {
        fn on_enter(&self, state: AgentState, id: AgentId) {
            self.events.borrow_mut().push((state, id, true));
        }
        fn on_exit(&self, state: AgentState, id: AgentId) {
            self.events.borrow_mut().push((state, id, false));
        }
    }

    // ---- 12 legal transition tests ----

    #[test]
    fn test_transition_legal_created_to_ready() {
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Created);
        let mgr = LifecycleManager::new(reg);
        assert_eq!(mgr.transition(id, AgentState::Ready), Ok(AgentState::Ready));
    }

    #[test]
    fn test_transition_legal_ready_to_running() {
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Ready);
        let mgr = LifecycleManager::new(reg);
        assert_eq!(
            mgr.transition(id, AgentState::Running),
            Ok(AgentState::Running)
        );
    }

    #[test]
    fn test_transition_legal_running_to_suspended() {
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Running);
        let mgr = LifecycleManager::new(reg);
        assert_eq!(
            mgr.transition(id, AgentState::Suspended),
            Ok(AgentState::Suspended)
        );
    }

    #[test]
    fn test_transition_legal_running_to_error() {
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Running);
        let mgr = LifecycleManager::new(reg);
        assert_eq!(mgr.transition(id, AgentState::Error), Ok(AgentState::Error));
    }

    #[test]
    fn test_transition_legal_suspended_to_running() {
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Suspended);
        let mgr = LifecycleManager::new(reg);
        assert_eq!(
            mgr.transition(id, AgentState::Running),
            Ok(AgentState::Running)
        );
    }

    #[test]
    fn test_transition_legal_suspended_to_error() {
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Suspended);
        let mgr = LifecycleManager::new(reg);
        assert_eq!(mgr.transition(id, AgentState::Error), Ok(AgentState::Error));
    }

    #[test]
    fn test_transition_legal_error_to_recovering() {
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Error);
        let mgr = LifecycleManager::new(reg);
        assert_eq!(
            mgr.transition(id, AgentState::Recovering),
            Ok(AgentState::Recovering)
        );
    }

    #[test]
    fn test_transition_legal_recovering_to_ready() {
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Recovering);
        let mgr = LifecycleManager::new(reg);
        assert_eq!(mgr.transition(id, AgentState::Ready), Ok(AgentState::Ready));
    }

    #[test]
    fn test_transition_legal_recovering_to_dead() {
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Recovering);
        let mgr = LifecycleManager::new(reg);
        assert_eq!(mgr.transition(id, AgentState::Dead), Ok(AgentState::Dead));
    }

    #[test]
    fn test_transition_legal_error_to_dead() {
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Error);
        let mgr = LifecycleManager::new(reg);
        assert_eq!(mgr.transition(id, AgentState::Dead), Ok(AgentState::Dead));
    }

    #[test]
    fn test_transition_legal_running_to_dead() {
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Running);
        let mgr = LifecycleManager::new(reg);
        assert_eq!(mgr.transition(id, AgentState::Dead), Ok(AgentState::Dead));
    }

    #[test]
    fn test_transition_legal_ready_to_dead() {
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Ready);
        let mgr = LifecycleManager::new(reg);
        assert_eq!(mgr.transition(id, AgentState::Dead), Ok(AgentState::Dead));
    }

    // ---- illegal transition tests ----

    #[test]
    fn test_transition_illegal_created_to_running() {
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Created);
        let mgr = LifecycleManager::new(reg);
        assert_eq!(
            mgr.transition(id, AgentState::Running),
            Err(AgentError::InvalidStateTransition {
                from: AgentState::Created,
                to: AgentState::Running
            })
        );
    }

    #[test]
    fn test_transition_illegal_error_to_running() {
        // 蓝图 §8.5: Error -> Running 非法（必须经过 Recovering -> Ready -> Running）
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Error);
        let mgr = LifecycleManager::new(reg);
        assert_eq!(
            mgr.transition(id, AgentState::Running),
            Err(AgentError::InvalidStateTransition {
                from: AgentState::Error,
                to: AgentState::Running
            })
        );
    }

    #[test]
    fn test_transition_illegal_dead_to_ready() {
        // 蓝图 §8.1: Dead 不可逆
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Dead);
        let mgr = LifecycleManager::new(reg);
        assert_eq!(
            mgr.transition(id, AgentState::Ready),
            Err(AgentError::InvalidStateTransition {
                from: AgentState::Dead,
                to: AgentState::Ready
            })
        );
    }

    #[test]
    fn test_transition_illegal_self_transition() {
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Running);
        let mgr = LifecycleManager::new(reg);
        assert_eq!(
            mgr.transition(id, AgentState::Running),
            Err(AgentError::InvalidStateTransition {
                from: AgentState::Running,
                to: AgentState::Running
            })
        );
    }

    // ---- error path tests ----

    #[test]
    fn test_transition_nonexistent_agent() {
        let (reg, _id) = make_registry_with_agent(AgentType::Energy, AgentState::Created);
        let mgr = LifecycleManager::new(reg);
        let fake_id = AgentId::generate();
        assert_eq!(
            mgr.transition(fake_id, AgentState::Ready),
            Err(AgentError::AgentNotFound)
        );
    }

    #[test]
    fn test_current_state_existing() {
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Running);
        let mgr = LifecycleManager::new(reg);
        assert_eq!(mgr.current_state(id), Ok(AgentState::Running));
    }

    #[test]
    fn test_current_state_nonexistent() {
        let (reg, _id) = make_registry_with_agent(AgentType::Energy, AgentState::Created);
        let mgr = LifecycleManager::new(reg);
        let fake_id = AgentId::generate();
        assert_eq!(mgr.current_state(fake_id), Err(AgentError::AgentNotFound));
    }

    // ---- force_state tests ----

    #[test]
    fn test_force_state_bypasses_table() {
        // D2: force_state 直接设置状态，绕过转换表
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Created);
        let mut mgr = LifecycleManager::new(reg);
        // Created -> Running normally illegal, but force_state bypasses
        assert_eq!(mgr.force_state(id, AgentState::Running), Ok(()));
        assert_eq!(mgr.current_state(id), Ok(AgentState::Running));
    }

    #[test]
    fn test_force_state_nonexistent() {
        let (reg, _id) = make_registry_with_agent(AgentType::Energy, AgentState::Created);
        let mut mgr = LifecycleManager::new(reg);
        let fake_id = AgentId::generate();
        assert_eq!(
            mgr.force_state(fake_id, AgentState::Running),
            Err(AgentError::AgentNotFound)
        );
    }

    #[test]
    fn test_force_state_no_hooks() {
        // D2 偏差: force_state 不触发 hooks
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Created);
        let mut mgr = LifecycleManager::new(reg);
        let hook = RecordingHook::new();
        let events = hook.events.clone();
        mgr.add_hook(Box::new(hook));
        // force_state should not trigger hooks
        assert_eq!(mgr.force_state(id, AgentState::Running), Ok(()));
        assert_eq!(mgr.current_state(id), Ok(AgentState::Running));
        // Hook events list should be empty
        assert_eq!(events.borrow().len(), 0);
    }

    // ---- hook tests ----

    #[test]
    fn test_hook_on_exit_before_on_enter() {
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Created);
        let mut mgr = LifecycleManager::new(reg);
        let hook = RecordingHook::new();
        let events = hook.events.clone();
        mgr.add_hook(Box::new(hook));

        assert_eq!(mgr.transition(id, AgentState::Ready), Ok(AgentState::Ready));

        let events = events.borrow();
        assert_eq!(events.len(), 2);
        // on_exit(Created) called first
        assert_eq!(events[0], (AgentState::Created, id, false));
        // on_enter(Ready) called second
        assert_eq!(events[1], (AgentState::Ready, id, true));
    }

    #[test]
    fn test_hook_receives_correct_states() {
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Created);
        let mut mgr = LifecycleManager::new(reg);
        let hook = RecordingHook::new();
        let events = hook.events.clone();
        mgr.add_hook(Box::new(hook));

        // Multi-step: Created -> Ready -> Running
        mgr.transition(id, AgentState::Ready).unwrap();
        mgr.transition(id, AgentState::Running).unwrap();

        let events = events.borrow();
        assert_eq!(events.len(), 4);
        // Created -> Ready: on_exit(Created), on_enter(Ready)
        assert_eq!(events[0], (AgentState::Created, id, false));
        assert_eq!(events[1], (AgentState::Ready, id, true));
        // Ready -> Running: on_exit(Ready), on_enter(Running)
        assert_eq!(events[2], (AgentState::Ready, id, false));
        assert_eq!(events[3], (AgentState::Running, id, true));
    }

    #[test]
    fn test_add_hook() {
        // add_hook 后 hooks 列表增长；通过 transition 触发 hook 验证生效
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Created);
        let mut mgr = LifecycleManager::new(reg);
        let hook = RecordingHook::new();
        let events = hook.events.clone();
        mgr.add_hook(Box::new(hook));
        // Trigger a transition to verify hook fires
        assert_eq!(mgr.transition(id, AgentState::Ready), Ok(AgentState::Ready));
        // Should have 2 events: on_exit(Created), on_enter(Ready)
        assert_eq!(events.borrow().len(), 2);
    }

    // ---- Dead irreversibility ----

    #[test]
    fn test_dead_irreversible_all_states() {
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Dead);
        let mgr = LifecycleManager::new(reg);
        let non_dead_states = [
            AgentState::Created,
            AgentState::Ready,
            AgentState::Running,
            AgentState::Suspended,
            AgentState::Error,
            AgentState::Recovering,
        ];
        for target in non_dead_states {
            assert_eq!(
                mgr.transition(id, target),
                Err(AgentError::InvalidStateTransition {
                    from: AgentState::Dead,
                    to: target
                }),
                "Dead -> {:?} must fail (Dead is irreversible)",
                target
            );
        }
        // Agent should still be Dead
        assert_eq!(mgr.current_state(id), Ok(AgentState::Dead));
    }

    // ---- End-to-end lifecycle ----

    #[test]
    fn test_full_lifecycle_path() {
        // 完整路径: Created -> Ready -> Running -> Suspended -> Running
        //         -> Error -> Recovering -> Ready -> Running -> Dead
        let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Created);
        let mgr = LifecycleManager::new(reg);
        let path = [
            AgentState::Ready,
            AgentState::Running,
            AgentState::Suspended,
            AgentState::Running,
            AgentState::Error,
            AgentState::Recovering,
            AgentState::Ready,
            AgentState::Running,
            AgentState::Dead,
        ];
        for target in path {
            assert!(
                mgr.transition(id, target).is_ok(),
                "transition to {:?} should succeed in full lifecycle path",
                target
            );
        }
        assert_eq!(mgr.current_state(id), Ok(AgentState::Dead));
    }

    // ---- LifecycleEvent PartialEq ----

    #[test]
    fn test_lifecycle_event_eq() {
        let id = AgentId::generate();
        let e1 = LifecycleEvent::StateChanged {
            from: AgentState::Created,
            to: AgentState::Ready,
            agent_id: id,
        };
        let e2 = LifecycleEvent::StateChanged {
            from: AgentState::Created,
            to: AgentState::Ready,
            agent_id: id,
        };
        assert_eq!(e1, e2);

        let e3 = LifecycleEvent::StateChanged {
            from: AgentState::Created,
            to: AgentState::Running,
            agent_id: id,
        };
        assert_ne!(e1, e3);

        // TransitionRejected with same data should be equal
        let r1 = LifecycleEvent::TransitionRejected {
            from: AgentState::Created,
            to: AgentState::Running,
            reason: String::from("illegal"),
        };
        let r2 = LifecycleEvent::TransitionRejected {
            from: AgentState::Created,
            to: AgentState::Running,
            reason: String::from("illegal"),
        };
        assert_eq!(r1, r2);
    }
}
