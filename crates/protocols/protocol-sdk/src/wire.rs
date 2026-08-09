//! IPC wire codec（ARCH-S01 / ADR-0025）— 协议插件跨进程消息的二进制编解码。
//!
//! 本模块将 `protocol-sdk` 从「cross-binary trait-object 插件」收敛为
//! **IPC wire codec 客户端**：插件与宿主（EL0 协议服务进程）之间的一切请求/
//! 响应都编码为有界、可校验的二进制帧，经版本化 IPC wire 交换；不再以
//! `*mut dyn` / trait object 跨进程传递。
//!
//! # 帧格式（对齐 agent-sdk `wire.rs` 的 EIPC 规范，ADR-0025）
//!
//! ```text
//! +--------+--------+--------+--------+--------+--------+--------+--------+
//! | 0x45   | 0x50   | 0x52   | 0x4F   | ver    | msg    | flags  | rsvd   |
//! | 'E'    | 'P'    | 'R'    | 'O'    | (=1)   | type   | (=0)   | (=0)   |
//! +--------+--------+--------+--------+--------+--------+--------+--------+
//! | payload_len (u32 LE)                                               |
//! +--------+--------+--------+--------+--------+--------+--------+--------+
//! | payload (variable, 0..=1024 bytes)                                  |
//! +--------+--------+--------+--------+--------+--------+--------+--------+
//! | crc32 (u32 LE, over magic..payload end)                            |
//! +--------+--------+--------+--------+--------+--------+--------+--------+
//! ```
//!
//! 最大帧长：12 (header) + 1024 (payload) + 4 (crc) = 1040 字节。
//!
//! # no_std
//!
//! 帧层纯 `core` 无分配；语义层（`ProtocolRequest`/`ProtocolResponse`）仅在
//! 携带 `String` 字段时使用 `alloc`，所有长度均有上界。

#![allow(clippy::result_large_err)]

use alloc::string::String;
use alloc::vec::Vec;

use eneros_upa_model::{DataPoint, PointId, PointQuality, PointValue};

/// Magic bytes: "EPRO" (0x45 0x50 0x52 0x4F).
pub const MAGIC: [u8; 4] = *b"EPRO";

/// Wire format version.
pub const WIRE_VERSION: u8 = 1;

/// Header size: magic(4) + version(1) + msg_type(1) + flags(1) + rsvd(1) + payload_len(4).
pub const HEADER_LEN: usize = 12;

/// Maximum payload length (bytes).
pub const MAX_PAYLOAD_LEN: usize = 1024;

/// CRC32 trailer size.
pub const CRC_LEN: usize = 4;

/// Maximum wire frame size.
pub const MAX_WIRE_SIZE: usize = HEADER_LEN + MAX_PAYLOAD_LEN + CRC_LEN;

// ============================================================================
// 消息类型
// ============================================================================

/// 帧消息类型（协议插件 IPC 消息）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MsgType {
    /// 连接请求。
    Connect = 1,
    /// 读点请求。
    ReadPoint = 2,
    /// 写点请求。
    WritePoint = 3,
    /// 断开请求。
    Disconnect = 4,
    /// 成功响应。
    Response = 5,
    /// 错误响应。
    Error = 6,
}

impl MsgType {
    /// 从 u8 还原消息类型。
    pub fn from_u8(v: u8) -> Option<MsgType> {
        match v {
            1 => Some(MsgType::Connect),
            2 => Some(MsgType::ReadPoint),
            3 => Some(MsgType::WritePoint),
            4 => Some(MsgType::Disconnect),
            5 => Some(MsgType::Response),
            6 => Some(MsgType::Error),
            _ => None,
        }
    }
}

// ============================================================================
// 帧层（纯 core，零分配）
// ============================================================================

/// Wire encode error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// Payload too large (max 1024 bytes).
    PayloadTooLarge,
    /// Output buffer too small.
    BufferTooSmall,
}

/// Encode a frame into `out`. Returns the number of bytes written.
///
/// 调用方提供输出缓冲；本函数零分配。出错时 `out` 内容未定义。
pub fn encode_frame(msg_type: MsgType, payload: &[u8], out: &mut [u8]) -> Result<usize, WireError> {
    if payload.len() > MAX_PAYLOAD_LEN {
        return Err(WireError::PayloadTooLarge);
    }
    let total_len = HEADER_LEN + payload.len() + CRC_LEN;
    if out.len() < total_len {
        return Err(WireError::BufferTooSmall);
    }

    out[0..4].copy_from_slice(&MAGIC);
    out[4] = WIRE_VERSION;
    out[5] = msg_type as u8;
    out[6] = 0; // flags
    out[7] = 0; // reserved
    out[8..12].copy_from_slice(&(payload.len() as u32).to_le_bytes());

    let payload_offset = HEADER_LEN;
    out[payload_offset..payload_offset + payload.len()].copy_from_slice(payload);

    let crc_offset = payload_offset + payload.len();
    let crc = crc32(&out[..crc_offset]);
    out[crc_offset..crc_offset + 4].copy_from_slice(&crc.to_le_bytes());

    Ok(total_len)
}

/// Wire decode error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// Buffer too short for header.
    TruncatedHeader,
    /// Magic bytes mismatch.
    BadMagic,
    /// Unsupported wire version.
    UnsupportedVersion,
    /// Unknown message type.
    UnknownMessageType,
    /// Declared length exceeds bounds.
    LengthOutOfBounds,
    /// CRC32 mismatch (corrupted message).
    CrcMismatch,
    /// Payload semantically invalid.
    InvalidPayload,
}

/// Decoded frame (zero-copy borrow).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFrame<'a> {
    pub msg_type: MsgType,
    pub payload: &'a [u8],
}

/// Decode a frame from `buf` (borrowed view).
pub fn decode_frame(buf: &[u8]) -> Result<DecodedFrame<'_>, DecodeError> {
    if buf.len() < HEADER_LEN + CRC_LEN {
        return Err(DecodeError::TruncatedHeader);
    }
    if buf[0..4] != MAGIC {
        return Err(DecodeError::BadMagic);
    }
    let version = buf[4];
    if version != WIRE_VERSION {
        return Err(DecodeError::UnsupportedVersion);
    }
    let msg_type = MsgType::from_u8(buf[5]).ok_or(DecodeError::UnknownMessageType)?;

    // Reason: 前置长度校验（buf.len() < total_len）保证切片长度恰好匹配，try_into 不可达失败（21-B D1 豁免）
    let payload_len = u32::from_le_bytes(buf[8..12].try_into().expect("8..12 in bounds")) as usize;
    if payload_len > MAX_PAYLOAD_LEN {
        return Err(DecodeError::LengthOutOfBounds);
    }

    let total_len = HEADER_LEN + payload_len + CRC_LEN;
    if buf.len() < total_len {
        return Err(DecodeError::LengthOutOfBounds);
    }

    let crc_offset = HEADER_LEN + payload_len;
    // Reason: 前置长度校验（buf.len() < total_len）保证切片长度恰好匹配，try_into 不可达失败（21-B D1 豁免）
    let expected_crc = u32::from_le_bytes(buf[crc_offset..crc_offset + 4].try_into().unwrap());
    let actual_crc = crc32(&buf[..crc_offset]);
    if actual_crc != expected_crc {
        return Err(DecodeError::CrcMismatch);
    }

    Ok(DecodedFrame {
        msg_type,
        payload: &buf[HEADER_LEN..crc_offset],
    })
}

/// Software CRC32（共享实现：`eneros-ipc-wire::crc32`，ARCH-S04）。
///
/// 仅用于帧完整性；认证由传输层（HMAC/AEAD）负责，非加密 MAC。
/// 本函数为兼容导出，转发到共享 crate。
pub fn crc32(data: &[u8]) -> u32 {
    eneros_ipc_wire::crc32(data)
}

/// Compute the exact encoded frame size for a payload.
pub fn frame_size(payload_len: usize) -> usize {
    HEADER_LEN + payload_len + CRC_LEN
}

// ============================================================================
// 载荷字段原语（长度有界、显式 little-endian）
// ============================================================================

/// 从字节切片读取长度前缀字符串（u16 LE 长度 + UTF-8 bytes，≤ MAX_PAYLOAD_LEN）。
pub fn decode_len_str(buf: &[u8]) -> Result<(&str, usize), DecodeError> {
    if buf.len() < 2 {
        return Err(DecodeError::InvalidPayload);
    }
    // Reason: 前置长度校验（payload 长度检查）保证切片长度恰好匹配，try_into 不可达失败（21-B D1 豁免）
    let len = u16::from_le_bytes(buf[0..2].try_into().unwrap()) as usize;
    if len > MAX_PAYLOAD_LEN || buf.len() < 2 + len {
        return Err(DecodeError::InvalidPayload);
    }
    let s = core::str::from_utf8(&buf[2..2 + len]).map_err(|_| DecodeError::InvalidPayload)?;
    Ok((s, 2 + len))
}

/// 将长度前缀字符串写入 `out`（u16 LE 长度 + bytes）。
pub fn encode_len_str(s: &str, out: &mut Vec<u8>) -> Result<(), WireError> {
    if s.len() > MAX_PAYLOAD_LEN {
        return Err(WireError::PayloadTooLarge);
    }
    out.extend_from_slice(&(s.len() as u16).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

/// 点品质 wire 编码（7 位标志 → u8）。
pub fn encode_quality(q: PointQuality) -> u8 {
    let mut v = 0u8;
    if q.valid {
        v |= 1 << 0;
    }
    if q.invalid {
        v |= 1 << 1;
    }
    if q.questionable {
        v |= 1 << 2;
    }
    if q.substituted {
        v |= 1 << 3;
    }
    if q.overflow {
        v |= 1 << 4;
    }
    if q.blocked {
        v |= 1 << 5;
    }
    if q.outdated {
        v |= 1 << 6;
    }
    v
}

/// 点品质 wire 解码（u8 → 7 位标志）。
pub fn decode_quality(v: u8) -> PointQuality {
    PointQuality {
        valid: v & (1 << 0) != 0,
        invalid: v & (1 << 1) != 0,
        questionable: v & (1 << 2) != 0,
        substituted: v & (1 << 3) != 0,
        overflow: v & (1 << 4) != 0,
        blocked: v & (1 << 5) != 0,
        outdated: v & (1 << 6) != 0,
    }
}

/// 点值 wire 编码：tag(u8) + 值（小端）。
pub fn encode_point_value(v: &PointValue, out: &mut Vec<u8>) -> Result<(), WireError> {
    match v {
        PointValue::Float(f) => {
            out.push(0);
            out.extend_from_slice(&f.to_le_bytes());
        }
        PointValue::Int(i) => {
            out.push(1);
            out.extend_from_slice(&i.to_le_bytes());
        }
        PointValue::Bool(b) => {
            out.push(2);
            out.push(if *b { 1 } else { 0 });
        }
        PointValue::Enum(e) => {
            out.push(3);
            out.extend_from_slice(&e.to_le_bytes());
        }
        PointValue::String(s) => {
            out.push(4);
            encode_len_str(s, out)?;
        }
        PointValue::Null => {
            out.push(5);
        }
    }
    Ok(())
}

/// 点值 wire 解码。
pub fn decode_point_value(buf: &[u8]) -> Result<(PointValue, usize), DecodeError> {
    let (tag, rest) = buf.split_first().ok_or(DecodeError::InvalidPayload)?;
    match tag {
        0 => {
            if rest.len() < 8 {
                return Err(DecodeError::InvalidPayload);
            }
            // Reason: 前置长度校验（rest.len() 检查）保证切片长度恰好匹配，try_into 不可达失败（21-B D1 豁免）
            let f = f64::from_le_bytes(rest[0..8].try_into().unwrap());
            Ok((PointValue::Float(f), 9))
        }
        1 => {
            if rest.len() < 8 {
                return Err(DecodeError::InvalidPayload);
            }
            // Reason: 前置长度校验（rest.len() 检查）保证切片长度恰好匹配，try_into 不可达失败（21-B D1 豁免）
            let i = i64::from_le_bytes(rest[0..8].try_into().unwrap());
            Ok((PointValue::Int(i), 9))
        }
        2 => {
            if rest.is_empty() {
                return Err(DecodeError::InvalidPayload);
            }
            Ok((PointValue::Bool(rest[0] != 0), 2))
        }
        3 => {
            if rest.len() < 2 {
                return Err(DecodeError::InvalidPayload);
            }
            // Reason: 前置长度校验（rest.len() 检查）保证切片长度恰好匹配，try_into 不可达失败（21-B D1 豁免）
            let e = u16::from_le_bytes(rest[0..2].try_into().unwrap());
            Ok((PointValue::Enum(e), 3))
        }
        4 => {
            let (s, consumed) = decode_len_str(rest)?;
            Ok((PointValue::String(String::from(s)), 1 + consumed))
        }
        5 => Ok((PointValue::Null, 1)),
        _ => Err(DecodeError::InvalidPayload),
    }
}

// ============================================================================
// 语义消息层（ProtocolRequest / ProtocolResponse）
// ============================================================================

/// 协议插件请求（客户端 → EL0 协议服务进程）。
#[derive(Debug, Clone, PartialEq)]
pub enum ProtocolRequest {
    /// 按配置 ID 连接（配置由宿主侧持有）。
    Connect { config_id: u32 },
    /// 读单点。
    ReadPoint { point_id: PointId },
    /// 写单点。
    WritePoint {
        point_id: PointId,
        value: PointValue,
    },
    /// 断开。
    Disconnect,
}

/// 协议插件响应（服务进程 → 客户端）。
#[derive(Debug, Clone, PartialEq)]
pub enum ProtocolResponse {
    /// 操作成功（无载荷）。
    Ack,
    /// 读点结果。
    Point(DataPoint),
    /// 错误（错误码 + 消息）。
    Error { code: u16, message: String },
}

/// 请求编码错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MsgEncodeError {
    /// 载荷超出上界。
    PayloadTooLarge,
}

/// 将请求编码为完整 wire 帧（写入调用方缓冲）。
pub fn encode_request(req: &ProtocolRequest, out: &mut [u8]) -> Result<usize, MsgEncodeError> {
    let mut payload: Vec<u8> = Vec::new();
    match req {
        ProtocolRequest::Connect { config_id } => {
            payload.extend_from_slice(&config_id.to_le_bytes());
            frame(msg_type_of(req), &payload, out)
        }
        ProtocolRequest::ReadPoint { point_id } => {
            payload.extend_from_slice(&point_id.to_le_bytes());
            frame(msg_type_of(req), &payload, out)
        }
        ProtocolRequest::WritePoint { point_id, value } => {
            payload.extend_from_slice(&point_id.to_le_bytes());
            encode_point_value(value, &mut payload).map_err(|_| MsgEncodeError::PayloadTooLarge)?;
            frame(msg_type_of(req), &payload, out)
        }
        ProtocolRequest::Disconnect => frame(msg_type_of(req), &payload, out),
    }
}

fn msg_type_of(req: &ProtocolRequest) -> MsgType {
    match req {
        ProtocolRequest::Connect { .. } => MsgType::Connect,
        ProtocolRequest::ReadPoint { .. } => MsgType::ReadPoint,
        ProtocolRequest::WritePoint { .. } => MsgType::WritePoint,
        ProtocolRequest::Disconnect => MsgType::Disconnect,
    }
}

fn frame(msg_type: MsgType, payload: &[u8], out: &mut [u8]) -> Result<usize, MsgEncodeError> {
    encode_frame(msg_type, payload, out).map_err(|_| MsgEncodeError::PayloadTooLarge)
}

/// 将完整 wire 帧解码为请求。
pub fn decode_request(buf: &[u8]) -> Result<ProtocolRequest, DecodeError> {
    let f = decode_frame(buf)?;
    match f.msg_type {
        MsgType::Connect => {
            if f.payload.len() < 4 {
                return Err(DecodeError::InvalidPayload);
            }
            // Reason: 前置长度校验（payload 长度检查）保证切片长度恰好匹配，try_into 不可达失败（21-B D1 豁免）
            let config_id = u32::from_le_bytes(f.payload[0..4].try_into().unwrap());
            Ok(ProtocolRequest::Connect { config_id })
        }
        MsgType::ReadPoint => {
            if f.payload.len() < 4 {
                return Err(DecodeError::InvalidPayload);
            }
            // Reason: 前置长度校验（payload 长度检查）保证切片长度恰好匹配，try_into 不可达失败（21-B D1 豁免）
            let point_id = u32::from_le_bytes(f.payload[0..4].try_into().unwrap());
            Ok(ProtocolRequest::ReadPoint { point_id })
        }
        MsgType::WritePoint => {
            if f.payload.len() < 4 {
                return Err(DecodeError::InvalidPayload);
            }
            // Reason: 前置长度校验（payload 长度检查）保证切片长度恰好匹配，try_into 不可达失败（21-B D1 豁免）
            let point_id = u32::from_le_bytes(f.payload[0..4].try_into().unwrap());
            let (value, consumed) = decode_point_value(&f.payload[4..])?;
            if consumed != f.payload.len() - 4 {
                return Err(DecodeError::InvalidPayload);
            }
            Ok(ProtocolRequest::WritePoint { point_id, value })
        }
        MsgType::Disconnect => {
            if !f.payload.is_empty() {
                return Err(DecodeError::InvalidPayload);
            }
            Ok(ProtocolRequest::Disconnect)
        }
        _ => Err(DecodeError::UnknownMessageType),
    }
}

/// 将响应编码为完整 wire 帧。
pub fn encode_response(resp: &ProtocolResponse, out: &mut [u8]) -> Result<usize, MsgEncodeError> {
    let mut payload: Vec<u8> = Vec::new();
    let msg_type = match resp {
        ProtocolResponse::Ack => MsgType::Response,
        ProtocolResponse::Point(dp) => {
            encode_data_point(dp, &mut payload).map_err(|_| MsgEncodeError::PayloadTooLarge)?;
            MsgType::Response
        }
        ProtocolResponse::Error { code, message } => {
            payload.extend_from_slice(&code.to_le_bytes());
            encode_len_str(message, &mut payload).map_err(|_| MsgEncodeError::PayloadTooLarge)?;
            MsgType::Error
        }
    };
    frame(msg_type, &payload, out)
}

/// 将完整 wire 帧解码为响应。
pub fn decode_response(buf: &[u8]) -> Result<ProtocolResponse, DecodeError> {
    let f = decode_frame(buf)?;
    match f.msg_type {
        MsgType::Response => {
            if f.payload.is_empty() {
                Ok(ProtocolResponse::Ack)
            } else {
                let (dp, consumed) = decode_data_point(f.payload)?;
                if consumed != f.payload.len() {
                    return Err(DecodeError::InvalidPayload);
                }
                Ok(ProtocolResponse::Point(dp))
            }
        }
        MsgType::Error => {
            if f.payload.len() < 2 {
                return Err(DecodeError::InvalidPayload);
            }
            // Reason: 前置长度校验（payload 长度检查）保证切片长度恰好匹配，try_into 不可达失败（21-B D1 豁免）
            let code = u16::from_le_bytes(f.payload[0..2].try_into().unwrap());
            let (message, consumed) = decode_len_str(&f.payload[2..])?;
            if consumed != f.payload.len() - 2 {
                return Err(DecodeError::InvalidPayload);
            }
            Ok(ProtocolResponse::Error {
                code,
                message: String::from(message),
            })
        }
        _ => Err(DecodeError::UnknownMessageType),
    }
}

/// DataPoint wire 编码（point_id u32 + device_id u16 + name + value + quality u8
/// + timestamp u64 + source u8 + unit 长度前缀）。
pub fn encode_data_point(dp: &DataPoint, out: &mut Vec<u8>) -> Result<(), WireError> {
    out.extend_from_slice(&dp.point_id.to_le_bytes());
    out.extend_from_slice(&dp.device_id.to_le_bytes());
    encode_len_str(&dp.name, out)?;
    encode_point_value(&dp.value, out)?;
    out.push(encode_quality(dp.quality));
    out.extend_from_slice(&dp.timestamp_ms.to_le_bytes());
    out.push(dp.source as u8);
    match &dp.unit {
        Some(u) => encode_len_str(u, out)?,
        None => out.extend_from_slice(&0u16.to_le_bytes()),
    }
    Ok(())
}

/// DataPoint wire 解码。
pub fn decode_data_point(buf: &[u8]) -> Result<(DataPoint, usize), DecodeError> {
    let mut pos = 0usize;
    if buf.len() < 4 + 2 {
        return Err(DecodeError::InvalidPayload);
    }
    // Reason: 前置长度校验（payload 长度检查）保证切片长度恰好匹配，try_into 不可达失败（21-B D1 豁免）
    let point_id = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap());
    pos += 4;
    // Reason: 前置长度校验（payload 长度检查）保证切片长度恰好匹配，try_into 不可达失败（21-B D1 豁免）
    let device_id = u16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap());
    pos += 2;

    let (name, c) = decode_len_str(&buf[pos..])?;
    pos += c;
    let (value, c) = decode_point_value(&buf[pos..])?;
    pos += c;

    if buf.len() <= pos {
        return Err(DecodeError::InvalidPayload);
    }
    let quality = decode_quality(buf[pos]);
    pos += 1;

    if buf.len() < pos + 8 {
        return Err(DecodeError::InvalidPayload);
    }
    // Reason: 前置长度校验（payload 长度检查）保证切片长度恰好匹配，try_into 不可达失败（21-B D1 豁免）
    let timestamp_ms = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
    pos += 8;

    if buf.len() <= pos {
        return Err(DecodeError::InvalidPayload);
    }
    let source = match buf[pos] {
        0 => eneros_upa_model::DataSource::ModbusRtu,
        1 => eneros_upa_model::DataSource::ModbusTcp,
        2 => eneros_upa_model::DataSource::Iec104,
        3 => eneros_upa_model::DataSource::Can,
        4 => eneros_upa_model::DataSource::Internal,
        5 => eneros_upa_model::DataSource::Manual,
        _ => return Err(DecodeError::InvalidPayload),
    };
    pos += 1;

    let (unit, c) = decode_len_str(&buf[pos..])?;
    pos += c;

    let unit = if unit.is_empty() {
        None
    } else {
        Some(String::from(unit))
    };

    Ok((
        DataPoint {
            point_id,
            device_id,
            name: String::from(name),
            description: None,
            point_type: eneros_upa_model::PointType::Analog,
            value,
            quality,
            timestamp_ms,
            source,
            unit,
        },
        pos,
    ))
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use eneros_upa_model::{DataSource, PointQuality, PointType};

    use super::*;

    fn sample_point(name: &str, value: PointValue, q: PointQuality) -> DataPoint {
        DataPoint {
            point_id: 42,
            device_id: 7,
            name: String::from(name),
            description: None,
            point_type: PointType::Analog,
            value,
            quality: q,
            timestamp_ms: 1_700_000_000_000,
            source: DataSource::ModbusRtu,
            unit: Some(String::from("kW")),
        }
    }

    /// 帧层 round-trip：多种载荷长度（0/1/边界/超界）。
    #[test]
    fn frame_roundtrip_multi_payloads() {
        for (msg_type, payload) in [
            (MsgType::Connect, &[][..]),
            (MsgType::ReadPoint, &[1u8, 2, 3, 4][..]),
            (MsgType::WritePoint, &[0u8; 512][..]),
            (MsgType::Response, &[0xABu8; MAX_PAYLOAD_LEN][..]),
        ] {
            let mut buf = [0u8; MAX_WIRE_SIZE];
            let n = encode_frame(msg_type, payload, &mut buf).unwrap();
            assert_eq!(n, frame_size(payload.len()));
            let f = decode_frame(&buf[..n]).unwrap();
            assert_eq!(f.msg_type, msg_type);
            assert_eq!(f.payload, payload);
        }
    }

    /// 帧层畸形输入。
    #[test]
    fn frame_malformed_rejected() {
        // 截断头
        assert_eq!(
            decode_frame(&[0u8; HEADER_LEN + CRC_LEN - 1]),
            Err(DecodeError::TruncatedHeader)
        );
        // 坏 magic
        let mut buf = [0u8; MAX_WIRE_SIZE];
        let n = encode_frame(MsgType::ReadPoint, &[], &mut buf).unwrap();
        let mut bad = buf[..n].to_vec();
        bad[0] = 0x00;
        assert_eq!(decode_frame(&bad), Err(DecodeError::BadMagic));
        // 未知版本
        let mut bad = buf[..n].to_vec();
        bad[4] = 99;
        assert_eq!(decode_frame(&bad), Err(DecodeError::UnsupportedVersion));
        // 未知消息类型
        let mut bad = buf[..n].to_vec();
        bad[5] = 99;
        assert_eq!(decode_frame(&bad), Err(DecodeError::UnknownMessageType));
        // CRC 损坏
        let mut bad = buf[..n].to_vec();
        let last = bad.len() - 1;
        bad[last] ^= 0xFF;
        assert_eq!(decode_frame(&bad), Err(DecodeError::CrcMismatch));
        // 声明长度越界
        let mut bad = buf[..n].to_vec();
        bad[8..12].copy_from_slice(&(MAX_PAYLOAD_LEN as u32 + 1).to_le_bytes());
        assert_eq!(decode_frame(&bad), Err(DecodeError::LengthOutOfBounds));
        // payload 超界拒绝
        assert_eq!(
            encode_frame(MsgType::ReadPoint, &[0u8; MAX_PAYLOAD_LEN + 1], &mut buf),
            Err(WireError::PayloadTooLarge)
        );
        // 缓冲过小
        let mut small = [0u8; HEADER_LEN + CRC_LEN];
        assert_eq!(
            encode_frame(MsgType::ReadPoint, &[0u8; 16], &mut small),
            Err(WireError::BufferTooSmall)
        );
    }

    /// 请求 round-trip（全部变体）。
    #[test]
    fn request_roundtrip_all_variants() {
        let reqs = [
            ProtocolRequest::Connect { config_id: 7 },
            ProtocolRequest::ReadPoint {
                point_id: 0xDEAD_BEEF,
            },
            ProtocolRequest::WritePoint {
                point_id: 3,
                value: PointValue::Float(12.5),
            },
            ProtocolRequest::WritePoint {
                point_id: 4,
                value: PointValue::String(String::from("hello")),
            },
            ProtocolRequest::WritePoint {
                point_id: 5,
                value: PointValue::Null,
            },
            ProtocolRequest::Disconnect,
        ];
        for req in reqs.iter() {
            let mut buf = [0u8; MAX_WIRE_SIZE];
            let n = encode_request(req, &mut buf).unwrap();
            let decoded = decode_request(&buf[..n]).unwrap();
            assert_eq!(&decoded, req);
        }
    }

    /// 响应 round-trip（Ack / Point / Error）。
    #[test]
    fn response_roundtrip_all_variants() {
        let good = PointQuality::good();
        let dp = sample_point("U_a", PointValue::Float(230.5), good);
        let resp = [
            ProtocolResponse::Ack,
            ProtocolResponse::Point(dp.clone()),
            ProtocolResponse::Error {
                code: 5,
                message: String::from("timeout"),
            },
        ];
        for r in resp.iter() {
            let mut buf = [0u8; MAX_WIRE_SIZE];
            let n = encode_response(r, &mut buf).unwrap();
            let decoded = decode_response(&buf[..n]).unwrap();
            assert_eq!(&decoded, r);
        }
    }

    /// 属性式 round-trip：值类型 × 品质组合循环（不接受单一样例）。
    #[test]
    fn data_point_roundtrip_combinations() {
        let values = [
            PointValue::Float(-1.5e-9),
            PointValue::Int(i64::MIN),
            PointValue::Int(i64::MAX),
            PointValue::Bool(true),
            PointValue::Enum(u16::MAX),
            PointValue::String(String::from("状态字符串-中文")),
            PointValue::Null,
        ];
        let qualities = [
            PointQuality::good(),
            PointQuality::invalid(),
            {
                let mut q = PointQuality::good();
                q.questionable = true;
                q.substituted = true;
                q
            },
            PointQuality {
                valid: false,
                invalid: true,
                questionable: true,
                substituted: true,
                overflow: true,
                blocked: true,
                outdated: true,
            },
        ];
        let mut roundtrip_count = 0usize;
        for v in values.iter() {
            for q in qualities.iter() {
                let dp = sample_point("P", v.clone(), *q);
                let mut payload: Vec<u8> = Vec::new();
                encode_data_point(&dp, &mut payload).unwrap();
                let (decoded, consumed) = decode_data_point(&payload).unwrap();
                assert_eq!(consumed, payload.len());
                assert_eq!(decoded.point_id, dp.point_id);
                assert_eq!(decoded.device_id, dp.device_id);
                assert_eq!(decoded.name, dp.name);
                assert_eq!(decoded.value, dp.value);
                assert_eq!(decoded.quality, dp.quality);
                assert_eq!(decoded.timestamp_ms, dp.timestamp_ms);
                assert_eq!(decoded.source, dp.source);
                assert_eq!(decoded.unit, dp.unit);
                roundtrip_count += 1;
            }
        }
        assert!(roundtrip_count >= 28, "round-trip 组合数应 ≥ 28");
    }

    /// 语义层畸形输入。
    #[test]
    fn semantic_malformed_rejected() {
        // 空请求载荷
        let mut buf = [0u8; MAX_WIRE_SIZE];
        let n = encode_frame(MsgType::ReadPoint, &[], &mut buf).unwrap();
        assert_eq!(decode_request(&buf[..n]), Err(DecodeError::InvalidPayload));
        // WritePoint 载荷缺值
        let mut buf = [0u8; MAX_WIRE_SIZE];
        let n = encode_frame(MsgType::WritePoint, &[0u8; 4], &mut buf).unwrap();
        assert_eq!(decode_request(&buf[..n]), Err(DecodeError::InvalidPayload));
        // PointValue 未知 tag
        let mut bad = vec![99u8];
        bad.extend_from_slice(&[0u8; 8]);
        assert!(decode_point_value(&bad).is_err());
        // String 长度越界
        assert_eq!(
            decode_len_str(&[0xFF, 0xFF, 0x00]),
            Err(DecodeError::InvalidPayload)
        );
        // 非法 UTF-8 字符串
        assert_eq!(
            decode_len_str(&[0x01, 0x00, 0xFF]),
            Err(DecodeError::InvalidPayload)
        );
        // 质量位编码往返
        let q = PointQuality::good();
        assert_eq!(decode_quality(encode_quality(q)), q);
    }
}
