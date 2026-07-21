//! SM4-GCM 认证加密工作模式 (NIST SP 800-38D).
//!
//! 提供 SM4 分组密码的 Galois/Counter Mode (GCM) 实现，结合 CTR 模式
//! 加密与 GHASH 认证，提供 AEAD (Authenticated Encryption with Associated
//! Data) 安全保证。
//!
//! # 算法概述
//! GCM 模式结合 CTR 模式加密与基于 GF(2^128) 的 GHASH 认证：
//! - 密钥长度：128 位（16 字节，SM4 密钥）
//! - Nonce 长度：96 位（12 字节，NIST SP 800-38D 推荐）
//! - 认证标签：128 位（16 字节）
//! - 认证多项式：x^128 + x^7 + x^2 + x + 1（GF(2^128) 不可约多项式）
//!
//! # 工作流程
//! 1. 预计算哈希子密钥 H = E_K(0^128)
//! 2. 构造初始计数器 J0 = nonce || 0x00000001（96 位 Nonce 情形）
//! 3. CTR 加密：计数器从 J0+1 开始递增，密钥流与明文异或
//! 4. GHASH 认证：对 AAD || 密文 || 长度块 执行 GF(2^128) 链式乘法
//! 5. 标签：Tag = E_K(J0) XOR GHASH(H, input)
//!
//! # no_std 合规
//! 仅使用 `alloc::vec::Vec`，不使用 `std::*`。

use alloc::vec::Vec;

use crate::constant_time::ct_eq;
use crate::error::CryptoError;
use crate::sm4::Sm4;

/// SM4-GCM 认证加密 (AEAD).
///
/// 使用 96 位（12 字节）Nonce（NIST SP 800-38D 推荐）。
/// 产生 128 位（16 字节）认证标签。
///
/// 同一 `Sm4Gcm` 实例可配合不同 Nonce 多次使用（Nonce 通过
/// `encrypt`/`decrypt` 参数传入），但同一 (Key, Nonce) 组合
/// 绝不可重复使用（否则会泄露明文 XOR）。
///
/// # 示例
/// ```
/// use eneros_crypto::sm4::gcm::Sm4Gcm;
/// let key = [0x01u8; 16];
/// let nonce = [0x00u8; 12];
/// let cipher = Sm4Gcm::new(&key);
/// let plaintext = b"hello SM4-GCM";
/// let aad = b"associated data";
/// let (ciphertext, tag) = cipher.encrypt(&nonce, plaintext, aad);
/// let decrypted = cipher
///     .decrypt(&nonce, &ciphertext, aad, &tag)
///     .expect("decrypt should succeed");
/// assert_eq!(decrypted, plaintext);
/// ```
pub struct Sm4Gcm {
    /// SM4 分组密码实例
    cipher: Sm4,
    /// 哈希子密钥 H = E_K(0^128)
    h: [u8; 16],
}

impl Sm4Gcm {
    /// 创建新的 SM4-GCM 实例.
    ///
    /// 预计算哈希子密钥 H = E_K(0^128)，后续 `encrypt`/`decrypt`
    /// 可复用同一实例配合不同 Nonce。
    ///
    /// # 参数
    /// * `key` - 128 位（16 字节）SM4 密钥
    pub fn new(key: &[u8; 16]) -> Self {
        let cipher = Sm4::new(key);
        // H = E_K(0^128)
        let h = cipher.encrypt_block(&[0u8; 16]);
        Self { cipher, h }
    }

    /// 加密明文并生成认证标签.
    ///
    /// 使用 CTR 模式加密（计数器从 J0+1 开始），对 AAD 和密文
    /// 执行 GHASH 认证，生成 128 位标签。
    ///
    /// # 参数
    /// * `nonce` - 96 位（12 字节）Nonce
    /// * `plaintext` - 待加密明文（可为空）
    /// * `aad` - 附加认证数据（可为空，不加密但参与认证）
    ///
    /// # 返回
    /// (密文, 128 位认证标签)
    pub fn encrypt(&self, nonce: &[u8; 12], plaintext: &[u8], aad: &[u8]) -> (Vec<u8>, [u8; 16]) {
        // J0 = nonce || 0x00000001（96 位 Nonce 情形）
        let mut j0 = [0u8; 16];
        j0[..12].copy_from_slice(nonce);
        j0[15] = 1;

        // CTR 模式加密，计数器从 J0+1 开始
        let mut ciphertext = Vec::with_capacity(plaintext.len());
        let mut counter = j0;
        for chunk in plaintext.chunks(16) {
            inc_counter(&mut counter);
            let keystream = self.cipher.encrypt_block(&counter);
            for (i, &b) in chunk.iter().enumerate() {
                ciphertext.push(b ^ keystream[i]);
            }
        }

        // 计算 GHASH：AAD || pad || CT || pad || 长度块
        let s = ghash_with_lengths(&self.h, aad, &ciphertext);

        // Tag = E_K(J0) XOR S
        let e_j0 = self.cipher.encrypt_block(&j0);
        let tag = core::array::from_fn(|i| e_j0[i] ^ s[i]);

        (ciphertext, tag)
    }

    /// 解密密文并验证认证标签.
    ///
    /// 先重新计算预期标签并使用恒定时间比较验证，验证通过后
    /// 才执行 CTR 模式解密。标签验证失败返回 `CryptoError::TagMismatch`，
    /// 不返回任何明文（防止选择密文攻击）。
    ///
    /// # 参数
    /// * `nonce` - 96 位（12 字节）Nonce（须与加密时一致）
    /// * `ciphertext` - 待解密密文
    /// * `aad` - 附加认证数据（须与加密时一致）
    /// * `tag` - 128 位认证标签
    ///
    /// # 错误
    /// * `CryptoError::TagMismatch` - 标签验证失败（数据被篡改或密钥/Nonce/AAD 不匹配）
    pub fn decrypt(
        &self,
        nonce: &[u8; 12],
        ciphertext: &[u8],
        aad: &[u8],
        tag: &[u8; 16],
    ) -> Result<Vec<u8>, CryptoError> {
        // 重新计算预期标签
        let mut j0 = [0u8; 16];
        j0[..12].copy_from_slice(nonce);
        j0[15] = 1;

        let s = ghash_with_lengths(&self.h, aad, ciphertext);
        let e_j0 = self.cipher.encrypt_block(&j0);
        let expected_tag: [u8; 16] = core::array::from_fn(|i| e_j0[i] ^ s[i]);

        // 恒定时间标签比较（抗时序侧信道攻击）
        if !ct_eq(&expected_tag, tag) {
            return Err(CryptoError::TagMismatch);
        }

        // 标签验证通过，执行 CTR 模式解密
        let mut plaintext = Vec::with_capacity(ciphertext.len());
        let mut counter = j0;
        for chunk in ciphertext.chunks(16) {
            inc_counter(&mut counter);
            let keystream = self.cipher.encrypt_block(&counter);
            for (i, &b) in chunk.iter().enumerate() {
                plaintext.push(b ^ keystream[i]);
            }
        }
        Ok(plaintext)
    }
}

impl Drop for Sm4Gcm {
    fn drop(&mut self) {
        // 析构时安全清零哈希子密钥 H
        // SAFETY: write_volatile 防止编译器优化删除写入
        for byte in self.h.iter_mut() {
            unsafe {
                core::ptr::write_volatile(byte, 0);
            }
        }
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        // self.cipher 会在结构体字段自动 drop 时调用 Sm4::drop 清零轮密钥
    }
}

// ============================================================
// GF(2^128) arithmetic and GHASH
// ============================================================

/// GF(2^128) 乘法，使用 GCM 多项式 x^128 + x^7 + x^2 + x + 1.
///
/// 元素以 128 位大端字节数组表示，bit 0 为 byte[0] 的 MSB
/// （即 x^0 的系数），bit 127 为 byte[15] 的 LSB（即 x^127 的系数）。
///
/// 算法（NIST SP 800-38D §6.3）：
/// - Z = 0, V = X
/// - 逐位扫描 Y（bit 0 到 bit 127）：
///   - 若 Y[i] == 1，则 Z = Z XOR V
///   - V = V >> 1；若移出位（bit 127）为 1，则 V = V XOR R
/// - R = 11100001 || 0^120 = 0xE1 || 0^120
fn gf_mult(x: &[u8; 16], y: &[u8; 16]) -> [u8; 16] {
    let mut z = [0u8; 16];
    let mut v = *x; // V = X

    // 逐位扫描 Y，从 bit 0（byte[0] 的 MSB）到 bit 127（byte[15] 的 LSB）
    for i in 0..128 {
        let bit = (y[i / 8] >> (7 - (i % 8))) & 1;
        if bit == 1 {
            // Z = Z XOR V
            for (zj, &vj) in z.iter_mut().zip(v.iter()) {
                *zj ^= vj;
            }
        }
        // 检查 V 的 bit 127（byte[15] 的 LSB）
        let lsb = v[15] & 1;
        // V = V >> 1（整体右移一位，大端方向）
        for j in (1..16).rev() {
            v[j] = (v[j] >> 1) | ((v[j - 1] & 1) << 7);
        }
        v[0] >>= 1;
        // 若移出位为 1，XOR R = 0xE1 || 0^120
        if lsb == 1 {
            v[0] ^= 0xE1;
        }
    }
    z
}

/// GHASH 函数：对数据分块执行 Y = (Y XOR block) * H 链式运算.
///
/// 数据自动按 16 字节分块，不足部分补零。返回最终的 GHASH 值。
fn ghash(h: &[u8; 16], data: &[u8]) -> [u8; 16] {
    let mut y = [0u8; 16];
    for chunk in data.chunks(16) {
        let mut block = [0u8; 16];
        block[..chunk.len()].copy_from_slice(chunk);
        // Y = (Y XOR block) * H
        for (y_byte, &b) in y.iter_mut().zip(block.iter()) {
            *y_byte ^= b;
        }
        y = gf_mult(&y, h);
    }
    y
}

/// 计算 GHASH(AAD || pad || CT || pad || len_block).
///
/// - AAD 和密文各自填充到 16 字节边界（零填充）
/// - len_block = 64-bit AAD 比特长度 || 64-bit 密文比特长度（大端）
fn ghash_with_lengths(h: &[u8; 16], aad: &[u8], ciphertext: &[u8]) -> [u8; 16] {
    let mut ghash_input = Vec::new();
    // AAD + 零填充到 16 字节边界
    ghash_input.extend_from_slice(aad);
    if aad.len() % 16 != 0 {
        ghash_input.extend(core::iter::repeat(0u8).take(16 - (aad.len() % 16)));
    }
    // 密文 + 零填充到 16 字节边界
    ghash_input.extend_from_slice(ciphertext);
    if ciphertext.len() % 16 != 0 {
        ghash_input.extend(core::iter::repeat(0u8).take(16 - (ciphertext.len() % 16)));
    }
    // 长度块：64-bit AAD 比特长度 || 64-bit 密文比特长度（大端）
    let aad_bits = (aad.len() as u64) * 8;
    let ct_bits = (ciphertext.len() as u64) * 8;
    ghash_input.extend_from_slice(&aad_bits.to_be_bytes());
    ghash_input.extend_from_slice(&ct_bits.to_be_bytes());

    ghash(h, &ghash_input)
}

/// 递增计数器（大端，仅最后 32 位递增）.
///
/// GCM 规范要求计数器的右侧 32 位递增，溢出时回绕（wrapping）。
fn inc_counter(counter: &mut [u8; 16]) {
    for i in (12..16).rev() {
        counter[i] = counter[i].wrapping_add(1);
        if counter[i] != 0 {
            break;
        }
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

    fn sample_nonce() -> [u8; 12] {
        [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b,
        ]
    }

    // ============================================================
    // Round-trip tests
    // ============================================================

    #[test]
    fn test_gcm_encrypt_decrypt_round_trip() {
        let cipher = Sm4Gcm::new(&sample_key());
        let nonce = sample_nonce();
        let plaintext = b"Hello, SM4-GCM mode!";
        let aad = b"associated data";
        let (ciphertext, tag) = cipher.encrypt(&nonce, plaintext, aad);
        assert_ne!(ciphertext.as_slice(), plaintext);
        let decrypted = cipher
            .decrypt(&nonce, &ciphertext, aad, &tag)
            .expect("decrypt should succeed");
        assert_eq!(decrypted, plaintext);
    }

    // ============================================================
    // Edge case tests
    // ============================================================

    #[test]
    fn test_gcm_empty_plaintext_no_aad() {
        let cipher = Sm4Gcm::new(&sample_key());
        let nonce = sample_nonce();
        let (ciphertext, tag) = cipher.encrypt(&nonce, b"", b"");
        // 空明文 → 空密文
        assert!(ciphertext.is_empty());
        // 标签应为 E_K(J0) XOR GHASH(H, len_block)
        // len_block = 0 || 0，GHASH = 0，故 tag = E_K(J0)
        let mut j0 = [0u8; 16];
        j0[..12].copy_from_slice(&nonce);
        j0[15] = 1;
        let expected_tag = cipher.cipher.encrypt_block(&j0);
        assert_eq!(tag, expected_tag);
        // 解密空密文应成功
        let decrypted = cipher
            .decrypt(&nonce, &ciphertext, b"", &tag)
            .expect("decrypt should succeed");
        assert!(decrypted.is_empty());
    }

    #[test]
    fn test_gcm_empty_plaintext_with_aad() {
        let cipher = Sm4Gcm::new(&sample_key());
        let nonce = sample_nonce();
        let aad = b"authentication only, no payload";
        let (ciphertext, tag) = cipher.encrypt(&nonce, b"", aad);
        assert!(ciphertext.is_empty());
        let decrypted = cipher
            .decrypt(&nonce, &ciphertext, aad, &tag)
            .expect("decrypt should succeed");
        assert!(decrypted.is_empty());
    }

    #[test]
    fn test_gcm_plaintext_no_aad() {
        let cipher = Sm4Gcm::new(&sample_key());
        let nonce = sample_nonce();
        let plaintext = b"plaintext without aad";
        let (ciphertext, tag) = cipher.encrypt(&nonce, plaintext, b"");
        assert_eq!(ciphertext.len(), plaintext.len());
        let decrypted = cipher
            .decrypt(&nonce, &ciphertext, b"", &tag)
            .expect("decrypt should succeed");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_gcm_plaintext_with_aad() {
        let cipher = Sm4Gcm::new(&sample_key());
        let nonce = sample_nonce();
        let plaintext = b"secret message with aad";
        let aad = b"metadata header";
        let (ciphertext, tag) = cipher.encrypt(&nonce, plaintext, aad);
        assert_eq!(ciphertext.len(), plaintext.len());
        let decrypted = cipher
            .decrypt(&nonce, &ciphertext, aad, &tag)
            .expect("decrypt should succeed");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_gcm_aad_only_no_plaintext() {
        // 空明文 + AAD，验证标签确定且非零
        let cipher = Sm4Gcm::new(&sample_key());
        let nonce = sample_nonce();
        let aad = b"header-only aad";
        let (ct1, tag1) = cipher.encrypt(&nonce, b"", aad);
        let (_ct2, tag2) = cipher.encrypt(&nonce, b"", aad);
        assert!(ct1.is_empty());
        assert_eq!(tag1, tag2, "same inputs should yield same tag");
        // 标签不应全零（极大概率）
        assert!(tag1.iter().any(|&b| b != 0));
    }

    // ============================================================
    // Tamper detection tests
    // ============================================================

    #[test]
    fn test_gcm_tag_tamper_fails() {
        let cipher = Sm4Gcm::new(&sample_key());
        let nonce = sample_nonce();
        let plaintext = b"sensitive data";
        let aad = b"header";
        let (ciphertext, mut tag) = cipher.encrypt(&nonce, plaintext, aad);
        // 翻转标签首字节
        tag[0] ^= 0xFF;
        let result = cipher.decrypt(&nonce, &ciphertext, aad, &tag);
        assert_eq!(result, Err(CryptoError::TagMismatch));
    }

    #[test]
    fn test_gcm_ciphertext_tamper_fails() {
        let cipher = Sm4Gcm::new(&sample_key());
        let nonce = sample_nonce();
        let plaintext = b"sensitive data";
        let aad = b"header";
        let (mut ciphertext, tag) = cipher.encrypt(&nonce, plaintext, aad);
        // 翻转密文首字节
        ciphertext[0] ^= 0xFF;
        let result = cipher.decrypt(&nonce, &ciphertext, aad, &tag);
        assert_eq!(result, Err(CryptoError::TagMismatch));
    }

    #[test]
    fn test_gcm_aad_tamper_fails() {
        let cipher = Sm4Gcm::new(&sample_key());
        let nonce = sample_nonce();
        let plaintext = b"sensitive data";
        let aad = b"header";
        let (ciphertext, tag) = cipher.encrypt(&nonce, plaintext, aad);
        // 篡改 AAD
        let tampered_aad = b"hedder";
        let result = cipher.decrypt(&nonce, &ciphertext, tampered_aad, &tag);
        assert_eq!(result, Err(CryptoError::TagMismatch));
    }

    // ============================================================
    // Differential tests
    // ============================================================

    #[test]
    fn test_gcm_different_nonce_different_ciphertext() {
        let cipher = Sm4Gcm::new(&sample_key());
        let nonce1 = sample_nonce();
        let mut nonce2 = sample_nonce();
        nonce2[0] ^= 0xFF;
        let plaintext = b"same plaintext for both";
        let (ct1, tag1) = cipher.encrypt(&nonce1, plaintext, b"");
        let (ct2, tag2) = cipher.encrypt(&nonce2, plaintext, b"");
        assert_ne!(
            ct1, ct2,
            "different nonces should yield different ciphertexts"
        );
        assert_ne!(tag1, tag2, "different nonces should yield different tags");
    }

    #[test]
    fn test_gcm_different_key_different_ciphertext() {
        let key1 = sample_key();
        let mut key2 = sample_key();
        key2[0] ^= 0xFF;
        let cipher1 = Sm4Gcm::new(&key1);
        let cipher2 = Sm4Gcm::new(&key2);
        let nonce = sample_nonce();
        let plaintext = b"same plaintext for both";
        let (ct1, tag1) = cipher1.encrypt(&nonce, plaintext, b"");
        let (ct2, tag2) = cipher2.encrypt(&nonce, plaintext, b"");
        assert_ne!(
            ct1, ct2,
            "different keys should yield different ciphertexts"
        );
        assert_ne!(tag1, tag2, "different keys should yield different tags");
    }

    // ============================================================
    // Block size boundary tests
    // ============================================================

    #[test]
    fn test_gcm_block_size_boundary() {
        let cipher = Sm4Gcm::new(&sample_key());
        let nonce = sample_nonce();
        // 16, 32, 48 字节明文（恰好对齐到块边界）
        for &size in &[16usize, 32, 48] {
            let plaintext = vec![0x42u8; size];
            let (ciphertext, tag) = cipher.encrypt(&nonce, &plaintext, b"");
            assert_eq!(
                ciphertext.len(),
                size,
                "ciphertext length mismatch at size {}",
                size
            );
            let decrypted = cipher
                .decrypt(&nonce, &ciphertext, b"", &tag)
                .expect("decrypt should succeed");
            assert_eq!(decrypted, plaintext, "round-trip failed at size {}", size);
        }
    }

    #[test]
    fn test_gcm_partial_block() {
        let cipher = Sm4Gcm::new(&sample_key());
        let nonce = sample_nonce();
        // 7, 13, 20 字节明文（非块对齐）
        for &size in &[7usize, 13, 20] {
            let plaintext = vec![0x33u8; size];
            let (ciphertext, tag) = cipher.encrypt(&nonce, &plaintext, b"");
            assert_eq!(
                ciphertext.len(),
                size,
                "ciphertext length mismatch at size {}",
                size
            );
            let decrypted = cipher
                .decrypt(&nonce, &ciphertext, b"", &tag)
                .expect("decrypt should succeed");
            assert_eq!(decrypted, plaintext, "round-trip failed at size {}", size);
        }
    }

    #[test]
    fn test_gcm_large_aad() {
        let cipher = Sm4Gcm::new(&sample_key());
        let nonce = sample_nonce();
        let plaintext = b"payload with large aad";
        // 100 字节 AAD（跨越多个块）
        let aad: Vec<u8> = (0..100u8).collect();
        let (ciphertext, tag) = cipher.encrypt(&nonce, plaintext, &aad);
        assert_eq!(ciphertext.len(), plaintext.len());
        let decrypted = cipher
            .decrypt(&nonce, &ciphertext, &aad, &tag)
            .expect("decrypt should succeed");
        assert_eq!(decrypted, plaintext);
    }

    // ============================================================
    // Internal function tests
    // ============================================================

    #[test]
    fn test_gcm_counter_increment() {
        // 基本递增
        let mut counter = [0u8; 16];
        inc_counter(&mut counter);
        assert_eq!(&counter[12..16], &[0, 0, 0, 1]);
        inc_counter(&mut counter);
        assert_eq!(&counter[12..16], &[0, 0, 0, 2]);

        // 进位测试：0xFF → 0x00 + 进位
        let mut counter = [0u8; 16];
        counter[15] = 0xFF;
        inc_counter(&mut counter);
        assert_eq!(&counter[12..16], &[0, 0, 1, 0]);

        // 连续进位：0x00FFFFFF → 0x01000000
        let mut counter = [0u8; 16];
        counter[12] = 0x00;
        counter[13] = 0xFF;
        counter[14] = 0xFF;
        counter[15] = 0xFF;
        inc_counter(&mut counter);
        assert_eq!(&counter[12..16], &[0x01, 0x00, 0x00, 0x00]);

        // 溢出回绕：0xFFFFFFFF → 0x00000000
        let mut counter = [0u8; 16];
        counter[12] = 0xFF;
        counter[13] = 0xFF;
        counter[14] = 0xFF;
        counter[15] = 0xFF;
        inc_counter(&mut counter);
        assert_eq!(&counter[12..16], &[0x00, 0x00, 0x00, 0x00]);

        // 前 12 字节（Nonce 部分）不受影响，仅最后 32 位递增
        let mut counter = [0xAA; 16];
        inc_counter(&mut counter);
        assert_eq!(&counter[..12], &[0xAA; 12]);
        // counter[12..15] 保持 0xAA，仅 counter[15] 从 0xAA 递增到 0xAB
        assert_eq!(&counter[12..16], &[0xAA, 0xAA, 0xAA, 0xAB]);
    }

    #[test]
    fn test_gf_mult() {
        // 1. 乘零 = 零
        let zero = [0u8; 16];
        let x = [0x42; 16];
        assert_eq!(gf_mult(&zero, &x), zero);
        assert_eq!(gf_mult(&x, &zero), zero);

        // 2. 乘单位元 = 原值
        // 单位元 1 在 GCM 表示中为 bit 0 = 0x80 || 0^120
        let mut identity = [0u8; 16];
        identity[0] = 0x80;
        assert_eq!(gf_mult(&x, &identity), x);
        assert_eq!(gf_mult(&identity, &x), x);

        // 3. 1 * 1 = 1
        assert_eq!(gf_mult(&identity, &identity), identity);

        // 4. 已知向量：x^127 * x = x^128 = x^7 + x^2 + x + 1
        //    x^127 表示为 [0, ..., 0, 0x01]（bit 127 = byte[15] 的 LSB）
        //    x^1   表示为 [0x40, 0, ..., 0]（bit 1 = byte[0] 的第 6 位）
        //    结果  = [0xE1, 0, ..., 0]（bit 0,1,2,7 = 0x80|0x40|0x20|0x01 = 0xE1）
        let mut x127 = [0u8; 16];
        x127[15] = 0x01;
        let mut x1 = [0u8; 16];
        x1[0] = 0x40;
        let mut expected = [0u8; 16];
        expected[0] = 0xE1;
        assert_eq!(gf_mult(&x127, &x1), expected);
        // 交换律
        assert_eq!(gf_mult(&x1, &x127), expected);

        // 5. 交换律：gf_mult(a, b) == gf_mult(b, a)
        let a = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        let b = [
            0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa, 0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22,
            0x11, 0x00,
        ];
        assert_eq!(gf_mult(&a, &b), gf_mult(&b, &a));

        // 6. 确定性：相同输入 → 相同输出
        assert_eq!(gf_mult(&a, &b), gf_mult(&a, &b));
    }
}
