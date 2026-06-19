use std::net::SocketAddr;
use std::sync::Arc;

use eneros_constraint::ConstraintEngine;
use eneros_eventbus::EventBus;
use eneros_gateway::decision_pipeline::ConstrainedDecisionPipeline;
use eneros_network::PowerNetwork;
use eneros_powerflow::PowerFlowSolver;
use eneros_scada::{ScadaCollector, SnapshotBuilder};
use eneros_timeseries::TimeSeriesEngine;
use eneros_topology::TopologyEngine;

use eneros_agent::{AgentOrchestrator, DataDrivenAgentLoop};

use crate::app::{self, AppState};

/// TLS configuration for the API server (v0.7.0 — deferred from v0.6.0 S1).
#[derive(Debug, Clone)]
pub struct TlsConfig {
    /// Path to the PEM-encoded certificate file.
    pub cert_path: String,
    /// Path to the PEM-encoded private key file.
    pub key_path: String,
}

/// API server for EnerOS
pub struct ApiServer {
    state: AppState,
    addr: SocketAddr,
    /// Optional TLS configuration. When set, the server uses HTTPS.
    tls: Option<TlsConfig>,
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
            tls: None,
        }
    }

    /// Create with a custom AppState and address
    pub fn with_state(state: AppState, addr: SocketAddr) -> Self {
        Self {
            state,
            addr,
            tls: None,
        }
    }

    /// Enable TLS (v0.7.0). When set, the server uses HTTPS.
    pub fn with_tls(mut self, tls: Option<TlsConfig>) -> Self {
        self.tls = tls;
        self
    }

    /// Start the axum HTTP server. If TLS is configured, starts an HTTPS
    /// server using `tokio-rustls`; otherwise starts a plaintext HTTP server.
    pub async fn start(&self) -> anyhow::Result<()> {
        let app = app::create_router(self.state.clone());

        if let Some(ref tls) = self.tls {
            tracing::info!(
                addr = %self.addr,
                cert = %tls.cert_path,
                "EnerOS API server listening (HTTPS)"
            );
            // Load certificate and private key
            let cert_file = std::fs::File::open(&tls.cert_path)
                .map_err(|e| anyhow::anyhow!("failed to open TLS cert: {}", e))?;
            let mut reader = std::io::BufReader::new(cert_file);
            let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
                rustls_pemfile::certs(&mut reader)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| anyhow::anyhow!("failed to parse TLS cert: {}", e))?;

            let key_file = std::fs::File::open(&tls.key_path)
                .map_err(|e| anyhow::anyhow!("failed to open TLS key: {}", e))?;
            let mut key_reader = std::io::BufReader::new(key_file);
            let key = rustls_pemfile::private_key(&mut key_reader)
                .map_err(|e| anyhow::anyhow!("failed to parse TLS key: {}", e))?
                .ok_or_else(|| anyhow::anyhow!("no private key found in TLS key file"))?;

            let config = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(certs, key)
                .map_err(|e| anyhow::anyhow!("failed to build TLS config: {}", e))?;

            // Use axum_server for TLS support (standard axum 0.7 pattern)
            let rustls_config =
                axum_server::tls_rustls::RustlsConfig::from_config(std::sync::Arc::new(config));
            axum_server::bind_rustls(self.addr, rustls_config)
                .serve(app.into_make_service())
                .await?;
        } else {
            tracing::info!(addr = %self.addr, "EnerOS API server listening (HTTP)");
            let listener = tokio::net::TcpListener::bind(self.addr).await?;
            axum::serve(listener, app).await?;
        }
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

    pub fn with_decision_pipeline(mut self, pipeline: Arc<ConstrainedDecisionPipeline>) -> Self {
        self.state.decision_pipeline = Some(pipeline);
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
