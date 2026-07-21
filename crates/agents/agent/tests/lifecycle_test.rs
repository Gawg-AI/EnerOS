//! Integration tests for v0.35.0 LifecycleManager.
//!
//! 验证生命周期状态机的端到端行为：
//! - 完整生命周期路径
//! - 多 Agent 独立生命周期
//! - Hook 调用序列
//! - force_state 崩溃恢复场景
//! - Dead 不可逆
//! - 共享注册表多 Manager 可见性

use std::cell::RefCell;
use std::rc::Rc;
use std::vec::Vec;

use eneros_agent::{
    AgentDescriptor, AgentError, AgentId, AgentRegistry, AgentState, AgentType, LifecycleHook,
    LifecycleManager,
};

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

/// 创建注册表并注册一个处于指定状态的 Agent.
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

#[test]
fn integration_full_lifecycle() {
    // 完整路径: Created -> Ready -> Running -> Suspended -> Running
    //         -> Error -> Recovering -> Ready -> Running -> Dead
    let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Created);
    let mgr = LifecycleManager::new(reg);

    assert_eq!(mgr.transition(id, AgentState::Ready), Ok(AgentState::Ready));
    assert_eq!(
        mgr.transition(id, AgentState::Running),
        Ok(AgentState::Running)
    );
    assert_eq!(
        mgr.transition(id, AgentState::Suspended),
        Ok(AgentState::Suspended)
    );
    assert_eq!(
        mgr.transition(id, AgentState::Running),
        Ok(AgentState::Running)
    );
    assert_eq!(mgr.transition(id, AgentState::Error), Ok(AgentState::Error));
    assert_eq!(
        mgr.transition(id, AgentState::Recovering),
        Ok(AgentState::Recovering)
    );
    assert_eq!(mgr.transition(id, AgentState::Ready), Ok(AgentState::Ready));
    assert_eq!(
        mgr.transition(id, AgentState::Running),
        Ok(AgentState::Running)
    );
    assert_eq!(mgr.transition(id, AgentState::Dead), Ok(AgentState::Dead));

    // Final state should be Dead
    assert_eq!(mgr.current_state(id), Ok(AgentState::Dead));
}

#[test]
fn integration_multiple_agents_independent_lifecycles() {
    // 3 个 Agent 各自独立走不同生命周期路径
    let mut reg = AgentRegistry::new();

    // Agent 1: Created -> Ready -> Running -> Dead
    let mut d1 = AgentDescriptor::new(AgentType::Energy, "a1", 0);
    d1.state = AgentState::Created;
    let id1 = reg.register(d1).unwrap();

    // Agent 2: Created -> Ready -> Running -> Suspended -> Running
    let mut d2 = AgentDescriptor::new(AgentType::Market, "a2", 0);
    d2.state = AgentState::Created;
    let id2 = reg.register(d2).unwrap();

    // Agent 3: Created -> Ready -> Running -> Error -> Recovering -> Ready
    let mut d3 = AgentDescriptor::new(AgentType::Grid, "a3", 0);
    d3.state = AgentState::Created;
    let id3 = reg.register(d3).unwrap();

    let reg = Rc::new(RefCell::new(reg));
    let mgr = LifecycleManager::new(reg);

    // Agent 1: Created -> Ready -> Running -> Dead
    assert_eq!(
        mgr.transition(id1, AgentState::Ready),
        Ok(AgentState::Ready)
    );
    assert_eq!(
        mgr.transition(id1, AgentState::Running),
        Ok(AgentState::Running)
    );
    assert_eq!(mgr.transition(id1, AgentState::Dead), Ok(AgentState::Dead));

    // Agent 2: Created -> Ready -> Running -> Suspended -> Running
    assert_eq!(
        mgr.transition(id2, AgentState::Ready),
        Ok(AgentState::Ready)
    );
    assert_eq!(
        mgr.transition(id2, AgentState::Running),
        Ok(AgentState::Running)
    );
    assert_eq!(
        mgr.transition(id2, AgentState::Suspended),
        Ok(AgentState::Suspended)
    );
    assert_eq!(
        mgr.transition(id2, AgentState::Running),
        Ok(AgentState::Running)
    );

    // Agent 3: Created -> Ready -> Running -> Error -> Recovering -> Ready
    assert_eq!(
        mgr.transition(id3, AgentState::Ready),
        Ok(AgentState::Ready)
    );
    assert_eq!(
        mgr.transition(id3, AgentState::Running),
        Ok(AgentState::Running)
    );
    assert_eq!(
        mgr.transition(id3, AgentState::Error),
        Ok(AgentState::Error)
    );
    assert_eq!(
        mgr.transition(id3, AgentState::Recovering),
        Ok(AgentState::Recovering)
    );
    assert_eq!(
        mgr.transition(id3, AgentState::Ready),
        Ok(AgentState::Ready)
    );

    // Verify final states are independent
    assert_eq!(mgr.current_state(id1), Ok(AgentState::Dead));
    assert_eq!(mgr.current_state(id2), Ok(AgentState::Running));
    assert_eq!(mgr.current_state(id3), Ok(AgentState::Ready));
}

#[test]
fn integration_hook_recording() {
    // 注册 RecordingHook，执行多步转换，验证 hook 调用序列
    let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Created);
    let mut mgr = LifecycleManager::new(reg);
    let hook = RecordingHook::new();
    let events = hook.events.clone();
    mgr.add_hook(Box::new(hook));

    // Multi-step: Created -> Ready -> Running -> Suspended
    mgr.transition(id, AgentState::Ready).unwrap();
    mgr.transition(id, AgentState::Running).unwrap();
    mgr.transition(id, AgentState::Suspended).unwrap();

    // Verify hook call sequence
    // Created -> Ready: on_exit(Created), on_enter(Ready)
    // Ready -> Running: on_exit(Ready), on_enter(Running)
    // Running -> Suspended: on_exit(Running), on_enter(Suspended)
    let events = events.borrow();
    assert_eq!(events.len(), 6);
    assert_eq!(events[0], (AgentState::Created, id, false)); // on_exit Created
    assert_eq!(events[1], (AgentState::Ready, id, true)); // on_enter Ready
    assert_eq!(events[2], (AgentState::Ready, id, false)); // on_exit Ready
    assert_eq!(events[3], (AgentState::Running, id, true)); // on_enter Running
    assert_eq!(events[4], (AgentState::Running, id, false)); // on_exit Running
    assert_eq!(events[5], (AgentState::Suspended, id, true)); // on_enter Suspended
}

#[test]
fn integration_force_state_for_recovery() {
    // D2 偏差: force_state 模拟崩溃恢复 — Agent 进入 Error 后直接 force_state 到 Ready
    let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Error);
    let mut mgr = LifecycleManager::new(reg);

    // Verify agent is in Error
    assert_eq!(mgr.current_state(id), Ok(AgentState::Error));

    // force_state directly to Ready (bypasses Error -> Recovering -> Ready path)
    assert_eq!(mgr.force_state(id, AgentState::Ready), Ok(()));
    assert_eq!(mgr.current_state(id), Ok(AgentState::Ready));

    // Can now continue normal lifecycle: Ready -> Running
    assert_eq!(
        mgr.transition(id, AgentState::Running),
        Ok(AgentState::Running)
    );
}

#[test]
fn integration_dead_agent_rejected() {
    // Agent 进入 Dead 后所有后续转换均失败
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
            "Dead -> {:?} must be rejected",
            target
        );
    }
    // Agent should still be Dead
    assert_eq!(mgr.current_state(id), Ok(AgentState::Dead));
}

#[test]
fn integration_shared_registry_multiple_managers() {
    // 两个 LifecycleManager 共享同一 Rc<RefCell<AgentRegistry>>，
    // 一个 manager 的状态变更对另一个可见
    let (reg, id) = make_registry_with_agent(AgentType::Energy, AgentState::Created);
    let mgr1 = LifecycleManager::new(reg.clone());
    let mgr2 = LifecycleManager::new(reg);

    // mgr1 transitions agent to Ready
    assert_eq!(
        mgr1.transition(id, AgentState::Ready),
        Ok(AgentState::Ready)
    );

    // mgr2 can see the state change
    assert_eq!(mgr2.current_state(id), Ok(AgentState::Ready));

    // mgr2 transitions agent to Running
    assert_eq!(
        mgr2.transition(id, AgentState::Running),
        Ok(AgentState::Running)
    );

    // mgr1 can see the state change
    assert_eq!(mgr1.current_state(id), Ok(AgentState::Running));
}
