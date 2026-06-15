use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use eneros_api::app::{create_router, AppState};
use eneros_constraint::projector::{FeasibilityProjector, NetworkSimulator, WhatIfResult};
use eneros_constraint::ConstraintEngine;
use eneros_core::{AuthorityLevel, StructuredAction};
use eneros_gateway::constraint_validator::ConstraintAwareValidator;
use eneros_gateway::decision_pipeline::ConstrainedDecisionPipeline;
use eneros_gateway::SafetyGateway;
use serde_json::Value;
use tower::ServiceExt;

struct FeasibleSimulator;

impl NetworkSimulator for FeasibleSimulator {
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
        vec![(2, 0.0, 140.0)]
    }

    fn current_voltages(&self) -> Vec<(u64, f64)> {
        vec![(2, 1.0)]
    }
}

fn app_with_decision_pipeline() -> axum::Router {
    let projector = Arc::new(FeasibilityProjector::new(Arc::new(FeasibleSimulator)));
    let constraint_engine = Arc::new(ConstraintEngine::new());
    let safety_gateway = Arc::new(SafetyGateway::new(100));
    let validator = Arc::new(ConstraintAwareValidator::with_default_interlocking(
        constraint_engine,
        safety_gateway.clone(),
    ));
    let pipeline = Arc::new(ConstrainedDecisionPipeline::new(
        projector,
        validator,
        safety_gateway,
    ));

    create_router(AppState::new().with_decision_pipeline(pipeline))
}

async fn post_structured_action(authority: AuthorityLevel) -> Value {
    let body = serde_json::json!({
        "action": StructuredAction::StartGenerator {
            gen_id: 2,
            target_mw: 40.0,
        },
        "authority": authority
    });

    let response = app_with_decision_pipeline()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/actions/structured")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn test_http_structured_action_executes_through_pipeline_for_supervisor() {
    let payload = post_structured_action(AuthorityLevel::Supervisor).await;

    assert_eq!(payload["success"], true);
    assert_eq!(payload["data"]["executed"], true, "{payload:#}");
    assert_eq!(payload["data"]["verdict"], "Approved", "{payload:#}");
}

#[tokio::test]
async fn test_http_structured_action_rejects_observer_via_pipeline() {
    let payload = post_structured_action(AuthorityLevel::Observer).await;

    assert_eq!(payload["success"], true);
    assert_eq!(payload["data"]["executed"], false);
    assert!(payload["data"]["verdict"]
        .as_str()
        .unwrap()
        .contains("Rejected"));
}
