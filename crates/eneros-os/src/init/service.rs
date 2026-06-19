use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Service restart policy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    Always,
    #[default]
    OnFailure,
    No,
}

/// Service status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceStatus {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
    Degraded,
}

/// Service configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    pub name: String,
    pub binary: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub restart_policy: RestartPolicy,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default = "default_graceful_timeout")]
    pub graceful_timeout_secs: u64,
}

fn default_graceful_timeout() -> u64 {
    10
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            binary: String::new(),
            args: Vec::new(),
            restart_policy: RestartPolicy::default(),
            dependencies: Vec::new(),
            env: HashMap::new(),
            working_dir: None,
            user: None,
            graceful_timeout_secs: 10,
        }
    }
}

/// A managed service instance
#[derive(Debug)]
pub struct Service {
    pub config: ServiceConfig,
    pub status: ServiceStatus,
    pub pid: Option<u32>,
    pub restart_count: u32,
    pub last_start_time: Option<chrono::DateTime<chrono::Utc>>,
}

impl Service {
    pub fn new(config: ServiceConfig) -> Self {
        Self {
            config,
            status: ServiceStatus::Stopped,
            pid: None,
            restart_count: 0,
            last_start_time: None,
        }
    }
}
