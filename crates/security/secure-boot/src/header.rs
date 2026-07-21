//! 镜像签名头（v0.113.0，D7/D11：全固定字段 118B 可 Copy，零 serde 二进制编解码）.
//!
//! 帧布局（全小端，共 [`HEADER_LEN`] = 118 字节）：
//!
//! | 偏移 | 长度 | 字段 | 说明 |
//! |------|------|------|------|
//! | 0    | 4    | magic      | 固定 "ESIG" |
//! | 4    | 2    | version    | 帧格式版本，当前仅 1 |
//! | 6    | 8    | image_size | 镜像字节数（u64 小端） |
//! | 14   | 32   | image_hash | 镜像 SM3 哈希 |
//! | 46   | 64   | signature  | SM2 签名（r ‖ s） |
//! | 110  | 8    | timestamp  | 签名时间戳（u64 小端，防降级用） |

use crate::BootError;

/// 镜像签名头长度（字节）.
pub const HEADER_LEN: usize = 118;

/// 镜像签名头魔数（"ESIG"）.
const MAGIC: [u8; 4] = *b"ESIG";

/// 签名头帧格式版本（当前仅支持 1）.
const VERSION: u16 = 1;

/// 镜像签名头（D7：全固定字段共 118 字节，可 Copy）.
///
/// 信任锚为构造注入的信任根公钥，证书链验证归 v0.32.0 PKI 层职责，
/// 本结构不携带签名者证书（蓝图 `signer_cert: Vec<u8>` 已删除）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageSignature {
    /// 魔数，必须为 `b"ESIG"`.
    pub magic: [u8; 4],
    /// 帧格式版本，必须为 1.
    pub version: u16,
    /// 镜像字节数（验签时与实际镜像长度比对，防截断）.
    pub image_size: u64,
    /// 镜像 SM3 哈希（32 字节）.
    pub image_hash: [u8; 32],
    /// SM2 签名值（64 字节，r ‖ s）.
    pub signature: [u8; 64],
    /// 签名时间戳（防降级下限校验用，由集成层与熔丝值比对）.
    pub timestamp: u64,
}

/// 将镜像签名头编码为 118 字节帧（全小端）.
pub fn encode_header(sig: &ImageSignature) -> [u8; HEADER_LEN] {
    let mut out = [0u8; HEADER_LEN];
    out[0..4].copy_from_slice(&sig.magic);
    out[4..6].copy_from_slice(&sig.version.to_le_bytes());
    out[6..14].copy_from_slice(&sig.image_size.to_le_bytes());
    out[14..46].copy_from_slice(&sig.image_hash);
    out[46..110].copy_from_slice(&sig.signature);
    out[110..118].copy_from_slice(&sig.timestamp.to_le_bytes());
    out
}

/// 从字节流解码镜像签名头.
///
/// - 输入长度 < [`HEADER_LEN`] → `Err(InvalidHeader)`
/// - magic 非 "ESIG" → `Err(InvalidMagic)`
/// - version 非 1 → `Err(UnsupportedVersion)`
pub fn decode_header(bytes: &[u8]) -> Result<ImageSignature, BootError> {
    if bytes.len() < HEADER_LEN {
        return Err(BootError::InvalidHeader);
    }
    let mut magic = [0u8; 4];
    magic.copy_from_slice(&bytes[0..4]);
    if magic != MAGIC {
        return Err(BootError::InvalidMagic);
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != VERSION {
        return Err(BootError::UnsupportedVersion);
    }
    let mut image_size = [0u8; 8];
    image_size.copy_from_slice(&bytes[6..14]);
    let mut image_hash = [0u8; 32];
    image_hash.copy_from_slice(&bytes[14..46]);
    let mut signature = [0u8; 64];
    signature.copy_from_slice(&bytes[46..110]);
    let mut timestamp = [0u8; 8];
    timestamp.copy_from_slice(&bytes[110..118]);
    Ok(ImageSignature {
        magic,
        version,
        image_size: u64::from_le_bytes(image_size),
        image_hash,
        signature,
        timestamp: u64::from_le_bytes(timestamp),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造全字段非零的测试用签名头.
    fn sample_header() -> ImageSignature {
        let mut image_hash = [0u8; 32];
        for (i, b) in image_hash.iter_mut().enumerate() {
            *b = i as u8;
        }
        let mut signature = [0u8; 64];
        for (i, b) in signature.iter_mut().enumerate() {
            *b = 0xA5 ^ i as u8;
        }
        ImageSignature {
            magic: *b"ESIG",
            version: 1,
            image_size: 0x0102_0304_0506_0708,
            image_hash,
            signature,
            timestamp: 0x1122_3344_5566_7788,
        }
    }

    /// HDR1 编解码往返：全字段 encode → decode 逐字段相等.
    #[test]
    fn hdr1_encode_decode_roundtrip() {
        let header = sample_header();
        let encoded = encode_header(&header);
        assert_eq!(encoded.len(), HEADER_LEN);
        let decoded = decode_header(&encoded).unwrap();
        assert_eq!(decoded, header);
    }

    /// HDR2 坏 magic → Err(InvalidMagic).
    #[test]
    fn hdr2_bad_magic_rejected() {
        let mut header = sample_header();
        header.magic = *b"BSIG";
        let encoded = encode_header(&header);
        assert_eq!(decode_header(&encoded), Err(BootError::InvalidMagic));
    }

    /// HDR3 version != 1 → Err(UnsupportedVersion).
    #[test]
    fn hdr3_unsupported_version_rejected() {
        let mut header = sample_header();
        header.version = 2;
        let encoded = encode_header(&header);
        assert_eq!(decode_header(&encoded), Err(BootError::UnsupportedVersion));
    }

    /// HDR4 截断输入（117B / 空输入）→ Err(InvalidHeader).
    #[test]
    fn hdr4_truncated_input_rejected() {
        let encoded = encode_header(&sample_header());
        assert_eq!(
            decode_header(&encoded[..HEADER_LEN - 1]),
            Err(BootError::InvalidHeader)
        );
        assert_eq!(decode_header(&[]), Err(BootError::InvalidHeader));
    }

    /// HDR5 HEADER_LEN 常量 == 118（帧布局静态保证）.
    #[test]
    fn hdr5_header_len_constant() {
        assert_eq!(HEADER_LEN, 118);
        // 布局字段宽度之和 == HEADER_LEN
        assert_eq!(4 + 2 + 8 + 32 + 64 + 8, HEADER_LEN);
    }
}
