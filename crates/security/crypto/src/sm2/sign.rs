//! SM2 数字签名算法 (GB/T 32918.2-2017).
//!
//! 提供 SM2 数字签名生成与验证功能，基于椭圆曲线密码算法。
//!
//! # 算法概述
//! SM2 数字签名使用椭圆曲线离散对数问题（ECDLP）保证安全性。
//! 签名过程涉及用户私钥 `d` 和随机数 `k`，验证过程使用公钥 `P = d * G`。
//!
//! ## 签名流程
//! 1. 计算 Z = SM3(ENTL ‖ ID ‖ a ‖ b ‖ Gx ‖ Gy ‖ Px ‖ Py)
//! 2. 计算 e = SM3(Z ‖ M)
//! 3. 生成随机 k ∈ [1, n-1]，计算 (x1, y1) = k * G
//! 4. r = (e + x1) mod n，若 r = 0 或 r + k = n 则重新生成 k
//! 5. s = ((1 + d)^(-1) * (k - r * d)) mod n，若 s = 0 则重新生成 k
//! 6. 输出签名 (r, s)
//!
//! ## 验签流程
//! 1. 验证 r, s ∈ [1, n-1]
//! 2. 计算 Z 和 e（与签名一致）
//! 3. t = (r + s) mod n，若 t = 0 则验证失败
//! 4. (x1, y1) = s * G + t * P
//! 5. R = (e + x1) mod n
//! 6. 验证 R == r（恒定时间比较）
//!
//! # no_std 合规
//! 仅使用 `core::*` / `alloc::*`，不依赖 `std::*`。
//!
//! # 参考
//! - GB/T 32918.2-2017 信息安全技术 SM2 椭圆曲线公钥密码算法 第2部分：数字签名算法

use alloc::vec::Vec;

use super::{EcPoint, Sm2PrivateKey, Sm2PublicKey, SM2_A, SM2_B, SM2_GX, SM2_GY, SM2_N};
use crate::bigint::U256;
use crate::constant_time::ct_eq;
use crate::error::CryptoError;
use crate::rng::CsRng;
use crate::sm3::Sm3Hasher;

// ============================================================
// Sm2Signature: SM2 签名 (r, s)
// ============================================================

/// SM2 签名值 (r, s).
///
/// 每个分量为 32 字节大端序，共 64 字节。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sm2Signature {
    /// 签名分量 r（32 字节大端序）.
    pub r: [u8; 32],
    /// 签名分量 s（32 字节大端序）.
    pub s: [u8; 32],
}

impl Sm2Signature {
    /// 编码为 64 字节格式 (r ‖ s).
    pub fn to_bytes(&self) -> [u8; 64] {
        let mut out = [0u8; 64];
        out[..32].copy_from_slice(&self.r);
        out[32..].copy_from_slice(&self.s);
        out
    }

    /// 从 64 字节 (r ‖ s) 解码.
    pub fn from_bytes(bytes: &[u8; 64]) -> Self {
        let mut r = [0u8; 32];
        let mut s = [0u8; 32];
        r.copy_from_slice(&bytes[..32]);
        s.copy_from_slice(&bytes[32..]);
        Self { r, s }
    }
}

// ============================================================
// Sm2Signer: SM2 签名器
// ============================================================

/// SM2 签名器.
///
/// 封装用户 ID 并提供签名/验签功能。
/// 默认用户 ID 为 `"1234567812345678"`（国标默认值）。
pub struct Sm2Signer {
    /// 用户 ID（国标默认 "1234567812345678"）.
    user_id: Vec<u8>,
}

impl Sm2Signer {
    /// 创建签名器，使用默认用户 ID "1234567812345678".
    pub fn new() -> Self {
        Self {
            user_id: b"1234567812345678".to_vec(),
        }
    }

    /// 创建签名器，使用自定义用户 ID.
    pub fn with_user_id(user_id: &[u8]) -> Self {
        Self {
            user_id: user_id.to_vec(),
        }
    }

    /// 返回用户 ID.
    pub fn user_id(&self) -> &[u8] {
        &self.user_id
    }

    /// 计算 Z 值: Z = SM3(ENTL ‖ ID ‖ a ‖ b ‖ Gx ‖ Gy ‖ Px ‖ Py).
    ///
    /// 其中 ENTL 为用户 ID 的比特长度（16 位大端序）。
    pub fn compute_z(&self, public_key: &Sm2PublicKey) -> [u8; 32] {
        let mut hasher = Sm3Hasher::new();
        // ENTL = 比特长度（大端 16 位）
        let entl = (self.user_id.len() * 8) as u16;
        hasher.update(&entl.to_be_bytes());
        // 用户 ID
        hasher.update(&self.user_id);
        // 曲线参数 a, b
        hasher.update(&SM2_A.to_be_bytes());
        hasher.update(&SM2_B.to_be_bytes());
        // 基点 Gx, Gy
        hasher.update(&SM2_GX.to_be_bytes());
        hasher.update(&SM2_GY.to_be_bytes());
        // 公钥 Px, Py
        hasher.update(&public_key.point.x.to_be_bytes());
        hasher.update(&public_key.point.y.to_be_bytes());
        hasher.finalize()
    }

    /// 对消息进行签名.
    ///
    /// # 参数
    /// - `msg`: 待签名消息
    /// - `sk`: 私钥
    /// - `pk`: 对应公钥（用于计算 Z 值）
    /// - `rng`: 随机数生成器（用于生成 k）
    ///
    /// # 返回
    /// 签名值 `Sm2Signature` 或错误。
    pub fn sign(
        &self,
        msg: &[u8],
        sk: &Sm2PrivateKey,
        pk: &Sm2PublicKey,
        rng: &mut CsRng,
    ) -> Result<Sm2Signature, CryptoError> {
        // Z = SM3(ENTL ‖ ID ‖ a ‖ b ‖ Gx ‖ Gy ‖ Px ‖ Py)
        let z = self.compute_z(pk);
        // e = SM3(Z ‖ M)
        let mut hasher = Sm3Hasher::new();
        hasher.update(&z);
        hasher.update(msg);
        let e_bytes = hasher.finalize();
        let e = U256::from_be_bytes(&e_bytes);

        loop {
            // 生成随机 k ∈ [1, n-1]
            let k = loop {
                let mut buf = [0u8; 32];
                rng.fill_bytes(&mut buf);
                let candidate = U256::from_be_bytes(&buf);
                if !candidate.is_zero() && candidate < SM2_N {
                    break candidate;
                }
            };

            // (x1, y1) = k * G
            let point = EcPoint::scalar_base_mult(&k);
            if point.is_infinity {
                continue;
            }

            // r = (e + x1) mod n
            let r = e.add_mod(&point.x, &SM2_N);
            if r.is_zero() {
                continue;
            }
            // r + k == n → 重试
            let r_plus_k = r.add_mod(&k, &SM2_N);
            if r_plus_k.is_zero() {
                continue;
            }

            // s = ((1 + d)^(-1) * (k - r * d)) mod n
            let one_plus_d = crate::bigint::ONE.add_mod(&sk.d, &SM2_N);
            let inv_one_plus_d = match one_plus_d.inv_mod(&SM2_N) {
                Ok(inv) => inv,
                Err(_) => continue,
            };
            let r_times_d = r.mul_mod(&sk.d, &SM2_N);
            let k_minus_rd = k.sub_mod(&r_times_d, &SM2_N);
            let s = inv_one_plus_d.mul_mod(&k_minus_rd, &SM2_N);
            if s.is_zero() {
                continue;
            }

            return Ok(Sm2Signature {
                r: r.to_be_bytes(),
                s: s.to_be_bytes(),
            });
        }
    }

    /// 验证签名.
    ///
    /// # 参数
    /// - `msg`: 原始消息
    /// - `sig`: 待验证签名
    /// - `pk`: 签名者公钥
    ///
    /// # 返回
    /// - `Ok(true)`: 签名有效
    /// - `Ok(false)`: 签名无效
    /// - `Err(_)`: 内部错误
    pub fn verify(
        &self,
        msg: &[u8],
        sig: &Sm2Signature,
        pk: &Sm2PublicKey,
    ) -> Result<bool, CryptoError> {
        let r = U256::from_be_bytes(&sig.r);
        let s = U256::from_be_bytes(&sig.s);

        // 验证 r, s ∈ [1, n-1]
        if r.is_zero() || r >= SM2_N || s.is_zero() || s >= SM2_N {
            return Ok(false);
        }

        // Z 和 e
        let z = self.compute_z(pk);
        let mut hasher = Sm3Hasher::new();
        hasher.update(&z);
        hasher.update(msg);
        let e_bytes = hasher.finalize();
        let e = U256::from_be_bytes(&e_bytes);

        // t = (r + s) mod n
        let t = r.add_mod(&s, &SM2_N);
        if t.is_zero() {
            return Ok(false);
        }

        // (x1, y1) = s * G + t * P
        let s_g = EcPoint::scalar_base_mult(&s);
        let t_p = pk.point.scalar_mult(&t);
        let point = s_g.add(&t_p);
        if point.is_infinity {
            return Ok(false);
        }

        // R = (e + x1) mod n
        let r_check = e.add_mod(&point.x, &SM2_N);

        // 恒定时间比较
        let r_check_bytes = r_check.to_be_bytes();
        Ok(ct_eq(&r_check_bytes, &sig.r))
    }
}

impl Default for Sm2Signer {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// Convenience functions
// ============================================================

/// 使用默认 Sm2Signer 对消息进行签名.
pub fn sm2_sign(
    msg: &[u8],
    sk: &Sm2PrivateKey,
    pk: &Sm2PublicKey,
    rng: &mut CsRng,
) -> Result<Sm2Signature, CryptoError> {
    Sm2Signer::new().sign(msg, sk, pk, rng)
}

/// 使用默认 Sm2Signer 验证签名.
pub fn sm2_verify(msg: &[u8], sig: &Sm2Signature, pk: &Sm2PublicKey) -> Result<bool, CryptoError> {
    Sm2Signer::new().verify(msg, sig, pk)
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
    // 签名/验签往返测试
    // ============================================================

    #[test]
    fn test_sign_verify_round_trip() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let signer = Sm2Signer::new();
        let msg = b"Hello, SM2!";

        let sig = signer
            .sign(msg, &kp.private_key, &kp.public_key, &mut rng)
            .expect("签名失败");
        let valid = signer.verify(msg, &sig, &kp.public_key).expect("验签失败");
        assert!(valid, "签名验证应通过");
    }

    #[test]
    fn test_verify_tampered_message_fails() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let signer = Sm2Signer::new();
        let msg = b"original message";

        let sig = signer
            .sign(msg, &kp.private_key, &kp.public_key, &mut rng)
            .expect("签名失败");

        // 篡改消息
        let tampered = b"tampered message";
        let valid = signer
            .verify(tampered, &sig, &kp.public_key)
            .expect("验签失败");
        assert!(!valid, "篡改消息后验签应失败");
    }

    #[test]
    fn test_verify_tampered_signature_r_fails() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let signer = Sm2Signer::new();
        let msg = b"test message";

        let mut sig = signer
            .sign(msg, &kp.private_key, &kp.public_key, &mut rng)
            .expect("签名失败");

        // 篡改 r
        sig.r[0] ^= 0xFF;
        let valid = signer.verify(msg, &sig, &kp.public_key).expect("验签失败");
        assert!(!valid, "篡改 r 后验签应失败");
    }

    #[test]
    fn test_verify_tampered_signature_s_fails() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let signer = Sm2Signer::new();
        let msg = b"test message";

        let mut sig = signer
            .sign(msg, &kp.private_key, &kp.public_key, &mut rng)
            .expect("签名失败");

        // 篡改 s
        sig.s[0] ^= 0xFF;
        let valid = signer.verify(msg, &sig, &kp.public_key).expect("验签失败");
        assert!(!valid, "篡改 s 后验签应失败");
    }

    #[test]
    fn test_verify_wrong_public_key_fails() {
        let mut rng = CsRng::new();
        let kp1 = gen_keypair(&mut rng);
        let kp2 = gen_keypair(&mut rng);
        let signer = Sm2Signer::new();
        let msg = b"test message";

        let sig = signer
            .sign(msg, &kp1.private_key, &kp1.public_key, &mut rng)
            .expect("签名失败");

        // 使用错误的公钥验签
        let valid = signer.verify(msg, &sig, &kp2.public_key).expect("验签失败");
        assert!(!valid, "使用错误公钥验签应失败");
    }

    // ============================================================
    // 序列化测试
    // ============================================================

    #[test]
    fn test_signature_serialization() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let signer = Sm2Signer::new();
        let msg = b"serialization test";

        let sig = signer
            .sign(msg, &kp.private_key, &kp.public_key, &mut rng)
            .expect("签名失败");

        let bytes = sig.to_bytes();
        assert_eq!(bytes.len(), 64);
        let restored = Sm2Signature::from_bytes(&bytes);
        assert_eq!(sig, restored, "序列化往返应一致");
    }

    // ============================================================
    // 用户 ID 测试
    // ============================================================

    #[test]
    fn test_signer_default_user_id() {
        let signer = Sm2Signer::new();
        assert_eq!(signer.user_id(), b"1234567812345678");
    }

    #[test]
    fn test_signer_custom_user_id() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let msg = b"custom user id test";

        // 默认用户 ID 签名
        let default_signer = Sm2Signer::new();
        let sig_default = default_signer
            .sign(msg, &kp.private_key, &kp.public_key, &mut rng)
            .expect("签名失败");

        // 自定义用户 ID 签名
        let custom_signer = Sm2Signer::with_user_id(b"custom-id-12345");
        assert_eq!(custom_signer.user_id(), b"custom-id-12345");
        let sig_custom = custom_signer
            .sign(msg, &kp.private_key, &kp.public_key, &mut rng)
            .expect("签名失败");

        // 不同用户 ID 应产生不同签名
        assert_ne!(sig_default, sig_custom, "不同用户 ID 应产生不同签名");
    }

    #[test]
    fn test_signer_with_user_id_verify() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let custom_id = b"my-custom-user-id";
        let signer = Sm2Signer::with_user_id(custom_id);
        let msg = b"custom id verify test";

        let sig = signer
            .sign(msg, &kp.private_key, &kp.public_key, &mut rng)
            .expect("签名失败");

        let valid = signer.verify(msg, &sig, &kp.public_key).expect("验签失败");
        assert!(valid, "相同用户 ID 签名/验签应通过");
    }

    #[test]
    fn test_signer_wrong_user_id_verify() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let msg = b"wrong user id test";

        // 使用 ID1 签名
        let signer1 = Sm2Signer::with_user_id(b"user-id-one");
        let sig = signer1
            .sign(msg, &kp.private_key, &kp.public_key, &mut rng)
            .expect("签名失败");

        // 使用 ID2 验签
        let signer2 = Sm2Signer::with_user_id(b"user-id-two");
        let valid = signer2.verify(msg, &sig, &kp.public_key).expect("验签失败");
        assert!(!valid, "不同用户 ID 验签应失败");
    }

    // ============================================================
    // 边界条件测试
    // ============================================================

    #[test]
    fn test_sign_invalid_private_key() {
        // 零私钥应被拒绝
        let zero_bytes = [0u8; 32];
        let result = Sm2PrivateKey::from_bytes(&zero_bytes);
        assert_eq!(result, Err(CryptoError::ScalarOutOfRange));

        // 私钥 >= n 应被拒绝
        let n_bytes = SM2_N.to_be_bytes();
        let result = Sm2PrivateKey::from_bytes(&n_bytes);
        assert_eq!(result, Err(CryptoError::ScalarOutOfRange));
    }

    #[test]
    fn test_z_value_deterministic() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let signer = Sm2Signer::new();

        // 相同公钥和用户 ID 应产生相同 Z 值
        let z1 = signer.compute_z(&kp.public_key);
        let z2 = signer.compute_z(&kp.public_key);
        assert_eq!(z1, z2, "Z 值应是确定性的");
    }

    #[test]
    fn test_multiple_signatures_different() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let signer = Sm2Signer::new();
        let msg = b"same message";

        // 同一消息签名两次应产生不同签名（k 随机）
        let sig1 = signer
            .sign(msg, &kp.private_key, &kp.public_key, &mut rng)
            .expect("签名失败");
        let sig2 = signer
            .sign(msg, &kp.private_key, &kp.public_key, &mut rng)
            .expect("签名失败");

        assert_ne!(sig1, sig2, "不同 k 应产生不同签名");

        // 两个签名都应验签通过
        assert!(
            signer.verify(msg, &sig1, &kp.public_key).unwrap(),
            "第一个签名应验签通过"
        );
        assert!(
            signer.verify(msg, &sig2, &kp.public_key).unwrap(),
            "第二个签名应验签通过"
        );
    }

    #[test]
    fn test_empty_message() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let signer = Sm2Signer::new();
        let msg = b"";

        let sig = signer
            .sign(msg, &kp.private_key, &kp.public_key, &mut rng)
            .expect("签名失败");
        let valid = signer.verify(msg, &sig, &kp.public_key).expect("验签失败");
        assert!(valid, "空消息签名应验签通过");
    }

    #[test]
    fn test_large_message() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let signer = Sm2Signer::new();
        let msg = [0xABu8; 1000];

        let sig = signer
            .sign(&msg, &kp.private_key, &kp.public_key, &mut rng)
            .expect("签名失败");
        let valid = signer.verify(&msg, &sig, &kp.public_key).expect("验签失败");
        assert!(valid, "大消息签名应验签通过");
    }

    // ============================================================
    // 便捷函数测试
    // ============================================================

    #[test]
    fn test_convenience_functions() {
        let mut rng = CsRng::new();
        let kp = gen_keypair(&mut rng);
        let msg = b"convenience test";

        let sig = sm2_sign(msg, &kp.private_key, &kp.public_key, &mut rng).expect("签名失败");
        let valid = sm2_verify(msg, &sig, &kp.public_key).expect("验签失败");
        assert!(valid, "便捷函数签名/验签应通过");
    }
}
