//! Integration tests for v0.41.0 SystemAgent.
//!
//! 验证 SystemAgent 的端到端行为：
//! - start/stop/suspend/resume 生命周期编排
//! - tick 心跳崩溃恢复（Unhealthy → AgentRecovered）
//! - tick OOM victim 挂起（最低优先级存活 Agent）
//! - tick 过热事件
//! - get_system_stats 统计（6 字段）
//! - find_oom_victim victim 选择
//! - 多次 tick 健康系统无事件
//! - start/stop 心跳注册/注销

use std::cell::RefCell;
use std::rc::Rc;
use std::string::String;

use eneros_agent::{
    AgentConfig, AgentContext, AgentEntry, AgentError, AgentFactory, AgentRegistry, AgentSpawner,
    AgentState, AgentType, CheckpointStore, CrashRecovery, HeartbeatMonitor,
    InMemoryCheckpointStore, LifecycleManager, SystemAgent, SystemConfig, SystemEvent,
};

// ---- Helper agent implementations ----

/// Stub AgentEntry: on_init/on_start return Ok, on_stop is no-op.
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

// ---- Helper functions ----

/// Create a minimal AgentConfig with the given name.
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

/// Create an AgentConfig with the given name and priority override.
fn make_config_with_priority(name: &str, priority: u8) -> AgentConfig {
    AgentConfig {
        agent_type: AgentType::Energy,
        name: String::from(name),
        binary_path: None,
        config_path: None,
        priority_override: Some(priority),
        mem_override: None,
    }
}

/// Construct a SystemAgent with all dependencies and shared handles.
///
/// Returns (SystemAgent, registry, lifecycle, heartbeat) so tests can inspect
/// shared state after calling SystemAgent methods.
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
    let checkpoint_store: Rc<dyn CheckpointStore> = Rc::new(InMemoryCheckpointStore::new());
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

// ---- Integration tests ----

/// 1. Start an agent, verify it's registered; stop it, verify it's unregistered.
#[test]
fn test_system_agent_start_and_stop_agent() {
    let (sa, reg, _lifecycle, _heartbeat) = make_system_agent();
    let config = make_config("agent-1");
    let id = sa
        .start_agent(config, 1000)
        .expect("start_agent should succeed");
    assert!(
        reg.borrow().get(id).is_some(),
        "agent should be registered after start"
    );

    sa.stop_agent(id).expect("stop_agent should succeed");
    assert!(
        reg.borrow().get(id).is_none(),
        "agent should be unregistered after stop"
    );
}

/// 2. Start an agent, suspend, verify Suspended; resume, verify Running.
#[test]
fn test_system_agent_suspend_and_resume() {
    let (sa, _reg, lifecycle, _heartbeat) = make_system_agent();
    let config = make_config("agent-1");
    let id = sa.start_agent(config, 1000).unwrap();
    assert_eq!(
        lifecycle.borrow().current_state(id),
        Ok(AgentState::Running)
    );

    sa.suspend_agent(id).expect("suspend should succeed");
    assert_eq!(
        lifecycle.borrow().current_state(id),
        Ok(AgentState::Suspended)
    );

    sa.resume_agent(id).expect("resume should succeed");
    assert_eq!(
        lifecycle.borrow().current_state(id),
        Ok(AgentState::Running)
    );
}

/// 3. Start an agent, advance time past heartbeat timeout (default 1000ms interval,
///    3 missed beats = Unhealthy), call tick, verify SystemEvent::AgentRecovered.
#[test]
fn test_system_agent_tick_heartbeat_crash_recovery() {
    let (mut sa, _reg, _lifecycle, _heartbeat) = make_system_agent();
    let config = make_config("agent-1");
    let id = sa.start_agent(config, 1000).unwrap();

    // Heartbeat registered at now=1000. Advance to now=4001:
    // elapsed = 4001 - 1000 = 3001 > 1000 (interval)
    // missed_count = 3001 / 1000 = 3 >= 3 (max_missed) → Unhealthy
    // tick: force_state(Error) + handle_crash → restart_count 0 < 3 → recovered
    let events = sa.tick(4001);

    assert!(
        events
            .iter()
            .any(|e| matches!(e, SystemEvent::AgentRecovered { agent } if *agent == id)),
        "expected AgentRecovered for agent {:?}, got: {:?}",
        id,
        events
    );
}

/// 4. Set up 2 agents with different priorities, set monitor to OOM condition
///    (mem 95/100, threshold 0.9), call tick, verify OomVictimSuspended for the
///    lower priority agent.
#[test]
fn test_system_agent_tick_oom_suspends_lowest_priority() {
    let (mut sa, reg, lifecycle, _heartbeat) = make_system_agent();

    // Two agents with different priorities (via priority_override).
    let _high = sa
        .start_agent(make_config_with_priority("high", 200), 1000)
        .unwrap();
    let low = sa
        .start_agent(make_config_with_priority("low", 100), 1000)
        .unwrap();

    // OOM condition: mem_used/mem_total = 95/100 = 0.95 > 0.9 threshold.
    sa.monitor.set_values(0.5, 95, 100, 50.0);

    // tick at now=1500: elapsed = 500 (not > 1000), so no heartbeat unhealthy.
    let events = sa.tick(1500);

    assert!(
        events
            .iter()
            .any(|e| matches!(e, SystemEvent::OomVictimSuspended { agent } if *agent == low)),
        "expected OomVictimSuspended for low-priority agent, got: {:?}",
        events
    );
    // Verify low is now Suspended.
    assert_eq!(
        lifecycle.borrow().current_state(low),
        Ok(AgentState::Suspended)
    );
    // high should still be Running.
    let high = reg.borrow().list_all()[0].agent_id;
    assert_eq!(
        lifecycle.borrow().current_state(high),
        Ok(AgentState::Running)
    );
}

/// 5. Set monitor temperature above threshold (85 > 80), call tick, verify Overheat.
#[test]
fn test_system_agent_tick_overheat_event() {
    let (mut sa, _reg, _lifecycle, _heartbeat) = make_system_agent();
    // 85.0 > 80.0 (default overheat_threshold).
    sa.monitor.set_values(0.5, 50, 100, 85.0);

    let events = sa.tick(1000);

    assert!(
        events
            .iter()
            .any(|e| matches!(e, SystemEvent::Overheat { temp: 85.0 })),
        "expected Overheat event with temp=85.0, got: {:?}",
        events
    );
}

/// 6. Start agents, set monitor values, call get_system_stats, verify all 6 fields.
#[test]
fn test_system_agent_get_system_stats() {
    let (mut sa, _reg, _lifecycle, _heartbeat) = make_system_agent();
    // Start one agent (Running → alive).
    sa.start_agent(make_config("agent-1"), 1000).unwrap();
    // Set monitor values.
    sa.monitor.set_values(0.5, 50, 100, 65.0);

    let stats = sa.get_system_stats();
    assert_eq!(stats.cpu_usage, 0.5);
    assert_eq!(stats.mem_usage, 0.5); // 50/100
    assert_eq!(stats.temperature, 65.0);
    assert_eq!(stats.agent_count, 1);
    assert_eq!(stats.alive_agents, 1); // Running is alive
    assert_eq!(stats.error_agents, 0);
}

/// 7. Start 3 agents with different priorities, call find_oom_victim, verify it
///    returns the lowest priority.
#[test]
fn test_system_agent_find_oom_victim() {
    let (sa, _reg, _lifecycle, _heartbeat) = make_system_agent();

    let _a1 = sa
        .start_agent(make_config_with_priority("a1", 200), 1000)
        .unwrap();
    let low = sa
        .start_agent(make_config_with_priority("low", 50), 1000)
        .unwrap();
    let _a3 = sa
        .start_agent(make_config_with_priority("a3", 255), 1000)
        .unwrap();

    let victim = sa.find_oom_victim();
    assert_eq!(
        victim,
        Some(low),
        "lowest priority (50) should be OOM victim"
    );
}

/// 8. Call tick multiple times with advancing now, verify no events when system is
///    healthy.
#[test]
fn test_system_agent_multiple_ticks() {
    let (mut sa, _reg, _lifecycle, _heartbeat) = make_system_agent();
    // Start an agent at now=1000 (heartbeat registered at 1000).
    sa.start_agent(make_config("agent-1"), 1000).unwrap();

    // Multiple ticks with advancing now, all within heartbeat interval.
    // elapsed = now - 1000; stays <= 1000 so agent remains Healthy.
    let events1 = sa.tick(1100); // elapsed = 100
    assert!(
        events1.is_empty(),
        "no events expected on healthy tick, got: {:?}",
        events1
    );

    let events2 = sa.tick(1300); // elapsed = 300
    assert!(
        events2.is_empty(),
        "no events expected on healthy tick, got: {:?}",
        events2
    );

    let events3 = sa.tick(1500); // elapsed = 500 (still <= 1000)
    assert!(
        events3.is_empty(),
        "no events expected on healthy tick, got: {:?}",
        events3
    );
}

/// 9. Start an agent, verify it's tracked by heartbeat monitor (advance time
///    slightly, no unhealthy reported).
#[test]
fn test_system_agent_start_registers_heartbeat() {
    let (mut sa, _reg, _lifecycle, heartbeat) = make_system_agent();
    let config = make_config("agent-1");
    let id = sa.start_agent(config, 1000).unwrap();

    // Agent should be tracked by heartbeat monitor as Healthy.
    assert!(
        heartbeat.borrow().is_healthy(id),
        "agent should be tracked by heartbeat monitor after start"
    );

    // Advance time slightly (elapsed = 500, not > 1000) — no unhealthy.
    let events = sa.tick(1500);
    assert!(
        !events.iter().any(|e| matches!(
            e,
            SystemEvent::AgentCrashed { agent }
            | SystemEvent::AgentRecovered { agent }
            | SystemEvent::AgentRecoveryFailed { agent }
            if *agent == id
        )),
        "no crash/recovery events expected for healthy agent, got: {:?}",
        events
    );
    // Still healthy after the tick.
    assert!(
        heartbeat.borrow().is_healthy(id),
        "agent should still be healthy after tick"
    );
}

/// 10. Start + stop an agent, verify heartbeat no longer tracks it (call tick after
///     stop, no AgentCrashed event for that id).
#[test]
fn test_system_agent_stop_unregisters() {
    let (mut sa, _reg, _lifecycle, heartbeat) = make_system_agent();
    let config = make_config("agent-1");
    let id = sa.start_agent(config, 1000).unwrap();

    // Verify heartbeat is tracking the agent.
    assert!(
        heartbeat.borrow().is_healthy(id),
        "agent should be tracked by heartbeat before stop"
    );

    // Stop the agent — this unregisters from heartbeat.
    sa.stop_agent(id).unwrap();
    assert!(
        !heartbeat.borrow().is_healthy(id),
        "agent should not be tracked by heartbeat after stop"
    );

    // Call tick at a time that WOULD trigger unhealthy if still tracked.
    // elapsed = 4001 - 1000 = 3001 > 1000, missed = 3 >= 3 → would be Unhealthy.
    let events = sa.tick(4001);
    assert!(
        !events.iter().any(|e| matches!(
            e,
            SystemEvent::AgentCrashed { agent }
            | SystemEvent::AgentRecovered { agent }
            | SystemEvent::AgentRecoveryFailed { agent }
            if *agent == id
        )),
        "no crash/recovery events expected for stopped agent, got: {:?}",
        events
    );
}
