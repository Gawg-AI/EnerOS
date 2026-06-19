//! WhatIf hypothesis calculation API handler (v0.7.0 — deferred from v0.6.0 S4).
//!
//! Exposes the `FeasibilityProjector`'s What-If analysis via
//! `POST /api/whatif`. Clients submit a `StructuredAction` and receive
//! a `ProjectionResult` indicating whether the action is feasible,
//! needs projection, or is infeasible.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use eneros_core::StructuredAction;

/// Request body for `POST /api/whatif`.
#[derive(Debug, Deserialize)]
pub struct WhatIfRequest {
    /// The action to evaluate.
    pub action: StructuredAction,
}

/// Response body for `POST /api/whatif`.
#[derive(Debug, Serialize)]
pub struct WhatIfResponse {
    /// Whether the action is feasible as-is.
    pub feasible: bool,
    /// Whether the action was projected (modified) to become feasible.
    pub projected: bool,
    /// Whether the action is completely infeasible.
    pub infeasible: bool,
    /// Human-readable summary.
    pub summary: String,
    /// The projection result as a JSON value (full detail).
    pub projection: serde_json::Value,
}

/// `POST /api/whatif` — evaluate an action's feasibility via What-If analysis.
pub async fn whatif_handler(
    State(state): State<AppState>,
    Json(req): Json<WhatIfRequest>,
) -> axum::response::Response {
    // The projector lives inside the decision_pipeline; we reach it via
    // the pipeline's public accessor. If no pipeline is configured, we
    // fall back to a 503.
    let pipeline = match &state.decision_pipeline {
        Some(p) => p,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "decision pipeline not configured",
            )
                .into_response()
        }
    };

    let projection = pipeline.project(&req.action);

    let (feasible, projected, infeasible) = (
        projection.is_feasible(),
        projection.is_projected(),
        projection.is_infeasible(),
    );

    let summary = if feasible {
        "Action is feasible as proposed".to_string()
    } else if projected {
        "Action was projected to the nearest feasible point".to_string()
    } else {
        "Action is infeasible".to_string()
    };

    let projection_json = serde_json::to_value(&projection).unwrap_or(serde_json::json!({}));

    let response = WhatIfResponse {
        feasible,
        projected,
        infeasible,
        summary,
        projection: projection_json,
    };

    (StatusCode::OK, Json(response)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_whatif_response_serialization() {
        let resp = WhatIfResponse {
            feasible: true,
            projected: false,
            infeasible: false,
            summary: "Action is feasible".to_string(),
            projection: serde_json::json!({"status": "feasible"}),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"feasible\":true"));
        assert!(json.contains("\"infeasible\":false"));
    }
}
