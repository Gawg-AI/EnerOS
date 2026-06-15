use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use parking_lot::RwLock;

use eneros_core::Result;
use super::protocol::ProtocolType;

#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    pub host: String,
    pub port: u16,
    pub timeout_ms: u64,
    pub credentials: Option<Credentials>,
    pub protocol_config: ProtocolConfig,
}

#[derive(Debug, Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone)]
pub enum ProtocolConfig {
    Modbus {
        slave_id: u8,
        baud_rate: Option<u32>,
    },
    Iec61850 {
        logical_devices: Vec<String>,
    },
    Iec104 {
        common_address: u16,
        ioa_size: u8,
    },
    Mqtt {
        client_id: String,
        topics: Vec<String>,
    },
    OpcUa {
        namespace_url: String,
        security_policy: String,
    },
    Dnp3 {
        master_address: u16,
        slave_address: u16,
    },
    RawTcp,
}

#[derive(Debug, Clone)]
pub struct DataPoint {
    pub address: String,
    pub value: DataValue,
    pub timestamp: i64,
    pub quality: DataQuality,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DataValue {
    Bool(bool),
    Int16(i16),
    Int32(i32),
    Int64(i64),
    Float32(f32),
    Float64(f64),
    String(String),
    Bytes(Vec<u8>),
}

impl fmt::Display for DataValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataValue::Bool(v) => write!(f, "{}", v),
            DataValue::Int16(v) => write!(f, "{}", v),
            DataValue::Int32(v) => write!(f, "{}", v),
            DataValue::Int64(v) => write!(f, "{}", v),
            DataValue::Float32(v) => write!(f, "{}", v),
            DataValue::Float64(v) => write!(f, "{}", v),
            DataValue::String(v) => write!(f, "{}", v),
            DataValue::Bytes(v) => write!(f, "{:?}", v),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataQuality {
    Good,
    Uncertain,
    Bad,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Error(String),
}

impl fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConnectionState::Disconnected => write!(f, "disconnected"),
            ConnectionState::Connecting => write!(f, "connecting"),
            ConnectionState::Connected => write!(f, "connected"),
            ConnectionState::Reconnecting => write!(f, "reconnecting"),
            ConnectionState::Error(e) => write!(f, "error: {}", e),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub name: String,
    pub protocol: ProtocolType,
    pub manufacturer: String,
    pub model: String,
    pub firmware_version: String,
    pub ip_address: String,
    pub port: u16,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AdapterStatistics {
    pub messages_sent: u64,
    pub messages_received: u64,
    pub errors: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub reconnect_count: u32,
    pub last_message_time: Option<DateTime<Utc>>,
    pub uptime_seconds: u64,
}

#[derive(Debug, Clone)]
pub struct ReadRequest {
    pub address: String,
}

#[derive(Debug, Clone)]
pub struct ReadResponse {
    pub point: DataPoint,
}

#[derive(Debug, Clone)]
pub struct WriteRequest {
    pub address: String,
    pub value: DataValue,
}

#[derive(Debug, Clone)]
pub struct BatchReadRequest {
    pub requests: Vec<ReadRequest>,
}

#[derive(Debug, Clone)]
pub struct BatchReadResponse {
    pub responses: Vec<ReadResponse>,
    pub errors: Vec<BatchError>,
}

#[derive(Debug, Clone)]
pub struct BatchWriteRequest {
    pub requests: Vec<WriteRequest>,
}

#[derive(Debug, Clone)]
pub struct BatchWriteResponse {
    pub success_count: usize,
    pub errors: Vec<BatchError>,
}

#[derive(Debug, Clone)]
pub struct BatchError {
    pub address: String,
    pub error: String,
}

struct AdapterInner {
    state: ConnectionState,
    statistics: AdapterStatistics,
    connected_at: Option<DateTime<Utc>>,
}

pub struct SharedAdapterState {
    inner: RwLock<AdapterInner>,
    connection_id: AtomicU64,
}

impl SharedAdapterState {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(AdapterInner {
                state: ConnectionState::Disconnected,
                statistics: AdapterStatistics::default(),
                connected_at: None,
            }),
            connection_id: AtomicU64::new(0),
        }
    }

    pub fn set_state(&self, state: ConnectionState) {
        self.inner.write().state = state;
    }

    pub fn state(&self) -> ConnectionState {
        self.inner.read().state.clone()
    }

    pub fn mark_connected(&self) {
        let mut inner = self.inner.write();
        inner.state = ConnectionState::Connected;
        inner.connected_at = Some(Utc::now());
        self.connection_id.fetch_add(1, Ordering::Relaxed);
    }

    pub fn mark_disconnected(&self) {
        let mut inner = self.inner.write();
        inner.state = ConnectionState::Disconnected;
        inner.connected_at = None;
    }

    pub fn mark_error(&self, error: String) {
        let mut inner = self.inner.write();
        inner.state = ConnectionState::Error(error);
    }

    pub fn record_sent(&self, bytes: u64) {
        let mut inner = self.inner.write();
        inner.statistics.messages_sent += 1;
        inner.statistics.bytes_sent += bytes;
        inner.statistics.last_message_time = Some(Utc::now());
    }

    pub fn record_received(&self, bytes: u64) {
        let mut inner = self.inner.write();
        inner.statistics.messages_received += 1;
        inner.statistics.bytes_received += bytes;
        inner.statistics.last_message_time = Some(Utc::now());
    }

    pub fn record_error(&self) {
        self.inner.write().statistics.errors += 1;
    }

    pub fn record_reconnect(&self) {
        self.inner.write().statistics.reconnect_count += 1;
    }

    pub fn statistics(&self) -> AdapterStatistics {
        let inner = self.inner.read();
        let mut stats = inner.statistics.clone();
        if let Some(connected_at) = inner.connected_at {
            stats.uptime_seconds = (Utc::now() - connected_at).num_seconds() as u64;
        }
        stats
    }

    pub fn connection_id(&self) -> u64 {
        self.connection_id.load(Ordering::Relaxed)
    }
}

impl Default for SharedAdapterState {
    fn default() -> Self {
        Self::new()
    }
}

pub type SharedState = Arc<SharedAdapterState>;

pub fn new_shared_state() -> SharedState {
    Arc::new(SharedAdapterState::new())
}

#[async_trait]
pub trait ProtocolAdapter: Send + Sync {
    async fn connect(&mut self, config: &ConnectionConfig) -> Result<()>;
    async fn disconnect(&mut self) -> Result<()>;

    async fn read(&self, address: &str) -> Result<DataPoint>;
    async fn write(&mut self, address: &str, value: &DataValue) -> Result<()>;

    async fn read_batch(&self, addresses: &[&str]) -> Result<Vec<DataPoint>> {
        let mut results = Vec::with_capacity(addresses.len());
        for addr in addresses {
            match self.read(addr).await {
                Ok(point) => results.push(point),
                Err(e) => {
                    results.push(DataPoint {
                        address: addr.to_string(),
                        value: DataValue::Bool(false),
                        timestamp: chrono::Utc::now().timestamp_millis(),
                        quality: DataQuality::Bad,
                    });
                    tracing::warn!("Batch read failed for {}: {}", addr, e);
                }
            }
        }
        Ok(results)
    }

    async fn write_batch(&mut self, items: &[(&str, &DataValue)]) -> Result<BatchWriteResponse> {
        let mut success_count = 0;
        let mut errors = Vec::new();
        for (addr, val) in items {
            match self.write(addr, val).await {
                Ok(()) => success_count += 1,
                Err(e) => errors.push(BatchError {
                    address: addr.to_string(),
                    error: e.to_string(),
                }),
            }
        }
        Ok(BatchWriteResponse {
            success_count,
            errors,
        })
    }

    async fn subscribe(
        &mut self,
        addresses: Vec<String>,
        callback: Box<dyn Fn(DataPoint) + Send + Sync>,
    ) -> Result<()>;

    async fn unsubscribe(&mut self, addresses: &[String]) -> Result<()> {
        let _ = addresses;
        Ok(())
    }

    fn name(&self) -> &str;
    fn protocol_type(&self) -> ProtocolType;
    fn is_connected(&self) -> bool;
    fn shared_state(&self) -> SharedState;

    fn connection_state(&self) -> ConnectionState {
        self.shared_state().state()
    }

    fn statistics(&self) -> AdapterStatistics {
        self.shared_state().statistics()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_value_display() {
        assert_eq!(DataValue::Bool(true).to_string(), "true");
        assert_eq!(DataValue::Int16(42).to_string(), "42");
        assert_eq!(DataValue::Int32(100000).to_string(), "100000");
        assert_eq!(DataValue::Float64(2.5).to_string(), "2.5");
        assert_eq!(DataValue::String("hello".into()).to_string(), "hello");
    }

    #[test]
    fn test_connection_state_display() {
        assert_eq!(ConnectionState::Disconnected.to_string(), "disconnected");
        assert_eq!(ConnectionState::Connected.to_string(), "connected");
        assert_eq!(
            ConnectionState::Error("timeout".into()).to_string(),
            "error: timeout"
        );
    }

    #[test]
    fn test_shared_state_lifecycle() {
        let state = SharedAdapterState::new();
        assert_eq!(state.state(), ConnectionState::Disconnected);
        assert_eq!(state.connection_id(), 0);

        state.mark_connected();
        assert_eq!(state.state(), ConnectionState::Connected);
        assert_eq!(state.connection_id(), 1);

        state.record_sent(100);
        state.record_received(200);
        state.record_error();

        let stats = state.statistics();
        assert_eq!(stats.messages_sent, 1);
        assert_eq!(stats.messages_received, 1);
        assert_eq!(stats.errors, 1);
        assert_eq!(stats.bytes_sent, 100);
        assert_eq!(stats.bytes_received, 200);

        state.mark_disconnected();
        assert_eq!(state.state(), ConnectionState::Disconnected);

        state.mark_connected();
        assert_eq!(state.connection_id(), 2);
    }

    #[test]
    fn test_batch_write_response() {
        let resp = BatchWriteResponse {
            success_count: 3,
            errors: vec![
                BatchError {
                    address: "holding:40005".into(),
                    error: "timeout".into(),
                },
            ],
        };
        assert_eq!(resp.success_count, 3);
        assert_eq!(resp.errors.len(), 1);
    }

    #[test]
    fn test_adapter_statistics_default() {
        let stats = AdapterStatistics::default();
        assert_eq!(stats.messages_sent, 0);
        assert_eq!(stats.messages_received, 0);
        assert_eq!(stats.errors, 0);
        assert!(stats.last_message_time.is_none());
    }
}
