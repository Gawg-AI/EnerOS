//! Integration tests for EnerOS v0.38.0 Agent Crash Recovery.
//!
//! These tests exercise the public API of `CrashRecovery` end-to-end,
//! including state transitions, heartbeat re-registration, checkpoint
//! persistence, and max-restarts exhaustion.

extern crate alloc;

use alloc::rc::Rc;
use core::cell::RefCell;

use eneros_agent::{
    AgentDescriptor, AgentError, AgentId, AgentRegistry, AgentState, AgentType, CheckpointStore,
    CrashRecovery, HeartbeatMonitor, InMemoryCheckpointStore, LifecycleManager,
};

// ---- Helper functions ----

/// Construct a full CrashRecovery environment with default max_restarts=3.
///
/// Returns `(recovery, registry, lifecycle, heartbeat)`.
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

/// Spawn an agent and force it into Error state (simulating a crash).
///
/// 1. Creates `AgentDescriptor::new(agent_type, name, now)`
/// 2. Registers it in `reg`
/// 3. Force-transitions to `AgentState::Error`
/// 4. Returns the `AgentId`
fn spawn_and_crash(
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

// ---- Integration tests ----

// 1. Full crash recovery lifecycle: Error → handle_crash → Running, restart_count=1.
#[test]
fn integration_crash_recovery_full_lifecycle() {
    let (recovery, reg, lifecycle, _) = make_recovery();
    let id = spawn_and_crash(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);

    // Agent starts at Error with restart_count=0.
    assert_eq!(reg.borrow().get(id).unwrap().restart_count, 0);

    // handle_crash recovers the agent.
    let result = recovery.handle_crash(id, 2000);
    assert!(result.is_ok());

    // State should be Running after recovery.
    assert_eq!(
        lifecycle.borrow().current_state(id),
        Ok(AgentState::Running)
    );

    // restart_count should be incremented to 1.
    assert_eq!(reg.borrow().get(id).unwrap().restart_count, 1);
}

// 2. Crash recovery with checkpoint: save before crash, restore after recovery.
#[test]
fn integration_crash_recovery_with_checkpoint() {
    let (recovery, reg, lifecycle, _) = make_recovery();

    // Spawn agent in Created state (not yet crashed).
    let desc = AgentDescriptor::new(AgentType::Energy, "agent-1", 1000);
    let id = reg.borrow_mut().register(desc).unwrap();

    // Save checkpoint before crash.
    let save_result = recovery.save_checkpoint(id, &[10, 20, 30]);
    assert!(save_result.is_ok());

    // Crash: force to Error.
    lifecycle
        .borrow_mut()
        .force_state(id, AgentState::Error)
        .unwrap();

    // handle_crash recovers the agent.
    let result = recovery.handle_crash(id, 2000);
    assert!(result.is_ok());

    // restore_checkpoint returns the previously saved data.
    let restore_result = recovery.restore_checkpoint(id);
    assert!(restore_result.is_ok());
    assert_eq!(restore_result.unwrap(), Some(vec![10u8, 20, 30]));
}

// 3. Crash recovery with no checkpoint: restore returns Ok(None).
#[test]
fn integration_crash_recovery_no_checkpoint() {
    let (recovery, reg, lifecycle, _) = make_recovery();
    let id = spawn_and_crash(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);

    // handle_crash recovers the agent.
    let result = recovery.handle_crash(id, 2000);
    assert!(result.is_ok());

    // No checkpoint was saved, restore returns Ok(None).
    let restore_result = recovery.restore_checkpoint(id);
    assert!(restore_result.is_ok());
    assert_eq!(restore_result.unwrap(), None);
}

// 4. Max restarts exhaustion: 3 successful restarts, 4th crash → Dead.
#[test]
fn integration_max_restarts_exceeds_to_dead() {
    let (recovery, reg, lifecycle, _) = make_recovery();
    let id = spawn_and_crash(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);

    // 1st crash: restart_count 0 → 1, Running.
    assert!(recovery.handle_crash(id, 1100).is_ok());
    assert_eq!(reg.borrow().get(id).unwrap().restart_count, 1);
    lifecycle
        .borrow_mut()
        .force_state(id, AgentState::Error)
        .unwrap();

    // 2nd crash: restart_count 1 → 2, Running.
    assert!(recovery.handle_crash(id, 1200).is_ok());
    assert_eq!(reg.borrow().get(id).unwrap().restart_count, 2);
    lifecycle
        .borrow_mut()
        .force_state(id, AgentState::Error)
        .unwrap();

    // 3rd crash: restart_count 2 → 3, Running.
    assert!(recovery.handle_crash(id, 1300).is_ok());
    assert_eq!(reg.borrow().get(id).unwrap().restart_count, 3);
    lifecycle
        .borrow_mut()
        .force_state(id, AgentState::Error)
        .unwrap();

    // 4th crash: restart_count 3 >= 3 (max_restarts) → Dead, MaxRestartsExceeded.
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

// 5. Recovery re-registers heartbeat: not healthy before, healthy after.
#[test]
fn integration_recovery_re_registers_heartbeat() {
    let (recovery, reg, lifecycle, heartbeat) = make_recovery();
    let id = spawn_and_crash(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);

    // Agent not registered with heartbeat monitor → not healthy.
    assert!(!heartbeat.borrow().is_healthy(id));

    // handle_crash recovers and re-registers heartbeat.
    let result = recovery.handle_crash(id, 2000);
    assert!(result.is_ok());

    // After recovery, agent is registered with heartbeat → healthy.
    assert!(heartbeat.borrow().is_healthy(id));
}

// 6. Multiple agents recover independently: both Running, restart_counts=1 each.
#[test]
fn integration_multiple_agents_independent_recovery() {
    let (recovery, reg, lifecycle, _) = make_recovery();
    let id1 = spawn_and_crash(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);
    let id2 = spawn_and_crash(&reg, &lifecycle, AgentType::Energy, "agent-2", 1000);

    // Both agents recover independently.
    assert!(recovery.handle_crash(id1, 2000).is_ok());
    assert!(recovery.handle_crash(id2, 2000).is_ok());

    // Both are Running.
    assert_eq!(
        lifecycle.borrow().current_state(id1),
        Ok(AgentState::Running)
    );
    assert_eq!(
        lifecycle.borrow().current_state(id2),
        Ok(AgentState::Running)
    );

    // Both have restart_count=1, independent of each other.
    assert_eq!(reg.borrow().get(id1).unwrap().restart_count, 1);
    assert_eq!(reg.borrow().get(id2).unwrap().restart_count, 1);
}

// 7. Custom max_restarts=2: 2 successful restarts, 3rd crash → Dead.
#[test]
fn integration_custom_max_restarts() {
    // Construct CrashRecovery with custom max_restarts=2.
    let reg = Rc::new(RefCell::new(AgentRegistry::new()));
    let heartbeat = Rc::new(RefCell::new(HeartbeatMonitor::with_defaults()));
    let lifecycle = Rc::new(RefCell::new(LifecycleManager::new(reg.clone())));
    let checkpoint_store: Rc<dyn CheckpointStore> = Rc::new(InMemoryCheckpointStore::new());
    let recovery = CrashRecovery::new(
        reg.clone(),
        heartbeat.clone(),
        lifecycle.clone(),
        checkpoint_store,
        2,
    );

    let id = spawn_and_crash(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);

    // 1st crash: restart_count 0 → 1, Running.
    assert!(recovery.handle_crash(id, 1100).is_ok());
    assert_eq!(reg.borrow().get(id).unwrap().restart_count, 1);
    lifecycle
        .borrow_mut()
        .force_state(id, AgentState::Error)
        .unwrap();

    // 2nd crash: restart_count 1 → 2, Running.
    assert!(recovery.handle_crash(id, 1200).is_ok());
    assert_eq!(reg.borrow().get(id).unwrap().restart_count, 2);
    lifecycle
        .borrow_mut()
        .force_state(id, AgentState::Error)
        .unwrap();

    // 3rd crash: restart_count 2 >= 2 (max_restarts) → Dead, MaxRestartsExceeded.
    let result = recovery.handle_crash(id, 1300);
    assert_eq!(
        result,
        Err(AgentError::MaxRestartsExceeded {
            agent_id: id,
            count: 2
        })
    );
    assert_eq!(lifecycle.borrow().current_state(id), Ok(AgentState::Dead));
}

// 8. CheckpointStore trait object: save/restore works through Rc<dyn CheckpointStore>.
#[test]
fn integration_checkpoint_store_trait_object() {
    let reg = Rc::new(RefCell::new(AgentRegistry::new()));
    let heartbeat = Rc::new(RefCell::new(HeartbeatMonitor::with_defaults()));
    let lifecycle = Rc::new(RefCell::new(LifecycleManager::new(reg.clone())));

    // Explicitly create Rc<dyn CheckpointStore> trait object.
    let checkpoint_store: Rc<dyn CheckpointStore> = Rc::new(InMemoryCheckpointStore::new());
    let recovery = CrashRecovery::with_defaults(
        reg.clone(),
        heartbeat.clone(),
        lifecycle.clone(),
        checkpoint_store,
    );

    let id = spawn_and_crash(&reg, &lifecycle, AgentType::Energy, "agent-1", 1000);

    // save_checkpoint through the trait object.
    let save_result = recovery.save_checkpoint(id, &[10, 20, 30]);
    assert!(save_result.is_ok());

    // restore_checkpoint through the trait object.
    let restore_result = recovery.restore_checkpoint(id);
    assert!(restore_result.is_ok());
    assert_eq!(restore_result.unwrap(), Some(vec![10u8, 20, 30]));
}
