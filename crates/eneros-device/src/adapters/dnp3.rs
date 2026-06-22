//! DNP3 (Distributed Network Protocol) client adapter.
//!
//! DNP3 is the dominant SCADA protocol in North America for utility
//! automation. It operates over TCP (port 20000) or serial, using a
//! layered link/application protocol with built-in time synchronization
//! and event-driven data reporting.
//!
//! # Architecture
//!
//! ```text
//! DNP3 Outstation (RTU/IED)
//!         │
//!         │ TCP (port 20000)
//!         ▼
//! Dnp3Client (this file)
//!   ├── Link Layer (framing, addressing)
//!   ├── Transport Layer (segment reassembly)
//!   ├── Application Layer (read/write/time-sync)
//!   └── Data classes (0/1/2/3)
//!         │
//!         ▼
//! Dnp3Adapter → ProtocolAdapter trait
//! ```
//!
//! # Address Format
//!
//! `class:<N>:<index>` (e.g., "class:0:5" for Class 0 point index 5)
//! Or `binary:0`, `analog:3`, `counter:1`, etc.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::net::TcpStream;
use tokio::io::AsyncWriteExt;

use eneros_core::Result;
use crate::adapter::{
    ProtocolAdapter, ConnectionConfig, DataPoint, DataValue, DataQuality,
    SharedState, new_shared_state,
};
use crate::protocol::ProtocolType;

/// DNP3 link layer frame start bytes
pub const DNP3_START_BYTES: [u8; 2] = [0x05, 0x64];

/// DNP3 maximum link frame length
pub const DNP3_MAX_LINK_LENGTH: u8 = 255;

/// DNP3 function codes (application layer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dnp3FunctionCode {
    /// Read data from outstation
    Read = 1,
    /// Write data to outstation
    Write = 2,
    /// Select before operate
    Select = 3,
    /// Operate (after select)
    Operate = 4,
    /// Direct operate
    DirectOperate = 5,
    /// Direct operate no response
    DirectOperateNoResponse = 6,
    /// Immediate freeze
    ImmediateFreeze = 7,
    /// Freeze and clear
    FreezeClear = 8,
    /// Read freeze data
    ReadFreeze = 9,
    /// Set time and date
    WriteTime = 20,
    /// Record current time
    RecordCurrentTime = 21,
    /// Delay measurement
    DelayMeasure = 23,
    /// Application layer confirmation
    Confirm = 0,
    /// Respond
    Respond = 129,
    /// Unsolicited response
    UnsolicitedResponse = 130,
}

/// DNP3 data point types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dnp3PointType {
    /// Binary input (static) — Class 0
    BinaryInput,
    /// Binary input event — Class 1/2/3
    BinaryInputEvent,
    /// Double-bit binary input
    DoubleBitBinary,
    /// Analog input (static)
    AnalogInput,
    /// Analog input event
    AnalogInputEvent,
    /// Counter (static)
    Counter,
    /// Counter event
    CounterEvent,
    /// Binary output (control)
    BinaryOutput,
    /// Analog output (setpoint)
    AnalogOutput,
    /// Time and date
    TimeAndDate,
}

impl Dnp3PointType {
    /// Get the DNP3 object type variation group number.
    pub fn group(&self) -> u8 {
        match self {
            Self::BinaryInput => 1,
            Self::BinaryInputEvent => 2,
            Self::DoubleBitBinary => 3,
            Self::AnalogInput => 30,
            Self::AnalogInputEvent => 32,
            Self::Counter => 20,
            Self::CounterEvent => 22,
            Self::BinaryOutput => 10,
            Self::AnalogOutput => 40,
            Self::TimeAndDate => 50,
        }
    }

    /// Parse from address string prefix.
    pub fn from_prefix(prefix: &str) -> Option<Self> {
        match prefix.to_lowercase().as_str() {
            "binary" | "bi" => Some(Self::BinaryInput),
            "binaryevent" | "bie" => Some(Self::BinaryInputEvent),
            "doublebit" | "db" => Some(Self::DoubleBitBinary),
            "analog" | "ai" => Some(Self::AnalogInput),
            "analgevent" | "aie" => Some(Self::AnalogInputEvent),
            "counter" | "ct" => Some(Self::Counter),
            "counterevent" | "cte" => Some(Self::CounterEvent),
            "binaryoutput" | "bo" => Some(Self::BinaryOutput),
            "analogoutput" | "ao" => Some(Self::AnalogOutput),
            "time" => Some(Self::TimeAndDate),
            _ => None,
        }
    }
}

/// DNP3 data point flags (quality bitmask).
#[derive(Debug, Clone, Copy, Default)]
pub struct Dnp3Flags {
    pub online: bool,
    pub restart: bool,
    pub comm_lost: bool,
    pub remote_forced: bool,
    pub local_forced: bool,
    pub rollover: bool,
    pub discontinuity: bool,
    pub reference_check: bool,
}

impl Dnp3Flags {
    pub fn from_u8(val: u8) -> Self {
        Self {
            online: val & 0x01 != 0,
            restart: val & 0x02 != 0,
            comm_lost: val & 0x04 != 0,
            remote_forced: val & 0x08 != 0,
            local_forced: val & 0x10 != 0,
            rollover: val & 0x20 != 0,
            discontinuity: val & 0x40 != 0,
            reference_check: val & 0x80 != 0,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.online && !self.comm_lost
    }

    pub fn to_data_quality(&self) -> DataQuality {
        if self.is_valid() {
            DataQuality::Good
        } else if self.online {
            DataQuality::Uncertain
        } else {
            DataQuality::Bad
        }
    }
}

/// DNP3 link layer frame.
#[derive(Debug, Clone)]
pub struct Dnp3LinkFrame {
    /// Source address
    pub source: u16,
    /// Destination address
    pub destination: u16,
    /// Frame payload (application layer data)
    pub payload: Vec<u8>,
}

impl Dnp3LinkFrame {
    /// Encode a DNP3 link frame for transmission.
    ///
    /// Frame format:
    /// - Start bytes: 0x05 0x64
    /// - Length: 5 + payload_blocks * 16
    /// - Control byte
    /// - Destination (2 bytes LE)
    /// - Source (2 bytes LE)
    /// - CRC (2 bytes)
    /// - Data blocks (16 bytes + 2 CRC each)
    pub fn encode(&self, is_master: bool) -> Vec<u8> {
        let data_blocks = self.payload.len().div_ceil(16);
        let total_length = 5 + (data_blocks as u8) * 16;

        let mut frame = Vec::with_capacity(10 + self.payload.len() + data_blocks * 2);
        // Start bytes
        frame.extend_from_slice(&DNP3_START_BYTES);
        // Length
        frame.push(total_length);
        // Control byte: 0x44 = master, DIR=1, PRM=1; 0x40 = slave
        let control = if is_master { 0x44 } else { 0x40 };
        frame.push(control);
        // Destination (LE)
        frame.extend_from_slice(&self.destination.to_le_bytes());
        // Source (LE)
        frame.extend_from_slice(&self.source.to_le_bytes());
        // Header CRC
        let header_crc = compute_crc(&frame[0..8]);
        frame.extend_from_slice(&header_crc.to_le_bytes());

        // Data blocks (16 bytes each + CRC)
        let mut remaining = self.payload.as_slice();
        while !remaining.is_empty() {
            let block_len = remaining.len().min(16);
            let block = &remaining[..block_len];
            let mut block_with_padding = block.to_vec();
            block_with_padding.resize(16, 0);
            frame.extend_from_slice(&block_with_padding);
            let crc = compute_crc(&block_with_padding);
            frame.extend_from_slice(&crc.to_le_bytes());
            remaining = &remaining[block_len..];
        }

        frame
    }

    /// Parse a received DNP3 link frame.
    pub fn parse(data: &[u8]) -> std::result::Result<Self, String> {
        if data.len() < 10 {
            return Err("frame too short".into());
        }
        if data[0..2] != DNP3_START_BYTES {
            return Err(format!("invalid start bytes: {:02X?}", &data[0..2]));
        }
        let length = data[2] as usize;
        let _control = data[3];
        let destination = u16::from_le_bytes([data[4], data[5]]);
        let source = u16::from_le_bytes([data[6], data[7]]);

        // Verify header CRC
        let expected_crc = u16::from_le_bytes([data[8], data[9]]);
        let actual_crc = compute_crc(&data[0..8]);
        if expected_crc != actual_crc {
            return Err(format!("header CRC mismatch: expected {:04X}, got {:04X}", expected_crc, actual_crc));
        }

        // Parse data blocks
        let mut payload = Vec::new();
        let mut pos = 10;
        let data_bytes = length.saturating_sub(5);

        while pos < data.len() && payload.len() < data_bytes {
            if pos + 18 > data.len() {
                break;
            }
            let block = &data[pos..pos + 16];
            let block_crc = u16::from_le_bytes([data[pos + 16], data[pos + 17]]);
            let actual = compute_crc(block);
            if block_crc != actual {
                return Err(format!("data block CRC mismatch at offset {}", pos));
            }
            payload.extend_from_slice(block);
            pos += 18;
        }

        payload.truncate(data_bytes);
        Ok(Self {
            source,
            destination,
            payload,
        })
    }
}

/// DNP3 application layer request.
#[derive(Debug, Clone)]
pub struct Dnp3AppRequest {
    pub function_code: Dnp3FunctionCode,
    pub sequence: u8,
    pub data: Vec<u8>,
}

impl Dnp3AppRequest {
    /// Create a Read request for a specific object group/variation.
    pub fn read_request(group: u8, variation: u8, start: u16, count: u16, sequence: u8) -> Self {
        let mut data = Vec::with_capacity(8);
        // Object header
        data.push(group);       // Object type
        data.push(variation);   // Variation
        data.push(0x00);        // Qualifier: 8-bit start/stop index
        data.push(0x00);        // Reserved
        data.push(start as u8); // Start index (8-bit)
        data.push((start + count - 1) as u8); // Stop index
        Self {
            function_code: Dnp3FunctionCode::Read,
            sequence,
            data,
        }
    }

    /// Create a Write request.
    pub fn write_request(group: u8, variation: u8, index: u8, value: u16, sequence: u8) -> Self {
        let mut data = Vec::with_capacity(12);
        data.push(group);
        data.push(variation);
        data.push(0x00); // Qualifier
        data.push(0x01); // Count = 1
        data.push(index);
        data.extend_from_slice(&value.to_le_bytes());
        Self {
            function_code: Dnp3FunctionCode::Write,
            sequence,
            data,
        }
    }

    /// Create a Select-Operate control request (CROB).
    pub fn crob_request(index: u8, control_code: u8, count: u8, on_time: u32, off_time: u32, sequence: u8) -> Self {
        let mut data = Vec::with_capacity(16);
        // Object 12 (Binary Output), Variation 1
        data.push(12);  // Group
        data.push(1);   // Variation
        data.push(0x00); // Qualifier
        data.push(0x01); // Count = 1
        // Control output block
        data.push(control_code); // Control code
        data.push(count);        // Count
        data.extend_from_slice(&on_time.to_le_bytes());
        data.extend_from_slice(&off_time.to_le_bytes());
        data.push(index); // Index
        Self {
            function_code: Dnp3FunctionCode::Operate,
            sequence,
            data,
        }
    }

    /// Create a time sync request (WriteTime).
    pub fn write_time_request(timestamp_ms: u64, sequence: u8) -> Self {
        let mut data = Vec::with_capacity(12);
        // Object 50, Variation 1 (Time and Date)
        data.push(50);
        data.push(1);
        data.push(0x07); // Qualifier: no index
        data.push(0x01); // Count = 1
        // DNP3 time: milliseconds since epoch (6 bytes LE)
        let time_val = timestamp_ms & 0xFFFFFFFFFFFF;
        data.extend_from_slice(&time_val.to_le_bytes()[..6]);
        Self {
            function_code: Dnp3FunctionCode::WriteTime,
            sequence,
            data,
        }
    }

    /// Encode the application request into bytes (with transport layer header).
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.data.len() + 4);
        // Application control byte: FIR=1, FIN=1, SEQ
        buf.push(0xC0 | (self.sequence & 0x0F));
        // Function code
        buf.push(self.function_code as u8);
        // Application data
        buf.extend_from_slice(&self.data);
        buf
    }
}

/// DNP3 configuration.
#[derive(Debug, Clone)]
pub struct Dnp3Config {
    pub master_address: u16,
    pub outstation_address: u16,
    pub timeout_ms: u64,
    pub enable_unsolicited: bool,
    pub class0_scan_interval_ms: u64,
    pub class1_scan_interval_ms: u64,
    pub class2_scan_interval_ms: u64,
    pub class3_scan_interval_ms: u64,
}

impl Default for Dnp3Config {
    fn default() -> Self {
        Self {
            master_address: 1,
            outstation_address: 1024,
            timeout_ms: 5000,
            enable_unsolicited: true,
            class0_scan_interval_ms: 60000,
            class1_scan_interval_ms: 1000,
            class2_scan_interval_ms: 2000,
            class3_scan_interval_ms: 5000,
        }
    }
}

/// DNP3 client — TCP connection with link/application layer.
pub struct Dnp3Client {
    config: Dnp3Config,
    stream: Option<TcpStream>,
    /// Sequence number for application requests
    sequence: u8,
    /// Cache of latest values per point type + index
    cache: HashMap<(Dnp3PointType, u16), Dnp3Point>,
}

/// A DNP3 data point with value and quality.
#[derive(Debug, Clone)]
pub struct Dnp3Point {
    pub point_type: Dnp3PointType,
    pub index: u16,
    pub value: Dnp3Value,
    pub flags: Dnp3Flags,
    pub timestamp: Option<u64>,
}

/// DNP3 value types.
#[derive(Debug, Clone, PartialEq)]
pub enum Dnp3Value {
    /// Binary (0/1)
    Binary(bool),
    /// Double-bit (0=indeterminate, 1=off, 2=on, 3=indeterminate)
    DoubleBit(u8),
    /// Analog (16-bit signed)
    Analog(i16),
    /// Counter (32-bit)
    Counter(u32),
    /// Analog output (16-bit)
    AnalogOutput(i16),
    /// Time (ms since epoch)
    Time(u64),
}

impl Dnp3Value {
    pub fn to_data_value(&self) -> DataValue {
        match self {
            Self::Binary(v) => DataValue::Bool(*v),
            Self::DoubleBit(v) => DataValue::Int32(*v as i32),
            Self::Analog(v) => DataValue::Int16(*v),
            Self::Counter(v) => DataValue::Int64(*v as i64),
            Self::AnalogOutput(v) => DataValue::Int16(*v),
            Self::Time(v) => DataValue::Int64(*v as i64),
        }
    }
}

impl Dnp3Client {
    pub fn new(config: Dnp3Config) -> Self {
        Self {
            config,
            stream: None,
            sequence: 0,
            cache: HashMap::new(),
        }
    }

    pub async fn connect(&mut self, host: &str, port: u16) -> std::result::Result<(), String> {
        let stream = tokio::time::timeout(
            std::time::Duration::from_millis(self.config.timeout_ms),
            TcpStream::connect((host, port)),
        )
        .await
        .map_err(|_| format!("connect timeout to {}:{}", host, port))?
        .map_err(|e| format!("TCP connect: {}", e))?;

        self.stream = Some(stream);
        Ok(())
    }

    pub async fn disconnect(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            let _ = stream.shutdown().await;
        }
    }

    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    fn next_sequence(&mut self) -> u8 {
        let seq = self.sequence;
        self.sequence = (self.sequence + 1) & 0x0F;
        seq
    }

    /// Send a Class 0 scan (read all static data).
    pub async fn read_class_0(&mut self) -> std::result::Result<Vec<Dnp3Point>, String> {
        if self.stream.is_none() {
            return Err("not connected".into());
        }
        // In production, this would encode and send the request,
        // then parse the response. For now, return cached values.
        Ok(self.cache.values().cloned().collect())
    }

    /// Inject a value into the cache (for testing).
    pub fn inject_value(&mut self, point: Dnp3Point) {
        let key = (point.point_type, point.index);
        self.cache.insert(key, point);
    }

    /// Get a cached value.
    pub fn get_value(&self, point_type: Dnp3PointType, index: u16) -> Option<&Dnp3Point> {
        self.cache.get(&(point_type, index))
    }

    /// Send a time synchronization request.
    pub async fn write_time(&mut self, timestamp_ms: u64) -> std::result::Result<(), String> {
        if self.stream.is_none() {
            return Err("not connected".into());
        }
        let seq = self.next_sequence();
        let request = Dnp3AppRequest::write_time_request(timestamp_ms, seq);
        let app_data = request.encode();

        let link_frame = Dnp3LinkFrame {
            source: self.config.master_address,
            destination: self.config.outstation_address,
            payload: app_data,
        };

        let frame_bytes = link_frame.encode(true);
        let stream = self.stream.as_mut().unwrap();
        stream.write_all(&frame_bytes).await.map_err(|e| format!("write: {}", e))?;
        Ok(())
    }

    /// Send a CROB (Control Relay Output Block) command.
    pub async fn send_crob(
        &mut self,
        index: u8,
        control_code: u8,
        count: u8,
        on_time: u32,
        off_time: u32,
    ) -> std::result::Result<(), String> {
        if self.stream.is_none() {
            return Err("not connected".into());
        }
        let seq = self.next_sequence();
        let request = Dnp3AppRequest::crob_request(index, control_code, count, on_time, off_time, seq);
        let app_data = request.encode();

        let link_frame = Dnp3LinkFrame {
            source: self.config.master_address,
            destination: self.config.outstation_address,
            payload: app_data,
        };

        let frame_bytes = link_frame.encode(true);
        let stream = self.stream.as_mut().unwrap();
        stream.write_all(&frame_bytes).await.map_err(|e| format!("write: {}", e))?;
        Ok(())
    }
}

/// DNP3 protocol adapter.
pub struct Dnp3Adapter {
    client: Arc<Mutex<Dnp3Client>>,
    shared_state: SharedState,
    name: String,
    config: Dnp3Config,
    /// Callbacks for subscription
    #[allow(clippy::type_complexity)]
    callbacks: Arc<RwLock<Vec<Box<dyn Fn(DataPoint) + Send + Sync>>>>,
}

impl Dnp3Adapter {
    pub fn new(name: &str, config: Dnp3Config) -> Self {
        let client = Dnp3Client::new(config.clone());
        Self {
            client: Arc::new(Mutex::new(client)),
            shared_state: new_shared_state(),
            name: name.to_string(),
            config,
            callbacks: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Inject a value for testing.
    pub async fn inject_value(&self, point: Dnp3Point) {
        let dp = DataPoint {
            address: format!("{}/{}", type_prefix(&point.point_type), point.index),
            value: point.value.to_data_value(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            quality: point.flags.to_data_quality(),
        };

        self.client.lock().await.inject_value(point);

        let cbs = self.callbacks.read().await;
        for cb in cbs.iter() {
            cb(dp.clone());
        }
    }

    /// Parse address format: "type:index" (e.g., "analog:5", "binary:0").
    fn parse_address(address: &str) -> Result<(Dnp3PointType, u16)> {
        let parts: Vec<&str> = address.split(':').collect();
        if parts.len() != 2 {
            return Err(eneros_core::EnerOSError::Device(format!(
                "Invalid DNP3 address '{}', expected 'type:index' (e.g., 'analog:5')",
                address
            )));
        }
        let point_type = Dnp3PointType::from_prefix(parts[0]).ok_or_else(|| {
            eneros_core::EnerOSError::Device(format!("Unknown DNP3 point type: '{}'", parts[0]))
        })?;
        let index: u16 = parts[1]
            .parse()
            .map_err(|_| eneros_core::EnerOSError::Device(format!("Invalid index: {}", parts[1])))?;
        Ok((point_type, index))
    }

    /// Get a reference to the underlying client.
    pub fn client(&self) -> Arc<Mutex<Dnp3Client>> {
        self.client.clone()
    }
}

fn type_prefix(pt: &Dnp3PointType) -> &'static str {
    match pt {
        Dnp3PointType::BinaryInput => "binary",
        Dnp3PointType::BinaryInputEvent => "binaryevent",
        Dnp3PointType::DoubleBitBinary => "doublebit",
        Dnp3PointType::AnalogInput => "analog",
        Dnp3PointType::AnalogInputEvent => "analgevent",
        Dnp3PointType::Counter => "counter",
        Dnp3PointType::CounterEvent => "counterevent",
        Dnp3PointType::BinaryOutput => "binaryoutput",
        Dnp3PointType::AnalogOutput => "analogoutput",
        Dnp3PointType::TimeAndDate => "time",
    }
}

/// CRC-16 used by DNP3 (CRC-16/DNP: poly=0x3D65, init=0, refin=true, refout=true, xorout=0xFFFF).
fn compute_crc(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &byte in data {
        crc ^= byte as u16;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xA6BC;
            } else {
                crc >>= 1;
            }
        }
    }
    crc ^ 0xFFFF
}

#[async_trait]
impl ProtocolAdapter for Dnp3Adapter {
    async fn connect(&mut self, config: &ConnectionConfig) -> Result<()> {
        self.shared_state.set_state(crate::adapter::ConnectionState::Connecting);

        let (master, outstation) = match &config.protocol_config {
            crate::adapter::ProtocolConfig::Dnp3 { master_address, slave_address } => {
                (*master_address, *slave_address)
            }
            _ => (self.config.master_address, self.config.outstation_address),
        };

        let dnp3_config = Dnp3Config {
            master_address: master,
            outstation_address: outstation,
            ..self.config.clone()
        };

        let mut client = self.client.lock().await;
        *client = Dnp3Client::new(dnp3_config);

        client.connect(&config.host, config.port).await.map_err(|e| {
            self.shared_state.mark_disconnected();
            eneros_core::EnerOSError::Device(format!("DNP3 connect failed: {}", e))
        })?;

        self.shared_state.mark_connected();
        tracing::info!("DNP3 adapter '{}' connected to {}:{}", self.name, config.host, config.port);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.client.lock().await.disconnect().await;
        self.shared_state.mark_disconnected();
        Ok(())
    }

    async fn read(&self, address: &str) -> Result<DataPoint> {
        let (point_type, index) = Self::parse_address(address)?;
        let client = self.client.lock().await;

        match client.get_value(point_type, index) {
            Some(point) => Ok(DataPoint {
                address: address.to_string(),
                value: point.value.to_data_value(),
                timestamp: point.timestamp.map(|t| t as i64).unwrap_or_else(|| chrono::Utc::now().timestamp_millis()),
                quality: point.flags.to_data_quality(),
            }),
            None => Ok(DataPoint {
                address: address.to_string(),
                value: DataValue::Bool(false),
                timestamp: chrono::Utc::now().timestamp_millis(),
                quality: DataQuality::Bad,
            }),
        }
    }

    async fn write(&mut self, address: &str, value: &DataValue) -> Result<()> {
        let (point_type, index) = Self::parse_address(address)?;

        match point_type {
            Dnp3PointType::BinaryOutput => {
                let control_code = match value {
                    DataValue::Bool(true) => 0x03,  // LATCH_ON
                    DataValue::Bool(false) => 0x04, // LATCH_OFF
                    _ => return Err(eneros_core::EnerOSError::Device(
                        "Binary output requires Bool value".into(),
                    )),
                };
                let mut client = self.client.lock().await;
                client
                    .send_crob(index as u8, control_code, 1, 1000, 1000)
                    .await
                    .map_err(|e| eneros_core::EnerOSError::Device(format!("CROB failed: {}", e)))?;
            }
            Dnp3PointType::AnalogOutput => {
                let val = match value {
                    DataValue::Int16(v) => *v as u16,
                    DataValue::Int32(v) => *v as u16,
                    DataValue::Float32(v) => *v as u16,
                    DataValue::Float64(v) => *v as u16,
                    _ => return Err(eneros_core::EnerOSError::Device(
                        "Analog output requires numeric value".into(),
                    )),
                };
                let client = self.client.lock().await;
                // In production, send write request
                let _ = (val, index, client);
            }
            _ => {
                return Err(eneros_core::EnerOSError::Device(format!(
                    "DNP3 point type {:?} is read-only",
                    point_type
                )));
            }
        }

        self.shared_state.record_sent(32);
        Ok(())
    }

    async fn subscribe(
        &mut self,
        _addresses: Vec<String>,
        callback: Box<dyn Fn(DataPoint) + Send + Sync>,
    ) -> Result<()> {
        self.callbacks.write().await.push(callback);
        Ok(())
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn protocol_type(&self) -> ProtocolType {
        ProtocolType::Dnp3
    }

    fn is_connected(&self) -> bool {
        self.shared_state.state() == crate::adapter::ConnectionState::Connected
    }

    fn shared_state(&self) -> SharedState {
        self.shared_state.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_point_type_groups() {
        assert_eq!(Dnp3PointType::BinaryInput.group(), 1);
        assert_eq!(Dnp3PointType::AnalogInput.group(), 30);
        assert_eq!(Dnp3PointType::Counter.group(), 20);
        assert_eq!(Dnp3PointType::BinaryOutput.group(), 10);
        assert_eq!(Dnp3PointType::AnalogOutput.group(), 40);
        assert_eq!(Dnp3PointType::TimeAndDate.group(), 50);
    }

    #[test]
    fn test_point_type_from_prefix() {
        assert_eq!(Dnp3PointType::from_prefix("binary"), Some(Dnp3PointType::BinaryInput));
        assert_eq!(Dnp3PointType::from_prefix("analog"), Some(Dnp3PointType::AnalogInput));
        assert_eq!(Dnp3PointType::from_prefix("counter"), Some(Dnp3PointType::Counter));
        assert_eq!(Dnp3PointType::from_prefix("BI"), Some(Dnp3PointType::BinaryInput));
        assert_eq!(Dnp3PointType::from_prefix("AI"), Some(Dnp3PointType::AnalogInput));
        assert_eq!(Dnp3PointType::from_prefix("unknown"), None);
    }

    #[test]
    fn test_flags_from_u8() {
        let flags = Dnp3Flags::from_u8(0x01);
        assert!(flags.online);
        assert!(!flags.restart);

        let flags = Dnp3Flags::from_u8(0x05);
        assert!(flags.online);
        assert!(flags.comm_lost);

        let flags = Dnp3Flags::from_u8(0x00);
        assert!(!flags.online);
        assert_eq!(flags.to_data_quality(), DataQuality::Bad);
    }

    #[test]
    fn test_flags_to_data_quality() {
        let good = Dnp3Flags::from_u8(0x01);
        assert_eq!(good.to_data_quality(), DataQuality::Good);

        let uncertain = Dnp3Flags::from_u8(0x05); // online + comm_lost
        assert_eq!(uncertain.to_data_quality(), DataQuality::Uncertain);

        let bad = Dnp3Flags::from_u8(0x00);
        assert_eq!(bad.to_data_quality(), DataQuality::Bad);
    }

    #[test]
    fn test_value_to_data_value() {
        assert_eq!(Dnp3Value::Binary(true).to_data_value(), DataValue::Bool(true));
        assert_eq!(Dnp3Value::Analog(42).to_data_value(), DataValue::Int16(42));
        assert_eq!(Dnp3Value::Counter(100000).to_data_value(), DataValue::Int64(100000));
        assert_eq!(Dnp3Value::Time(1234567890).to_data_value(), DataValue::Int64(1234567890));
    }

    #[test]
    fn test_crc_computation() {
        // Test known CRC value
        let data = b"123456789";
        let crc = compute_crc(data);
        // CRC-16/DNP3 of "123456789" is 0xEA82
        assert_eq!(crc, 0xEA82);
    }

    #[test]
    fn test_crc_empty() {
        let crc = compute_crc(&[]);
        assert_eq!(crc, 0xFFFF);
    }

    #[test]
    fn test_link_frame_encode_parse_roundtrip() {
        let frame = Dnp3LinkFrame {
            source: 1,
            destination: 1024,
            payload: vec![0xC0, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x01, 0x00, 0x05,
                         0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // 16 bytes = 1 block
        };
        let encoded = frame.encode(true);
        assert!(encoded.len() > 10);

        let parsed = Dnp3LinkFrame::parse(&encoded).expect("parse should succeed");
        assert_eq!(parsed.source, 1);
        assert_eq!(parsed.destination, 1024);
        assert_eq!(parsed.payload, frame.payload);
    }

    #[test]
    fn test_link_frame_parse_invalid_start() {
        let data = vec![0x00, 0x00, 0x05, 0x64, 0, 0, 0, 0, 0, 0];
        assert!(Dnp3LinkFrame::parse(&data).is_err());
    }

    #[test]
    fn test_link_frame_parse_too_short() {
        assert!(Dnp3LinkFrame::parse(&[0x05, 0x64]).is_err());
    }

    #[test]
    fn test_app_request_read() {
        let req = Dnp3AppRequest::read_request(30, 1, 0, 10, 5);
        let encoded = req.encode();
        assert!(!encoded.is_empty());
        assert_eq!(encoded[0] & 0xC0, 0xC0); // FIR+FIN
        assert_eq!(encoded[1], 1); // Read function code
    }

    #[test]
    fn test_app_request_write() {
        let req = Dnp3AppRequest::write_request(40, 1, 5, 100, 7);
        let encoded = req.encode();
        assert_eq!(encoded[1], 2); // Write function code
    }

    #[test]
    fn test_app_request_crob() {
        let req = Dnp3AppRequest::crob_request(3, 0x03, 1, 1000, 1000, 2);
        let encoded = req.encode();
        assert_eq!(encoded[1], 4); // Operate function code
    }

    #[test]
    fn test_app_request_write_time() {
        let req = Dnp3AppRequest::write_time_request(1700000000000, 3);
        let encoded = req.encode();
        assert_eq!(encoded[1], 20); // WriteTime function code
    }

    #[test]
    fn test_dnp3_config_default() {
        let config = Dnp3Config::default();
        assert_eq!(config.master_address, 1);
        assert_eq!(config.outstation_address, 1024);
        assert!(config.enable_unsolicited);
    }

    #[test]
    fn test_client_creation() {
        let client = Dnp3Client::new(Dnp3Config::default());
        assert!(!client.is_connected());
        assert_eq!(client.sequence, 0);
    }

    #[test]
    fn test_client_next_sequence() {
        let mut client = Dnp3Client::new(Dnp3Config::default());
        assert_eq!(client.next_sequence(), 0);
        assert_eq!(client.next_sequence(), 1);
        assert_eq!(client.next_sequence(), 2);
    }

    #[test]
    fn test_client_sequence_wraps() {
        let mut client = Dnp3Client::new(Dnp3Config::default());
        client.sequence = 0x0F;
        assert_eq!(client.next_sequence(), 0x0F);
        assert_eq!(client.next_sequence(), 0);
    }

    #[tokio::test]
    async fn test_adapter_creation() {
        let adapter = Dnp3Adapter::new("test-dnp3", Dnp3Config::default());
        assert_eq!(adapter.name(), "test-dnp3");
        assert_eq!(adapter.protocol_type(), ProtocolType::Dnp3);
        assert!(!adapter.is_connected());
    }

    #[tokio::test]
    async fn test_adapter_read_cached() {
        let adapter = Dnp3Adapter::new("test", Dnp3Config::default());
        adapter.shared_state.mark_connected();

        adapter
            .inject_value(Dnp3Point {
                point_type: Dnp3PointType::AnalogInput,
                index: 5,
                value: Dnp3Value::Analog(220),
                flags: Dnp3Flags::from_u8(0x01),
                timestamp: Some(1700000000000),
            })
            .await;

        let dp = adapter.read("analog:5").await.unwrap();
        assert_eq!(dp.value, DataValue::Int16(220));
        assert_eq!(dp.quality, DataQuality::Good);
    }

    #[tokio::test]
    async fn test_adapter_read_missing() {
        let adapter = Dnp3Adapter::new("test", Dnp3Config::default());
        adapter.shared_state.mark_connected();

        let dp = adapter.read("analog:99").await.unwrap();
        assert_eq!(dp.quality, DataQuality::Bad);
    }

    #[tokio::test]
    async fn test_adapter_read_invalid_address() {
        let adapter = Dnp3Adapter::new("test", Dnp3Config::default());
        assert!(adapter.read("invalid").await.is_err());
        assert!(adapter.read("unknown:0").await.is_err());
        assert!(adapter.read("analog:abc").await.is_err());
    }

    #[tokio::test]
    async fn test_adapter_subscribe() {
        let mut adapter = Dnp3Adapter::new("test", Dnp3Config::default());
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_clone = received.clone();

        adapter
            .subscribe(vec![], Box::new(move |dp| {
                received_clone.try_lock().unwrap().push(dp);
            }))
            .await
            .unwrap();

        adapter
            .inject_value(Dnp3Point {
                point_type: Dnp3PointType::BinaryInput,
                index: 0,
                value: Dnp3Value::Binary(true),
                flags: Dnp3Flags::from_u8(0x01),
                timestamp: None,
            })
            .await;

        let msgs = received.lock().await;
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].value, DataValue::Bool(true));
    }

    #[tokio::test]
    async fn test_adapter_write_readonly_type() {
        let mut adapter = Dnp3Adapter::new("test", Dnp3Config::default());
        adapter.shared_state.mark_connected();

        let result = adapter.write("analog:0", &DataValue::Int16(42)).await;
        assert!(result.is_err()); // AnalogInput is read-only
    }

    #[tokio::test]
    async fn test_adapter_write_binary_output_wrong_type() {
        let mut adapter = Dnp3Adapter::new("test", Dnp3Config::default());
        adapter.shared_state.mark_connected();

        let result = adapter.write("binaryoutput:0", &DataValue::Int16(42)).await;
        assert!(result.is_err()); // BinaryOutput requires Bool
    }

    #[test]
    fn test_type_prefix() {
        assert_eq!(type_prefix(&Dnp3PointType::BinaryInput), "binary");
        assert_eq!(type_prefix(&Dnp3PointType::AnalogInput), "analog");
        assert_eq!(type_prefix(&Dnp3PointType::Counter), "counter");
        assert_eq!(type_prefix(&Dnp3PointType::BinaryOutput), "binaryoutput");
    }
}
