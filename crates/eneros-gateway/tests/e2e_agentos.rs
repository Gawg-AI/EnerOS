//! End-to-end integration tests for the Gateway IPC system (v0.16.0)
//!
//! Verifies the full IPC path:
//! `RemoteGatewayClient` → TCP → `GatewayServer` →
//! `SafetyGateway` / `ConstrainedDecisionPipeline` → response.
//!
//! Each test spawns a real `GatewayServer` on an ephemeral TCP port and
//! drives it through a `RemoteGatewayClient`, exercising the same wire
//! format (4-byte LE length prefix + JSON) that production Agent ↔ Gateway
//! IPC uses.

use std::sync::Arc;

use eneros_constraint::projector::{FeasibilityProjector, NetworkSimulator, WhatIfResult};
use eneros_constraint::ConstraintEngine;
use eneros_core::agentos_types::{
    AuthorityLevel, Jurisdiction, StructuredAction, SystemOperatingState,
};
use eneros_core::pipeline_types::DecisionContextCore;
use eneros_core::{ActionVerdict, Command, CommandPriority, CommandType, GatewayClient};
use eneros_gateway::constraint_validator::ConstraintAwareValidator;
use eneros_gateway::decision_pipeline::ConstrainedDecisionPipeline;
use eneros_gateway::{
    GatewayServer, LocalGatewayClient, RemoteGatewayClient, SafetyGateway,
    SharedPriorityCommandQueue,
};

// ============================================================================
// Mock simulator
// ============================================================================

/// Always-feasible mock simulator (mirrors the one in
/// `decision_pipeline_verification.rs`).
struct FeasibleMockSimulator;

impl NetworkSimulator for FeasibleMockSimulator {
    fn simulate_action(&self, _action: &StructuredAction) -> WhatIfResult {
        WhatIfResult {
            applicable: true,
            converged: true,
            voltage_violations: vec![],
            thermal_violations: vec![],
            all_constraints_satisfied: true,
            summary: "OK".to_string(),
        }
    }
    fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
        vec![(1, 0.0, 200.0), (2, 0.0, 150.0)]
    }
    fn current_voltages(&self) -> Vec<(u64, f64)> {
        vec![(1, 1.02), (2, 0.98)]
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Build a `ConstrainedDecisionPipeline` backed by `FeasibleMockSimulator`.
fn make_pipeline() -> ConstrainedDecisionPipeline {
    let simulator: Arc<dyn NetworkSimulator> = Arc::new(FeasibleMockSimulator);
    let projector = Arc::new(FeasibilityProjector::new(simulator));
    let constraint_engine = Arc::new(ConstraintEngine::new());
    let gateway = Arc::new(SafetyGateway::new(100));
    let validator = Arc::new(ConstraintAwareValidator::with_default_interlocking(
        constraint_engine,
        gateway.clone(),
    ));
    ConstrainedDecisionPipeline::new(projector, validator, gateway)
}

fn sample_command() -> Command {
    Command::new(CommandType::SwitchToggle, 1, CommandPriority::Normal, "test")
}

fn sample_context_core() -> DecisionContextCore {
    DecisionContextCore {
        authority: AuthorityLevel::Supervisor,
        jurisdiction: Jurisdiction::unrestricted(),
        system_state: SystemOperatingState::Normal,
        observation: None,
        agent_id: "test".to_string(),
        reasoning: "".to_string(),
    }
}

/// Bind a `TcpListener` to `127.0.0.1:0` to obtain an ephemeral port, then
/// drop the listener and return the address. There is a small race window
/// before the server rebinds, but it is acceptable for tests and avoids the
/// port-conflict problems of fixed ports.
async fn pick_free_port() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind 127.0.0.1:0 should succeed");
    let addr = listener.local_addr().expect("local_addr should succeed");
    drop(listener);
    addr.to_string()
}

/// Spawn a `GatewayServer` in a background tokio task and return its address.
/// The server's `run()` method blocks forever; the task is cancelled when the
/// test's runtime is torn down.
async fn start_server(client: LocalGatewayClient) -> String {
    let addr = pick_free_port().await;
    let server = GatewayServer::new(client, addr.clone());
    tokio::spawn(async move {
        let _ = server.run().await;
    });
    // Give the server time to bind and enter the accept loop.
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    addr
}

// ============================================================================
// Tests
// ============================================================================

#[tokio::test]
async fn test_e2e_validate_command() {
    // Gateway stack without pipeline — validate_command only needs SafetyGateway.
    let gateway = Arc::new(SafetyGateway::new(100));
    let client = LocalGatewayClient::new(gateway);
    let addr = start_server(client).await;

    let remote = RemoteGatewayClient::new(&addr);
    let cmd = sample_command();
    let result = remote.validate_command(&cmd).await;
    assert!(result.is_ok(), "validate_command should succeed: {:?}", result);
}

#[tokio::test]
async fn test_e2e_execute_command() {
    // Gateway stack without pipeline — execute_command uses the LoggingExecutor
    // (default), which always returns a successful ExecutionResult.
    let gateway = Arc::new(SafetyGateway::new(100));
    let client = LocalGatewayClient::new(gateway);
    let addr = start_server(client).await;

    let remote = RemoteGatewayClient::new(&addr);
    let cmd = sample_command();
    let result = remote.execute_command(cmd).await;
    assert!(result.is_ok(), "execute_command should succeed: {:?}", result);
    let exec = result.unwrap();
    assert!(
        exec.success,
        "execution result should be success (LoggingExecutor): {:?}",
        exec
    );
}

#[tokio::test]
async fn test_e2e_submit_command() {
    // Gateway configured with a priority queue so submit_command succeeds.
    let gateway = Arc::new(SafetyGateway::with_queue(
        100,
        Arc::new(SharedPriorityCommandQueue::new()),
    ));
    let client = LocalGatewayClient::new(gateway);
    let addr = start_server(client).await;

    let remote = RemoteGatewayClient::new(&addr);
    let cmd = sample_command();
    let result = remote.submit_command(cmd).await;
    assert!(result.is_ok(), "submit_command should succeed: {:?}", result);
}

#[tokio::test]
async fn test_e2e_decide_with_pipeline() {
    // Full stack: SafetyGateway + ConstrainedDecisionPipeline.
    // The decide() path only touches the pipeline, so the LocalGatewayClient's
    // gateway is not exercised here; a fresh gateway is used for the client
    // wrapper while the pipeline holds its own internal gateway clone.
    let pipeline = make_pipeline();
    let gateway = Arc::new(SafetyGateway::new(100));
    let client = LocalGatewayClient::with_pipeline(gateway, Arc::new(pipeline));
    let addr = start_server(client).await;

    let remote = RemoteGatewayClient::new(&addr);
    let action = StructuredAction::StartGenerator {
        gen_id: 1,
        target_mw: 100.0,
    };
    let result = remote.decide(action, sample_context_core()).await;
    assert!(result.is_ok(), "decide should succeed: {:?}", result);
    let decision = result.unwrap();
    // Supervisor + Normal state + feasible action → Approved & executed.
    assert_eq!(decision.verdict, ActionVerdict::Approved);
    assert!(decision.executed, "action should be executed");
}

#[tokio::test]
async fn test_e2e_decide_without_pipeline_returns_error() {
    // Gateway stack WITHOUT pipeline — decide() must return an error.
    let gateway = Arc::new(SafetyGateway::new(100));
    let client = LocalGatewayClient::new(gateway);
    let addr = start_server(client).await;

    let remote = RemoteGatewayClient::new(&addr);
    let action = StructuredAction::StartGenerator {
        gen_id: 1,
        target_mw: 100.0,
    };
    let result = remote.decide(action, sample_context_core()).await;
    assert!(
        result.is_err(),
        "decide without pipeline should fail: {:?}",
        result
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("pipeline"),
        "error should mention pipeline: {}",
        err
    );
}

#[tokio::test]
async fn test_e2e_connection_refused() {
    // Port 1 is a privileged port and should refuse connections on all
    // non-root test environments, exercising the TCP connect error path.
    let remote = RemoteGatewayClient::new("127.0.0.1:1");
    let cmd = sample_command();
    let result = remote.execute_command(cmd).await;
    assert!(
        result.is_err(),
        "execute_command should fail with connection refused: {:?}",
        result
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("connect"),
        "error should mention connect: {}",
        err
    );
}
