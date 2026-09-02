//! # Phase 7 联邦密钥与证书状态（蓝图 61 §12 §2.4）
//!
//! key rotation/revocation：每个签名 key 有 [`KeyStatus`]，签名接收方必须
//! 先查注册表状态再验签；已撤销 key 的报价/承诺一律结构化拒绝，不影响
//! 其他站点的会话（单 key 毒化不扩散）。轮换 = 签名切换到新 key_id 后
//! 旧 key 置 [`KeyStatus::Retiring`] 至宽限期结束转 Retired；撤销 =
//! [`KeyStatus::Revoked`] 立即生效。全部事件 append-only 审计。
//!
//! `key_digest` 是站点证书/公钥的 SM3 摘要——联邦协议不内联证书，
//! 证书链（X.509）由真实 mTLS transport 层持有（E3 真实 TLS 证据见
//! `tls_transport` + `tests/phase7_tls.rs`）。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_crypto::sm2::Sm2PublicKey;
use eneros_crypto::sm2::Sm2Signature;
use eneros_crypto::sm3::hash as sm3_hash;

pub use crate::offer::{FedEnvelope, FedFrame};

/// key 生命周期状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum KeyStatus {
    /// 当前可用签名/验签 key。
    Active,
    /// 已轮换出：仍可验签（存量消息），宽限期后转 Retired。
    Retiring,
    /// 已退出服务：可验签但标记，不可签发新消息。
    Retired,
    /// 已撤销：立即拒绝一切消息（含验签通过的）。
    Revoked,
}

/// key 记录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyRecord {
    pub status: KeyStatus,
    /// 公钥/证书摘要（SM3）。
    pub key_digest: [u8; 32],
    /// 状态转换时刻（ns；Active 为注册时刻）。
    pub changed_at_ns: u64,
    /// 公钥本体（验签直接使用，避免回调/查找错配）。
    pub public_key: Sm2PublicKey,
}

/// 审计事件（append-only）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyEvent {
    /// 注册新 key 并置 Active。
    Registered {
        key_id: u32,
        digest: [u8; 32],
        at_ns: u64,
    },
    /// 轮换：新 key 就位，旧 key → Retiring。
    Rotated {
        old_key_id: u32,
        new_key_id: u32,
        at_ns: u64,
    },
    /// 宽限期结束：Retiring → Retired。
    Retired { key_id: u32, at_ns: u64 },
    /// 撤销（立即生效）。
    Revoked { key_id: u32, at_ns: u64 },
}

/// key 校验错误（fail-closed）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyError {
    UnknownKey,
    RevokedKey,
    RetiredKey,
    EmptyRegistry,
    BadSignature,
    KeyIdCollision,
}

/// 站点密钥注册表（bounded：key_id ≤ [`MAX_KEYS`]）。
#[derive(Clone, Debug)]
pub struct KeyRegistry {
    keys: BTreeMap<u32, KeyRecord>,
    /// key_id → owner（站点 id，协调器=0）。P0 身份绑定：验签只证明
    /// 「key 真实」，身份归属必须由注册表强制（防站点冒充）。
    owners: BTreeMap<u32, u32>,
    log: Vec<KeyEvent>,
    next_key_id: u32,
}

/// 单端点最大活跃 key 数（协调器需容纳全部站点 + 轮换宽限；C3 100 站 ×10 余量）。
pub const MAX_KEYS: u32 = 2048;

impl Default for KeyRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyRegistry {
    pub fn new() -> Self {
        Self {
            keys: BTreeMap::new(),
            owners: BTreeMap::new(),
            log: Vec::new(),
            next_key_id: 1,
        }
    }

    /// 注册公钥为 Active（key_id 由注册表分配，从 1 起单调；测试/单边场景）。
    pub fn register(&mut self, pk: &Sm2PublicKey, at_ns: u64) -> Result<u32, KeyError> {
        let key_id = self.next_key_id;
        self.register_with_id(key_id, pk, at_ns)?;
        self.next_key_id += 1;
        Ok(key_id)
    }

    /// 以显式 key_id 注册（owner=0，即协调器自己的 key；站点 key 请用
    /// [`Self::register_with_owner`]）。
    pub fn register_with_id(
        &mut self,
        key_id: u32,
        pk: &Sm2PublicKey,
        at_ns: u64,
    ) -> Result<(), KeyError> {
        self.register_with_owner(key_id, 0, pk, at_ns)
    }

    /// 以显式 key_id + owner（站点 id）注册。验签通过后上层必须比对
    /// `owner_of(signer_key_id) == env.site_id`，否则视为冒充。
    pub fn register_with_owner(
        &mut self,
        key_id: u32,
        owner: u32,
        pk: &Sm2PublicKey,
        at_ns: u64,
    ) -> Result<(), KeyError> {
        if key_id == 0 {
            return Err(KeyError::UnknownKey);
        }
        if self.keys.contains_key(&key_id) {
            return Err(KeyError::KeyIdCollision);
        }
        if self.keys.len() as u32 >= MAX_KEYS {
            return Err(KeyError::KeyIdCollision);
        }
        let digest = sm3_hash(&pk.to_bytes_uncompressed());
        self.keys.insert(
            key_id,
            KeyRecord {
                status: KeyStatus::Active,
                key_digest: digest,
                changed_at_ns: at_ns,
                public_key: *pk,
            },
        );
        self.owners.insert(key_id, owner);
        self.log.push(KeyEvent::Registered {
            key_id,
            digest,
            at_ns,
        });
        Ok(())
    }

    /// key 的持有者（站点 id；协调器 key = 0）。
    pub fn owner_of(&self, key_id: u32) -> Option<u32> {
        self.owners.get(&key_id).copied()
    }

    /// 轮换：新 key 就位为 Active，旧 key → Retiring（宽限期由调用方
    /// 在 [`KeyRegistry::retire_expired`] 依 `grace_ns` 推进）。
    pub fn rotate(
        &mut self,
        old_key_id: u32,
        new_pk: &Sm2PublicKey,
        at_ns: u64,
    ) -> Result<u32, KeyError> {
        let old = self.keys.get(&old_key_id).ok_or(KeyError::UnknownKey)?;
        if old.status == KeyStatus::Revoked {
            return Err(KeyError::RevokedKey);
        }
        let owner = self.owners.get(&old_key_id).copied().unwrap_or(0);
        let new_id = self.register(new_pk, at_ns)?;
        self.owners.insert(new_id, owner);
        if let Some(r) = self.keys.get_mut(&old_key_id) {
            r.status = KeyStatus::Retiring;
            r.changed_at_ns = at_ns;
        }
        self.log.push(KeyEvent::Rotated {
            old_key_id,
            new_key_id: new_id,
            at_ns,
        });
        Ok(new_id)
    }

    /// 撤销：立即拒绝该 key 的一切新消息（存量 commitment 不受影响——
    /// 已签承诺在 expires 内仍由双方各自账本承担）。
    pub fn revoke(&mut self, key_id: u32, at_ns: u64) -> Result<(), KeyError> {
        let r = self.keys.get_mut(&key_id).ok_or(KeyError::UnknownKey)?;
        r.status = KeyStatus::Revoked;
        r.changed_at_ns = at_ns;
        self.log.push(KeyEvent::Revoked { key_id, at_ns });
        Ok(())
    }

    /// 宽限期推进：Retiring 且 `now - changed_at > grace_ns` → Retired；
    /// Retired 且宽限期超过 `revoke_after_ns` 的可在 `purge_retired` 清理。
    pub fn retire_expired(&mut self, now_ns: u64, grace_ns: u64) {
        let mut to_retire: Vec<u32> = Vec::new();
        for (id, r) in self.keys.iter() {
            if r.status == KeyStatus::Retiring && now_ns.saturating_sub(r.changed_at_ns) > grace_ns
            {
                to_retire.push(*id);
            }
        }
        for id in to_retire {
            if let Some(r) = self.keys.get_mut(&id) {
                r.status = KeyStatus::Retired;
                r.changed_at_ns = now_ns;
            }
            self.log.push(KeyEvent::Retired {
                key_id: id,
                at_ns: now_ns,
            });
        }
    }

    /// 删除 Retired 记录（可观测清理，释放配额）。
    pub fn purge_retired(&mut self) -> usize {
        let dead: Vec<u32> = self
            .keys
            .iter()
            .filter(|(_, r)| r.status == KeyStatus::Retired)
            .map(|(id, _)| *id)
            .collect();
        let n = dead.len();
        for id in dead {
            self.keys.remove(&id);
            self.owners.remove(&id);
        }
        n
    }

    pub fn status(&self, key_id: u32) -> Option<KeyStatus> {
        self.keys.get(&key_id).map(|r| r.status)
    }

    pub fn key_digest(&self, key_id: u32) -> Option<[u8; 32]> {
        self.keys.get(&key_id).map(|r| r.key_digest)
    }

    /// 审计事件（append-only，只读）。
    pub fn audit_log(&self) -> &[KeyEvent] {
        &self.log
    }

    /// 校验并验签信封：key 必须存在且非 Revoked/Retired，签名必须通过
    /// （keyring 类型化面——底层验签布尔双通道不在本 crate 出现）。
    pub fn verify_envelope(&self, env: &FedEnvelope, now_ns: u64) -> Result<(), KeyError> {
        let _ = now_ns;
        let rec = self
            .keys
            .get(&env.signer_key_id)
            .ok_or(KeyError::UnknownKey)?;
        match rec.status {
            KeyStatus::Revoked => return Err(KeyError::RevokedKey),
            KeyStatus::Retired => return Err(KeyError::RetiredKey),
            _ => {}
        }
        // 注册表内公钥与摘要绑定一致（防 key_id 张冠李戴）。
        if sm3_hash(&rec.public_key.to_bytes_uncompressed()) != rec.key_digest {
            return Err(KeyError::BadSignature);
        }
        // 验签管线单源（keyring 类型化——布尔返回的 Ok(false)/Err 双通道
        // 在 keyring 收口，本 crate 不触碰；蓝图 62 卷 62.3 S5）。
        let canon = FedFrame::canonical(env).map_err(|_| KeyError::BadSignature)?;
        let sig = Sm2Signature::from_bytes(&env.signature);
        eneros_signer_keyring::verify_signature(&canon, &sig, &rec.public_key)
            .map_err(|_| KeyError::BadSignature)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]
    use eneros_crypto::rng::CsRng;
    use eneros_crypto::sm2::Sm2KeyPair;

    use super::*;
    use crate::offer::*;

    fn kp() -> (Sm2KeyPair, CsRng) {
        let mut rng = CsRng::new();
        (Sm2KeyPair::generate(&mut rng).expect("kp"), rng)
    }

    fn offer_body() -> FedBody {
        FedBody::Offer(CapabilityOffer {
            offer_id: [1u8; 16],
            interval_start_ns: 0,
            interval_end_ns: 3_600_000_000_000,
            p_min_kw: 0.0,
            p_max_kw: 100.0,
            energy_min_kwh: 0.0,
            energy_max_kwh: 100.0,
            ramp_kw_s: 10.0,
            reserve_up_kw: 5.0,
            reserve_down_kw: 5.0,
            price_curve_ref: BlobRefV1::new(1, b"price-curve"),
            capability_root: [7u8; 32],
            valid_until_ns: 9_999,
            revision: 1,
        })
    }

    fn hdr(key_id: u32) -> EnvelopeHdr {
        EnvelopeHdr {
            epoch: 1,
            sequence: 10,
            tenant_id: 1,
            site_id: 2,
            created_ns: 1_000,
            deadline_ns: 100_000,
            signer_key_id: key_id,
        }
    }

    #[test]
    fn register_rotate_revoke_lifecycle() {
        let mut reg = KeyRegistry::new();
        let (k1, _) = kp();
        let (k2, _) = kp();
        let id1 = reg.register(&k1.public_key, 100).expect("reg");
        assert_eq!(reg.status(id1), Some(KeyStatus::Active));
        let id2 = reg.rotate(id1, &k2.public_key, 200).expect("rot");
        assert_eq!(reg.status(id1), Some(KeyStatus::Retiring));
        assert_eq!(reg.status(id2), Some(KeyStatus::Active));
        reg.retire_expired(200 + 10_000 + 1, 10_000);
        assert_eq!(reg.status(id1), Some(KeyStatus::Retired));
        reg.revoke(id2, 300).expect("rev");
        assert_eq!(reg.status(id2), Some(KeyStatus::Revoked));
        assert_eq!(reg.audit_log().len(), 5);
        assert_eq!(
            reg.key_digest(id1),
            Some(sm3_hash(&k1.public_key.to_bytes_uncompressed()))
        );
        // Retired 可清理
        assert_eq!(reg.purge_retired(), 1);
        assert_eq!(reg.status(id1), None);
    }

    #[test]
    fn verify_accepts_active_rejects_revoked() {
        let mut reg = KeyRegistry::new();
        let (k1, mut rng) = kp();
        let pairs = [(1u32, k1)];
        let id1 = reg.register(&pairs[0].1.public_key, 100).expect("reg");
        let env = sign_envelope(&offer_body(), &hdr(id1), &pairs[0].1, &mut rng).expect("s");
        assert_eq!(reg.verify_envelope(&env, 1_000), Ok(()));
        // 撤销后拒收
        reg.revoke(id1, 200).expect("rev");
        assert_eq!(reg.verify_envelope(&env, 1_000), Err(KeyError::RevokedKey));
        // 未知 key 拒收
        let env2 = sign_envelope(&offer_body(), &hdr(99), &pairs[0].1, &mut rng).expect("s");
        assert_eq!(reg.verify_envelope(&env2, 1_000), Err(KeyError::UnknownKey));
    }

    #[test]
    fn retired_rejected_new_messages() {
        let mut reg = KeyRegistry::new();
        let (k1, mut rng) = kp();
        let (k2, _) = kp();
        let id1 = reg.register(&k1.public_key, 100).expect("r1");
        let id2 = reg.rotate(id1, &k2.public_key, 200).expect("r2");
        reg.retire_expired(200 + 10_000 + 1, 10_000);
        assert_eq!(reg.status(id1), Some(KeyStatus::Retired));
        let env = sign_envelope(&offer_body(), &hdr(id1), &k1, &mut rng).expect("s");
        let _ = (id2, k2);
        assert_eq!(reg.verify_envelope(&env, 1_000), Err(KeyError::RetiredKey));
    }
}
