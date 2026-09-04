//! # SignerKeyring（蓝图 62 卷 62.3 WP-62.3.1 · S5 收敛）
//!
//! "接入一封认证外部事实"的密钥注册/验签/撤销/修订水位纪律**唯一实现**。
//! 消费方（market_ingest / external_facts / model-harness skill+registry /
//! federation KeyRegistry / model-security ModelSignatureVerifier——蓝图 64
//! 卷 64.3 WP-64.3.5 adapter 化 / ota-ab + recovery flash + attestation
//! `verify_signature` 类型化原语）保留各自 payload schema 与业务闸，四张
//! 裸 key 表与五条 register/verify 管线收敛于此。域守卫
//! （`primitive_domain_guard`，蓝图 65 WP-65.3 加宽）锁**全仓非原始域**
//! 裸 sm2_verify 只存在于原始域 crate + secure-boot（ADR-033 豁免）+
//! EXEMPT_FILES 逐行豁免（REG-SM2 台账）。
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
//! - 修订水位（external_id → 已见最高 revision）只升不降，判据单点在
//!   [`SignerKeyring::check_revision`]（判定）/ [`SignerKeyring::
//!   commit_revision`]（推进）；判拒语义（存档/拒绝计数）
//!   保留在消费方。
//! - **撤销终态 = tombstone 集**（蓝图 65 WP-65.1，本卷唯一授权的深模块
//!   语义变更）：`revoke` 对任意 id 一律记账（含未登记 id——"吊销优先于
//!   可信"）；`register` 永不触碰 tombstone（旧「同 id 覆盖即洗白撤销」
//!   语义废除——全仓无消费方要该语义，且是唯一已发货 fail-open）；
//!   `verify_authenticated` 撤销判定先于查表；显式洗白仅 [`SignerKeyring::
//!   reinstate`] 一口（当前零生产调用方）。
//!
//! no_std + alloc（消费方为 no_std crate；不引入 std）。

#![cfg_attr(not(test), no_std)]
extern crate alloc;

use alloc::collections::{BTreeMap, BTreeSet};

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

/// 修订判定拒绝（[`SignerKeyring::check_revision`] 专用——与推进拒绝
/// [`WatermarkReject::NotAdvancing`] 分名：判定非变异，语义是"陈旧"
/// 不是"推进失败"；63.3 WP-63.3.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevisionStale {
    /// 已见最高 revision（消费方错误载荷透传）。
    pub seen: u32,
}

/// 登记条目（撤销表示不在 entry——tombstone 集单一持有，见
/// [`SignerKeyring`] 文档）。
#[derive(Debug, Clone)]
struct KeyEntry {
    public_key: Sm2PublicKey,
}

/// 签名密钥注册表（S5 收敛点：注册/验签/撤销/修订水位唯一实现）。
///
/// 撤销终态单源 = `revoked` tombstone 集：登记表与吊销账分离，register
/// 只写登记表（不洗白），revoke 只写 tombstone（无登记前置）。
#[derive(Debug, Clone, Default)]
pub struct SignerKeyring {
    keys: BTreeMap<KeyId, KeyEntry>,
    /// 吊销账（tombstone；含未登记 id）。
    revoked: BTreeSet<KeyId>,
    revisions: BTreeMap<[u8; 16], u32>,
}

impl SignerKeyring {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记（同 id 覆盖公钥字节——与既有消费方 `BTreeMap::insert` 语义
    /// 一致；消费方审计表见执行卡：model-harness ×2、business-services
    /// ×4，全为 register-once）。
    ///
    /// **永不触碰 tombstone**（蓝图 65 R-65.a）：已吊销 id 重登后仍
    /// `RevokedKey`——洗白仅 [`Self::reinstate`] 一口。
    pub fn register(&mut self, key_id: KeyId, pk: &Sm2PublicKey) {
        self.keys.insert(key_id, KeyEntry { public_key: *pk });
    }

    /// 撤销（立即拒验）。任意 id 一律记账 tombstone（含未登记 id——
    /// "吊销优先于可信"），幂等；返回 `()`（撤销是无条件记账，无
    /// "未登记则失败"分支可报——返回值全仓零消费方，rg 审计见执行卡）。
    pub fn revoke(&mut self, key_id: KeyId) {
        self.revoked.insert(key_id);
    }

    /// 显式洗白（移除 tombstone；返回是否本有 tombstone 可移）。当前
    /// 零生产调用方——撤销终态的唯一逆转口，留给未来真实消费方。
    pub fn reinstate(&mut self, key_id: KeyId) -> bool {
        self.revoked.remove(&key_id)
    }

    /// 撤销态读面（已吊销即真，无论是否登记——model-security 三门所需）。
    pub fn is_revoked(&self, key_id: KeyId) -> bool {
        self.revoked.contains(&key_id)
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

    /// 类型化验签（唯一验签纪律：**撤销门先于查表** → 查表 → SM2；产出
    /// 证明 token 或结构化拒绝。撤销先查保证"未登记即吊销"判
    /// `RevokedKey` 不降级 `UnknownKey`——SIG 既有语义逐 case 不变，
    /// R-65.a 对照表见执行卡）。
    pub fn verify_authenticated(
        &self,
        key_id: KeyId,
        msg: &[u8],
        sig: &[u8; 64],
    ) -> Result<VerifiedSignature, VerifyReject> {
        if self.revoked.contains(&key_id) {
            return Err(VerifyReject::RevokedKey { key_id });
        }
        let entry = self
            .keys
            .get(&key_id)
            .ok_or(VerifyReject::UnknownKey { key_id })?;
        verify_signature(msg, &Sm2Signature::from_bytes(sig), &entry.public_key)
    }

    /// 修订水位读面（source → 已见最高 revision）。
    pub fn revision(&self, source: &[u8; 16]) -> Option<u32> {
        self.revisions.get(source).copied()
    }

    /// 修订判定（只升不降判据唯一实现；**非变异**——消费方在自身判拒
    /// 语义放行后仍须 [`Self::commit_revision`] 才落表）。
    ///
    /// 与推进的分离（63.3 WP-63.3.1）：判定/推进共用同一私有比较
    /// [`Self::revision_stale_seen`]——消费方的差异化语义（market
    /// archive vs fact reject）保留在消费方，判据算式不再两抄。
    pub fn check_revision(&self, source: &[u8; 16], revision: u32) -> Result<(), RevisionStale> {
        match self.revision_stale_seen(source, revision) {
            Some(seen) => Err(RevisionStale { seen }),
            None => Ok(()),
        }
    }

    /// 修订推进（前置 = [`Self::check_revision`] 已过；内部走同一私有
    /// 比较——判据物理一份）。`NotAdvancing` 防御臂保留在判据 owner
    /// （消费方不再手写比较，成功路径此臂不可达——check 与 commit 之间
    /// 无第二写入者，单线程流）。
    pub fn commit_revision(
        &mut self,
        source: &[u8; 16],
        revision: u32,
    ) -> Result<(), WatermarkReject> {
        match self.revision_stale_seen(source, revision) {
            Some(seen) => Err(WatermarkReject::NotAdvancing { seen }),
            None => {
                self.revisions.insert(*source, revision);
                Ok(())
            }
        }
    }

    /// 判据唯一私有实现（`revision <= seen`——全仓唯一一处）。
    fn revision_stale_seen(&self, source: &[u8; 16], revision: u32) -> Option<u32> {
        match self.revisions.get(source) {
            Some(&seen) if revision <= seen => Some(seen),
            _ => None,
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

/// WP-64.3.5 + 蓝图 65 WP-65.3 rg 守卫：全仓裸 `sm2_verify(` 调用只允许——
/// 原始域 crate（crypto/crypto-gmssl 实现、signer-keyring 自身纪律
/// owner）+ secure-boot（ADR-033 登记豁免 REG-SM2-SB）+ `EXEMPT_FILES`
/// 逐行豁免（REG-SM2 台账：拒绝词形/错误传输面与类型化原语不相容的
/// 站点）。其余消费面必须走 [`verify_signature`] 类型化原语——布尔
/// 陷阱**全仓非原始域**清零（原「security 域内」口径加宽；豁免清单
/// = EXEMPT_CRATES ∪ EXEMPT_FILES，逐行登记于 REG-SM2）。
#[cfg(test)]
mod primitive_domain_guard {
    #[test]
    fn sm2_verify_restricted_to_primitive_domain() {
        use alloc::string::{String, ToString};
        use alloc::vec::Vec;
        use std::path::Path;

        const EXEMPT_CRATES: [&str; 4] = [
            "/security/crypto/",
            "/security/crypto-gmssl/",
            "/security/signer-keyring/",
            // ADR-033：secure-boot 复用 eneros-crypto 原语（登记豁免）。
            "/security/secure-boot/",
        ];

        /// 逐行豁免（REG-SM2 台账，docs/candidates-registry.md F 节）：
        /// 1) business-services config_service.rs——Ok(false)/Err 双通道已
        ///    分词形（"bad-entry-signature" vs "sm2-verify-failed"）；类型化
        ///    原语统一为 BadSignature 即改词形（禁止项：拒绝词形逐字节
        ///    不变）。
        /// 2) agent process.rs——Err 通道错误传输面 "SM2 verify failed:
        ///    {e:?}"（内层 e 文案透传）；类型化原语统一即丢弃内层错误
        ///    文案。
        const EXEMPT_FILES: [&str; 2] = [
            "/business-services/src/config_service.rs",
            "/agent/src/process.rs",
        ];

        fn walk(dir: &Path, hits: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).expect("workspace root/") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    // 蓝图 66 WP-66.4.3：walk 根加宽至全仓——产物/版本
                    // 控制目录非源码面，排除。
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if name == "target" || name == ".git" {
                        continue;
                    }
                    walk(&path, hits);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let as_str = path.to_string_lossy().to_string();
                    if EXEMPT_CRATES.iter().any(|w| as_str.contains(w))
                        || EXEMPT_FILES.iter().any(|w| as_str.ends_with(w))
                    {
                        continue;
                    }
                    let src = std::fs::read_to_string(&path).expect("read src");
                    for (i, line) in src.lines().enumerate() {
                        // 注释行不匹配（R-65.d 剥注释——3 行注释锚点不误报）。
                        if line.trim_start().starts_with("//") {
                            continue;
                        }
                        if line.contains("sm2_verify(") {
                            hits.push(alloc::format!("{as_str}:{}: {line}", i + 1));
                        }
                    }
                }
            }
        }

        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("workspace root")
            .to_path_buf();
        let mut hits = Vec::new();
        // 蓝图 66 WP-66.4.3：walk 根 = workspace 全仓（原 crates/ 口径
        // 加宽；xtask/ci 等域外 .rs 站点同受守卫——「布尔陷阱域外清零」
        // 措辞自此与 walk 范围一致）。
        walk(&workspace, &mut hits);
        assert!(
            hits.is_empty(),
            "全仓非原始域 sm2_verify 残存（须改 signer_keyring::verify_signature；豁免 = EXEMPT_CRATES ∪ EXEMPT_FILES，逐行登记于 REG-SM2）:\n{}",
            hits.join("\n")
        );
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
        kr.revoke([7u8; 8]);
        assert_eq!(
            kr.verify_authenticated([7u8; 8], msg, &sig),
            Err(VerifyReject::RevokedKey { key_id: [7u8; 8] })
        );
        // 幂等 revoke：重复记账无差别，仍终态。
        kr.revoke([7u8; 8]);
        assert_eq!(
            kr.verify_authenticated([7u8; 8], msg, &sig),
            Err(VerifyReject::RevokedKey { key_id: [7u8; 8] })
        );
    }

    /// 撤销终态红测（蓝图 65 WP-65.1，R-65.a；替换旧
    /// `reregister_clears_revocation_and_overwrites`——旧名锁的是被废
    /// 「覆盖即洗白」语义）：revoke → re-register → verify 仍
    /// `RevokedKey`。旧实现（entry.revoked 位 + register 覆盖重置）下
    /// 本测红（is_revoked=false / verify=Ok）；tombstone 化后转绿。
    #[test]
    fn register_after_revoke_keeps_revocation() {
        let a = kp();
        let b = kp();
        let msg = b"m";
        let sig = sign(&b, msg);
        let mut kr = SignerKeyring::new();
        kr.register([7u8; 8], &a.public_key);
        kr.revoke([7u8; 8]);
        kr.register([7u8; 8], &b.public_key);
        assert_eq!(kr.len(), 1);
        assert_eq!(
            kr.public_key([7u8; 8]),
            Some(&b.public_key),
            "覆盖公钥字节语义保持"
        );
        assert!(kr.is_revoked([7u8; 8]), "重登不洗白（撤销终态）");
        assert_eq!(
            kr.verify_authenticated([7u8; 8], msg, &sig),
            Err(VerifyReject::RevokedKey { key_id: [7u8; 8] })
        );
    }

    /// 未登记 id 吊销 = 记账 tombstone（"吊销优先于可信"）：verify 恒
    /// `RevokedKey`，不降级 `UnknownKey`。旧实现 revoke(未知)→false 且
    /// 不记账，本测红（is_revoked=false / verify=UnknownKey）。
    #[test]
    fn unregistered_revoke_rejects_verification() {
        let pair = kp();
        let msg = b"m";
        let sig = sign(&pair, msg);
        let mut kr = SignerKeyring::new();
        assert!(!kr.is_revoked([8u8; 8]), "未吊销未登记 → false");
        kr.revoke([8u8; 8]);
        assert!(kr.is_revoked([8u8; 8]), "已吊销即真（无论是否登记）");
        assert_eq!(
            kr.verify_authenticated([8u8; 8], msg, &sig),
            Err(VerifyReject::RevokedKey { key_id: [8u8; 8] })
        );
    }

    /// 覆盖公钥字节正例（自旧 `reregister_clears_revocation_and_overwrites`
    /// 拆出——与撤销终态分立，register 的覆盖语义只指 entry）。
    #[test]
    fn register_overwrites_public_key_bytes() {
        let a = kp();
        let b = kp();
        let mut kr = SignerKeyring::new();
        kr.register([7u8; 8], &a.public_key);
        kr.register([7u8; 8], &b.public_key);
        assert_eq!(kr.len(), 1);
        assert_eq!(kr.public_key([7u8; 8]), Some(&b.public_key));
        assert!(!kr.is_revoked([7u8; 8]));
    }

    /// 显式洗白唯一口（蓝图 65 WP-65.1）：reinstate 移除 tombstone 后可
    /// 验签；无 tombstone 时返回 false。当前零生产调用方（留给未来真实
    /// 消费方）。
    #[test]
    fn reinstate_after_revoke_allows_verification() {
        let pair = kp();
        let msg = b"m";
        let sig = sign(&pair, msg);
        let mut kr = SignerKeyring::new();
        kr.register([7u8; 8], &pair.public_key);
        kr.revoke([7u8; 8]);
        assert_eq!(
            kr.verify_authenticated([7u8; 8], msg, &sig),
            Err(VerifyReject::RevokedKey { key_id: [7u8; 8] })
        );
        assert!(!kr.reinstate([9u8; 8]), "无 tombstone 可移 → false");
        assert!(kr.reinstate([7u8; 8]), "有 tombstone → true");
        assert!(!kr.is_revoked([7u8; 8]));
        assert!(
            kr.verify_authenticated([7u8; 8], msg, &sig).is_ok(),
            "reinstate 后可验签"
        );
    }

    #[test]
    fn revision_watermark_only_advances() {
        let mut kr = SignerKeyring::new();
        let source = [7u8; 16];
        assert_eq!(kr.revision(&source), None, "未见面 → None");
        kr.commit_revision(&source, 1).expect("adv1");
        assert_eq!(kr.revision(&source), Some(1));
        assert_eq!(
            kr.commit_revision(&source, 1),
            Err(WatermarkReject::NotAdvancing { seen: 1 }),
            "等值拒"
        );
        assert_eq!(kr.revision(&source), Some(1), "拒绝后水位不降");
        assert_eq!(
            kr.commit_revision(&source, 0),
            Err(WatermarkReject::NotAdvancing { seen: 1 }),
            "回退拒"
        );
        kr.commit_revision(&source, 2).expect("adv2");
        assert_eq!(kr.revision(&source), Some(2));
    }

    /// check_revision 定义锁（63.3 WP-63.3.1 红测）：判定与推进判据同源
    /// ——等值拒绝/回退拒绝/首见放行/跨源独立/非变异（check 不落表）。
    #[test]
    fn check_revision_definition_lock() {
        let kr = SignerKeyring::new();
        let source = [9u8; 16];
        // 首见放行（未登记源）。
        assert_eq!(kr.check_revision(&source, 1), Ok(()), "首见放行");
        assert_eq!(kr.revision(&source), None, "check 非变异——不落表");
        // 回退拒绝：check 报告 seen（变异验证锚——改成 `<` 时本臂转 Ok，
        // market/facts 既有 StaleRevision 测试必红）。
        assert_eq!(
            kr.check_revision(&source, 5),
            Ok(()),
            "水位仍空 → 任何首见 revision 放行"
        );
        let mut kr = SignerKeyring::new();
        kr.commit_revision(&source, 5).expect("seed");
        assert_eq!(
            kr.check_revision(&source, 5),
            Err(RevisionStale { seen: 5 }),
            "等值拒绝"
        );
        assert_eq!(
            kr.check_revision(&source, 4),
            Err(RevisionStale { seen: 5 }),
            "回退拒绝（载荷 = 已见水位）"
        );
        assert_eq!(kr.check_revision(&source, 6), Ok(()), "递增放行");
        // 跨源独立（判据按 source 隔离）。
        let other = [10u8; 16];
        assert_eq!(kr.check_revision(&other, 0), Ok(()), "跨源独立");
        // 判据同源证明：commit 对同一输入的拒绝面与 check 一致。
        assert_eq!(
            kr.commit_revision(&source, 5),
            Err(WatermarkReject::NotAdvancing { seen: 5 }),
            "check 拒 ⇒ commit 拒（同一私有比较）"
        );
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
        kr.commit_revision(&[1u8; 16], 5).expect("a");
        assert_eq!(kr.revision(&[2u8; 16]), None, "源间隔离");
        kr.commit_revision(&[2u8; 16], 3).expect("b");
        assert_eq!(kr.revision(&[1u8; 16]), Some(5));
    }
}
