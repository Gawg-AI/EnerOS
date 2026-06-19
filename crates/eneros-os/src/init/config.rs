//! Init configuration loading
//!
//! Loads service configuration from `/etc/eneros/init.toml` (or a custom path)
//! and supports environment variable overrides for selected fields.

use crate::agentos::quota::QuotaConfig;
use crate::agentos::registry::AgentType;
use crate::agentos::scheduler::SchedulingPolicy;
use crate::init::service::ServiceConfig;
use eneros_core::AuthorityLevel;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

/// Top-level init configuration file schema.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InitConfig {
    /// Service definitions in startup-agnostic order (dependencies drive ordering).
    #[serde(default)]
    pub services: Vec<ServiceConfig>,
    /// Agent process definitions (started after system services).
    #[serde(default)]
    pub agents: Vec<AgentServiceConfig>,
}

/// Agent process configuration for the init system.
///
/// Each entry describes one Agent process to be spawned by `AgentSupervisor`
/// after all system services are running. The Agent is registered in
/// `AgentRegistry`, scheduled by `AgentScheduler`, granted capabilities by
/// `AuthorityEnforcer`, and resource-limited by `ResourceQuota`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentServiceConfig {
    /// Unique Agent identifier (registered in AgentRegistry).
    pub agent_id: String,
    /// Agent type classification (drives default scheduling policy).
    pub agent_type: AgentType,
    /// Authority level (mapped to Linux capabilities by AuthorityEnforcer).
    pub authority: AuthorityLevel,
    /// Binary path, e.g. "/bin/eneros-dispatch-agent".
    pub binary: String,
    /// Command-line arguments passed to the binary.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables for the Agent process.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Scheduling policy (Normal or Realtime/SCHED_FIFO).
    #[serde(default)]
    pub scheduling_policy: SchedulingPolicy,
    /// Resource quota (cgroups v2 CPU/memory/PID limits).
    #[serde(default)]
    pub resource_quota: QuotaConfig,
    /// System services that must be running before this Agent starts.
    #[serde(default)]
    pub dependencies: Vec<String>,
}

impl Default for AgentServiceConfig {
    fn default() -> Self {
        Self {
            agent_id: String::new(),
            agent_type: AgentType::Custom("unknown".to_string()),
            authority: AuthorityLevel::Observer,
            binary: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            scheduling_policy: SchedulingPolicy::default(),
            resource_quota: QuotaConfig::default(),
            dependencies: Vec::new(),
        }
    }
}

/// Errors that can occur while loading init configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file {0}: {1}")]
    Io(String, String),
    #[error("failed to parse config file {0}: {1}")]
    Parse(String, String),
    #[error("invalid config: {0}")]
    Invalid(String),
}

impl InitConfig {
    /// Load configuration from a TOML file at the given path.
    pub fn load_from_file(path: &str) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            ConfigError::Io(path.to_string(), e.to_string())
        })?;
        let mut cfg: InitConfig = toml::from_str(&content).map_err(|e| {
            ConfigError::Parse(path.to_string(), e.to_string())
        })?;
        cfg.apply_env_overrides();
        cfg.validate()?;
        Ok(cfg)
    }

    /// Load configuration from a path, falling back to the default
    /// (empty) configuration if the file does not exist.
    pub fn load_from_file_or_default(path: &str) -> Self {
        if Path::new(path).exists() {
            match Self::load_from_file(path) {
                Ok(cfg) => cfg,
                Err(e) => {
                    tracing::warn!(
                        "Failed to load init config from {}: {} — using default",
                        path,
                        e
                    );
                    Self::load_default()
                }
            }
        } else {
            tracing::info!(
                "Init config file {} not found — using built-in default services",
                path
            );
            Self::load_default()
        }
    }

    /// Built-in default service configuration used when no config file is
    /// present. Mirrors the historical hardcoded service set so that the
    /// system remains bootable out of the box.
    pub fn load_default() -> Self {
        use crate::init::service::RestartPolicy;
        use std::collections::HashMap;

        let mut env = HashMap::new();
        env.insert("RUST_LOG".to_string(), "info".to_string());

        let services = vec![
            ServiceConfig {
                name: "network".to_string(),
                binary: "/bin/eneros-netcfg".to_string(),
                restart_policy: RestartPolicy::Always,
                env: env.clone(),
                ..Default::default()
            },
            ServiceConfig {
                name: "timesync".to_string(),
                binary: "/bin/eneros-timesync".to_string(),
                dependencies: vec!["network".to_string()],
                restart_policy: RestartPolicy::Always,
                ..Default::default()
            },
            ServiceConfig {
                name: "syslog".to_string(),
                binary: "/bin/eneros-syslog".to_string(),
                restart_policy: RestartPolicy::Always,
                ..Default::default()
            },
            ServiceConfig {
                name: "devmgr".to_string(),
                binary: "/bin/eneros-devmgr".to_string(),
                restart_policy: RestartPolicy::Always,
                ..Default::default()
            },
            ServiceConfig {
                name: "power-app".to_string(),
                binary: "/bin/eneros-api".to_string(),
                args: vec![
                    "run".to_string(),
                    "--config".to_string(),
                    "/etc/eneros/eneros.toml".to_string(),
                ],
                dependencies: vec![
                    "network".to_string(),
                    "timesync".to_string(),
                    "syslog".to_string(),
                    "devmgr".to_string(),
                ],
                restart_policy: RestartPolicy::OnFailure,
                ..Default::default()
            },
        ];

        // Default Agent processes — started after system services.
        // Each Agent runs as an independent OS process managed by AgentSupervisor.
        let agents = vec![
            AgentServiceConfig {
                agent_id: "dispatch-1".to_string(),
                agent_type: AgentType::Dispatch,
                authority: AuthorityLevel::Supervisor,
                binary: "/bin/eneros-dispatch-agent".to_string(),
                args: vec![
                    "--agent-id".to_string(),
                    "dispatch-1".to_string(),
                    "--eventbus-addr".to_string(),
                    "127.0.0.1:9876".to_string(),
                    "--gateway-addr".to_string(),
                    "127.0.0.1:9877".to_string(),
                ],
                env: env.clone(),
                scheduling_policy: SchedulingPolicy::default_for_agent_type(&AgentType::Dispatch),
                resource_quota: QuotaConfig::limited(50, 512, 100),
                dependencies: vec!["power-app".to_string()],
            },
            AgentServiceConfig {
                agent_id: "forecast-1".to_string(),
                agent_type: AgentType::Forecast,
                authority: AuthorityLevel::Observer,
                binary: "/bin/eneros-forecast-agent".to_string(),
                args: vec![
                    "--agent-id".to_string(),
                    "forecast-1".to_string(),
                    "--eventbus-addr".to_string(),
                    "127.0.0.1:9876".to_string(),
                    "--gateway-addr".to_string(),
                    "127.0.0.1:9877".to_string(),
                ],
                env: env.clone(),
                scheduling_policy: SchedulingPolicy::default_for_agent_type(&AgentType::Forecast),
                resource_quota: QuotaConfig::limited(30, 512, 100),
                dependencies: vec!["power-app".to_string()],
            },
            AgentServiceConfig {
                agent_id: "operation-1".to_string(),
                agent_type: AgentType::Operation,
                authority: AuthorityLevel::Supervisor,
                binary: "/bin/eneros-operation-agent".to_string(),
                args: vec![
                    "--agent-id".to_string(),
                    "operation-1".to_string(),
                    "--eventbus-addr".to_string(),
                    "127.0.0.1:9876".to_string(),
                    "--gateway-addr".to_string(),
                    "127.0.0.1:9877".to_string(),
                ],
                env: env.clone(),
                scheduling_policy: SchedulingPolicy::default_for_agent_type(&AgentType::Operation),
                resource_quota: QuotaConfig::limited(50, 512, 100),
                dependencies: vec!["power-app".to_string()],
            },
            AgentServiceConfig {
                agent_id: "self-healing-1".to_string(),
                agent_type: AgentType::SelfHealing,
                authority: AuthorityLevel::Emergency,
                binary: "/bin/eneros-self-healing-agent".to_string(),
                args: vec![
                    "--agent-id".to_string(),
                    "self-healing-1".to_string(),
                    "--eventbus-addr".to_string(),
                    "127.0.0.1:9876".to_string(),
                    "--gateway-addr".to_string(),
                    "127.0.0.1:9877".to_string(),
                    "--tick-interval-ms".to_string(),
                    "500".to_string(),
                ],
                env: env.clone(),
                // RT 进程：SCHED_FIFO 优先级 80，CPU 隔离 [2,3]，mlockall
                scheduling_policy: SchedulingPolicy::default_for_agent_type(&AgentType::SelfHealing),
                resource_quota: QuotaConfig::limited(80, 1024, 50),
                dependencies: vec!["power-app".to_string()],
            },
            AgentServiceConfig {
                agent_id: "planning-1".to_string(),
                agent_type: AgentType::Planning,
                authority: AuthorityLevel::Operator,
                binary: "/bin/eneros-planning-agent".to_string(),
                args: vec![
                    "--agent-id".to_string(),
                    "planning-1".to_string(),
                    "--eventbus-addr".to_string(),
                    "127.0.0.1:9876".to_string(),
                    "--gateway-addr".to_string(),
                    "127.0.0.1:9877".to_string(),
                ],
                env: env.clone(),
                scheduling_policy: SchedulingPolicy::default_for_agent_type(&AgentType::Planning),
                resource_quota: QuotaConfig::limited(30, 512, 100),
                dependencies: vec!["power-app".to_string()],
            },
            AgentServiceConfig {
                agent_id: "trading-1".to_string(),
                agent_type: AgentType::Trading,
                authority: AuthorityLevel::Operator,
                binary: "/bin/eneros-trading-agent".to_string(),
                args: vec![
                    "--agent-id".to_string(),
                    "trading-1".to_string(),
                    "--eventbus-addr".to_string(),
                    "127.0.0.1:9876".to_string(),
                    "--gateway-addr".to_string(),
                    "127.0.0.1:9877".to_string(),
                ],
                env: env.clone(),
                scheduling_policy: SchedulingPolicy::default_for_agent_type(&AgentType::Trading),
                resource_quota: QuotaConfig::limited(30, 512, 100),
                dependencies: vec!["power-app".to_string()],
            },
        ];

        Self { services, agents }
    }

    /// Apply environment variable overrides for selected service fields.
    ///
    /// Supported overrides:
    /// - `ENEROS_INIT_<SERVICE>_BINARY` — override the service binary path
    /// - `ENEROS_INIT_<SERVICE>_ARGS` — override args (whitespace-separated)
    /// - `ENEROS_INIT_<SERVICE>_RESTART_POLICY` — override restart policy
    ///
    /// Service names are uppercased and non-alphanumeric characters are
    /// replaced with `_` when forming the env var name.
    fn apply_env_overrides(&mut self) {
        for svc in &mut self.services {
            let prefix = env_prefix(&svc.name);

            if let Ok(binary) = std::env::var(format!("{}_BINARY", prefix)) {
                if !binary.is_empty() {
                    svc.binary = binary;
                }
            }

            if let Ok(args) = std::env::var(format!("{}_ARGS", prefix)) {
                if !args.is_empty() {
                    svc.args = args.split_whitespace().map(String::from).collect();
                }
            }

            if let Ok(policy) = std::env::var(format!("{}_RESTART_POLICY", prefix)) {
                if !policy.is_empty() {
                    svc.restart_policy = match policy.to_lowercase().as_str() {
                        "always" => crate::init::service::RestartPolicy::Always,
                        "on_failure" | "onfailure" => {
                            crate::init::service::RestartPolicy::OnFailure
                        }
                        "no" | "never" => crate::init::service::RestartPolicy::No,
                        _ => svc.restart_policy,
                    };
                }
            }
        }
    }

    /// Validate the loaded configuration.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut seen = std::collections::HashSet::new();
        for svc in &self.services {
            if svc.name.is_empty() {
                return Err(ConfigError::Invalid(
                    "service with empty name".to_string(),
                ));
            }
            if svc.binary.is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "service '{}' has empty binary",
                    svc.name
                )));
            }
            if !seen.insert(&svc.name) {
                return Err(ConfigError::Invalid(format!(
                    "duplicate service name: '{}'",
                    svc.name
                )));
            }
        }

        // Validate agent entries.
        let mut seen_agents = std::collections::HashSet::new();
        for agent in &self.agents {
            if agent.agent_id.is_empty() {
                return Err(ConfigError::Invalid(
                    "agent with empty agent_id".to_string(),
                ));
            }
            if agent.binary.is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "agent '{}' has empty binary",
                    agent.agent_id
                )));
            }
            if !seen_agents.insert(&agent.agent_id) {
                return Err(ConfigError::Invalid(format!(
                    "duplicate agent_id: '{}'",
                    agent.agent_id
                )));
            }
        }
        Ok(())
    }
}

/// Convert a service name into an environment-variable-safe prefix.
fn env_prefix(name: &str) -> String {
    let mut s = String::with_capacity(name.len() + "ENEROS_INIT_".len());
    s.push_str("ENEROS_INIT_");
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_uppercase());
        } else {
            s.push('_');
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::service::RestartPolicy;

    #[test]
    fn test_default_config_has_services() {
        let cfg = InitConfig::load_default();
        assert!(!cfg.services.is_empty());
        assert!(cfg.services.iter().any(|s| s.name == "network"));
        assert!(cfg.services.iter().any(|s| s.name == "power-app"));
    }

    #[test]
    fn test_default_config_validates() {
        let cfg = InitConfig::load_default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_parse_minimal_toml() {
        let toml = r#"
[[services]]
name = "test"
binary = "/bin/test"
restart_policy = "always"
dependencies = []
"#;
        let cfg: InitConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.services.len(), 1);
        assert_eq!(cfg.services[0].name, "test");
        assert_eq!(cfg.services[0].binary, "/bin/test");
        assert_eq!(cfg.services[0].restart_policy, RestartPolicy::Always);
    }

    #[test]
    fn test_parse_with_args_and_deps() {
        let toml = r#"
[[services]]
name = "power-app"
binary = "/bin/eneros-api"
args = ["run", "--config", "/etc/eneros/eneros.toml"]
restart_policy = "on_failure"
dependencies = ["network", "timesync"]
"#;
        let cfg: InitConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.services.len(), 1);
        let svc = &cfg.services[0];
        assert_eq!(svc.args, vec!["run", "--config", "/etc/eneros/eneros.toml"]);
        assert_eq!(svc.dependencies, vec!["network", "timesync"]);
        assert_eq!(svc.restart_policy, RestartPolicy::OnFailure);
    }

    #[test]
    fn test_validate_rejects_empty_name() {
        let cfg = InitConfig {
            services: vec![ServiceConfig {
                name: String::new(),
                binary: "/bin/x".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_rejects_empty_binary() {
        let cfg = InitConfig {
            services: vec![ServiceConfig {
                name: "x".to_string(),
                binary: String::new(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_rejects_duplicate_names() {
        let cfg = InitConfig {
            services: vec![
                ServiceConfig {
                    name: "dup".to_string(),
                    binary: "/bin/a".to_string(),
                    ..Default::default()
                },
                ServiceConfig {
                    name: "dup".to_string(),
                    binary: "/bin/b".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_load_from_missing_file_uses_default() {
        let cfg = InitConfig::load_from_file_or_default(
            "/nonexistent/path/that/does/not/exist/init.toml",
        );
        assert!(!cfg.services.is_empty());
    }

    #[test]
    fn test_env_prefix_normalization() {
        assert_eq!(env_prefix("network"), "ENEROS_INIT_NETWORK");
        assert_eq!(env_prefix("power-app"), "ENEROS_INIT_POWER_APP");
        assert_eq!(env_prefix("foo.bar"), "ENEROS_INIT_FOO_BAR");
    }

    #[test]
    fn test_apply_env_overrides_binary() {
        std::env::set_var("ENEROS_INIT_TESTSVC_BINARY", "/custom/bin");
        std::env::set_var("ENEROS_INIT_TESTSVC_ARGS", "a b c");
        std::env::set_var("ENEROS_INIT_TESTSVC_RESTART_POLICY", "no");

        let mut cfg = InitConfig {
            services: vec![ServiceConfig {
                name: "testsvc".to_string(),
                binary: "/original".to_string(),
                args: vec!["old".to_string()],
                restart_policy: RestartPolicy::Always,
                ..Default::default()
            }],
            ..Default::default()
        };
        cfg.apply_env_overrides();

        std::env::remove_var("ENEROS_INIT_TESTSVC_BINARY");
        std::env::remove_var("ENEROS_INIT_TESTSVC_ARGS");
        std::env::remove_var("ENEROS_INIT_TESTSVC_RESTART_POLICY");

        let svc = &cfg.services[0];
        assert_eq!(svc.binary, "/custom/bin");
        assert_eq!(svc.args, vec!["a", "b", "c"]);
        assert_eq!(svc.restart_policy, RestartPolicy::No);
    }

    #[test]
    fn test_apply_env_overrides_empty_values_ignored() {
        std::env::set_var("ENEROS_INIT_KEEPSVC_BINARY", "");

        let mut cfg = InitConfig {
            services: vec![ServiceConfig {
                name: "keepsvc".to_string(),
                binary: "/original".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        cfg.apply_env_overrides();

        std::env::remove_var("ENEROS_INIT_KEEPSVC_BINARY");

        assert_eq!(cfg.services[0].binary, "/original");
    }

    // ----- Agent config tests -----

    #[test]
    fn test_default_config_has_agents() {
        let cfg = InitConfig::load_default();
        assert!(!cfg.agents.is_empty());
        // 6 default agents
        assert_eq!(cfg.agents.len(), 6);
        assert!(cfg.agents.iter().any(|a| a.agent_id == "dispatch-1"));
        assert!(cfg.agents.iter().any(|a| a.agent_id == "self-healing-1"));
    }

    #[test]
    fn test_default_config_self_healing_is_rt() {
        let cfg = InitConfig::load_default();
        let sh = cfg
            .agents
            .iter()
            .find(|a| a.agent_id == "self-healing-1")
            .unwrap();
        assert!(sh.scheduling_policy.is_realtime());
    }

    #[test]
    fn test_parse_agent_toml() {
        let toml = r#"
[[agents]]
agent_id = "test-agent"
agent_type = "Dispatch"
authority = "Supervisor"
binary = "/bin/eneros-dispatch-agent"
args = ["--agent-id", "test-agent"]
dependencies = ["power-app"]

[agents.resource_quota]
cpu_percent = 50
memory_mb = 512
max_pids = 100
"#;
        let cfg: InitConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.agents.len(), 1);
        let agent = &cfg.agents[0];
        assert_eq!(agent.agent_id, "test-agent");
        assert_eq!(agent.agent_type, AgentType::Dispatch);
        assert_eq!(agent.authority, AuthorityLevel::Supervisor);
        assert_eq!(agent.binary, "/bin/eneros-dispatch-agent");
        assert_eq!(agent.args, vec!["--agent-id", "test-agent"]);
        assert_eq!(agent.dependencies, vec!["power-app"]);
        assert_eq!(agent.resource_quota.cpu_percent, 50);
        assert_eq!(agent.resource_quota.memory_mb, 512);
    }

    #[test]
    fn test_parse_agent_with_rt_scheduling() {
        let toml = r#"
[[agents]]
agent_id = "rt-agent"
agent_type = "SelfHealing"
authority = "Emergency"
binary = "/bin/eneros-self-healing-agent"

[agents.scheduling_policy]
Realtime = { priority = 80, cpus = [2, 3], lock_memory = true }
"#;
        let cfg: InitConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.agents.len(), 1);
        assert!(cfg.agents[0].scheduling_policy.is_realtime());
    }

    #[test]
    fn test_validate_rejects_empty_agent_id() {
        let cfg = InitConfig {
            agents: vec![AgentServiceConfig {
                agent_id: String::new(),
                binary: "/bin/x".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_rejects_empty_agent_binary() {
        let cfg = InitConfig {
            agents: vec![AgentServiceConfig {
                agent_id: "a1".to_string(),
                binary: String::new(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_rejects_duplicate_agent_ids() {
        let cfg = InitConfig {
            agents: vec![
                AgentServiceConfig {
                    agent_id: "dup".to_string(),
                    binary: "/bin/a".to_string(),
                    ..Default::default()
                },
                AgentServiceConfig {
                    agent_id: "dup".to_string(),
                    binary: "/bin/b".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_accepts_agents() {
        let cfg = InitConfig::load_default();
        assert!(cfg.validate().is_ok());
    }
}
