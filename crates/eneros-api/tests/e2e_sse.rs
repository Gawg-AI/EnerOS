//! T029-11: Dashboard SSE 实时刷新 — 集成测试。
//!
//! 验证 `GET /api/v1/dashboard/stream` SSE 端点的行为：
//! - 返回正确的 Content-Type (`text/event-stream`)
//! - SSE 消息格式 (`event: metric\ndata: {...}\n\n`)
//! - 事件推送：发布 EventBus 事件后，SSE 流应收到对应的 metric 消息
//! - 无 EventBus 时连接仍可建立（KeepAlive 维持）

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::StreamExt;
use tower::ServiceExt;

use eneros_api::app::{create_router, AppState};
use eneros_runtime::eventbus::EventBus;
use eneros_runtime::eventbus::event::{EventPayload, EventType};
use eneros_runtime::eventbus::Event;

/// 验证 SSE 端点返回 200 且 Content-Type 为 `text/event-stream`。
#[tokio::test]
async fn test_sse_returns_correct_content_type() {
    let state = AppState::new();
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/dashboard/stream")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .expect("响应应包含 content-type 头")
        .to_str()
        .unwrap();
    assert!(
        content_type.starts_with("text/event-stream"),
        "content-type 应为 text/event-stream，实际为: {}",
        content_type
    );
}

/// 验证无 EventBus 时 SSE 连接仍可建立（流保持空闲，不产生错误）。
#[tokio::test]
async fn test_sse_without_event_bus_stays_open() {
    let state = AppState::new();
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/dashboard/stream")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // 轮询流 200ms，不应收到任何数据（流应保持空闲）
    let mut body_stream = response.into_body().into_data_stream();
    let result = tokio::time::timeout(
        tokio::time::Duration::from_millis(200),
        body_stream.next(),
    )
    .await;

    // 超时表示流保持空闲（未产生数据），这是预期行为
    assert!(
        result.is_err(),
        "无 EventBus 时流应保持空闲，不应产生数据"
    );
}

/// 验证 SSE 流正确推送 EventBus 事件，且消息格式符合 SSE 规范。
#[tokio::test]
async fn test_sse_receives_event_bus_events() {
    let event_bus = Arc::new(EventBus::new(16));
    let state = AppState::new().with_event_bus(event_bus.clone());
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/dashboard/stream")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let mut body_stream = response.into_body().into_data_stream();

    // 先轮询流一小段时间，确保 SSE handler 已订阅 EventBus
    // （async_stream 是惰性的，订阅在首次 poll 时发生）
    let _ = tokio::time::timeout(
        tokio::time::Duration::from_millis(100),
        body_stream.next(),
    )
    .await;

    // 发布一个事件到 EventBus
    let event = Event::new(
        EventType::SystemAlarm,
        "sse-test-source",
        EventPayload::Message("hello-sse".to_string()),
    );
    event_bus.publish(event).expect("发布事件应成功");

    // 从 SSE 流读取数据，直到收到完整的 metric 消息
    let mut buffer = String::new();
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
    while let Ok(Some(Ok(bytes))) = tokio::time::timeout_at(deadline, body_stream.next()).await {
        buffer.push_str(&String::from_utf8_lossy(&bytes));
        if buffer.contains("event: metric") && buffer.contains("hello-sse") {
            break;
        }
    }

    // 验证 SSE 消息格式
    assert!(
        buffer.contains("event: metric"),
        "SSE 消息应包含 'event: metric' 行，实际内容: {}",
        buffer
    );
    assert!(
        buffer.contains("data: "),
        "SSE 消息应包含 'data: ' 行，实际内容: {}",
        buffer
    );
    // 验证事件内容
    assert!(
        buffer.contains("SystemAlarm"),
        "SSE 消息应包含事件类型 SystemAlarm"
    );
    assert!(
        buffer.contains("sse-test-source"),
        "SSE 消息应包含事件源 sse-test-source"
    );
    assert!(
        buffer.contains("hello-sse"),
        "SSE 消息应包含事件负载 hello-sse"
    );
    assert!(
        buffer.contains("\"type\":\"event\""),
        "SSE 消息 JSON 应包含 type:event 字段（与 WS 桥接格式一致）"
    );
}

/// 验证 SSE 流推送多个事件，且每个事件格式正确。
#[tokio::test]
async fn test_sse_receives_multiple_events() {
    let event_bus = Arc::new(EventBus::new(64));
    let state = AppState::new().with_event_bus(event_bus.clone());
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/dashboard/stream")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let mut body_stream = response.into_body().into_data_stream();

    // 确保 SSE handler 已订阅
    let _ = tokio::time::timeout(
        tokio::time::Duration::from_millis(100),
        body_stream.next(),
    )
    .await;

    // 发布多个事件
    let events = vec![
        Event::new(
            EventType::DeviceConnected,
            "device-mgr",
            EventPayload::DeviceEvent {
                device_id: "rtu-1".to_string(),
                event_type: "connected".to_string(),
            },
        ),
        Event::new(
            EventType::DataReceived,
            "scada",
            EventPayload::Message("data-update".to_string()),
        ),
        Event::new(
            EventType::ConstraintViolation,
            "constraint-engine",
            EventPayload::Message("violation-1".to_string()),
        ),
    ];

    for event in &events {
        event_bus.publish(event.clone()).expect("发布事件应成功");
    }

    // 读取所有三个事件
    let mut buffer = String::new();
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
    while let Ok(Some(Ok(bytes))) = tokio::time::timeout_at(deadline, body_stream.next()).await {
        buffer.push_str(&String::from_utf8_lossy(&bytes));
        // 等待收到全部三个事件
        let metric_count = buffer.matches("event: metric").count();
        if metric_count >= 3 {
            break;
        }
    }

    // 验证收到三条 metric 消息
    let metric_count = buffer.matches("event: metric").count();
    assert!(
        metric_count >= 3,
        "应收到至少 3 条 metric 消息，实际收到 {} 条",
        metric_count
    );

    // 验证每个事件类型都出现
    assert!(buffer.contains("DeviceConnected"), "应包含 DeviceConnected 事件");
    assert!(buffer.contains("DataReceived"), "应包含 DataReceived 事件");
    assert!(buffer.contains("ConstraintViolation"), "应包含 ConstraintViolation 事件");
}

/// 验证 SSE 消息中 data 行的 JSON 是有效的 JSON 对象。
#[tokio::test]
async fn test_sse_data_is_valid_json() {
    let event_bus = Arc::new(EventBus::new(16));
    let state = AppState::new().with_event_bus(event_bus.clone());
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/dashboard/stream")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let mut body_stream = response.into_body().into_data_stream();

    // 确保 SSE handler 已订阅
    let _ = tokio::time::timeout(
        tokio::time::Duration::from_millis(100),
        body_stream.next(),
    )
    .await;

    let event = Event::new(
        EventType::PowerFlowConverged,
        "powerflow-solver",
        EventPayload::Message("converged".to_string()),
    );
    event_bus.publish(event).expect("发布事件应成功");

    let mut buffer = String::new();
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
    while let Ok(Some(Ok(bytes))) = tokio::time::timeout_at(deadline, body_stream.next()).await {
        buffer.push_str(&String::from_utf8_lossy(&bytes));
        if buffer.contains("PowerFlowConverged") {
            break;
        }
    }

    // 提取 data 行的 JSON 内容并验证
    let data_line = buffer
        .lines()
        .find(|line| line.starts_with("data: "))
        .expect("应包含 data: 行");

    let json_str = &data_line["data: ".len()..];
    let parsed: serde_json::Value = serde_json::from_str(json_str)
        .unwrap_or_else(|e| panic!("data 行应为有效 JSON: {} (内容: {})", e, json_str));

    // 验证 JSON 结构
    assert_eq!(parsed["type"], "event");
    assert!(parsed["event_type"].is_string());
    assert!(parsed["id"].is_string());
    assert!(parsed["timestamp"].is_string());
    assert!(parsed["source"].is_string());
    assert!(parsed["payload"].is_object());
}
