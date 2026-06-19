//! Integration tests for GatewayClient / GatewayServer (v0.15.0)
//!
//! 覆盖：
//! - LocalGatewayClient: execute_command / validate_command / decide
//! - RemoteGatewayClient + GatewayServer: TCP 往返测试
//! - 并发连接测试

use std::sync::Arc;

use eneros_constraint::projector::{
    FeasibilityProjector, NetworkSimulator, WhatIfResult,
};
use eneros_constraint::ConstraintEngine;
use eneros_core::agentos_types::{
    AuthorityLevel, Jurisdiction, StructuredAction, SystemOperatingState,
};
use eneros_core::pipeline_types::DecisionContextCore;
use eneros_core::{Command, CommandPriority, CommandType, GatewayClient};
use eneros_gateway::constraint_validator::ConstraintAwareValidator;
use eneros_gateway::decision_pipeline::ConstrainedDecisionPipeline;
use eneros_gateway::{
    GatewayServer, LocalGatewayClient, RemoteGatewayClient, SafetyGateway,
};

// ============================================================================
// Mock simulator
// ============================================================================

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
        vec![(1, 0.0, 200.0)]
    }
    fn current_voltages(&self) -> Vec<(u64, f64)> {
        vec![(1, 1.02)]
    }
}

// ============================================================================
// Helpers
// ============================================================================

fn make_pipeline_with_gateway() -> (Arc<SafetyGateway>, Arc<ConstrainedDecisionPipeline>) {
    let projector = Arc::new(FeasibilityProjector::new(Arc::new(FeasibleMockSimulator)));
    let constraint_engine = Arc::new(ConstraintEngine::new());
    let gateway = Arc::new(SafetyGateway::new(100));
    let validator = Arc::new(ConstraintAwareValidator::with_default_interlocking(
        constraint_engine,
        gateway.clone(),
    ));
    let pipeline = Arc::new(ConstrainedDecisionPipeline::new(
        projector,
        validator,
        gateway.clone(),
    ));
    (gateway, pipeline)
}

fn make_local_client_with_pipeline() -> LocalGatewayClient {
    let (gateway, pipeline) = make_pipeline_with_gateway();
    LocalGatewayClient::with_pipeline(gateway, pipeline)
}

fn make_local_client_no_pipeline() -> LocalGatewayClient {
    let gateway = Arc::new(SafetyGateway::new(100));
    LocalGatewayClient::new(gateway)
}

fn sample_command() -> Command {
    Command::new(CommandType::SwitchToggle, 42, CommandPriority::Normal, "test")
}

fn sample_context_core() -> DecisionContextCore {
    DecisionContextCore {
        authority: AuthorityLevel::Supervisor,
        jurisdiction: Jurisdiction::unrestricted(),
        system_state: SystemOperatingState::Normal,
        observation: None,
        agent_id: "test-agent".to_string(),
        reasoning: "integration test".to_string(),
    }
}

// ============================================================================
// LocalGatewayClient tests
// ============================================================================

#[tokio::test]
async fn test_local_client_execute_command_succeeds() {
    let client = make_local_client_no_pipeline();
    let cmd = sample_command();
    let result = client.execute_command(cmd).await;
    assert!(result.is_ok(), "execute_command should succeed: {:?}", result);
    let exec = result.unwrap();
    assert!(exec.success, "execution result should be success");
}

#[tokio::test]
async fn test_local_client_validate_command_ok() {
    let client = make_local_client_no_pipeline();
    let cmd = sample_command();
    let result = client.validate_command(&cmd).await;
    assert!(result.is_ok(), "validate_command should succeed: {:?}", result);
}

#[tokio::test]
async fn test_local_client_submit_command_without_queue_fails() {
    // SafetyGateway::new() 不配置优先级队列，submit_command 应失败
    let client = make_local_client_no_pipeline();
    let cmd = sample_command();
    let result = client.submit_command(cmd).await;
    assert!(
        result.is_err(),
        "submit_command without queue should fail: {:?}",
        result
    );
}

#[tokio::test]
async fn test_local_client_decide_without_pipeline_returns_error() {
    let client = make_local_client_no_pipeline();
    let action = StructuredAction::StartGenerator {
        gen_id: 1,
        target_mw: 100.0,
    };
    let result = client.decide(action, sample_context_core()).await;
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
async fn test_local_client_decide_with_pipeline_approved() {
    let client = make_local_client_with_pipeline();
    let action = StructuredAction::StartGenerator {
        gen_id: 1,
        target_mw: 100.0,
    };
    let result = client.decide(action, sample_context_core()).await;
    assert!(result.is_ok(), "decide should succeed: {:?}", result);
    let decision = result.unwrap();
    // Supervisor + Normal state + feasible action → approved & executed
    assert!(decision.executed, "action should be executed");
    assert_eq!(decision.verdict, eneros_core::ActionVerdict::Approved);
}

#[tokio::test]
async fn test_local_client_decide_with_pipeline_observer_rejected() {
    let client = make_local_client_with_pipeline();
    let action = StructuredAction::StartGenerator {
        gen_id: 1,
        target_mw: 100.0,
    };
    let mut ctx = sample_context_core();
    ctx.authority = AuthorityLevel::Observer;
    let result = client.decide(action, ctx).await;
    assert!(result.is_ok(), "decide should still return Ok: {:?}", result);
    let decision = result.unwrap();
    assert!(!decision.executed, "action should NOT be executed for Observer");
    assert!(matches!(
        decision.verdict,
        eneros_core::ActionVerdict::Rejected(_)
    ));
}

// ============================================================================
// RemoteGatewayClient + GatewayServer round-trip tests
// ============================================================================

/// 在 127.0.0.1:0 上绑定一个临时 TCP 监听器以获取空闲端口，然后立即关闭
/// 并返回该地址。注意：存在极小的端口竞争窗口，但测试场景下可接受。
async fn pick_free_port() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr.to_string()
}

/// 启动一个 GatewayServer 并返回其地址。服务端在后台 tokio 任务中运行。
async fn start_server(client: LocalGatewayClient) -> String {
    let addr = pick_free_port().await;
    let server = GatewayServer::new(client, addr.clone());
    let _handle = tokio::spawn(async move {
        let _ = server.run().await;
    });
    // 给服务端一点时间完成 bind + accept 循环就绪
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    addr
}

#[tokio::test]
async fn test_remote_client_execute_command_round_trip() {
    let local_client = make_local_client_no_pipeline();
    let addr = start_server(local_client).await;

    let remote = RemoteGatewayClient::new(&addr);
    let cmd = sample_command();
    let result = remote.execute_command(cmd).await;
    assert!(
        result.is_ok(),
        "remote execute_command should succeed: {:?}",
        result
    );
    let exec = result.unwrap();
    assert!(exec.success, "remote execution result should be success");
}

#[tokio::test]
async fn test_remote_client_validate_command_round_trip() {
    let local_client = make_local_client_no_pipeline();
    let addr = start_server(local_client).await;

    let remote = RemoteGatewayClient::new(&addr);
    let cmd = sample_command();
    let result = remote.validate_command(&cmd).await;
    assert!(
        result.is_ok(),
        "remote validate_command should succeed: {:?}",
        result
    );
}

#[tokio::test]
async fn test_remote_client_submit_command_without_queue_returns_error() {
    let local_client = make_local_client_no_pipeline();
    let addr = start_server(local_client).await;

    let remote = RemoteGatewayClient::new(&addr);
    let cmd = sample_command();
    let result = remote.submit_command(cmd).await;
    assert!(
        result.is_err(),
        "remote submit_command without queue should fail: {:?}",
        result
    );
}

#[tokio::test]
async fn test_remote_client_decide_without_pipeline_returns_error() {
    let local_client = make_local_client_no_pipeline();
    let addr = start_server(local_client).await;

    let remote = RemoteGatewayClient::new(&addr);
    let action = StructuredAction::StartGenerator {
        gen_id: 1,
        target_mw: 100.0,
    };
    let result = remote.decide(action, sample_context_core()).await;
    assert!(
        result.is_err(),
        "remote decide without pipeline should fail: {:?}",
        result
    );
}

#[tokio::test]
async fn test_remote_client_decide_with_pipeline_approved() {
    let local_client = make_local_client_with_pipeline();
    let addr = start_server(local_client).await;

    let remote = RemoteGatewayClient::new(&addr);
    let action = StructuredAction::StartGenerator {
        gen_id: 1,
        target_mw: 100.0,
    };
    let result = remote.decide(action, sample_context_core()).await;
    assert!(
        result.is_ok(),
        "remote decide with pipeline should succeed: {:?}",
        result
    );
    let decision = result.unwrap();
    assert!(decision.executed, "remote action should be executed");
    assert_eq!(decision.verdict, eneros_core::ActionVerdict::Approved);
}

#[tokio::test]
async fn test_remote_client_decide_observer_rejected() {
    let local_client = make_local_client_with_pipeline();
    let addr = start_server(local_client).await;

    let remote = RemoteGatewayClient::new(&addr);
    let action = StructuredAction::StartGenerator {
        gen_id: 1,
        target_mw: 100.0,
    };
    let mut ctx = sample_context_core();
    ctx.authority = AuthorityLevel::Observer;
    let result = remote.decide(action, ctx).await;
    assert!(result.is_ok(), "remote decide should return Ok: {:?}", result);
    let decision = result.unwrap();
    assert!(!decision.executed, "Observer action should NOT be executed");
    assert!(matches!(
        decision.verdict,
        eneros_core::ActionVerdict::Rejected(_)
    ));
}

// ============================================================================
// Concurrency test: multiple concurrent remote clients
// ============================================================================

#[tokio::test]
async fn test_server_handles_concurrent_connections() {
    let local_client = make_local_client_no_pipeline();
    let addr = start_server(local_client).await;

    // 5 个并发客户端各自执行一次 validate_command
    let mut handles = Vec::new();
    for i in 0..5 {
        let addr_clone = addr.clone();
        handles.push(tokio::spawn(async move {
            let remote = RemoteGatewayClient::new(&addr_clone);
            let cmd = Command::new(
                CommandType::SwitchToggle,
                i,
                CommandPriority::Normal,
                "concurrent-test",
            );
            remote.validate_command(&cmd).await
        }));
    }
    for (i, h) in handles.into_iter().enumerate() {
        let result = h.await.unwrap();
        assert!(
            result.is_ok(),
            "concurrent client {} should succeed: {:?}",
            i,
            result
        );
    }
}

// ============================================================================
// Local vs Remote parity test
// ============================================================================

#[tokio::test]
async fn test_local_and_remote_decide_produce_same_verdict() {
    let local_client = make_local_client_with_pipeline();
    let addr = start_server(local_client.clone()).await;

    let action = StructuredAction::StartGenerator {
        gen_id: 1,
        target_mw: 100.0,
    };
    let ctx = sample_context_core();

    let local_result = local_client.decide(action.clone(), ctx.clone()).await.unwrap();
    let remote = RemoteGatewayClient::new(&addr);
    let remote_result = remote.decide(action, ctx).await.unwrap();

    // 两次决策的 verdict 应一致（都应 Approved）
    assert_eq!(local_result.verdict, remote_result.verdict);
    assert_eq!(local_result.executed, remote_result.executed);
}
