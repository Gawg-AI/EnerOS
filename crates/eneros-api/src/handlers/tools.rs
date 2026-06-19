//! Tools API handlers (v0.6.0 — S4).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::app::AppState;

/// Tool info in the list response.
#[derive(Debug, Serialize)]
pub struct ToolInfoDto {
    pub name: String,
    pub description: String,
    pub parameters_schema: serde_json::Value,
}

/// Request body for tool execution.
#[derive(Debug, Deserialize)]
pub struct ExecuteToolRequest {
    pub params: serde_json::Value,
}

/// Response for tool execution.
#[derive(Debug, Serialize)]
pub struct ExecuteToolResponse {
    pub success: bool,
    pub data: serde_json::Value,
    pub message: String,
}

/// `GET /api/tools` — list all registered tools.
pub async fn list_handler(
    State(state): State<AppState>,
) -> axum::response::Response {
    let tool_engine = match &state.tool_engine {
        Some(e) => e,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "tool engine not configured").into_response(),
    };

    let tools: Vec<ToolInfoDto> = tool_engine
        .read()
        .await
        .list_tools()
        .into_iter()
        .map(|t| ToolInfoDto {
            name: t.name,
            description: t.description,
            parameters_schema: t.parameters_schema,
        })
        .collect();

    (StatusCode::OK, Json(serde_json::json!({"tools": tools}))).into_response()
}

/// `POST /api/tools/{name}/execute` — execute a tool by name.
pub async fn execute_handler(
    State(state): State<AppState>,
    Path(tool_name): Path<String>,
    Json(req): Json<ExecuteToolRequest>,
) -> axum::response::Response {
    let tool_engine = match &state.tool_engine {
        Some(e) => e,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "tool engine not configured").into_response(),
    };

    let engine = tool_engine.read().await;
    match engine.execute(&tool_name, req.params).await {
        Ok(output) => {
            let response = ExecuteToolResponse {
                success: output.success,
                data: output.data,
                message: output.message,
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            tracing::warn!("tool execution failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("tool execution failed: {}", e)).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_info_dto_serialization() {
        let dto = ToolInfoDto {
            name: "power_flow".to_string(),
            description: "Run power flow analysis".to_string(),
            parameters_schema: serde_json::json!({"type": "object"}),
        };
        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("\"name\":\"power_flow\""));
    }

    #[test]
    fn test_execute_tool_request_deserialization() {
        let req: ExecuteToolRequest = serde_json::from_str(
            r#"{"params":{"case":"ieee14"}}"#,
        ).unwrap();
        assert_eq!(req.params["case"], "ieee14");
    }

    #[test]
    fn test_execute_tool_response_serialization() {
        let resp = ExecuteToolResponse {
            success: true,
            data: serde_json::json!({"result": "converged"}),
            message: "ok".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"success\":true"));
    }
}
