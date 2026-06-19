//! Agent control API handler (v0.7.0 — deferred from v0.6.0 S4).
//!
//! Exposes agent lifecycle control (start/stop/pause/resume) via
//! `POST /api/agents/{id}/control`. The handler delegates to the
//! `AgentOrchestrator` when available, otherwise returns 503.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::app::AppState;

/// Request body for `POST /api/agents/{id}/control`.
#[derive(Debug, Deserialize)]
pub struct AgentControlRequest {
    /// Control action: "start" | "stop" | "pause" | "resume"
    pub action: String,
}

/// Response body for `POST /api/agents/{id}/control`.
#[derive(Debug, Serialize)]
pub struct AgentControlResponse {
    pub agent_id: String,
    pub action: String,
    pub result: String,
    pub message: String,
}

/// `POST /api/agents/{id}/control` — control an agent's lifecycle.
pub async fn control_handler(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<AgentControlRequest>,
) -> axum::response::Response {
    let orchestrator = match &state.agent_orchestrator {
        Some(o) => o,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "agent orchestrator not configured",
            )
                .into_response()
        }
    };

    // Validate the action
    let action_lower = req.action.to_lowercase();
    if !matches!(
        action_lower.as_str(),
        "start" | "stop" | "pause" | "resume"
    ) {
        return (
            StatusCode::BAD_REQUEST,
            format!(
                "invalid action '{}': must be one of start/stop/pause/resume",
                req.action
            ),
        )
            .into_response();
    }

    // Check if the agent is registered
    let registered = orchestrator.registered_agents();
    let found = registered.iter().any(|(name, _, _)| name == &agent_id);

    if !found {
        return (
            StatusCode::NOT_FOUND,
            format!("agent '{}' not found", agent_id),
        )
            .into_response();
    }

    // The AgentOrchestrator currently does not expose start/stop/pause/resume
    // methods directly. We log the control request and return success.
    // In a future version, this will delegate to the orchestrator's lifecycle
    // management API.
    tracing::info!(
        agent_id = %agent_id,
        action = %action_lower,
        "agent control request received"
    );

    let response = AgentControlResponse {
        agent_id: agent_id.clone(),
        action: action_lower.clone(),
        result: "accepted".to_string(),
        message: format!(
            "Agent '{}' control action '{}' accepted (lifecycle management pending orchestrator support)",
            agent_id, action_lower
        ),
    };

    (StatusCode::OK, Json(response)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_control_request_deserialize() {
        let json = r#"{"action": "pause"}"#;
        let req: AgentControlRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.action, "pause");
    }

    #[test]
    fn test_agent_control_response_serialization() {
        let resp = AgentControlResponse {
            agent_id: "dispatch-1".to_string(),
            action: "stop".to_string(),
            result: "accepted".to_string(),
            message: "Agent stopped".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"agent_id\":\"dispatch-1\""));
        assert!(json.contains("\"action\":\"stop\""));
    }
}
