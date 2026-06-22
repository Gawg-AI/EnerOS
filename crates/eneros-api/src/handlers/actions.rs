use axum::extract::State;
use axum::Json;
use eneros_core::{AuthorityLevel, Jurisdiction, StructuredAction, SystemOperatingState};
use eneros_runtime::gateway::pipeline_types::DecisionContext;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::app::AppState;
use crate::types::ApiResponse;

/// Schema mirror of `eneros_core::StructuredAction` for OpenAPI documentation.
///
/// `StructuredAction` is defined in `eneros-core` and cannot derive `ToSchema`
/// there without adding a utoipa dependency to that crate. This enum mirrors
/// its variants so the `/api/actions/structured` endpoint can be documented.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub enum StructuredActionSchema {
    /// Execute a device operation
    ExecuteDevice { device_id: u64, operation: String, value: f64 },
    /// Shed load
    ShedLoad { zone_id: u32, amount_mw: f64 },
    /// Start/adjust generator
    StartGenerator { gen_id: u64, target_mw: f64 },
    /// Notify an agent
    NotifyAgent { agent_id: String, message: String },
    /// Isolate fault section
    IsolateFault { upstream_switch: u64, downstream_switch: u64 },
    /// Close tie switch for restoration
    CloseTieSwitch { switch_id: u64 },
}

/// Schema wrapper for the structured action request body.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StructuredActionRequestSchema {
    pub action: StructuredActionSchema,
    pub authority: String,
    pub system_state: Option<String>,
}

/// Schema wrapper for the structured action response body.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StructuredActionResponseSchema {
    pub executed: bool,
    pub verdict: String,
    pub audit_count: usize,
    pub total_latency_us: u64,
}

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

/// POST /api/actions/structured
#[utoipa::path(
    post,
    path = "/api/actions/structured",
    request_body = StructuredActionRequestSchema,
    responses(
        (status = 200, description = "结构化动作执行结果", body = StructuredActionResponseSchema),
        (status = 400, description = "请求参数错误或决策管道未配置"),
    )
)]
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
