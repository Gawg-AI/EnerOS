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
use crate::adapters::af_packet::{AfPacketConfig, AdapterError};
#[cfg(target_os = "linux")]
use crate::adapters::af_packet::AfPacketTransport;

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

/// 单个 ASDU（应用服务数据单元）—— IEC 61850-9-2。
///
/// 一个 SV 帧可包含多个 ASDU（典型 8 个），每个 ASDU 携带一组
/// 瞬时采样值（4 路电流 + 4 路电压）及对应的 smpCnt/confRev 等元数据。
#[derive(Debug, Clone)]
pub struct SvAsdu {
    /// 采样计数器——每个采样点递增，到达 smpRate 后回绕
    pub smp_cnt: u32,
    /// 配置修订号
    pub conf_rev: u32,
    /// 刷新时间戳（可选，毫秒）
    pub refr_tm: Option<u64>,
    /// 采样率（Hz）
    pub smp_rate: u32,
    /// 采样值序列（通常 iA, iB, iC, iN, uA, uB, uC, uN）
    pub seq_data: Vec<i16>,
}

/// Parsed SV frame (IEC 61850-9-2).
#[derive(Debug, Clone)]
pub struct SvFrame {
    /// APPID from the SV header
    pub appid: u16,
    /// SV dataset identifier (e.g., "MU01")
    pub sv_id: String,
    /// Sample counter — increments per sample, wraps at sample_rate
    /// (取自第一个 ASDU，向后兼容)
    pub smp_cnt: u32,
    /// Configuration revision
    /// (取自第一个 ASDU，向后兼容)
    pub conf_rev: u32,
    /// RefrTm (optional timestamp)
    /// (取自第一个 ASDU，向后兼容)
    pub refr_tm: Option<u64>,
    /// Sample rate (smpRate)
    /// (取自第一个 ASDU，向后兼容)
    pub smp_rate: u32,
    /// Sequence data — instantaneous values (typically iA, iB, iC, iN, uA, uB, uC, uN)
    /// (取自第一个 ASDU，向后兼容；多 ASDU 场景请使用 asdus 字段或 all_asdus() 方法)
    pub seq_data: Vec<i16>,
    /// 所有 ASDU 列表（多 ASDU 支持）
    ///
    /// 解析时填充所有 ASDU；手动构造的单 ASDU 帧此字段为空，
    /// 此时 asdu_count()/asdu_at()/all_asdus() 会回退到顶层字段。
    pub asdus: Vec<SvAsdu>,
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
        // 修复 M20：校验 length 字段下限。SV header 自身占 8 字节
        // (appid 2 + length 2 + reserved 4)，length < 8 表示 PDU 长度字段
        // 非法（连 header 都装不下），按 IEC 61850-9-2 规范应拒绝。
        if length < 8 {
            return Err(SvParseError::HeaderTooShort);
        }
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
        // 确定要编码的 ASDU 列表：
        // 若 asdus 非空则编码所有 ASDU（多 ASDU 场景），
        // 否则从顶层字段构造单个 ASDU（向后兼容）。
        let asdus: Vec<SvAsdu> = if self.asdus.is_empty() {
            vec![SvAsdu {
                smp_cnt: self.smp_cnt,
                conf_rev: self.conf_rev,
                refr_tm: self.refr_tm,
                smp_rate: self.smp_rate,
                seq_data: self.seq_data.clone(),
            }]
        } else {
            self.asdus.clone()
        };

        let mut inner = Vec::new();
        // noASDU [0] INTEGER — ASDU 数量
        encode_int_tlv(&mut inner, 0x80, asdus.len() as i64);
        // seqASDU [1] SEQUENCE OF ASDU
        let mut asdu_seq = Vec::new();
        for asdu in &asdus {
            let mut a = Vec::new();
            // svID [0x80] VisibleString (context tag 0, primitive)
            encode_string_tlv(&mut a, 0x80, &self.sv_id);
            // smpCnt [0x82] INTEGER
            encode_int_tlv(&mut a, 0x82, asdu.smp_cnt as i64);
            // confRev [0x83] INTEGER
            encode_int_tlv(&mut a, 0x83, asdu.conf_rev as i64);
            // refrTm [0x84] UtcTime (optional, skip)
            // smpRate [0x85] INTEGER
            encode_int_tlv(&mut a, 0x85, asdu.smp_rate as i64);
            // sample [0xA6] SEQUENCE
            let mut sample_seq = Vec::new();
            for &val in &asdu.seq_data {
                // Each value is INTEGER (context tag 7, primitive)
                encode_int_tlv(&mut sample_seq, 0x87, val as i64);
            }
            encode_constructed(&mut a, 0xA6, &sample_seq);
            // ASDU as SEQUENCE
            encode_constructed(&mut asdu_seq, 0x30, &a);
        }
        encode_constructed(&mut inner, 0xA1, &asdu_seq);
        // Wrap in SEQUENCE
        encode_constructed(buf, 0x60, &inner);
    }

    /// 返回 ASDU 数量。
    ///
    /// 若 asdus 字段非空则返回其长度；否则返回 1（单 ASDU 向后兼容）。
    pub fn asdu_count(&self) -> usize {
        if self.asdus.is_empty() {
            1
        } else {
            self.asdus.len()
        }
    }

    /// 获取指定索引处的 ASDU 数据。
    ///
    /// 返回 Some(SvAsdu) 表示成功，None 表示索引越界。
    /// 若 asdus 字段为空（手动构造的单 ASDU 帧），仅 index==0 返回顶层字段数据。
    pub fn asdu_at(&self, index: usize) -> Option<SvAsdu> {
        if !self.asdus.is_empty() {
            self.asdus.get(index).cloned()
        } else if index == 0 {
            Some(SvAsdu {
                smp_cnt: self.smp_cnt,
                conf_rev: self.conf_rev,
                refr_tm: self.refr_tm,
                smp_rate: self.smp_rate,
                seq_data: self.seq_data.clone(),
            })
        } else {
            None
        }
    }

    /// 获取所有 ASDU 的迭代器（以 Vec 返回，便于遍历）。
    ///
    /// 若 asdus 字段为空，则返回包含单个 ASDU（从顶层字段构造）的 Vec。
    pub fn all_asdus(&self) -> Vec<SvAsdu> {
        if !self.asdus.is_empty() {
            self.asdus.clone()
        } else {
            vec![SvAsdu {
                smp_cnt: self.smp_cnt,
                conf_rev: self.conf_rev,
                refr_tm: self.refr_tm,
                smp_rate: self.smp_rate,
                seq_data: self.seq_data.clone(),
            }]
        }
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
            asdus: Vec::new(),
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

        // 遍历解析所有 ASDU（修复：此前仅取第一个 ASDU）
        let mut asdu_parser = SvBerParser::new(asdu_content);
        while asdu_parser.pos < asdu_parser.data.len() {
            let (_seq_tag, asdu_inner) = asdu_parser.read_tlv()?;
            let mut p = SvBerParser::new(asdu_inner);
            let mut asdu = SvAsdu {
                smp_cnt: 0,
                conf_rev: 0,
                refr_tm: None,
                smp_rate: SV_DEFAULT_SAMPLE_RATE,
                seq_data: Vec::new(),
            };

            while p.pos < p.data.len() {
                let (tag, content) = p.read_tlv()?;
                match tag {
                    0x80 => frame.sv_id = String::from_utf8_lossy(content).into_owned(),
                    0x82 => asdu.smp_cnt = decode_ber_int(content)? as u32,
                    0x83 => asdu.conf_rev = decode_ber_int(content)? as u32,
                    0x84 => asdu.refr_tm = Some(decode_ber_utc(content)?),
                    0x85 => asdu.smp_rate = decode_ber_int(content)? as u32,
                    0xA6 => {
                        // sample SEQUENCE — contains channel values
                        let mut sample_parser = SvBerParser::new(content);
                        while sample_parser.pos < sample_parser.data.len() {
                            let (stag, scontent) = sample_parser.read_tlv()?;
                            if stag == 0x87 {
                                asdu.seq_data.push(decode_ber_int(scontent)? as i16);
                            }
                        }
                    }
                    _ => {}
                }
            }

            frame.asdus.push(asdu);
        }

        // 向后兼容：从第一个 ASDU 填充顶层字段
        if let Some(first) = frame.asdus.first() {
            frame.smp_cnt = first.smp_cnt;
            frame.conf_rev = first.conf_rev;
            frame.refr_tm = first.refr_tm;
            frame.smp_rate = first.smp_rate;
            frame.seq_data = first.seq_data.clone();
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
        if bytes.last().is_some_and(|&b| b & 0x80 != 0) {
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
            if v == -1 && bytes.last().is_some_and(|&b| b & 0x80 != 0) {
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
/// instantaneous sample values via the ProtocolAdapter trait.
pub struct SvAdapter {
    transport: Arc<Box<dyn GooseTransport>>,
    shared_state: SharedState,
    name: String,
    config: SvConfig,
    /// Ring buffer of recent samples per svID
    cache: Arc<RwLock<HashMap<String, SvFrame>>>,
    /// Callbacks
    #[allow(clippy::type_complexity)]
    callbacks: Arc<RwLock<Vec<Box<dyn Fn(DataPoint) + Send + Sync>>>>,
    recv_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl SvAdapter {
    /// Create a new SV adapter with the given transport.
    pub fn with_transport(name: &str, config: SvConfig, transport: Box<dyn GooseTransport>) -> Self {
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

    /// Create an SV adapter with mock transport (for testing).
    pub fn new_mock(name: &str) -> (Self, tokio::sync::mpsc::Sender<Vec<u8>>) {
        let (transport, sender) = MockGooseTransport::new();
        let adapter = Self::with_transport(name, SvConfig::default(), Box::new(transport));
        (adapter, sender)
    }

    /// 使用 AF_PACKET 原始套接字创建 SV 适配器（Linux 原生直采）。
    ///
    /// 非 Linux 平台返回 AdapterError::Unsupported。
    /// 创建套接字需要 CAP_NET_RAW 能力（root 或 setcap）。
    ///
    /// # 参数
    /// - `name`: 适配器实例名（与 `GooseAdapter::with_af_packet` API 一致）
    /// - `config`: SV 协议配置
    /// - `af_config`: AF_PACKET 传输配置（网卡名、源 MAC），通常通过
    ///   `AfPacketConfig::for_sv(interface, src_mac)` 构建
    #[cfg(target_os = "linux")]
    pub fn with_af_packet(
        name: &str,
        config: SvConfig,
        af_config: AfPacketConfig,
    ) -> std::result::Result<Self, AdapterError> {
        let transport = AfPacketTransport::new(af_config)?;
        Ok(Self::with_transport(name, config, Box::new(transport)))
    }

    /// 使用 AF_PACKET 原始套接字创建 SV 适配器（非 Linux stub）。
    ///
    /// 非 Linux 平台始终返回 AdapterError::Unsupported。
    /// AF_PACKET 是 Linux 专有的原始套接字机制，在 Windows/macOS 等
    /// 平台不可用。此 stub 保证 API 跨平台编译通过。
    #[cfg(not(target_os = "linux"))]
    pub fn with_af_packet(
        _name: &str,
        _config: SvConfig,
        _af_config: AfPacketConfig,
    ) -> std::result::Result<Self, AdapterError> {
        Err(AdapterError::Unsupported(
            "AF_PACKET requires Linux".into(),
        ))
    }

    /// Get the latest cached frame for a given svID.
    pub async fn get_latest_frame(&self, sv_id: &str) -> Option<SvFrame> {
        self.cache.read().await.get(sv_id).cloned()
    }

    /// Inject a parsed frame directly (for testing).
    ///
    /// 修复 H15：遍历所有 ASDU 通知回调，避免多 ASDU 数据丢失。
    /// 地址格式 `svID/asdu_idx/ch_idx`，便于订阅者区分不同采样点。
    /// 单 ASDU 回退（asdus 为空）时 asdu_idx 恒为 0。
    pub async fn inject_frame(&self, frame: SvFrame) {
        let sv_id = frame.sv_id.clone();
        let asdus = frame.all_asdus();
        self.cache.write().await.insert(sv_id.clone(), frame);

        let callbacks = self.callbacks.read().await;
        let timestamp = chrono::Utc::now().timestamp_millis();
        for (asdu_idx, asdu) in asdus.iter().enumerate() {
            for (ch_idx, val) in asdu.seq_data.iter().enumerate() {
                let dp = DataPoint {
                    address: format!("{}/{}/{}", sv_id, asdu_idx, ch_idx),
                    value: DataValue::Int16(*val),
                    timestamp,
                    quality: DataQuality::Good,
                };
                for cb in callbacks.iter() {
                    cb(dp.clone());
                }
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
                let frame_bytes = match transport.receive().await {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        tracing::warn!("SV receive error: {}", e);
                        break;
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
                        let asdus = sv.all_asdus();

                        cache.write().await.insert(sv_id.clone(), sv);

                        let cbs = callbacks.read().await;
                        let timestamp = chrono::Utc::now().timestamp_millis();
                        for (asdu_idx, asdu) in asdus.iter().enumerate() {
                            for (ch_idx, val) in asdu.seq_data.iter().enumerate() {
                                let dp = DataPoint {
                                    address: format!("{}/{}/{}", sv_id, asdu_idx, ch_idx),
                                    value: DataValue::Int16(*val),
                                    timestamp,
                                    quality: DataQuality::Good,
                                };
                                for cb in cbs.iter() {
                                    cb(dp.clone());
                                }
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
            asdus: Vec::new(),
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
        let val = SvFrame::to_engineering(0, 4000, 100.0);
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
        let (adapter, _) = SvAdapter::new_mock("test-sv");
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
        let (adapter, _) = SvAdapter::new_mock("test-sv");
        adapter.shared_state.mark_connected();

        let dp = adapter.read("missing/0").await.unwrap();
        assert_eq!(dp.quality, DataQuality::Bad);
    }

    #[tokio::test]
    async fn test_mock_adapter_read_out_of_range() {
        let (adapter, _) = SvAdapter::new_mock("test-sv");
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
        let (adapter, _) = SvAdapter::new_mock("test");
        adapter.shared_state.mark_connected();

        let frame = make_test_frame(1, "MU01", vec![100, 200]);
        adapter.inject_frame(frame).await;

        let latest = adapter.get_latest_frame("MU01").await.unwrap();
        assert_eq!(latest.sv_id, "MU01");
        assert_eq!(latest.seq_data, vec![100, 200]);
    }

    #[tokio::test]
    async fn test_multiple_sv_ids() {
        let (adapter, _) = SvAdapter::new_mock("test");
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

    // ========================================================================
    // 多 ASDU 解析测试（IEC 61850-9-2LE 典型 8 ASDU）
    // ========================================================================

    #[test]
    fn test_multi_asdu_parse_8_asdus() {
        let frame = SvFrame {
            appid: 0x4000,
            sv_id: "MU01".to_string(),
            smp_cnt: 100,
            conf_rev: 1,
            refr_tm: None,
            smp_rate: SV_DEFAULT_SAMPLE_RATE,
            seq_data: Vec::new(),
            asdus: (0..8u32)
                .map(|i| SvAsdu {
                    smp_cnt: 100 + i,
                    conf_rev: 1,
                    refr_tm: None,
                    smp_rate: SV_DEFAULT_SAMPLE_RATE,
                    seq_data: vec![
                        100 + i as i16,
                        200 + i as i16,
                        300 + i as i16,
                        400 + i as i16,
                        500 + i as i16,
                        600 + i as i16,
                        700 + i as i16,
                        800 + i as i16,
                    ],
                })
                .collect(),
        };

        let src_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let bytes = frame.serialize(&src_mac);
        assert!(bytes.len() > 14);

        let parsed = SvFrame::parse(&bytes).expect("parse should succeed");
        assert_eq!(parsed.asdu_count(), 8, "should parse 8 ASDUs");

        for i in 0..8 {
            let asdu = parsed.asdu_at(i).expect("ASDU should exist");
            assert_eq!(asdu.smp_cnt, 100 + i as u32, "ASDU {} smp_cnt mismatch", i);
            assert_eq!(asdu.conf_rev, 1);
            assert_eq!(asdu.smp_rate, SV_DEFAULT_SAMPLE_RATE);
            assert_eq!(asdu.seq_data.len(), 8, "ASDU {} should have 8 channels", i);
            assert_eq!(asdu.seq_data[0], 100 + i as i16);
            assert_eq!(asdu.seq_data[7], 800 + i as i16);
        }

        // 向后兼容：顶层字段取自第一个 ASDU
        assert_eq!(parsed.smp_cnt, 100);
        assert_eq!(parsed.seq_data.len(), 8);
        assert_eq!(parsed.seq_data[0], 100);
    }

    #[test]
    fn test_multi_asdu_all_asdus_iterator() {
        let frame = SvFrame {
            appid: 0x4000,
            sv_id: "MU01".to_string(),
            smp_cnt: 0,
            conf_rev: 1,
            refr_tm: None,
            smp_rate: SV_DEFAULT_SAMPLE_RATE,
            seq_data: Vec::new(),
            asdus: (0..4u32)
                .map(|i| SvAsdu {
                    smp_cnt: i,
                    conf_rev: 1,
                    refr_tm: None,
                    smp_rate: SV_DEFAULT_SAMPLE_RATE,
                    seq_data: vec![i as i16, (i + 1) as i16],
                })
                .collect(),
        };

        let all = frame.all_asdus();
        assert_eq!(all.len(), 4);
        for (i, asdu) in all.iter().enumerate() {
            assert_eq!(asdu.smp_cnt, i as u32);
            assert_eq!(asdu.seq_data.len(), 2);
        }
    }

    #[test]
    fn test_single_asdu_fallback_methods() {
        let frame = make_test_frame(0x4000, "MU01", vec![100, 200, 300]);

        assert_eq!(frame.asdu_count(), 1);

        let asdu0 = frame.asdu_at(0).expect("index 0 should succeed");
        assert_eq!(asdu0.smp_cnt, 100);
        assert_eq!(asdu0.seq_data, vec![100, 200, 300]);

        assert!(frame.asdu_at(1).is_none());

        let all = frame.all_asdus();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].seq_data, vec![100, 200, 300]);
    }

    #[test]
    fn test_multi_asdu_roundtrip_preserves_count() {
        let frame = SvFrame {
            appid: 0x4000,
            sv_id: "MU02".to_string(),
            smp_cnt: 0,
            conf_rev: 2,
            refr_tm: None,
            smp_rate: 4800,
            seq_data: Vec::new(),
            asdus: (0..3u32)
                .map(|i| SvAsdu {
                    smp_cnt: 200 + i,
                    conf_rev: 2,
                    refr_tm: None,
                    smp_rate: 4800,
                    seq_data: vec![-1000, 0, 1000],
                })
                .collect(),
        };

        let bytes = frame.serialize(&[0; 6]);
        let parsed = SvFrame::parse(&bytes).expect("parse should succeed");

        assert_eq!(parsed.asdu_count(), 3);
        assert_eq!(parsed.sv_id, "MU02");
        assert_eq!(parsed.smp_rate, 4800);
        assert_eq!(parsed.conf_rev, 2);

        for i in 0..3 {
            let asdu = parsed.asdu_at(i).unwrap();
            assert_eq!(asdu.smp_cnt, 200 + i as u32);
            assert_eq!(asdu.seq_data, vec![-1000, 0, 1000]);
        }
    }

    // ========================================================================
    // Mock transport 集成测试
    // ========================================================================

    #[tokio::test]
    async fn test_mock_transport_sv_roundtrip() {
        let (adapter, sender) = SvAdapter::new_mock("test-sv-roundtrip");

        let frame = make_test_frame(
            0x4000,
            "MU01",
            vec![100, 200, 300, 400, 500, 600, 700, 800],
        );
        let src_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let bytes = frame.serialize(&src_mac);

        sender.send(bytes).await.unwrap();

        adapter.start_receive_loop().await;
        adapter.shared_state.mark_connected();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let latest = adapter
            .get_latest_frame("MU01")
            .await
            .expect("frame should be cached");
        assert_eq!(latest.sv_id, "MU01");
        assert_eq!(latest.appid, 0x4000);
        assert_eq!(latest.seq_data.len(), 8);
        assert_eq!(latest.seq_data[0], 100);
        assert_eq!(latest.seq_data[7], 800);

        adapter.stop_receive_loop().await;
    }

    #[tokio::test]
    async fn test_mock_transport_multi_asdu_roundtrip() {
        let (adapter, sender) = SvAdapter::new_mock("test-sv-multi");

        let frame = SvFrame {
            appid: 0x4000,
            sv_id: "MU03".to_string(),
            smp_cnt: 0,
            conf_rev: 1,
            refr_tm: None,
            smp_rate: SV_DEFAULT_SAMPLE_RATE,
            seq_data: Vec::new(),
            asdus: (0..4u32)
                .map(|i| SvAsdu {
                    smp_cnt: 50 + i,
                    conf_rev: 1,
                    refr_tm: None,
                    smp_rate: SV_DEFAULT_SAMPLE_RATE,
                    seq_data: vec![1000 + i as i16 * 10, 2000, 3000],
                })
                .collect(),
        };
        let src_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let bytes = frame.serialize(&src_mac);

        sender.send(bytes).await.unwrap();

        adapter.start_receive_loop().await;
        adapter.shared_state.mark_connected();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let latest = adapter
            .get_latest_frame("MU03")
            .await
            .expect("frame should be cached");
        assert_eq!(latest.sv_id, "MU03");
        assert_eq!(latest.asdu_count(), 4, "cached frame should have 4 ASDUs");

        assert_eq!(latest.smp_cnt, 50);
        assert_eq!(latest.seq_data.len(), 3);
        assert_eq!(latest.seq_data[0], 1000);

        adapter.stop_receive_loop().await;
    }

    // ========================================================================
    // with_af_packet 构造测试
    // ========================================================================

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn test_with_af_packet_unsupported_on_nonlinux() {
        let cfg = AfPacketConfig::for_sv("eth0", [0; 6]);
        let result = SvAdapter::with_af_packet("test-sv-af", SvConfig::default(), cfg);
        assert!(result.is_err(), "non-Linux should return error");
        match result {
            Err(AdapterError::Unsupported(_)) => {}
            Err(e) => panic!("expected Unsupported error, got: {:?}", e),
            Ok(_) => panic!("should not succeed on non-Linux"),
        }
    }

    #[test]
    fn test_with_af_packet_config_creation() {
        let cfg = AfPacketConfig::for_sv("eth1", [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        assert_eq!(cfg.interface, "eth1");
        assert_eq!(cfg.ethertype, SV_ETHERTYPE);
        assert_eq!(cfg.src_mac, [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
    }

    // ========================================================================
    // H15 修复测试：多 ASDU 回调收到所有 ASDU 数据
    // ========================================================================

    /// 验证 8 ASDU × 8 通道 = 64 个数据点全部通过回调通知，且地址格式
    /// `svID/asdu_idx/ch_idx` 正确区分不同采样点。
    #[tokio::test]
    async fn test_multi_asdu_callback_receives_all_asdus() {
        let (mut adapter, _) = SvAdapter::new_mock("test-sv-multi-cb");
        let received: Arc<Mutex<Vec<DataPoint>>> = Arc::new(Mutex::new(Vec::new()));
        let received_clone = received.clone();

        adapter
            .subscribe(vec![], Box::new(move |dp| {
                received_clone.try_lock().unwrap().push(dp);
            }))
            .await
            .unwrap();

        // 构造 8 ASDU × 8 通道的 SV 帧
        let frame = SvFrame {
            appid: 0x4000,
            sv_id: "MU04".to_string(),
            smp_cnt: 0,
            conf_rev: 1,
            refr_tm: None,
            smp_rate: SV_DEFAULT_SAMPLE_RATE,
            seq_data: Vec::new(),
            asdus: (0..8u32)
                .map(|i| SvAsdu {
                    smp_cnt: 100 + i,
                    conf_rev: 1,
                    refr_tm: None,
                    smp_rate: SV_DEFAULT_SAMPLE_RATE,
                    seq_data: vec![
                        (1000 + i as i16),
                        (1100 + i as i16),
                        (1200 + i as i16),
                        (1300 + i as i16),
                        (1400 + i as i16),
                        (1500 + i as i16),
                        (1600 + i as i16),
                        (1700 + i as i16),
                    ],
                })
                .collect(),
        };

        adapter.inject_frame(frame).await;

        let msgs = received.lock().await;
        assert_eq!(msgs.len(), 64, "8 ASDU × 8 channels should yield 64 data points");

        // 验证每个 ASDU 的每个通道都收到，且地址格式正确
        for asdu_idx in 0..8usize {
            for ch_idx in 0..8usize {
                let expected_addr = format!("MU04/{}/{}", asdu_idx, ch_idx);
                // seq_data[ch_idx] = (1000 + ch_idx*100) + asdu_idx
                let expected_val = 1000 + ch_idx as i16 * 100 + asdu_idx as i16;
                let found = msgs.iter().find(|dp| dp.address == expected_addr);
                assert!(
                    found.is_some(),
                    "missing data point for address {}",
                    expected_addr
                );
                let dp = found.unwrap();
                assert_eq!(
                    dp.value,
                    DataValue::Int16(expected_val),
                    "value mismatch at {}",
                    expected_addr
                );
                assert_eq!(dp.quality, DataQuality::Good);
            }
        }
    }

    /// 验证单 ASDU 回退：asdus 为空时使用顶层 seq_data，asdu_idx 恒为 0。
    /// 地址格式 `svID/0/ch_idx`。
    #[tokio::test]
    async fn test_single_asdu_fallback_callback() {
        let (mut adapter, _) = SvAdapter::new_mock("test-sv-single-cb");
        let received: Arc<Mutex<Vec<DataPoint>>> = Arc::new(Mutex::new(Vec::new()));
        let received_clone = received.clone();

        adapter
            .subscribe(vec![], Box::new(move |dp| {
                received_clone.try_lock().unwrap().push(dp);
            }))
            .await
            .unwrap();

        // asdus 为空的单 ASDU 帧（向后兼容路径）
        let frame = make_test_frame(0x4000, "MU05", vec![100, 200, 300]);
        adapter.inject_frame(frame).await;

        let msgs = received.lock().await;
        assert_eq!(msgs.len(), 3, "single ASDU with 3 channels should yield 3 data points");

        // asdu_idx 恒为 0
        assert_eq!(msgs[0].address, "MU05/0/0");
        assert_eq!(msgs[0].value, DataValue::Int16(100));
        assert_eq!(msgs[1].address, "MU05/0/1");
        assert_eq!(msgs[1].value, DataValue::Int16(200));
        assert_eq!(msgs[2].address, "MU05/0/2");
        assert_eq!(msgs[2].value, DataValue::Int16(300));
    }

    // ========================================================================
    // M20 修复测试：parse 检查 length 下限
    // ========================================================================

    /// 验证 length 字段 < 8 时返回 HeaderTooShort 错误。
    /// SV header 自身占 8 字节（appid 2 + length 2 + reserved 4），
    /// length < 8 表示 PDU 长度字段非法。
    #[test]
    fn test_parse_length_below_minimum() {
        // 构造一个 length 字段 < 8 的帧
        let mut frame = vec![0u8; 22]; // 14 ethernet + 8 SV header
        // Ethernet header with SV ethertype
        frame[12] = 0x88;
        frame[13] = 0xBA;
        // SV header: appid=0x4000, length=4 (< 8, 非法)
        frame[14] = 0x40;
        frame[15] = 0x00;
        frame[16] = 0x00;
        frame[17] = 0x04; // length = 4
        let result = SvFrame::parse(&frame);
        assert!(
            matches!(result, Err(SvParseError::HeaderTooShort)),
            "length < 8 should return HeaderTooShort, got: {:?}",
            result
        );
    }

    /// 验证 length 字段 = 0 时也返回 HeaderTooShort 错误（边界值）。
    #[test]
    fn test_parse_length_zero() {
        let mut frame = vec![0u8; 22];
        frame[12] = 0x88;
        frame[13] = 0xBA;
        // appid=0x4000, length=0
        frame[14] = 0x40;
        frame[15] = 0x00;
        frame[16] = 0x00;
        frame[17] = 0x00;
        let result = SvFrame::parse(&frame);
        assert!(
            matches!(result, Err(SvParseError::HeaderTooShort)),
            "length = 0 should return HeaderTooShort, got: {:?}",
            result
        );
    }
}
