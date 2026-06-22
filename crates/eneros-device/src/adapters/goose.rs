//! GOOSE (Generic Object Oriented Substation Events) protocol adapter.
//!
//! GOOSE is a Layer 2 Ethernet multicast protocol defined in IEC 61850-8-1
//! for fast event transmission in substation automation. It operates directly
//! on Ethernet frames (EtherType 0x88B8) without IP/TCP/UDP overhead, enabling
//! sub-4ms event delivery.
//!
//! # Architecture
//!
//! ```text
//! IED (publisher) ──Ethernet multicast──► GooseAdapter (subscriber)
//!                                              │
//!                                              ├── GooseFrame parser (BER)
//!                                              ├── Dataset → DataValue mapping
//!                                              └── ProtocolAdapter trait
//! ```
//!
//! # Portability
//!
//! Because raw Ethernet sockets require elevated privileges and are
//! platform-specific, the transport is abstracted behind `GooseTransport`.
//! Production deployments use `AfPacketTransport` (Linux AF_PACKET); tests use
//! `MockGooseTransport` which injects raw frames directly.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use eneros_core::Result;
use crate::adapter::{
    ProtocolAdapter, ConnectionConfig, DataPoint, DataValue, DataQuality,
    SharedState, new_shared_state,
};
use crate::protocol::ProtocolType;
// AF_PACKET 传输集成（Linux 原始套接字，用于生产环境 GOOSE Layer 2 直采）
use crate::adapters::af_packet::{AfPacketConfig, AdapterError as AfPacketAdapterError};
#[cfg(target_os = "linux")]
use crate::adapters::af_packet::AfPacketTransport;

/// GOOSE EtherType (IEEE 802.3)
pub const GOOSE_ETHERTYPE: u16 = 0x88B8;

/// Default GOOSE multicast MAC prefix (IEC 61850-8-1)
/// 01-0C-CD-01-00-00 .. 01-0C-CD-01-00-3F (64 addresses)
pub const GOOSE_MULTICAST_PREFIX: [u8; 5] = [0x01, 0x0C, 0xCD, 0x01, 0x00];

/// GOOSE configuration
#[derive(Debug, Clone)]
pub struct GooseConfig {
    /// Interface name (e.g., "eth0") for pcap backend
    pub interface: String,
    /// APPID to filter (0 = accept all)
    pub appid_filter: u16,
    /// GOOSE control block reference to subscribe (empty = accept all)
    pub gocb_ref_filter: String,
    /// Multicast MAC address (last byte 0x00-0x3F)
    pub multicast_mac: [u8; 6],
    /// Time allowed to live (ms) — publisher hint for subscriber
    pub time_allowed_to_live_ms: u32,
}

impl Default for GooseConfig {
    fn default() -> Self {
        Self {
            interface: "any".to_string(),
            appid_filter: 0,
            gocb_ref_filter: String::new(),
            multicast_mac: [0x01, 0x0C, 0xCD, 0x01, 0x00, 0x00],
            time_allowed_to_live_ms: 1000,
        }
    }
}

/// GOOSE dataset entry — a single value in the allData sequence.
#[derive(Debug, Clone, PartialEq)]
pub enum GooseData {
    /// Boolean state (single point)
    Bool(bool),
    /// Quality (bitmask)
    Quality(u8),
    /// Time tag (CP56Time2a)
    TimeTag(u64),
    /// Integer
    Int(i64),
    /// Floating point
    Float(f64),
    /// Octet string
    Bytes(Vec<u8>),
    /// Visible string
    String(String),
}

impl GooseData {
    /// Convert to DataValue for the ProtocolAdapter interface.
    pub fn to_data_value(&self) -> DataValue {
        match self {
            GooseData::Bool(v) => DataValue::Bool(*v),
            GooseData::Quality(v) => DataValue::Int32(*v as i32),
            GooseData::TimeTag(v) => DataValue::Int64(*v as i64),
            GooseData::Int(v) => DataValue::Int64(*v),
            GooseData::Float(v) => DataValue::Float64(*v),
            GooseData::Bytes(v) => DataValue::Bytes(v.clone()),
            GooseData::String(v) => DataValue::String(v.clone()),
        }
    }
}

/// Parsed GOOSE frame (IEC 61850-8-1).
#[derive(Debug, Clone)]
pub struct GooseFrame {
    /// APPID from the SV/GOOSE header
    pub appid: u16,
    /// GOOSE control block reference (e.g., "IED1_LD0/LLN0$GO$gcb1")
    pub gocb_ref: String,
    /// Time allowed to live (ms)
    pub time_allowed_to_live: u32,
    /// Dataset reference (e.g., "IED1_LD0/LLN0$dsGeneric")
    pub dat_set: String,
    /// GOOSE ID (optional, may equal gocb_ref)
    pub go_id: String,
    /// Event timestamp (ms since epoch, from CP56Time2a)
    pub t: u64,
    /// State number — increments on value change
    pub st_num: u32,
    /// Sequence number — increments on retransmission
    pub sq_num: u32,
    /// Test mode flag
    pub simulation: bool,
    /// Configuration revision number
    pub conf_rev: u32,
    /// Needs commissioning flag
    pub nds_com: bool,
    /// Number of dataset entries
    pub num_dat_set_entries: u32,
    /// Dataset values
    pub all_data: Vec<GooseData>,
}

impl GooseFrame {
    /// Parse a raw Ethernet frame into a GooseFrame.
    ///
    /// The input should be the full Ethernet frame starting with the
    /// destination MAC. This function validates the EtherType and
    /// extracts the GOOSE PDU.
    pub fn parse(eth_frame: &[u8]) -> std::result::Result<Self, GooseParseError> {
        // Ethernet header: 6 dst + 6 src + 2 ethertype = 14 bytes
        if eth_frame.len() < 14 {
            return Err(GooseParseError::TooShort);
        }
        let ethertype = u16::from_be_bytes([eth_frame[12], eth_frame[13]]);
        if ethertype != GOOSE_ETHERTYPE {
            return Err(GooseParseError::WrongEtherType(ethertype));
        }

        // GOOSE header: 2 APPID + 2 Length + 4 Reserved = 8 bytes
        let payload = &eth_frame[14..];
        if payload.len() < 8 {
            return Err(GooseParseError::HeaderTooShort);
        }
        let appid = u16::from_be_bytes([payload[0], payload[1]]);
        let length = u16::from_be_bytes([payload[2], payload[3]]) as usize;
        if length < 8 {
            return Err(GooseParseError::HeaderTooShort);
        }
        // Reserved 4 bytes at payload[4..8]
        let pdu = &payload[8..];
        if pdu.len() < length.saturating_sub(8) {
            return Err(GooseParseError::LengthMismatch);
        }

        // Parse GOOSE PDU (ASN.1 BER)
        let parser = BerParser::new(pdu);
        let frame = parser.parse_goose_pdu(appid)?;
        Ok(frame)
    }

    /// Serialize this GooseFrame into a raw Ethernet frame.
    /// The destination MAC is set to the GOOSE multicast address.
    pub fn serialize(&self, src_mac: &[u8; 6]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(64 + self.all_data.len() * 16);
        // Ethernet header
        buf.extend_from_slice(&GOOSE_MULTICAST_PREFIX);
        buf.push(0x00); // last byte of multicast MAC
        buf.extend_from_slice(src_mac);
        buf.extend_from_slice(&GOOSE_ETHERTYPE.to_be_bytes());
        // GOOSE header
        buf.extend_from_slice(&self.appid.to_be_bytes());
        let length_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes()); // placeholder
        buf.extend_from_slice(&0u32.to_be_bytes()); // reserved
        // GOOSE PDU (BER encoded)
        let pdu_start = buf.len();
        self.encode_ber(&mut buf);
        let pdu_len = buf.len() - pdu_start;
        let total_len = 8 + pdu_len;
        buf[length_pos..length_pos + 2]
            .copy_from_slice(&(total_len as u16).to_be_bytes());
        buf
    }

    fn encode_ber(&self, buf: &mut Vec<u8>) {
        // GOOSE PDU is a SEQUENCE (tag 0x60)
        let mut inner = Vec::new();
        // gocbRef [0] VisibleString
        encode_visible_string(&mut inner, 0x80, &self.gocb_ref);
        // timeAllowedToLive [1] INTEGER
        encode_integer(&mut inner, 0x81, self.time_allowed_to_live as i64);
        // datSet [2] VisibleString
        encode_visible_string(&mut inner, 0x82, &self.dat_set);
        // goID [3] VisibleString (optional)
        if !self.go_id.is_empty() {
            encode_visible_string(&mut inner, 0x83, &self.go_id);
        }
        // t [4] UtcTime (7 bytes: 4 sec + 3 frac)
        encode_utc_time(&mut inner, 0x84, self.t);
        // stNum [5] INTEGER
        encode_integer(&mut inner, 0x85, self.st_num as i64);
        // sqNum [6] INTEGER
        encode_integer(&mut inner, 0x86, self.sq_num as i64);
        // simulation [7] BOOLEAN
        encode_boolean(&mut inner, 0x87, self.simulation);
        // confRev [8] INTEGER
        encode_integer(&mut inner, 0x88, self.conf_rev as i64);
        // ndsCom [9] BOOLEAN
        encode_boolean(&mut inner, 0x89, self.nds_com);
        // numDatSetEntries [10] INTEGER
        encode_integer(&mut inner, 0x8A, self.num_dat_set_entries as i64);
        // allData [11] SEQUENCE OF Data
        let mut data_seq = Vec::new();
        for d in &self.all_data {
            match d {
                GooseData::Bool(v) => encode_boolean(&mut data_seq, 0x01, *v),
                GooseData::Quality(v) => encode_bit_string(&mut data_seq, 0x03, *v, 8),
                GooseData::TimeTag(v) => encode_utc_time(&mut data_seq, 0x04, *v),
                GooseData::Int(v) => encode_integer_universal(&mut data_seq, 0x02, *v),
                GooseData::Float(v) => encode_float(&mut data_seq, 0x04, *v),
                GooseData::Bytes(v) => encode_octet_string(&mut data_seq, 0x04, v),
                GooseData::String(v) => encode_visible_string(&mut data_seq, 0x1A, v),
            }
        }
        encode_tlv(&mut inner, 0xAB, &data_seq);

        // Wrap in SEQUENCE
        encode_tlv(buf, 0x60, &inner);
    }
}

/// GOOSE parse error
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GooseParseError {
    TooShort,
    HeaderTooShort,
    WrongEtherType(u16),
    LengthMismatch,
    BerError(String),
    MissingField(&'static str),
}

impl std::fmt::Display for GooseParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort => write!(f, "frame too short for Ethernet header"),
            Self::HeaderTooShort => write!(f, "GOOSE header too short"),
            Self::WrongEtherType(et) => write!(f, "wrong EtherType 0x{:04X}, expected 0x{:04X}", et, GOOSE_ETHERTYPE),
            Self::LengthMismatch => write!(f, "GOOSE length field doesn't match payload"),
            Self::BerError(msg) => write!(f, "BER decode error: {}", msg),
            Self::MissingField(name) => write!(f, "missing required GOOSE field: {}", name),
        }
    }
}

impl std::error::Error for GooseParseError {}

/// Minimal ASN.1 BER parser for GOOSE PDU.
struct BerParser<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BerParser<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn parse_goose_pdu(mut self, appid: u16) -> std::result::Result<GooseFrame, GooseParseError> {
        // Expect SEQUENCE (tag 0x60)
        let (tag, content) = self.read_tlv()?;
        if tag != 0x60 {
            return Err(GooseParseError::BerError(format!(
                "expected SEQUENCE 0x60, got 0x{:02X}",
                tag
            )));
        }

        let mut parser = BerParser::new(content);
        let mut frame = GooseFrame {
            appid,
            gocb_ref: String::new(),
            time_allowed_to_live: 0,
            dat_set: String::new(),
            go_id: String::new(),
            t: 0,
            st_num: 0,
            sq_num: 0,
            simulation: false,
            conf_rev: 0,
            nds_com: false,
            num_dat_set_entries: 0,
            all_data: Vec::new(),
        };

        while parser.pos < parser.data.len() {
            let (tag, content) = parser.read_tlv()?;
            match tag {
                0x80 => frame.gocb_ref = decode_visible_string(content)?,
                0x81 => frame.time_allowed_to_live = decode_integer(content)? as u32,
                0x82 => frame.dat_set = decode_visible_string(content)?,
                0x83 => frame.go_id = decode_visible_string(content)?,
                0x84 => frame.t = decode_utc_time(content)?,
                0x85 => frame.st_num = decode_integer(content)? as u32,
                0x86 => frame.sq_num = decode_integer(content)? as u32,
                0x87 => frame.simulation = decode_boolean(content),
                0x88 => frame.conf_rev = decode_integer(content)? as u32,
                0x89 => frame.nds_com = decode_boolean(content),
                0x8A => frame.num_dat_set_entries = decode_integer(content)? as u32,
                0xAB => frame.all_data = parse_all_data(content)?,
                _ => {
                    // Unknown tag — skip for forward compatibility
                }
            }
        }

        if frame.gocb_ref.is_empty() {
            return Err(GooseParseError::MissingField("gocbRef"));
        }
        if frame.dat_set.is_empty() {
            return Err(GooseParseError::MissingField("datSet"));
        }

        Ok(frame)
    }

    fn read_tlv(&mut self) -> std::result::Result<(u8, &'a [u8]), GooseParseError> {
        if self.pos + 2 > self.data.len() {
            return Err(GooseParseError::BerError("unexpected end of data".into()));
        }
        let tag = self.data[self.pos];
        self.pos += 1;
        let len_byte = self.data[self.pos];
        self.pos += 1;

        let len = if len_byte & 0x80 == 0 {
            len_byte as usize
        } else {
            let num_bytes = (len_byte & 0x7F) as usize;
            if num_bytes == 0 || num_bytes > 4 {
                return Err(GooseParseError::BerError(format!(
                    "unsupported length encoding: {} bytes",
                    num_bytes
                )));
            }
            if self.pos + num_bytes > self.data.len() {
                return Err(GooseParseError::BerError("length bytes truncated".into()));
            }
            let mut len = 0usize;
            for i in 0..num_bytes {
                len = (len << 8) | self.data[self.pos + i] as usize;
            }
            self.pos += num_bytes;
            len
        };

        if self.pos + len > self.data.len() {
            return Err(GooseParseError::BerError(format!(
                "content length {} exceeds remaining {}",
                len,
                self.data.len() - self.pos
            )));
        }

        let content = &self.data[self.pos..self.pos + len];
        self.pos += len;
        Ok((tag, content))
    }
}

fn parse_all_data(data: &[u8]) -> std::result::Result<Vec<GooseData>, GooseParseError> {
    let mut parser = BerParser::new(data);
    let mut result = Vec::new();
    while parser.pos < parser.data.len() {
        let (tag, content) = parser.read_tlv()?;
        let value = match tag {
            0x01 => GooseData::Bool(decode_boolean(content)),
            0x03 => {
                // BIT STRING — first byte is unused bits count
                if content.len() >= 2 {
                    GooseData::Quality(content[1])
                } else {
                    // content 只有 unused bits 字节或为空，无实际数据
                    GooseData::Quality(0)
                }
            }
            0x04 => {
                // Could be OCTET STRING or UtcTime — treat as bytes
                GooseData::Bytes(content.to_vec())
            }
            0x02 => GooseData::Int(decode_integer(content)?),
            0x05 => {
                // NULL
                GooseData::Bool(false)
            }
            0x1A => GooseData::String(decode_visible_string(content)?),
            _ => {
                // Unknown — store as bytes
                GooseData::Bytes(content.to_vec())
            }
        };
        result.push(value);
    }
    Ok(result)
}

fn decode_visible_string(data: &[u8]) -> std::result::Result<String, GooseParseError> {
    String::from_utf8(data.to_vec())
        .map_err(|e| GooseParseError::BerError(format!("invalid UTF-8 in string: {}", e)))
}

fn decode_integer(data: &[u8]) -> std::result::Result<i64, GooseParseError> {
    if data.is_empty() {
        return Ok(0);
    }
    let mut val = if data[0] & 0x80 != 0 {
        -1i64
    } else {
        0i64
    };
    for &b in data {
        val = (val << 8) | (b as i64 & 0xFF);
    }
    Ok(val)
}

fn decode_boolean(data: &[u8]) -> bool {
    !data.is_empty() && data[0] != 0
}

fn decode_utc_time(data: &[u8]) -> std::result::Result<u64, GooseParseError> {
    if data.len() < 4 {
        return Err(GooseParseError::BerError("UtcTime too short".into()));
    }
    let secs = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as u64;
    // Fractional part (3 bytes) — ignored for ms precision
    Ok(secs * 1000)
}

// BER encoding helpers
fn encode_tlv(buf: &mut Vec<u8>, tag: u8, content: &[u8]) {
    buf.push(tag);
    encode_length(buf, content.len());
    buf.extend_from_slice(content);
}

fn encode_length(buf: &mut Vec<u8>, len: usize) {
    if len < 0x80 {
        buf.push(len as u8);
    } else if len <= 0xFF {
        buf.push(0x81);
        buf.push(len as u8);
    } else if len <= 0xFFFF {
        buf.push(0x82);
        buf.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        buf.push(0x83);
        buf.extend_from_slice(&((len as u32) & 0x00FFFFFF).to_be_bytes()[1..]);
    }
}

fn encode_visible_string(buf: &mut Vec<u8>, tag: u8, s: &str) {
    encode_tlv(buf, tag, s.as_bytes());
}

fn encode_integer(buf: &mut Vec<u8>, tag: u8, val: i64) {
    let bytes = if val == 0 {
        vec![0u8]
    } else if val > 0 {
        let mut v = val as u64;
        let mut bytes = Vec::new();
        while v > 0 {
            bytes.push((v & 0xFF) as u8);
            v >>= 8;
        }
        if bytes.last().is_some_and(|&b| b & 0x80 != 0) {
            bytes.push(0);
        }
        bytes.reverse();
        bytes
    } else {
        // Negative — simple two's complement
        let mut bytes = Vec::new();
        let mut v = val;
        while v != -1 || bytes.is_empty() {
            bytes.push((v & 0xFF) as u8);
            v >>= 8;
            if v == -1 && bytes.last().is_some_and(|&b| b & 0x80 != 0) {
                break;
            }
        }
        bytes.reverse();
        bytes
    };
    encode_tlv(buf, tag, &bytes);
}

fn encode_integer_universal(buf: &mut Vec<u8>, tag: u8, val: i64) {
    encode_integer(buf, tag, val);
}

fn encode_boolean(buf: &mut Vec<u8>, tag: u8, val: bool) {
    encode_tlv(buf, tag, &[if val { 0xFF } else { 0x00 }]);
}

fn encode_bit_string(buf: &mut Vec<u8>, tag: u8, val: u8, _bits: u8) {
    // BIT STRING: 1 byte unused bits + data
    encode_tlv(buf, tag, &[0x00, val]);
}

fn encode_utc_time(buf: &mut Vec<u8>, tag: u8, ms: u64) {
    let secs = (ms / 1000) as u32;
    let frac = ((ms % 1000) * 0xFFFFFF / 1000) as u32;
    let mut content = Vec::with_capacity(7);
    content.extend_from_slice(&secs.to_be_bytes());
    content.push(((frac >> 16) & 0xFF) as u8);
    content.push(((frac >> 8) & 0xFF) as u8);
    content.push((frac & 0xFF) as u8);
    encode_tlv(buf, tag, &content);
}

fn encode_float(buf: &mut Vec<u8>, tag: u8, val: f64) {
    // IEC 61850 uses 64-bit float (BDOUBLE)
    encode_tlv(buf, tag, &val.to_be_bytes());
}

fn encode_octet_string(buf: &mut Vec<u8>, tag: u8, data: &[u8]) {
    encode_tlv(buf, tag, data);
}

/// Transport abstraction for GOOSE frame I/O.
///
/// Production uses Linux AF_PACKET (`AfPacketTransport`); tests use
/// `MockGooseTransport`. The transport handles raw Ethernet frame I/O —
/// callers provide/expect full Ethernet frames (dst MAC + src MAC +
/// EtherType + payload).
#[async_trait]
pub trait GooseTransport: Send + Sync {
    /// Receive the next GOOSE frame (blocks until a frame is available).
    ///
    /// Returns the full Ethernet frame (including the 14-byte header).
    /// Implementations are expected to filter by EtherType 0x88B8.
    async fn receive(&self) -> std::result::Result<Vec<u8>, String>;
    /// Send a raw Ethernet frame.
    ///
    /// The `frame` argument must be a complete Ethernet frame (dst MAC +
    /// src MAC + EtherType + payload).
    async fn send(&self, frame: &[u8]) -> std::result::Result<(), String>;
}

/// Mock transport for testing — injects frames via a channel.
pub struct MockGooseTransport {
    rx: Mutex<tokio::sync::mpsc::Receiver<Vec<u8>>>,
    /// Retained sender to keep the receive channel open. In `new()`, a clone
    /// is also returned to the caller for injecting frames.
    /// 该字段从不被读取，仅用于保持通道开启（防止 `receive()` 立即返回错误）。
    #[allow(dead_code)]
    tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    sent: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl MockGooseTransport {
    /// Create a mock transport and a sender handle for injecting frames.
    pub fn new() -> (Self, tokio::sync::mpsc::Sender<Vec<u8>>) {
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let sent = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                rx: Mutex::new(rx),
                tx: tx.clone(),
                sent,
            },
            tx,
        )
    }

    /// Get all frames that were sent via `send()`.
    pub async fn sent_frames(&self) -> Vec<Vec<u8>> {
        self.sent.lock().await.clone()
    }
}

impl Default for MockGooseTransport {
    fn default() -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let sent = Arc::new(Mutex::new(Vec::new()));
        Self {
            rx: Mutex::new(rx),
            tx,
            sent,
        }
    }
}

#[async_trait]
impl GooseTransport for MockGooseTransport {
    async fn receive(&self) -> std::result::Result<Vec<u8>, String> {
        self.rx
            .lock()
            .await
            .recv()
            .await
            .ok_or_else(|| "channel closed".to_string())
    }

    async fn send(&self, frame: &[u8]) -> std::result::Result<(), String> {
        self.sent.lock().await.push(frame.to_vec());
        Ok(())
    }
}

/// GOOSE protocol adapter.
///
/// Subscribes to GOOSE multicast frames, parses them, and exposes
/// dataset values via the `ProtocolAdapter` trait. Address format:
/// `dataset_index` (e.g., "0" for the first entry in allData).
pub struct GooseAdapter {
    transport: Arc<Box<dyn GooseTransport>>,
    shared_state: SharedState,
    name: String,
    config: GooseConfig,
    /// Cache of latest GOOSE frame per gocb_ref
    cache: Arc<RwLock<HashMap<String, GooseFrame>>>,
    /// Callbacks registered via subscribe()
    #[allow(clippy::type_complexity)]
    callbacks: Arc<RwLock<Vec<Box<dyn Fn(DataPoint) + Send + Sync>>>>,
    /// Receive task handle
    recv_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl GooseAdapter {
    /// Create a new GOOSE adapter with the given transport.
    ///
    /// 任意实现了 `GooseTransport` 的传输层均可注入，包括
    /// `MockGooseTransport`（测试）与 `AfPacketTransport`（生产）。
    pub fn with_transport(name: &str, config: GooseConfig, transport: Box<dyn GooseTransport>) -> Self {
        Self {
            transport: Arc::new(transport),
            shared_state: new_shared_state(),
            name: name.to_string(),
            config,
            cache: Arc::new(RwLock::new(HashMap::new())),
            callbacks: Arc::new(RwLock::new(Vec::new())),
            recv_handle: Arc::new(Mutex::new(None)),
        }
    }

    /// Create a GOOSE adapter with mock transport (for testing).
    pub fn new_mock(name: &str) -> (Self, tokio::sync::mpsc::Sender<Vec<u8>>) {
        let (transport, sender) = MockGooseTransport::new();
        let adapter = Self::with_transport(name, GooseConfig::default(), Box::new(transport));
        (adapter, sender)
    }

    /// 使用 AF_PACKET 传输创建 GOOSE 适配器（生产环境推荐）。
    ///
    /// 在 Linux 平台创建 `AfPacketTransport` 并接入 `GooseAdapter`，
    /// 实现真实的 Layer 2 GOOSE 收发。非 Linux 平台返回
    /// `AdapterError::Unsupported`。
    ///
    /// # 参数
    /// - `name`: 适配器实例名称
    /// - `config`: GOOSE 协议配置（APPID 过滤、gocb_ref 过滤、组播 MAC 等）
    /// - `af_config`: AF_PACKET 传输配置（网卡名、源 MAC），通常通过
    ///   `AfPacketConfig::for_goose(interface, src_mac)` 构建
    ///
    /// # 平台
    /// - Linux：创建 `AF_PACKET + SOCK_RAW` 套接字，需要 `CAP_NET_RAW` 能力
    /// - 非 Linux：返回 `AdapterError::Unsupported`
    #[cfg(target_os = "linux")]
    pub fn with_af_packet(
        name: &str,
        config: GooseConfig,
        af_config: AfPacketConfig,
    ) -> std::result::Result<Self, AfPacketAdapterError> {
        let transport = AfPacketTransport::new(af_config)?;
        Ok(Self::with_transport(name, config, Box::new(transport)))
    }

    /// 非 Linux 平台的 `with_af_packet` stub —— 始终返回 `Unsupported`。
    ///
    /// AF_PACKET 是 Linux 专有的原始套接字机制，在 Windows/macOS 等
    /// 平台不可用。此 stub 保证 API 跨平台编译通过。
    #[cfg(not(target_os = "linux"))]
    pub fn with_af_packet(
        _name: &str,
        _config: GooseConfig,
        _af_config: AfPacketConfig,
    ) -> std::result::Result<Self, AfPacketAdapterError> {
        Err(AfPacketAdapterError::Unsupported(
            "AF_PACKET requires Linux".into(),
        ))
    }

    /// Get the latest cached frame for a given gocb_ref.
    pub async fn get_latest_frame(&self, gocb_ref: &str) -> Option<GooseFrame> {
        self.cache.read().await.get(gocb_ref).cloned()
    }

    /// Get all cached gocb_refs.
    pub async fn cached_refs(&self) -> Vec<String> {
        self.cache.read().await.keys().cloned().collect()
    }

    /// Inject a parsed frame directly into the cache (for testing).
    pub async fn inject_frame(&self, frame: GooseFrame) {
        let gocb_ref = frame.gocb_ref.clone();
        let all_data = frame.all_data.clone();
        self.cache.write().await.insert(gocb_ref.clone(), frame);

        // Notify callbacks
        let callbacks = self.callbacks.read().await;
        for (idx, data) in all_data.iter().enumerate() {
            let dp = DataPoint {
                address: format!("{}/{}", gocb_ref, idx),
                value: data.to_data_value(),
                timestamp: chrono::Utc::now().timestamp_millis(),
                quality: DataQuality::Good,
            };
            for cb in callbacks.iter() {
                cb(dp.clone());
            }
        }
    }

    /// Start the background receive loop that parses frames and updates the cache.
    pub async fn start_receive_loop(&self) {
        let transport = self.transport.clone();
        let cache = self.cache.clone();
        let callbacks = self.callbacks.clone();
        let shared = self.shared_state.clone();
        let appid_filter = self.config.appid_filter;
        let gocb_filter = self.config.gocb_ref_filter.clone();

        let handle = tokio::spawn(async move {
            loop {
                let frame_bytes = match transport.receive().await {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        tracing::warn!("GOOSE receive error: {}", e);
                        break;
                    }
                };

                match GooseFrame::parse(&frame_bytes) {
                    Ok(goose) => {
                        // Apply filters
                        if appid_filter != 0 && goose.appid != appid_filter {
                            continue;
                        }
                        if !gocb_filter.is_empty() && goose.gocb_ref != gocb_filter {
                            continue;
                        }

                        shared.record_received(frame_bytes.len() as u64);
                        let gocb_ref = goose.gocb_ref.clone();
                        let all_data = goose.all_data.clone();

                        cache.write().await.insert(gocb_ref.clone(), goose);

                        // Notify callbacks
                        let cbs = callbacks.read().await;
                        for (idx, data) in all_data.iter().enumerate() {
                            let dp = DataPoint {
                                address: format!("{}/{}", gocb_ref, idx),
                                value: data.to_data_value(),
                                timestamp: chrono::Utc::now().timestamp_millis(),
                                quality: DataQuality::Good,
                            };
                            for cb in cbs.iter() {
                                cb(dp.clone());
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("GOOSE parse error: {}", e);
                    }
                }
            }
            tracing::info!("GOOSE receive loop stopped");
        });

        *self.recv_handle.lock().await = Some(handle);
    }

    /// Stop the receive loop.
    pub async fn stop_receive_loop(&self) {
        if let Some(h) = self.recv_handle.lock().await.take() {
            h.abort();
        }
    }

    /// Publish a GOOSE frame.
    pub async fn publish(&self, frame: &GooseFrame, src_mac: &[u8; 6]) -> Result<()> {
        let bytes = frame.serialize(src_mac);
        self.transport
            .send(&bytes)
            .await
            .map_err(|e| eneros_core::EnerOSError::Device(format!("GOOSE send failed: {}", e)))?;
        self.shared_state.record_sent(bytes.len() as u64);
        Ok(())
    }

    fn parse_address(address: &str) -> Result<(String, usize)> {
        let parts: Vec<&str> = address.split('/').collect();
        if parts.len() < 2 {
            return Err(eneros_core::EnerOSError::Device(format!(
                "Invalid GOOSE address '{}', expected 'gocb_ref/index' (e.g., 'IED1_LD0/LLN0$GO$gcb1/0')",
                address
            )));
        }
        let gocb_ref = parts[..parts.len() - 1].join("/");
        let idx: usize = parts[parts.len() - 1]
            .parse()
            .map_err(|_| eneros_core::EnerOSError::Device(format!("Invalid dataset index: {}", parts[parts.len() - 1])))?;
        Ok((gocb_ref, idx))
    }
}

#[async_trait]
impl ProtocolAdapter for GooseAdapter {
    async fn connect(&mut self, _config: &ConnectionConfig) -> Result<()> {
        // GOOSE is connectionless (Layer 2 multicast) — "connect" starts the receive loop
        self.shared_state.set_state(crate::adapter::ConnectionState::Connecting);
        self.start_receive_loop().await;
        self.shared_state.mark_connected();
        tracing::info!("GOOSE adapter '{}' listening on {}", self.name, self.config.interface);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.stop_receive_loop().await;
        self.shared_state.mark_disconnected();
        tracing::info!("GOOSE adapter '{}' stopped", self.name);
        Ok(())
    }

    async fn read(&self, address: &str) -> Result<DataPoint> {
        let (gocb_ref, idx) = Self::parse_address(address)?;
        let cache = self.cache.read().await;
        match cache.get(&gocb_ref) {
            Some(frame) => {
                if idx >= frame.all_data.len() {
                    return Err(eneros_core::EnerOSError::Device(format!(
                        "dataset index {} out of range ({} entries)",
                        idx,
                        frame.all_data.len()
                    )));
                }
                let data = &frame.all_data[idx];
                Ok(DataPoint {
                    address: address.to_string(),
                    value: data.to_data_value(),
                    timestamp: frame.t as i64,
                    quality: DataQuality::Good,
                })
            }
            None => Ok(DataPoint {
                address: address.to_string(),
                value: DataValue::Bool(false),
                timestamp: chrono::Utc::now().timestamp_millis(),
                quality: DataQuality::Bad,
            }),
        }
    }

    async fn write(&mut self, _address: &str, _value: &DataValue) -> Result<()> {
        // GOOSE is publish-subscribe; writes are done via publish()
        Err(eneros_core::EnerOSError::Device(
            "GOOSE does not support direct write; use publish() instead".into(),
        ))
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
        ProtocolType::Goose
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

    fn make_test_frame(appid: u16, gocb_ref: &str, data: Vec<GooseData>) -> GooseFrame {
        GooseFrame {
            appid,
            gocb_ref: gocb_ref.to_string(),
            time_allowed_to_live: 1000,
            dat_set: "IED1_LD0/LLN0$dsGeneric".to_string(),
            go_id: gocb_ref.to_string(),
            t: 1700000000000,
            st_num: 1,
            sq_num: 0,
            simulation: false,
            conf_rev: 1,
            nds_com: false,
            num_dat_set_entries: data.len() as u32,
            all_data: data,
        }
    }

    #[test]
    fn test_frame_serialize_parse_roundtrip() {
        let frame = make_test_frame(
            0x0001,
            "IED1_LD0/LLN0$GO$gcb1",
            vec![
                GooseData::Bool(true),
                GooseData::Int(42),
                GooseData::Float(220.5),
            ],
        );
        let src_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let bytes = frame.serialize(&src_mac);
        assert!(bytes.len() > 14);

        let parsed = GooseFrame::parse(&bytes).expect("parse should succeed");
        assert_eq!(parsed.appid, 0x0001);
        assert_eq!(parsed.gocb_ref, "IED1_LD0/LLN0$GO$gcb1");
        assert_eq!(parsed.dat_set, "IED1_LD0/LLN0$dsGeneric");
        assert_eq!(parsed.st_num, 1);
        assert_eq!(parsed.all_data.len(), 3);
        assert_eq!(parsed.all_data[0], GooseData::Bool(true));
        assert_eq!(parsed.all_data[1], GooseData::Int(42));
    }

    #[test]
    fn test_parse_wrong_ethertype() {
        let mut frame = vec![0u8; 20];
        frame[12] = 0x08;
        frame[13] = 0x00; // IPv4
        let result = GooseFrame::parse(&frame);
        assert!(matches!(result, Err(GooseParseError::WrongEtherType(0x0800))));
    }

    #[test]
    fn test_parse_too_short() {
        let result = GooseFrame::parse(&[0u8; 5]);
        assert!(matches!(result, Err(GooseParseError::TooShort)));
    }

    #[test]
    fn test_parse_empty_pdu() {
        let mut frame = vec![0u8; 22];
        // Ethernet header
        frame[12] = (GOOSE_ETHERTYPE >> 8) as u8;
        frame[13] = GOOSE_ETHERTYPE as u8;
        // GOOSE header
        frame[14] = 0x00;
        frame[15] = 0x01; // APPID
        frame[16] = 0x00;
        frame[17] = 0x08; // Length = 8
        // No PDU
        let result = GooseFrame::parse(&frame);
        assert!(result.is_err());
    }

    #[test]
    fn test_goose_data_to_data_value() {
        assert_eq!(GooseData::Bool(true).to_data_value(), DataValue::Bool(true));
        assert_eq!(GooseData::Int(42).to_data_value(), DataValue::Int64(42));
        assert_eq!(GooseData::Float(1.5).to_data_value(), DataValue::Float64(1.5));
        assert_eq!(
            GooseData::String("hello".into()).to_data_value(),
            DataValue::String("hello".into())
        );
        assert_eq!(
            GooseData::Bytes(vec![1, 2, 3]).to_data_value(),
            DataValue::Bytes(vec![1, 2, 3])
        );
    }

    #[test]
    fn test_parse_address() {
        let (gocb, idx) = GooseAdapter::parse_address("IED1_LD0/LLN0$GO$gcb1/2").unwrap();
        assert_eq!(gocb, "IED1_LD0/LLN0$GO$gcb1");
        assert_eq!(idx, 2);
    }

    #[test]
    fn test_parse_address_invalid() {
        assert!(GooseAdapter::parse_address("noindex").is_err());
        assert!(GooseAdapter::parse_address("ref/abc").is_err());
    }

    #[tokio::test]
    async fn test_mock_adapter_read_cached() {
        let (adapter, _sender) = GooseAdapter::new_mock("test-goose");
        adapter.shared_state.mark_connected();

        let frame = make_test_frame(
            1,
            "gcb1",
            vec![GooseData::Bool(true), GooseData::Float(110.0)],
        );
        adapter.inject_frame(frame).await;

        let dp = adapter.read("gcb1/0").await.unwrap();
        assert_eq!(dp.value, DataValue::Bool(true));
        assert_eq!(dp.quality, DataQuality::Good);

        let dp2 = adapter.read("gcb1/1").await.unwrap();
        assert_eq!(dp2.value, DataValue::Float64(110.0));
    }

    #[tokio::test]
    async fn test_mock_adapter_read_missing() {
        let (adapter, _sender) = GooseAdapter::new_mock("test-goose");
        adapter.shared_state.mark_connected();

        let dp = adapter.read("missing/0").await.unwrap();
        assert_eq!(dp.quality, DataQuality::Bad);
    }

    #[tokio::test]
    async fn test_mock_adapter_read_out_of_range() {
        let (adapter, _sender) = GooseAdapter::new_mock("test-goose");
        adapter.shared_state.mark_connected();

        let frame = make_test_frame(1, "gcb1", vec![GooseData::Bool(true)]);
        adapter.inject_frame(frame).await;

        let result = adapter.read("gcb1/5").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mock_adapter_subscribe() {
        let (mut adapter, _sender) = GooseAdapter::new_mock("test-goose");
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_clone = received.clone();

        adapter
            .subscribe(vec![], Box::new(move |dp| {
                received_clone.try_lock().unwrap().push(dp);
            }))
            .await
            .unwrap();

        let frame = make_test_frame(1, "gcb1", vec![GooseData::Bool(true)]);
        adapter.inject_frame(frame).await;

        let msgs = received.lock().await;
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].value, DataValue::Bool(true));
    }

    #[tokio::test]
    async fn test_mock_adapter_publish() {
        let (adapter, _sender) = GooseAdapter::new_mock("test-goose");
        let frame = make_test_frame(1, "gcb1", vec![GooseData::Bool(true)]);
        let src_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];

        adapter.publish(&frame, &src_mac).await.unwrap();

        // Verify frame was sent (record_sent was called)
        let stats = adapter.statistics();
        assert_eq!(stats.messages_sent, 1);
    }

    #[tokio::test]
    async fn test_mock_adapter_write_not_supported() {
        let (mut adapter, _sender) = GooseAdapter::new_mock("test-goose");
        let result = adapter.write("gcb1/0", &DataValue::Bool(true)).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_adapter_creation() {
        let (adapter, _) = GooseAdapter::new_mock("test-goose");
        assert_eq!(adapter.name(), "test-goose");
        assert_eq!(adapter.protocol_type(), ProtocolType::Goose);
        assert!(!adapter.is_connected());
    }

    #[test]
    fn test_multicast_mac_prefix() {
        assert_eq!(GOOSE_MULTICAST_PREFIX, [0x01, 0x0C, 0xCD, 0x01, 0x00]);
    }

    #[test]
    fn test_ber_encode_decode_integer_positive() {
        let mut buf = Vec::new();
        encode_integer(&mut buf, 0x81, 1000);
        let mut parser = BerParser::new(&buf);
        let (tag, content) = parser.read_tlv().unwrap();
        assert_eq!(tag, 0x81);
        assert_eq!(decode_integer(content).unwrap(), 1000);
    }

    #[test]
    fn test_ber_encode_decode_integer_zero() {
        let mut buf = Vec::new();
        encode_integer(&mut buf, 0x81, 0);
        let mut parser = BerParser::new(&buf);
        let (_tag, content) = parser.read_tlv().unwrap();
        assert_eq!(decode_integer(content).unwrap(), 0);
    }

    #[test]
    fn test_ber_encode_decode_string() {
        let mut buf = Vec::new();
        encode_visible_string(&mut buf, 0x80, "hello world");
        let mut parser = BerParser::new(&buf);
        let (tag, content) = parser.read_tlv().unwrap();
        assert_eq!(tag, 0x80);
        assert_eq!(decode_visible_string(content).unwrap(), "hello world");
    }

    #[test]
    fn test_ber_encode_decode_boolean() {
        let mut buf = Vec::new();
        encode_boolean(&mut buf, 0x87, true);
        let mut parser = BerParser::new(&buf);
        let (_tag, content) = parser.read_tlv().unwrap();
        assert!(decode_boolean(content));

        let mut buf2 = Vec::new();
        encode_boolean(&mut buf2, 0x87, false);
        let mut parser2 = BerParser::new(&buf2);
        let (_tag, content) = parser2.read_tlv().unwrap();
        assert!(!decode_boolean(content));
    }

    #[tokio::test]
    async fn test_cached_refs() {
        let (adapter, _) = GooseAdapter::new_mock("test");
        adapter.shared_state.mark_connected();

        adapter
            .inject_frame(make_test_frame(1, "gcb1", vec![GooseData::Bool(true)]))
            .await;
        adapter
            .inject_frame(make_test_frame(2, "gcb2", vec![GooseData::Bool(false)]))
            .await;

        let refs = adapter.cached_refs().await;
        assert_eq!(refs.len(), 2);
        assert!(refs.contains(&"gcb1".to_string()));
        assert!(refs.contains(&"gcb2".to_string()));
    }

    #[tokio::test]
    async fn test_get_latest_frame() {
        let (adapter, _) = GooseAdapter::new_mock("test");
        adapter.shared_state.mark_connected();

        let frame = make_test_frame(1, "gcb1", vec![GooseData::Bool(true)]);
        adapter.inject_frame(frame).await;

        let latest = adapter.get_latest_frame("gcb1").await.unwrap();
        assert_eq!(latest.appid, 1);
        assert_eq!(latest.gocb_ref, "gcb1");
    }

    // ========================================================================
    // AF_PACKET 集成测试（Task 2）
    // ========================================================================

    /// 非 Linux 平台：`with_af_packet` 应返回 `Unsupported` 错误。
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn test_with_af_packet_unsupported_on_nonlinux() {
        let cfg = AfPacketConfig::for_goose("eth0", [0; 6]);
        let result = GooseAdapter::with_af_packet("test", GooseConfig::default(), cfg);
        assert!(
            matches!(result, Err(AfPacketAdapterError::Unsupported(_))),
            "非 Linux 平台应返回 Unsupported 错误"
        );
    }

    /// Linux 平台：使用不存在的网卡名应返回错误。
    #[cfg(target_os = "linux")]
    #[test]
    fn test_with_af_packet_nonexistent_interface() {
        let cfg = AfPacketConfig::for_goose("nonexistent_iface_xyz_12345", [0; 6]);
        let result = GooseAdapter::with_af_packet("test", GooseConfig::default(), cfg);
        assert!(result.is_err(), "不存在的网卡应返回错误");
    }

    /// GOOSE PDU 编码 → transport.send → transport.recv → GOOSE PDU 解码 往返测试。
    #[tokio::test]
    async fn test_goose_pdu_roundtrip_through_transport() {
        let (transport, sender) = MockGooseTransport::new();

        let frame = GooseFrame {
            appid: 0x0001,
            gocb_ref: "IED1_LD0/LLN0$GO$gcb1".into(),
            time_allowed_to_live: 1000,
            dat_set: "IED1_LD0/LLN0$dsGeneric".into(),
            go_id: "goID-test".into(),
            t: 1700000000000,
            st_num: 1,
            sq_num: 0,
            simulation: false,
            conf_rev: 1,
            nds_com: false,
            num_dat_set_entries: 3,
            all_data: vec![
                GooseData::Bool(true),
                GooseData::Int(42),
                GooseData::Float(220.5),
            ],
        };
        let src_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];

        // 1. 编码：序列化为完整以太网帧
        let bytes = frame.serialize(&src_mac);
        assert!(bytes.len() > 14, "序列化后的帧应包含以太网头");

        // 2. transport.send
        transport.send(&bytes).await.expect("send should succeed");

        // 3. 取回已发送的帧，注入到接收通道模拟回环
        let sent = transport.sent_frames().await;
        assert_eq!(sent.len(), 1, "应记录 1 帧已发送");
        sender.send(sent[0].clone()).await.unwrap();

        // 4. transport.recv
        let received = transport.receive().await.expect("receive should succeed");
        assert_eq!(received, bytes, "接收到的帧应与发送的帧一致");

        // 5. 解码：解析回 GooseFrame
        let parsed = GooseFrame::parse(&received).expect("parse should succeed");
        assert_eq!(parsed.appid, frame.appid);
        assert_eq!(parsed.gocb_ref, frame.gocb_ref);
        assert_eq!(parsed.dat_set, frame.dat_set);
        assert_eq!(parsed.go_id, frame.go_id);
        assert_eq!(parsed.st_num, frame.st_num);
        assert_eq!(parsed.sq_num, frame.sq_num);
        assert_eq!(parsed.conf_rev, frame.conf_rev);
        assert_eq!(parsed.all_data.len(), frame.all_data.len());
        assert_eq!(parsed.all_data[0], GooseData::Bool(true));
        assert_eq!(parsed.all_data[1], GooseData::Int(42));
    }

    /// GooseAdapter 完整收发流程测试：
    /// publish → 注入回接收通道 → receive loop 解析 → cache 命中 → read 读取。
    #[tokio::test]
    async fn test_adapter_complete_send_receive_flow() {
        let (adapter, sender) = GooseAdapter::new_mock("test-flow");
        adapter.shared_state.mark_connected();

        adapter.start_receive_loop().await;

        let frame = GooseFrame {
            appid: 0x0001,
            gocb_ref: "flow_test_gcb".into(),
            time_allowed_to_live: 1000,
            dat_set: "IED1_LD0/LLN0$dsGeneric".into(),
            go_id: String::new(),
            t: 1700000000000,
            st_num: 1,
            sq_num: 0,
            simulation: false,
            conf_rev: 1,
            nds_com: false,
            num_dat_set_entries: 2,
            all_data: vec![GooseData::Bool(true), GooseData::Int(99)],
        };
        let src_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];

        // 1. 通过 adapter.publish 发送
        adapter.publish(&frame, &src_mac).await.expect("publish should succeed");

        let stats = adapter.statistics();
        assert_eq!(stats.messages_sent, 1, "应记录 1 帧已发送");

        // 2. 将序列化后的帧注入到接收通道（模拟网络回环）
        let sent_bytes = frame.serialize(&src_mac);
        sender.send(sent_bytes).await.unwrap();

        // 3. 等待接收循环处理
        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

        // 4. 验证 cache 中有该帧
        let latest = adapter.get_latest_frame("flow_test_gcb").await;
        assert!(latest.is_some(), "帧应已进入缓存");
        let latest = latest.unwrap();
        assert_eq!(latest.appid, 0x0001);
        assert_eq!(latest.gocb_ref, "flow_test_gcb");
        assert_eq!(latest.all_data.len(), 2);
        assert_eq!(latest.all_data[0], GooseData::Bool(true));
        assert_eq!(latest.all_data[1], GooseData::Int(99));

        // 5. 通过 read 读取数据点
        let dp0 = adapter.read("flow_test_gcb/0").await.unwrap();
        assert_eq!(dp0.value, DataValue::Bool(true));
        assert_eq!(dp0.quality, DataQuality::Good);

        let dp1 = adapter.read("flow_test_gcb/1").await.unwrap();
        assert_eq!(dp1.value, DataValue::Int64(99));
        assert_eq!(dp1.quality, DataQuality::Good);

        // 6. 验证接收统计
        let stats = adapter.statistics();
        assert_eq!(stats.messages_received, 1, "应记录 1 帧已接收");

        adapter.stop_receive_loop().await;
    }

    /// 验证 `with_transport` 可接受任意 `GooseTransport` 实现。
    #[tokio::test]
    async fn test_with_transport_custom_implementation() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingTransport {
            send_count: Arc<AtomicUsize>,
            recv_count: Arc<AtomicUsize>,
            pending: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Vec<u8>>>,
        }

        #[async_trait]
        impl GooseTransport for CountingTransport {
            async fn receive(&self) -> std::result::Result<Vec<u8>, String> {
                self.recv_count.fetch_add(1, Ordering::SeqCst);
                self.pending
                    .lock()
                    .await
                    .recv()
                    .await
                    .ok_or_else(|| "closed".into())
            }
            async fn send(&self, frame: &[u8]) -> std::result::Result<(), String> {
                self.send_count.fetch_add(1, Ordering::SeqCst);
                assert!(frame.len() > 14, "发送的帧应包含以太网头");
                Ok(())
            }
        }

        let send_count = Arc::new(AtomicUsize::new(0));
        let recv_count = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = tokio::sync::mpsc::channel(8);

        let transport = CountingTransport {
            send_count: send_count.clone(),
            recv_count: recv_count.clone(),
            pending: tokio::sync::Mutex::new(rx),
        };

        let adapter = GooseAdapter::with_transport(
            "counting-test",
            GooseConfig::default(),
            Box::new(transport),
        );

        let frame = make_test_frame(1, "custom_gcb", vec![GooseData::Bool(true)]);
        adapter.publish(&frame, &[0; 6]).await.unwrap();
        assert_eq!(send_count.load(Ordering::SeqCst), 1, "send 应被调用 1 次");

        let bytes = frame.serialize(&[0; 6]);
        tx.send(bytes).await.unwrap();

        adapter.start_receive_loop().await;
        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
        adapter.stop_receive_loop().await;

        assert!(recv_count.load(Ordering::SeqCst) >= 1, "receive 应被调用至少 1 次");

        let latest = adapter.get_latest_frame("custom_gcb").await;
        assert!(latest.is_some(), "帧应已进入缓存");
    }

    // ========================================================================
    // 新增测试：修复验证（C4 / H14 / M15）
    // ========================================================================

    /// C4: BIT STRING content 长度为 1 时不 panic，返回 Quality(0)。
    #[test]
    fn test_parse_bit_string_content_length_1_no_panic() {
        // 构造一个 allData 序列，包含一个 BIT STRING，content 仅 1 字节（unused bits）
        // tag=0x03, length=0x01, content=[0x00]
        let all_data_bytes = vec![0x03, 0x01, 0x00];
        let result = parse_all_data(&all_data_bytes);
        assert!(result.is_ok(), "BIT STRING content len=1 不应 panic");
        let data = result.unwrap();
        assert_eq!(data.len(), 1);
        assert_eq!(data[0], GooseData::Quality(0));
    }

    /// C4: BIT STRING content 长度为 0 时不 panic，返回 Quality(0)。
    #[test]
    fn test_parse_bit_string_content_length_0_no_panic() {
        // tag=0x03, length=0x00 (空 content)
        let all_data_bytes = vec![0x03, 0x00];
        let result = parse_all_data(&all_data_bytes);
        assert!(result.is_ok(), "BIT STRING content len=0 不应 panic");
        let data = result.unwrap();
        assert_eq!(data.len(), 1);
        assert_eq!(data[0], GooseData::Quality(0));
    }

    /// C4: BIT STRING content 长度 >= 2 时正常解析 Quality 值。
    #[test]
    fn test_parse_bit_string_content_length_2_normal() {
        // tag=0x03, length=0x02, content=[0x00, 0x0A] → Quality(0x0A)
        let all_data_bytes = vec![0x03, 0x02, 0x00, 0x0A];
        let result = parse_all_data(&all_data_bytes);
        assert!(result.is_ok());
        let data = result.unwrap();
        assert_eq!(data.len(), 1);
        assert_eq!(data[0], GooseData::Quality(0x0A));
    }

    /// M15: GOOSE length 字段 < 8 应返回 HeaderTooShort 错误。
    #[test]
    fn test_parse_length_field_too_short() {
        let mut frame = vec![0u8; 22];
        // Ethernet header
        frame[12] = (GOOSE_ETHERTYPE >> 8) as u8;
        frame[13] = GOOSE_ETHERTYPE as u8;
        // GOOSE header
        frame[14] = 0x00;
        frame[15] = 0x01; // APPID
        frame[16] = 0x00;
        frame[17] = 0x07; // Length = 7 (< 8，应被拒绝)
                           // Reserved 4 bytes at [18..22]
        let result = GooseFrame::parse(&frame);
        assert!(
            matches!(result, Err(GooseParseError::HeaderTooShort)),
            "length < 8 应返回 HeaderTooShort"
        );
    }

    /// H14: MockGooseTransport::default() 的 receive() 不应立即返回错误（通道保持开启）。
    #[tokio::test]
    async fn test_mock_transport_default_channel_not_closed() {
        let transport = MockGooseTransport::default();
        // 通道应保持开启：receive() 应阻塞等待，而非立即返回 "channel closed"
        let result = tokio::time::timeout(
            tokio::time::Duration::from_millis(100),
            transport.receive(),
        )
        .await;
        // 超时意味着 receive() 在阻塞等待（通道未关闭）—— 这是预期行为
        assert!(
            result.is_err(),
            "receive() 应阻塞等待（超时），而非立即返回错误"
        );
    }
}
