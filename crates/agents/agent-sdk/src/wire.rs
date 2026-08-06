//! IPC wire codec (AGT-006).
//!
//! MASTER §G6 (L2065): "stable IPC wire ABI、真实 SDK syscalls."
//! MASTER §G6 禁止捷径 (L2085): "SDK send 只增加计数."
//!
//! This module defines the **binary wire format** for all Agent-to-Agent
//! and Agent-to-Service IPC. Every message is encoded as a fixed-header +
//! variable payload + CRC32 trailer. The codec is length-bounded,
//! no-alloc on the encode path (writes into a caller-provided buffer),
//! and rejects malformed input on decode.
//!
//! # Wire format
//!
//! ```text
//! +--------+--------+--------+--------+--------+--------+--------+--------+
//! | 0x45   | 0x49   | 0x50   | 0x43   | ver    | type   | flags  | rsvd   |
//! +--------+--------+--------+--------+--------+--------+--------+--------+
//! | target_len (u16 LE)              | payload_len (u32 LE)              |
//! +--------+--------+--------+--------+--------+--------+--------+--------+
//! | target_name (variable, 0..=64 bytes, NUL-padded to target_len)       |
//! +--------+--------+--------+--------+--------+--------+--------+--------+
//! | payload (variable, 0..=4096 bytes)                                    |
//! +--------+--------+--------+--------+--------+--------+--------+--------+
//! | crc32 (u32 LE, over header + target + payload)                       |
//! +--------+--------+--------+--------+--------+--------+--------+--------+
//! ```
//!
//! Total max size: 8 (magic+ver+type+flags+rsvd) + 6 (lengths) + 64 (target)
//!               + 4096 (payload) + 4 (crc) = 4178 bytes.
//!
//! # no_std
//!
//! Pure `core`, no `alloc`. CRC32 uses a software table-free implementation.

#![allow(dead_code)]

use core::convert::TryInto;

/// Magic bytes: "EIPC" (0x45 0x49 0x50 0x43).
pub const MAGIC: [u8; 4] = [0x45, 0x49, 0x50, 0x43];

/// Current wire protocol version.
pub const WIRE_VERSION: u8 = 1;

/// Fixed header size (magic + version + type + flags + reserved + target_len + payload_len).
pub const HEADER_LEN: usize = 16;

/// CRC32 trailer size.
pub const CRC_LEN: usize = 4;

/// Maximum target name length.
pub const MAX_TARGET_LEN: usize = 64;

/// Maximum payload length.
pub const MAX_PAYLOAD_LEN: usize = 4096;

/// Maximum total wire message size.
pub const MAX_WIRE_SIZE: usize = HEADER_LEN + MAX_TARGET_LEN + MAX_PAYLOAD_LEN + CRC_LEN;

/// Message type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    /// Fire-and-forget send.
    Send = 1,
    /// Request expecting a response.
    Request = 2,
    /// Response to a previous Request.
    Response = 3,
    /// Cancel a pending Request.
    Cancel = 4,
    /// Agent lifecycle event (spawn/stop/crash notification).
    Event = 5,
}

impl MessageType {
    /// Convert from raw u8.
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            1 => Some(MessageType::Send),
            2 => Some(MessageType::Request),
            3 => Some(MessageType::Response),
            4 => Some(MessageType::Cancel),
            5 => Some(MessageType::Event),
            _ => None,
        }
    }
}

/// Message flags (bitfield).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MessageFlags(pub u8);

impl MessageFlags {
    pub const NONE: MessageFlags = MessageFlags(0);
    /// High-priority message (preempts queue).
    pub const HIGH_PRIORITY: MessageFlags = MessageFlags(0x01);
    /// Encrypted payload.
    pub const ENCRYPTED: MessageFlags = MessageFlags(0x02);

    pub fn contains(self, other: MessageFlags) -> bool {
        (self.0 & other.0) == other.0
    }
}

/// Wire encode error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// Target name too long (max 64 bytes).
    TargetTooLong,
    /// Payload too large (max 4096 bytes).
    PayloadTooLarge,
    /// Output buffer too small.
    BufferTooSmall,
    /// Target name contains interior NUL.
    InvalidTarget,
}

/// Encode an IPC message into `out`. Returns the number of bytes written.
///
/// The caller provides the output buffer; this function performs zero
/// allocations. On error, `out` is left in an unspecified state.
pub fn encode(
    msg_type: MessageType,
    flags: MessageFlags,
    target: &str,
    payload: &[u8],
    out: &mut [u8],
) -> Result<usize, WireError> {
    let target_bytes = target.as_bytes();
    if target_bytes.len() > MAX_TARGET_LEN {
        return Err(WireError::TargetTooLong);
    }
    if payload.len() > MAX_PAYLOAD_LEN {
        return Err(WireError::PayloadTooLarge);
    }
    if target_bytes.contains(&0) {
        return Err(WireError::InvalidTarget);
    }

    let target_len = target_bytes.len() as u16;
    let payload_len = payload.len() as u32;
    let total_len = HEADER_LEN + target_bytes.len() + payload.len() + CRC_LEN;
    if out.len() < total_len {
        return Err(WireError::BufferTooSmall);
    }

    // Header.
    out[0..4].copy_from_slice(&MAGIC);
    out[4] = WIRE_VERSION;
    out[5] = msg_type as u8;
    out[6] = flags.0;
    out[7] = 0; // reserved
    out[8..10].copy_from_slice(&target_len.to_le_bytes());
    out[10..14].copy_from_slice(&payload_len.to_le_bytes());
    out[14..16].copy_from_slice(&[0u8; 2]); // reserved

    // Target name.
    let target_offset = HEADER_LEN;
    out[target_offset..target_offset + target_bytes.len()].copy_from_slice(target_bytes);

    // Payload.
    let payload_offset = target_offset + target_bytes.len();
    out[payload_offset..payload_offset + payload.len()].copy_from_slice(payload);

    // CRC32 over header + target + payload (everything before CRC).
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
    /// Declared lengths exceed bounds.
    LengthOutOfBounds,
    /// CRC32 mismatch (corrupted message).
    CrcMismatch,
}

/// Decoded IPC message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedMessage<'a> {
    pub msg_type: MessageType,
    pub flags: MessageFlags,
    pub target: &'a [u8],
    pub payload: &'a [u8],
}

/// Decode an IPC message from `buf`. Returns a borrowed view (zero-copy).
pub fn decode(buf: &[u8]) -> Result<DecodedMessage<'_>, DecodeError> {
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

    let msg_type = MessageType::from_u8(buf[5]).ok_or(DecodeError::UnknownMessageType)?;
    let flags = MessageFlags(buf[6]);

    let target_len = u16::from_le_bytes(buf[8..10].try_into().unwrap()) as usize;
    let payload_len = u32::from_le_bytes(buf[10..14].try_into().unwrap()) as usize;

    if target_len > MAX_TARGET_LEN || payload_len > MAX_PAYLOAD_LEN {
        return Err(DecodeError::LengthOutOfBounds);
    }

    let total_len = HEADER_LEN + target_len + payload_len + CRC_LEN;
    if buf.len() < total_len {
        return Err(DecodeError::LengthOutOfBounds);
    }

    // Verify CRC32.
    let crc_offset = HEADER_LEN + target_len + payload_len;
    let expected_crc = u32::from_le_bytes(buf[crc_offset..crc_offset + 4].try_into().unwrap());
    let actual_crc = crc32(&buf[..crc_offset]);
    if actual_crc != expected_crc {
        return Err(DecodeError::CrcMismatch);
    }

    let target = &buf[HEADER_LEN..HEADER_LEN + target_len];
    let payload = &buf[HEADER_LEN + target_len..HEADER_LEN + target_len + payload_len];

    Ok(DecodedMessage {
        msg_type,
        flags,
        target,
        payload,
    })
}

/// Software CRC32（共享实现：`eneros-ipc-wire::crc32`，ARCH-S04）。
///
/// 仅用于帧完整性；认证由传输层（HMAC/AEAD）负责，非加密 MAC。
/// 本函数为兼容导出，转发到共享 crate。
pub fn crc32(data: &[u8]) -> u32 {
    eneros_ipc_wire::crc32(data)
}

/// Compute the exact encoded size for a given target + payload.
pub fn encoded_size(target: &str, payload: &[u8]) -> usize {
    HEADER_LEN + target.len() + payload.len() + CRC_LEN
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    #[test]
    fn test_encode_decode_roundtrip() {
        let mut buf = [0u8; MAX_WIRE_SIZE];
        let n = encode(
            MessageType::Send,
            MessageFlags::NONE,
            "agent-42",
            b"hello world",
            &mut buf,
        )
        .unwrap();

        let msg = decode(&buf[..n]).unwrap();
        assert_eq!(msg.msg_type, MessageType::Send);
        assert_eq!(msg.flags, MessageFlags::NONE);
        assert_eq!(msg.target, b"agent-42");
        assert_eq!(msg.payload, b"hello world");
    }

    #[test]
    fn test_encode_decode_request_response() {
        let mut buf = [0u8; MAX_WIRE_SIZE];

        // Request.
        let n = encode(
            MessageType::Request,
            MessageFlags::HIGH_PRIORITY,
            "solver-agent",
            b"solve LP: max x+y",
            &mut buf,
        )
        .unwrap();
        let msg = decode(&buf[..n]).unwrap();
        assert_eq!(msg.msg_type, MessageType::Request);
        assert!(msg.flags.contains(MessageFlags::HIGH_PRIORITY));
        assert_eq!(msg.target, b"solver-agent");

        // Response.
        let n = encode(
            MessageType::Response,
            MessageFlags::NONE,
            "requester",
            b"optimal: x=5, y=0, obj=5",
            &mut buf,
        )
        .unwrap();
        let msg = decode(&buf[..n]).unwrap();
        assert_eq!(msg.msg_type, MessageType::Response);
        assert_eq!(msg.payload, b"optimal: x=5, y=0, obj=5");
    }

    #[test]
    fn test_empty_payload() {
        let mut buf = [0u8; MAX_WIRE_SIZE];
        let n = encode(
            MessageType::Event,
            MessageFlags::NONE,
            "supervisor",
            &[],
            &mut buf,
        )
        .unwrap();
        let msg = decode(&buf[..n]).unwrap();
        assert!(msg.payload.is_empty());
    }

    #[test]
    fn test_max_payload() {
        let payload = [0xABu8; MAX_PAYLOAD_LEN];
        let mut buf = [0u8; MAX_WIRE_SIZE];
        let n = encode(
            MessageType::Send,
            MessageFlags::NONE,
            "t",
            &payload,
            &mut buf,
        )
        .unwrap();
        let msg = decode(&buf[..n]).unwrap();
        assert_eq!(msg.payload.len(), MAX_PAYLOAD_LEN);
        assert_eq!(msg.payload[0], 0xAB);
    }

    #[test]
    fn test_target_too_long() {
        let long_target = "a".repeat(MAX_TARGET_LEN + 1);
        let mut buf = [0u8; MAX_WIRE_SIZE];
        let err = encode(
            MessageType::Send,
            MessageFlags::NONE,
            &long_target,
            b"hi",
            &mut buf,
        )
        .unwrap_err();
        assert_eq!(err, WireError::TargetTooLong);
    }

    #[test]
    fn test_payload_too_large() {
        let big_payload = vec![0u8; MAX_PAYLOAD_LEN + 1];
        let mut buf = [0u8; MAX_WIRE_SIZE];
        let err = encode(
            MessageType::Send,
            MessageFlags::NONE,
            "t",
            &big_payload,
            &mut buf,
        )
        .unwrap_err();
        assert_eq!(err, WireError::PayloadTooLarge);
    }

    #[test]
    fn test_buffer_too_small() {
        let mut buf = [0u8; 10]; // way too small
        let err = encode(MessageType::Send, MessageFlags::NONE, "t", b"hi", &mut buf).unwrap_err();
        assert_eq!(err, WireError::BufferTooSmall);
    }

    #[test]
    fn test_bad_magic_rejected() {
        let mut buf = [0u8; MAX_WIRE_SIZE];
        let n = encode(MessageType::Send, MessageFlags::NONE, "t", b"hi", &mut buf).unwrap();
        // Corrupt magic.
        buf[0] = 0x00;
        let err = decode(&buf[..n]).unwrap_err();
        assert_eq!(err, DecodeError::BadMagic);
    }

    #[test]
    fn test_crc_corruption_rejected() {
        let mut buf = [0u8; MAX_WIRE_SIZE];
        let n = encode(MessageType::Send, MessageFlags::NONE, "t", b"hi", &mut buf).unwrap();
        // Corrupt payload (flip a bit after header).
        buf[HEADER_LEN] ^= 0xFF;
        let err = decode(&buf[..n]).unwrap_err();
        assert_eq!(err, DecodeError::CrcMismatch);
    }

    #[test]
    fn test_truncated_header_rejected() {
        let short = [0u8; 5];
        let err = decode(&short).unwrap_err();
        assert_eq!(err, DecodeError::TruncatedHeader);
    }

    #[test]
    fn test_unsupported_version_rejected() {
        let mut buf = [0u8; MAX_WIRE_SIZE];
        let n = encode(MessageType::Send, MessageFlags::NONE, "t", b"hi", &mut buf).unwrap();
        // Bump version.
        buf[4] = 99;
        let err = decode(&buf[..n]).unwrap_err();
        assert_eq!(err, DecodeError::UnsupportedVersion);
    }

    #[test]
    fn test_unknown_message_type_rejected() {
        let mut buf = [0u8; MAX_WIRE_SIZE];
        let n = encode(MessageType::Send, MessageFlags::NONE, "t", b"hi", &mut buf).unwrap();
        // Set unknown message type.
        buf[5] = 0xFF;
        let err = decode(&buf[..n]).unwrap_err();
        assert_eq!(err, DecodeError::UnknownMessageType);
    }

    #[test]
    fn test_crc32_known_values() {
        // CRC32 of empty string.
        assert_eq!(crc32(b""), 0x0000_0000);
        // CRC32 of "123456789" (standard IEEE 802.3 check value).
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn test_encoded_size() {
        // "agent" = 5 bytes, "payload" = 7 bytes.
        let size = encoded_size("agent", b"payload");
        assert_eq!(size, HEADER_LEN + 5 + 7 + CRC_LEN);
    }

    #[test]
    fn test_invalid_target_nul_rejected() {
        let mut buf = [0u8; MAX_WIRE_SIZE];
        let err = encode(
            MessageType::Send,
            MessageFlags::NONE,
            "agent\0bad",
            b"hi",
            &mut buf,
        )
        .unwrap_err();
        assert_eq!(err, WireError::InvalidTarget);
    }

    #[test]
    fn test_encrypted_flag() {
        let mut buf = [0u8; MAX_WIRE_SIZE];
        let n = encode(
            MessageType::Send,
            MessageFlags::ENCRYPTED,
            "t",
            b"secret",
            &mut buf,
        )
        .unwrap();
        let msg = decode(&buf[..n]).unwrap();
        assert!(msg.flags.contains(MessageFlags::ENCRYPTED));
    }
}
