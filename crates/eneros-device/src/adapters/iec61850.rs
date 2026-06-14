use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::collections::HashMap;

use eneros_core::Result;
use crate::adapter::{
    ProtocolAdapter, ConnectionConfig, DataPoint, DataValue, DataQuality,
    SharedState, new_shared_state,
};
use crate::protocol::ProtocolType;

#[derive(Debug, Clone)]
struct Iec61850DataObject {
    value: DataValue,
    quality: DataQuality,
    timestamp: i64,
}

/// Configuration for IEC 61850 adapter
#[derive(Debug, Clone)]
pub struct Iec61850Config {
    pub host: String,
    pub port: u16,
    pub ied_name: String,
    pub logical_devices: Vec<String>,
    pub dataset_ref: String,
    pub report_interval_ms: u32,
}

impl Default for Iec61850Config {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 102,
            ied_name: "IED1".to_string(),
            logical_devices: vec!["LD0".to_string()],
            dataset_ref: "LD0/LLN0.dsGeneric".to_string(),
            report_interval_ms: 1000,
        }
    }
}

pub struct Iec61850Adapter {
    connected: Arc<Mutex<bool>>,
    shared_state: SharedState,
    name: String,
    data_model: Arc<Mutex<HashMap<String, Iec61850DataObject>>>,
    reports: Arc<Mutex<Vec<Iec61850Report>>>,
}

#[derive(Debug, Clone)]
pub struct Iec61850Report {
    pub report_id: String,
    pub data_set: String,
    pub values: HashMap<String, DataValue>,
    pub timestamp: i64,
}

#[derive(Debug, Clone)]
pub struct GooseMessage {
    pub dataset: String,
    pub values: HashMap<String, DataValue>,
    pub timestamp: i64,
    pub go_id: String,
    pub st_num: u32,
    pub sq_num: u32,
}

impl Iec61850Adapter {
    pub fn new(name: &str) -> Self {
        Self {
            connected: Arc::new(Mutex::new(false)),
            shared_state: new_shared_state(),
            name: name.to_string(),
            data_model: Arc::new(Mutex::new(HashMap::new())),
            reports: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn inject_data(&self, path: &str, value: DataValue) {
        let mut model = self.data_model.lock().await;
        model.insert(
            path.to_string(),
            Iec61850DataObject {
                value,
                quality: DataQuality::Good,
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
        );
    }

    pub async fn inject_report(&self, report: Iec61850Report) {
        self.reports.lock().await.push(report);
    }

}

#[async_trait]
impl ProtocolAdapter for Iec61850Adapter {
    async fn connect(&mut self, _config: &ConnectionConfig) -> Result<()> {
        self.shared_state
            .set_state(crate::adapter::ConnectionState::Connecting);

        *self.connected.lock().await = true;
        self.shared_state.mark_connected();

        tracing::info!("IEC 61850 adapter '{}' connected", self.name);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        *self.connected.lock().await = false;
        self.shared_state.mark_disconnected();
        tracing::info!("IEC 61850 adapter '{}' disconnected", self.name);
        Ok(())
    }

    async fn read(&self, address: &str) -> Result<DataPoint> {
        if !*self.connected.lock().await {
            return Err(eneros_core::EnerOSError::Device("Not connected".into()));
        }

        let model = self.data_model.lock().await;
        if let Some(obj) = model.get(address) {
            self.shared_state.record_received(64);
            Ok(DataPoint {
                address: address.to_string(),
                value: obj.value.clone(),
                timestamp: obj.timestamp,
                quality: obj.quality.clone(),
            })
        } else {
            Ok(DataPoint {
                address: address.to_string(),
                value: DataValue::Bool(false),
                timestamp: chrono::Utc::now().timestamp_millis(),
                quality: DataQuality::Bad,
            })
        }
    }

    async fn write(&mut self, address: &str, value: &DataValue) -> Result<()> {
        if !*self.connected.lock().await {
            return Err(eneros_core::EnerOSError::Device("Not connected".into()));
        }

        let mut model = self.data_model.lock().await;
        model.insert(
            address.to_string(),
            Iec61850DataObject {
                value: value.clone(),
                quality: DataQuality::Good,
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
        );

        self.shared_state.record_sent(64);
        tracing::debug!("IEC 61850 write {} = {}", address, value);
        Ok(())
    }

    async fn subscribe(
        &mut self,
        _addresses: Vec<String>,
        _callback: Box<dyn Fn(DataPoint) + Send + Sync>,
    ) -> Result<()> {
        tracing::info!("IEC 61850 adapter '{}' subscribed", self.name);
        Ok(())
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn protocol_type(&self) -> ProtocolType {
        ProtocolType::Iec61850
    }

    fn is_connected(&self) -> bool {
        self.shared_state.state() == crate::adapter::ConnectionState::Connected
    }

    fn shared_state(&self) -> SharedState {
        self.shared_state.clone()
    }
}

impl Iec61850Adapter {
    pub async fn get_report(&self) -> Option<Iec61850Report> {
        self.reports.lock().await.pop()
    }

    pub async fn data_model_size(&self) -> usize {
        self.data_model.lock().await.len()
    }

    /// Reconnect to the IEC 61850 server
    pub async fn reconnect(&mut self, config: &ConnectionConfig) -> Result<()> {
        self.disconnect().await?;
        self.connect(config).await
    }

    /// Subscribe to a specific dataset report
    pub async fn subscribe_dataset(&self, dataset_ref: &str) -> Result<()> {
        if !*self.connected.lock().await {
            return Err(eneros_core::EnerOSError::Device("Not connected".into()));
        }
        tracing::info!("IEC 61850 adapter '{}' subscribed to dataset: {}", self.name, dataset_ref);
        Ok(())
    }

    /// Read a specific node from the data model by MMS path
    pub async fn read_node(&self, ld: &str, ln: &str, do_da: &str) -> Result<DataPoint> {
        let path = format!("{}/{}/{}", ld, ln, do_da);
        self.read(&path).await
    }

    /// Write a value to a specific node in the data model
    pub async fn write_node(&mut self, ld: &str, ln: &str, do_da: &str, value: &DataValue) -> Result<()> {
        let path = format!("{}/{}/{}", ld, ln, do_da);
        self.write(&path, value).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::ProtocolConfig;

    fn test_config() -> ConnectionConfig {
        ConnectionConfig {
            host: "127.0.0.1".to_string(),
            port: 102,
            timeout_ms: 5000,
            credentials: None,
            protocol_config: ProtocolConfig::Iec61850 {
                logical_devices: vec!["LD0".to_string()],
            },
        }
    }

    #[tokio::test]
    async fn test_iec61850_connect_disconnect() {
        let mut adapter = Iec61850Adapter::new("test-ied");
        assert!(!adapter.is_connected());

        adapter.connect(&test_config()).await.unwrap();
        assert!(adapter.is_connected());

        adapter.disconnect().await.unwrap();
        assert!(!adapter.is_connected());
    }

    #[tokio::test]
    async fn test_iec61850_read_write() {
        let mut adapter = Iec61850Adapter::new("test-ied");
        adapter.connect(&test_config()).await.unwrap();

        // Write
        adapter.write("LD0/GGIO1/AnIn1.mag", &DataValue::Float64(42.5)).await.unwrap();

        // Read
        let point = adapter.read("LD0/GGIO1/AnIn1.mag").await.unwrap();
        assert_eq!(point.address, "LD0/GGIO1/AnIn1.mag");
        assert_eq!(point.quality, DataQuality::Good);
    }

    #[tokio::test]
    async fn test_iec61850_read_nonexistent() {
        let mut adapter = Iec61850Adapter::new("test-ied");
        adapter.connect(&test_config()).await.unwrap();

        let point = adapter.read("LD0/GGIO1/NonExistent").await.unwrap();
        assert_eq!(point.quality, DataQuality::Bad);
    }

    #[tokio::test]
    async fn test_iec61850_read_node() {
        let mut adapter = Iec61850Adapter::new("test-ied");
        adapter.connect(&test_config()).await.unwrap();

        adapter.write_node("LD0", "GGIO1", "AnIn1.mag", &DataValue::Float64(110.0)).await.unwrap();
        let point = adapter.read_node("LD0", "GGIO1", "AnIn1.mag").await.unwrap();
        assert_eq!(point.quality, DataQuality::Good);
    }

    #[tokio::test]
    async fn test_iec61850_report() {
        let adapter = Iec61850Adapter::new("test-ied");
        let report = Iec61850Report {
            report_id: "rpt1".to_string(),
            data_set: "LD0/LLN0.dsGeneric".to_string(),
            values: vec![("AnIn1".to_string(), DataValue::Float64(220.0))].into_iter().collect(),
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        adapter.inject_report(report.clone()).await;
        let received = adapter.get_report().await.unwrap();
        assert_eq!(received.report_id, "rpt1");
    }

    #[tokio::test]
    async fn test_iec61850_config_default() {
        let config = Iec61850Config::default();
        assert_eq!(config.port, 102);
        assert_eq!(config.ied_name, "IED1");
        assert_eq!(config.report_interval_ms, 1000);
    }

    #[tokio::test]
    async fn test_iec61850_reconnect() {
        let mut adapter = Iec61850Adapter::new("test-ied");
        adapter.connect(&test_config()).await.unwrap();
        assert!(adapter.is_connected());

        adapter.reconnect(&test_config()).await.unwrap();
        assert!(adapter.is_connected());
    }

    #[tokio::test]
    async fn test_iec61850_not_connected_read() {
        let adapter = Iec61850Adapter::new("test-ied");
        let result = adapter.read("LD0/GGIO1/AnIn1").await;
        assert!(result.is_err());
    }
}
