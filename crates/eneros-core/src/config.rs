use std::path::Path;

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
    /// Agent subsystem configuration
    pub agent: AgentConfig,
    /// Emergency response configuration
    pub emergency: EmergencyConfig,
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

/// Agent subsystem configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Maximum number of concurrent agents
    pub max_agents: usize,
    /// Agent tick interval in milliseconds
    pub tick_interval_ms: u64,
    /// Maximum execution time per agent tick in milliseconds
    pub execution_timeout_ms: u64,
    /// Enable constraint-aware action validation
    pub enable_constraint_validation: bool,
    /// Enable audit trail
    pub enable_audit_trail: bool,
}

/// Emergency response configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmergencyConfig {
    /// Frequency threshold for alert state (Hz)
    pub alert_frequency_hz: f64,
    /// Frequency threshold for emergency state (Hz)
    pub emergency_frequency_hz: f64,
    /// Voltage threshold for alert state (p.u.)
    pub alert_voltage_pu: f64,
    /// Voltage threshold for emergency state (p.u.)
    pub emergency_voltage_pu: f64,
    /// Minimum branches tripped for cascading failure detection
    pub cascading_failure_threshold: usize,
    /// Enable automatic authority escalation in emergency
    pub auto_escalation: bool,
    /// Voltage limit relaxation in emergency (p.u.) — e.g., 0.1 means ±10% instead of ±5%
    pub emergency_voltage_relaxation_pu: f64,
    /// Frequency limit relaxation in emergency (Hz) — e.g., 0.5 means ±0.5Hz instead of ±0.2Hz
    pub emergency_frequency_relaxation_hz: f64,
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
            agent: AgentConfig {
                max_agents: 1000,
                tick_interval_ms: 1000,
                execution_timeout_ms: 5000,
                enable_constraint_validation: true,
                enable_audit_trail: true,
            },
            emergency: EmergencyConfig {
                alert_frequency_hz: 49.8,
                emergency_frequency_hz: 49.5,
                alert_voltage_pu: 0.95,
                emergency_voltage_pu: 0.90,
                cascading_failure_threshold: 3,
                auto_escalation: true,
                emergency_voltage_relaxation_pu: 0.1,
                emergency_frequency_relaxation_hz: 0.5,
            },
        }
    }
}

impl EnerOSConfig {
    /// Parse configuration from a TOML string.
    pub fn load_from_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// Load configuration from a TOML file.
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        Ok(Self::load_from_str(&content)?)
    }

    /// Save configuration to a TOML file.
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> Result<(), Box<dyn std::error::Error>> {
        let content = self.to_toml_string()?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Serialize configuration to a TOML string.
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_from_str_valid() {
        let toml_str = r#"
[topology]
max_buses = 100
max_branches = 200
incremental_update = true

[powerflow]
max_iterations = 50
tolerance = 1e-6
enable_n1_analysis = true
n1_parallel_workers = 4

[constraint]
check_interval_ms = 100
auto_response = true
max_violation_history = 5000

[timeseries]
retention_days = 30
sampling_interval_ms = 500
enable_compression = false

[eventbus]
max_queue_size = 10000
timeout_ms = 3000

[device]
max_connections = 100
connection_timeout_ms = 2000
buffer_size = 2048

[agent]
max_agents = 50
tick_interval_ms = 500
execution_timeout_ms = 10000
enable_constraint_validation = false
enable_audit_trail = false

[emergency]
alert_frequency_hz = 49.5
emergency_frequency_hz = 49.0
alert_voltage_pu = 0.95
emergency_voltage_pu = 0.90
cascading_failure_threshold = 3
auto_escalation = true
emergency_voltage_relaxation_pu = 0.1
emergency_frequency_relaxation_hz = 0.5
"#;
        let config = EnerOSConfig::load_from_str(toml_str).expect("should parse valid TOML");
        assert_eq!(config.topology.max_buses, 100);
        assert_eq!(config.powerflow.max_iterations, 50);
        assert_eq!(config.agent.max_agents, 50);
        assert!(!config.timeseries.enable_compression);
    }

    #[test]
    fn test_load_from_str_invalid() {
        let toml_str = "this is not valid toml [[[";
        let result = EnerOSConfig::load_from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_from_str_missing_field() {
        let toml_str = r#"
[topology]
max_buses = 100
"#;
        let result = EnerOSConfig::load_from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join("eneros_config_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test_config.toml");

        let original = EnerOSConfig::default();
        original.save_to_file(&path).expect("should save config");

        let loaded = EnerOSConfig::load_from_file(&path).expect("should load config");

        assert_eq!(original.topology.max_buses, loaded.topology.max_buses);
        assert_eq!(original.powerflow.tolerance, loaded.powerflow.tolerance);
        assert_eq!(original.agent.max_agents, loaded.agent.max_agents);
        assert_eq!(
            original.emergency.alert_frequency_hz,
            loaded.emergency.alert_frequency_hz
        );

        // cleanup
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn test_default_values() {
        let config = EnerOSConfig::default();
        assert_eq!(config.topology.max_buses, 100_000);
        assert_eq!(config.topology.max_branches, 200_000);
        assert!(config.topology.incremental_update);
        assert_eq!(config.powerflow.max_iterations, 50);
        assert_eq!(config.powerflow.tolerance, 1e-6);
        assert!(config.powerflow.enable_n1_analysis);
        assert_eq!(config.powerflow.n1_parallel_workers, 8);
        assert_eq!(config.constraint.check_interval_ms, 100);
        assert!(config.constraint.auto_response);
        assert_eq!(config.constraint.max_violation_history, 10_000);
        assert_eq!(config.timeseries.retention_days, 365);
        assert_eq!(config.timeseries.sampling_interval_ms, 1000);
        assert!(config.timeseries.enable_compression);
        assert_eq!(config.eventbus.max_queue_size, 100_000);
        assert_eq!(config.eventbus.timeout_ms, 5000);
        assert_eq!(config.device.max_connections, 10_000);
        assert_eq!(config.device.connection_timeout_ms, 5000);
        assert_eq!(config.device.buffer_size, 1024);
        assert_eq!(config.agent.max_agents, 1000);
        assert_eq!(config.agent.tick_interval_ms, 1000);
        assert_eq!(config.agent.execution_timeout_ms, 5000);
        assert!(config.agent.enable_constraint_validation);
        assert!(config.agent.enable_audit_trail);
        assert_eq!(config.emergency.alert_frequency_hz, 49.8);
        assert_eq!(config.emergency.emergency_frequency_hz, 49.5);
        assert_eq!(config.emergency.alert_voltage_pu, 0.95);
        assert_eq!(config.emergency.emergency_voltage_pu, 0.90);
        assert_eq!(config.emergency.cascading_failure_threshold, 3);
        assert!(config.emergency.auto_escalation);
        assert_eq!(config.emergency.emergency_voltage_relaxation_pu, 0.1);
        assert_eq!(config.emergency.emergency_frequency_relaxation_hz, 0.5);
    }

    #[test]
    fn test_to_toml_string_valid() {
        let config = EnerOSConfig::default();
        let toml_str = config.to_toml_string().expect("should serialize to TOML");
        // Verify the output is valid TOML by parsing it back
        let reparsed = EnerOSConfig::load_from_str(&toml_str).expect("should parse back");
        assert_eq!(config.topology.max_buses, reparsed.topology.max_buses);
        assert_eq!(config.powerflow.tolerance, reparsed.powerflow.tolerance);
    }

    #[test]
    fn test_load_from_file_nonexistent() {
        let result = EnerOSConfig::load_from_file("/nonexistent/path/eneros.toml");
        assert!(result.is_err());
    }
}
