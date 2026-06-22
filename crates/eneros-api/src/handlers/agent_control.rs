//! Agent 控制 API handler (T029-08)。
//!
//! 暴露 Agent 生命周期控制（start/stop/pause/resume/status）通过
//! `POST /api/agents/{id}/control`。Handler 委托给 `AgentController`
//! 进行真实的状态转换和任务管理。
//!
//! T029-06: 从请求扩展中提取 trace_id（由 T029-04 中间件注入），
//! 贯穿到控制日志和响应体，便于调用方在日志中关联整条 Agent 执行链路。
//!
//! ## 请求
//!
//! ```json
//! POST /api/agents/{id}/control
//! {"action": "start|stop|pause|resume|status"}
//! ```
//!
//! ## 响应
//!
//! ```json
//! {
//!   "agent_id": "dispatch-1",
//!   "previous_state": "stopped",
//!   "current_state": "running",
//!   "action": "start",
//!   "timestamp": "2026-06-21T10:00:00Z",
//!   "trace_id": "550e8400-e29b-41d4-a716-446655440000"
//! }
//! ```
//!
//! ## 错误
//!
//! - `400 Bad Request`: 无效的 action 或非法状态转换
//! - `404 Not Found`: Agent 未注册
//! - `503 Service Unavailable`: AgentController 未配置

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use eneros_runtime::agent::{AgentLifecycleState, ControlCommand, ControlError};

use crate::app::AppState;
use crate::middleware::TraceId;

/// 请求体：`POST /api/agents/{id}/control`
#[derive(Debug, Deserialize, ToSchema)]
pub struct AgentControlRequest {
    /// 控制动作：`start` | `stop` | `pause` | `resume` | `status`
    pub action: String,
}

/// 响应体：`POST /api/agents/{id}/control`
#[derive(Debug, Serialize, ToSchema)]
pub struct AgentControlResponse {
    /// Agent ID
    pub agent_id: String,
    /// 执行的动作
    pub action: String,
    /// 执行前的状态
    pub previous_state: String,
    /// 执行后的状态
    pub current_state: String,
    /// ISO 8601 时间戳（UTC）
    pub timestamp: String,
    /// 分布式追踪 ID（T029-06）。
    ///
    /// 与请求的 `X-Trace-Id` 响应头一致，便于调用方在日志中
    /// 关联整条 Agent 执行链路。
    pub trace_id: String,
}

/// `POST /api/agents/{id}/control` — 控制 Agent 的生命周期。
///
/// 支持的动作：`start`、`stop`、`pause`、`resume`、`status`。
/// 状态转换必须合法，否则返回 400。
///
/// trace_id 从请求扩展中提取（T029-04 中间件注入），并贯穿到
/// 控制日志和响应体（T029-06）。
#[utoipa::path(
    post,
    path = "/api/agents/{id}/control",
    params(("id" = String, Path, description = "Agent ID")),
    request_body = AgentControlRequest,
    responses(
        (status = 200, description = "Agent 控制结果", body = AgentControlResponse),
        (status = 400, description = "无效的控制动作或非法状态转换"),
        (status = 404, description = "Agent 未找到"),
        (status = 503, description = "AgentController 未配置"),
    )
)]
pub async fn control_handler(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Extension(trace_id_ext): Extension<TraceId>,
    Json(req): Json<AgentControlRequest>,
) -> axum::response::Response {
    // T029-06: 从请求扩展提取 trace_id（由 T029-04 中间件注入）
    let trace_id = trace_id_ext.0;

    // 1. 检查 AgentController 是否配置
    let controller = match &state.agent_controller {
        Some(c) => c.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "agent controller not configured",
            )
                .into_response()
        }
    };

    // 2. 解析并校验 action
    let action_lower = req.action.to_lowercase();
    let command = match ControlCommand::parse_action(&action_lower) {
        Some(cmd) => cmd,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                format!(
                    "invalid action '{}': must be one of start/stop/pause/resume/status",
                    req.action
                ),
            )
                .into_response()
        }
    };

    // T029-06: 在控制操作前记录日志，携带 trace_id
    tracing::info!(
        agent_id = %agent_id,
        action = %action_lower,
        trace_id = %trace_id,
        "agent control request received"
    );

    // 3. 委托给 AgentController 执行
    match controller.control(&agent_id, command).await {
        Ok(result) => {
            // T029-06: 控制成功后记录日志，携带 trace_id 和状态转换
            tracing::info!(
                agent_id = %agent_id,
                action = %action_lower,
                trace_id = %trace_id,
                previous_state = %result.previous_state,
                current_state = %result.current_state,
                "agent control request completed"
            );
            let response = AgentControlResponse {
                agent_id: agent_id.clone(),
                action: action_lower,
                previous_state: result.previous_state.to_string(),
                current_state: result.current_state.to_string(),
                timestamp: Utc::now().to_rfc3339(),
                trace_id,
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(ControlError::NotFound) => {
            tracing::warn!(
                agent_id = %agent_id,
                trace_id = %trace_id,
                "agent not found"
            );
            (
                StatusCode::NOT_FOUND,
                format!("agent '{}' not found", agent_id),
            )
                .into_response()
        }
        Err(ControlError::InvalidTransition { from, command }) => {
            tracing::warn!(
                agent_id = %agent_id,
                trace_id = %trace_id,
                from = %from,
                command = %command,
                "invalid state transition"
            );
            (
                StatusCode::BAD_REQUEST,
                format!(
                    "invalid state transition: cannot {} from {}",
                    command, from
                ),
            )
                .into_response()
        }
    }
}

/// 将 `AgentLifecycleState` 转换为字符串（用于序列化/响应）
#[allow(dead_code)]
fn state_to_string(state: AgentLifecycleState) -> String {
    state.to_string()
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
    fn test_agent_control_request_deserialize_all_actions() {
        for action in ["start", "stop", "pause", "resume", "status"] {
            let json = format!(r#"{{"action": "{}"}}"#, action);
            let req: AgentControlRequest = serde_json::from_str(&json).unwrap();
            assert_eq!(req.action, action);
        }
    }

    #[test]
    fn test_agent_control_response_serialization() {
        let resp = AgentControlResponse {
            agent_id: "dispatch-1".to_string(),
            action: "stop".to_string(),
            previous_state: "running".to_string(),
            current_state: "stopped".to_string(),
            timestamp: "2026-06-21T10:00:00+00:00".to_string(),
            trace_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"agent_id\":\"dispatch-1\""));
        assert!(json.contains("\"action\":\"stop\""));
        assert!(json.contains("\"previous_state\":\"running\""));
        assert!(json.contains("\"current_state\":\"stopped\""));
        assert!(json.contains("\"timestamp\""));
        assert!(json.contains("\"trace_id\":\"550e8400-e29b-41d4-a716-446655440000\""));
    }

    #[test]
    fn test_state_to_string_all_variants() {
        assert_eq!(state_to_string(AgentLifecycleState::Stopped), "stopped");
        assert_eq!(state_to_string(AgentLifecycleState::Running), "running");
        assert_eq!(state_to_string(AgentLifecycleState::Paused), "paused");
        assert_eq!(state_to_string(AgentLifecycleState::Error), "error");
    }
}
