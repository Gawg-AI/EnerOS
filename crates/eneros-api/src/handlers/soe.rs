//! SOE (Sequence of Events) API handlers (v0.10.0 — Task 4).

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::app::AppState;

/// Query parameters for `GET /api/soe`.
#[derive(Debug, Deserialize, IntoParams)]
pub struct SoeQueryParams {
    /// Start time (RFC3339). Defaults to 1 hour ago.
    pub start: Option<String>,
    /// End time (RFC3339). Defaults to now.
    pub end: Option<String>,
    /// Filter by device id.
    pub device_id: Option<String>,
    /// Filter by event type (`breaker_open` / `breaker_close` / `protection_trip` / `alarm` / `manual`).
    pub event_type: Option<String>,
    /// Maximum number of records to return (applies the most recent `limit`).
    pub limit: Option<usize>,
}

/// Query parameters for `GET /api/soe/latest`.
#[derive(Debug, Deserialize, IntoParams)]
pub struct SoeLatestParams {
    /// Maximum number of records to return. Defaults to 100.
    pub limit: Option<usize>,
}

/// Response envelope for SOE queries.
#[derive(Debug, Serialize, ToSchema)]
pub struct SoeResponse {
    pub success: bool,
    pub count: usize,
    pub data: Vec<eneros_timeseries::SoeRecord>,
    pub error: Option<String>,
}

/// `GET /api/soe` — query SOE events by time range with optional filters.
#[utoipa::path(
    get,
    path = "/api/soe",
    params(SoeQueryParams),
    responses(
        (status = 200, description = "SOE 事件查询结果", body = SoeResponse),
        (status = 400, description = "请求参数错误"),
        (status = 503, description = "SOE 记录器未配置"),
    )
)]
pub async fn query_handler(
    State(state): State<AppState>,
    Query(params): Query<SoeQueryParams>,
) -> axum::response::Response {
    let recorder = match &state.soe_recorder {
        Some(r) => r,
        None => {
            let resp = SoeResponse {
                success: false,
                count: 0,
                data: Vec::new(),
                error: Some("soe recorder not configured".to_string()),
            };
            return (StatusCode::SERVICE_UNAVAILABLE, Json(resp)).into_response();
        }
    };

    let end = match &params.end {
        Some(t) => match chrono::DateTime::parse_from_rfc3339(t) {
            Ok(dt) => dt.with_timezone(&chrono::Utc),
            Err(_) => {
                let resp = SoeResponse {
                    success: false,
                    count: 0,
                    data: Vec::new(),
                    error: Some("invalid end time format".to_string()),
                };
                return (StatusCode::BAD_REQUEST, Json(resp)).into_response();
            }
        },
        None => chrono::Utc::now(),
    };
    let start = match &params.start {
        Some(t) => match chrono::DateTime::parse_from_rfc3339(t) {
            Ok(dt) => dt.with_timezone(&chrono::Utc),
            Err(_) => {
                let resp = SoeResponse {
                    success: false,
                    count: 0,
                    data: Vec::new(),
                    error: Some("invalid start time format".to_string()),
                };
                return (StatusCode::BAD_REQUEST, Json(resp)).into_response();
            }
        },
        None => end - chrono::Duration::hours(1),
    };

    let event_type = match &params.event_type {
        Some(s) => match eneros_timeseries::SoeEventType::from_str(s) {
            Some(et) => Some(et),
            None => {
                let resp = SoeResponse {
                    success: false,
                    count: 0,
                    data: Vec::new(),
                    error: Some(format!("unknown event_type '{}'", s)),
                };
                return (StatusCode::BAD_REQUEST, Json(resp)).into_response();
            }
        },
        None => None,
    };

    let mut records = match recorder.query(
        start,
        end,
        params.device_id.as_deref(),
        event_type.as_ref(),
    ) {
        Ok(r) => r,
        Err(e) => {
            let resp = SoeResponse {
                success: false,
                count: 0,
                data: Vec::new(),
                error: Some(e),
            };
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(resp)).into_response();
        }
    };

    // If a limit is provided, keep only the most recent `limit` records.
    if let Some(limit) = params.limit {
        if records.len() > limit {
            let split_at = records.len() - limit;
            records.drain(0..split_at);
        }
    }

    let count = records.len();
    let resp = SoeResponse {
        success: true,
        count,
        data: records,
        error: None,
    };
    (StatusCode::OK, Json(resp)).into_response()
}

/// `GET /api/soe/latest` — return the most recent SOE events.
#[utoipa::path(
    get,
    path = "/api/soe/latest",
    params(SoeLatestParams),
    responses(
        (status = 200, description = "最近 SOE 事件", body = SoeResponse),
        (status = 503, description = "SOE 记录器未配置"),
    )
)]
pub async fn latest_handler(
    State(state): State<AppState>,
    Query(params): Query<SoeLatestParams>,
) -> axum::response::Response {
    let recorder = match &state.soe_recorder {
        Some(r) => r,
        None => {
            let resp = SoeResponse {
                success: false,
                count: 0,
                data: Vec::new(),
                error: Some("soe recorder not configured".to_string()),
            };
            return (StatusCode::SERVICE_UNAVAILABLE, Json(resp)).into_response();
        }
    };

    let limit = params.limit.unwrap_or(100);

    match recorder.latest(limit) {
        Ok(data) => {
            let count = data.len();
            let resp = SoeResponse {
                success: true,
                count,
                data,
                error: None,
            };
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => {
            let resp = SoeResponse {
                success: false,
                count: 0,
                data: Vec::new(),
                error: Some(e),
            };
            (StatusCode::INTERNAL_SERVER_ERROR, Json(resp)).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use tower::util::ServiceExt;

    fn app_with_recorder(recorder: Option<Arc<eneros_timeseries::SoeRecorder>>) -> axum::Router {
        let mut state = AppState::new();
        if let Some(r) = recorder {
            state = state.with_soe_recorder(r);
        }
        crate::app::create_router(state)
    }

    #[tokio::test]
    async fn test_soe_handler_no_recorder() {
        let app = app_with_recorder(None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/soe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(json["success"], false);
        assert_eq!(json["count"], 0);
        assert!(json["error"].as_str().unwrap().contains("not configured"));
    }

    #[tokio::test]
    async fn test_soe_handler_query() {
        let recorder = Arc::new(eneros_timeseries::SoeRecorder::new_memory());
        // Seed a couple of events.
        recorder
            .record_now("dev1", eneros_timeseries::SoeEventType::BreakerOpen, 1, "1 -> 0")
            .unwrap();
        recorder
            .record_now("dev2", eneros_timeseries::SoeEventType::Alarm, 2, "overload")
            .unwrap();

        let app = app_with_recorder(Some(recorder.clone()));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/soe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(json["success"], true);
        assert_eq!(json["count"], 2);
        assert_eq!(json["data"][0]["device_id"], "dev1");
        assert_eq!(json["data"][1]["device_id"], "dev2");
    }

    #[tokio::test]
    async fn test_soe_handler_latest_default_limit() {
        let recorder = Arc::new(eneros_timeseries::SoeRecorder::new_memory());
        for i in 0..5 {
            recorder
                .record_now("d", eneros_timeseries::SoeEventType::Manual, 1, &format!("v{}", i))
                .unwrap();
        }

        let app = app_with_recorder(Some(recorder.clone()));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/soe/latest")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(json["success"], true);
        assert_eq!(json["count"], 5);
        // Newest first: sequence_number 4
        assert_eq!(json["data"][0]["sequence_number"], 4);
    }

    #[tokio::test]
    async fn test_soe_handler_latest_with_limit() {
        let recorder = Arc::new(eneros_timeseries::SoeRecorder::new_memory());
        for i in 0..10 {
            recorder
                .record_now("d", eneros_timeseries::SoeEventType::Manual, 1, &format!("v{}", i))
                .unwrap();
        }

        let app = app_with_recorder(Some(recorder.clone()));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/soe/latest?limit=3")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(json["count"], 3);
        assert_eq!(json["data"][0]["sequence_number"], 9);
        assert_eq!(json["data"][2]["sequence_number"], 7);
    }

    #[tokio::test]
    async fn test_soe_handler_bad_event_type() {
        let recorder = Arc::new(eneros_timeseries::SoeRecorder::new_memory());
        let app = app_with_recorder(Some(recorder));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/soe?event_type=unknown")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
