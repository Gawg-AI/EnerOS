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
    path: String,
    value: DataValue,
    quality: DataQuality,
    timestamp: i64,
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
                path: path.to_string(),
                value,
                quality: DataQuality::Good,
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
        );
    }

    pub async fn inject_report(&self, report: Iec61850Report) {
        self.reports.lock().await.push(report);
    }

    fn parse_iec61850_path(address: &str) -> Result<(String, String, String)> {
        let parts: Vec<&str> = address.split('/').collect();
        if parts.len() < 3 {
            return Err(eneros_core::EnerOSError::Device(format!(
                "Invalid IEC 61850 path '{}', expected 'LD/LN.DataObject.DataAttribute'",
                address
            )));
        }
        Ok((
            parts[0].to_string(),
            parts[1].to_string(),
            parts[2..].join("/"),
        ))
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
                path: address.to_string(),
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
}
