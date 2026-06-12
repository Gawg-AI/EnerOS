use std::collections::HashMap;
use tokio::sync::{RwLock, broadcast};

use eneros_core::{Result, EnerOSError};
use eneros_eventbus::event::{Event, EventType, EventPayload};

use super::adapter::{
    ProtocolAdapter, ConnectionConfig, DataPoint, DataValue, ConnectionState,
    DeviceInfo, AdapterStatistics, SharedState,
};
use super::protocol::ProtocolType;

struct ManagedDevice {
    adapter: Box<dyn ProtocolAdapter>,
    config: ConnectionConfig,
    info: DeviceInfo,
    shared_state: SharedState,
}

pub struct DeviceManager {
    devices: RwLock<HashMap<String, ManagedDevice>>,
    event_tx: broadcast::Sender<Event>,
}

impl DeviceManager {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(10_000);
        Self {
            devices: RwLock::new(HashMap::new()),
            event_tx,
        }
    }

    pub fn with_event_bus(event_tx: broadcast::Sender<Event>) -> Self {
        Self {
            devices: RwLock::new(HashMap::new()),
            event_tx,
        }
    }

    fn emit_event(&self, event_type: EventType, device_id: &str, payload: EventPayload) {
        let event = Event::new(event_type, device_id, payload);
        if self.event_tx.send(event).is_err() {
            tracing::warn!("No event bus receivers for device event");
        }
    }

    pub async fn register_device(
        &self,
        device_id: &str,
        adapter: Box<dyn ProtocolAdapter>,
        config: ConnectionConfig,
        info: DeviceInfo,
    ) {
        let shared_state = adapter.shared_state();
        let mut devices = self.devices.write().await;
        devices.insert(
            device_id.to_string(),
            ManagedDevice {
                adapter,
                config,
                info,
                shared_state,
            },
        );
        tracing::info!("Device '{}' registered", device_id);
    }

    pub async fn connect(&self, device_id: &str) -> Result<()> {
        let mut devices = self.devices.write().await;
        let device = devices
            .get_mut(device_id)
            .ok_or_else(|| EnerOSError::Device(format!("Device '{}' not found", device_id)))?;

        match device.adapter.connect(&device.config).await {
            Ok(()) => {
                self.emit_event(
                    EventType::DeviceConnected,
                    device_id,
                    EventPayload::DeviceEvent {
                        device_id: device_id.to_string(),
                        event_type: "connected".to_string(),
                    },
                );
                tracing::info!("Device '{}' connected", device_id);
                Ok(())
            }
            Err(e) => {
                self.emit_event(
                    EventType::SystemError,
                    device_id,
                    EventPayload::DeviceEvent {
                        device_id: device_id.to_string(),
                        event_type: format!("connection_failed: {}", e),
                    },
                );
                Err(e)
            }
        }
    }

    pub async fn disconnect(&self, device_id: &str) -> Result<()> {
        let mut devices = self.devices.write().await;
        let device = devices
            .get_mut(device_id)
            .ok_or_else(|| EnerOSError::Device(format!("Device '{}' not found", device_id)))?;

        device.adapter.disconnect().await?;
        self.emit_event(
            EventType::DeviceDisconnected,
            device_id,
            EventPayload::DeviceEvent {
                device_id: device_id.to_string(),
                event_type: "disconnected".to_string(),
            },
        );
        tracing::info!("Device '{}' disconnected", device_id);
        Ok(())
    }

    pub async fn connect_all(&self) -> Vec<(String, Result<()>)> {
        let device_ids: Vec<String> = {
            let devices = self.devices.read().await;
            devices.keys().cloned().collect()
        };

        let mut results = Vec::new();
        for id in &device_ids {
            let result = self.connect(id).await;
            results.push((id.clone(), result));
        }
        results
    }

    pub async fn disconnect_all(&self) -> Vec<(String, Result<()>)> {
        let device_ids: Vec<String> = {
            let devices = self.devices.read().await;
            devices.keys().cloned().collect()
        };

        let mut results = Vec::new();
        for id in &device_ids {
            let result = self.disconnect(id).await;
            results.push((id.clone(), result));
        }
        results
    }

    pub async fn read(&self, device_id: &str, address: &str) -> Result<DataPoint> {
        let devices = self.devices.read().await;
        let device = devices
            .get(device_id)
            .ok_or_else(|| EnerOSError::Device(format!("Device '{}' not found", device_id)))?;

        if !device.adapter.is_connected() {
            return Err(EnerOSError::Device(format!(
                "Device '{}' not connected",
                device_id
            )));
        }

        let result = device.adapter.read(address).await;

        if let Ok(ref point) = result {
            self.emit_event(
                EventType::DataReceived,
                device_id,
                EventPayload::Message(format!("{}: {}", address, point.value)),
            );
        }

        result
    }

    pub async fn write(
        &self,
        device_id: &str,
        address: &str,
        value: &DataValue,
    ) -> Result<()> {
        let mut devices = self.devices.write().await;
        let device = devices
            .get_mut(device_id)
            .ok_or_else(|| EnerOSError::Device(format!("Device '{}' not found", device_id)))?;

        if !device.adapter.is_connected() {
            return Err(EnerOSError::Device(format!(
                "Device '{}' not connected",
                device_id
            )));
        }

        device.adapter.write(address, value).await
    }

    pub async fn read_batch(
        &self,
        device_id: &str,
        addresses: &[&str],
    ) -> Result<Vec<DataPoint>> {
        let devices = self.devices.read().await;
        let device = devices
            .get(device_id)
            .ok_or_else(|| EnerOSError::Device(format!("Device '{}' not found", device_id)))?;

        if !device.adapter.is_connected() {
            return Err(EnerOSError::Device(format!(
                "Device '{}' not connected",
                device_id
            )));
        }

        device.adapter.read_batch(addresses).await
    }

    pub async fn write_batch(
        &self,
        device_id: &str,
        items: &[(&str, &DataValue)],
    ) -> Result<super::adapter::BatchWriteResponse> {
        let mut devices = self.devices.write().await;
        let device = devices
            .get_mut(device_id)
            .ok_or_else(|| EnerOSError::Device(format!("Device '{}' not found", device_id)))?;

        if !device.adapter.is_connected() {
            return Err(EnerOSError::Device(format!(
                "Device '{}' not connected",
                device_id
            )));
        }

        device.adapter.write_batch(items).await
    }

    pub async fn subscribe(
        &self,
        device_id: &str,
        addresses: Vec<String>,
        callback: Box<dyn Fn(DataPoint) + Send + Sync>,
    ) -> Result<()> {
        let mut devices = self.devices.write().await;
        let device = devices
            .get_mut(device_id)
            .ok_or_else(|| EnerOSError::Device(format!("Device '{}' not found", device_id)))?;

        if !device.adapter.is_connected() {
            return Err(EnerOSError::Device(format!(
                "Device '{}' not connected",
                device_id
            )));
        }

        device.adapter.subscribe(addresses, callback).await
    }

    pub async fn remove_device(&self, device_id: &str) -> Result<()> {
        let mut devices = self.devices.write().await;
        devices
            .remove(device_id)
            .ok_or_else(|| EnerOSError::Device(format!("Device '{}' not found", device_id)))?;
        tracing::info!("Device '{}' removed", device_id);
        Ok(())
    }

    pub async fn device_count(&self) -> usize {
        self.devices.read().await.len()
    }

    pub async fn connected_count(&self) -> usize {
        let devices = self.devices.read().await;
        devices
            .values()
            .filter(|d| d.adapter.is_connected())
            .count()
    }

    pub async fn is_connected(&self, device_id: &str) -> bool {
        let devices = self.devices.read().await;
        devices
            .get(device_id)
            .map(|d| d.adapter.is_connected())
            .unwrap_or(false)
    }

    pub async fn connection_state(&self, device_id: &str) -> Option<ConnectionState> {
        let devices = self.devices.read().await;
        devices.get(device_id).map(|d| d.shared_state.state())
    }

    pub async fn device_ids(&self) -> Vec<String> {
        let devices = self.devices.read().await;
        devices.keys().cloned().collect()
    }

    pub async fn device_info(&self, device_id: &str) -> Option<DeviceInfo> {
        let devices = self.devices.read().await;
        devices.get(device_id).map(|d| d.info.clone())
    }

    pub async fn statistics(&self, device_id: &str) -> Option<AdapterStatistics> {
        let devices = self.devices.read().await;
        devices.get(device_id).map(|d| d.adapter.statistics())
    }

    pub async fn all_statistics(&self) -> HashMap<String, AdapterStatistics> {
        let devices = self.devices.read().await;
        devices
            .iter()
            .map(|(id, d)| (id.clone(), d.adapter.statistics()))
            .collect()
    }

    pub async fn devices_by_protocol(&self, protocol: ProtocolType) -> Vec<String> {
        let devices = self.devices.read().await;
        devices
            .iter()
            .filter(|(_, d)| d.adapter.protocol_type() == protocol)
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub async fn devices_by_state(&self, state: &ConnectionState) -> Vec<String> {
        let devices = self.devices.read().await;
        devices
            .iter()
            .filter(|(_, d)| &d.adapter.connection_state() == state)
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<Event> {
        self.event_tx.subscribe()
    }
}

impl Default for DeviceManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock_adapter::mock::MockAdapter;
    use crate::adapter::{ProtocolConfig, DeviceInfo};

    fn test_config() -> ConnectionConfig {
        ConnectionConfig {
            host: "127.0.0.1".into(),
            port: 502,
            timeout_ms: 3000,
            credentials: None,
            protocol_config: ProtocolConfig::Modbus {
                slave_id: 1,
                baud_rate: None,
            },
        }
    }

    fn test_info(name: &str) -> DeviceInfo {
        DeviceInfo {
            device_id: name.to_string(),
            name: name.to_string(),
            protocol: ProtocolType::Modbus,
            manufacturer: "Test".into(),
            model: "Mock-100".into(),
            firmware_version: "1.0.0".into(),
            ip_address: "127.0.0.1".into(),
            port: 502,
            capabilities: vec!["read".into(), "write".into()],
        }
    }

    #[tokio::test]
    async fn test_register_and_connect() {
        let manager = DeviceManager::new();
        let adapter = Box::new(MockAdapter::new("mock1"));
        manager.register_device("dev1", adapter, test_config(), test_info("dev1")).await;

        assert_eq!(manager.device_count().await, 1);
        assert!(!manager.is_connected("dev1").await);

        manager.connect("dev1").await.unwrap();
        assert!(manager.is_connected("dev1").await);
        assert_eq!(manager.connected_count().await, 1);
    }

    #[tokio::test]
    async fn test_read_write() {
        let manager = DeviceManager::new();
        let adapter = Box::new(MockAdapter::new("mock1"));
        manager.register_device("dev1", adapter, test_config(), test_info("dev1")).await;
        manager.connect("dev1").await.unwrap();

        manager
            .write("dev1", "holding:40001", &DataValue::Int16(42))
            .await
            .unwrap();

        let point = manager.read("dev1", "holding:40001").await.unwrap();
        assert_eq!(point.value, DataValue::Int16(42));
    }

    #[tokio::test]
    async fn test_batch_operations() {
        let manager = DeviceManager::new();
        let adapter = Box::new(MockAdapter::new("mock1"));
        manager.register_device("dev1", adapter, test_config(), test_info("dev1")).await;
        manager.connect("dev1").await.unwrap();

        manager
            .write("dev1", "holding:40001", &DataValue::Int16(10))
            .await
            .unwrap();
        manager
            .write("dev1", "holding:40002", &DataValue::Int16(20))
            .await
            .unwrap();

        let points = manager
            .read_batch("dev1", &["holding:40001", "holding:40002"])
            .await
            .unwrap();
        assert_eq!(points.len(), 2);

        let write_result = manager
            .write_batch(
                "dev1",
                &[
                    ("holding:40003", &DataValue::Int16(30)),
                    ("holding:40004", &DataValue::Int16(40)),
                ],
            )
            .await
            .unwrap();
        assert_eq!(write_result.success_count, 2);
    }

    #[tokio::test]
    async fn test_device_not_found() {
        let manager = DeviceManager::new();
        let result = manager.connect("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_remove_device() {
        let manager = DeviceManager::new();
        let adapter = Box::new(MockAdapter::new("mock1"));
        manager.register_device("dev1", adapter, test_config(), test_info("dev1")).await;
        assert_eq!(manager.device_count().await, 1);

        manager.remove_device("dev1").await.unwrap();
        assert_eq!(manager.device_count().await, 0);
    }

    #[tokio::test]
    async fn test_connect_disconnect_all() {
        let manager = DeviceManager::new();
        let adapter1 = Box::new(MockAdapter::new("mock1"));
        let adapter2 = Box::new(MockAdapter::new("mock2"));
        manager.register_device("dev1", adapter1, test_config(), test_info("dev1")).await;
        manager.register_device("dev2", adapter2, test_config(), test_info("dev2")).await;

        let results = manager.connect_all().await;
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|(_, r)| r.is_ok()));
        assert_eq!(manager.connected_count().await, 2);

        let results = manager.disconnect_all().await;
        assert_eq!(results.len(), 2);
        assert_eq!(manager.connected_count().await, 0);
    }

    #[tokio::test]
    async fn test_device_filtering() {
        let manager = DeviceManager::new();
        let adapter1 = Box::new(MockAdapter::new("mock1"));
        let adapter2 = Box::new(MockAdapter::new("mock2"));
        manager.register_device("dev1", adapter1, test_config(), test_info("dev1")).await;
        manager.register_device("dev2", adapter2, test_config(), test_info("dev2")).await;

        let modbus_devices = manager.devices_by_protocol(ProtocolType::Modbus).await;
        assert_eq!(modbus_devices.len(), 2);

        let disconnected = manager.devices_by_state(&ConnectionState::Disconnected).await;
        assert_eq!(disconnected.len(), 2);

        manager.connect("dev1").await.unwrap();
        let connected = manager.devices_by_state(&ConnectionState::Connected).await;
        assert_eq!(connected.len(), 1);
    }

    #[tokio::test]
    async fn test_statistics() {
        let manager = DeviceManager::new();
        let adapter = Box::new(MockAdapter::new("mock1"));
        manager.register_device("dev1", adapter, test_config(), test_info("dev1")).await;
        manager.connect("dev1").await.unwrap();

        let _ = manager.read("dev1", "holding:40001").await;
        let _ = manager.read("dev1", "holding:40002").await;

        let stats = manager.statistics("dev1").await.unwrap();
        assert_eq!(stats.messages_received, 2);

        let all_stats = manager.all_statistics().await;
        assert_eq!(all_stats.len(), 1);
    }

    #[tokio::test]
    async fn test_event_emission() {
        let manager = DeviceManager::new();
        let mut rx = manager.subscribe_events();

        let adapter = Box::new(MockAdapter::new("mock1"));
        manager.register_device("dev1", adapter, test_config(), test_info("dev1")).await;
        manager.connect("dev1").await.unwrap();

        let event = rx.recv().await.unwrap();
        assert_eq!(event.event_type, EventType::DeviceConnected);
    }

    #[tokio::test]
    async fn test_device_info() {
        let manager = DeviceManager::new();
        let adapter = Box::new(MockAdapter::new("mock1"));
        manager.register_device("dev1", adapter, test_config(), test_info("dev1")).await;

        let info = manager.device_info("dev1").await.unwrap();
        assert_eq!(info.name, "dev1");
        assert_eq!(info.protocol, ProtocolType::Modbus);
        assert_eq!(info.firmware_version, "1.0.0");

        let ids = manager.device_ids().await;
        assert_eq!(ids, vec!["dev1"]);
    }
}
