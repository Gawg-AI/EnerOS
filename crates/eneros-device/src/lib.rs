pub mod adapter;
pub mod adapters;
pub mod discovery;
pub mod health;
pub mod manager;
pub mod protocol;
#[cfg(test)]
pub mod mock_adapter;

pub use adapter::{
    ProtocolAdapter, ConnectionConfig, Credentials, ProtocolConfig,
    DataPoint, DataValue, DataQuality, ConnectionState,
    DeviceInfo, AdapterStatistics,
    BatchReadRequest, BatchReadResponse, BatchWriteRequest, BatchWriteResponse, BatchError,
    SharedState, new_shared_state,
};
pub use adapters::{ModbusTcpAdapter, MqttAdapter, Iec61850Adapter, Iec104Adapter};
pub use manager::DeviceManager;
pub use protocol::ProtocolType;
