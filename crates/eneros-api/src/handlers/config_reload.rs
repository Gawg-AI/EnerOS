//! Configuration reload API handler (v0.9.0).
//!
//! Exposes `POST /api/config/reload` for manually triggering a config reload
//! and `GET /api/config` for inspecting the current runtime configuration.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;

use crate::app::AppState;
use crate::config_reload::ReloadResult;

/// `POST /api/config/reload` — manually trigger a config reload from disk.
pub async fn reload_handler(State(state): State<AppState>) -> axum::response::Response {
    let watcher = match &state.config_watcher {
        Some(w) => w,
        None => {
            return (
                StatusCode::NOT_IMPLEMENTED,
                Json(serde_json::json!({
                    "success": false,
                    "message": "Config hot reload is not enabled. Start with --config <path>."
                })),
            )
                .into_response();
        }
    };

    match watcher.reload() {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "success": false,
                "message": e,
            })),
        )
            .into_response(),
    }
}

/// `GET /api/config` — return the current runtime configuration (sanitized).
///
/// Sensitive fields (jwt_secret, api_keys) are redacted.
pub async fn get_config_handler(State(state): State<AppState>) -> axum::response::Response {
    let config = match &state.shared_config {
        Some(sc) => sc.read().clone(),
        None => {
            return (
                StatusCode::NOT_IMPLEMENTED,
                Json(serde_json::json!({
                    "message": "Shared config is not available."
                })),
            )
                .into_response();
        }
    };

    // Sanitize sensitive fields
    let mut sanitized = serde_json::to_value(&config).unwrap_or_default();
    if let Some(obj) = sanitized.as_object_mut() {
        if let Some(security) = obj.get_mut("security").and_then(|s| s.as_object_mut()) {
            if security.contains_key("jwt_secret") {
                security.insert("jwt_secret".to_string(), serde_json::json!("***REDACTED***"));
            }
            if let Some(keys) = security.get_mut("api_keys").and_then(|k| k.as_array_mut()) {
                for _k in keys.iter_mut() {
                    *_k = serde_json::json!("***REDACTED***");
                }
            }
        }
    }

    (StatusCode::OK, Json(sanitized)).into_response()
}

/// Response for the reload endpoint (re-exported for convenience).
#[derive(Debug, Serialize)]
pub struct ConfigReloadResponse {
    pub success: bool,
    pub message: String,
    pub applied_fields: Vec<String>,
    pub skipped_fields: Vec<String>,
}

impl From<ReloadResult> for ConfigReloadResponse {
    fn from(r: ReloadResult) -> Self {
        Self {
            success: r.success,
            message: r.message,
            applied_fields: r.applied_fields,
            skipped_fields: r.skipped_fields,
        }
    }
}
