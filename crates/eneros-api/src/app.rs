use std::sync::Arc;

use axum::middleware::from_fn;
use axum::routing::{delete, get, post};
use axum::Router;
use parking_lot::RwLock;
use tokio::sync::mpsc;
use tower_http::cors::CorsLayer;
use utoipa::OpenApi;

use eneros_runtime::agent::{AgentController, AgentOrchestrator, DataDrivenAgentLoop};
use eneros_runtime::constraint::ConstraintEngine;
use eneros_runtime::eventbus::EventBus;
use eneros_runtime::gateway::decision_pipeline::ConstrainedDecisionPipeline;
use eneros_runtime::network::PowerNetwork;
use eneros_powerflow::PowerFlowSolver;
use eneros_runtime::scada::{ScadaCollector, SnapshotBuilder};
use eneros_runtime::timeseries::TimeSeriesEngine;
use eneros_topology::TopologyEngine;

use crate::handlers;
use crate::middleware::trace_id_middleware;
use crate::openapi::OpenApiDoc;

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
    pub data_pipeline: Option<Arc<eneros_runtime::scada::DataPipeline>>,
    pub snapshot_builder: Option<Arc<SnapshotBuilder>>,
    pub data_driven_loop: Option<Arc<DataDrivenAgentLoop>>,
    pub decision_pipeline: Option<Arc<ConstrainedDecisionPipeline>>,
    /// Metrics registry for observability (v0.6.0)
    pub metrics_registry: Option<Arc<crate::handlers::metrics::MetricsRegistry>>,
    /// Audit log for security (v0.6.0)
    pub audit_log: Option<Arc<crate::audit::AuditLog>>,
    /// Auth manager for JWT/API Key authentication (v0.6.0)
    pub auth_manager: Option<Arc<crate::auth::AuthManager>>,
    /// Device manager for device control (v0.6.0)
    pub device_manager: Option<Arc<eneros_runtime::device::DeviceManager>>,
    /// Tool engine for tool execution (v0.6.0)
    pub tool_engine: Option<Arc<tokio::sync::RwLock<eneros_runtime::tool::ToolEngine>>>,
    /// Agent memory store (v0.6.0)
    pub agent_memory: Option<Arc<dyn eneros_runtime::memory::AgentMemory>>,
    /// Shared runtime config for hot reload (v0.9.0)
    pub shared_config: Option<crate::config_reload::SharedConfig>,
    /// Config file watcher for hot reload (v0.9.0)
    pub config_watcher: Option<Arc<crate::config_reload::ConfigWatcher>>,
    /// SOE (Sequence of Events) recorder (v0.10.0 — Task 4)
    pub soe_recorder: Option<Arc<eneros_runtime::timeseries::SoeRecorder>>,
    /// Plugin market client for remote plugin search/install (v0.28.0 — Task 17)
    pub plugin_market_client:
        Option<Arc<tokio::sync::RwLock<eneros_runtime::plugin::PluginMarketClient>>>,
    /// 日志级别动态调整的 reload handle (T029-05)
    pub log_reload_handle: Option<crate::LogReloadHandle>,
    /// 当前日志级别（与 reload handle 配合使用，用于读取当前级别）(T029-05)
    pub current_log_level: Arc<RwLock<String>>,
    /// Agent 生命周期控制器 (T029-08)：管理 Agent 的 start/stop/pause/resume/status
    pub agent_controller: Option<AgentController>,
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
            decision_pipeline: None,
            metrics_registry: None,
            audit_log: None,
            auth_manager: None,
            device_manager: None,
            tool_engine: None,
            agent_memory: None,
            shared_config: None,
            config_watcher: None,
            soe_recorder: None,
            plugin_market_client: None,
            log_reload_handle: None,
            current_log_level: Arc::new(RwLock::new("info".to_string())),
            agent_controller: None,
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
    pub fn with_data_pipeline(mut self, pipeline: Arc<eneros_runtime::scada::DataPipeline>) -> Self {
        self.data_pipeline = Some(pipeline);
        self
    }

    /// Builder: inject DataDrivenAgentLoop
    pub fn with_data_driven_loop(mut self, dd_loop: Arc<DataDrivenAgentLoop>) -> Self {
        self.data_driven_loop = Some(dd_loop);
        self
    }

    pub fn with_decision_pipeline(mut self, pipeline: Arc<ConstrainedDecisionPipeline>) -> Self {
        self.decision_pipeline = Some(pipeline);
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

    /// Builder: inject MetricsRegistry (v0.6.0)
    pub fn with_metrics_registry(
        mut self,
        registry: Arc<crate::handlers::metrics::MetricsRegistry>,
    ) -> Self {
        self.metrics_registry = Some(registry);
        self
    }

    /// Builder: inject AuditLog (v0.6.0)
    pub fn with_audit_log(mut self, audit_log: Arc<crate::audit::AuditLog>) -> Self {
        self.audit_log = Some(audit_log);
        self
    }

    /// Builder: inject AuthManager (v0.6.0)
    pub fn with_auth_manager(mut self, auth_manager: Arc<crate::auth::AuthManager>) -> Self {
        self.auth_manager = Some(auth_manager);
        self
    }

    /// Builder: inject DeviceManager (v0.6.0)
    pub fn with_device_manager(mut self, dm: Arc<eneros_runtime::device::DeviceManager>) -> Self {
        self.device_manager = Some(dm);
        self
    }

    /// Builder: inject ToolEngine (v0.6.0)
    pub fn with_tool_engine(mut self, engine: Arc<tokio::sync::RwLock<eneros_runtime::tool::ToolEngine>>) -> Self {
        self.tool_engine = Some(engine);
        self
    }

    /// Builder: inject AgentMemory (v0.6.0)
    pub fn with_agent_memory(mut self, memory: Arc<dyn eneros_runtime::memory::AgentMemory>) -> Self {
        self.agent_memory = Some(memory);
        self
    }

    /// Builder: inject shared config (v0.9.0)
    pub fn with_shared_config(mut self, config: crate::config_reload::SharedConfig) -> Self {
        self.shared_config = Some(config);
        self
    }

    /// Builder: inject config watcher (v0.9.0)
    pub fn with_config_watcher(mut self, watcher: Arc<crate::config_reload::ConfigWatcher>) -> Self {
        self.config_watcher = Some(watcher);
        self
    }

    /// Builder: inject SOE recorder (v0.10.0 — Task 4)
    pub fn with_soe_recorder(mut self, recorder: Arc<eneros_runtime::timeseries::SoeRecorder>) -> Self {
        self.soe_recorder = Some(recorder);
        self
    }

    /// Builder: inject plugin market client (v0.28.0 — Task 17)
    pub fn with_plugin_market_client(
        mut self,
        client: Arc<tokio::sync::RwLock<eneros_runtime::plugin::PluginMarketClient>>,
    ) -> Self {
        self.plugin_market_client = Some(client);
        self
    }

    /// Builder: inject log reload handle for dynamic log level (T029-05)
    pub fn with_log_reload_handle(mut self, handle: crate::LogReloadHandle) -> Self {
        self.log_reload_handle = Some(handle);
        self
    }

    /// Builder: set initial log level string (T029-05)
    pub fn with_initial_log_level(self, level: impl Into<String>) -> Self {
        *self.current_log_level.write() = level.into();
        self
    }

    /// Builder: inject AgentController for agent lifecycle management (T029-08)
    pub fn with_agent_controller(mut self, controller: AgentController) -> Self {
        self.agent_controller = Some(controller);
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
        .route(
            "/constraints",
            get(handlers::constraints::constraints_handler),
        )
        .route("/agents", get(handlers::agents::agents_handler))
        .route("/scada/latest", get(handlers::scada::scada_latest_handler))
        .route(
            "/actions/structured",
            post(handlers::actions::structured_action_handler),
        )
        .route("/analysis/opf", post(handlers::analysis::opf_handler))
        .route(
            "/analysis/state-estimation",
            post(handlers::analysis::state_estimation_handler),
        )
        .route(
            "/analysis/short-circuit",
            post(handlers::analysis::short_circuit_handler),
        )
        // v0.8.0 — Advanced analysis endpoints
        .route(
            "/analysis/ac-opf",
            post(handlers::analysis::ac_opf_handler),
        )
        .route(
            "/analysis/transient",
            post(handlers::analysis::transient_handler),
        )
        .route(
            "/analysis/observability",
            post(handlers::analysis::observability_handler),
        )
        .route(
            "/analysis/bad-data",
            post(handlers::analysis::bad_data_handler),
        )
        .route(
            "/analysis/short-circuit/asymmetric",
            post(handlers::analysis::asymmetric_short_circuit_handler),
        )
        .route("/topology", get(handlers::topology::topology_handler))
        .route(
            "/dashboard/topology-svg",
            get(handlers::dashboard::topology_svg_handler),
        )
        .route(
            "/dashboard/flow-heatmap",
            get(handlers::dashboard::flow_heatmap_handler),
        )
        // Auth routes (v0.6.0)
        .route("/auth/login", post(handlers::auth::login_handler))
        .route("/auth/refresh", post(handlers::auth::refresh_handler))
        .route("/auth/me", get(handlers::auth::me_handler))
        // Timeseries routes (v0.6.0)
        .route(
            "/timeseries/query",
            get(handlers::timeseries::query_handler),
        )
        .route(
            "/timeseries/latest",
            get(handlers::timeseries::latest_handler),
        )
        .route(
            "/timeseries/statistics",
            get(handlers::timeseries::statistics_handler),
        )
        // SOE routes (v0.10.0 — Task 4)
        .route("/soe", get(handlers::soe::query_handler))
        .route("/soe/latest", get(handlers::soe::latest_handler))
        // Events routes (v0.6.0)
        .route(
            "/events/publish",
            post(handlers::events::publish_handler),
        )
        .route("/events/stats", get(handlers::events::stats_handler))
        // Devices routes (v0.6.0)
        .route("/devices", get(handlers::devices::list_handler))
        .route(
            "/devices/:id/health",
            get(handlers::devices::health_handler),
        )
        .route(
            "/devices/:id/connect",
            post(handlers::devices::connect_handler),
        )
        .route(
            "/devices/:id/disconnect",
            post(handlers::devices::disconnect_handler),
        )
        // Tools routes (v0.6.0)
        .route("/tools", get(handlers::tools::list_handler))
        .route(
            "/tools/:name/execute",
            post(handlers::tools::execute_handler),
        )
        // Memory routes (v0.6.0)
        .route(
            "/memory/:agent_id/store",
            post(handlers::memory::store_handler),
        )
        .route(
            "/memory/:agent_id/recall",
            post(handlers::memory::recall_handler),
        )
        .route(
            "/memory/:agent_id/count",
            get(handlers::memory::count_handler),
        )
        .route(
            "/memory/:agent_id/:entry_id",
            delete(handlers::memory::forget_handler),
        )
        .route(
            "/memory/:agent_id",
            delete(handlers::memory::clear_handler),
        )
        // v0.7.0 — deferred from v0.6.0 S4: additional API endpoints
        .route("/audit", get(handlers::audit_query::query_handler))
        .route("/whatif", post(handlers::whatif::whatif_handler))
        .route(
            "/validation/check",
            post(handlers::validation::check_handler),
        )
        .route(
            "/compliance/check",
            post(handlers::compliance::check_handler),
        )
        .route(
            "/planning/evaluate",
            post(handlers::planning::evaluate_handler),
        )
        .route(
            "/agents/:id/control",
            post(handlers::agent_control::control_handler),
        )
        // v0.7.0 — deferred from v0.6.0 S3: dynamic log level
        .route("/log-level", get(handlers::log_level::get_level_handler))
        .route(
            "/log-level",
            post(handlers::log_level::set_level_handler),
        )
        // v0.9.0 — config hot reload
        .route("/config", get(handlers::config_reload::get_config_handler))
        .route(
            "/config/reload",
            post(handlers::config_reload::reload_handler),
        )
        // v0.28.0 — Task 17: 模拟器 API 端点
        .route("/simulator/run", post(handlers::simulator::run_handler))
        .route(
            "/simulator/scenarios",
            get(handlers::simulator::scenarios_handler),
        )
        .route(
            "/simulator/validate",
            post(handlers::simulator::validate_handler),
        )
        // v0.28.0 — Task 17: 插件市场 API 端点
        .route(
            "/plugins/market/search",
            get(handlers::plugin_market::search_handler),
        )
        .route(
            "/plugins/market/install",
            post(handlers::plugin_market::install_handler),
        );

    Router::new()
        .route("/ws", get(handlers::ws::ws_handler))
        // T029-11: Dashboard SSE 实时刷新端点
        .route(
            "/api/v1/dashboard/stream",
            get(handlers::sse::dashboard_stream),
        )
        .route("/health", get(handlers::health::health_handler))
        .route("/metrics", get(handlers::metrics::metrics_handler))
        // OpenAPI documentation endpoints (v0.10.0 — Task 8)
        .route("/api/openapi.json", get(openapi_json_handler))
        .route("/docs", get(swagger_ui_handler))
        .nest("/api", api_routes)
        .route("/", get(handlers::dashboard::dashboard_handler))
        .layer(CorsLayer::permissive())
        // TraceLayer：创建包含 trace_id 的 span，记录请求/响应日志 (T029-04)
        .layer(
            tower_http::trace::TraceLayer::new_for_http()
                .make_span_with(|request: &axum::http::Request<axum::body::Body>| {
                    let trace_id = request
                        .extensions()
                        .get::<crate::middleware::TraceId>()
                        .map(|t| t.0.as_str())
                        .unwrap_or("");
                    tracing::info_span!(
                        "http_request",
                        trace_id = %trace_id,
                        method = %request.method(),
                        uri = %request.uri()
                    )
                })
                .on_request(
                    |request: &axum::http::Request<axum::body::Body>, _span: &tracing::Span| {
                        let trace_id = request
                            .extensions()
                            .get::<crate::middleware::TraceId>()
                            .map(|t| t.0.as_str())
                            .unwrap_or("");
                        tracing::info!(
                            trace_id = %trace_id,
                            method = %request.method(),
                            path = %request.uri().path(),
                            "request started"
                        );
                    },
                )
                .on_response(
                    |response: &axum::http::Response<axum::body::Body>,
                     latency: std::time::Duration,
                     _span: &tracing::Span| {
                        tracing::info!(
                            status = %response.status().as_u16(),
                            latency_ms = latency.as_millis() as u64,
                            "request completed"
                        );
                    },
                ),
        )
        // trace_id 中间件：最外层，先生成/复用 trace_id 注入 extensions (T029-04)
        .layer(from_fn(trace_id_middleware))
        .with_state(state)
}

/// GET /api/openapi.json — return the OpenAPI 3.0 specification as JSON.
async fn openapi_json_handler() -> axum::Json<utoipa::openapi::OpenApi> {
    axum::Json(OpenApiDoc::openapi())
}

/// Swagger UI HTML page served at `/docs`.
///
/// Uses the Swagger UI CDN distribution and points to `/api/openapi.json`
/// for the API specification.
const SWAGGER_UI_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <title>EnerOS API — Swagger UI</title>
    <link rel="stylesheet" type="text/css" href="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui.css">
    <style>
        html { box-sizing: border-box; overflow: -moz-scrollbars-vertical; overflow-y: scroll; }
        body { margin: 0; background: #fafafa; }
    </style>
</head>
<body>
    <div id="swagger-ui"></div>
    <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
    <script>
        window.onload = function() {
            window.ui = SwaggerUIBundle({
                url: "/api/openapi.json",
                dom_id: "#swagger-ui",
                deepLinking: true,
                presets: [SwaggerUIBundle.presets.apis],
                layout: "BaseLayout",
            });
        };
    </script>
</body>
</html>"##;

/// GET /docs — serve the Swagger UI page.
async fn swagger_ui_handler() -> axum::response::Html<&'static str> {
    axum::response::Html(SWAGGER_UI_HTML)
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

/// Start a background task that bridges EventBus events to all connected
/// WebSocket clients (v0.6.0 — S7).
///
/// Subscribes to the EventBus broadcast channel and forwards each event as a
/// JSON-encoded message to every registered WS client. Non-blocking: failed
/// sends (client buffer full) are logged and skipped.
///
/// Returns a `JoinHandle` that can be used to abort the bridge on shutdown.
pub fn start_event_bus_ws_bridge(state: AppState) -> Option<tokio::task::JoinHandle<()>> {
    let event_bus = state.event_bus.clone()?;
    let mut rx = event_bus.subscribe();

    let handle = tokio::spawn(async move {
        tracing::info!("EventBus→WS bridge started");
        while let Ok(event) = rx.recv().await {
            // Serialize the event as a JSON object for WS clients
            let msg = serde_json::json!({
                "type": "event",
                "event_type": format!("{:?}", event.event_type),
                "id": event.id,
                "timestamp": event.timestamp.to_rfc3339(),
                "source": event.source,
                "payload": event.payload,
            });
            let msg_str = match serde_json::to_string(&msg) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("Failed to serialize event for WS: {}", e);
                    continue;
                }
            };
            broadcast_event(&state, &msg_str);
        }
        tracing::info!("EventBus→WS bridge stopped");
    });
    Some(handle)
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
        assert!(state.decision_pipeline.is_none());
        // T029-05: 验证日志级别字段初始化
        assert!(state.log_reload_handle.is_none());
        assert_eq!(*state.current_log_level.read(), "info");
        // T029-08: 验证 AgentController 字段初始化
        assert!(state.agent_controller.is_none());
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

    /// T029-11: 验证 SSE 端点路由可达且返回 text/event-stream
    #[tokio::test]
    async fn test_sse_dashboard_stream_endpoint() {
        let state = AppState::new();
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/dashboard/stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get("content-type")
            .expect("响应应包含 content-type 头")
            .to_str()
            .unwrap();
        assert!(
            content_type.starts_with("text/event-stream"),
            "SSE content-type 应为 text/event-stream"
        );
    }

    #[tokio::test]
    async fn test_dashboard_topology_svg_endpoint() {
        let state = AppState::new();
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/dashboard/topology-svg")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_dashboard_flow_heatmap_endpoint() {
        let state = AppState::new();
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/dashboard/flow-heatmap")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_topology_endpoint() {
        let state = AppState::new();
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/topology")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_agents_endpoint() {
        let state = AppState::new();
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/agents")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_constraints_endpoint() {
        let state = AppState::new();
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/constraints")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_scada_latest_endpoint() {
        let state = AppState::new();
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/scada/latest")
                    .body(Body::empty())
                    .unwrap(),
            )
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
    async fn test_openapi_json_endpoint() {
        let state = AppState::new();
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

        // Verify it's a valid OpenAPI document
        assert_eq!(json["openapi"], "3.1.0");
        assert_eq!(json["info"]["title"], "EnerOS API");
        // v0.28.0 — Task 14: OpenAPI 版本号应与 Cargo.toml 中的 CARGO_PKG_VERSION 一致
        assert_eq!(json["info"]["version"], env!("CARGO_PKG_VERSION"));

        // Verify all 6 annotated endpoints are present in the paths
        let paths = json["paths"].as_object().unwrap();
        assert!(paths.contains_key("/api/power-flow"));
        assert!(paths.contains_key("/api/analysis/opf"));
        assert!(paths.contains_key("/api/actions/structured"));
        assert!(paths.contains_key("/api/scada/latest"));
        assert!(paths.contains_key("/api/timeseries/query"));
        assert!(paths.contains_key("/api/auth/login"));
    }

    /// v0.28.0 — Task 17: 验证 OpenAPI schema 包含新增的模拟器与插件市场端点
    #[tokio::test]
    async fn test_openapi_schema_contains_new_endpoints() {
        let openapi = OpenApiDoc::openapi();
        let json = serde_json::to_value(&openapi).expect("OpenAPI 序列化为 JSON 应成功");

        // 验证 paths 中包含 5 个新端点
        let paths = json["paths"]
            .as_object()
            .expect("paths 应为 JSON 对象");
        assert!(
            paths.contains_key("/api/simulator/run"),
            "OpenAPI 应包含 POST /api/simulator/run 端点"
        );
        assert!(
            paths.contains_key("/api/simulator/scenarios"),
            "OpenAPI 应包含 GET /api/simulator/scenarios 端点"
        );
        assert!(
            paths.contains_key("/api/simulator/validate"),
            "OpenAPI 应包含 POST /api/simulator/validate 端点"
        );
        assert!(
            paths.contains_key("/api/plugins/market/search"),
            "OpenAPI 应包含 GET /api/plugins/market/search 端点"
        );
        assert!(
            paths.contains_key("/api/plugins/market/install"),
            "OpenAPI 应包含 POST /api/plugins/market/install 端点"
        );

        // 验证新端点的 HTTP 方法正确
        assert_eq!(
            json["paths"]["/api/simulator/run"]["post"]["operationId"],
            "run_handler"
        );
        assert_eq!(
            json["paths"]["/api/simulator/scenarios"]["get"]["operationId"],
            "scenarios_handler"
        );
        assert_eq!(
            json["paths"]["/api/simulator/validate"]["post"]["operationId"],
            "validate_handler"
        );
        assert_eq!(
            json["paths"]["/api/plugins/market/search"]["get"]["operationId"],
            "search_handler"
        );
        assert_eq!(
            json["paths"]["/api/plugins/market/install"]["post"]["operationId"],
            "install_handler"
        );

        // 验证 components.schemas 中包含新 schema
        let schemas = json["components"]["schemas"]
            .as_object()
            .expect("components.schemas 应为 JSON 对象");
        assert!(schemas.contains_key("SimulatorRunRequest"), "缺少 SimulatorRunRequest schema");
        assert!(schemas.contains_key("SimulatorRunResponse"), "缺少 SimulatorRunResponse schema");
        assert!(schemas.contains_key("ObservationDto"), "缺少 ObservationDto schema");
        assert!(schemas.contains_key("FaultScenarioDto"), "缺少 FaultScenarioDto schema");
        assert!(
            schemas.contains_key("SimulatorScenariosResponse"),
            "缺少 SimulatorScenariosResponse schema"
        );
        assert!(
            schemas.contains_key("SimulatorValidateRequest"),
            "缺少 SimulatorValidateRequest schema"
        );
        assert!(
            schemas.contains_key("SimulatorValidateResponse"),
            "缺少 SimulatorValidateResponse schema"
        );
        assert!(
            schemas.contains_key("MarketSearchResultEntry"),
            "缺少 MarketSearchResultEntry schema"
        );
        assert!(
            schemas.contains_key("MarketSearchResponse"),
            "缺少 MarketSearchResponse schema"
        );
        assert!(schemas.contains_key("MarketInstallRequest"), "缺少 MarketInstallRequest schema");
        assert!(
            schemas.contains_key("MarketInstallResponse"),
            "缺少 MarketInstallResponse schema"
        );
    }

    #[tokio::test]
    async fn test_swagger_ui_endpoint() {
        let state = AppState::new();
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/docs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(html.contains("swagger-ui"));
        assert!(html.contains("/api/openapi.json"));
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

    #[tokio::test]
    async fn test_event_bus_ws_bridge_no_event_bus() {
        // Without an EventBus configured, the bridge should return None
        let state = AppState::new();
        let handle = start_event_bus_ws_bridge(state);
        assert!(handle.is_none(), "bridge should be None without EventBus");
    }

    #[tokio::test]
    async fn test_event_bus_ws_bridge_forwards_events() {
        use eneros_runtime::eventbus::EventBus;
        use eneros_runtime::eventbus::event::{EventType, EventPayload};

        let event_bus = Arc::new(EventBus::new(16));
        let state = AppState::new().with_event_bus(event_bus.clone());

        // Register a WS client to receive forwarded events
        let (tx, mut rx) = mpsc::channel::<String>(100);
        let client = WsClient {
            id: "bridge-test-client".to_string(),
            sender: tx,
        };
        state.ws_clients.write().push(client);

        // Start the bridge
        let bridge_handle = start_event_bus_ws_bridge(state.clone())
            .expect("bridge should start with EventBus");

        // Give the bridge task time to subscribe
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Publish an event
        let event = eneros_runtime::eventbus::Event::new(
            EventType::SystemAlarm,
            "test-source",
            EventPayload::Message("bridge test".to_string()),
        );
        event_bus.publish(event).expect("publish should succeed");

        // The WS client should receive the forwarded event
        let msg = tokio::time::timeout(
            tokio::time::Duration::from_secs(2),
            rx.recv(),
        ).await;
        assert!(msg.is_ok(), "should receive a message before timeout");
        let msg = msg.unwrap().expect("message should not be None");
        assert!(msg.contains("SystemAlarm"), "message should contain event type");
        assert!(msg.contains("test-source"), "message should contain source");
        assert!(msg.contains("bridge test"), "message should contain payload");

        bridge_handle.abort();
    }

    #[tokio::test]
    async fn test_event_bus_ws_bridge_handles_multiple_clients() {
        use eneros_runtime::eventbus::EventBus;
        use eneros_runtime::eventbus::event::{EventType, EventPayload};

        let event_bus = Arc::new(EventBus::new(16));
        let state = AppState::new().with_event_bus(event_bus.clone());

        // Register two WS clients
        let (tx1, mut rx1) = mpsc::channel::<String>(100);
        let (tx2, mut rx2) = mpsc::channel::<String>(100);
        state.ws_clients.write().push(WsClient {
            id: "client-1".to_string(),
            sender: tx1,
        });
        state.ws_clients.write().push(WsClient {
            id: "client-2".to_string(),
            sender: tx2,
        });

        let bridge_handle = start_event_bus_ws_bridge(state.clone())
            .expect("bridge should start");
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let event = eneros_runtime::eventbus::Event::new(
            EventType::DeviceConnected,
            "device-mgr",
            EventPayload::DeviceEvent {
                device_id: "rtu-1".to_string(),
                event_type: "connected".to_string(),
            },
        );
        event_bus.publish(event).expect("publish should succeed");

        // Both clients should receive the event
        let msg1 = tokio::time::timeout(
            tokio::time::Duration::from_secs(2), rx1.recv(),
        ).await;
        let msg2 = tokio::time::timeout(
            tokio::time::Duration::from_secs(2), rx2.recv(),
        ).await;
        assert!(msg1.is_ok(), "client 1 should receive the event");
        assert!(msg2.is_ok(), "client 2 should receive the event");
        assert!(msg1.unwrap().unwrap().contains("DeviceConnected"));
        assert!(msg2.unwrap().unwrap().contains("DeviceConnected"));

        bridge_handle.abort();
    }

    // ── T029-04: trace_id 中间件测试 ──────────────────────────────────────

    #[tokio::test]
    async fn test_trace_id_header_in_response() {
        let state = AppState::new();
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        // 响应应包含 X-Trace-Id 头
        assert!(
            response.headers().contains_key("x-trace-id"),
            "response should contain X-Trace-Id header"
        );
    }

    #[tokio::test]
    async fn test_trace_id_reused_from_upstream() {
        let state = AppState::new();
        let app = create_router(state);

        let upstream_trace_id = "550e8400-e29b-41d4-a716-446655440000";
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header("x-trace-id", upstream_trace_id)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let trace_id = response
            .headers()
            .get("x-trace-id")
            .expect("response should have X-Trace-Id header")
            .to_str()
            .unwrap();
        // 上游传入的 trace_id 应被复用
        assert_eq!(
            trace_id, upstream_trace_id,
            "upstream trace_id should be reused"
        );
    }

    #[tokio::test]
    async fn test_trace_id_is_valid_uuid_when_not_provided() {
        let state = AppState::new();
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let trace_id = response
            .headers()
            .get("x-trace-id")
            .expect("response should have X-Trace-Id header")
            .to_str()
            .unwrap();
        // 未传入上游 trace_id 时，应生成有效的 UUID v4
        assert!(
            uuid::Uuid::parse_str(trace_id).is_ok(),
            "generated trace_id should be a valid UUID: {}",
            trace_id
        );
    }

    // ── T029-05: 动态日志级别 API 测试 ────────────────────────────────────

    #[tokio::test]
    async fn test_get_log_level() {
        let state = AppState::new();
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/log-level")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert!(json["level"].is_string(), "level should be a string");
        assert!(
            json["available_levels"].is_array(),
            "available_levels should be an array"
        );
    }

    #[tokio::test]
    async fn test_set_log_level_valid() {
        let state = AppState::new();
        let app = create_router(state);

        let body = serde_json::json!({"level": "debug"});
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/log-level")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(json["current_level"], "debug");
        assert_eq!(json["previous_level"], "info");
        assert_eq!(json["result"], "success");
    }

    #[tokio::test]
    async fn test_set_log_level_invalid() {
        let state = AppState::new();
        let app = create_router(state);

        let body = serde_json::json!({"level": "verbose"});
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/log-level")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_set_log_level_with_reload_handle() {
        // 测试带有 reload handle 的真实动态级别切换
        let filter = tracing_subscriber::EnvFilter::new("info");
        let (_filter_layer, reload_handle) =
            tracing_subscriber::reload::Layer::new(filter);

        let state = AppState::new()
            .with_log_reload_handle(reload_handle)
            .with_initial_log_level("info");
        let app = create_router(state);

        // 切换到 debug
        let body = serde_json::json!({"level": "debug"});
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/log-level")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(json["current_level"], "debug");
        assert_eq!(json["previous_level"], "info");
    }
}
