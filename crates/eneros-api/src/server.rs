use std::net::SocketAddr;
use std::sync::Arc;

use eneros_constraint::ConstraintEngine;
use eneros_eventbus::EventBus;
use eneros_network::PowerNetwork;
use eneros_powerflow::PowerFlowSolver;
use eneros_scada::{ScadaCollector, SnapshotBuilder};
use eneros_timeseries::TimeSeriesEngine;
use eneros_topology::TopologyEngine;

use eneros_agent::{AgentOrchestrator, DataDrivenAgentLoop};

use crate::app::{self, AppState};

/// API server for EnerOS
pub struct ApiServer {
    state: AppState,
    addr: SocketAddr,
}

impl ApiServer {
    /// Create a new API server with default (empty) AppState
    pub fn new(host: &str, port: u16) -> Self {
        let addr = format!("{}:{}", host, port)
            .parse()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], port)));
        Self {
            state: AppState::new(),
            addr,
        }
    }

    /// Create with a custom AppState and address
    pub fn with_state(state: AppState, addr: SocketAddr) -> Self {
        Self { state, addr }
    }

    /// Start the axum HTTP server
    pub async fn start(&self) -> anyhow::Result<()> {
        let app = app::create_router(self.state.clone());

        let listener = tokio::net::TcpListener::bind(self.addr).await?;
        tracing::info!("EnerOS API server listening on {}", self.addr);

        axum::serve(listener, app).await?;
        Ok(())
    }

    // ---- Builder methods for injecting dependencies ----

    /// Inject a TopologyEngine
    pub fn with_topology_engine(mut self, engine: Arc<TopologyEngine>) -> Self {
        self.state.topology_engine = Some(engine);
        self
    }

    /// Inject a PowerFlowSolver
    pub fn with_powerflow_solver(mut self, solver: Arc<PowerFlowSolver>) -> Self {
        self.state.powerflow_solver = Some(solver);
        self
    }

    /// Inject a ConstraintEngine
    pub fn with_constraint_engine(mut self, engine: Arc<ConstraintEngine>) -> Self {
        self.state.constraint_engine = Some(engine);
        self
    }

    /// Inject a PowerNetwork
    pub fn with_network(mut self, network: Arc<PowerNetwork>) -> Self {
        self.state.network = Some(network);
        self
    }

    /// Inject a TimeSeriesEngine
    pub fn with_ts_engine(mut self, engine: Arc<TimeSeriesEngine>) -> Self {
        self.state.ts_engine = Some(engine);
        self
    }

    /// Inject a ScadaCollector
    pub fn with_scada_collector(mut self, collector: Arc<ScadaCollector>) -> Self {
        self.state.scada_collector = Some(collector);
        self
    }

    /// Inject an EventBus
    pub fn with_event_bus(mut self, bus: Arc<EventBus>) -> Self {
        self.state.event_bus = Some(bus);
        self
    }

    /// Inject an AgentOrchestrator
    pub fn with_agent_orchestrator(mut self, orchestrator: Arc<AgentOrchestrator>) -> Self {
        self.state.agent_orchestrator = Some(orchestrator);
        self
    }

    /// Inject a DataPipeline
    pub fn with_data_pipeline(mut self, pipeline: Arc<eneros_scada::DataPipeline>) -> Self {
        self.state.data_pipeline = Some(pipeline);
        self
    }

    /// Inject a SnapshotBuilder
    pub fn with_snapshot_builder(mut self, builder: Arc<SnapshotBuilder>) -> Self {
        self.state.snapshot_builder = Some(builder);
        self
    }

    /// Inject a DataDrivenAgentLoop
    pub fn with_data_driven_loop(mut self, dd_loop: Arc<DataDrivenAgentLoop>) -> Self {
        self.state.data_driven_loop = Some(dd_loop);
        self
    }

    /// Get a reference to the AppState
    pub fn state(&self) -> &AppState {
        &self.state
    }
}

impl Default for ApiServer {
    fn default() -> Self {
        Self::new("0.0.0.0", 8080)
    }
}
