//! SM4-CBC 工作模式 (GB/T 32907-2016 + PKCS#7 填充).
//!
//! 提供 SM4 分组密码的密文分组链接（CBC）模式实现，配合 PKCS#7 填充。
//!
//! # 算法概述
//! CBC 模式：每个明文分组在加密前先与前一个密文分组（首分组使用 IV）异或。
//! PKCS#7 填充：将明文长度补齐到 16 字节的整数倍，填充字节值等于填充字节数。
//! - pad_len = 16 - (len % 16)，取值范围 [1, 16]
//! - 追加 pad_len 个值为 pad_len 的字节
//! - 即使明文长度已是 16 的倍数，也追加一整块 16 字节填充
//!
//! # no_std 合规
//! 仅使用 `alloc::vec::Vec`，不使用 `std::*`。

use alloc::vec::Vec;

use crate::error::CryptoError;
use crate::sm4::Sm4;

/// SM4-CBC 分组密码，使用 PKCS#7 填充.
///
/// CBC 模式：每个明文分组先与前一个密文分组（首分组使用 IV）异或，再加密。
/// PKCS#7 填充：始终填充（即使长度已是 16 的倍数，也补一整块 16 字节）。
///
/// # 示例
/// ```
/// use eneros_crypto::sm4::cbc::Sm4Cbc;
/// let key = [0x01u8; 16];
/// let iv = [0x00u8; 16];
/// let cipher = Sm4Cbc::new(&key, &iv);
/// let plaintext = b"hello";
/// let ciphertext = cipher.encrypt(plaintext);
/// let decrypted = cipher.decrypt(&ciphertext).expect("decrypt should succeed");
/// assert_eq!(decrypted, plaintext);
/// ```
pub struct Sm4Cbc {
    cipher: Sm4,
    iv: [u8; 16],
}

impl Sm4Cbc {
    /// 创建新的 SM4-CBC 实例.
    ///
    /// # 参数
    /// * `key` - 128-bit (16 字节) SM4 密钥
    /// * `iv` - 128-bit (16 字节) 初始化向量
    pub fn new(key: &[u8; 16], iv: &[u8; 16]) -> Self {
        Self {
            cipher: Sm4::new(key),
            iv: *iv,
        }
    }

    /// 加密明文，返回密文（含 PKCS#7 填充）.
    ///
    /// 始终应用 PKCS#7 填充（即使明文长度是 16 的倍数，也补一整块 16 字节）。
    /// CBC 链：C[i] = E_K(P[i] XOR C[i-1])，C[0] 使用 IV。
    pub fn encrypt(&self, plaintext: &[u8]) -> Vec<u8> {
        // PKCS#7 填充：pad_len ∈ [1, 16]
        let pad_len = 16 - (plaintext.len() % 16);
        let mut padded = Vec::with_capacity(plaintext.len() + pad_len);
        padded.extend_from_slice(plaintext);
        padded.extend(core::iter::repeat(pad_len as u8).take(pad_len));

        // CBC 加密
        let mut ciphertext = Vec::with_capacity(padded.len());
        let mut prev_block = self.iv;
        for chunk in padded.chunks(16) {
            let mut block = [0u8; 16];
            for ((b, &c), &p) in block.iter_mut().zip(chunk.iter()).zip(prev_block.iter()) {
                *b = c ^ p;
            }
            let encrypted = self.cipher.encrypt_block(&block);
            ciphertext.extend_from_slice(&encrypted);
            prev_block = encrypted;
        }
        ciphertext
    }

    /// 解密密文，验证并去除 PKCS#7 填充后返回明文.
    ///
    /// CBC 链：P[i] = D_K(C[i]) XOR C[i-1]，C[0] 使用 IV。
    /// 填充验证采用恒定时间比较（累积差异），避免提前返回的时序侧信道。
    ///
    /// # 错误
    /// - `CryptoError::InvalidLength`：密文为空或长度不是 16 的倍数
    /// - `CryptoError::InvalidPadding`：PKCS#7 填充无效
    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if ciphertext.is_empty() || ciphertext.len() % 16 != 0 {
            return Err(CryptoError::InvalidLength {
                expected: 16,
                actual: ciphertext.len(),
            });
        }

        // CBC 解密
        let mut plaintext = Vec::with_capacity(ciphertext.len());
        let mut prev_block = self.iv;
        for chunk in ciphertext.chunks(16) {
            let mut block = [0u8; 16];
            block.copy_from_slice(chunk);
            let decrypted = self.cipher.decrypt_block(&block);
            for (&d, &p) in decrypted.iter().zip(prev_block.iter()) {
                plaintext.push(d ^ p);
            }
            prev_block = block;
        }

        // 验证并去除 PKCS#7 填充
        // pad_len 来自最后一个字节，取值范围检查（pad_len 本身非秘密，无需恒定时间）
        let pad_len = plaintext[plaintext.len() - 1] as usize;
        if pad_len == 0 || pad_len > 16 || pad_len > plaintext.len() {
            return Err(CryptoError::InvalidPadding);
        }

        // 恒定时间校验所有填充字节均等于 pad_len（累积差异，不提前返回）
        let mut diff: u8 = 0;
        for &b in plaintext.iter().skip(plaintext.len() - pad_len) {
            diff |= b ^ (pad_len as u8);
        }
        if diff != 0 {
            return Err(CryptoError::InvalidPadding);
        }

        plaintext.truncate(plaintext.len() - pad_len);
        Ok(plaintext)
    }
}

// ============================================================
// Unit Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_key() -> [u8; 16] {
        [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ]
    }

    fn sample_iv() -> [u8; 16] {
        [0x00; 16]
    }

    #[test]
    fn test_cbc_encrypt_decrypt_round_trip() {
        let cipher = Sm4Cbc::new(&sample_key(), &sample_iv());
        let plaintext = b"Hello, SM4-CBC mode!";
        let ciphertext = cipher.encrypt(plaintext);
        let decrypted = cipher.decrypt(&ciphertext).expect("decrypt should succeed");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_cbc_empty_plaintext() {
        let cipher = Sm4Cbc::new(&sample_key(), &sample_iv());
        let ciphertext = cipher.encrypt(b"");
        // 空明文 → 填充一整块 16 字节（值均为 0x10）
        assert_eq!(ciphertext.len(), 16);
        let decrypted = cipher.decrypt(&ciphertext).expect("decrypt should succeed");
        assert_eq!(decrypted, b"");
    }

    #[test]
    fn test_cbc_single_byte() {
        let cipher = Sm4Cbc::new(&sample_key(), &sample_iv());
        let plaintext = b"A";
        let ciphertext = cipher.encrypt(plaintext);
        assert_eq!(ciphertext.len(), 16);
        let decrypted = cipher.decrypt(&ciphertext).expect("decrypt should succeed");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_cbc_15_bytes() {
        let cipher = Sm4Cbc::new(&sample_key(), &sample_iv());
        let plaintext = &[0xAAu8; 15];
        let ciphertext = cipher.encrypt(plaintext);
        assert_eq!(ciphertext.len(), 16);
        let decrypted = cipher.decrypt(&ciphertext).expect("decrypt should succeed");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_cbc_16_bytes() {
        let cipher = Sm4Cbc::new(&sample_key(), &sample_iv());
        let plaintext = &[0x42u8; 16];
        let ciphertext = cipher.encrypt(plaintext);
        // 16 字节明文 → pad_len=16，补一整块 → 32 字节密文
        assert_eq!(ciphertext.len(), 32);
        let decrypted = cipher.decrypt(&ciphertext).expect("decrypt should succeed");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_cbc_17_bytes() {
        let cipher = Sm4Cbc::new(&sample_key(), &sample_iv());
        let plaintext = &[0x33u8; 17];
        let ciphertext = cipher.encrypt(plaintext);
        // 17 字节 → pad_len=15 → 32 字节密文
        assert_eq!(ciphertext.len(), 32);
        let decrypted = cipher.decrypt(&ciphertext).expect("decrypt should succeed");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_cbc_invalid_ciphertext_length() {
        let cipher = Sm4Cbc::new(&sample_key(), &sample_iv());
        // 15 字节密文（非 16 的倍数）
        let ciphertext = [0u8; 15];
        let result = cipher.decrypt(&ciphertext);
        assert_eq!(
            result,
            Err(CryptoError::InvalidLength {
                expected: 16,
                actual: 15
            })
        );
    }

    #[test]
    fn test_cbc_empty_ciphertext() {
        let cipher = Sm4Cbc::new(&sample_key(), &sample_iv());
        let result = cipher.decrypt(b"");
        assert_eq!(
            result,
            Err(CryptoError::InvalidLength {
                expected: 16,
                actual: 0
            })
        );
    }

    #[test]
    fn test_cbc_invalid_padding() {
        // 构造一个有效密文，然后破坏最后一字节使填充无效
        let cipher = Sm4Cbc::new(&sample_key(), &sample_iv());
        let plaintext = b"test";
        let mut ciphertext = cipher.encrypt(plaintext);
        // 翻转最后一字节（破坏填充字节值）
        let last = ciphertext.len() - 1;
        ciphertext[last] ^= 0xFF;
        let result = cipher.decrypt(&ciphertext);
        assert!(
            matches!(result, Err(CryptoError::InvalidPadding)),
            "corrupted padding should yield InvalidPadding"
        );
    }

    #[test]
    fn test_cbc_different_iv_different_ciphertext() {
        let key = sample_key();
        let iv1 = [0x00u8; 16];
        let iv2 = [0xFFu8; 16];
        let plaintext = b"same plaintext here";
        let c1 = Sm4Cbc::new(&key, &iv1);
        let c2 = Sm4Cbc::new(&key, &iv2);
        let ct1 = c1.encrypt(plaintext);
        let ct2 = c2.encrypt(plaintext);
        assert_ne!(ct1, ct2, "different IVs should yield different ciphertexts");
    }

    #[test]
    fn test_cbc_different_key_different_ciphertext() {
        let key1 = [0x01u8; 16];
        let key2 = [0x02u8; 16];
        let iv = sample_iv();
        let plaintext = b"same plaintext here";
        let c1 = Sm4Cbc::new(&key1, &iv);
        let c2 = Sm4Cbc::new(&key2, &iv);
        let ct1 = c1.encrypt(plaintext);
        let ct2 = c2.encrypt(plaintext);
        assert_ne!(
            ct1, ct2,
            "different keys should yield different ciphertexts"
        );
    }

    #[test]
    fn test_cbc_pkcs7_padding_values() {
        let cipher = Sm4Cbc::new(&sample_key(), &sample_iv());

        // 15 字节 → pad_len = 1（最后一字节 = 0x01）
        let pt15 = [0xABu8; 15];
        let ct = cipher.encrypt(&pt15);
        assert_eq!(ct.len(), 16);
        let mut ct_block = [0u8; 16];
        ct_block.copy_from_slice(&ct);
        let dec = cipher.cipher.decrypt_block(&ct_block);
        let raw: [u8; 16] = core::array::from_fn(|i| dec[i] ^ cipher.iv[i]);
        assert_eq!(&raw[0..15], &pt15[..]);
        assert_eq!(raw[15], 0x01);

        // 14 字节 → pad_len = 2（最后两字节 = 0x02 0x02）
        let pt14 = [0xCDu8; 14];
        let ct = cipher.encrypt(&pt14);
        assert_eq!(ct.len(), 16);
        ct_block.copy_from_slice(&ct);
        let dec = cipher.cipher.decrypt_block(&ct_block);
        let raw: [u8; 16] = core::array::from_fn(|i| dec[i] ^ cipher.iv[i]);
        assert_eq!(&raw[0..14], &pt14[..]);
        assert_eq!(raw[14], 0x02);
        assert_eq!(raw[15], 0x02);

        // 16 字节 → pad_len = 16（第二块全为 0x10）
        let pt16 = [0xEFu8; 16];
        let ct = cipher.encrypt(&pt16);
        assert_eq!(ct.len(), 32);
        // 第二块是纯填充块，前一密文块为 ct[0..16]
        let mut second_ct = [0u8; 16];
        second_ct.copy_from_slice(&ct[16..]);
        let dec = cipher.cipher.decrypt_block(&second_ct);
        let mut prev = [0u8; 16];
        prev.copy_from_slice(&ct[0..16]);
        let raw: [u8; 16] = core::array::from_fn(|i| dec[i] ^ prev[i]);
        for (i, &b) in raw.iter().enumerate() {
            assert_eq!(b, 0x10, "padding byte {} should be 0x10", i);
        }
    }
}
