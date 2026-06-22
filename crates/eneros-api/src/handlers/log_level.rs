//! Dynamic log level adjustment API handler (v0.7.0 — deferred from v0.6.0 S3).
//!
//! Exposes runtime log level control via `POST /api/log-level`. The handler
//! uses `tracing_subscriber::reload::Layer` to dynamically swap the `EnvFilter`
//! at runtime, allowing operators to increase/decrease verbosity without
//! restarting the server (T029-05).

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::app::AppState;

/// Request body for `POST /api/log-level`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct LogLevelRequest {
    /// New log level: "error" | "warn" | "info" | "debug" | "trace"
    pub level: String,
}

/// Response body for `POST /api/log-level`.
#[derive(Debug, Serialize, ToSchema)]
pub struct LogLevelResponse {
    pub previous_level: String,
    pub current_level: String,
    pub result: String,
}

/// `POST /api/log-level` — dynamically adjust the log level.
///
/// 通过 `reload::Handle` 替换 `EnvFilter`，实现真正的运行时日志级别切换 (T029-05)。
#[utoipa::path(
    post,
    path = "/api/log-level",
    request_body = LogLevelRequest,
    responses(
        (status = 200, description = "日志级别调整结果", body = LogLevelResponse),
        (status = 400, description = "无效的日志级别"),
        (status = 500, description = "日志级别切换失败"),
    )
)]
pub async fn set_level_handler(
    State(state): State<AppState>,
    Json(req): Json<LogLevelRequest>,
) -> axum::response::Response {
    // 验证日志级别
    let new_level = match parse_level(&req.level) {
        Ok(l) => l,
        Err(msg) => return (StatusCode::BAD_REQUEST, msg).into_response(),
    };

    // 读取切换前的日志级别
    let previous = state.current_log_level.read().clone();
    let current = req.level.to_lowercase();

    // 通过 reload handle 动态替换 EnvFilter (T029-05)
    if let Some(ref handle) = state.log_reload_handle {
        let filter = tracing_subscriber::EnvFilter::new(&current);
        if let Err(e) = handle.reload(filter) {
            tracing::error!(error = %e, "failed to reload log filter");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to set log level",
            )
                .into_response();
        }
    } else {
        // 回退：无 reload handle 时使用 log::set_max_level（测试场景）
        log::set_max_level(match new_level {
            tracing::Level::ERROR => log::LevelFilter::Error,
            tracing::Level::WARN => log::LevelFilter::Warn,
            tracing::Level::INFO => log::LevelFilter::Info,
            tracing::Level::DEBUG => log::LevelFilter::Debug,
            tracing::Level::TRACE => log::LevelFilter::Trace,
        });
    }

    // 更新当前级别记录
    *state.current_log_level.write() = current.clone();

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
#[utoipa::path(
    get,
    path = "/api/log-level",
    responses(
        (status = 200, description = "当前日志级别与可选级别"),
    )
)]
pub async fn get_level_handler(State(state): State<AppState>) -> axum::response::Response {
    let current = state.current_log_level.read().clone();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;
    use tracing_subscriber::prelude::*;

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

    // ── T029-05: JSON 日志格式测试 ────────────────────────────────────────

    /// 自定义 MakeWriter：捕获日志输出到内存缓冲区，用于验证 JSON 格式 (T029-05)。
    #[derive(Clone)]
    struct CapturingWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl CapturingWriter {
        fn new() -> (Self, Arc<Mutex<Vec<u8>>>) {
            let buf = Arc::new(Mutex::new(Vec::new()));
            (Self { buf: buf.clone() }, buf)
        }
    }

    impl<'a> MakeWriter<'a> for CapturingWriter {
        type Writer = BufWriter;
        fn make_writer(&'a self) -> Self::Writer {
            BufWriter {
                buf: self.buf.clone(),
            }
        }
    }

    struct BufWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl std::io::Write for BufWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.buf.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// 测试 JSON 日志格式输出：验证日志为合法 JSON，包含 file 和 line 字段 (T029-05)。
    #[test]
    fn test_json_log_format() {
        let (writer, buf) = CapturingWriter::new();
        let filter = tracing_subscriber::EnvFilter::new("info");
        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_file(true)
            .with_line_number(true)
            .with_writer(writer)
            .json();

        let subscriber = tracing_subscriber::registry()
            .with(filter)
            .with(fmt_layer);

        tracing::dispatcher::with_default(
            &tracing::dispatcher::Dispatch::new(subscriber),
            || {
                tracing::info!("test json log message");
            },
        );

        // 读取捕获的日志输出
        let captured = String::from_utf8(buf.lock().unwrap().clone())
            .expect("captured log should be valid UTF-8");

        // JSON 日志每行一条记录，取第一行解析
        let first_line = captured.lines().next().unwrap_or(&captured);
        let parsed: serde_json::Value = serde_json::from_str(first_line)
            .expect("JSON log output should be valid JSON");

        // 验证包含必要字段
        assert!(
            parsed["fields"]["message"].is_string(),
            "JSON log should contain fields.message, got: {}",
            first_line
        );
        assert!(
            parsed["filename"].is_string(),
            "JSON log should contain filename field, got: {}",
            first_line
        );
        assert!(
            parsed["line_number"].is_number(),
            "JSON log should contain line_number field, got: {}",
            first_line
        );
    }
}
