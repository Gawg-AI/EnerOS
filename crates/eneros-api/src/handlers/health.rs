use axum::Json;
use serde_json::{json, Value};

/// GET /health
pub async fn health_handler() -> Json<Value> {
    Json(json!({
        "status": "ok"
    }))
}
