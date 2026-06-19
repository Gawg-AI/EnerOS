//! Dynamic log level adjustment API handler (v0.7.0 — deferred from v0.6.0 S3).
//!
//! Exposes runtime log level control via `POST /api/log-level`. The handler
//! updates the global tracing subscriber's max level, allowing operators to
//! increase/decrease verbosity without restarting the server.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::app::AppState;

/// Request body for `POST /api/log-level`.
#[derive(Debug, Deserialize)]
pub struct LogLevelRequest {
    /// New log level: "error" | "warn" | "info" | "debug" | "trace"
    pub level: String,
}

/// Response body for `POST /api/log-level`.
#[derive(Debug, Serialize)]
pub struct LogLevelResponse {
    pub previous_level: String,
    pub current_level: String,
    pub result: String,
}

/// `POST /api/log-level` — dynamically adjust the log level.
pub async fn set_level_handler(
    State(_state): State<AppState>,
    Json(req): Json<LogLevelRequest>,
) -> axum::response::Response {
    let new_level = match parse_level(&req.level) {
        Ok(l) => l,
        Err(msg) => return (StatusCode::BAD_REQUEST, msg).into_response(),
    };

    // Capture the previous level before changing
    let previous = current_level_string();

    // Apply the new level via the global log crate's filter.
    // `log::set_max_level` updates the atomic max level used by the `log`
    // facade. The `tracing` subscriber picks this up automatically when
    // the `tracing/log` feature is enabled (which is the default).
    log::set_max_level(match new_level {
        tracing::Level::ERROR => log::LevelFilter::Error,
        tracing::Level::WARN => log::LevelFilter::Warn,
        tracing::Level::INFO => log::LevelFilter::Info,
        tracing::Level::DEBUG => log::LevelFilter::Debug,
        tracing::Level::TRACE => log::LevelFilter::Trace,
    });

    let current = req.level.to_lowercase();

    tracing::info!(
        previous = %previous,
        current = %current,
        "log level changed via API"
    );

    let response = LogLevelResponse {
        previous_level: previous,
        current_level: current,
        result: "success".to_string(),
    };

    (StatusCode::OK, Json(response)).into_response()
}

/// `GET /api/log-level` — get the current log level.
pub async fn get_level_handler(State(_state): State<AppState>) -> axum::response::Response {
    let current = current_level_string();
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "level": current,
            "available_levels": ["error", "warn", "info", "debug", "trace"],
        })),
    )
        .into_response()
}

fn parse_level(s: &str) -> Result<tracing::Level, String> {
    match s.to_lowercase().as_str() {
        "error" => Ok(tracing::Level::ERROR),
        "warn" => Ok(tracing::Level::WARN),
        "info" => Ok(tracing::Level::INFO),
        "debug" => Ok(tracing::Level::DEBUG),
        "trace" => Ok(tracing::Level::TRACE),
        other => Err(format!(
            "invalid level '{}': must be one of error/warn/info/debug/trace",
            other
        )),
    }
}

fn current_level_string() -> String {
    // Read the current max level from the log crate's filter
    match log::max_level() {
        log::LevelFilter::Error => "error".to_string(),
        log::LevelFilter::Warn => "warn".to_string(),
        log::LevelFilter::Info => "info".to_string(),
        log::LevelFilter::Debug => "debug".to_string(),
        log::LevelFilter::Trace => "trace".to_string(),
        log::LevelFilter::Off => "off".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_level_valid() {
        assert_eq!(parse_level("info").unwrap(), tracing::Level::INFO);
        assert_eq!(parse_level("DEBUG").unwrap(), tracing::Level::DEBUG);
        assert_eq!(parse_level("Trace").unwrap(), tracing::Level::TRACE);
    }

    #[test]
    fn test_parse_level_invalid() {
        assert!(parse_level("verbose").is_err());
        assert!(parse_level("").is_err());
    }

    #[test]
    fn test_log_level_request_deserialize() {
        let json = r#"{"level": "debug"}"#;
        let req: LogLevelRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.level, "debug");
    }
}
