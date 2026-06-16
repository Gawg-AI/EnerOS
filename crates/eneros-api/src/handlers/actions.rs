use axum::extract::State;
use axum::Json;
use eneros_core::{AuthorityLevel, Jurisdiction, StructuredAction, SystemOperatingState};
use eneros_gateway::pipeline_types::DecisionContext;
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::types::ApiResponse;

#[derive(Debug, Deserialize)]
pub struct StructuredActionRequest {
    pub action: StructuredAction,
    pub authority: AuthorityLevel,
    pub system_state: Option<SystemOperatingState>,
}

#[derive(Debug, Serialize)]
pub struct StructuredActionResponse {
    pub executed: bool,
    pub verdict: String,
    pub audit_count: usize,
    pub total_latency_us: u64,
}

pub async fn structured_action_handler(
    State(state): State<AppState>,
    Json(request): Json<StructuredActionRequest>,
) -> Json<ApiResponse<StructuredActionResponse>> {
    let Some(pipeline) = &state.decision_pipeline else {
        return Json(ApiResponse::error(
            "Constrained decision pipeline is not configured".to_string(),
        ));
    };

    let ctx = DecisionContext::new(
        request.authority,
        Jurisdiction::unrestricted(),
        request.system_state.unwrap_or(SystemOperatingState::Normal),
    );
    let decision = pipeline.decide_enhanced(&request.action, &ctx).await;

    Json(ApiResponse::success(StructuredActionResponse {
        executed: decision.executed_action.is_some(),
        verdict: format!("{:?}", decision.verdict),
        audit_count: decision.audit.len(),
        total_latency_us: decision.total_latency_us,
    }))
}
