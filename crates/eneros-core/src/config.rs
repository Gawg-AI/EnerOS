use std::path::Path;

use serde::{Deserialize, Serialize};

/// EnerOS system configuration
///
/// This is the top-level configuration loaded from `eneros.toml`. It wires
/// together every subsystem of the Power-Native AgentOS: network model
/// source, SCADA scan points, device connections, power flow, constraints,
/// time-series, event bus, agents, and emergency response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnerOSConfig {
    /// Network model source configuration
    #[serde(default)]
    pub network: NetworkConfig,
    /// SCADA scan configuration
    #[serde(default)]
    pub scada: ScadaSourceConfig,
    /// Device connections (IEC 104 RTUs, Modbus devices, etc.)
    #[serde(default)]
    pub devices: Vec<DeviceConnectionConfig>,
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
    /// API server configuration (v0.6.0)
    #[serde(default)]
    pub api: ApiConfig,
    /// Security configuration (v0.6.0)
    #[serde(default)]
    pub security: SecurityConfig,
    /// Observability configuration (v0.6.0)
    #[serde(default)]
    pub observability: ObservabilityConfig,
}

/// Network model source configuration.
///
/// Determines how the `PowerNetwork` is loaded at startup. Three sources
/// are supported:
/// - `ieee14` — built-in IEEE 14-bus test case (default, no external deps)
/// - `cnpower` — load from cnpower equipment database via `eneros-bridge`
/// - `cim` — load from CIM/CGMES XML profile (future)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Network model source: "ieee14" | "cnpower" | "cim"
    pub source: String,
    /// Path to the network model file (for cim/cnpower file-based loaders)
    pub path: Option<String>,
    /// Whether to run an initial power flow solve after loading
    #[serde(default = "default_true")]
    pub initial_powerflow: bool,
}

fn default_true() -> bool {
    true
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            source: "ieee14".to_string(),
            path: None,
            initial_powerflow: true,
        }
    }
}

/// SCADA source configuration.
///
/// Determines which `DataSource` implementation is wired into the
/// `ScadaCollector` and `DataPipeline`. Two modes are supported:
/// - `simulated` — use `SimulatedDataSource` with built-in IEEE 14 data
///   (default, for development/testing without real RTUs)
/// - `iec104` — use `Iec104DataSource` connected to a real IEC 104 server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScadaSourceConfig {
    /// SCADA data source: "simulated" | "iec104"
    pub source: String,
    /// IEC 104 server address (only used when source = "iec104")
    #[serde(default = "default_iec104_addr")]
    pub iec104_addr: String,
    /// IEC 104 ASDU address (only used when source = "iec104")
    #[serde(default = "default_asdu_addr")]
    pub iec104_asdu: u16,
    /// Fast scan interval in milliseconds
    #[serde(default = "default_fast_interval_ms")]
    pub fast_interval_ms: u64,
    /// Normal scan interval in milliseconds
    #[serde(default = "default_normal_interval_ms")]
    pub normal_interval_ms: u64,
}

fn default_iec104_addr() -> String {
    "127.0.0.1:2404".to_string()
}

fn default_asdu_addr() -> u16 {
    1
}

fn default_fast_interval_ms() -> u64 {
    100
}

fn default_normal_interval_ms() -> u64 {
    1000
}

impl Default for ScadaSourceConfig {
    fn default() -> Self {
        Self {
            source: "simulated".to_string(),
            iec104_addr: default_iec104_addr(),
            iec104_asdu: default_asdu_addr(),
            fast_interval_ms: default_fast_interval_ms(),
            normal_interval_ms: default_normal_interval_ms(),
        }
    }
}

/// Device connection configuration for a single physical device.
///
/// Each entry in `[[devices]]` becomes a registered device in the
/// `DeviceManager`, wired to the appropriate protocol adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConnectionConfig {
    /// Unique device ID (e.g., "rtu-1", "ied-bay-3")
    pub device_id: String,
    /// Protocol: "iec104" | "iec61850" | "modbus" | "mqtt"
    pub protocol: String,
    /// Host address (IP or hostname)
    pub host: String,
    /// Port number
    pub port: u16,
    /// Protocol-specific parameters (e.g., ASDU address, slave ID)
    #[serde(default)]
    pub params: std::collections::HashMap<String, serde_json::Value>,
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
    /// 存储后端（"memory", "sqlite", "tdengine", "influxdb"），默认 "memory"
    #[serde(default = "default_storage_backend")]
    pub storage_backend: String,
}

/// 默认存储后端：内存
fn default_storage_backend() -> String {
    "memory".to_string()
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

/// API server configuration (v0.6.0).
///
/// Controls the HTTP/WS server bind address, TLS, and request limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    /// HTTP server bind address (default "0.0.0.0")
    #[serde(default = "default_bind_host")]
    pub host: String,
    /// HTTP server bind port (default 8080)
    #[serde(default = "default_bind_port")]
    pub port: u16,
    /// Maximum request body size in bytes (default 10 MB)
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: usize,
    /// Request timeout in seconds (default 30)
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
    /// Enable TLS (requires cert_path and key_path)
    #[serde(default)]
    pub enable_tls: bool,
    /// Path to TLS certificate file (PEM)
    pub tls_cert_path: Option<String>,
    /// Path to TLS private key file (PEM)
    pub tls_key_path: Option<String>,
}

fn default_bind_host() -> String {
    "0.0.0.0".to_string()
}
fn default_bind_port() -> u16 {
    8080
}
fn default_max_body_bytes() -> usize {
    10 * 1024 * 1024
}
fn default_request_timeout_secs() -> u64 {
    30
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            host: default_bind_host(),
            port: default_bind_port(),
            max_body_bytes: default_max_body_bytes(),
            request_timeout_secs: default_request_timeout_secs(),
            enable_tls: false,
            tls_cert_path: None,
            tls_key_path: None,
        }
    }
}

/// Security configuration (v0.6.0).
///
/// Controls authentication (JWT/API Key), RBAC, and audit logging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Enable authentication (default false for backward compat — set true in production)
    #[serde(default)]
    pub enable_auth: bool,
    /// JWT signing secret (HS256). Required when enable_auth=true.
    pub jwt_secret: Option<String>,
    /// JWT token lifetime in seconds (default 3600 = 1h)
    #[serde(default = "default_jwt_ttl_secs")]
    pub jwt_ttl_secs: u64,
    /// JWT refresh token lifetime in seconds (default 86400 = 24h)
    #[serde(default = "default_jwt_refresh_secs")]
    pub jwt_refresh_secs: u64,
    /// Static API keys (key → role mapping). When empty, only JWT is used.
    #[serde(default)]
    pub api_keys: Vec<ApiKeyEntry>,
    /// Enable audit logging for all write operations
    #[serde(default = "default_true")]
    pub enable_audit: bool,
    /// Audit log file path (when None, logs go to tracing)
    pub audit_log_path: Option<String>,
}

fn default_jwt_ttl_secs() -> u64 {
    3600
}
fn default_jwt_refresh_secs() -> u64 {
    86400
}

/// A static API key entry with associated role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyEntry {
    /// The API key string (passed in X-API-Key header)
    pub key: String,
    /// Role assigned to this key: "observer" | "operator" | "supervisor" | "emergency"
    pub role: String,
    /// Optional description
    #[serde(default)]
    pub description: String,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            enable_auth: false,
            jwt_secret: None,
            jwt_ttl_secs: default_jwt_ttl_secs(),
            jwt_refresh_secs: default_jwt_refresh_secs(),
            api_keys: Vec::new(),
            enable_audit: true,
            audit_log_path: None,
        }
    }
}

/// Observability configuration (v0.6.0).
///
/// Controls metrics export, structured logging, and tracing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservabilityConfig {
    /// Enable Prometheus metrics endpoint at /metrics
    #[serde(default = "default_true")]
    pub enable_metrics: bool,
    /// Enable structured JSON logging
    #[serde(default = "default_true")]
    pub enable_json_logging: bool,
    /// Log level: "trace" | "debug" | "info" | "warn" | "error"
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// Enable distributed tracing (trace_id propagation)
    #[serde(default)]
    pub enable_tracing: bool,
    /// OTLP endpoint for trace export (e.g., "http://localhost:4317")
    /// Only used when enable_tracing = true
    #[serde(default)]
    pub otel_endpoint: Option<String>,
    /// Service name for OpenTelemetry traces (default: "eneros")
    #[serde(default = "default_otel_service_name")]
    pub otel_service_name: String,
    /// Metrics retention in seconds (how long metrics are kept in memory)
    #[serde(default = "default_metrics_retention_secs")]
    pub metrics_retention_secs: u64,
}

fn default_log_level() -> String {
    "info".to_string()
}
fn default_metrics_retention_secs() -> u64 {
    300
}
fn default_otel_service_name() -> String {
    "eneros".to_string()
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            enable_metrics: true,
            enable_json_logging: true,
            log_level: default_log_level(),
            enable_tracing: false,
            otel_endpoint: None,
            otel_service_name: default_otel_service_name(),
            metrics_retention_secs: default_metrics_retention_secs(),
        }
    }
}

impl Default for EnerOSConfig {
    fn default() -> Self {
        Self {
            network: NetworkConfig::default(),
            scada: ScadaSourceConfig::default(),
            devices: Vec::new(),
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
                storage_backend: "memory".to_string(),
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
            api: ApiConfig::default(),
            security: SecurityConfig::default(),
            observability: ObservabilityConfig::default(),
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

    /// Load configuration from a file, then apply environment variable overrides.
    ///
    /// Environment variables follow the pattern `ENEROS_<SECTION>__<FIELD>`:
    /// - `ENEROS_NETWORK__SOURCE=cnpower` → `config.network.source = "cnpower"`
    /// - `ENEROS_API__PORT=9090` → `config.api.port = 9090`
    /// - `ENEROS_SECURITY__ENABLE_AUTH=true` → `config.security.enable_auth = true`
    ///
    /// Nested fields use double underscore `__` as separator. Values are
    /// parsed as TOML literals (so `true`, `42`, `1e-6`, `"string"` all work).
    ///
    /// If the file does not exist, returns the default config with env overrides applied.
    pub fn load_with_env_overrides(
        path: Option<&str>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut config = match path {
            Some(p) => Self::load_from_file(p)?,
            None => Self::default(),
        };
        config.apply_env_overrides()?;
        config.validate()?;
        Ok(config)
    }

    /// Apply environment variable overrides to this configuration.
    ///
    /// Scans all environment variables starting with `ENEROS_` and applies
    /// them to the corresponding config field using TOML deserialization.
    #[allow(clippy::type_complexity)]
    pub fn apply_env_overrides(&mut self) -> Result<(), ConfigError> {
        let prefixes: &[(&str, &dyn Fn(&mut Self, &str) -> Result<(), ConfigError>)] = &[
            ("NETWORK__SOURCE", &|c, v| {
                c.network.source = parse_toml_value(v)?;
                Ok(())
            }),
            ("NETWORK__PATH", &|c, v| {
                c.network.path = Some(parse_toml_value(v)?);
                Ok(())
            }),
            ("NETWORK__INITIAL_POWERFLOW", &|c, v| {
                c.network.initial_powerflow = parse_toml_value(v)?;
                Ok(())
            }),
            ("SCADA__SOURCE", &|c, v| {
                c.scada.source = parse_toml_value(v)?;
                Ok(())
            }),
            ("SCADA__IEC104_ADDR", &|c, v| {
                c.scada.iec104_addr = parse_toml_value(v)?;
                Ok(())
            }),
            ("SCADA__IEC104_ASDU", &|c, v| {
                c.scada.iec104_asdu = parse_toml_value(v)?;
                Ok(())
            }),
            ("SCADA__FAST_INTERVAL_MS", &|c, v| {
                c.scada.fast_interval_ms = parse_toml_value(v)?;
                Ok(())
            }),
            ("SCADA__NORMAL_INTERVAL_MS", &|c, v| {
                c.scada.normal_interval_ms = parse_toml_value(v)?;
                Ok(())
            }),
            ("POWERFLOW__MAX_ITERATIONS", &|c, v| {
                c.powerflow.max_iterations = parse_toml_value(v)?;
                Ok(())
            }),
            ("POWERFLOW__TOLERANCE", &|c, v| {
                c.powerflow.tolerance = parse_toml_value(v)?;
                Ok(())
            }),
            ("POWERFLOW__ENABLE_N1_ANALYSIS", &|c, v| {
                c.powerflow.enable_n1_analysis = parse_toml_value(v)?;
                Ok(())
            }),
            ("CONSTRAINT__CHECK_INTERVAL_MS", &|c, v| {
                c.constraint.check_interval_ms = parse_toml_value(v)?;
                Ok(())
            }),
            ("CONSTRAINT__AUTO_RESPONSE", &|c, v| {
                c.constraint.auto_response = parse_toml_value(v)?;
                Ok(())
            }),
            ("TIMESERIES__RETENTION_DAYS", &|c, v| {
                c.timeseries.retention_days = parse_toml_value(v)?;
                Ok(())
            }),
            ("TIMESERIES__SAMPLING_INTERVAL_MS", &|c, v| {
                c.timeseries.sampling_interval_ms = parse_toml_value(v)?;
                Ok(())
            }),
            ("EVENTBUS__MAX_QUEUE_SIZE", &|c, v| {
                c.eventbus.max_queue_size = parse_toml_value(v)?;
                Ok(())
            }),
            ("EVENTBUS__TIMEOUT_MS", &|c, v| {
                c.eventbus.timeout_ms = parse_toml_value(v)?;
                Ok(())
            }),
            ("AGENT__MAX_AGENTS", &|c, v| {
                c.agent.max_agents = parse_toml_value(v)?;
                Ok(())
            }),
            ("AGENT__TICK_INTERVAL_MS", &|c, v| {
                c.agent.tick_interval_ms = parse_toml_value(v)?;
                Ok(())
            }),
            ("AGENT__EXECUTION_TIMEOUT_MS", &|c, v| {
                c.agent.execution_timeout_ms = parse_toml_value(v)?;
                Ok(())
            }),
            ("AGENT__ENABLE_CONSTRAINT_VALIDATION", &|c, v| {
                c.agent.enable_constraint_validation = parse_toml_value(v)?;
                Ok(())
            }),
            ("AGENT__ENABLE_AUDIT_TRAIL", &|c, v| {
                c.agent.enable_audit_trail = parse_toml_value(v)?;
                Ok(())
            }),
            ("API__HOST", &|c, v| {
                c.api.host = parse_toml_value(v)?;
                Ok(())
            }),
            ("API__PORT", &|c, v| {
                c.api.port = parse_toml_value(v)?;
                Ok(())
            }),
            ("API__ENABLE_TLS", &|c, v| {
                c.api.enable_tls = parse_toml_value(v)?;
                Ok(())
            }),
            ("API__TLS_CERT_PATH", &|c, v| {
                c.api.tls_cert_path = Some(parse_toml_value(v)?);
                Ok(())
            }),
            ("API__TLS_KEY_PATH", &|c, v| {
                c.api.tls_key_path = Some(parse_toml_value(v)?);
                Ok(())
            }),
            ("SECURITY__ENABLE_AUTH", &|c, v| {
                c.security.enable_auth = parse_toml_value(v)?;
                Ok(())
            }),
            ("SECURITY__JWT_SECRET", &|c, v| {
                c.security.jwt_secret = Some(parse_toml_value(v)?);
                Ok(())
            }),
            ("SECURITY__JWT_TTL_SECS", &|c, v| {
                c.security.jwt_ttl_secs = parse_toml_value(v)?;
                Ok(())
            }),
            ("SECURITY__ENABLE_AUDIT", &|c, v| {
                c.security.enable_audit = parse_toml_value(v)?;
                Ok(())
            }),
            ("OBSERVABILITY__ENABLE_METRICS", &|c, v| {
                c.observability.enable_metrics = parse_toml_value(v)?;
                Ok(())
            }),
            ("OBSERVABILITY__ENABLE_JSON_LOGGING", &|c, v| {
                c.observability.enable_json_logging = parse_toml_value(v)?;
                Ok(())
            }),
            ("OBSERVABILITY__LOG_LEVEL", &|c, v| {
                c.observability.log_level = parse_toml_value(v)?;
                Ok(())
            }),
            ("OBSERVABILITY__ENABLE_TRACING", &|c, v| {
                c.observability.enable_tracing = parse_toml_value(v)?;
                Ok(())
            }),
            ("OBSERVABILITY__OTEL_ENDPOINT", &|c, v| {
                c.observability.otel_endpoint = Some(parse_toml_value(v)?);
                Ok(())
            }),
            ("OBSERVABILITY__OTEL_SERVICE_NAME", &|c, v| {
                c.observability.otel_service_name = parse_toml_value(v)?;
                Ok(())
            }),
        ];

        for (suffix, apply_fn) in prefixes {
            let env_key = format!("ENEROS_{}", suffix);
            if let Ok(value) = std::env::var(&env_key) {
                apply_fn(self, &value).map_err(|e| {
                    ConfigError::EnvOverrideFailed {
                        key: env_key.clone(),
                        value: value.clone(),
                        reason: e.to_string(),
                    }
                })?;
            }
        }

        Ok(())
    }

    /// Validate the configuration, returning a list of errors if invalid.
    ///
    /// Checks performed:
    /// - `network.source` must be one of: ieee14, cnpower, cim
    /// - `scada.source` must be one of: simulated, iec104
    /// - `powerflow.max_iterations` > 0
    /// - `powerflow.tolerance` > 0
    /// - `powerflow.n1_parallel_workers` > 0
    /// - `constraint.check_interval_ms` > 0
    /// - `timeseries.retention_days` > 0
    /// - `timeseries.sampling_interval_ms` > 0
    /// - `eventbus.max_queue_size` > 0
    /// - `agent.max_agents` > 0
    /// - `agent.tick_interval_ms` > 0
    /// - `api.port` > 0
    /// - If `security.enable_auth` = true, `security.jwt_secret` must be set
    /// - If `api.enable_tls` = true, both `tls_cert_path` and `tls_key_path` must be set
    /// - `observability.log_level` must be one of: trace, debug, info, warn, error
    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut errors = Vec::new();

        // Network source validation
        match self.network.source.as_str() {
            "ieee14" | "cnpower" | "cim" => {}
            other => errors.push(format!(
                "network.source: invalid value '{}', must be one of: ieee14, cnpower, cim",
                other
            )),
        }

        // SCADA source validation
        match self.scada.source.as_str() {
            "simulated" | "iec104" => {}
            other => errors.push(format!(
                "scada.source: invalid value '{}', must be one of: simulated, iec104",
                other
            )),
        }

        // Numeric range validations
        if self.powerflow.max_iterations == 0 {
            errors.push("powerflow.max_iterations: must be > 0".to_string());
        }
        if self.powerflow.tolerance <= 0.0 {
            errors.push("powerflow.tolerance: must be > 0".to_string());
        }
        if self.powerflow.n1_parallel_workers == 0 {
            errors.push("powerflow.n1_parallel_workers: must be > 0".to_string());
        }
        if self.constraint.check_interval_ms == 0 {
            errors.push("constraint.check_interval_ms: must be > 0".to_string());
        }
        if self.timeseries.retention_days == 0 {
            errors.push("timeseries.retention_days: must be > 0".to_string());
        }
        if self.timeseries.sampling_interval_ms == 0 {
            errors.push("timeseries.sampling_interval_ms: must be > 0".to_string());
        }
        if self.eventbus.max_queue_size == 0 {
            errors.push("eventbus.max_queue_size: must be > 0".to_string());
        }
        if self.agent.max_agents == 0 {
            errors.push("agent.max_agents: must be > 0".to_string());
        }
        if self.agent.tick_interval_ms == 0 {
            errors.push("agent.tick_interval_ms: must be > 0".to_string());
        }
        if self.api.port == 0 {
            errors.push("api.port: must be > 0".to_string());
        }

        // Security validation
        if self.security.enable_auth && self.security.jwt_secret.is_none() {
            errors.push(
                "security.jwt_secret: required when security.enable_auth = true".to_string(),
            );
        }

        // TLS validation
        if self.api.enable_tls {
            if self.api.tls_cert_path.is_none() {
                errors.push("api.tls_cert_path: required when api.enable_tls = true".to_string());
            }
            if self.api.tls_key_path.is_none() {
                errors.push("api.tls_key_path: required when api.enable_tls = true".to_string());
            }
        }

        // Log level validation
        match self.observability.log_level.as_str() {
            "trace" | "debug" | "info" | "warn" | "error" => {}
            other => errors.push(format!(
                "observability.log_level: invalid value '{}', must be one of: trace, debug, info, warn, error",
                other
            )),
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(ConfigError::ValidationFailed { errors })
        }
    }
}

/// Configuration error type.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Environment variable override failed (could not parse value)
    #[error("env override failed for {key}={value}: {reason}")]
    EnvOverrideFailed {
        key: String,
        value: String,
        reason: String,
    },
    /// Configuration validation failed (multiple errors)
    #[error("configuration validation failed:\n  - {}", errors.join("\n  - "))]
    ValidationFailed { errors: Vec<String> },
}

/// Parse a TOML literal value from a string.
///
/// This wraps the value in a minimal TOML document so that the `toml` crate
/// can parse it. Strings should be passed without quotes (they're auto-quoted),
/// while numbers and booleans are passed as-is.
fn parse_toml_value<T: serde::de::DeserializeOwned>(s: &str) -> Result<T, ConfigError> {
    // Auto-detect: if the value is already a TOML literal (quoted string,
    // boolean, or number), parse it directly; otherwise treat as a string
    // and auto-quote it.
    let toml_doc = if s.starts_with('"')
        || s.starts_with('\'')
        || s.eq_ignore_ascii_case("true")
        || s.eq_ignore_ascii_case("false")
        || s.parse::<f64>().is_ok()
    {
        format!("__v__ = {}", s)
    } else {
        // Treat as string
        format!("__v__ = \"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    };

    #[derive(serde::Deserialize)]
    struct Wrapper<T> {
        __v__: T,
    }

    toml::from_str::<Wrapper<T>>(&toml_doc)
        .map(|w| w.__v__)
        .map_err(|e| ConfigError::EnvOverrideFailed {
            key: String::new(),
            value: s.to_string(),
            reason: e.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

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

    #[test]
    fn test_validate_default_config() {
        let config = EnerOSConfig::default();
        assert!(config.validate().is_ok(), "default config should be valid");
    }

    #[test]
    fn test_validate_invalid_network_source() {
        let mut config = EnerOSConfig::default();
        config.network.source = "invalid".to_string();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("network.source"));
    }

    #[test]
    fn test_validate_invalid_scada_source() {
        let mut config = EnerOSConfig::default();
        config.scada.source = "invalid".to_string();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("scada.source"));
    }

    #[test]
    fn test_validate_zero_max_iterations() {
        let mut config = EnerOSConfig::default();
        config.powerflow.max_iterations = 0;
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("powerflow.max_iterations"));
    }

    #[test]
    fn test_validate_auth_without_secret() {
        let mut config = EnerOSConfig::default();
        config.security.enable_auth = true;
        config.security.jwt_secret = None;
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("jwt_secret"));
    }

    #[test]
    fn test_validate_tls_without_cert() {
        let mut config = EnerOSConfig::default();
        config.api.enable_tls = true;
        config.api.tls_cert_path = None;
        config.api.tls_key_path = None;
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("tls_cert_path"));
        assert!(err.to_string().contains("tls_key_path"));
    }

    #[test]
    fn test_validate_invalid_log_level() {
        let mut config = EnerOSConfig::default();
        config.observability.log_level = "verbose".to_string();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("log_level"));
    }

    #[test]
    fn test_validate_multiple_errors() {
        let mut config = EnerOSConfig::default();
        config.network.source = "bad".to_string();
        config.powerflow.max_iterations = 0;
        config.api.port = 0;
        let err = config.validate().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("network.source"));
        assert!(msg.contains("max_iterations"));
        assert!(msg.contains("api.port"));
    }

    #[test]
    fn test_validate_auth_with_secret() {
        let mut config = EnerOSConfig::default();
        config.security.enable_auth = true;
        config.security.jwt_secret = Some("my-secret".to_string());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_tls_with_paths() {
        let mut config = EnerOSConfig::default();
        config.api.enable_tls = true;
        config.api.tls_cert_path = Some("/path/cert.pem".to_string());
        config.api.tls_key_path = Some("/path/key.pem".to_string());
        assert!(config.validate().is_ok());
    }

    #[test]
    #[serial]
    fn test_apply_env_overrides_string() {
        // Use a unique port to avoid conflicts with other tests
        std::env::set_var("ENEROS_NETWORK__SOURCE", "cnpower");
        std::env::set_var("ENEROS_API__PORT", "9090");
        std::env::set_var("ENEROS_SECURITY__ENABLE_AUTH", "true");

        let mut config = EnerOSConfig::default();
        config.apply_env_overrides().unwrap();

        assert_eq!(config.network.source, "cnpower");
        assert_eq!(config.api.port, 9090);
        assert!(config.security.enable_auth);

        // Cleanup
        std::env::remove_var("ENEROS_NETWORK__SOURCE");
        std::env::remove_var("ENEROS_API__PORT");
        std::env::remove_var("ENEROS_SECURITY__ENABLE_AUTH");
    }

    #[test]
    #[serial]
    fn test_apply_env_overrides_float_and_bool() {
        // Test float and bool overrides together (single test to avoid env var races)
        std::env::set_var("ENEROS_POWERFLOW__TOLERANCE", "1e-8");
        std::env::set_var("ENEROS_OBSERVABILITY__ENABLE_METRICS", "false");

        let mut config = EnerOSConfig::default();
        config.apply_env_overrides().unwrap();

        assert!(
            (config.powerflow.tolerance - 1e-8).abs() < 1e-15,
            "tolerance should be 1e-8, got {}",
            config.powerflow.tolerance
        );
        assert!(
            !config.observability.enable_metrics,
            "enable_metrics should be false"
        );

        std::env::remove_var("ENEROS_POWERFLOW__TOLERANCE");
        std::env::remove_var("ENEROS_OBSERVABILITY__ENABLE_METRICS");
    }

    #[test]
    #[serial]
    fn test_apply_env_overrides_no_vars() {
        // Ensure no ENEROS_ vars are set — clean up any that might exist from other tests
        let keys_to_remove: Vec<String> = std::env::vars()
            .filter(|(k, _)| k.starts_with("ENEROS_"))
            .map(|(k, _)| k)
            .collect();
        for key in keys_to_remove {
            std::env::remove_var(&key);
        }

        let mut config = EnerOSConfig::default();
        let original = config.clone();
        config.apply_env_overrides().unwrap();
        assert_eq!(config.network.source, original.network.source);
        assert_eq!(config.api.port, original.api.port);
    }

    #[test]
    #[serial]
    fn test_load_with_env_overrides_validates() {
        std::env::set_var("ENEROS_API__PORT", "8888");

        let config = EnerOSConfig::load_with_env_overrides(None).unwrap();
        assert_eq!(config.api.port, 8888);
        // validate() was called internally and passed
        assert_eq!(config.network.source, "ieee14");

        std::env::remove_var("ENEROS_API__PORT");
    }

    #[test]
    #[serial]
    fn test_load_with_env_overrides_validation_fails() {
        std::env::set_var("ENEROS_NETWORK__SOURCE", "invalid_source");

        let result = EnerOSConfig::load_with_env_overrides(None);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("network.source"));

        std::env::remove_var("ENEROS_NETWORK__SOURCE");
    }

    #[test]
    fn test_parse_toml_value_string() {
        let v: String = parse_toml_value("hello world").unwrap();
        assert_eq!(v, "hello world");
    }

    #[test]
    fn test_parse_toml_value_integer() {
        let v: u64 = parse_toml_value("42").unwrap();
        assert_eq!(v, 42);
    }

    #[test]
    fn test_parse_toml_value_float() {
        let v: f64 = parse_toml_value("1.5").unwrap();
        assert!((v - 1.5).abs() < 1e-10);
    }

    #[test]
    fn test_parse_toml_value_bool() {
        let v: bool = parse_toml_value("true").unwrap();
        assert!(v);

        let v: bool = parse_toml_value("false").unwrap();
        assert!(!v);
    }

    #[test]
    fn test_parse_toml_value_quoted_string() {
        let v: String = parse_toml_value("\"quoted value\"").unwrap();
        assert_eq!(v, "quoted value");
    }

    #[test]
    fn test_new_config_sections_default() {
        let config = EnerOSConfig::default();
        assert_eq!(config.api.host, "0.0.0.0");
        assert_eq!(config.api.port, 8080);
        assert!(!config.api.enable_tls);
        assert!(!config.security.enable_auth);
        assert!(config.security.enable_audit);
        assert!(config.observability.enable_metrics);
        assert!(config.observability.enable_json_logging);
        assert_eq!(config.observability.log_level, "info");
    }

    #[test]
    fn test_config_with_new_sections_roundtrip() {
        let mut config = EnerOSConfig::default();
        config.api.port = 9090;
        config.security.enable_auth = true;
        config.security.jwt_secret = Some("secret123".to_string());
        config.observability.log_level = "debug".to_string();

        let toml_str = config.to_toml_string().unwrap();
        let reparsed = EnerOSConfig::load_from_str(&toml_str).unwrap();

        assert_eq!(reparsed.api.port, 9090);
        assert!(reparsed.security.enable_auth);
        assert_eq!(reparsed.security.jwt_secret.as_deref(), Some("secret123"));
        assert_eq!(reparsed.observability.log_level, "debug");
    }
}
