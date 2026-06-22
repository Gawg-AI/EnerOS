//! OpenTelemetry OTLP 导出 (v0.29.0 — T029-18)。
//!
//! 实现真实的 OpenTelemetry SDK 初始化，通过 OTLP gRPC 协议将 trace/span
//! 数据导出到兼容 OTLP 的后端（如 Jaeger、Tempo、OpenTelemetry Collector）。

use std::sync::OnceLock;

use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::TracerProvider;
use opentelemetry_sdk::Resource;
use thiserror::Error;

/// OTLP 导出器初始化错误。
#[derive(Debug, Error)]
pub enum OtelError {
    /// SpanExporter 构建失败（端点不可达、TLS 配置错误等）
    #[error("OTLP exporter build failed: {0}")]
    ExporterBuild(String),
}

/// OTLP 配置快照，从配置文件 / CLI / 环境变量解析后传入初始化函数。
#[derive(Debug, Clone)]
pub struct OtelConfig {
    /// 是否启用 OTLP 导出
    pub enabled: bool,
    /// OTLP gRPC 端点（如 `http://localhost:4317`）
    pub endpoint: String,
    /// 服务名（资源属性 `service.name`）
    pub service_name: String,
}

impl Default for OtelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: default_otlp_endpoint().to_string(),
            service_name: "eneros".to_string(),
        }
    }
}

/// OTLP gRPC 默认端点（OpenTelemetry Collector 标准端口 4317）。
pub fn default_otlp_endpoint() -> &'static str {
    "http://localhost:4317"
}

/// 全局持有的 TracerProvider，用于优雅关闭时 flush 残留 span。
static GLOBAL_PROVIDER: OnceLock<TracerProvider> = OnceLock::new();

/// 解析最终的 OTLP endpoint，按优先级顺序检查各配置源。
///
/// 优先级（高 → 低）：
/// 1. `cli_endpoint` — 命令行参数 `--otel-endpoint`
/// 2. `OTEL_EXPORTER_OTLP_ENDPOINT` 环境变量（OpenTelemetry 标准）
/// 3. `config_endpoint` — 配置文件 `[observability] otel_endpoint`
/// 4. `default_otlp_endpoint()` — `http://localhost:4317`
pub fn resolve_otlp_endpoint(
    cli_endpoint: Option<&str>,
    config_endpoint: Option<&str>,
) -> String {
    if let Some(ep) = cli_endpoint {
        if !ep.is_empty() {
            return ep.to_string();
        }
    }

    if let Ok(ep) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
        if !ep.is_empty() {
            return ep;
        }
    }

    if let Some(ep) = config_endpoint {
        if !ep.is_empty() {
            return ep.to_string();
        }
    }

    default_otlp_endpoint().to_string()
}

/// 初始化 OTLP tracer 并返回 `tracing_opentelemetry::layer()` 所需的 tracer。
///
/// 此函数执行真实的 OpenTelemetry SDK 初始化：
/// 1. 构建 `SpanExporter`（tonic gRPC 客户端连接到 OTLP collector）
/// 2. 构建 `TracerProvider`（batch exporter + 资源属性）
/// 3. 返回命名 tracer，供 `tracing_opentelemetry::layer().with_tracer()` 使用
///
/// # 注意
/// - 此函数必须在 Tokio runtime 上下文中调用（batch exporter 需要 Tokio）
/// - 进程退出前应调用 `shutdown_otlp()` flush 残留 span
pub fn init_otlp_tracer(
    config: &OtelConfig,
) -> Result<opentelemetry_sdk::trace::Tracer, OtelError> {
    if !config.enabled {
        return Err(OtelError::ExporterBuild(
            "OTLP export is disabled".to_string(),
        ));
    }

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&config.endpoint)
        .build()
        .map_err(|e| OtelError::ExporterBuild(e.to_string()))?;

    let resource = Resource::new(vec![KeyValue::new(
        "service.name",
        config.service_name.clone(),
    )]);

    let provider = TracerProvider::builder()
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .with_resource(resource)
        .build();

    let tracer = provider.tracer(config.service_name.clone());

    // GLOBAL_PROVIDER 持有 provider 所有权，用于 shutdown 时 force_flush
    let _ = GLOBAL_PROVIDER.set(provider);

    Ok(tracer)
}

/// 优雅关闭 OTLP 导出器，flush 所有未导出的 span。
///
/// 应在进程退出前调用，确保 batch exporter 缓冲区中的 span 被发送到 collector。
/// 如果 OTLP 未初始化，此函数为 no-op。
pub fn shutdown_otlp() {
    if let Some(provider) = GLOBAL_PROVIDER.get() {
        let _ = provider.force_flush();
        // 调用 shutdown() 释放 gRPC 连接资源，避免连接泄漏
        let _ = provider.shutdown();
    }
}

/// 创建 `tracing_opentelemetry::OpenTelemetryLayer` 并返回。
///
/// 封装 `init_otlp_tracer` + `tracing_opentelemetry::layer()` 调用，
/// 供 `main.rs` 在构建 tracing-subscriber 时使用。
///
/// 泛型 `S` 使返回的 layer 可组合到任意 `Subscriber + LookupSpan` 之上，
/// 从而与 `reload::Layer<EnvFilter, Registry>` 等其他 layer 正确组合
/// (T029-05: 动态日志级别 API 接线)。
pub fn build_otel_layer<S>(
    config: &OtelConfig,
) -> Result<
    tracing_opentelemetry::OpenTelemetryLayer<S, opentelemetry_sdk::trace::Tracer>,
    OtelError,
>
where
    S: tracing::Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span>,
{
    let tracer = init_otlp_tracer(config)?;
    Ok(tracing_opentelemetry::layer::<S>().with_tracer(tracer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = OtelConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.endpoint, "http://localhost:4317");
        assert_eq!(cfg.service_name, "eneros");
    }

    #[test]
    fn test_default_otlp_endpoint() {
        assert_eq!(default_otlp_endpoint(), "http://localhost:4317");
    }

    #[test]
    fn test_resolve_endpoint_cli_priority() {
        std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
        let cli = Some("http://cli:4317");
        let config = Some("http://config:4317");
        let result = resolve_otlp_endpoint(cli, config);
        assert_eq!(result, "http://cli:4317");
    }

    #[test]
    fn test_resolve_endpoint_env_var_fallback() {
        std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://env:4317");
        let cli = Some("");
        let config = Some("http://config:4317");
        let result = resolve_otlp_endpoint(cli, config);
        assert_eq!(result, "http://env:4317");
        std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
    }

    #[test]
    fn test_resolve_endpoint_config_fallback() {
        std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
        let cli: Option<&str> = None;
        let config = Some("http://config:4317");
        let result = resolve_otlp_endpoint(cli, config);
        assert_eq!(result, "http://config:4317");
    }

    #[test]
    fn test_resolve_endpoint_default_fallback() {
        std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
        let cli: Option<&str> = None;
        let config: Option<&str> = None;
        let result = resolve_otlp_endpoint(cli, config);
        assert_eq!(result, "http://localhost:4317");
    }

    #[test]
    fn test_resolve_endpoint_empty_config_fallback() {
        std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
        let cli: Option<&str> = None;
        let config = Some("");
        let result = resolve_otlp_endpoint(cli, config);
        assert_eq!(result, "http://localhost:4317");
    }

    #[test]
    fn test_init_otlp_disabled_returns_error() {
        let cfg = OtelConfig {
            enabled: false,
            endpoint: "http://localhost:4317".to_string(),
            service_name: "eneros-test".to_string(),
        };
        let result = init_otlp_tracer(&cfg);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, OtelError::ExporterBuild(_)));
        assert!(err.to_string().contains("disabled"));
    }

    #[tokio::test]
    async fn test_init_otlp_enabled_with_unreachable_endpoint() {
        let cfg = OtelConfig {
            enabled: true,
            endpoint: "http://127.0.0.1:1".to_string(),
            service_name: "eneros-test-unreachable".to_string(),
        };
        let result = init_otlp_tracer(&cfg);
        assert!(
            result.is_ok(),
            "OTLP tracer init should succeed even with unreachable endpoint: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_otel_config_clone_debug() {
        let cfg = OtelConfig {
            enabled: true,
            endpoint: "http://test:4317".to_string(),
            service_name: "test-svc".to_string(),
        };
        let cloned = cfg.clone();
        assert_eq!(cfg.enabled, cloned.enabled);
        assert_eq!(cfg.endpoint, cloned.endpoint);
        assert_eq!(cfg.service_name, cloned.service_name);

        let debug_str = format!("{:?}", cfg);
        assert!(debug_str.contains("test-svc"));
        assert!(debug_str.contains("http://test:4317"));
    }

    #[test]
    fn test_otel_error_display() {
        let err1 = OtelError::ExporterBuild("connection refused".to_string());
        assert!(err1.to_string().contains("OTLP exporter build failed"));
        assert!(err1.to_string().contains("connection refused"));
    }

    #[test]
    fn test_shutdown_otlp_noop_when_not_initialized() {
        shutdown_otlp();
    }
}
