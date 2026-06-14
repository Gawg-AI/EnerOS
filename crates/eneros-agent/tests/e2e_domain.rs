use std::sync::Arc;
use eneros_agent::agents::dispatch_agent::*;
use eneros_agent::agents::operation_agent::*;
use eneros_agent::agents::self_healing_agent::*;
use eneros_agent::emergency::EmergencyResponsePipeline;
use eneros_agent::audit::AuditTrail;
use eneros_agent::context::AgentContext;
use eneros_agent::agent::Agent;
use eneros_core::{AuthorityLevel, BusType, BranchType, SystemOperatingState, ActionVerdict};
use eneros_eventbus::{EventBus, Event, event::{EventType, EventPayload}};
use eneros_gateway::SafetyGateway;
use eneros_reasoning::RuleBasedEngine;
use eneros_topology::NetworkGraph;

#[test]
fn test_e2e_scenario_1_economic_dispatch() {
    // Scenario: Load growth -> DispatchAgent economic dispatch -> constraint check -> command
    let generators = vec![
        GeneratorCostCurve { gen_id: "G1".to_string(), a: 0.001, b: 10.0, c: 100.0, p_min_mw: 20.0, p_max_mw: 200.0 },
        GeneratorCostCurve { gen_id: "G2".to_string(), a: 0.002, b: 12.0, c: 80.0, p_min_mw: 10.0, p_max_mw: 150.0 },
    ];

    let result = economic_dispatch(&generators, 250.0);
    assert!(result.total_generation_mw > 0.0);
    assert!(result.total_cost > 0.0);

    // Verify dispatch respects limits
    for (gen_id, p) in &result.gen_outputs {
        let gen = generators.iter().find(|g| &g.gen_id == gen_id).unwrap();
        assert!(*p >= gen.p_min_mw);
        assert!(*p <= gen.p_max_mw);
    }
}

#[test]
fn test_e2e_scenario_2_fault_diagnosis() {
    // Scenario: Device alarm -> OperationAgent fault diagnosis -> recommendation
    let mut agent = OperationAgent::new("op1", "Op-1", vec![1]);
    agent.update_device_health(1, "变压器T1", 0.5, vec!["温度偏高".to_string()]);

    let symptoms = vec!["high_temperature".to_string(), "overload".to_string()];
    let diagnoses = agent.diagnose(&symptoms);
    assert!(!diagnoses.is_empty());
    assert!(diagnoses[0].fault_type.contains("transformer") || diagnoses[0].fault_type.contains("overheat"));
}

#[test]
fn test_e2e_scenario_3_self_healing() {
    // Scenario: Feeder fault -> SelfHealingAgent isolation + restoration -> notify dispatch
    let mut agent = SelfHealingAgent::new("sh1", "SelfHeal-1", vec![1]);

    // Build a simple topology for the test
    let mut topology = NetworkGraph::new();
    topology.initialize(
        vec![
            eneros_topology::Bus {
                id: 1, name: "Source".to_string(), bus_type: BusType::Slack,
                voltage_kv: 110.0, zone_id: 0, bus_type_pf: BusType::Slack,
                p_gen: 0.0, q_gen: 0.0, p_load: 0.0, q_load: 0.0, v_pu: 1.0,
            },
            eneros_topology::Bus {
                id: 5, name: "FaultBus".to_string(), bus_type: BusType::PQ,
                voltage_kv: 110.0, zone_id: 0, bus_type_pf: BusType::PQ,
                p_gen: 0.0, q_gen: 0.0, p_load: 0.0, q_load: 0.0, v_pu: 1.0,
            },
        ],
        vec![eneros_topology::Branch {
            id: 1, name: "Line1-5".to_string(), from_bus: 1, to_bus: 5,
            branch_type: BranchType::Line, status: true, r: 0.01, x: 0.1, b: 0.01, tap_ratio: 1.0,
        }],
        vec![],
    ).unwrap();

    let result = agent.heal_fault(5, &topology).unwrap();

    assert!(result.success);
    assert_eq!(result.isolation_sequence.len(), 2); // Open upstream + downstream
    assert!(!result.restoration_sequence.is_empty());

    // Convert to actions
    let actions = SelfHealingAgent::operations_to_actions(&result.isolation_sequence);
    assert_eq!(actions.len(), 2);
}

#[test]
fn test_e2e_scenario_4_emergency_response() {
    // Scenario: Frequency collapse -> emergency response auto-execute -> audit trail
    let pipeline = EmergencyResponsePipeline::new();
    let results = pipeline.auto_respond(49.0, 0, 1.0, 0, SystemOperatingState::Emergency);

    assert!(!results.is_empty());

    // Verify actions can be mapped to AgentActions
    let plan = &pipeline.plans()[0];
    let actions = pipeline.execute_with_mapper(plan);
    assert!(!actions.is_empty());

    // Record in audit trail
    let audit = AuditTrail::new();
    audit.record(eneros_core::AuditEntry {
        entry_id: 0,
        agent_id: "emergency".to_string(),
        authority_level: AuthorityLevel::Emergency,
        action_description: "频率崩溃紧急响应".to_string(),
        constraint_check_result: "bypassed_non_critical".to_string(),
        approval_chain: vec![],
        timestamp: chrono::Utc::now(),
        reasoning_summary: "频率49.0Hz < 49.5Hz阈值".to_string(),
        system_state: SystemOperatingState::Emergency,
        verdict: ActionVerdict::EmergencyBypassed {
            bypassed_checks: vec!["approval_flow".to_string()],
            reason: "频率崩溃".to_string(),
        },
    });
    assert_eq!(audit.len(), 1);
    assert!(audit.verify_integrity());
}

/// Helper: create a minimal AgentContext with RuleBasedEngine for testing
fn make_test_ctx() -> AgentContext {
    let event_bus = Arc::new(EventBus::new(100));
    let gateway = Arc::new(SafetyGateway::new(100));
    let tool_engine = Arc::new(parking_lot::RwLock::new(eneros_tool::ToolEngine::new()));
    let network_rw = Arc::new(parking_lot::RwLock::new(eneros_network::PowerNetwork::from_ieee14()));
    let memory = Arc::new(eneros_memory::InMemoryMemory::default());
    let reasoning = Arc::new(RuleBasedEngine::new());

    AgentContext::new(
        event_bus,
        gateway,
        tool_engine,
        network_rw,
        memory,
        reasoning,
    )
}

#[tokio::test]
async fn test_operation_agent_reasoning_diagnosis() {
    // Scenario: Unknown symptoms (no hardcoded pattern match) -> reasoning engine called
    let mut agent = OperationAgent::new("op1", "Op-1", vec![1]);
    let ctx = make_test_ctx();

    // Create a constraint violation event with symptoms that don't match any hardcoded pattern
    let event = Event::new(
        EventType::ConstraintViolation,
        "test",
        EventPayload::Message("voltage_drop frequency_deviation unknown_alarm".to_string()),
    );

    let actions = agent.handle_event(&event, &ctx).await.unwrap();

    // Should have at least one action (hardcoded diagnosis or reasoning result)
    assert!(!actions.is_empty());

    // At least one action should be a LogMessage (from either hardcoded or reasoning path)
    let has_log = actions.iter().any(|a| matches!(a, eneros_agent::agent::AgentAction::LogMessage(_)));
    assert!(has_log);
}

#[tokio::test]
async fn test_operation_agent_hardcoded_match_skips_reasoning() {
    // Scenario: Known symptoms (high confidence hardcoded match) -> reasoning engine NOT called
    let mut agent = OperationAgent::new("op1", "Op-1", vec![1]);
    let ctx = make_test_ctx();

    // Create an event with symptoms matching the transformer_overheating pattern
    let event = Event::new(
        EventType::SystemAlarm,
        "test",
        EventPayload::Message("high_temperature overload".to_string()),
    );

    let actions = agent.handle_event(&event, &ctx).await.unwrap();

    // Should have hardcoded diagnosis actions
    assert!(!actions.is_empty());
    let log_messages: Vec<&str> = actions.iter().filter_map(|a| {
        if let eneros_agent::agent::AgentAction::LogMessage(msg) = a { Some(msg.as_str()) } else { None }
    }).collect();
    // Should contain the hardcoded transformer_overheating diagnosis
    assert!(log_messages.iter().any(|m| m.contains("transformer_overheating")));
}

#[tokio::test]
async fn test_dispatch_agent_reasoning_review() {
    // Scenario: ConstraintViolation -> economic dispatch + reasoning review
    let mut agent = DispatchAgent::new("d1", "Dispatch-1", vec![1])
        .with_generators(vec![
            GeneratorCostCurve { gen_id: "1".to_string(), a: 0.001, b: 10.0, c: 100.0, p_min_mw: 20.0, p_max_mw: 200.0 },
            GeneratorCostCurve { gen_id: "2".to_string(), a: 0.002, b: 12.0, c: 80.0, p_min_mw: 10.0, p_max_mw: 150.0 },
        ]);
    let ctx = make_test_ctx();

    let event = Event::new(
        EventType::ConstraintViolation,
        "test",
        EventPayload::Message("Branch overload detected".to_string()),
    );

    let actions = agent.handle_event(&event, &ctx).await.unwrap();

    // Should have dispatch commands + reasoning review
    assert!(!actions.is_empty());

    // Should have at least one ExecuteCommand (dispatch)
    let has_command = actions.iter().any(|a| matches!(a, eneros_agent::agent::AgentAction::ExecuteCommand(_)));
    assert!(has_command);

    // Should have reasoning review log (contains "dispatch review" from review_dispatch_with_reasoning)
    let has_review = actions.iter().any(|a| {
        if let eneros_agent::agent::AgentAction::LogMessage(msg) = a {
            msg.contains("dispatch review")
        } else {
            false
        }
    });
    assert!(has_review, "Expected dispatch review log, got actions: {:?}", actions);
}

#[tokio::test]
async fn test_agent_reasoning_fallback() {
    // Scenario: Reasoning engine returns error -> agent still produces hardcoded results
    let mut agent = OperationAgent::new("op1", "Op-1", vec![1]);
    let ctx = make_test_ctx();

    // Use symptoms that partially match (low confidence) to trigger reasoning path
    let event = Event::new(
        EventType::SystemAlarm,
        "test",
        EventPayload::Message("unbalance".to_string()), // Partial match with CAP_BANK_FAULT
    );

    let actions = agent.handle_event(&event, &ctx).await.unwrap();

    // Should still produce actions even if reasoning engine is just RuleBasedEngine
    assert!(!actions.is_empty());
}
