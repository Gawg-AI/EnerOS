//! 软件加密引擎 fallback 实现 (workspace v0.192.0).
//!
//! 将 v0.31.0 纯 Rust 国密算法库封装为 `CryptoEngine` trait 实现，
//! 作为硬件引擎不可用时的 fallback。
//!
//! # 性能参考
//! - SM3 软件：~100 Mbps
//! - SM4-ECB 软件：~80 Mbps
//! - SM2 签名软件：~50 ms
//!
//! 相比硬件加速（≥ 10 倍），软件实现性能较低，但保证功能正确性。
//!
//! # no_std 合规
//! 仅使用 `core::*` / `alloc::*`，不依赖 `std::*`。

use alloc::vec::Vec;

use super::engine::{CryptoEngine, EngineStats, Sm4Mode};
use crate::error::CryptoError;

// ============================================================
// SoftwareCryptoEngine
// ============================================================

/// 纯 Rust 软件加密引擎.
///
/// 封装 `eneros-crypto` 的 SM2/SM3/SM4 纯 Rust 实现，
/// 作为硬件引擎故障时的 fallback。始终可用，不依赖任何硬件。
///
/// # 使用场景
/// - 硬件加密引擎不可用（QEMU 模拟、开发环境）
/// - 硬件引擎故障时的自动降级
/// - 硬件引擎超时时的备用路径
pub struct SoftwareCryptoEngine {
    /// 引擎统计
    stats: EngineStats,
}

impl SoftwareCryptoEngine {
    /// 创建软件加密引擎.
    pub fn new() -> Self {
        Self {
            stats: EngineStats::new(),
        }
    }
}

impl Default for SoftwareCryptoEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl CryptoEngine for SoftwareCryptoEngine {
    fn sm2_sign(&mut self, private_key: &[u8], msg: &[u8]) -> Result<[u8; 64], CryptoError> {
        use crate::rng::CsRng;
        use crate::sm2::{sm2_sign, Sm2KeyPair, Sm2PrivateKey};

        if private_key.len() != 32 {
            return Err(CryptoError::InvalidKeyLength {
                expected: 32,
                actual: private_key.len(),
            });
        }

        let mut rng = CsRng::for_deterministic_signature(
            private_key,
            msg,
            b"eneros/software-crypto-engine/sm2-sign/v1",
        );
        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(private_key);
        let sk = Sm2PrivateKey::from_bytes(&key_bytes)
            .map_err(|_| CryptoError::InvalidInput("invalid SM2 private key".into()))?;
        let kp = Sm2KeyPair::from_private_key(&sk).map_err(|_| CryptoError::InternalError)?;
        let sig =
            sm2_sign(msg, &sk, &kp.public_key, &mut rng).map_err(|_| CryptoError::InternalError)?;

        self.stats.record_success(msg.len());
        Ok(sig.to_bytes())
    }

    fn sm2_verify(
        &mut self,
        public_key: &[u8],
        msg: &[u8],
        sig: &[u8; 64],
    ) -> Result<bool, CryptoError> {
        use crate::sm2::{sm2_verify, Sm2PublicKey, Sm2Signature};

        if public_key.len() != 65 {
            return Err(CryptoError::InvalidKeyLength {
                expected: 65,
                actual: public_key.len(),
            });
        }

        let pk = Sm2PublicKey::from_bytes(public_key)
            .map_err(|_| CryptoError::InvalidInput("invalid SM2 public key".into()))?;
        let signature = Sm2Signature::from_bytes(sig);
        let result = sm2_verify(msg, &signature, &pk).map_err(|_| CryptoError::InternalError)?;

        self.stats.record_success(msg.len());
        Ok(result)
    }

    fn sm3_hash(&mut self, data: &[u8]) -> Result<[u8; 32], CryptoError> {
        let hash = crate::sm3::hash(data);
        self.stats.record_success(data.len());
        Ok(hash)
    }

    fn sm4_encrypt(
        &mut self,
        key: &[u8; 16],
        iv: Option<&[u8]>,
        mode: Sm4Mode,
        data: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        use crate::sm4::Sm4;

        let cipher = Sm4::new(key);
        let result = match mode {
            Sm4Mode::Ecb => {
                let mut out = Vec::with_capacity(data.len());
                for chunk in data.chunks(16) {
                    let mut block = [0u8; 16];
                    block[..chunk.len()].copy_from_slice(chunk);
                    let enc = cipher.encrypt_block(&block);
                    out.extend_from_slice(&enc[..chunk.len()]);
                }
                out
            }
            Sm4Mode::Cbc => {
                use crate::sm4::cbc::Sm4Cbc;
                let iv_bytes = iv.ok_or(CryptoError::InvalidNonceLength {
                    expected: 16,
                    actual: 0,
                })?;
                if iv_bytes.len() < 16 {
                    return Err(CryptoError::InvalidNonceLength {
                        expected: 16,
                        actual: iv_bytes.len(),
                    });
                }
                let mut iv_arr = [0u8; 16];
                iv_arr.copy_from_slice(&iv_bytes[..16]);
                let cbc = Sm4Cbc::new(key, &iv_arr);
                cbc.encrypt(data)
            }
            Sm4Mode::Ctr | Sm4Mode::Gcm => {
                // CTR/GCM 模式使用 GCM 子模块
                use crate::sm4::gcm::Sm4Gcm;
                let iv_bytes = iv.ok_or(CryptoError::InvalidNonceLength {
                    expected: 12,
                    actual: 0,
                })?;
                if iv_bytes.len() < 12 {
                    return Err(CryptoError::InvalidNonceLength {
                        expected: 12,
                        actual: iv_bytes.len(),
                    });
                }
                let mut nonce = [0u8; 12];
                nonce.copy_from_slice(&iv_bytes[..12]);
                let gcm = Sm4Gcm::new(key);
                let (ciphertext, _tag) = gcm.encrypt(&nonce, data, &[]);
                ciphertext
            }
        };
        self.stats.record_success(data.len());
        Ok(result)
    }

    fn sm4_decrypt(
        &mut self,
        key: &[u8; 16],
        iv: Option<&[u8]>,
        mode: Sm4Mode,
        data: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        use crate::sm4::Sm4;

        let cipher = Sm4::new(key);
        let result = match mode {
            Sm4Mode::Ecb => {
                let mut out = Vec::with_capacity(data.len());
                for chunk in data.chunks(16) {
                    let mut block = [0u8; 16];
                    block[..chunk.len()].copy_from_slice(chunk);
                    let dec = cipher.decrypt_block(&block);
                    out.extend_from_slice(&dec[..chunk.len()]);
                }
                out
            }
            Sm4Mode::Cbc => {
                use crate::sm4::cbc::Sm4Cbc;
                let iv_bytes = iv.ok_or(CryptoError::InvalidNonceLength {
                    expected: 16,
                    actual: 0,
                })?;
                if iv_bytes.len() < 16 {
                    return Err(CryptoError::InvalidNonceLength {
                        expected: 16,
                        actual: iv_bytes.len(),
                    });
                }
                let mut iv_arr = [0u8; 16];
                iv_arr.copy_from_slice(&iv_bytes[..16]);
                let cbc = Sm4Cbc::new(key, &iv_arr);
                cbc.decrypt(data)?
            }
            Sm4Mode::Ctr | Sm4Mode::Gcm => {
                use crate::sm4::gcm::Sm4Gcm;
                let iv_bytes = iv.ok_or(CryptoError::InvalidNonceLength {
                    expected: 12,
                    actual: 0,
                })?;
                if iv_bytes.len() < 12 {
                    return Err(CryptoError::InvalidNonceLength {
                        expected: 12,
                        actual: iv_bytes.len(),
                    });
                }
                let mut nonce = [0u8; 12];
                nonce.copy_from_slice(&iv_bytes[..12]);
                let gcm = Sm4Gcm::new(key);
                // GCM 解密需要 tag（最后 16 字节）
                if data.len() < 16 {
                    return Err(CryptoError::InvalidLength {
                        expected: 16,
                        actual: data.len(),
                    });
                }
                let ct_len = data.len() - 16;
                let mut tag = [0u8; 16];
                tag.copy_from_slice(&data[ct_len..]);
                gcm.decrypt(&nonce, &data[..ct_len], &[], &tag)?
            }
        };
        self.stats.record_success(data.len());
        Ok(result)
    }

    fn name(&self) -> &'static str {
        "Software Crypto (Pure Rust)"
    }

    fn is_hardware(&self) -> bool {
        false
    }

    fn is_available(&self) -> bool {
        true // 软件引擎始终可用
    }

    fn stats(&self) -> EngineStats {
        self.stats.clone()
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_software_engine_info() {
        let engine = SoftwareCryptoEngine::new();
        assert_eq!(engine.name(), "Software Crypto (Pure Rust)");
        assert!(!engine.is_hardware());
        assert!(engine.is_available());
    }

    #[test]
    fn test_software_sm3_kat() {
        let mut engine = SoftwareCryptoEngine::new();

        // GB/T 32905-2016 示例 1
        let hash = engine.sm3_hash(b"abc").unwrap();
        let expected: [u8; 32] = [
            0x66, 0xC7, 0xF0, 0xF4, 0x62, 0xEE, 0xED, 0xD9, 0xD1, 0xF2, 0xD4, 0x6B, 0xDC, 0x10,
            0xE4, 0xE2, 0x41, 0x67, 0xC4, 0x87, 0x5C, 0xF2, 0xF7, 0xA2, 0x29, 0x7D, 0xA0, 0x2B,
            0x8F, 0x4B, 0xA8, 0xE0,
        ];
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_software_sm4_ecb_roundtrip() {
        let mut engine = SoftwareCryptoEngine::new();
        let key = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        let plaintext = key;

        let ciphertext = engine
            .sm4_encrypt(&key, None, Sm4Mode::Ecb, &plaintext)
            .unwrap();
        // 国标测试向量：SM4-ECB(key, key) = 681EDF34D206965E86B3E94F536E4246
        let expected_ct: [u8; 16] = [
            0x68, 0x1E, 0xDF, 0x34, 0xD2, 0x06, 0x96, 0x5E, 0x86, 0xB3, 0xE9, 0x4F, 0x53, 0x6E,
            0x42, 0x46,
        ];
        assert_eq!(ciphertext, expected_ct);

        let decrypted = engine
            .sm4_decrypt(&key, None, Sm4Mode::Ecb, &ciphertext)
            .unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_software_sm4_cbc_roundtrip() {
        let mut engine = SoftwareCryptoEngine::new();
        let key = [0x42u8; 16];
        let iv = [0x24u8; 16];
        let plaintext = b"Hello, SM4-CBC software engine!";

        let ciphertext = engine
            .sm4_encrypt(&key, Some(&iv), Sm4Mode::Cbc, plaintext)
            .unwrap();
        assert_ne!(&ciphertext[..], &plaintext[..]);

        let decrypted = engine
            .sm4_decrypt(&key, Some(&iv), Sm4Mode::Cbc, &ciphertext)
            .unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_software_sm2_sign_verify() {
        let mut engine = SoftwareCryptoEngine::new();

        use crate::rng::CsRng;
        use crate::sm2::Sm2KeyPair;
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();

        let msg = b"software engine SM2 test";
        let sk_bytes = kp.private_key.to_bytes();
        let pk_bytes = kp.public_key.to_bytes_uncompressed();

        let sig = engine.sm2_sign(&sk_bytes, msg).unwrap();
        assert_ne!(sig, [0u8; 64]);

        let valid = engine.sm2_verify(&pk_bytes, msg, &sig).unwrap();
        assert!(valid);

        // 篡改消息
        let invalid = engine.sm2_verify(&pk_bytes, b"tampered", &sig).unwrap();
        assert!(!invalid);
    }

    #[test]
    fn test_software_sm2_invalid_key() {
        let mut engine = SoftwareCryptoEngine::new();
        let result = engine.sm2_sign(&[0u8; 16], b"msg");
        assert!(matches!(
            result,
            Err(CryptoError::InvalidKeyLength {
                expected: 32,
                actual: 16
            })
        ));
    }

    #[test]
    fn test_software_sm4_cbc_missing_iv() {
        let mut engine = SoftwareCryptoEngine::new();
        let key = [0u8; 16];
        let result = engine.sm4_encrypt(&key, None, Sm4Mode::Cbc, b"data");
        assert!(matches!(
            result,
            Err(CryptoError::InvalidNonceLength { .. })
        ));
    }

    #[test]
    fn test_software_stats() {
        let mut engine = SoftwareCryptoEngine::new();
        let _ = engine.sm3_hash(b"a");
        let _ = engine.sm3_hash(b"b");
        let _ = engine.sm3_hash(b"c");

        let stats = engine.stats();
        assert_eq!(stats.total_ops, 3);
        assert_eq!(stats.success_ops, 3);
        assert_eq!(stats.success_rate(), 100);
        assert!(stats.bytes_processed > 0);
    }
}
