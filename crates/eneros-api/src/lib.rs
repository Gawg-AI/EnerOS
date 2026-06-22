pub mod app;
pub mod audit;
pub mod auth;
pub mod config_reload;
pub mod middleware;
pub mod openapi;
pub mod server;
pub mod client;
pub mod types;
pub mod handlers;
/// OpenTelemetry OTLP 导出 (v0.29.0 — T029-18)
pub mod otel;

/// 日志级别动态调整的 reload handle 类型 (T029-05)。
///
/// 包装 `tracing_subscriber::reload::Handle<EnvFilter, Registry>`，
/// 用于在运行时通过 API 动态切换日志级别。
pub type LogReloadHandle =
    tracing_subscriber::reload::Handle<tracing_subscriber::EnvFilter, tracing_subscriber::Registry>;

pub use server::{load_rustls_server_config, ApiServer, TlsConfig};
pub use client::ApiClient;
pub use app::AppState;
pub use config_reload::{ConfigWatcher, SharedConfig, shared as shared_config};
pub use openapi::OpenApiDoc;
pub use types::*;
