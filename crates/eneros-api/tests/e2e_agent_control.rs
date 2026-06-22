//! T029-08: Agent 控制 API 集成测试
//!
//! 测试 `POST /api/agents/{id}/control` 端点的 5 个动作（start/stop/pause/resume/status）
//! 以及错误场景（404 Agent 不存在、400 非法动作、400 非法状态转换、503 控制器未配置）。
//!
//! 这些测试使用真实的 `AgentController`（非 mock），验证：
//! - 状态转换真实生效（start 真实启动 tokio 任务，stop 真实停止）
//! - 状态机合法性约束（Stopped 不能 pause，Paused 不能直接 pause 等）
//! - HTTP 状态码和响应体格式正确

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use eneros_runtime::agent::AgentController;
use eneros_api::app::{create_router, AppState};

// ---------------------------------------------------------------------------
// 测试辅助函数
// ---------------------------------------------------------------------------

/// 构建一个注入了 AgentController 的 AppState，注册 6 个 Agent
fn build_state_with_controller() -> AppState {
    let controller = AgentController::new();
    controller.register("dispatch-1", "Dispatcher");
    controller.register("operation-1", "Operator");
    controller.register("self-healing-1", "SelfHealing");
    controller.register("forecast-1", "Forecaster");
    controller.register("planning-1", "Planner");
    controller.register("trading-1", "Trader");
    AppState::new().with_agent_controller(controller)
}

/// 构建一个未注入 AgentController 的 AppState（用于测试 503）
fn build_state_without_controller() -> AppState {
    AppState::new()
}

/// 发送 control 请求并返回 (StatusCode, 响应体 JSON)
async fn send_control_request(
    app: axum::Router,
    agent_id: &str,
    action: &str,
) -> (StatusCode, serde_json::Value) {
    let uri = format!("/api/agents/{}/control", agent_id);
    let body = serde_json::json!({ "action": action });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = if body_bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body_bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

// ---------------------------------------------------------------------------
// 测试 1: start 动作 — Stopped → Running
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_control_start_stopped_to_running() {
    let state = build_state_with_controller();
    let app = create_router(state);

    let (status, json) = send_control_request(app, "dispatch-1", "start").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["agent_id"], "dispatch-1");
    assert_eq!(json["action"], "start");
    assert_eq!(json["previous_state"], "stopped");
    assert_eq!(json["current_state"], "running");
    assert!(json["timestamp"].is_string());
}

// ---------------------------------------------------------------------------
// 测试 2: stop 动作 — Running → Stopped
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_control_stop_running_to_stopped() {
    let state = build_state_with_controller();
    // 先 start
    state
        .agent_controller
        .as_ref()
        .unwrap()
        .control("dispatch-1", eneros_runtime::agent::ControlCommand::Start)
        .await
        .unwrap();
    let app = create_router(state);

    let (status, json) = send_control_request(app, "dispatch-1", "stop").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["agent_id"], "dispatch-1");
    assert_eq!(json["action"], "stop");
    assert_eq!(json["previous_state"], "running");
    assert_eq!(json["current_state"], "stopped");
}

// ---------------------------------------------------------------------------
// 测试 3: pause 动作 — Running → Paused
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_control_pause_running_to_paused() {
    let state = build_state_with_controller();
    state
        .agent_controller
        .as_ref()
        .unwrap()
        .control("dispatch-1", eneros_runtime::agent::ControlCommand::Start)
        .await
        .unwrap();
    let app = create_router(state);

    let (status, json) = send_control_request(app, "dispatch-1", "pause").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["agent_id"], "dispatch-1");
    assert_eq!(json["action"], "pause");
    assert_eq!(json["previous_state"], "running");
    assert_eq!(json["current_state"], "paused");
}

// ---------------------------------------------------------------------------
// 测试 4: resume 动作 — Paused → Running
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_control_resume_paused_to_running() {
    let state = build_state_with_controller();
    let controller = state.agent_controller.as_ref().unwrap();
    controller
        .control("dispatch-1", eneros_runtime::agent::ControlCommand::Start)
        .await
        .unwrap();
    controller
        .control("dispatch-1", eneros_runtime::agent::ControlCommand::Pause)
        .await
        .unwrap();
    let app = create_router(state);

    let (status, json) = send_control_request(app, "dispatch-1", "resume").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["agent_id"], "dispatch-1");
    assert_eq!(json["action"], "resume");
    assert_eq!(json["previous_state"], "paused");
    assert_eq!(json["current_state"], "running");
}

// ---------------------------------------------------------------------------
// 测试 5: status 动作 — 返回当前状态
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_control_status_returns_current_state() {
    let state = build_state_with_controller();
    let app = create_router(state.clone());

    // 初始状态为 stopped
    let (status, json) = send_control_request(app, "dispatch-1", "status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["agent_id"], "dispatch-1");
    assert_eq!(json["action"], "status");
    assert_eq!(json["previous_state"], "stopped");
    assert_eq!(json["current_state"], "stopped");
}

#[tokio::test]
async fn test_control_status_after_start() {
    let state = build_state_with_controller();
    state
        .agent_controller
        .as_ref()
        .unwrap()
        .control("dispatch-1", eneros_runtime::agent::ControlCommand::Start)
        .await
        .unwrap();
    let app = create_router(state);

    let (status, json) = send_control_request(app, "dispatch-1", "status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["previous_state"], "running");
    assert_eq!(json["current_state"], "running");
}

// ---------------------------------------------------------------------------
// 测试 6: 非法动作 — 返回 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_control_invalid_action_returns_400() {
    let state = build_state_with_controller();
    let app = create_router(state);

    let (status, _json) = send_control_request(app, "dispatch-1", "restart").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_control_empty_action_returns_400() {
    let state = build_state_with_controller();
    let app = create_router(state);

    let (status, _json) = send_control_request(app, "dispatch-1", "").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// 测试 7: 不存在的 Agent — 返回 404
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_control_nonexistent_agent_returns_404() {
    let state = build_state_with_controller();
    let app = create_router(state);

    let (status, _json) = send_control_request(app, "nonexistent-agent", "status").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_control_nonexistent_agent_start_returns_404() {
    let state = build_state_with_controller();
    let app = create_router(state);

    let (status, _json) = send_control_request(app, "ghost-agent", "start").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// 测试 8: 非法状态转换 — 返回 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_control_pause_when_stopped_returns_400() {
    let state = build_state_with_controller();
    let app = create_router(state);

    // Stopped 状态下 pause 应返回 400
    let (status, _json) = send_control_request(app, "dispatch-1", "pause").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_control_resume_when_stopped_returns_400() {
    let state = build_state_with_controller();
    let app = create_router(state);

    // Stopped 状态下 resume 应返回 400
    let (status, _json) = send_control_request(app, "dispatch-1", "resume").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_control_stop_when_stopped_returns_400() {
    let state = build_state_with_controller();
    let app = create_router(state);

    // Stopped 状态下 stop 应返回 400
    let (status, _json) = send_control_request(app, "dispatch-1", "stop").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_control_start_when_running_returns_400() {
    let state = build_state_with_controller();
    state
        .agent_controller
        .as_ref()
        .unwrap()
        .control("dispatch-1", eneros_runtime::agent::ControlCommand::Start)
        .await
        .unwrap();
    let app = create_router(state);

    // Running 状态下 start 应返回 400
    let (status, _json) = send_control_request(app, "dispatch-1", "start").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_control_pause_when_paused_returns_400() {
    let state = build_state_with_controller();
    let controller = state.agent_controller.as_ref().unwrap();
    controller
        .control("dispatch-1", eneros_runtime::agent::ControlCommand::Start)
        .await
        .unwrap();
    controller
        .control("dispatch-1", eneros_runtime::agent::ControlCommand::Pause)
        .await
        .unwrap();
    let app = create_router(state);

    // Paused 状态下 pause 应返回 400
    let (status, _json) = send_control_request(app, "dispatch-1", "pause").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_control_resume_when_running_returns_400() {
    let state = build_state_with_controller();
    state
        .agent_controller
        .as_ref()
        .unwrap()
        .control("dispatch-1", eneros_runtime::agent::ControlCommand::Start)
        .await
        .unwrap();
    let app = create_router(state);

    // Running 状态下 resume 应返回 400
    let (status, _json) = send_control_request(app, "dispatch-1", "resume").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// 测试 9: AgentController 未配置 — 返回 503
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_control_without_controller_returns_503() {
    let state = build_state_without_controller();
    let app = create_router(state);

    let (status, _json) = send_control_request(app, "dispatch-1", "status").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

// ---------------------------------------------------------------------------
// 测试 10: 完整生命周期 — Stopped → Running → Paused → Running → Stopped
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_control_full_lifecycle_via_api() {
    let state = build_state_with_controller();
    let controller = state.agent_controller.as_ref().unwrap().clone();
    let app = create_router(state);

    // 1. status: stopped
    let (status, json) = send_control_request(app.clone(), "dispatch-1", "status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["current_state"], "stopped");

    // 2. start: stopped → running
    let (status, json) = send_control_request(app.clone(), "dispatch-1", "start").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["previous_state"], "stopped");
    assert_eq!(json["current_state"], "running");

    // 3. pause: running → paused
    let (status, json) = send_control_request(app.clone(), "dispatch-1", "pause").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["previous_state"], "running");
    assert_eq!(json["current_state"], "paused");

    // 4. resume: paused → running
    let (status, json) = send_control_request(app.clone(), "dispatch-1", "resume").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["previous_state"], "paused");
    assert_eq!(json["current_state"], "running");

    // 5. stop: running → stopped
    let (status, json) = send_control_request(app.clone(), "dispatch-1", "stop").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["previous_state"], "running");
    assert_eq!(json["current_state"], "stopped");

    // 6. status: stopped
    let (status, json) = send_control_request(app, "dispatch-1", "status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["current_state"], "stopped");

    // 清理：确保 controller 内部状态一致
    let _ = controller;
}

// ---------------------------------------------------------------------------
// 测试 11: stop from paused — Paused → Stopped
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_control_stop_from_paused() {
    let state = build_state_with_controller();
    let controller = state.agent_controller.as_ref().unwrap();
    controller
        .control("dispatch-1", eneros_runtime::agent::ControlCommand::Start)
        .await
        .unwrap();
    controller
        .control("dispatch-1", eneros_runtime::agent::ControlCommand::Pause)
        .await
        .unwrap();
    let app = create_router(state);

    // Paused 状态下 stop 应成功
    let (status, json) = send_control_request(app, "dispatch-1", "stop").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["previous_state"], "paused");
    assert_eq!(json["current_state"], "stopped");
}

// ---------------------------------------------------------------------------
// 测试 12: 大小写不敏感 — action 大小写均可
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_control_action_case_insensitive() {
    let state = build_state_with_controller();
    let app = create_router(state);

    // 大写 START
    let (status, json) = send_control_request(app.clone(), "dispatch-1", "START").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["action"], "start");
    assert_eq!(json["current_state"], "running");

    // 大写 STATUS
    let (status, json) = send_control_request(app.clone(), "dispatch-1", "STATUS").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["action"], "status");
    assert_eq!(json["current_state"], "running");

    // 混合大小写 Stop
    let (status, json) = send_control_request(app, "dispatch-1", "Stop").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["action"], "stop");
    assert_eq!(json["current_state"], "stopped");
}

// ---------------------------------------------------------------------------
// 测试 13: 多个 Agent 独立控制
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_control_multiple_agents_independent() {
    let state = build_state_with_controller();
    let app = create_router(state);

    // dispatch-1: start
    let (status, json) = send_control_request(app.clone(), "dispatch-1", "start").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["current_state"], "running");

    // operation-1: status (应仍为 stopped)
    let (status, json) = send_control_request(app.clone(), "operation-1", "status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["current_state"], "stopped");

    // operation-1: start
    let (status, json) = send_control_request(app.clone(), "operation-1", "start").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["current_state"], "running");

    // dispatch-1: status (应仍为 running)
    let (status, json) = send_control_request(app.clone(), "dispatch-1", "status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["current_state"], "running");

    // 清理
    let (_status, _json) = send_control_request(app.clone(), "dispatch-1", "stop").await;
    let (_status, _json) = send_control_request(app, "operation-1", "stop").await;
}

// ---------------------------------------------------------------------------
// 测试 14: 响应体格式验证
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_control_response_format() {
    let state = build_state_with_controller();
    let app = create_router(state);

    let (status, json) = send_control_request(app, "dispatch-1", "status").await;
    assert_eq!(status, StatusCode::OK);

    // 验证响应体包含所有必需字段
    assert!(json["agent_id"].is_string());
    assert!(json["action"].is_string());
    assert!(json["previous_state"].is_string());
    assert!(json["current_state"].is_string());
    assert!(json["timestamp"].is_string());

    // 验证 timestamp 是有效的 RFC 3339 时间戳
    let timestamp = json["timestamp"].as_str().unwrap();
    assert!(chrono::DateTime::parse_from_rfc3339(timestamp).is_ok());
}
