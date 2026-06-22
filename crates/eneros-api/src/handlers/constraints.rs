use axum::extract::State;
use axum::Json;

use crate::app::AppState;
use crate::types::{ApiResponse, ConstraintViolationResponse};

/// GET /api/constraints
#[utoipa::path(
    get,
    path = "/api/constraints",
    responses(
        (status = 200, description = "当前约束越限列表", body = ConstraintViolationResponse),
    )
)]
pub async fn constraints_handler(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<ConstraintViolationResponse>>> {
    // Try using the network's constraint engine (runs power flow first)
    if let Some(network) = &state.network {
        match network.solve() {
            Ok(pf_result) => {
                let violations = network.check_constraints(&pf_result);
                let response: Vec<ConstraintViolationResponse> = violations
                    .iter()
                    .map(|v| ConstraintViolationResponse {
                        constraint_id: v.constraint_id.clone(),
                        element_id: v.element_id,
                        actual_value: v.actual_value,
                        limit_min: v.limit_min,
                        limit_max: v.limit_max,
                        severity: format!("{:?}", v.severity),
                    })
                    .collect();
                return Json(ApiResponse::success(response));
            }
            Err(e) => {
                return Json(ApiResponse::error(format!(
                    "Power flow for constraint check failed: {}",
                    e
                )))
            }
        }
    }

    // Try standalone constraint engine with current violations
    if let Some(engine) = &state.constraint_engine {
        let violations = engine.get_current_violations();
        if !violations.is_empty() {
            let response: Vec<ConstraintViolationResponse> = violations
                .iter()
                .map(|v| ConstraintViolationResponse {
                    constraint_id: v.constraint_id.clone(),
                    element_id: v.element_id,
                    actual_value: v.actual_value,
                    limit_min: v.limit_min,
                    limit_max: v.limit_max,
                    severity: format!("{:?}", v.severity),
                })
                .collect();
            return Json(ApiResponse::success(response));
        }
    }

    // No constraint engine available — return empty
    Json(ApiResponse::success(Vec::new()))
}
