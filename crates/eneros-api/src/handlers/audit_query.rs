//! Audit log query API handler (v0.7.0 — deferred from v0.6.0 S4).
//!
//! Exposes the in-memory audit log recorded by `crate::audit::AuditLog`
//! via `GET /api/audit`. Supports filtering by `actor` and `result`,
//! plus a `limit` parameter (default 100, max 1000).

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::app::AppState;

/// Query parameters for `GET /api/audit`.
#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    /// Filter by actor (user/principal name). Optional.
    #[serde(default)]
    pub actor: Option<String>,
    /// Filter by result ("success" | "failed" | "denied"). Optional.
    #[serde(default)]
    pub result: Option<String>,
    /// Maximum number of entries to return (default 100, max 1000).
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    100
}

/// Single audit entry in the response.
#[derive(Debug, Serialize)]
pub struct AuditEntryResponse {
    pub id: String,
    pub timestamp: i64,
    pub actor: String,
    pub role: String,
    pub method: String,
    pub path: String,
    pub client_ip: String,
    pub result: String,
    pub detail: Option<String>,
}

/// `GET /api/audit` — query audit log entries (most recent first).
pub async fn query_handler(
    State(state): State<AppState>,
    Query(q): Query<AuditQuery>,
) -> axum::response::Response {
    let audit_log = match &state.audit_log {
        Some(log) => log,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "audit log not configured",
            )
                .into_response()
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

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "entries": response,
            "total": total,
            "returned": q.limit.min(limit),
        })),
    )
        .into_response()
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
