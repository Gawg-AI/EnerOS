//! 模型签名验证：验签纪律唯一实现（keyring）的 model-security adapter
//! （蓝图 64 卷 64.3 WP-64.3.5）.
//!
//! 自持 trusted/revoked 两 Vec + 手写三门的旧实现删除——
//! [`ModelSignatureVerifier`] 内部持 [`SignerKeyring`]：查表（→
//! `KeyNotTrusted`）→ 撤销门（→ `KeyRevoked`）→ SM2 验签（→
//! `SigMismatch`）三门单源，keyring lib「唯一实现」声称随之复真。
//! interface 方法签名零变化（pentest 消费面不改即绿——修复面窄的实证）.

use eneros_crypto::Sm2PublicKey;
use eneros_signer_keyring::{fingerprint, SignerKeyring, VerifyReject};

use crate::{ModelSig, VerifyError};

/// 模型签名验证器（验签纪律唯一实现的 model-security adapter）.
pub struct ModelSignatureVerifier {
    /// keyring 单源：查表/撤销门/SM2 验签全部委托.
    keyring: SignerKeyring,
}

impl ModelSignatureVerifier {
    /// 创建空验证器（无可信公钥、无吊销公钥）.
    pub fn new() -> Self {
        Self {
            keyring: SignerKeyring::new(),
        }
    }

    /// 验证模型签名.
    ///
    /// 三门顺序由 keyring [`SignerKeyring::verify_authenticated`] 唯一
    /// 裁决：`Ok(())` = 签名有效且来源可信；篡改/伪造 =
    /// `Err(SigMismatch)`（63.3 布尔陷阱收口保持）。
    ///
    /// 身份语义（64.3 复查登记）：来源判定 = `SM3(未压缩钥) 前 8B`
    /// 指纹查表（keyring `fingerprint`），且验签用**登记钥**而非信封
    /// 内嵌钥——keyring 化前为全钥等值 `contains`。碰撞/替换方向均
    /// fail-closed：指纹 2^-64 撞挂他条目 → 用他钥验签必 `BadSignature`；
    /// 未登记即吊销语义不变。
    pub fn verify(&self, model_data: &[u8], sig: &ModelSig) -> Result<(), VerifyError> {
        let key_id = fingerprint(&sig.signer_pubkey);
        let sig_bytes: &[u8; 64] = &sig.signature.to_bytes();
        match self
            .keyring
            .verify_authenticated(key_id, model_data, sig_bytes)
        {
            Ok(_) => Ok(()),
            Err(VerifyReject::UnknownKey { .. }) => Err(VerifyError::KeyNotTrusted),
            Err(VerifyReject::RevokedKey { .. }) => Err(VerifyError::KeyRevoked),
            Err(VerifyReject::BadSignature) => Err(VerifyError::SigMismatch),
        }
    }

    /// 添加可信公钥（同 id 覆盖 = 去重语义由 keyring `register` 保持）.
    ///
    /// 吊销终态由 keyring tombstone **单一持有**（蓝图 65 WP-65.1）：
    /// `register` 永不触碰撤销账，revoke → re-add 后 verify 仍
    /// [`VerifyError::KeyRevoked`]（SIG12 锁）。本 adapter 无需任何
    /// 恢复舞步——旧「is_revoked→register→条件 revoke」三行舞步随
    /// keyring 语义归位删除（不变式的所有者 = keyring，adapter 零补丁）.
    pub fn add_trusted_key(&mut self, key: Sm2PublicKey) {
        self.keyring.register(fingerprint(&key), &key);
    }

    /// 吊销公钥（纯 `revoke`——keyring tombstone 对未登记 id 一律记账，
    /// 未登记过的 key 也保持"吊销优先于可信"：后续 verify 一律
    /// `KeyRevoked`；旧 register-before-revoke 垫步删除）.
    pub fn revoke_key(&mut self, key: &Sm2PublicKey) {
        self.keyring.revoke(fingerprint(key));
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
        assert_eq!(result, Ok(()), "可信签名者 + 未篡改数据必须验签通过");
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
        assert_ne!(result, Ok(()), "篡改数据不得验签通过");
        // 签名不匹配 = Err(SigMismatch)（非密码学内部错误）
        assert_eq!(
            result,
            Err(VerifyError::SigMismatch),
            "篡改应判签名不匹配而非内部错误"
        );
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

    /// SIG10（WP-64.3.5 行为化改写）：revoke 幂等 + 重复 add 去重——
    /// 观测面由 count 改为 verify 判定（keyring 同 id 覆盖去重）.
    #[test]
    fn sig10_revoke_idempotent() {
        let data = b"model-weights-blob-v1";
        let (kp, sig) = make_signed_model(data);
        let mut verifier = ModelSignatureVerifier::new();
        verifier.add_trusted_key(kp.public_key);
        assert_eq!(verifier.verify(data, &sig), Ok(()));

        verifier.revoke_key(&kp.public_key);
        assert_eq!(verifier.verify(data, &sig), Err(VerifyError::KeyRevoked));

        // 重复 revoke：仍 KeyRevoked（不得翻回 KeyNotTrusted/Ok）。
        verifier.revoke_key(&kp.public_key);
        assert_eq!(verifier.verify(data, &sig), Err(VerifyError::KeyRevoked));

        // 重复 add 同一 key：去重语义（仍可信、仍可验签）。
        let mut v2 = ModelSignatureVerifier::new();
        v2.add_trusted_key(kp.public_key);
        v2.add_trusted_key(kp.public_key);
        assert_eq!(v2.verify(data, &sig), Ok(()));
    }

    /// WP-64.3.5 新增：吊销从未登记过的 key → verify 仍 KeyRevoked
    /// （"吊销优先于可信"语义保持——register+revoke 组合）.
    #[test]
    fn sig11_revoke_without_register_still_revoked() {
        let data = b"model-weights-blob-v1";
        let (kp, sig) = make_signed_model(data);
        let mut verifier = ModelSignatureVerifier::new();
        verifier.revoke_key(&kp.public_key);
        assert_eq!(
            verifier.verify(data, &sig),
            Err(VerifyError::KeyRevoked),
            "未登记即吊销的 key 不得降级为 KeyNotTrusted"
        );
    }

    /// SIG12（64 复查三轮）：吊销终态——revoke 后 re-add 不得复活.
    ///
    /// keyring 化前语义：revoked 列表只增不减，`add_trusted_key` 不触碰
    /// 吊销门（verify ①吊销 ②可信 ③验签）——revoke → add → verify 恒
    /// `Err(KeyRevoked)`.（蓝图 65 WP-65.1 起：该不变式由 keyring
    /// tombstone 单一持有——register 永不触碰撤销账；本测是消费面对照
    /// 锁，断言零变化。）
    #[test]
    fn sig12_readd_after_revoke_stays_revoked() {
        let data = b"model-weights-blob-v1";
        let (kp, sig) = make_signed_model(data);
        let mut verifier = ModelSignatureVerifier::new();
        verifier.revoke_key(&kp.public_key);
        verifier.add_trusted_key(kp.public_key);
        assert_eq!(
            verifier.verify(data, &sig),
            Err(VerifyError::KeyRevoked),
            "吊销是终态：re-add 不得洗白已吊销钥"
        );
        // 再 revoke 仍终态；换数据/换签名同样拒绝（门序无关验签结果）.
        verifier.add_trusted_key(kp.public_key);
        verifier.revoke_key(&kp.public_key);
        verifier.add_trusted_key(kp.public_key);
        assert_eq!(
            verifier.verify(data, &sig),
            Err(VerifyError::KeyRevoked),
            "多次 revoke/re-add 循环后吊销仍终态"
        );
    }
}
