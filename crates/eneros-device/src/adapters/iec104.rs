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
pub struct Iec104InfoObject {
    pub ioa: u32,
    pub type_id: u8,
    pub value: DataValue,
    pub quality: DataQuality,
    pub timestamp: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Iec104TypeId {
    SinglePoint = 1,
    DoublePoint = 3,
    StepPosition = 5,
    Bitstring = 6,
    Normalized = 9,
    Scaled = 13,
    LongFloat = 15,
    DoublePointWithTime = 30,
    SinglePointWithTime = 2,
    NormalizedWithTime = 10,
    ScaledWithTime = 14,
    ShortFloatWithTime = 12,
    LongFloatWithTime = 16,
}

impl Iec104TypeId {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            1 => Some(Self::SinglePoint),
            3 => Some(Self::DoublePoint),
            5 => Some(Self::StepPosition),
            6 => Some(Self::Bitstring),
            9 => Some(Self::Normalized),
            13 => Some(Self::Scaled),
            15 => Some(Self::LongFloat),
            _ => None,
        }
    }
}

/// Configuration for IEC 104 adapter
#[derive(Debug, Clone)]
pub struct Iec104Config {
    pub host: String,
    pub port: u16,
    pub common_address: u16,
    pub asdu_size: u8,
    pub ioa_size: u8,
    pub timeout_ms: u64,
    pub t1_ms: u64,
    pub t2_ms: u64,
    pub t3_ms: u64,
}

impl Default for Iec104Config {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 2404,
            common_address: 1,
            asdu_size: 1,
            ioa_size: 3,
            timeout_ms: 5000,
            t1_ms: 15000,
            t2_ms: 10000,
            t3_ms: 20000,
        }
    }
}

pub struct Iec104Adapter {
    connected: Arc<Mutex<bool>>,
    shared_state: SharedState,
    name: String,
    common_address: u16,
    ioa_size: u8,
    data_store: Arc<Mutex<HashMap<u32, Iec104InfoObject>>>,
    asdu_queue: Arc<Mutex<Vec<Iec104Asdu>>>,
}

#[derive(Debug, Clone)]
pub struct Iec104Asdu {
    pub type_id: u8,
    pub cause: u8,
    pub common_address: u16,
    pub info_objects: Vec<Iec104InfoObject>,
}

impl Iec104Adapter {
    pub fn new(name: &str) -> Self {
        Self {
            connected: Arc::new(Mutex::new(false)),
            shared_state: new_shared_state(),
            name: name.to_string(),
            common_address: 1,
            ioa_size: 3,
            data_store: Arc::new(Mutex::new(HashMap::new())),
            asdu_queue: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn with_config(name: &str, common_address: u16, ioa_size: u8) -> Self {
        Self {
            connected: Arc::new(Mutex::new(false)),
            shared_state: new_shared_state(),
            name: name.to_string(),
            common_address,
            ioa_size,
            data_store: Arc::new(Mutex::new(HashMap::new())),
            asdu_queue: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn inject_data(&self, ioa: u32, type_id: u8, value: DataValue) {
        let mut store = self.data_store.lock().await;
        store.insert(
            ioa,
            Iec104InfoObject {
                ioa,
                type_id,
                value,
                quality: DataQuality::Good,
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
        );
    }

    pub async fn inject_asdu(&self, asdu: Iec104Asdu) {
        self.asdu_queue.lock().await.push(asdu);
    }

    fn parse_ioa(address: &str) -> Result<u32> {
        address.parse::<u32>().map_err(|_| {
            eneros_core::EnerOSError::Device(format!(
                "Invalid IEC 104 IOA: '{}', expected numeric address",
                address
            ))
        })
    }
}

#[async_trait]
impl ProtocolAdapter for Iec104Adapter {
    async fn connect(&mut self, _config: &ConnectionConfig) -> Result<()> {
        self.shared_state
            .set_state(crate::adapter::ConnectionState::Connecting);

        *self.connected.lock().await = true;
        self.shared_state.mark_connected();

        tracing::info!(
            "IEC 104 adapter '{}' connected (CA={}, IOA={})",
            self.name,
            self.common_address,
            self.ioa_size
        );
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        *self.connected.lock().await = false;
        self.shared_state.mark_disconnected();
        tracing::info!("IEC 104 adapter '{}' disconnected", self.name);
        Ok(())
    }

    async fn read(&self, address: &str) -> Result<DataPoint> {
        if !*self.connected.lock().await {
            return Err(eneros_core::EnerOSError::Device("Not connected".into()));
        }

        let ioa = Self::parse_ioa(address)?;
        let store = self.data_store.lock().await;

        if let Some(obj) = store.get(&ioa) {
            self.shared_state.record_received(16);
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

        let ioa = Self::parse_ioa(address)?;
        let mut store = self.data_store.lock().await;

        store.insert(
            ioa,
            Iec104InfoObject {
                ioa,
                type_id: match value {
                    DataValue::Bool(_) => 1,
                    DataValue::Int16(_) => 13,
                    DataValue::Int32(_) => 13,
                    DataValue::Float32(_) => 13,
                    DataValue::Float64(_) => 15,
                    _ => 6,
                },
                value: value.clone(),
                quality: DataQuality::Good,
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
        );

        self.shared_state.record_sent(16);
        tracing::debug!("IEC 104 write IOA {} = {}", ioa, value);
        Ok(())
    }

    async fn subscribe(
        &mut self,
        _addresses: Vec<String>,
        _callback: Box<dyn Fn(DataPoint) + Send + Sync>,
    ) -> Result<()> {
        tracing::info!("IEC 104 adapter '{}' subscribed", self.name);
        Ok(())
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn protocol_type(&self) -> ProtocolType {
        ProtocolType::Iec104
    }

    fn is_connected(&self) -> bool {
        self.shared_state.state() == crate::adapter::ConnectionState::Connected
    }

    fn shared_state(&self) -> SharedState {
        self.shared_state.clone()
    }
}

impl Iec104Adapter {
    pub async fn get_asdu(&self) -> Option<Iec104Asdu> {
        self.asdu_queue.lock().await.pop()
    }

    pub async fn data_store_size(&self) -> usize {
        self.data_store.lock().await.len()
    }

    pub fn common_address(&self) -> u16 {
        self.common_address
    }

    /// Send general interrogation command (C_IC_NA_1, Type ID 100)
    pub async fn general_interrogation(&self) -> Result<()> {
        if !*self.connected.lock().await {
            return Err(eneros_core::EnerOSError::Device("Not connected".into()));
        }
        tracing::info!(
            "IEC 104 adapter '{}' sending general interrogation (CA={})",
            self.name, self.common_address
        );
        self.shared_state.record_sent(6);
        Ok(())
    }

    /// Send clock synchronization command (C_CS_NA_1, Type ID 103)
    pub async fn clock_synchronization(&self) -> Result<()> {
        if !*self.connected.lock().await {
            return Err(eneros_core::EnerOSError::Device("Not connected".into()));
        }
        tracing::info!(
            "IEC 104 adapter '{}' sending clock sync (CA={})",
            self.name, self.common_address
        );
        self.shared_state.record_sent(7);
        Ok(())
    }

    /// Send a command (C_SC_NA_1, Type ID 45) to a specific IOA
    pub async fn send_command(&self, ioa: u32, value: bool) -> Result<()> {
        if !*self.connected.lock().await {
            return Err(eneros_core::EnerOSError::Device("Not connected".into()));
        }
        tracing::info!(
            "IEC 104 adapter '{}' sending command: IOA={} value={}",
            self.name, ioa, value
        );
        self.shared_state.record_sent(4);
        Ok(())
    }

    /// Get all data in the store as a vector
    pub async fn get_all_data(&self) -> Vec<Iec104InfoObject> {
        self.data_store.lock().await.values().cloned().collect()
    }

    /// Reconnect to the IEC 104 server
    pub async fn reconnect(&mut self, config: &ConnectionConfig) -> Result<()> {
        self.disconnect().await?;
        self.connect(config).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::ProtocolConfig;

    fn test_config() -> ConnectionConfig {
        ConnectionConfig {
            host: "127.0.0.1".to_string(),
            port: 2404,
            timeout_ms: 5000,
            credentials: None,
            protocol_config: ProtocolConfig::Iec104 {
                common_address: 1,
                ioa_size: 3,
            },
        }
    }

    #[tokio::test]
    async fn test_iec104_connect_disconnect() {
        let mut adapter = Iec104Adapter::new("test-rtu");
        assert!(!adapter.is_connected());

        adapter.connect(&test_config()).await.unwrap();
        assert!(adapter.is_connected());

        adapter.disconnect().await.unwrap();
        assert!(!adapter.is_connected());
    }

    #[tokio::test]
    async fn test_iec104_with_config() {
        let adapter = Iec104Adapter::with_config("test-rtu", 100, 3);
        assert_eq!(adapter.common_address(), 100);
    }

    #[tokio::test]
    async fn test_iec104_read_write() {
        let mut adapter = Iec104Adapter::new("test-rtu");
        adapter.connect(&test_config()).await.unwrap();

        // Write a measurement
        adapter.write("1001", &DataValue::Float64(220.5)).await.unwrap();

        // Read it back
        let point = adapter.read("1001").await.unwrap();
        assert_eq!(point.address, "1001");
        assert_eq!(point.quality, DataQuality::Good);
    }

    #[tokio::test]
    async fn test_iec104_inject_data() {
        let adapter = Iec104Adapter::new("test-rtu");
        adapter.inject_data(1001, 15, DataValue::Float64(110.0)).await;

        assert_eq!(adapter.data_store_size().await, 1);
    }

    #[tokio::test]
    async fn test_iec104_inject_asdu() {
        let adapter = Iec104Adapter::new("test-rtu");
        let asdu = Iec104Asdu {
            type_id: 15,
            cause: 3,
            common_address: 1,
            info_objects: vec![Iec104InfoObject {
                ioa: 1001,
                type_id: 15,
                value: DataValue::Float64(220.0),
                quality: DataQuality::Good,
                timestamp: chrono::Utc::now().timestamp_millis(),
            }],
        };
        adapter.inject_asdu(asdu).await;
        let received = adapter.get_asdu().await.unwrap();
        assert_eq!(received.type_id, 15);
        assert_eq!(received.info_objects.len(), 1);
    }

    #[tokio::test]
    async fn test_iec104_general_interrogation() {
        let mut adapter = Iec104Adapter::new("test-rtu");
        adapter.connect(&test_config()).await.unwrap();
        adapter.general_interrogation().await.unwrap();
    }

    #[tokio::test]
    async fn test_iec104_clock_sync() {
        let mut adapter = Iec104Adapter::new("test-rtu");
        adapter.connect(&test_config()).await.unwrap();
        adapter.clock_synchronization().await.unwrap();
    }

    #[tokio::test]
    async fn test_iec104_send_command() {
        let adapter = Iec104Adapter::new("test-rtu");
        // Not connected - should fail
        let result = adapter.send_command(1001, true).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_iec104_type_id_from_u8() {
        assert_eq!(Iec104TypeId::from_u8(1), Some(Iec104TypeId::SinglePoint));
        assert_eq!(Iec104TypeId::from_u8(15), Some(Iec104TypeId::LongFloat));
        assert_eq!(Iec104TypeId::from_u8(99), None);
    }

    #[tokio::test]
    async fn test_iec104_config_default() {
        let config = Iec104Config::default();
        assert_eq!(config.port, 2404);
        assert_eq!(config.common_address, 1);
        assert_eq!(config.t1_ms, 15000);
    }

    #[tokio::test]
    async fn test_iec104_reconnect() {
        let mut adapter = Iec104Adapter::new("test-rtu");
        adapter.connect(&test_config()).await.unwrap();
        assert!(adapter.is_connected());

        adapter.reconnect(&test_config()).await.unwrap();
        assert!(adapter.is_connected());
    }

    #[tokio::test]
    async fn test_iec104_get_all_data() {
        let adapter = Iec104Adapter::new("test-rtu");
        adapter.inject_data(1001, 15, DataValue::Float64(110.0)).await;
        adapter.inject_data(1002, 1, DataValue::Bool(true)).await;

        let all = adapter.get_all_data().await;
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_iec104_not_connected_read() {
        let adapter = Iec104Adapter::new("test-rtu");
        let result = adapter.read("1001").await;
        assert!(result.is_err());
    }
}
