use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use parking_lot::RwLock;
use tokio::sync::mpsc;
use tower_http::cors::CorsLayer;

use eneros_agent::{AgentOrchestrator, DataDrivenAgentLoop};
use eneros_constraint::ConstraintEngine;
use eneros_eventbus::EventBus;
use eneros_network::PowerNetwork;
use eneros_powerflow::PowerFlowSolver;
use eneros_scada::{ScadaCollector, SnapshotBuilder};
use eneros_timeseries::TimeSeriesEngine;
use eneros_topology::TopologyEngine;

use crate::handlers;

/// WebSocket client for real-time push
pub struct WsClient {
    pub id: String,
    pub sender: mpsc::Sender<String>,
}

/// Shared application state injected into all handlers
#[derive(Clone)]
pub struct AppState {
    pub topology_engine: Option<Arc<TopologyEngine>>,
    pub powerflow_solver: Option<Arc<PowerFlowSolver>>,
    pub constraint_engine: Option<Arc<ConstraintEngine>>,
    pub network: Option<Arc<PowerNetwork>>,
    pub ts_engine: Option<Arc<TimeSeriesEngine>>,
    pub scada_collector: Option<Arc<ScadaCollector>>,
    pub event_bus: Option<Arc<EventBus>>,
    pub ws_clients: Arc<RwLock<Vec<WsClient>>>,
    // New fields for full component injection
    pub agent_orchestrator: Option<Arc<AgentOrchestrator>>,
    pub data_pipeline: Option<Arc<eneros_scada::DataPipeline>>,
    pub snapshot_builder: Option<Arc<SnapshotBuilder>>,
    pub data_driven_loop: Option<Arc<DataDrivenAgentLoop>>,
}

impl AppState {
    /// Create a default (empty) state — all engines are None
    pub fn new() -> Self {
        Self {
            topology_engine: None,
            powerflow_solver: None,
            constraint_engine: None,
            network: None,
            ts_engine: None,
            scada_collector: None,
            event_bus: None,
            ws_clients: Arc::new(RwLock::new(Vec::new())),
            agent_orchestrator: None,
            data_pipeline: None,
            snapshot_builder: None,
            data_driven_loop: None,
        }
    }

    /// Builder: inject PowerNetwork
    pub fn with_network(mut self, network: Arc<PowerNetwork>) -> Self {
        self.network = Some(network);
        self
    }

    /// Builder: inject ConstraintEngine
    pub fn with_constraint_engine(mut self, engine: Arc<ConstraintEngine>) -> Self {
        self.constraint_engine = Some(engine);
        self
    }

    /// Builder: inject TimeSeriesEngine
    pub fn with_ts_engine(mut self, engine: Arc<TimeSeriesEngine>) -> Self {
        self.ts_engine = Some(engine);
        self
    }

    /// Builder: inject ScadaCollector
    pub fn with_scada_collector(mut self, collector: Arc<ScadaCollector>) -> Self {
        self.scada_collector = Some(collector);
        self
    }

    /// Builder: inject AgentOrchestrator
    pub fn with_agent_orchestrator(mut self, orchestrator: Arc<AgentOrchestrator>) -> Self {
        self.agent_orchestrator = Some(orchestrator);
        self
    }

    /// Builder: inject DataPipeline
    pub fn with_data_pipeline(mut self, pipeline: Arc<eneros_scada::DataPipeline>) -> Self {
        self.data_pipeline = Some(pipeline);
        self
    }

    /// Builder: inject DataDrivenAgentLoop
    pub fn with_data_driven_loop(mut self, dd_loop: Arc<DataDrivenAgentLoop>) -> Self {
        self.data_driven_loop = Some(dd_loop);
        self
    }

    /// Builder: inject EventBus
    pub fn with_event_bus(mut self, bus: Arc<EventBus>) -> Self {
        self.event_bus = Some(bus);
        self
    }

    /// Builder: inject SnapshotBuilder
    pub fn with_snapshot_builder(mut self, builder: Arc<SnapshotBuilder>) -> Self {
        self.snapshot_builder = Some(builder);
        self
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the axum Router with all routes
pub fn create_router(state: AppState) -> Router {
    let api_routes = Router::new()
        .route("/power-flow", post(handlers::powerflow::power_flow_handler))
        .route("/constraints", get(handlers::constraints::constraints_handler))
        .route("/agents", get(handlers::agents::agents_handler))
        .route("/scada/latest", get(handlers::scada::scada_latest_handler))
        .route("/analysis/opf", post(handlers::analysis::opf_handler))
        .route("/analysis/state-estimation", post(handlers::analysis::state_estimation_handler))
        .route("/analysis/short-circuit", post(handlers::analysis::short_circuit_handler))
        .route("/topology", get(handlers::topology::topology_handler))
        .route("/dashboard/topology-svg", get(handlers::dashboard::topology_svg_handler))
        .route("/dashboard/flow-heatmap", get(handlers::dashboard::flow_heatmap_handler));

    Router::new()
        .route("/ws", get(handlers::ws::ws_handler))
        .route("/health", get(handlers::health::health_handler))
        .nest("/api", api_routes)
        .route("/", get(handlers::dashboard::dashboard_handler))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// Broadcast an event message to all connected WebSocket clients
pub fn broadcast_event(state: &AppState, event: &str) {
    let clients = state.ws_clients.read();
    for client in clients.iter() {
        if client.sender.try_send(event.to_string()).is_err() {
            tracing::warn!("Failed to send event to ws client {}", client.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::util::ServiceExt;

    #[test]
    fn test_app_state_default() {
        let state = AppState::default();
        assert!(state.topology_engine.is_none());
        assert!(state.powerflow_solver.is_none());
        assert!(state.constraint_engine.is_none());
        assert!(state.network.is_none());
        assert!(state.ts_engine.is_none());
        assert!(state.scada_collector.is_none());
        assert!(state.event_bus.is_none());
        assert!(state.ws_clients.read().is_empty());
        assert!(state.agent_orchestrator.is_none());
        assert!(state.data_pipeline.is_none());
        assert!(state.snapshot_builder.is_none());
        assert!(state.data_driven_loop.is_none());
    }

    #[test]
    fn test_create_router() {
        let state = AppState::new();
        let _router = create_router(state);
        // If this compiles, the router was created successfully
    }

    #[tokio::test]
    async fn test_dashboard_endpoint() {
        let state = AppState::new();
        let app = create_router(state);

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_dashboard_topology_svg_endpoint() {
        let state = AppState::new();
        let app = create_router(state);

        let response = app
            .oneshot(Request::builder().uri("/api/dashboard/topology-svg").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_dashboard_flow_heatmap_endpoint() {
        let state = AppState::new();
        let app = create_router(state);

        let response = app
            .oneshot(Request::builder().uri("/api/dashboard/flow-heatmap").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_topology_endpoint() {
        let state = AppState::new();
        let app = create_router(state);

        let response = app
            .oneshot(Request::builder().uri("/api/topology").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_agents_endpoint() {
        let state = AppState::new();
        let app = create_router(state);

        let response = app
            .oneshot(Request::builder().uri("/api/agents").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_constraints_endpoint() {
        let state = AppState::new();
        let app = create_router(state);

        let response = app
            .oneshot(Request::builder().uri("/api/constraints").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_scada_latest_endpoint() {
        let state = AppState::new();
        let app = create_router(state);

        let response = app
            .oneshot(Request::builder().uri("/api/scada/latest").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_power_flow_endpoint() {
        let state = AppState::new();
        let app = create_router(state);

        let body = serde_json::json!({});
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/power-flow")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_broadcast_event_no_clients() {
        let state = AppState::new();
        // Should not panic with no clients
        broadcast_event(&state, "test event");
    }

    #[tokio::test]
    async fn test_broadcast_event_with_client() {
        let state = AppState::new();
        let (tx, mut rx) = mpsc::channel::<String>(10);

        let client = WsClient {
            id: "test-client".to_string(),
            sender: tx,
        };
        state.ws_clients.write().push(client);

        broadcast_event(&state, "hello");

        let msg = rx.try_recv().unwrap();
        assert_eq!(msg, "hello");
    }
}
