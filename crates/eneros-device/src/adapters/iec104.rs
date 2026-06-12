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
}
