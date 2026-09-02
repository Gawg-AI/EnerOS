//! # SignerKeyring（蓝图 62 卷 62.3 WP-62.3.1 · S5 收敛）
//!
//! "接入一封认证外部事实"的密钥注册/验签/撤销/修订水位纪律**唯一实现**。
//! 消费方（market_ingest / external_facts / model-harness skill+registry /
//! federation KeyRegistry）保留各自 payload schema 与业务闸，四张裸 key
//! 表与五条 register/verify 管线收敛于此。
//!
//! 设计裁决（蓝图 62 卷 62.3）：
//!
//! - key_id 对外统一 [`KeyId`]（`[u8; 8]`，SM2 公钥指纹前 8B 约定见
//!   [`fingerprint`]）；wire 带 u32 key id 的消费方经 [`alias_key_id`]
//!   在登记层映射（别名层单源于此），任何 wire 字段格式不变。
//! - `sm2_verify` 返回 `Ok(false)` 的 API 陷阱**类型化收口**：调用方
//!   永不直接触碰布尔返回——验签产出 [`VerifiedSignature`] 证明 token
//!   或 [`VerifyReject`]（`Ok(false)` 与 `Err` 双通道统一为
//!   [`VerifyReject::BadSignature`]）。
//! - 修订水位（external_id → 已见最高 revision）只升不降，纪律单点在
//!   [`SignerKeyring::advance_revision`]；判拒语义（存档/拒绝计数）
//!   保留在消费方。
//!
//! no_std + alloc（消费方为 no_std crate；不引入 std）。

#![cfg_attr(not(test), no_std)]
extern crate alloc;

use alloc::collections::BTreeMap;

use eneros_crypto::{sm2_verify, sm3_hash, Sm2PublicKey, Sm2Signature};

/// key 标识（`[u8; 8]`）。
pub type KeyId = [u8; 8];

/// u32 别名 → [`KeyId`] 确定性映射（高 4B = LE 别名、低 4B 恒 0——别名
/// 空间与指纹空间分离）。wire 带 u32 key id 的消费方（model-harness
/// manifest canonical、federation EnvelopeHdr）经此在登记层映射，
/// wire 字段本身不动。
pub fn alias_key_id(alias: u32) -> KeyId {
    let mut id = [0u8; 8];
    id[..4].copy_from_slice(&alias.to_le_bytes());
    id
}

/// SM2 公钥指纹 = SM3(未压缩公钥) 前 8B（keyring 对外统一 `[u8; 8]` 的
/// 约定推导）。
pub fn fingerprint(pk: &Sm2PublicKey) -> KeyId {
    let digest = sm3_hash(&pk.to_bytes_uncompressed());
    let mut id = [0u8; 8];
    id.copy_from_slice(&digest[..8]);
    id
}

/// 验签证明 token（不可在 keyring 外构造——持有即"已验真"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedSignature {
    _seal: (),
}

/// 验签拒绝（fail-closed）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyReject {
    /// key_id 未登记。
    UnknownKey { key_id: KeyId },
    /// key_id 已撤销。
    RevokedKey { key_id: KeyId },
    /// 签名无效（`sm2_verify` 的 `Ok(false)` 与 `Err` 统一于此——两者
    /// 都不得泄漏为 panic 或"未验即过"）。
    BadSignature,
}

/// 修订水位拒绝（只升不降纪律）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatermarkReject {
    /// 非严格递增（等值/回退）。
    NotAdvancing { seen: u32 },
}

/// 登记条目。
#[derive(Debug, Clone)]
struct KeyEntry {
    public_key: Sm2PublicKey,
    revoked: bool,
}

/// 签名密钥注册表（S5 收敛点：注册/验签/撤销/修订水位唯一实现）。
#[derive(Debug, Clone, Default)]
pub struct SignerKeyring {
    keys: BTreeMap<KeyId, KeyEntry>,
    revisions: BTreeMap<[u8; 16], u32>,
}

impl SignerKeyring {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记（同 id 覆盖——与既有消费方 `BTreeMap::insert` 语义一致）。
    pub fn register(&mut self, key_id: KeyId, pk: &Sm2PublicKey) {
        self.keys.insert(
            key_id,
            KeyEntry {
                public_key: *pk,
                revoked: false,
            },
        );
    }

    /// 撤销（立即拒验；未知 id → false）。
    pub fn revoke(&mut self, key_id: KeyId) -> bool {
        match self.keys.get_mut(&key_id) {
            Some(entry) => {
                entry.revoked = true;
                true
            }
            None => false,
        }
    }

    /// 撤销态读面。
    pub fn is_revoked(&self, key_id: KeyId) -> bool {
        self.keys.get(&key_id).is_some_and(|entry| entry.revoked)
    }

    /// 公钥读面（登记映射诊断用；验签一律走
    /// [`Self::verify_authenticated`]）。
    pub fn public_key(&self, key_id: KeyId) -> Option<&Sm2PublicKey> {
        self.keys.get(&key_id).map(|entry| &entry.public_key)
    }

    /// 已登记 key 数。
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// 类型化验签（唯一验签纪律：查表 → 撤销门 → SM2；产出证明 token
    /// 或结构化拒绝）。
    pub fn verify_authenticated(
        &self,
        key_id: KeyId,
        msg: &[u8],
        sig: &[u8; 64],
    ) -> Result<VerifiedSignature, VerifyReject> {
        let entry = self
            .keys
            .get(&key_id)
            .ok_or(VerifyReject::UnknownKey { key_id })?;
        if entry.revoked {
            return Err(VerifyReject::RevokedKey { key_id });
        }
        verify_signature(msg, &Sm2Signature::from_bytes(sig), &entry.public_key)
    }

    /// 修订水位读面（source → 已见最高 revision）。
    pub fn revision(&self, source: &[u8; 16]) -> Option<u32> {
        self.revisions.get(source).copied()
    }

    /// 修订水位推进（只升不降——严格递增才接受；等值/回退 →
    /// [`WatermarkReject::NotAdvancing`]）。调用方在自身判拒语义放行后
    /// 调用（判拒语义保留在消费方）。
    pub fn advance_revision(
        &mut self,
        source: &[u8; 16],
        revision: u32,
    ) -> Result<(), WatermarkReject> {
        match self.revisions.get(source) {
            Some(&seen) if revision <= seen => Err(WatermarkReject::NotAdvancing { seen }),
            _ => {
                self.revisions.insert(*source, revision);
                Ok(())
            }
        }
    }
}

/// 类型化验签原语（政策表自持消费者——federation KeyRegistry（u32 分配
/// + grace/retire 策略）——的管线收口点）。
///
/// 底层布尔/Err 双通道在此统一，调用方永不直接触碰布尔返回。
pub fn verify_signature(
    msg: &[u8],
    sig: &Sm2Signature,
    pk: &Sm2PublicKey,
) -> Result<VerifiedSignature, VerifyReject> {
    match sm2_verify(msg, sig, pk) {
        Ok(true) => Ok(VerifiedSignature { _seal: () }),
        _ => Err(VerifyReject::BadSignature),
    }
}

#[cfg(test)]
mod tests {
    use eneros_crypto::{sm2_sign, CsRng, Sm2KeyPair};

    use super::*;

    fn kp() -> Sm2KeyPair {
        let mut rng = CsRng::new();
        Sm2KeyPair::generate(&mut rng).expect("kp")
    }

    fn sign(kp: &Sm2KeyPair, msg: &[u8]) -> [u8; 64] {
        let mut rng = CsRng::new();
        sm2_sign(msg, &kp.private_key, &kp.public_key, &mut rng)
            .expect("sign")
            .to_bytes()
    }

    #[test]
    fn register_then_valid_signature_passes() {
        let pair = kp();
        let msg = b"canonical-market-fact-v1";
        let sig = sign(&pair, msg);
        let mut kr = SignerKeyring::new();
        kr.register(*b"testkey1", &pair.public_key);
        let verdict = kr.verify_authenticated(*b"testkey1", msg, &sig);
        assert!(verdict.is_ok(), "合法签名必须通过，got {verdict:?}");
    }

    #[test]
    fn bad_signature_is_reject_not_ok_false_leak() {
        // Ok(false) 陷阱类型化：签名语法合法但验伪 → Err(BadSignature)，
        // 绝不 panic、绝不以布尔形式泄漏到调用方。
        let pair = kp();
        let sig = sign(&pair, b"other message");
        let mut kr = SignerKeyring::new();
        kr.register([7u8; 8], &pair.public_key);
        assert_eq!(
            kr.verify_authenticated([7u8; 8], b"target message", &sig),
            Err(VerifyReject::BadSignature)
        );
    }

    #[test]
    fn malformed_signature_bytes_rejected() {
        // Err 通道同样统一为 BadSignature（64B 全零 r||s 非法点）。
        let pair = kp();
        let mut kr = SignerKeyring::new();
        kr.register([7u8; 8], &pair.public_key);
        assert_eq!(
            kr.verify_authenticated([7u8; 8], b"msg", &[0u8; 64]),
            Err(VerifyReject::BadSignature)
        );
    }

    #[test]
    fn unknown_key_rejected() {
        let pair = kp();
        let msg = b"m";
        let sig = sign(&pair, msg);
        let kr = SignerKeyring::new();
        assert_eq!(
            kr.verify_authenticated([9u8; 8], msg, &sig),
            Err(VerifyReject::UnknownKey { key_id: [9u8; 8] })
        );
    }

    #[test]
    fn revoked_key_rejected() {
        let pair = kp();
        let msg = b"m";
        let sig = sign(&pair, msg);
        let mut kr = SignerKeyring::new();
        kr.register([7u8; 8], &pair.public_key);
        assert!(kr.revoke([7u8; 8]));
        assert!(!kr.revoke([8u8; 8]), "未知 id 撤销 → false");
        assert_eq!(
            kr.verify_authenticated([7u8; 8], msg, &sig),
            Err(VerifyReject::RevokedKey { key_id: [7u8; 8] })
        );
    }

    #[test]
    fn reregister_clears_revocation_and_overwrites() {
        // 同 id 覆盖语义（既有 BTreeMap::insert 现状锁）。
        let a = kp();
        let b = kp();
        let mut kr = SignerKeyring::new();
        kr.register([7u8; 8], &a.public_key);
        kr.register([7u8; 8], &b.public_key);
        assert_eq!(kr.len(), 1);
        assert_eq!(kr.public_key([7u8; 8]), Some(&b.public_key));
        kr.revoke([7u8; 8]);
        kr.register([7u8; 8], &b.public_key);
        assert!(!kr.is_revoked([7u8; 8]), "覆盖登记重置撤销态");
    }

    #[test]
    fn revision_watermark_only_advances() {
        let mut kr = SignerKeyring::new();
        let source = [7u8; 16];
        assert_eq!(kr.revision(&source), None, "未见面 → None");
        kr.advance_revision(&source, 1).expect("adv1");
        assert_eq!(kr.revision(&source), Some(1));
        assert_eq!(
            kr.advance_revision(&source, 1),
            Err(WatermarkReject::NotAdvancing { seen: 1 }),
            "等值拒"
        );
        assert_eq!(kr.revision(&source), Some(1), "拒绝后水位不降");
        assert_eq!(
            kr.advance_revision(&source, 0),
            Err(WatermarkReject::NotAdvancing { seen: 1 }),
            "回退拒"
        );
        kr.advance_revision(&source, 2).expect("adv2");
        assert_eq!(kr.revision(&source), Some(2));
    }

    #[test]
    fn alias_key_id_is_deterministic_and_disjoint() {
        assert_eq!(alias_key_id(1), alias_key_id(1));
        assert_ne!(alias_key_id(1), alias_key_id(2));
        // 低 4B 恒 0：别名空间与任意 [u8;8] 配置 id 的碰撞面由"单实例
        // 单口径"约束（消费方各自持独立 keyring 实例）。
        assert_eq!(&alias_key_id(0x0102_0304)[4..], &[0u8; 4]);
        let id = alias_key_id(0x0102_0304);
        assert_eq!(&id[..4], &0x0102_0304u32.to_le_bytes());
    }

    #[test]
    fn fingerprint_is_first_8b_of_sm3() {
        let pair = kp();
        let digest = eneros_crypto::sm3_hash(&pair.public_key.to_bytes_uncompressed());
        let mut want = [0u8; 8];
        want.copy_from_slice(&digest[..8]);
        assert_eq!(fingerprint(&pair.public_key), want);
    }

    #[test]
    fn free_verify_signature_types_the_bool_trap() {
        let pair = kp();
        let msg = b"m";
        let good = Sm2Signature::from_bytes(&sign(&pair, msg));
        assert!(verify_signature(msg, &good, &pair.public_key).is_ok());
        let bad = Sm2Signature::from_bytes(&sign(&pair, b"other"));
        assert_eq!(
            verify_signature(msg, &bad, &pair.public_key),
            Err(VerifyReject::BadSignature)
        );
    }

    #[test]
    fn watermark_sources_are_independent() {
        let mut kr = SignerKeyring::new();
        kr.advance_revision(&[1u8; 16], 5).expect("a");
        assert_eq!(kr.revision(&[2u8; 16]), None, "源间隔离");
        kr.advance_revision(&[2u8; 16], 3).expect("b");
        assert_eq!(kr.revision(&[1u8; 16]), Some(5));
    }
}
