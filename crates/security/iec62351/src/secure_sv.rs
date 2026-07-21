//! SV 加密封装（SM4-GCM 机密性 + SM3-HMAC 完整性/认证，蓝图 §4.5）。
//!
//! 同构于 [`crate::secure_goose::SecureGoose`]，独立类型避免 GOOSE/SV 语义
//! 混用（事件 vs 采样，D8）；内部委托同一私有 [`SecureChannel`] 实现。

use alloc::vec::Vec;

use crate::key_mgmt::SessionKey;
use crate::secure_goose::{SecureChannel, SecureFrame};
use crate::SecError;

/// SV 加密封装（同构于 SecureGoose，独立类型避免混淆，D8）。
pub struct SecureSv {
    /// 委托的公共安全通道（D8）。
    channel: SecureChannel,
}

impl SecureSv {
    /// 以会话密钥初始化 SV 安全通道。
    pub fn new(session: &SessionKey) -> Self {
        Self {
            channel: SecureChannel::new(session),
        }
    }

    /// 加密明文 → SecureFrame（SM4-GCM 加密 + SM3-HMAC 认证）。
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<SecureFrame, SecError> {
        self.channel.encrypt(plaintext)
    }

    /// 解密 SecureFrame → 明文（先校验 HMAC，再 GCM 解密）。
    pub fn decrypt(&self, frame: &SecureFrame) -> Result<Vec<u8>, SecError> {
        self.channel.decrypt(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用会话密钥（固定材料，与 secure_goose 测试一致）。
    fn test_session() -> SessionKey {
        SessionKey {
            key_id: 7,
            key_data: [0x42u8; 16],
            mac_key: [0x24u8; 32],
            expiry: 9999,
        }
    }

    /// SS20: SecureSv 加密后立即解密，往返结果等于原明文。
    #[test]
    fn ss20_encrypt_decrypt_roundtrip() {
        let mut sv = SecureSv::new(&test_session());
        let plaintext = b"SV sample payload 9-2";
        let frame = sv.encrypt(plaintext).unwrap();
        let decrypted = sv.decrypt(&frame).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    /// SS21: SecureSv 同一实例连续加密两次，IV 计数器递增。
    #[test]
    fn ss21_iv_counter_increments() {
        let mut sv = SecureSv::new(&test_session());
        let f1 = sv.encrypt(b"sv first").unwrap();
        let f2 = sv.encrypt(b"sv second").unwrap();
        assert_ne!(f1.iv, f2.iv);
        let c1 = u64::from_be_bytes(f1.iv[0..8].try_into().unwrap());
        let c2 = u64::from_be_bytes(f2.iv[0..8].try_into().unwrap());
        assert_eq!(c2, c1 + 1);
        assert_eq!(&f1.iv[8..12], &7u32.to_be_bytes());
    }

    /// SS22: SecureSv 篡改密文首字节 → HmacMismatch（与 SG13 同构抽样验证）。
    #[test]
    fn ss22_tamper_ciphertext_detected() {
        let mut sv = SecureSv::new(&test_session());
        let mut frame = sv.encrypt(b"sv tamper me").unwrap();
        frame.ciphertext[0] ^= 0xFF;
        assert_eq!(sv.decrypt(&frame), Err(SecError::HmacMismatch));
    }
}
