//! `dispatch_structured` 集成测试（P0）
//!
//! 验证 `ActionDispatcher` 与 `ConstrainedDecisionPipeline` 的集成行为。
//!
//! 本测试原位于 `eneros-gateway/tests/decision_pipeline_verification.rs`，
//! 因 gateway 的 dev-dependency 依赖 `eneros-agent` 而 agent 运行时依赖
//! `eneros-gateway`，形成循环。为消除循环，将依赖 `ActionDispatcher`
//! 的集成测试迁移至此：agent 运行时已依赖 gateway，此处可同时使用
//! 两者类型而不引入任何循环依赖。

use std::sync::Arc;

use eneros_agent::dispatcher::{ActionDispatcher, DispatchResult};
use eneros_constraint::projector::{FeasibilityProjector, NetworkSimulator};
use eneros_constraint::ConstraintEngine;
use eneros_core::{
    AuthorityLevel, Jurisdiction, StructuredAction, SystemOperatingState,
};
use eneros_eventbus::EventBus;
use eneros_gateway::constraint_validator::ConstraintAwareValidator;
use eneros_gateway::decision_pipeline::ConstrainedDecisionPipeline;
use eneros_gateway::SafetyGateway;
use eneros_test_utils::FeasibleMockSimulator;

fn make_pipeline_with(simulator: Arc<dyn NetworkSimulator>) -> ConstrainedDecisionPipeline {
    let projector = Arc::new(FeasibilityProjector::new(simulator));
    let constraint_engine = Arc::new(ConstraintEngine::new());
    let gateway = Arc::new(SafetyGateway::new(100));
    let validator = Arc::new(ConstraintAwareValidator::with_default_interlocking(
        constraint_engine,
        gateway.clone(),
    ));
    ConstrainedDecisionPipeline::new(projector, validator, gateway)
}

fn make_pipeline() -> ConstrainedDecisionPipeline {
    make_pipeline_with(Arc::new(FeasibleMockSimulator))
}

fn make_dispatcher_with_pipeline(pipeline: ConstrainedDecisionPipeline) -> ActionDispatcher {
    let event_bus = Arc::new(EventBus::new(64));
    let gateway = Arc::new(SafetyGateway::new(100));
    ActionDispatcher::new_local(event_bus, gateway).with_pipeline(Arc::new(pipeline))
}

fn make_dispatcher_without_pipeline() -> ActionDispatcher {
    let event_bus = Arc::new(EventBus::new(64));
    let gateway = Arc::new(SafetyGateway::new(100));
    ActionDispatcher::new_local(event_bus, gateway)
}

#[tokio::test]
async fn test_dispatch_structured_with_pipeline_approved() {
    let pipeline = make_pipeline();
    let dispatcher = make_dispatcher_with_pipeline(pipeline);
    let action = StructuredAction::StartGenerator {
        gen_id: 1,
        target_mw: 100.0,
    };
    let result = dispatcher
        .dispatch_structured(
            &action,
            AuthorityLevel::Supervisor,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        )
        .await
        .unwrap();
    assert_eq!(result, DispatchResult::CommandExecuted);
}

#[tokio::test]
async fn test_dispatch_structured_with_pipeline_rejected() {
    let pipeline = make_pipeline();
    let dispatcher = make_dispatcher_with_pipeline(pipeline);
    let action = StructuredAction::StartGenerator {
        gen_id: 1,
        target_mw: 100.0,
    };
    let result = dispatcher
        .dispatch_structured(
            &action,
            AuthorityLevel::Observer,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        )
        .await
        .unwrap();
    // Observer cannot execute commands → pipeline rejects → ConstraintRejected
    assert!(matches!(result, DispatchResult::ConstraintRejected(_)));
}

#[tokio::test]
async fn test_dispatch_structured_without_pipeline_returns_error() {
    let dispatcher = make_dispatcher_without_pipeline();
    let action = StructuredAction::StartGenerator {
        gen_id: 1,
        target_mw: 100.0,
    };
    let result = dispatcher
        .dispatch_structured(
            &action,
            AuthorityLevel::Operator,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        )
        .await;
    // Without a pipeline, gateway_client.decide() fails and the error
    // is propagated (NOT a false CommandExecuted). The orchestrator's
    // dispatch_via_pipeline handles this by checking has_pipeline() first
    // and falling back to direct ExecuteCommand dispatch.
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("gateway decide failed"),
        "unexpected error: {}",
        err_msg
    );
}

#[tokio::test]
async fn test_dispatch_structured_high_risk_requires_supervisor() {
    let pipeline = make_pipeline();
    let dispatcher = make_dispatcher_with_pipeline(pipeline);
    let action = StructuredAction::ShedLoad {
        zone_id: 1,
        amount_mw: 50.0,
    };
    let result = dispatcher
        .dispatch_structured(
            &action,
            AuthorityLevel::Operator,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        )
        .await
        .unwrap();
    // ShedLoad is high-risk; Operator cannot execute high-risk → rejected or pending
    match result {
        DispatchResult::ConstraintRejected(reason) => {
            // Rejected because Operator cannot execute high-risk
            assert!(
                reason.to_lowercase().contains("high-risk")
                    || reason.to_lowercase().contains("supervisor")
                    || reason.to_lowercase().contains("pending"),
                "Unexpected rejection reason: {}",
                reason
            );
        }
        DispatchResult::PendingApproval { .. } => {
            // Also acceptable: pending supervisor approval
        }
        other => panic!(
            "Expected ConstraintRejected or PendingApproval, got {:?}",
            other
        ),
    }
}

#[tokio::test]
async fn test_dispatch_structured_emergency_bypass() {
    let pipeline = make_pipeline();
    let dispatcher = make_dispatcher_with_pipeline(pipeline);
    let action = StructuredAction::StartGenerator {
        gen_id: 1,
        target_mw: 100.0,
    };
    let result = dispatcher
        .dispatch_structured(
            &action,
            AuthorityLevel::Emergency,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Emergency,
        )
        .await
        .unwrap();
    // Emergency authority in Emergency state should bypass constraints
    assert!(
        matches!(
            result,
            DispatchResult::CommandExecuted | DispatchResult::EmergencyBypassed { .. }
        ),
        "Expected CommandExecuted or EmergencyBypassed, got {:?}",
        result
    );
}
