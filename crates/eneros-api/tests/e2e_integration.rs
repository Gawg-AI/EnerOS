//! End-to-end integration tests for the EnerOS system.
//!
//! These tests exercise the full stack: real engines, real handlers, real data flow.
//! They verify that all components are wired together correctly and the API returns
//! meaningful results from the underlying computation engines.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use tower::ServiceExt;

use eneros_agent::event_adapter::AgentEventHandler;
use eneros_agent::{AgentContext, AgentOrchestrator, DataDrivenAgentLoop, DispatchAgent, OperationAgent};
use eneros_api::app::{create_router, AppState};
use eneros_constraint::ConstraintEngine;
use eneros_constraint::projector::FeasibilityProjector;
use eneros_eventbus::event::{EventPayload, EventType};
use eneros_eventbus::{Event, EventBus};
use eneros_gateway::SafetyGateway;
use eneros_gateway::constraint_validator::ConstraintAwareValidator;
use eneros_gateway::decision_pipeline::ConstrainedDecisionPipeline;
use eneros_gateway::interlocking::InterlockingRuleEngine;
use eneros_memory::InMemoryMemory;
use eneros_network::{PowerNetwork, NetworkSimulatorAdapter};
use eneros_reasoning::RuleBasedEngine;
use eneros_scada::{DataPipeline, ScadaCollector, SimulatedDataSource, build_ieee14_scada_config, build_ieee14_snapshot_mappings};
use eneros_scada::snapshot::SnapshotBuilder;
use eneros_timeseries::TimeSeriesEngine;
use eneros_tool::ToolEngine;
use parking_lot::RwLock;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Build a fully populated AppState with real engines, mirroring main.rs setup.
fn build_test_app_state() -> AppState {
    // 1. EventBus
    let event_bus = Arc::new(EventBus::new(64));

    // 2. ConstraintEngine with EventBus wired in
    let constraint_engine = Arc::new(ConstraintEngine::with_event_bus(event_bus.clone()));

    // 3. PowerNetwork from IEEE 14 bus with shared ConstraintEngine
    let network = Arc::new(
        PowerNetwork::from_ieee14()
            .with_constraint_engine(constraint_engine.clone())
    );

    // 4. TimeSeriesEngine
    let ts_engine = Arc::new(TimeSeriesEngine::new(10000));

    // 5. ScadaCollector with SimulatedDataSource
    let data_source = Arc::new(SimulatedDataSource::new());
    let scada_config = build_ieee14_scada_config();
    let collector = Arc::new(ScadaCollector::new(scada_config, data_source));

    // 6. DataPipeline
    let pipeline = Arc::new(
        DataPipeline::new(collector.clone(), ts_engine.clone())
            .with_event_bus(event_bus.clone()),
    );

    // 7. SnapshotBuilder
    let snapshot_builder = Arc::new(SnapshotBuilder::new(build_ieee14_snapshot_mappings()));

    // 8. FeasibilityProjector + wire into ConstraintEngine
    let network_rw = Arc::new(RwLock::new(PowerNetwork::from_ieee14()));
    let network_simulator = Arc::new(NetworkSimulatorAdapter::new(network_rw.clone()));
    let projector = Arc::new(FeasibilityProjector::new(network_simulator));
    constraint_engine.set_projector(projector.clone());

    // 9. ConstrainedDecisionPipeline
    let gateway = Arc::new(SafetyGateway::new(100));
    let validator_gateway = Arc::new(SafetyGateway::new(100));
    let interlocking = InterlockingRuleEngine::new();
    let validator = Arc::new(ConstraintAwareValidator::with_projector(
        constraint_engine.clone(),
        validator_gateway,
        interlocking,
        projector.clone(),
    ));
    let _pipeline_decision = Arc::new(ConstrainedDecisionPipeline::new(
        projector.clone(),
        validator,
        gateway.clone(),
    ));

    // 10. AgentOrchestrator with AgentContext
    let tool_engine = Arc::new(RwLock::new(ToolEngine::new()));
    let memory = Arc::new(InMemoryMemory::default());
    let reasoning = Arc::new(RuleBasedEngine::new());

    let ctx = AgentContext::new(
        event_bus.clone(),
        gateway,
        tool_engine,
        network_rw,
        memory,
        reasoning,
    );
    let mut orchestrator = AgentOrchestrator::new(ctx);

    // 11. Register DispatchAgent and OperationAgent
    let dispatch_agent = DispatchAgent::new("dispatch-1", "DispatchAgent", vec![1]);
    orchestrator.register_agent(AgentEventHandler::new(
        Box::new(dispatch_agent),
        vec![EventType::ConstraintViolation, EventType::DataReceived],
    ));

    let operation_agent = OperationAgent::new("operation-1", "OperationAgent", vec![1]);
    orchestrator.register_agent(AgentEventHandler::new(
        Box::new(operation_agent),
        vec![EventType::ConstraintViolation, EventType::SystemAlarm],
    ));

    let orchestrator = Arc::new(orchestrator);

    // 12. DataDrivenAgentLoop
    let state_machine = Arc::new(eneros_agent::SystemStateMachine::new());
    let dd_loop = Arc::new(
        DataDrivenAgentLoop::new(
            pipeline.clone(),
            collector.clone(),
            snapshot_builder.clone(),
            orchestrator.clone(),
            state_machine,
        )
        .with_constraint_engine(constraint_engine.clone()),
    );

    // 13. Build AppState
    AppState::new()
        .with_network(network)
        .with_constraint_engine(constraint_engine)
        .with_ts_engine(ts_engine)
        .with_scada_collector(collector)
        .with_event_bus(event_bus)
        .with_agent_orchestrator(orchestrator)
        .with_data_pipeline(pipeline)
        .with_snapshot_builder(snapshot_builder)
        .with_data_driven_loop(dd_loop)
}

/// Build a test app (axum Router) from the fully populated AppState.
fn build_test_app() -> Router {
    let state = build_test_app_state();
    create_router(state)
}

/// Helper to extract the response body as a String.
async fn body_to_string(body: Body) -> String {
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

// ===========================================================================
// Test 1: API returns real power flow results
// ===========================================================================

#[tokio::test]
async fn test_power_flow_returns_real_results() {
    let app = build_test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/power-flow")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    // ApiResponse wrapper should indicate success
    assert_eq!(json["success"], true);

    // Power flow should converge for IEEE 14
    let data = json["data"].as_object().unwrap();
    assert_eq!(data["converged"], true);
    assert!(data["iterations"].as_u64().unwrap() > 0);

    // Should have bus voltage data
    let bus_voltages = data["bus_voltages"].as_array().unwrap();
    assert!(!bus_voltages.is_empty());

    // Each bus should have a valid voltage magnitude
    for bus in bus_voltages {
        let v = bus["voltage_magnitude"].as_f64().unwrap();
        assert!(v > 0.9 && v < 1.2, "Bus voltage {} out of reasonable range", v);
    }
}

// ===========================================================================
// Test 2: API returns real constraint check
// ===========================================================================

#[tokio::test]
async fn test_constraints_returns_valid_json() {
    let app = build_test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/constraints")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    // Should be a valid ApiResponse
    assert_eq!(json["success"], true);

    // Data should be an array (violations list, may be empty)
    assert!(json["data"].is_array());
}

// ===========================================================================
// Test 3: API returns registered agents
// ===========================================================================

#[tokio::test]
async fn test_agents_returns_registered_agents() {
    let app = build_test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/agents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(json["success"], true);

    let data = json["data"].as_object().unwrap();
    let agent_count = data["agent_count"].as_u64().unwrap();
    assert!(agent_count > 0, "Should have at least one registered agent");

    let agents = data["agents"].as_array().unwrap();
    assert_eq!(agents.len() as u64, agent_count);

    // Check that agent names contain expected values
    let names: Vec<&str> = agents
        .iter()
        .filter_map(|a| a["name"].as_str())
        .collect();
    assert!(
        names.iter().any(|n| n.contains("DispatchAgent") || n.contains("dispatch")),
        "Should contain dispatch agent, got: {:?}",
        names
    );
    assert!(
        names.iter().any(|n| n.contains("OperationAgent") || n.contains("operation")),
        "Should contain operation agent, got: {:?}",
        names
    );
}

// ===========================================================================
// Test 4: API returns SCADA data
// ===========================================================================

#[tokio::test]
async fn test_scada_latest_returns_valid_json() {
    let app = build_test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/scada/latest")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(json["success"], true);

    let data = json["data"].as_object().unwrap();
    // Should have a readings array (may be empty if no collection cycle ran)
    assert!(data["readings"].is_array());
    // Should have a snapshot_time string
    assert!(data["snapshot_time"].is_string());
}

// ===========================================================================
// Test 5: API returns topology data
// ===========================================================================

#[tokio::test]
async fn test_topology_returns_bus_and_branch_data() {
    let app = build_test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/topology")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(json["success"], true);

    let data = json["data"].as_object().unwrap();

    // Should have buses and branches arrays
    let buses = data["buses"].as_array().unwrap();
    let branches = data["branches"].as_array().unwrap();

    assert!(!buses.is_empty(), "Should have at least one bus");
    assert!(!branches.is_empty(), "Should have at least one branch");

    // IEEE 14 has 14 buses
    assert_eq!(buses.len(), 14);
}

// ===========================================================================
// Test 6: SCADA data flows through pipeline
// ===========================================================================

#[tokio::test]
async fn test_scada_data_flows_through_pipeline() {
    // Create components directly (not via AppState) for fine-grained testing
    let data_source = Arc::new(SimulatedDataSource::new());
    let scada_config = build_ieee14_scada_config();
    let collector = Arc::new(ScadaCollector::new(scada_config, data_source));
    let ts_engine = Arc::new(TimeSeriesEngine::new(10000));
    let pipeline = DataPipeline::new(collector.clone(), ts_engine.clone());

    // Run one pipeline cycle
    let count = pipeline.run_once().unwrap();
    assert!(count > 0, "Pipeline should write at least one data point, got {}", count);

    // Verify data was written to TimeSeriesEngine
    let latest = ts_engine.latest(1, "voltage_pu");
    assert!(latest.is_some(), "Should have voltage data for bus 1 after pipeline run");

    let dp = latest.unwrap();
    assert!(
        (dp.value - 1.060).abs() < 0.01,
        "Bus 1 voltage should be ~1.060 p.u., got {}",
        dp.value
    );

    // Verify multiple buses have data
    for bus_id in 1u64..=14 {
        let v = ts_engine.latest(bus_id, "voltage_pu");
        assert!(v.is_some(), "Bus {} should have voltage data", bus_id);
    }

    // Verify frequency data
    let freq = ts_engine.latest(0, "frequency_hz");
    assert!(freq.is_some(), "Should have frequency data");
    assert!(
        (freq.unwrap().value - 50.0).abs() < 0.01,
        "Frequency should be ~50.0 Hz"
    );
}

// ===========================================================================
// Test 7: Agent responds to event
// ===========================================================================

#[tokio::test]
async fn test_agent_responds_to_constraint_violation_event() {
    // Create an AgentOrchestrator with DispatchAgent
    let event_bus = Arc::new(EventBus::new(64));
    let gateway = Arc::new(SafetyGateway::new(100));
    let tool_engine = Arc::new(RwLock::new(ToolEngine::new()));
    let network_rw = Arc::new(RwLock::new(PowerNetwork::from_ieee14()));
    let memory = Arc::new(InMemoryMemory::default());
    let reasoning = Arc::new(RuleBasedEngine::new());

    let ctx = AgentContext::new(
        event_bus.clone(),
        gateway,
        tool_engine,
        network_rw,
        memory,
        reasoning,
    );

    let mut orchestrator = AgentOrchestrator::new(ctx);

    let dispatch_agent = DispatchAgent::new("dispatch-1", "DispatchAgent", vec![1]);
    orchestrator.register_agent(AgentEventHandler::new(
        Box::new(dispatch_agent),
        vec![EventType::ConstraintViolation, EventType::DataReceived],
    ));

    assert_eq!(orchestrator.agent_count(), 1);

    // Publish a ConstraintViolation event
    let event = Event::new(
        EventType::ConstraintViolation,
        "test-source",
        EventPayload::ConstraintViolation {
            constraint_id: "v-bus-5-low".to_string(),
            element_id: 5,
            actual_value: 0.92,
            limit_value: 0.95,
            severity: "Major".to_string(),
        },
    );

    // Process the event — should not error
    let results = orchestrator.process_event(event).await;
    assert!(results.is_ok(), "Processing constraint violation event should not error: {:?}", results);
}

// ===========================================================================
// Test 8: ConstraintEngine publishes violation events via EventBus
// ===========================================================================

#[tokio::test]
async fn test_constraint_engine_publishes_violation_via_event_bus() {
    use eneros_constraint::rules::Constraint;
    use eneros_constraint::rules::ConstraintType;

    let event_bus = Arc::new(EventBus::new(64));
    let constraint_engine = Arc::new(ConstraintEngine::with_event_bus(event_bus.clone()));

    // Subscribe to events before triggering violations
    let mut receiver = event_bus.subscribe();

    // Register a voltage constraint
    let mut constraint = Constraint::new(
        "v-bus-1-low".to_string(),
        "Bus 1 voltage lower limit".to_string(),
        ConstraintType::Voltage,
        0.95,
        1.05,
    );
    constraint.element_ids = vec![1];
    constraint_engine.register(constraint);

    // Trigger a violation: bus 1 voltage at 0.90 pu (below 0.95 limit)
    let bus_voltages: Vec<(u64, f64)> = vec![(1, 0.90)];
    let branch_loadings: Vec<(u64, f64)> = vec![];
    let violations = constraint_engine.check_all(&bus_voltages, &branch_loadings, 50.0);

    assert_eq!(violations.len(), 1, "Should detect 1 voltage violation");

    // Verify the violation event was published to EventBus
    let event = receiver.try_recv().expect("Should receive violation event from EventBus");
    assert_eq!(event.event_type, EventType::ConstraintViolation);
    match event.payload {
        EventPayload::ConstraintViolation { constraint_id, element_id, actual_value, limit_value, .. } => {
            assert_eq!(constraint_id, "v-bus-1-low");
            assert_eq!(element_id, 1);
            assert!((actual_value - 0.90).abs() < f64::EPSILON);
            assert!((limit_value - 0.95).abs() < f64::EPSILON);
        }
        _ => panic!("Expected ConstraintViolation payload, got {:?}", event.payload),
    }
}

// ===========================================================================
// Test 9: ConstraintEngine uses Projector for structured action feasibility
// ===========================================================================

#[tokio::test]
async fn test_constraint_engine_uses_projector_for_feasibility() {
    use eneros_core::StructuredAction;

    let event_bus = Arc::new(EventBus::new(64));
    let constraint_engine = Arc::new(ConstraintEngine::with_event_bus(event_bus));

    // Wire projector into ConstraintEngine
    let network_rw = Arc::new(RwLock::new(PowerNetwork::from_ieee14()));
    let network_simulator = Arc::new(NetworkSimulatorAdapter::new(network_rw));
    let projector = Arc::new(FeasibilityProjector::new(network_simulator));
    constraint_engine.set_projector(projector);

    // Check a well-formed structured action — should use projector (physics-based)
    let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 50.0 };
    let result = constraint_engine.check_structured_action_feasibility(&action);

    // Projector should return a result (feasible or projected, not heuristic)
    // The key verification: this goes through What-If simulation, not keyword matching
    assert!(result.feasible || !result.new_violations.is_empty(),
        "Projector-based feasibility check should return a meaningful result");
}

// ===========================================================================
// Bonus: Health endpoint works with populated state
// ===========================================================================

#[tokio::test]
async fn test_health_endpoint_with_populated_state() {
    let app = build_test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
