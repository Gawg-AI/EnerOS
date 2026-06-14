use axum::extract::State;
use axum::Json;
use chrono::Utc;

use crate::app::AppState;
use crate::types::{ApiResponse, ScadaLatestResponse, ScadaReadingResponse};

/// GET /api/scada/latest
pub async fn scada_latest_handler(
    State(state): State<AppState>,
) -> Json<ApiResponse<ScadaLatestResponse>> {
    if let Some(collector) = &state.scada_collector {
        // Trigger a collection cycle first to ensure latest data
        let _ = collector.collect_once();
        let readings = collector.latest_all();
        let response = ScadaLatestResponse {
            readings: readings.iter().map(|r| ScadaReadingResponse {
                element_id: r.element_id,
                parameter: r.parameter.clone(),
                value: r.value,
                quality: format!("{:?}", r.quality),
            }).collect(),
            snapshot_time: Utc::now().to_rfc3339(),
        };
        return Json(ApiResponse::success(response));
    }

    // No SCADA collector — return empty with a message
    let response = ScadaLatestResponse {
        readings: Vec::new(),
        snapshot_time: Utc::now().to_rfc3339(),
    };
    let mut api_response: ApiResponse<ScadaLatestResponse> = ApiResponse::success(response);
    api_response.error = Some("No SCADA collector configured".to_string());
    Json(api_response)
}
