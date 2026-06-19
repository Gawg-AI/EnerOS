//! SV (Sampled Values) protocol adapter — IEC 61850-9-2 LE.
//!
//! SV transmits digitized voltage/current samples from merging units (MUs)
//! to IEDs at Layer 2 (EtherType 0x88BA). Unlike GOOSE which carries
//! state events, SV carries continuous waveform samples at 4000 Hz
//! (IEC 61850-9-2LE) or configurable rates.
//!
//! # Sample Format
//!
//! Each SV frame contains an ASDU sequence with:
//! - `svID` — dataset identifier
//! - `smpCnt` — sample counter (wraps at smpRate)
//! - `confRev` — configuration revision
//! - `seqData` — array of instantaneous values (iA, iB, iC, uA, uB, uC, ...)
//!
//! # Address Format
//!
//! `svID/channel_index` (e.g., "MU01/0" for first channel = iA)

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
use crate::adapters::goose::{GooseTransport, MockGooseTransport};

/// SV EtherType (IEEE 802.3)
pub const SV_ETHERTYPE: u16 = 0x88BA;

/// Default SV multicast MAC prefix (IEC 61850-9-2)
/// 01-0C-CD-04-00-00 .. 01-0C-CD-04-00-3F
pub const SV_MULTICAST_PREFIX: [u8; 5] = [0x01, 0x0C, 0xCD, 0x04, 0x00];

/// IEC 61850-9-2LE default sample rate (4000 Hz)
pub const SV_DEFAULT_SAMPLE_RATE: u32 = 4000;

/// Number of samples per nominal cycle (50Hz → 80, 60Hz → 67)
pub const SV_SAMPLES_PER_CYCLE_50HZ: u32 = 80;

/// SV configuration
#[derive(Debug, Clone)]
pub struct SvConfig {
    /// Interface name for pcap backend
    pub interface: String,
    /// APPID filter (0 = accept all)
    pub appid_filter: u16,
    /// svID filter (empty = accept all)
    pub sv_id_filter: String,
    /// Expected sample rate (Hz)
    pub sample_rate: u32,
    /// Number of channels per ASDU (typically 8: 4 current + 4 voltage)
    pub no_asdu: u8,
}

impl Default for SvConfig {
    fn default() -> Self {
        Self {
            interface: "any".to_string(),
            appid_filter: 0,
            sv_id_filter: String::new(),
            sample_rate: SV_DEFAULT_SAMPLE_RATE,
            no_asdu: 8,
        }
    }
}

/// Parsed SV frame (IEC 61850-9-2).
#[derive(Debug, Clone)]
pub struct SvFrame {
    /// APPID from the SV header
    pub appid: u16,
    /// SV dataset identifier (e.g., "MU01")
    pub sv_id: String,
    /// Sample counter — increments per sample, wraps at sample_rate
    pub smp_cnt: u32,
    /// Configuration revision
    pub conf_rev: u32,
    /// RefrTm (optional timestamp)
    pub refr_tm: Option<u64>,
    /// Sample rate (smpRate)
    pub smp_rate: u32,
    /// Sequence data — instantaneous values (typically iA, iB, iC, iN, uA, uB, uC, uN)
    pub seq_data: Vec<i16>,
}

impl SvFrame {
    /// Parse a raw Ethernet frame into an SvFrame.
    pub fn parse(eth_frame: &[u8]) -> std::result::Result<Self, SvParseError> {
        if eth_frame.len() < 14 {
            return Err(SvParseError::TooShort);
        }
        let ethertype = u16::from_be_bytes([eth_frame[12], eth_frame[13]]);
        if ethertype != SV_ETHERTYPE {
            return Err(SvParseError::WrongEtherType(ethertype));
        }

        let payload = &eth_frame[14..];
        if payload.len() < 8 {
            return Err(SvParseError::HeaderTooShort);
        }
        let appid = u16::from_be_bytes([payload[0], payload[1]]);
        let length = u16::from_be_bytes([payload[2], payload[3]]) as usize;
        let pdu = &payload[8..];
        if pdu.len() < length.saturating_sub(8) {
            return Err(SvParseError::LengthMismatch);
        }

        let parser = SvBerParser::new(pdu);
        parser.parse_sv_pdu(appid)
    }

    /// Serialize this SvFrame into a raw Ethernet frame.
    pub fn serialize(&self, src_mac: &[u8; 6]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(64 + self.seq_data.len() * 2);
        // Ethernet header
        buf.extend_from_slice(&SV_MULTICAST_PREFIX);
        buf.push(0x00);
        buf.extend_from_slice(src_mac);
        buf.extend_from_slice(&SV_ETHERTYPE.to_be_bytes());
        // SV header
        buf.extend_from_slice(&self.appid.to_be_bytes());
        let length_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        // SV PDU
        let pdu_start = buf.len();
        self.encode_ber(&mut buf);
        let pdu_len = buf.len() - pdu_start;
        let total_len = 8 + pdu_len;
        buf[length_pos..length_pos + 2]
            .copy_from_slice(&(total_len as u16).to_be_bytes());
        buf
    }

    fn encode_ber(&self, buf: &mut Vec<u8>) {
        let mut inner = Vec::new();
        // noASDU [0] INTEGER (typically 1)
        encode_int_tlv(&mut inner, 0x80, 1);
        // seqASDU [1] SEQUENCE OF ASDU
        let mut asdu_seq = Vec::new();
        // Single ASDU
        let mut asdu = Vec::new();
        // svID [0xA0] VisibleString (context tag 0, constructed)
        encode_string_tlv(&mut asdu, 0x80, &self.sv_id);
        // datSet [0xA1] (optional, skip)
        // smpCnt [0x82] INTEGER
        encode_int_tlv(&mut asdu, 0x82, self.smp_cnt as i64);
        // confRev [0x83] INTEGER
        encode_int_tlv(&mut asdu, 0x83, self.conf_rev as i64);
        // refrTm [0x84] UtcTime (optional, skip)
        // smpRate [0x85] INTEGER
        encode_int_tlv(&mut asdu, 0x85, self.smp_rate as i64);
        // sample [0x86] SEQUENCE
        let mut sample_seq = Vec::new();
        for &val in &self.seq_data {
            // Each value is INTEGER (universal tag 0x02)
            encode_int_tlv(&mut sample_seq, 0x87, val as i64);
        }
        encode_constructed(&mut asdu, 0xA6, &sample_seq);
        encode_constructed(&mut asdu_seq, 0x30, &asdu); // ASDU as SEQUENCE
        encode_constructed(&mut inner, 0xA1, &asdu_seq);
        // Wrap in SEQUENCE
        encode_constructed(buf, 0x60, &inner);
    }

    /// Convert a channel value to engineering units.
    ///
    /// IEC 61850-9-2LE defines nominal values:
    /// - Current: 1 A (primary) → ADC count depends on range
    /// - Voltage: 1 V (primary) → ADC count depends on range
    ///
    /// Typical scaling: nominal = 4000 counts for rated value
    pub fn to_engineering(_channel_idx: usize, raw: i16, nominal_primary: f64) -> f64 {
        let adc_nominal = 4000.0; // IEC 61850-9-2LE convention
        (raw as f64 / adc_nominal) * nominal_primary
    }
}

/// SV parse error
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SvParseError {
    TooShort,
    HeaderTooShort,
    WrongEtherType(u16),
    LengthMismatch,
    BerError(String),
    MissingField(&'static str),
}

impl std::fmt::Display for SvParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort => write!(f, "frame too short"),
            Self::HeaderTooShort => write!(f, "SV header too short"),
            Self::WrongEtherType(et) => write!(f, "wrong EtherType 0x{:04X}, expected 0x{:04X}", et, SV_ETHERTYPE),
            Self::LengthMismatch => write!(f, "length mismatch"),
            Self::BerError(msg) => write!(f, "BER decode error: {}", msg),
            Self::MissingField(name) => write!(f, "missing field: {}", name),
        }
    }
}

impl std::error::Error for SvParseError {}

/// BER parser specialized for SV PDU.
struct SvBerParser<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> SvBerParser<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn parse_sv_pdu(mut self, appid: u16) -> std::result::Result<SvFrame, SvParseError> {
        let (tag, content) = self.read_tlv()?;
        if tag != 0x60 {
            return Err(SvParseError::BerError(format!(
                "expected SEQUENCE 0x60, got 0x{:02X}",
                tag
            )));
        }

        let mut parser = SvBerParser::new(content);
        let mut frame = SvFrame {
            appid,
            sv_id: String::new(),
            smp_cnt: 0,
            conf_rev: 0,
            refr_tm: None,
            smp_rate: SV_DEFAULT_SAMPLE_RATE,
            seq_data: Vec::new(),
        };

        // noASDU [0]
        let (tag, _content) = parser.read_tlv()?;
        if tag != 0x80 {
            return Err(SvParseError::BerError(format!(
                "expected noASDU tag 0x80, got 0x{:02X}",
                tag
            )));
        }

        // seqASDU [1] — SEQUENCE OF ASDU
        let (tag, asdu_content) = parser.read_tlv()?;
        if tag != 0xA1 {
            return Err(SvParseError::BerError(format!(
                "expected seqASDU tag 0xA1, got 0x{:02X}",
                tag
            )));
        }

        // Parse first ASDU (SEQUENCE)
        let mut asdu_parser = SvBerParser::new(asdu_content);
        let (_seq_tag, asdu_inner) = asdu_parser.read_tlv()?;
        let mut asdu_parser = SvBerParser::new(asdu_inner);

        while asdu_parser.pos < asdu_parser.data.len() {
            let (tag, content) = asdu_parser.read_tlv()?;
            match tag {
                0x80 => frame.sv_id = String::from_utf8_lossy(content).into_owned(),
                0x82 => frame.smp_cnt = decode_ber_int(content)? as u32,
                0x83 => frame.conf_rev = decode_ber_int(content)? as u32,
                0x84 => frame.refr_tm = Some(decode_ber_utc(content)?),
                0x85 => frame.smp_rate = decode_ber_int(content)? as u32,
                0xA6 => {
                    // sample SEQUENCE — contains channel values
                    let mut sample_parser = SvBerParser::new(content);
                    while sample_parser.pos < sample_parser.data.len() {
                        let (stag, scontent) = sample_parser.read_tlv()?;
                        if stag == 0x87 {
                            frame.seq_data.push(decode_ber_int(scontent)? as i16);
                        }
                    }
                }
                _ => {}
            }
        }

        if frame.sv_id.is_empty() {
            return Err(SvParseError::MissingField("svID"));
        }

        Ok(frame)
    }

    fn read_tlv(&mut self) -> std::result::Result<(u8, &'a [u8]), SvParseError> {
        if self.pos + 2 > self.data.len() {
            return Err(SvParseError::BerError("unexpected end of data".into()));
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
                return Err(SvParseError::BerError(format!(
                    "unsupported length: {} bytes",
                    num_bytes
                )));
            }
            if self.pos + num_bytes > self.data.len() {
                return Err(SvParseError::BerError("length truncated".into()));
            }
            let mut len = 0usize;
            for i in 0..num_bytes {
                len = (len << 8) | self.data[self.pos + i] as usize;
            }
            self.pos += num_bytes;
            len
        };

        if self.pos + len > self.data.len() {
            return Err(SvParseError::BerError(format!(
                "content {} exceeds remaining {}",
                len,
                self.data.len() - self.pos
            )));
        }
        let content = &self.data[self.pos..self.pos + len];
        self.pos += len;
        Ok((tag, content))
    }
}

fn decode_ber_int(data: &[u8]) -> std::result::Result<i64, SvParseError> {
    if data.is_empty() {
        return Ok(0);
    }
    let mut val = if data[0] & 0x80 != 0 { -1i64 } else { 0i64 };
    for &b in data {
        val = (val << 8) | (b as i64 & 0xFF);
    }
    Ok(val)
}

fn decode_ber_utc(data: &[u8]) -> std::result::Result<u64, SvParseError> {
    if data.len() < 4 {
        return Err(SvParseError::BerError("UtcTime too short".into()));
    }
    let secs = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as u64;
    Ok(secs * 1000)
}

fn encode_int_tlv(buf: &mut Vec<u8>, tag: u8, val: i64) {
    let bytes = if val == 0 {
        vec![0u8]
    } else if val > 0 {
        let mut v = val as u64;
        let mut bytes = Vec::new();
        while v > 0 {
            bytes.push((v & 0xFF) as u8);
            v >>= 8;
        }
        if bytes.last().map_or(false, |&b| b & 0x80 != 0) {
            bytes.push(0);
        }
        bytes.reverse();
        bytes
    } else {
        let mut bytes = Vec::new();
        let mut v = val;
        while v != -1 || bytes.is_empty() {
            bytes.push((v & 0xFF) as u8);
            v >>= 8;
            if v == -1 && bytes.last().map_or(false, |&b| b & 0x80 != 0) {
                break;
            }
        }
        bytes.reverse();
        bytes
    };
    encode_tlv_raw(buf, tag, &bytes);
}

fn encode_string_tlv(buf: &mut Vec<u8>, tag: u8, s: &str) {
    encode_tlv_raw(buf, tag, s.as_bytes());
}

fn encode_constructed(buf: &mut Vec<u8>, tag: u8, content: &[u8]) {
    encode_tlv_raw(buf, tag, content);
}

fn encode_tlv_raw(buf: &mut Vec<u8>, tag: u8, content: &[u8]) {
    buf.push(tag);
    if content.len() < 0x80 {
        buf.push(content.len() as u8);
    } else if content.len() <= 0xFF {
        buf.push(0x81);
        buf.push(content.len() as u8);
    } else {
        buf.push(0x82);
        buf.extend_from_slice(&(content.len() as u16).to_be_bytes());
    }
    buf.extend_from_slice(content);
}

/// SV protocol adapter.
///
/// Subscribes to SV multicast frames, parses them, and exposes
/// instantaneous sample values via the `ProtocolAdapter` trait.
pub struct SvAdapter {
    transport: Arc<Mutex<Box<dyn GooseTransport>>>,
    shared_state: SharedState,
    name: String,
    config: SvConfig,
    /// Ring buffer of recent samples per svID
    cache: Arc<RwLock<HashMap<String, SvFrame>>>,
    /// Callbacks
    callbacks: Arc<RwLock<Vec<Box<dyn Fn(DataPoint) + Send + Sync>>>>,
    recv_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl SvAdapter {
    /// Create a new SV adapter with the given transport.
    pub fn with_transport(name: &str, config: SvConfig, transport: Box<dyn GooseTransport>) -> Self {
        Self {
            transport: Arc::new(Mutex::new(transport)),
            shared_state: new_shared_state(),
            name: name.to_string(),
            config,
            cache: Arc::new(RwLock::new(HashMap::new())),
            callbacks: Arc::new(RwLock::new(Vec::new())),
            recv_handle: Arc::new(Mutex::new(None)),
        }
    }

    /// Create an SV adapter with mock transport (for testing).
    pub fn new_mock(name: &str) -> (Self, tokio::sync::mpsc::Sender<Vec<u8>>) {
        let (transport, sender) = MockGooseTransport::new();
        let adapter = Self::with_transport(name, SvConfig::default(), Box::new(transport));
        (adapter, sender)
    }

    /// Get the latest cached frame for a given svID.
    pub async fn get_latest_frame(&self, sv_id: &str) -> Option<SvFrame> {
        self.cache.read().await.get(sv_id).cloned()
    }

    /// Inject a parsed frame directly (for testing).
    pub async fn inject_frame(&self, frame: SvFrame) {
        let sv_id = frame.sv_id.clone();
        let seq_data = frame.seq_data.clone();
        self.cache.write().await.insert(sv_id.clone(), frame);

        let callbacks = self.callbacks.read().await;
        for (idx, val) in seq_data.iter().enumerate() {
            let dp = DataPoint {
                address: format!("{}/{}", sv_id, idx),
                value: DataValue::Int16(*val),
                timestamp: chrono::Utc::now().timestamp_millis(),
                quality: DataQuality::Good,
            };
            for cb in callbacks.iter() {
                cb(dp.clone());
            }
        }
    }

    /// Start the background receive loop.
    pub async fn start_receive_loop(&self) {
        let transport = self.transport.clone();
        let cache = self.cache.clone();
        let callbacks = self.callbacks.clone();
        let shared = self.shared_state.clone();
        let appid_filter = self.config.appid_filter;
        let sv_filter = self.config.sv_id_filter.clone();

        let handle = tokio::spawn(async move {
            loop {
                let frame_bytes = {
                    let mut t = transport.lock().await;
                    match t.receive().await {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            tracing::warn!("SV receive error: {}", e);
                            break;
                        }
                    }
                };

                match SvFrame::parse(&frame_bytes) {
                    Ok(sv) => {
                        if appid_filter != 0 && sv.appid != appid_filter {
                            continue;
                        }
                        if !sv_filter.is_empty() && sv.sv_id != sv_filter {
                            continue;
                        }

                        shared.record_received(frame_bytes.len() as u64);
                        let sv_id = sv.sv_id.clone();
                        let seq_data = sv.seq_data.clone();

                        cache.write().await.insert(sv_id.clone(), sv);

                        let cbs = callbacks.read().await;
                        for (idx, val) in seq_data.iter().enumerate() {
                            let dp = DataPoint {
                                address: format!("{}/{}", sv_id, idx),
                                value: DataValue::Int16(*val),
                                timestamp: chrono::Utc::now().timestamp_millis(),
                                quality: DataQuality::Good,
                            };
                            for cb in cbs.iter() {
                                cb(dp.clone());
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("SV parse error: {}", e);
                    }
                }
            }
            tracing::info!("SV receive loop stopped");
        });

        *self.recv_handle.lock().await = Some(handle);
    }

    /// Stop the receive loop.
    pub async fn stop_receive_loop(&self) {
        if let Some(h) = self.recv_handle.lock().await.take() {
            h.abort();
        }
    }

    fn parse_address(address: &str) -> Result<(String, usize)> {
        let parts: Vec<&str> = address.split('/').collect();
        if parts.len() != 2 {
            return Err(eneros_core::EnerOSError::Device(format!(
                "Invalid SV address '{}', expected 'svID/channel_index' (e.g., 'MU01/0')",
                address
            )));
        }
        let sv_id = parts[0].to_string();
        let idx: usize = parts[1]
            .parse()
            .map_err(|_| eneros_core::EnerOSError::Device(format!("Invalid channel index: {}", parts[1])))?;
        Ok((sv_id, idx))
    }
}

#[async_trait]
impl ProtocolAdapter for SvAdapter {
    async fn connect(&mut self, _config: &ConnectionConfig) -> Result<()> {
        self.shared_state.set_state(crate::adapter::ConnectionState::Connecting);
        self.start_receive_loop().await;
        self.shared_state.mark_connected();
        tracing::info!("SV adapter '{}' listening on {}", self.name, self.config.interface);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.stop_receive_loop().await;
        self.shared_state.mark_disconnected();
        Ok(())
    }

    async fn read(&self, address: &str) -> Result<DataPoint> {
        let (sv_id, idx) = Self::parse_address(address)?;
        let cache = self.cache.read().await;
        match cache.get(&sv_id) {
            Some(frame) => {
                if idx >= frame.seq_data.len() {
                    return Err(eneros_core::EnerOSError::Device(format!(
                        "channel index {} out of range ({} channels)",
                        idx,
                        frame.seq_data.len()
                    )));
                }
                Ok(DataPoint {
                    address: address.to_string(),
                    value: DataValue::Int16(frame.seq_data[idx]),
                    timestamp: chrono::Utc::now().timestamp_millis(),
                    quality: DataQuality::Good,
                })
            }
            None => Ok(DataPoint {
                address: address.to_string(),
                value: DataValue::Int16(0),
                timestamp: chrono::Utc::now().timestamp_millis(),
                quality: DataQuality::Bad,
            }),
        }
    }

    async fn write(&mut self, _address: &str, _value: &DataValue) -> Result<()> {
        Err(eneros_core::EnerOSError::Device(
            "SV is read-only (subscribe to merging unit output)".into(),
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
        ProtocolType::Sv
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

    fn make_test_frame(appid: u16, sv_id: &str, channels: Vec<i16>) -> SvFrame {
        SvFrame {
            appid,
            sv_id: sv_id.to_string(),
            smp_cnt: 100,
            conf_rev: 1,
            refr_tm: None,
            smp_rate: SV_DEFAULT_SAMPLE_RATE,
            seq_data: channels,
        }
    }

    #[test]
    fn test_frame_serialize_parse_roundtrip() {
        let frame = make_test_frame(
            0x4000,
            "MU01",
            vec![100, -100, 200, -200, 300, -300, 400, -400],
        );
        let src_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let bytes = frame.serialize(&src_mac);
        assert!(bytes.len() > 14);

        let parsed = SvFrame::parse(&bytes).expect("parse should succeed");
        assert_eq!(parsed.appid, 0x4000);
        assert_eq!(parsed.sv_id, "MU01");
        assert_eq!(parsed.smp_cnt, 100);
        assert_eq!(parsed.seq_data.len(), 8);
        assert_eq!(parsed.seq_data[0], 100);
        assert_eq!(parsed.seq_data[7], -400);
    }

    #[test]
    fn test_parse_wrong_ethertype() {
        let mut frame = vec![0u8; 20];
        frame[12] = 0x08;
        frame[13] = 0x00;
        let result = SvFrame::parse(&frame);
        assert!(matches!(result, Err(SvParseError::WrongEtherType(0x0800))));
    }

    #[test]
    fn test_parse_too_short() {
        let result = SvFrame::parse(&[0u8; 5]);
        assert!(matches!(result, Err(SvParseError::TooShort)));
    }

    #[test]
    fn test_to_engineering_conversion() {
        // 4000 counts = nominal primary
        let val = SvFrame::to_engineering(0, 4000, 100.0); // 100A nominal
        assert!((val - 100.0).abs() < 0.01);

        let val2 = SvFrame::to_engineering(0, 2000, 100.0);
        assert!((val2 - 50.0).abs() < 0.01);

        let val3 = SvFrame::to_engineering(0, -4000, 100.0);
        assert!((val3 - (-100.0)).abs() < 0.01);
    }

    #[test]
    fn test_parse_address() {
        let (sv_id, idx) = SvAdapter::parse_address("MU01/3").unwrap();
        assert_eq!(sv_id, "MU01");
        assert_eq!(idx, 3);
    }

    #[test]
    fn test_parse_address_invalid() {
        assert!(SvAdapter::parse_address("noindex").is_err());
        assert!(SvAdapter::parse_address("MU01/abc").is_err());
        assert!(SvAdapter::parse_address("MU01/0/extra").is_err());
    }

    #[tokio::test]
    async fn test_mock_adapter_read_cached() {
        let (mut adapter, _) = SvAdapter::new_mock("test-sv");
        adapter.shared_state.mark_connected();

        let frame = make_test_frame(1, "MU01", vec![100, 200, 300]);
        adapter.inject_frame(frame).await;

        let dp = adapter.read("MU01/0").await.unwrap();
        assert_eq!(dp.value, DataValue::Int16(100));
        assert_eq!(dp.quality, DataQuality::Good);

        let dp2 = adapter.read("MU01/2").await.unwrap();
        assert_eq!(dp2.value, DataValue::Int16(300));
    }

    #[tokio::test]
    async fn test_mock_adapter_read_missing() {
        let (mut adapter, _) = SvAdapter::new_mock("test-sv");
        adapter.shared_state.mark_connected();

        let dp = adapter.read("missing/0").await.unwrap();
        assert_eq!(dp.quality, DataQuality::Bad);
    }

    #[tokio::test]
    async fn test_mock_adapter_read_out_of_range() {
        let (mut adapter, _) = SvAdapter::new_mock("test-sv");
        adapter.shared_state.mark_connected();

        let frame = make_test_frame(1, "MU01", vec![100]);
        adapter.inject_frame(frame).await;

        let result = adapter.read("MU01/5").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mock_adapter_subscribe() {
        let (mut adapter, _) = SvAdapter::new_mock("test-sv");
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_clone = received.clone();

        adapter
            .subscribe(vec![], Box::new(move |dp| {
                received_clone.try_lock().unwrap().push(dp);
            }))
            .await
            .unwrap();

        let frame = make_test_frame(1, "MU01", vec![100, 200]);
        adapter.inject_frame(frame).await;

        let msgs = received.lock().await;
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].value, DataValue::Int16(100));
        assert_eq!(msgs[1].value, DataValue::Int16(200));
    }

    #[tokio::test]
    async fn test_mock_adapter_write_not_supported() {
        let (mut adapter, _) = SvAdapter::new_mock("test-sv");
        let result = adapter.write("MU01/0", &DataValue::Int16(100)).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_adapter_creation() {
        let (adapter, _) = SvAdapter::new_mock("test-sv");
        assert_eq!(adapter.name(), "test-sv");
        assert_eq!(adapter.protocol_type(), ProtocolType::Sv);
        assert!(!adapter.is_connected());
    }

    #[test]
    fn test_sv_ethertype() {
        assert_eq!(SV_ETHERTYPE, 0x88BA);
    }

    #[test]
    fn test_sv_multicast_prefix() {
        assert_eq!(SV_MULTICAST_PREFIX, [0x01, 0x0C, 0xCD, 0x04, 0x00]);
    }

    #[test]
    fn test_default_sample_rate() {
        assert_eq!(SV_DEFAULT_SAMPLE_RATE, 4000);
        assert_eq!(SV_SAMPLES_PER_CYCLE_50HZ, 80);
    }

    #[tokio::test]
    async fn test_get_latest_frame() {
        let (mut adapter, _) = SvAdapter::new_mock("test");
        adapter.shared_state.mark_connected();

        let frame = make_test_frame(1, "MU01", vec![100, 200]);
        adapter.inject_frame(frame).await;

        let latest = adapter.get_latest_frame("MU01").await.unwrap();
        assert_eq!(latest.sv_id, "MU01");
        assert_eq!(latest.seq_data, vec![100, 200]);
    }

    #[tokio::test]
    async fn test_multiple_sv_ids() {
        let (mut adapter, _) = SvAdapter::new_mock("test");
        adapter.shared_state.mark_connected();

        adapter
            .inject_frame(make_test_frame(1, "MU01", vec![100]))
            .await;
        adapter
            .inject_frame(make_test_frame(2, "MU02", vec![200]))
            .await;

        let dp1 = adapter.read("MU01/0").await.unwrap();
        assert_eq!(dp1.value, DataValue::Int16(100));

        let dp2 = adapter.read("MU02/0").await.unwrap();
        assert_eq!(dp2.value, DataValue::Int16(200));
    }
}
