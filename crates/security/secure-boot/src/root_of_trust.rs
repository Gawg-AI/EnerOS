//! 信任根定义 (v0.160.0).
//!
//! 信任根是 Secure Boot 的锚点，绑定硬件信任根（TPM PCR）和
//! 密码学信任根（SM2 根公钥）。

use eneros_crypto::Sm2PublicKey;

// ============================================================
// Algorithm Identifiers
// ============================================================

/// 哈希算法标识.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlgo {
    /// 国密 SM3-256
    Sm3,
}

/// 签名算法标识.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignAlgo {
    /// 国密 SM2
    Sm2,
}

// ============================================================
// Root of Trust
// ============================================================

/// 信任根.
///
/// 绑定平台硬件信任根（TPM PCR 锚点）和密码学信任根（SM2 根公钥）。
/// 信任根私钥安全存储在 TPM 中，不可导出。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootOfTrust {
    /// 信任根 SM2 公钥（用于验证 Bootloader 签名）
    pub root_key: Sm2PublicKey,
    /// TPM PCR 锚点（平台初始度量基准）
    pub tpm_pcr_anchor: [u8; 32],
    /// 哈希算法（固定 SM3）
    pub hash_algorithm: HashAlgo,
    /// 签名算法（固定 SM2）
    pub sign_algorithm: SignAlgo,
}

impl RootOfTrust {
    /// 创建信任根.
    pub fn new(root_key: Sm2PublicKey, tpm_pcr_anchor: [u8; 32]) -> Self {
        Self {
            root_key,
            tpm_pcr_anchor,
            hash_algorithm: HashAlgo::Sm3,
            sign_algorithm: SignAlgo::Sm2,
        }
    }

    /// 验证信任根有效性.
    ///
    /// 检查根公钥非零（非全零无效密钥）。
    pub fn is_valid(&self) -> bool {
        let pk_bytes = self.root_key.to_bytes_uncompressed();
        // 检查非全零（04 前缀 + 非零坐标）
        pk_bytes[1..].iter().any(|&b| b != 0)
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use eneros_crypto::{CsRng, Sm2KeyPair};

    use super::*;

    #[test]
    fn test_root_of_trust_creation() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let rot = RootOfTrust::new(kp.public_key, [0u8; 32]);

        assert_eq!(rot.hash_algorithm, HashAlgo::Sm3);
        assert_eq!(rot.sign_algorithm, SignAlgo::Sm2);
        assert!(rot.is_valid());
    }

    #[test]
    fn test_root_of_trust_valid() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let rot = RootOfTrust::new(kp.public_key, [0xAA; 32]);
        assert!(rot.is_valid());
        assert_eq!(rot.tpm_pcr_anchor, [0xAA; 32]);
    }

    #[test]
    fn test_algorithm_enums() {
        assert_eq!(HashAlgo::Sm3, HashAlgo::Sm3);
        assert_eq!(SignAlgo::Sm2, SignAlgo::Sm2);
    }
}
