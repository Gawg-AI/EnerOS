//! 模型签名验证：可信公钥列表 + 吊销列表 + SM2 验签.
//!
//! 验证顺序固定：①吊销列表命中 → [`VerifyError::KeyRevoked`]；②不在可信
//! 列表 → [`VerifyError::KeyNotTrusted`]；③调用 eneros-crypto `sm2_verify`
//! 对模型原始数据验签（内部含 Z 值拼接 + SM3 摘要，GB/T 32918.2），密码学
//! 内部错误经 `?` 映射为 [`VerifyError::Crypto`]，验签布尔结果原样返回。

use alloc::vec::Vec;

use eneros_crypto::{sm2_verify, Sm2PublicKey};

use crate::{ModelSig, VerifyError};

/// 模型签名验证器（可信公钥 + 吊销公钥两份列表）.
pub struct ModelSignatureVerifier {
    /// 可信签名者公钥列表（去重维护）.
    trusted_keys: Vec<Sm2PublicKey>,
    /// 已吊销签名者公钥列表（去重维护；吊销优先于可信）.
    revoked_keys: Vec<Sm2PublicKey>,
}

impl ModelSignatureVerifier {
    /// 创建空验证器（无可信公钥、无吊销公钥）.
    pub fn new() -> Self {
        Self {
            trusted_keys: Vec::new(),
            revoked_keys: Vec::new(),
        }
    }

    /// 验证模型签名.
    ///
    /// 顺序：吊销检查 → 可信检查 → SM2 验签。返回 `Ok(true)` 表示签名有效
    /// 且来源可信；`Ok(false)` 表示签名与数据不匹配（篡改或伪造）。
    pub fn verify(&self, model_data: &[u8], sig: &ModelSig) -> Result<bool, VerifyError> {
        // ① 吊销优先：已吊销公钥直接拒绝（即使仍在可信列表）
        if self.revoked_keys.contains(&sig.signer_pubkey) {
            return Err(VerifyError::KeyRevoked);
        }
        // ② 可信检查：未登记为可信的公钥拒绝
        if !self.trusted_keys.contains(&sig.signer_pubkey) {
            return Err(VerifyError::KeyNotTrusted);
        }
        // ③ SM2 验签（对原始消息，内部含 Z 值 + SM3，GB/T 32918.2）
        let ok = sm2_verify(model_data, &sig.signature, &sig.signer_pubkey)?;
        Ok(ok)
    }

    /// 添加可信公钥（已存在则忽略，保持去重）.
    pub fn add_trusted_key(&mut self, key: Sm2PublicKey) {
        if !self.trusted_keys.contains(&key) {
            self.trusted_keys.push(key);
        }
    }

    /// 吊销公钥：从可信列表移除 + 追加到吊销列表（吊销列表去重）.
    pub fn revoke_key(&mut self, key: &Sm2PublicKey) {
        self.trusted_keys.retain(|k| k != key);
        if !self.revoked_keys.contains(key) {
            self.revoked_keys.push(*key);
        }
    }

    /// 可信公钥数量（测试观察用）.
    #[cfg(test)]
    pub fn trusted_count(&self) -> usize {
        self.trusted_keys.len()
    }

    /// 已吊销公钥数量（测试观察用）.
    #[cfg(test)]
    pub fn revoked_count(&self) -> usize {
        self.revoked_keys.len()
    }
}

impl Default for ModelSignatureVerifier {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 单元测试 SIG6~SIG10（测试模块内允许 std）
// ============================================================

#[cfg(test)]
mod tests {
    use alloc::string::String;

    use eneros_crypto::{sm2_sign, CsRng, Sm2KeyPair};

    use super::*;

    /// 构造一对密钥 + 对模型数据的签名（测试夹具）.
    fn make_signed_model(data: &[u8]) -> (Sm2KeyPair, ModelSig) {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).expect("密钥对生成");
        let signature = sm2_sign(data, &kp.private_key, &kp.public_key, &mut rng).expect("签名");
        let sig = ModelSig {
            signature,
            signer_pubkey: kp.public_key,
            signer_id: String::from("vendor-test-001"),
        };
        (kp, sig)
    }

    /// SIG6：生成密钥对 → sm2_sign 签名 → 公钥加入可信 → verify 返回 Ok(true).
    #[test]
    fn sig6_trusted_signature_verifies() {
        let data = b"model-weights-blob-v1";
        let (kp, sig) = make_signed_model(data);
        let mut verifier = ModelSignatureVerifier::new();
        verifier.add_trusted_key(kp.public_key);

        let result = verifier.verify(data, &sig);
        assert_eq!(result, Ok(true), "可信签名者 + 未篡改数据必须验签通过");
    }

    /// SIG7：篡改模型数据一字节后 verify 不再返回 Ok(true)（签名与数据不匹配）.
    #[test]
    fn sig7_tampered_data_rejected() {
        let data = b"model-weights-blob-v1";
        let (kp, sig) = make_signed_model(data);
        let mut verifier = ModelSignatureVerifier::new();
        verifier.add_trusted_key(kp.public_key);

        let mut tampered = data.to_vec();
        tampered[0] ^= 0xFF; // 篡改首字节
        let result = verifier.verify(&tampered, &sig);
        assert_ne!(result, Ok(true), "篡改数据不得验签通过");
        // 国密验签对不匹配返回 Ok(false)（非密码学内部错误）
        assert_eq!(result, Ok(false), "篡改应判签名不匹配而非内部错误");
    }

    /// SIG8：revoke 签名者公钥后 verify → Err(KeyRevoked).
    #[test]
    fn sig8_revoked_key_rejected() {
        let data = b"model-weights-blob-v1";
        let (kp, sig) = make_signed_model(data);
        let mut verifier = ModelSignatureVerifier::new();
        verifier.add_trusted_key(kp.public_key);
        verifier.revoke_key(&kp.public_key);

        let result = verifier.verify(data, &sig);
        assert_eq!(result, Err(VerifyError::KeyRevoked), "已吊销公钥必须拒绝");
    }

    /// SIG9：签名者公钥未加入可信列表 → Err(KeyNotTrusted).
    #[test]
    fn sig9_untrusted_key_rejected() {
        let data = b"model-weights-blob-v1";
        let (_kp, sig) = make_signed_model(data);
        let verifier = ModelSignatureVerifier::new(); // 空可信列表

        let result = verifier.verify(data, &sig);
        assert_eq!(
            result,
            Err(VerifyError::KeyNotTrusted),
            "不可信公钥必须拒绝"
        );
    }

    /// SIG10：revoke 后可信列表已移除且吊销列表不重复——同一 key revoke 两次，
    /// revoked 仅一条，trusted 保持为空.
    #[test]
    fn sig10_revoke_idempotent() {
        let data = b"model-weights-blob-v1";
        let (kp, _sig) = make_signed_model(data);
        let mut verifier = ModelSignatureVerifier::new();
        verifier.add_trusted_key(kp.public_key);
        assert_eq!(verifier.trusted_count(), 1);

        verifier.revoke_key(&kp.public_key);
        assert_eq!(verifier.trusted_count(), 0, "revoke 后可信列表必须移除");
        assert_eq!(verifier.revoked_count(), 1, "revoke 后吊销列表必须有一条");

        // 重复 revoke：吊销列表去重，不得追加第二条
        verifier.revoke_key(&kp.public_key);
        assert_eq!(verifier.trusted_count(), 0, "重复 revoke 可信列表仍为空");
        assert_eq!(verifier.revoked_count(), 1, "重复 revoke 吊销列表不得重复");

        // 去重验证：add_trusted_key 重复添加同一 key 仅一条
        let mut v2 = ModelSignatureVerifier::new();
        v2.add_trusted_key(kp.public_key);
        v2.add_trusted_key(kp.public_key);
        assert_eq!(v2.trusted_count(), 1, "可信列表必须去重");
    }
}
