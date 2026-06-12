use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use parking_lot::RwLock;

use crate::adapter::{ConnectionState, SharedState};
use crate::manager::DeviceManager;

#[derive(Debug, Clone)]
pub struct HealthConfig {
    pub check_interval_ms: u64,
    pub reconnect_interval_ms: u64,
    pub max_reconnect_attempts: u32,
    pub heartbeat_timeout_ms: u64,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            check_interval_ms: 10_000,
            reconnect_interval_ms: 5_000,
            max_reconnect_attempts: 10,
            heartbeat_timeout_ms: 30_000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DeviceHealth {
    pub device_id: String,
    pub state: ConnectionState,
    pub reconnect_attempts: u32,
    pub last_check: i64,
    pub last_connected: Option<i64>,
    pub error_message: Option<String>,
}

struct MonitoredDevice {
    shared_state: SharedState,
    health: RwLock<DeviceHealth>,
}

pub struct HealthMonitor {
    config: HealthConfig,
    monitored: Arc<RwLock<HashMap<String, Arc<MonitoredDevice>>>>,
    running: Arc<RwLock<bool>>,
}

impl HealthMonitor {
    pub fn new(config: HealthConfig) -> Self {
        Self {
            config,
            monitored: Arc::new(RwLock::new(HashMap::new())),
            running: Arc::new(RwLock::new(false)),
        }
    }

    pub fn register_device(&self, device_id: &str, shared_state: SharedState) {
        let health = DeviceHealth {
            device_id: device_id.to_string(),
            state: shared_state.state(),
            reconnect_attempts: 0,
            last_check: chrono::Utc::now().timestamp_millis(),
            last_connected: None,
            error_message: None,
        };

        let monitored = MonitoredDevice {
            shared_state,
            health: RwLock::new(health),
        };

        self.monitored
            .write()
            .insert(device_id.to_string(), Arc::new(monitored));
        tracing::info!("Health monitor: registered device '{}'", device_id);
    }

    pub fn unregister_device(&self, device_id: &str) {
        self.monitored.write().remove(device_id);
        tracing::info!("Health monitor: unregistered device '{}'", device_id);
    }

    pub async fn start(&self, manager: Arc<DeviceManager>) {
        {
            *self.running.write() = true;
        }

        let config = self.config.clone();
        let monitored = self.monitored.clone();
        let running = self.running.clone();

        tokio::spawn(async move {
            let mut check_interval =
                tokio::time::interval(Duration::from_millis(config.check_interval_ms));
            let mut reconnect_interval =
                tokio::time::interval(Duration::from_millis(config.reconnect_interval_ms));

            loop {
                if !*running.read() {
                    break;
                }

                tokio::select! {
                    _ = check_interval.tick() => {
                        Self::check_health(&monitored).await;
                    }
                    _ = reconnect_interval.tick() => {
                        Self::attempt_reconnects(&manager, &monitored, &config).await;
                    }
                }
            }

            tracing::info!("Health monitor stopped");
        });

        tracing::info!(
            "Health monitor started (check: {}ms, reconnect: {}ms)",
            self.config.check_interval_ms,
            self.config.reconnect_interval_ms
        );
    }

    pub async fn stop(&self) {
        *self.running.write() = false;
    }

    async fn check_health(monitored: &Arc<RwLock<HashMap<String, Arc<MonitoredDevice>>>>) {
        let devices: Vec<Arc<MonitoredDevice>> = monitored.read().values().cloned().collect();
        let now = chrono::Utc::now().timestamp_millis();

        for device in devices {
            let mut health = device.health.write();
            let state = device.shared_state.state();

            health.state = state.clone();
            health.last_check = now;

            match &state {
                ConnectionState::Connected => {
                    health.last_connected = Some(now);
                    health.reconnect_attempts = 0;
                    health.error_message = None;
                }
                ConnectionState::Error(e) => {
                    health.error_message = Some(e.clone());
                }
                _ => {}
            }
        }
    }

    async fn attempt_reconnects(
        manager: &DeviceManager,
        monitored: &Arc<RwLock<HashMap<String, Arc<MonitoredDevice>>>>,
        config: &HealthConfig,
    ) {
        let devices: Vec<(String, Arc<MonitoredDevice>)> = {
            let map = monitored.read();
            map.iter()
                .filter(|(_, d)| {
                    let health = d.health.read();
                    matches!(health.state, ConnectionState::Disconnected | ConnectionState::Error(_))
                        && health.reconnect_attempts < config.max_reconnect_attempts
                })
                .map(|(id, d)| (id.clone(), d.clone()))
                .collect()
        };

        for (device_id, device) in devices {
            let attempt = {
                let mut health = device.health.write();
                health.reconnect_attempts += 1;
                health.reconnect_attempts
            };
            tracing::info!(
                "Health monitor: reconnecting '{}' (attempt {}/{})",
                device_id,
                attempt,
                config.max_reconnect_attempts
            );

            match manager.connect(&device_id).await {
                Ok(()) => {
                    let mut health = device.health.write();
                    health.reconnect_attempts = 0;
                    health.last_connected = Some(chrono::Utc::now().timestamp_millis());
                    health.error_message = None;
                    tracing::info!("Health monitor: '{}' reconnected", device_id);
                }
                Err(e) => {
                    let mut health = device.health.write();
                    health.error_message = Some(e.to_string());
                    tracing::warn!(
                        "Health monitor: '{}' reconnect failed: {}",
                        device_id,
                        e
                    );
                }
            }
        }
    }

    pub fn device_health(&self, device_id: &str) -> Option<DeviceHealth> {
        self.monitored
            .read()
            .get(device_id)
            .map(|d| d.health.read().clone())
    }

    pub fn all_health(&self) -> HashMap<String, DeviceHealth> {
        self.monitored
            .read()
            .iter()
            .map(|(id, d)| (id.clone(), d.health.read().clone()))
            .collect()
    }

    pub fn devices_needing_attention(&self) -> Vec<DeviceHealth> {
        self.monitored
            .read()
            .values()
            .filter_map(|d| {
                let health = d.health.read();
                if matches!(
                    health.state,
                    ConnectionState::Error(_) | ConnectionState::Disconnected
                ) {
                    Some(health.clone())
                } else {
                    None
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_config_default() {
        let config = HealthConfig::default();
        assert_eq!(config.check_interval_ms, 10_000);
        assert_eq!(config.reconnect_interval_ms, 5_000);
        assert_eq!(config.max_reconnect_attempts, 10);
    }

    #[test]
    fn test_health_monitor_new() {
        let monitor = HealthMonitor::new(HealthConfig::default());
        assert_eq!(monitor.monitored.read().len(), 0);
    }

    #[test]
    fn test_device_health_fields() {
        let health = DeviceHealth {
            device_id: "test".to_string(),
            state: ConnectionState::Connected,
            reconnect_attempts: 0,
            last_check: 0,
            last_connected: None,
            error_message: None,
        };
        assert_eq!(health.device_id, "test");
        assert_eq!(health.state, ConnectionState::Connected);
    }
}
