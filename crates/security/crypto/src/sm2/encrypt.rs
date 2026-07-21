//! SM2 公钥加密算法 (GB/T 32918.4-2016).
//!
//! 提供 SM2 公钥加密与解密功能，基于椭圆曲线离散对数问题（ECDLP）和 SM3 KDF。
//!
//! # 算法概述
//! SM2 公钥加密使用接收方公钥 P 加密，接收方私钥 d 解密。
//! 共享秘密通过椭圆曲线 Diffie-Hellman（ECDH）计算：加密方计算 [k]P，
//! 解密方计算 [d]C1 = [d][k]G = [dk]G，二者得到相同的 (x2, y2)。
//!
//! ## 加密流程
//! 1. 生成随机 k ∈ [1, n-1]
//! 2. C1 = [k]G（椭圆曲线点，65 字节未压缩格式 04 ‖ x1 ‖ y1）
//! 3. (x2, y2) = [k]P（共享秘密点）
//! 4. C3 = SM3(x2 ‖ M ‖ y2)（32 字节）
//! 5. C2 = M XOR KDF(x2 ‖ y2, len(M))（与 M 等长）
//! 6. 输出 C1 ‖ C3 ‖ C2（国标顺序）
//!
//! ## 解密流程
//! 1. 解析 C1（65 字节）、C3（32 字节）、C2（剩余字节）
//! 2. 验证 C1 在曲线上
//! 3. (x2, y2) = [d]C1（共享秘密点）
//! 4. M' = C2 XOR KDF(x2 ‖ y2, len(C2))（先恢复明文）
//! 5. C3' = SM3(x2 ‖ M' ‖ y2)，恒定时间比较 C3' == C3
//! 6. 输出 M'
//!
//! # no_std 合规
//! 仅使用 `core::*` / `alloc::*`，不依赖 `std::*`。
//!
//! # 参考
//! - GB/T 32918.4-2016 信息安全技术 SM2 椭圆曲线公钥密码算法 第4部分：公钥加密算法

use alloc::vec::Vec;

use super::{EcPoint, Sm2PrivateKey, Sm2PublicKey, SM2_N};
use crate::bigint::U256;
use crate::constant_time::ct_eq;
use crate::error::CryptoError;
use crate::rng::CsRng;
use crate::sm3::Sm3Hasher;

// ============================================================
// KDF (Key Derivation Function)
// ============================================================

/// KDF（密钥派生函数），基于 SM3（GB/T 32918.4-2016）.
///
/// 从共享秘密 Z 派生 klen 字节的密钥。
///
/// # 算法
/// 1. ct = 0x00000001
/// 2. 循环：K_i = SM3(Z ‖ ct)，ct 递增
/// 3. 拼接 K_1 ‖ K_2 ‖ ... 并截断至 klen 字节
///
/// # 参数
/// - `z`: 共享秘密字节串（通常为 x2 ‖ y2）
/// - `klen`: 输出密钥长度（字节）
///
/// # 返回
/// klen 字节的派生密钥。若 klen = 0，返回空 Vec。
fn kdf(z: &[u8], klen: usize) -> Vec<u8> {
    let mut output = Vec::with_capacity(klen);
    let mut ct: u32 = 1;
    while output.len() < klen {
        let mut hasher = Sm3Hasher::new();
        hasher.update(z);
        hasher.update(&ct.to_be_bytes());
        let block = hasher.finalize();
        let remaining = klen - output.len();
        let to_copy = if remaining < 32 { remaining } else { 32 };
        output.extend_from_slice(&block[..to_copy]);
        ct = ct.wrapping_add(1);
    }
    output
}

// ============================================================
// SM2 加密/解密
// ============================================================

/// SM2 公钥加密.
///
/// 使用接收方公钥对明文进行加密，输出国标顺序的密文 C1 ‖ C3 ‖ C2。
///
/// # 输出格式（GB/T 32918.4-2016）
/// - C1: 65 字节椭圆曲线点（04 ‖ x1 ‖ y1，未压缩格式）
/// - C3: 32 字节 SM3 哈希（SM3(x2 ‖ M ‖ y2)）
/// - C2: len(M) 字节密文（M XOR KDF(x2 ‖ y2, len(M))）
///
/// # 参数
/// - `plaintext`: 待加密明文 M
/// - `pk`: 接收方公钥
/// - `rng`: 密码学安全随机数生成器（用于生成随机 k）
///
/// # 返回
/// 密文 `C1 ‖ C3 ‖ C2`，长度为 `65 + 32 + len(M)` 字节。
///
/// # 错误
/// - [`CryptoError::InvalidPointEncoding`][]: 公钥为无穷远点
pub fn sm2_encrypt(
    plaintext: &[u8],
    pk: &Sm2PublicKey,
    rng: &mut CsRng,
) -> Result<Vec<u8>, CryptoError> {
    if pk.point.is_infinity {
        return Err(CryptoError::InvalidPointEncoding);
    }

    loop {
        // 1. 生成随机 k ∈ [1, n-1]
        let k = loop {
            let mut buf = [0u8; 32];
            rng.fill_bytes(&mut buf);
            let candidate = U256::from_be_bytes(&buf);
            if !candidate.is_zero() && candidate < SM2_N {
                break candidate;
            }
        };

        // 2. C1 = [k]G
        let c1_point = EcPoint::scalar_base_mult(&k);
        if c1_point.is_infinity {
            continue;
        }
        let c1 = c1_point.to_bytes_uncompressed(); // 65 字节

        // 3. (x2, y2) = [k]P
        let s_point = pk.point.scalar_mult(&k);
        if s_point.is_infinity {
            continue;
        }
        let x2_bytes = s_point.x.to_be_bytes();
        let y2_bytes = s_point.y.to_be_bytes();

        // 4. C3 = SM3(x2 ‖ M ‖ y2)
        let mut hasher = Sm3Hasher::new();
        hasher.update(&x2_bytes);
        hasher.update(plaintext);
        hasher.update(&y2_bytes);
        let c3 = hasher.finalize(); // 32 字节

        // 5. C2 = M XOR KDF(x2 ‖ y2, len(M))
        let mut kdf_input = Vec::with_capacity(64);
        kdf_input.extend_from_slice(&x2_bytes);
        kdf_input.extend_from_slice(&y2_bytes);
        let kdf_output = kdf(&kdf_input, plaintext.len());
        let c2: Vec<u8> = plaintext
            .iter()
            .zip(kdf_output.iter())
            .map(|(m, kb)| m ^ kb)
            .collect();

        // 6. 输出 C1 ‖ C3 ‖ C2（国标顺序）
        let mut ciphertext = Vec::with_capacity(65 + 32 + plaintext.len());
        ciphertext.extend_from_slice(&c1);
        ciphertext.extend_from_slice(&c3);
        ciphertext.extend_from_slice(&c2);
        return Ok(ciphertext);
    }
}

/// SM2 私钥解密.
///
/// 使用接收方私钥对密文进行解密，恢复原始明文。
///
/// # 输入格式（GB/T 32918.4-2016）
/// - C1: 65 字节椭圆曲线点（04 ‖ x1 ‖ y1）
/// - C3: 32 字节 SM3 哈希
/// - C2: 密文
///
/// # 参数
/// - `ciphertext`: 密文 `C1 ‖ C3 ‖ C2`，长度 ≥ 97 字节
/// - `sk`: 接收方私钥
///
/// # 返回
/// 解密后的明文。
///
/// # 错误
/// - [`CryptoError::InvalidLength`][]: 密文长度 < 97 字节
/// - [`CryptoError::PointNotOnCurve`][]: C1 不在 SM2 曲线上
/// - [`CryptoError::InvalidPointEncoding`][]: C1 格式错误
/// - [`CryptoError::InternalError`][]: [d]C1 为无穷远点（不应发生）
/// - [`CryptoError::TagMismatch`][]: C3 校验失败（密文被篡改或密钥不匹配）
pub fn sm2_decrypt(ciphertext: &[u8], sk: &Sm2PrivateKey) -> Result<Vec<u8>, CryptoError> {
    // 1. 解析 C1 ‖ C3 ‖ C2
    if ciphertext.len() < 65 + 32 {
        return Err(CryptoError::InvalidLength {
            expected: 97,
            actual: ciphertext.len(),
        });
    }
    let c1_bytes = &ciphertext[..65];
    let c3 = &ciphertext[65..97];
    let c2 = &ciphertext[97..];

    // 2. 解析并验证 C1（from_bytes 内部校验曲线）
    let c1_point = EcPoint::from_bytes(c1_bytes)?;

    // 3. (x2, y2) = [d]C1
    let s_point = c1_point.scalar_mult(&sk.d);
    if s_point.is_infinity {
        return Err(CryptoError::InternalError);
    }
    let x2_bytes = s_point.x.to_be_bytes();
    let y2_bytes = s_point.y.to_be_bytes();

    // 4. 先恢复明文 M' = C2 XOR KDF(x2 ‖ y2, len(C2))
    //    （GB/T 32918.4-2016：必须先恢复 M'，再用 M' 计算 C3'）
    let mut kdf_input = Vec::with_capacity(64);
    kdf_input.extend_from_slice(&x2_bytes);
    kdf_input.extend_from_slice(&y2_bytes);
    let kdf_output = kdf(&kdf_input, c2.len());
    let plaintext: Vec<u8> = c2
        .iter()
        .zip(kdf_output.iter())
        .map(|(c, kb)| c ^ kb)
        .collect();

    // 5. C3' = SM3(x2 ‖ M' ‖ y2)，恒定时间比较 C3' == C3
    let mut hasher = Sm3Hasher::new();
    hasher.update(&x2_bytes);
    hasher.update(&plaintext);
    hasher.update(&y2_bytes);
    let c3_prime = hasher.finalize();

    if !ct_eq(&c3_prime, c3) {
        return Err(CryptoError::TagMismatch);
    }

    Ok(plaintext)
}

// ============================================================
// Unit Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::CsRng;
    use crate::sm2::Sm2KeyPair;

    /// 生成测试用密钥对.
    fn gen_keypair(rng: &mut CsRng) -> Sm2KeyPair {
        Sm2KeyPair::generate(rng).expect("密钥对生成失败")
    }

    // ============================================================
    // 加密/解密往返测试
    // ============================================================

    #[test]
    fn test_encrypt_decrypt_round_trip() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let plaintext = b"Hello, SM2 Encryption!";

        let ciphertext = sm2_encrypt(plaintext, &kp.public_key, &mut rng).expect("加密失败");
        let decrypted = sm2_decrypt(&ciphertext, &kp.private_key).expect("解密失败");
        assert_eq!(decrypted, plaintext, "解密结果应与原文一致");
    }

    #[test]
    fn test_encrypt_empty_plaintext() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let plaintext = b"";

        let ciphertext = sm2_encrypt(plaintext, &kp.public_key, &mut rng).expect("加密失败");
        // C1(65) + C3(32) + C2(0) = 97 字节
        assert_eq!(ciphertext.len(), 97, "空明文密文长度应为 97");
        let decrypted = sm2_decrypt(&ciphertext, &kp.private_key).expect("解密失败");
        assert_eq!(decrypted, plaintext, "解密结果应为空");
    }

    #[test]
    fn test_encrypt_single_byte() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let plaintext = b"A";

        let ciphertext = sm2_encrypt(plaintext, &kp.public_key, &mut rng).expect("加密失败");
        assert_eq!(ciphertext.len(), 98, "1 字节明文密文长度应为 98");
        let decrypted = sm2_decrypt(&ciphertext, &kp.private_key).expect("解密失败");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_large_plaintext() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let plaintext = [0xABu8; 1000];

        let ciphertext = sm2_encrypt(&plaintext, &kp.public_key, &mut rng).expect("加密失败");
        assert_eq!(ciphertext.len(), 65 + 32 + 1000, "大消息密文长度不正确");
        let decrypted = sm2_decrypt(&ciphertext, &kp.private_key).expect("解密失败");
        assert_eq!(decrypted, plaintext);
    }

    // ============================================================
    // 解密错误测试
    // ============================================================

    #[test]
    fn test_decrypt_invalid_ciphertext_too_short() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let short_ct = [0u8; 96]; // < 97
        let result = sm2_decrypt(&short_ct, &kp.private_key);
        assert_eq!(
            result,
            Err(CryptoError::InvalidLength {
                expected: 97,
                actual: 96
            })
        );
    }

    #[test]
    fn test_decrypt_invalid_c1_not_on_curve() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let plaintext = b"test message";

        let mut ciphertext = sm2_encrypt(plaintext, &kp.public_key, &mut rng).expect("加密失败");
        // 破坏 C1 的 x 坐标首字节（保持 04 前缀），使点不在曲线上
        ciphertext[1] ^= 0xFF;
        let result = sm2_decrypt(&ciphertext, &kp.private_key);
        assert_eq!(result, Err(CryptoError::PointNotOnCurve));
    }

    #[test]
    fn test_decrypt_c3_mismatch() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let plaintext = b"test message";

        let mut ciphertext = sm2_encrypt(plaintext, &kp.public_key, &mut rng).expect("加密失败");
        // 破坏 C3（偏移 65..97）
        ciphertext[65] ^= 0xFF;
        let result = sm2_decrypt(&ciphertext, &kp.private_key);
        assert_eq!(result, Err(CryptoError::TagMismatch));
    }

    #[test]
    fn test_decrypt_c2_tampered() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let plaintext = b"test message for c2 tamper";

        let mut ciphertext = sm2_encrypt(plaintext, &kp.public_key, &mut rng).expect("加密失败");
        // 破坏 C2（偏移 97..），使 C3' 校验失败
        ciphertext[97] ^= 0xFF;
        let result = sm2_decrypt(&ciphertext, &kp.private_key);
        assert_eq!(result, Err(CryptoError::TagMismatch));
    }

    #[test]
    fn test_decrypt_wrong_private_key() {
        let mut rng = CsRng::new();
        let kp1 = gen_keypair(&mut rng);
        let kp2 = gen_keypair(&mut rng);
        let plaintext = b"secret message";

        let ciphertext = sm2_encrypt(plaintext, &kp1.public_key, &mut rng).expect("加密失败");
        // 使用错误的私钥解密
        let result = sm2_decrypt(&ciphertext, &kp2.private_key);
        assert_eq!(result, Err(CryptoError::TagMismatch));
    }

    // ============================================================
    // KDF 测试
    // ============================================================

    #[test]
    fn test_kdf_deterministic() {
        let z = b"shared secret for kdf test";
        let k1 = kdf(z, 64);
        let k2 = kdf(z, 64);
        assert_eq!(k1, k2, "相同输入应产生相同输出");
    }

    #[test]
    fn test_kdf_length() {
        let z = b"test input";
        for klen in [0usize, 1, 16, 31, 32, 33, 64, 100, 256] {
            let k = kdf(z, klen);
            assert_eq!(k.len(), klen, "KDF 输出长度应为 {}", klen);
        }
    }

    #[test]
    fn test_kdf_multi_block() {
        let z = b"multi block kdf test input";
        let klen = 100; // > 32，需要多个 SM3 块
        let k = kdf(z, klen);
        assert_eq!(k.len(), klen);
        // 验证不同块不重复（K_1 ≠ K_2）
        assert_ne!(&k[..32], &k[32..64], "不同 KDF 块不应相同");
    }

    // ============================================================
    // 随机性测试
    // ============================================================

    #[test]
    fn test_encrypt_different_k_different_ciphertext() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let plaintext = b"same plaintext for both encryptions";

        // 同一明文加密两次，使用同一 RNG（k 不同）
        let ct1 = sm2_encrypt(plaintext, &kp.public_key, &mut rng).expect("加密失败");
        let ct2 = sm2_encrypt(plaintext, &kp.public_key, &mut rng).expect("加密失败");
        assert_ne!(ct1, ct2, "不同 k 应产生不同密文");

        // 两个密文都应能正确解密
        let pt1 = sm2_decrypt(&ct1, &kp.private_key).expect("解密失败");
        let pt2 = sm2_decrypt(&ct2, &kp.private_key).expect("解密失败");
        assert_eq!(pt1, plaintext, "第一个密文应正确解密");
        assert_eq!(pt2, plaintext, "第二个密文应正确解密");
    }
}
