//! HTTP 中间件：请求追踪 (T029-04)。
//!
//! 为每个 HTTP 请求生成或复用 trace_id，并通过 `X-Trace-Id` 响应头返回给调用方。
//! trace_id 同时存入请求扩展 (extensions)，供下游 handler 和 TraceLayer 使用。

use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;
use uuid::Uuid;

/// trace_id 传播所用的 HTTP 头名称。
pub const TRACE_ID_HEADER: &str = "x-trace-id";

/// 请求扩展：携带 trace_id 贯穿整个请求生命周期。
#[derive(Clone, Debug)]
pub struct TraceId(pub String);

/// trace_id 中间件：为每个请求生成或复用 trace_id。
///
/// 行为：
/// 1. 检查请求头 `X-Trace-Id`，若存在且非空则复用（支持上游调用方传播）
/// 2. 否则生成新的 UUID v4
/// 3. 将 trace_id 存入请求扩展，供下游 handler / TraceLayer 读取
/// 4. 在响应头中添加 `X-Trace-Id: <trace_id>`
pub async fn trace_id_middleware(
    mut request: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Response {
    // 优先复用上游传入的 trace_id
    let trace_id = request
        .headers()
        .get(TRACE_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    // 注入请求扩展，供下游 handler 和 TraceLayer 的 make_span_with 读取
    request.extensions_mut().insert(TraceId(trace_id.clone()));

    // 处理请求
    let mut response = next.run(request).await;

    // 在响应头中添加 X-Trace-Id
    if let Ok(value) = HeaderValue::from_str(&trace_id) {
        response.headers_mut().insert(TRACE_ID_HEADER, value);
    }

    response
}
