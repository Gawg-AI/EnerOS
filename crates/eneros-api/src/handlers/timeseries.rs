//! TimeSeries API handlers (v0.6.0 — S4).

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::app::AppState;

/// Query parameters for `GET /api/timeseries/query`.
#[derive(Debug, Deserialize, IntoParams)]
pub struct TimeseriesQueryParams {
    pub element_id: u64,
    pub parameter: String,
    /// Start time (RFC3339). Defaults to 1 hour ago.
    pub start: Option<String>,
    /// End time (RFC3339). Defaults to now.
    pub end: Option<String>,
}

/// Response for timeseries query.
#[derive(Debug, Serialize, ToSchema)]
pub struct TimeseriesResponse {
    pub element_id: u64,
    pub parameter: String,
    pub points: Vec<DataPointDto>,
}

/// DTO for a single data point.
#[derive(Debug, Serialize, ToSchema)]
pub struct DataPointDto {
    pub timestamp: String,
    pub value: f64,
    pub quality: String,
}

/// `GET /api/timeseries/query` — query historical time series data.
#[utoipa::path(
    get,
    path = "/api/timeseries/query",
    params(TimeseriesQueryParams),
    responses(
        (status = 200, description = "时间序列查询结果", body = TimeseriesResponse),
        (status = 400, description = "请求参数错误"),
        (status = 503, description = "时序引擎未配置"),
    )
)]
pub async fn query_handler(
    State(state): State<AppState>,
    Query(params): Query<TimeseriesQueryParams>,
) -> axum::response::Response {
    let ts_engine = match &state.ts_engine {
        Some(e) => e,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "timeseries engine not configured").into_response(),
    };

    let end = match &params.end {
        Some(t) => match chrono::DateTime::parse_from_rfc3339(t) {
            Ok(dt) => dt.with_timezone(&chrono::Utc),
            Err(_) => return (StatusCode::BAD_REQUEST, "invalid end time format").into_response(),
        },
        None => chrono::Utc::now(),
    };
    let start = match &params.start {
        Some(t) => match chrono::DateTime::parse_from_rfc3339(t) {
            Ok(dt) => dt.with_timezone(&chrono::Utc),
            Err(_) => return (StatusCode::BAD_REQUEST, "invalid start time format").into_response(),
        },
        None => end - chrono::Duration::hours(1),
    };

    let points = ts_engine.query(
        params.element_id,
        &params.parameter,
        start,
        end,
    );

    let response = TimeseriesResponse {
        element_id: params.element_id,
        parameter: params.parameter,
        points: points
            .into_iter()
            .map(|p| DataPointDto {
                timestamp: p.timestamp.to_rfc3339(),
                value: p.value,
                quality: format!("{:?}", p.quality),
            })
            .collect(),
    };

    (StatusCode::OK, Json(response)).into_response()
}

/// Query parameters for `GET /api/timeseries/latest`.
#[derive(Debug, Deserialize, IntoParams)]
pub struct LatestQueryParams {
    pub element_id: u64,
    pub parameter: String,
}

/// `GET /api/timeseries/latest` — get the latest value for a data point.
#[utoipa::path(
    get,
    path = "/api/timeseries/latest",
    params(LatestQueryParams),
    responses(
        (status = 200, description = "最新数据点", body = DataPointDto),
        (status = 404, description = "未找到数据"),
        (status = 503, description = "时序引擎未配置"),
    )
)]
pub async fn latest_handler(
    State(state): State<AppState>,
    Query(params): Query<LatestQueryParams>,
) -> axum::response::Response {
    let ts_engine = match &state.ts_engine {
        Some(e) => e,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "timeseries engine not configured").into_response(),
    };

    match ts_engine.latest(params.element_id, &params.parameter) {
        Some(point) => {
            let response = DataPointDto {
                timestamp: point.timestamp.to_rfc3339(),
                value: point.value,
                quality: format!("{:?}", point.quality),
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        None => (StatusCode::NOT_FOUND, "no data found").into_response(),
    }
}

/// `GET /api/timeseries/statistics` — get time series engine statistics.
#[utoipa::path(
    get,
    path = "/api/timeseries/statistics",
    responses(
        (status = 200, description = "时序引擎统计信息"),
        (status = 503, description = "时序引擎未配置"),
    )
)]
pub async fn statistics_handler(
    State(state): State<AppState>,
) -> axum::response::Response {
    let ts_engine = match &state.ts_engine {
        Some(e) => e,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "timeseries engine not configured").into_response(),
    };

    let stats = ts_engine.statistics();
    let response = serde_json::json!({
        "series_count": stats.series_count,
        "total_points": stats.total_points,
        "max_retention": stats.max_retention,
        "backend": format!("{:?}", stats.backend),
    });

    (StatusCode::OK, Json(response)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timeseries_query_params_deserialization() {
        let params: TimeseriesQueryParams = serde_json::from_str(
            r#"{"element_id":1,"parameter":"voltage_pu","start":"2026-01-01T00:00:00Z","end":"2026-01-01T01:00:00Z"}"#,
        ).unwrap();
        assert_eq!(params.element_id, 1);
        assert_eq!(params.parameter, "voltage_pu");
    }

    #[test]
    fn test_data_point_dto_serialization() {
        let dto = DataPointDto {
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            value: 1.05,
            quality: "Good".to_string(),
        };
        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("\"value\":1.05"));
    }
}
