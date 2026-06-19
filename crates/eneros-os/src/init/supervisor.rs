use crate::init::service::{Service, ServiceConfig, ServiceStatus, RestartPolicy};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Process supervisor that monitors and restarts services
#[derive(Debug)]
pub struct Supervisor {
    services: HashMap<String, Service>,
    crash_history: HashMap<String, Vec<Instant>>,
    max_restarts_per_minute: usize,
}

impl Supervisor {
    pub fn new() -> Self {
        Self {
            services: HashMap::new(),
            crash_history: HashMap::new(),
            max_restarts_per_minute: 5,
        }
    }

    pub fn register(&mut self, config: ServiceConfig) {
        let name = config.name.clone();
        self.services.insert(name, Service::new(config));
    }

    pub fn get_service(&self, name: &str) -> Option<&Service> {
        self.services.get(name)
    }

    pub fn get_service_mut(&mut self, name: &str) -> Option<&mut Service> {
        self.services.get_mut(name)
    }

    /// Check if a service should be restarted based on its policy and crash history
    pub fn should_restart(&mut self, name: &str) -> bool {
        let service = match self.services.get(name) {
            Some(s) => s,
            None => return false,
        };

        match service.config.restart_policy {
            RestartPolicy::No => false,
            RestartPolicy::Always => true,
            RestartPolicy::OnFailure => {
                // Check crash frequency
                let now = Instant::now();
                let history = self.crash_history.entry(name.to_string()).or_default();

                // Remove crashes older than 1 minute
                history.retain(|&t| now.duration_since(t) < Duration::from_secs(60));

                if history.len() >= self.max_restarts_per_minute {
                    // Enter degraded mode
                    if let Some(svc) = self.services.get_mut(name) {
                        svc.status = ServiceStatus::Degraded;
                    }
                    false
                } else {
                    history.push(now);
                    true
                }
            }
        }
    }

    pub fn services(&self) -> impl Iterator<Item = &Service> {
        self.services.values()
    }
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_service() {
        let mut sup = Supervisor::new();
        sup.register(ServiceConfig {
            name: "test".to_string(),
            binary: "/bin/test".to_string(),
            ..Default::default()
        });
        assert!(sup.get_service("test").is_some());
    }

    #[test]
    fn test_restart_policy_no() {
        let mut sup = Supervisor::new();
        sup.register(ServiceConfig {
            name: "test".to_string(),
            restart_policy: RestartPolicy::No,
            ..Default::default()
        });
        assert!(!sup.should_restart("test"));
    }

    #[test]
    fn test_restart_policy_always() {
        let mut sup = Supervisor::new();
        sup.register(ServiceConfig {
            name: "test".to_string(),
            restart_policy: RestartPolicy::Always,
            ..Default::default()
        });
        assert!(sup.should_restart("test"));
    }
}
