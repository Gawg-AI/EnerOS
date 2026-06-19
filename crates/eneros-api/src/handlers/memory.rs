//! Memory API handlers (v0.6.0 — S4).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;

use crate::app::AppState;

/// Request body for storing a memory entry.
#[derive(Debug, Deserialize)]
pub struct StoreMemoryRequest {
    pub memory_type: String,
    pub content: String,
    pub importance: f64,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Request body for recalling memories.
#[derive(Debug, Deserialize)]
pub struct RecallMemoryRequest {
    #[serde(default)]
    pub memory_type: Option<String>,
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default)]
    pub min_importance: Option<f64>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    100
}

/// `POST /api/memory/{agent_id}/store` — store a memory entry.
pub async fn store_handler(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<StoreMemoryRequest>,
) -> axum::response::Response {
    let memory = match &state.agent_memory {
        Some(m) => m,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "memory not configured").into_response(),
    };

    let mem_type = match req.memory_type.as_str() {
        "episodic" => eneros_memory::MemoryType::Episodic,
        "semantic" => eneros_memory::MemoryType::Semantic,
        "procedural" => eneros_memory::MemoryType::Procedural,
        _ => return (StatusCode::BAD_REQUEST, "invalid memory_type").into_response(),
    };

    let mut entry = eneros_memory::MemoryEntry::new(mem_type, req.content, req.importance);
    if !req.tags.is_empty() {
        entry = entry.with_tags(req.tags);
    }
    let entry_id = entry.id.clone();

    match memory.store(&agent_id, entry).await {
        Ok(_) => {
            (StatusCode::OK, Json(serde_json::json!({"entry_id": entry_id, "stored": true}))).into_response()
        }
        Err(e) => {
            tracing::warn!("memory store failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("store failed: {}", e)).into_response()
        }
    }
}

/// `POST /api/memory/{agent_id}/recall` — recall memories.
pub async fn recall_handler(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<RecallMemoryRequest>,
) -> axum::response::Response {
    let memory = match &state.agent_memory {
        Some(m) => m,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "memory not configured").into_response(),
    };

    let mut query = eneros_memory::RecallQuery::new().with_limit(req.limit);
    if let Some(ref kw) = req.keyword {
        query = query.with_keyword(kw);
    }
    if let Some(imp) = req.min_importance {
        query = query.with_min_importance(imp);
    }
    if let Some(ref mt) = req.memory_type {
        let mem_type = match mt.as_str() {
            "episodic" => eneros_memory::MemoryType::Episodic,
            "semantic" => eneros_memory::MemoryType::Semantic,
            "procedural" => eneros_memory::MemoryType::Procedural,
            _ => return (StatusCode::BAD_REQUEST, "invalid memory_type").into_response(),
        };
        query = query.with_type(mem_type);
    }

    match memory.recall(&agent_id, &query).await {
        Ok(entries) => {
            let results: Vec<serde_json::Value> = entries
                .into_iter()
                .map(|e| {
                    serde_json::json!({
                        "id": e.id,
                        "memory_type": format!("{:?}", e.memory_type),
                        "content": e.content,
                        "importance": e.importance,
                        "timestamp": e.timestamp.to_rfc3339(),
                        "tags": e.tags,
                        "access_count": e.access_count,
                    })
                })
                .collect();
            (StatusCode::OK, Json(serde_json::json!({"entries": results}))).into_response()
        }
        Err(e) => {
            tracing::warn!("memory recall failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("recall failed: {}", e)).into_response()
        }
    }
}

/// `GET /api/memory/{agent_id}/count` — get memory entry count.
pub async fn count_handler(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> axum::response::Response {
    let memory = match &state.agent_memory {
        Some(m) => m,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "memory not configured").into_response(),
    };

    let count = memory.count(&agent_id).await;
    (StatusCode::OK, Json(serde_json::json!({"agent_id": agent_id, "count": count}))).into_response()
}

/// `DELETE /api/memory/{agent_id}/{entry_id}` — forget a specific memory entry.
pub async fn forget_handler(
    State(state): State<AppState>,
    Path((agent_id, entry_id)): Path<(String, String)>,
) -> axum::response::Response {
    let memory = match &state.agent_memory {
        Some(m) => m,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "memory not configured").into_response(),
    };

    match memory.forget(&agent_id, &entry_id).await {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({"forgotten": true}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("forget failed: {}", e)).into_response(),
    }
}

/// `DELETE /api/memory/{agent_id}` — clear all memories for an agent.
pub async fn clear_handler(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> axum::response::Response {
    let memory = match &state.agent_memory {
        Some(m) => m,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "memory not configured").into_response(),
    };

    match memory.clear(&agent_id).await {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({"cleared": true}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("clear failed: {}", e)).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_memory_request_deserialization() {
        let req: StoreMemoryRequest = serde_json::from_str(
            r#"{"memory_type":"episodic","content":"test","importance":0.8,"tags":["voltage"]}"#,
        ).unwrap();
        assert_eq!(req.memory_type, "episodic");
        assert_eq!(req.content, "test");
        assert_eq!(req.importance, 0.8);
        assert_eq!(req.tags, vec!["voltage"]);
    }

    #[test]
    fn test_store_memory_request_minimal() {
        let req: StoreMemoryRequest = serde_json::from_str(
            r#"{"memory_type":"semantic","content":"bus1","importance":0.5}"#,
        ).unwrap();
        assert!(req.tags.is_empty());
    }

    #[test]
    fn test_recall_memory_request_deserialization() {
        let req: RecallMemoryRequest = serde_json::from_str(
            r#"{"keyword":"voltage","limit":10}"#,
        ).unwrap();
        assert_eq!(req.keyword.as_deref(), Some("voltage"));
        assert_eq!(req.limit, 10);
    }

    #[test]
    fn test_recall_memory_request_default_limit() {
        let req: RecallMemoryRequest = serde_json::from_str(
            r#"{}"#,
        ).unwrap();
        assert_eq!(req.limit, 100);
    }
}
