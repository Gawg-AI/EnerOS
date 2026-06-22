//! 审计日志查询 API handler (v0.7.0 — deferred from v0.6.0 S4)。
//!
//! 暴露 `crate::audit::AuditLog` 的内存审计日志通过
//! `GET /api/audit`。支持按 `actor` 和 `result` 过滤，
//! 以及 `limit` 参数（默认 100，最大 1000）。
//!
//! T029-09: 集成 trace_id（从请求扩展获取），贯穿到日志和响应体。
//! T029-09: 完善 OpenAPI 文档（params + response body schema）。

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::app::AppState;
use crate::middleware::TraceId;

/// `GET /api/audit` 查询参数。
#[derive(Debug, Deserialize, IntoParams)]
pub struct AuditQuery {
    /// 按 actor（用户/主体名）过滤。可选。
    #[serde(default)]
    pub actor: Option<String>,
    /// 按 result（"success" | "failed" | "denied"）过滤。可选。
    #[serde(default)]
    pub result: Option<String>,
    /// 返回最大条目数（默认 100，最大 1000）。
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    100
}

/// 审计日志响应中的单条记录。
#[derive(Debug, Serialize, ToSchema)]
pub struct AuditEntryResponse {
    /// 条目唯一 ID
    pub id: String,
    /// 时间戳（Unix epoch 秒）
    pub timestamp: i64,
    /// 操作者（用户/主体名）
    pub actor: String,
    /// 操作者角色
    pub role: String,
    /// HTTP 方法
    pub method: String,
    /// 请求路径
    pub path: String,
    /// 客户端 IP 地址
    pub client_ip: String,
    /// 结果："success" | "failed" | "denied"
    pub result: String,
    /// 可选详情/错误信息
    pub detail: Option<String>,
}

/// `GET /api/audit` 响应体 schema（用于 OpenAPI 文档）。
#[derive(Debug, Serialize, ToSchema)]
pub struct AuditQueryResponseSchema {
    /// 审计日志条目列表（最新在前）
    pub entries: Vec<AuditEntryResponse>,
    /// 审计日志总条目数
    pub total: usize,
    /// 本次返回的条目数
    pub returned: usize,
    /// 分布式追踪 ID（T029-09）
    pub trace_id: String,
}

/// `GET /api/audit` — 查询审计日志条目（最新在前）。
///
/// 从 `crate::audit::AuditLog` 读取真实审计记录，支持按 actor 和 result 过滤。
/// 审计日志记录所有写操作（POST/PUT/DELETE）的 who/what/when/result/IP。
///
/// trace_id 从请求扩展中提取（T029-04 中间件注入），贯穿到日志和响应体。
#[utoipa::path(
    get,
    tag = "audit",
    path = "/api/audit",
    params(AuditQuery),
    responses(
        (status = 200, description = "审计日志查询结果", body = AuditQueryResponseSchema),
        (status = 503, description = "审计日志未配置"),
    )
)]
pub async fn query_handler(
    State(state): State<AppState>,
    Extension(trace_id_ext): Extension<TraceId>,
    Query(q): Query<AuditQuery>,
) -> axum::response::Response {
    let trace_id = trace_id_ext.0;

    tracing::info!(
        trace_id = %trace_id,
        actor = ?q.actor,
        result = ?q.result,
        limit = q.limit,
        "audit query request received"
    );

    let audit_log = match &state.audit_log {
        Some(log) => log,
        None => {
            tracing::warn!(
                trace_id = %trace_id,
                "audit log not configured"
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "audit log not configured",
            )
                .into_response();
        }
    };

    // Clamp limit to [1, 1000]
    let limit = q.limit.clamp(1, 1000);

    let entries = audit_log.query(q.actor.as_deref(), q.result.as_deref(), limit);

    let response: Vec<AuditEntryResponse> = entries
        .into_iter()
        .map(|e| AuditEntryResponse {
            id: e.id,
            timestamp: e.timestamp,
            actor: e.actor,
            role: e.role,
            method: e.method,
            path: e.path,
            client_ip: e.client_ip,
            result: e.result,
            detail: e.detail,
        })
        .collect();

    let total = audit_log.count();
    let returned = response.len();

    tracing::info!(
        trace_id = %trace_id,
        total,
        returned,
        "audit query completed"
    );

    let response_body = AuditQueryResponseSchema {
        entries: response,
        total,
        returned,
        trace_id,
    };

    (StatusCode::OK, Json(response_body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_query_default_limit() {
        assert_eq!(default_limit(), 100);
    }

    #[test]
    fn test_audit_query_deserialize() {
        let qs = "actor=alice&result=success&limit=50";
        let parsed: AuditQuery = serde_urlencoded::from_str(qs).unwrap();
        assert_eq!(parsed.actor.as_deref(), Some("alice"));
        assert_eq!(parsed.result.as_deref(), Some("success"));
        assert_eq!(parsed.limit, 50);
    }

    #[test]
    fn test_audit_query_deserialize_defaults() {
        let parsed: AuditQuery = serde_urlencoded::from_str("").unwrap();
        assert!(parsed.actor.is_none());
        assert!(parsed.result.is_none());
        assert_eq!(parsed.limit, 100);
    }
}
