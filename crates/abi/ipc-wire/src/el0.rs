//! EL0 服务 IPC 帧（KRN-002）：root-task ↔ EL0 服务消息帧。
//!
//! 复用 `eneros-ipc-wire` 共享层（ADR-0025 / ARCH-S04）：帧完整性用
//! `crc32`（IEEE 802.3，全仓唯一实现），版本握手用 `WIRE_VERSION`。
//! 帧布局（与 agent-sdk EIPC 同族，root-task 侧 EL0 控制帧）：
//!
//! ```text
//! [0..4)   magic  "E0FR"
//! [4..6)   frame type (u16 LE)
//! [6..10)  payload len (u32 LE)
//! [10..)   payload
//! [尾 4B)  crc32(payload)（不含头）
//! ```

//! root-task ↔ EL0 服务 IPC 帧（KRN-002/003）：帧完整性 crc32 + 版本 WIRE_VERSION，由 services/root-task 共用。
use crate::{crc32, WIRE_VERSION};

/// EL0 帧魔数。
pub const EL0_FRAME_MAGIC: [u8; 4] = *b"E0FR";

/// EL0 帧头长（不含 crc 尾）。
pub const EL0_FRAME_HEADER_LEN: usize = 10;
/// CRC 尾长（ipc-wire 共享常量）。
pub const EL0_FRAME_CRC_LEN: usize = crate::CRC_LEN;

/// EL0 帧类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum El0FrameType {
    /// root-task → 服务：spawn 确认 / 心跳
    Control = 1,
    /// 服务 → root-task：就绪上报
    Ready = 2,
    /// 服务 → root-task：数据上报（含 config 请求，KRN-004）
    Report = 3,
}

impl El0FrameType {
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            1 => Some(Self::Control),
            2 => Some(Self::Ready),
            3 => Some(Self::Report),
            _ => None,
        }
    }
}

/// 帧编码/解码错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum El0FrameError {
    BadMagic,
    BadType,
    BadVersion,
    BadLength,
    BadCrc,
    TooLong,
}

/// 解码后的 EL0 帧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct El0Frame<'a> {
    pub frame_type: El0FrameType,
    pub payload: &'a [u8],
}

/// 编码一帧：`[magic][type][len][payload][crc]`。
pub fn encode_frame(frame_type: El0FrameType, payload: &[u8]) -> alloc::vec::Vec<u8> {
    let mut buf = alloc::vec![0u8; EL0_FRAME_HEADER_LEN + payload.len() + EL0_FRAME_CRC_LEN];
    buf[0..4].copy_from_slice(&EL0_FRAME_MAGIC);
    buf[4..6].copy_from_slice(&(frame_type as u16).to_le_bytes());
    buf[6..10].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    buf[10..10 + payload.len()].copy_from_slice(payload);
    let crc = crc32(payload);
    let tail = 10 + payload.len();
    buf[tail..tail + 4].copy_from_slice(&crc.to_le_bytes());
    buf
}

/// 解码并校验一帧（含 crc 与长度检查）。
pub fn decode_frame<'a>(buf: &'a [u8]) -> Result<El0Frame<'a>, El0FrameError> {
    if buf.len() < EL0_FRAME_HEADER_LEN + EL0_FRAME_CRC_LEN {
        return Err(El0FrameError::BadLength);
    }
    if buf[0..4] != EL0_FRAME_MAGIC {
        return Err(El0FrameError::BadMagic);
    }
    let frame_type = El0FrameType::from_u16(u16::from_le_bytes([buf[4], buf[5]]))
        .ok_or(El0FrameError::BadType)?;
    let payload_len = u32::from_le_bytes([buf[6], buf[7], buf[8], buf[9]]) as usize;
    if payload_len > crate::DEFAULT_MAX_PAYLOAD_LEN {
        return Err(El0FrameError::TooLong);
    }
    if buf.len() != EL0_FRAME_HEADER_LEN + payload_len + EL0_FRAME_CRC_LEN {
        return Err(El0FrameError::BadLength);
    }
    let payload = &buf[EL0_FRAME_HEADER_LEN..EL0_FRAME_HEADER_LEN + payload_len];
    let crc_expected = u32::from_le_bytes([
        buf[EL0_FRAME_HEADER_LEN + payload_len],
        buf[EL0_FRAME_HEADER_LEN + payload_len + 1],
        buf[EL0_FRAME_HEADER_LEN + payload_len + 2],
        buf[EL0_FRAME_HEADER_LEN + payload_len + 3],
    ]);
    if crc32(payload) != crc_expected {
        return Err(El0FrameError::BadCrc);
    }
    let _ = WIRE_VERSION; // 版本发现常量（未来握手使用）
    Ok(El0Frame {
        frame_type,
        payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_krn002_frame_roundtrip() {
        let payload = b"hello-el0";
        let buf = encode_frame(El0FrameType::Ready, payload);
        let frame = decode_frame(&buf).expect("roundtrip must decode");
        assert_eq!(frame.frame_type, El0FrameType::Ready);
        assert_eq!(frame.payload, payload);
    }

    #[test]
    fn test_krn002_frame_bad_crc_rejected() {
        let mut buf = encode_frame(El0FrameType::Report, b"data");
        let n = buf.len();
        buf[n - 1] ^= 0xff; // 破坏 crc 尾
        assert_eq!(decode_frame(&buf), Err(El0FrameError::BadCrc));
    }

    #[test]
    fn test_krn002_frame_bad_magic_rejected() {
        let mut buf = encode_frame(El0FrameType::Control, b"x");
        buf[0] = b'X';
        assert_eq!(decode_frame(&buf), Err(El0FrameError::BadMagic));
    }

    #[test]
    fn test_krn002_frame_unknown_type_rejected() {
        let mut buf = encode_frame(El0FrameType::Control, b"x");
        buf[4..6].copy_from_slice(&99u16.to_le_bytes());
        assert_eq!(decode_frame(&buf), Err(El0FrameError::BadType));
    }

    #[test]
    fn test_krn002_frame_oversize_rejected() {
        let mut buf = encode_frame(El0FrameType::Report, b"x");
        // 伪造超大 payload 长度
        buf[6..10].copy_from_slice(&(u32::MAX).to_le_bytes());
        assert_eq!(decode_frame(&buf), Err(El0FrameError::TooLong));
    }

    #[test]
    fn test_krn002_frame_truncated_rejected() {
        let buf = encode_frame(El0FrameType::Ready, b"abc");
        assert_eq!(
            decode_frame(&buf[..buf.len() - 2]),
            Err(El0FrameError::BadLength)
        );
    }
}
