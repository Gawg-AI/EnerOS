use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::app::AppState;

/// GET /health — system health check
pub async fn health_handler(State(state): State<AppState>) -> Json<Value> {
    let mut components = serde_json::Map::new();

    // Check network availability
    components.insert(
        "network".to_string(),
        json!(state.network.is_some()),
    );

    // Check topology engine
    components.insert(
        "topology_engine".to_string(),
        json!(state.topology_engine.is_some()),
    );

    // Check constraint engine
    components.insert(
        "constraint_engine".to_string(),
        json!(state.constraint_engine.is_some()),
    );

    // Check SCADA collector
    components.insert(
        "scada_collector".to_string(),
        json!(state.scada_collector.is_some()),
    );

    // Check agent orchestrator
    let agent_count = state.agent_orchestrator.as_ref().map(|o| o.agent_count()).unwrap_or(0);
    components.insert(
        "agent_orchestrator".to_string(),
        json!(state.agent_orchestrator.is_some()),
    );
    components.insert("agent_count".to_string(), json!(agent_count));

    // Check timeseries engine
    components.insert(
        "timeseries_engine".to_string(),
        json!(state.ts_engine.is_some()),
    );

    // Overall status: "ok" if at least network is available, "degraded" otherwise
    let overall = if state.network.is_some() {
        "ok"
    } else {
        "degraded"
    };

    Json(json!({
        "status": overall,
        "components": components,
        "version": env!("CARGO_PKG_VERSION"),
    }))
}
