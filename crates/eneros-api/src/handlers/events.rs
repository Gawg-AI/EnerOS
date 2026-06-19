//! Events API handlers (v0.6.0 — S4).

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::app::AppState;

/// Request body for publishing an event.
#[derive(Debug, Deserialize)]
pub struct PublishEventRequest {
    pub source: String,
    pub message: String,
}

/// Response for event publication.
#[derive(Debug, Serialize)]
pub struct PublishEventResponse {
    pub event_id: String,
    pub published: bool,
}

/// `POST /api/events/publish` — publish a message event to the event bus.
pub async fn publish_handler(
    State(state): State<AppState>,
    Json(req): Json<PublishEventRequest>,
) -> axum::response::Response {
    let event_bus = match &state.event_bus {
        Some(b) => b,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "event bus not configured").into_response(),
    };

    let event = eneros_eventbus::Event::new(
        eneros_eventbus::event::EventType::SystemAlarm,
        &req.source,
        eneros_eventbus::event::EventPayload::Message(req.message.clone()),
    );
    let event_id = event.id.clone();

    match event_bus.publish(event) {
        Ok(_) => {
            // Also broadcast to WebSocket clients
            crate::app::broadcast_event(&state, &format!(
                r#"{{"type":"event","source":"{}","message":"{}"}}"#,
                req.source, req.message
            ));
            let response = PublishEventResponse {
                event_id,
                published: true,
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            tracing::error!("event publish failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "event publish failed").into_response()
        }
    }
}

/// `GET /api/events/stats` — get event bus statistics.
pub async fn stats_handler(
    State(state): State<AppState>,
) -> axum::response::Response {
    let event_bus = match &state.event_bus {
        Some(b) => b,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "event bus not configured").into_response(),
    };

    let response = serde_json::json!({
        "handler_count": event_bus.handler_count(),
        "capacity": event_bus.capacity(),
        "lagged_messages": event_bus.lagged_message_count(),
        "dispatch_loop_running": event_bus.is_dispatch_loop_running(),
        "dispatch_loop_healthy": event_bus.is_dispatch_loop_healthy(),
    });

    (StatusCode::OK, Json(response)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_publish_event_request_deserialization() {
        let req: PublishEventRequest = serde_json::from_str(
            r#"{"source":"test","message":"hello world"}"#,
        ).unwrap();
        assert_eq!(req.source, "test");
        assert_eq!(req.message, "hello world");
    }

    #[test]
    fn test_publish_event_response_serialization() {
        let resp = PublishEventResponse {
            event_id: "evt-123".to_string(),
            published: true,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"published\":true"));
    }
}
