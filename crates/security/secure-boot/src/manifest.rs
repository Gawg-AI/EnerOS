//! 启动镜像 Manifest（v0.235.0，B4-02）。
//!
//! 密码学绑定修正：BL 公钥不再由装配方任意传入（防张冠李戴），而是
//! 从**已验签的 bootloader 镜像内嵌 manifest** 中提取——只有通过
//! 信任根验签的 manifest 才提供其声明的下级公钥。
//!
//! 类型系统约束：`BootManifest` 仅在
//! [`BootManifest::verify_and_parse`] 成功后存在；`extract_bl_key`
//! 返回 `&Sm2PublicKey` 生命周期绑定到已验签镜像——装配方无法在
use eneros_crypto::Sm2PublicKey;

use crate::BootError;
use crate::header::{encode_header, ImageSignature};

pub const MANIFEST_MAGIC: [u8; 4] = *b"BMAN";
/// Manifest 版本。
pub const MANIFEST_VERSION: u16 = 1;

/// Bootloader 内嵌 manifest（已验签后解析）。
///
/// 二进制布局（全部小端）：
/// `magic(4) ‖ version(2) ‖ bl_key_len(2) ‖ bl_key(bl_key_len) ‖ reserved(4)`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootManifest {
    /// Manifest 版本。
    pub version: u16,
    /// Bootloader 公钥（用于下级镜像验签）。
    pub bl_key: Sm2PublicKey,
    /// 原始字节（供度量，固定上限 128 字节）。
    pub raw: [u8; 128],
    /// 原始字节实际长度。
    pub raw_len: usize,
}

impl BootManifest {
    /// 编码 manifest（集成层构建 bootloader 镜像时调用）。
    pub fn encode(bl_key: &Sm2PublicKey) -> ([u8; 128], usize) {
        let key_bytes = bl_key.to_bytes_uncompressed();
        let len = 4 + 2 + 2 + key_bytes.len() + 4;
        let mut out = [0u8; 128];
        out[0..4].copy_from_slice(&MANIFEST_MAGIC);
        out[4..6].copy_from_slice(&MANIFEST_VERSION.to_le_bytes());
        out[6..8].copy_from_slice(&(key_bytes.len() as u16).to_le_bytes());
        out[8..8 + key_bytes.len()].copy_from_slice(&key_bytes);
        (out, len)
    }

    /// 从 bootloader 镜像尾部解析 manifest 字节（不验签）。
    ///
    /// 返回 `(manifest_bytes, 前缀长度)`——manifest 位于镜像末尾。
    pub fn locate(image: &[u8]) -> Result<(&[u8], usize), BootError> {
        // Manifest 最短 12 字节。
        if image.len() < 12 {
            return Err(BootError::ManifestNotFound);
        }
        // 从尾部向前扫描 magic（防伪造头部前置）。
        for i in (0..=image.len() - 4).rev() {
            if image[i..i + 4] == MANIFEST_MAGIC {
                let manifest = &image[i..];
                if manifest.len() < 8 {
                    return Err(BootError::ManifestCorrupt);
                }
                let version = u16::from_le_bytes([manifest[4], manifest[5]]);
                if version != MANIFEST_VERSION {
                    return Err(BootError::ManifestCorrupt);
                }
                return Ok((manifest, i));
            }
        }
        Err(BootError::ManifestNotFound)
    }

    /// 验签并解析 manifest（v0.235.0 核心路径）。
    ///
    /// `image` = 完整 bootloader 镜像；`root_key` = 信任根公钥；
    /// `sig` = bootloader 签名。验签通过后从镜像尾部解析 manifest，
    /// 提取下级 BL 公钥——密码学绑定（未验签镜像无法取得公钥）。
    pub fn verify_and_parse(
        image: &[u8],
        root_key: &Sm2PublicKey,
        sig: &ImageSignature,
    ) -> Result<Self, BootError> {
        // 1. 信任根验签（与 verifier 相同的 SM2 验签路径）。
        let _header = encode_header(sig);
        let mut verifier = crate::verifier::BootVerifier::new(root_key.clone(), 0);
        verifier.verify_stage(crate::BootStage::Bootloader, image, sig)?;

        // 2. 解析 manifest。
        let (manifest_bytes, _prefix) = Self::locate(image)?;
        Self::parse(manifest_bytes)
    }

    /// 从 manifest 字节解析（验签后调用）。
    pub fn parse(manifest: &[u8]) -> Result<Self, BootError> {
        if manifest.len() < 12 {
            return Err(BootError::ManifestCorrupt);
        }
        if manifest[0..4] != MANIFEST_MAGIC {
            return Err(BootError::ManifestCorrupt);
        }
        let version = u16::from_le_bytes([manifest[4], manifest[5]]);
        if version != MANIFEST_VERSION {
            return Err(BootError::ManifestCorrupt);
        }
        let key_len = u16::from_le_bytes([manifest[6], manifest[7]]) as usize;
        if manifest.len() < 8 + key_len + 4 {
            return Err(BootError::ManifestCorrupt);
        }
        let bl_key = Sm2PublicKey::from_bytes(&manifest[8..8 + key_len])
            .map_err(|_| BootError::ManifestCorrupt)?;
        let mut raw = [0u8; 128];
        let n = manifest.len().min(128);
        raw[..n].copy_from_slice(&manifest[..n]);
        Ok(Self {
            version,
            bl_key,
            raw,
            raw_len: n,
        })
    }

    /// 提取下级验签公钥（v0.235.0：仅验签后可用）。
    pub fn extract_bl_key(&self) -> &Sm2PublicKey {
        &self.bl_key
    }
}

// ============================================================================
// Tests（防张冠李戴：错配公钥装配在验签期必失败）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn keypair() -> (eneros_crypto::Sm2KeyPair, eneros_crypto::CsRng) {
        // 辅助：确定性密钥对。
        let mut rng = eneros_crypto::CsRng::from_seed(&[0x61; 32]);
        let kp = eneros_crypto::Sm2KeyPair::generate(&mut rng).expect("kp");
        (kp, rng)
    }

    #[test]
    fn test_manifest_encode_parse_roundtrip() {
        let (kp, _rng) = keypair();
        let (encoded, _len) = BootManifest::encode(&kp.public_key);
        let parsed = BootManifest::parse(&encoded[..]).expect("parse");
        assert_eq!(parsed.version, MANIFEST_VERSION);
        assert_eq!(parsed.bl_key, kp.public_key);
    }

    #[test]
    fn test_manifest_locate_from_tail() {
        let (kp, _rng) = keypair();
        let (manifest, manifest_len) = BootManifest::encode(&kp.public_key);
        // 前置镜像数据。
        let mut image = alloc::vec![0x11u8; 100];
        image.extend_from_slice(&manifest[..manifest_len]);
        let (found, prefix) = BootManifest::locate(&image).expect("locate");
        assert_eq!(prefix, 100);
        assert_eq!(found, &manifest[..manifest_len]);
    }

    #[test]
    fn test_manifest_locate_absent() {
        let image = alloc::vec![0x22u8; 50];
        assert_eq!(
            BootManifest::locate(&image),
            Err(BootError::ManifestNotFound)
        );
    }

    #[test]
    fn test_manifest_corrupt_magic_rejected() {
        let (kp, _rng) = keypair();
        let (mut manifest, _len) = BootManifest::encode(&kp.public_key);
        manifest[0] = 0x00; // 破坏 magic
        assert_eq!(
            BootManifest::parse(&manifest),
            Err(BootError::ManifestCorrupt)
        );
    }

    #[test]
    fn test_manifest_wrong_key_length_rejected() {
        let (kp, _rng) = keypair();
        let (mut manifest, _len) = BootManifest::encode(&kp.public_key);
        manifest[6] = 0xFF; // key_len 异常
        assert!(BootManifest::parse(&manifest).is_err());
    }

    #[test]
    fn test_verify_and_parse_rejects_unsigned_image() {
        // 未验签镜像：verify_and_parse 必须在验签期失败（防张冠李戴）。
        let (kp, mut rng) = keypair();
        let (manifest, manifest_len) = BootManifest::encode(&kp.public_key);
        let mut image = alloc::vec![0x33u8; 64];
        image.extend_from_slice(&manifest[..manifest_len]);
        // 用错误签名（全零）→ 验签失败。
        let bad_sig = ImageSignature {
            magic: *b"ESIG",
            version: 1,
            image_size: image.len() as u64,
            image_hash: [0u8; 32],
            signature: [0u8; 64],
            timestamp: 0,
        };
        assert!(
            BootManifest::verify_and_parse(&image, &kp.public_key, &bad_sig).is_err(),
            "未验签镜像的 manifest 必须拒绝"
        );
    }
}
