pub mod adapter;
pub mod adapters;
pub mod discovery;
pub mod health;
pub mod manager;
pub mod protocol;
pub mod mock_adapter;

pub use adapter::{
    ProtocolAdapter, ConnectionConfig, Credentials, ProtocolConfig,
    DataPoint, DataValue, DataQuality, ConnectionState,
    DeviceInfo, AdapterStatistics,
    BatchReadRequest, BatchReadResponse, BatchWriteRequest, BatchWriteResponse, BatchError,
    SharedState, new_shared_state,
};
pub use adapters::{
    ModbusTcpAdapter, MqttAdapter, Iec61850Adapter, Iec104Adapter,
    Iec104Client, Iec104Config, Iec104ConnectionState,
    InformationObject, Iec104TypeId, CauseOfTransmission,
    Iec61850Config, MmsClient, BerEncoder, BerDecoder, CotpTransport,
    GooseAdapter, GooseConfig, GooseFrame, GooseData, GooseTransport, MockGooseTransport,
    SvAdapter, SvConfig, SvFrame,
    OpcUaAdapter, OpcUaConfig, OpcUaClient, OpcUaNodeId, OpcUaVariant, NodeIdType, NodeClass, BrowseResult,
    Dnp3Adapter, Dnp3Config, Dnp3Client, Dnp3Point, Dnp3Value, Dnp3Flags, Dnp3PointType, Dnp3LinkFrame, Dnp3AppRequest, Dnp3FunctionCode,
    // v0.7.0: IEC 104 enhancements
    Iec104TlsConfig, Iec104RedundancyMode,
    // v0.7.0: IEC 61850 enhancements
    RcbManager, ReportControlBlock, RcbType as Iec61850RcbType, TrgOp, Iec61850ReportData,
    SclDocument, Ied, LogicalDevice, LogicalNode, SclDataSet, parse_scl,
    ControlService, ControlObject, ControlState, ControlMode, ControlResult, Originator, ControllableCdc,
    DataSetManager, Iec61850DataSet, FcdaRef, FunctionalConstraint, DataSetValue,
};
pub use manager::DeviceManager;
pub use protocol::ProtocolType;
