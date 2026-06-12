use serde::{Deserialize, Serialize};

/// EnerOS system configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnerOSConfig {
    /// Topology engine configuration
    pub topology: TopologyConfig,
    /// Power flow engine configuration
    pub powerflow: PowerFlowConfig,
    /// Constraint executor configuration
    pub constraint: ConstraintConfig,
    /// Time-series engine configuration
    pub timeseries: TimeSeriesConfig,
    /// Event bus configuration
    pub eventbus: EventBusConfig,
    /// Device access configuration
    pub device: DeviceConfig,
}

/// Topology engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyConfig {
    /// Maximum number of buses supported
    pub max_buses: usize,
    /// Maximum number of branches supported
    pub max_branches: usize,
    /// Enable incremental topology update
    pub incremental_update: bool,
}

/// Power flow engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerFlowConfig {
    /// Maximum iterations for Newton-Raphson
    pub max_iterations: u32,
    /// Convergence tolerance
    pub tolerance: f64,
    /// Enable N-1 analysis
    pub enable_n1_analysis: bool,
    /// Number of parallel workers for N-1
    pub n1_parallel_workers: usize,
}

/// Constraint executor configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstraintConfig {
    /// Real-time check interval in milliseconds
    pub check_interval_ms: u64,
    /// Enable automatic response to violations
    pub auto_response: bool,
    /// Maximum violation history to keep
    pub max_violation_history: usize,
}

/// Time-series engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeSeriesConfig {
    /// Data retention period in days
    pub retention_days: u32,
    /// Sampling interval in milliseconds
    pub sampling_interval_ms: u64,
    /// Enable compression
    pub enable_compression: bool,
}

/// Event bus configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventBusConfig {
    /// Maximum event queue size
    pub max_queue_size: usize,
    /// Event timeout in milliseconds
    pub timeout_ms: u64,
}

/// Device access configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    /// Maximum concurrent device connections
    pub max_connections: usize,
    /// Connection timeout in milliseconds
    pub connection_timeout_ms: u64,
    /// Data buffer size
    pub buffer_size: usize,
}

impl Default for EnerOSConfig {
    fn default() -> Self {
        Self {
            topology: TopologyConfig {
                max_buses: 100_000,
                max_branches: 200_000,
                incremental_update: true,
            },
            powerflow: PowerFlowConfig {
                max_iterations: 50,
                tolerance: 1e-6,
                enable_n1_analysis: true,
                n1_parallel_workers: 8,
            },
            constraint: ConstraintConfig {
                check_interval_ms: 100,
                auto_response: true,
                max_violation_history: 10_000,
            },
            timeseries: TimeSeriesConfig {
                retention_days: 365,
                sampling_interval_ms: 1000,
                enable_compression: true,
            },
            eventbus: EventBusConfig {
                max_queue_size: 100_000,
                timeout_ms: 5000,
            },
            device: DeviceConfig {
                max_connections: 10_000,
                connection_timeout_ms: 5000,
                buffer_size: 1024,
            },
        }
    }
}
