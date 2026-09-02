//! 蓝图 61 [07] §3.4/§3.6（v0.314.31）：市场事实接入 adapter。
//!
//! `MarketEventAdapter` 接受**经认证的外部 market envelope**（价格修订/
//! 时段/订单状态），经同一 [`eneros_ingestion_core::IngestionModule`]
//! 合同进入事件平面——低频业务事实与现场时序数据共用一条窄腰，证明
//! seam 覆盖面（此版本只接入事实，不发送报价；报价/ACK 生产 transport
//! 归 v0.314.44）。
//!
//! # 认证链（签名 sandbox 形态；E3）
//!
//! canonical 编码（定长小端字段序，不含签名/摘要自身）→ SM3 摘要比对
//! → SM2 验签（key_id 查注册表；未知 key 拒绝）。通过后 integrity =
//! `Authenticated`。
//!
//! # 事实纪律（[07] §3.4 逐条）
//!
//! - 价格修订 = 新事件（同 external_id、revision 严格递增）——不原地改值；
//! - 迟到/过期/早到/旧修订 → 有界存档（不进事件平面、不可能进入当前
//!   目标——存档区类型上与事件日志分离）；
//! - `f64` price/volume 必须 finite；
//! - 单位语义：price 单位在信封语义层（`YuanPerMwh` 等 market-types）；
//!   事件 `unit_code = 0`（未知——市场事实不得进控制快照，[02] §4.4）。

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_crypto::sm3::hash as sm3_hash;
use eneros_ingestion_core::{IngestOutcome, IngestionModule, RawObservation};
use eneros_signer_keyring::{SignerKeyring, VerifyReject, WatermarkReject};
use eneros_telemetry_model::event::{quality_bits, IntegrityTag, ObservationValue};

// ===========================================================================
// 信封
// ===========================================================================

/// 经认证的市场事实信封（[07] §3.4 最小封闭集）。
#[derive(Debug, Clone, PartialEq)]
pub struct MarketEnvelope {
    /// 市场/运营/产品/区域登记号（0 = 未登记）。
    pub market: u32,
    pub operator: u32,
    pub product: u32,
    pub region: u32,
    /// 交割时段（起点 ms + 时长 ns）。
    pub delivery_start_ms: u64,
    pub duration_ns: u64,
    /// 发布/生效/过期时刻（ms）。
    pub published_at_ms: u64,
    pub effective_at_ms: u64,
    pub expires_at_ms: u64,
    /// 修订号（同 external_id 内严格递增——每次修订新事件）。
    pub revision: u32,
    /// 外部事实 ID（修订链身份）。
    pub external_id: [u8; 16],
    /// 外部序列（观测；去重由 event_id 承担）。
    pub sequence: u64,
    /// 价格/电量（必须 finite；单位在语义层——YuanPerMwh/MWh）。
    pub price: f64,
    pub volume: f64,
    /// 价格单位码（UnitCatalog；0 = 语义层单位——见 crate 文档偏离登记；
    /// canonical 参与签名，杜绝单位中途被换）。
    pub unit_code: u16,
    /// 电量单位码（同上语义）。
    pub volume_unit_code: u16,
    /// 签名者密钥 id + SM2 签名（r||s 64B）。
    pub key_id: [u8; 8],
    pub signature: [u8; 64],
    /// canonical 载荷 SM3 摘要（防 payload/签名错配）。
    pub payload_digest: [u8; 32],
}

impl MarketEnvelope {
    /// canonical 编码（定长小端字段序；签名/摘要字段不参与——验签先
    /// 比对摘要再验签，摘要覆盖除其自身与签名外的全部字段）。
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(128);
        for f in [self.market, self.operator, self.product, self.region] {
            b.extend_from_slice(&f.to_le_bytes());
        }
        for f in [
            self.delivery_start_ms,
            self.duration_ns,
            self.published_at_ms,
            self.effective_at_ms,
            self.expires_at_ms,
        ] {
            b.extend_from_slice(&f.to_le_bytes());
        }
        b.extend_from_slice(&self.revision.to_le_bytes());
        b.extend_from_slice(&self.external_id);
        b.extend_from_slice(&self.sequence.to_le_bytes());
        b.extend_from_slice(&self.price.to_le_bytes());
        b.extend_from_slice(&self.volume.to_le_bytes());
        b.extend_from_slice(&self.unit_code.to_le_bytes());
        b.extend_from_slice(&self.volume_unit_code.to_le_bytes());
        b.extend_from_slice(&self.key_id);
        b
    }

    /// 载荷摘要（canonical → SM3）。
    pub fn compute_digest(&self) -> [u8; 32] {
        sm3_hash(&self.canonical_bytes())
    }
}

// ===========================================================================
// 拒绝与存档
// ===========================================================================

/// 接入拒绝（认证/纪律面；与 IngestReject 分层——本层是 adapter 侧
/// 事前门，事件平面的事后门在 module）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketReject {
    /// 摘要不匹配（载荷被改）。
    DigestMismatch,
    /// 未知签名密钥。
    UnknownKey { key_id: [u8; 8] },
    /// 签名无效。
    BadSignature,
    /// 价格/电量非有限。
    NotFinite,
    /// 同事实旧修订（新事件只认严格递增修订）。
    StaleRevision { revision: u32, seen: u32 },
    /// 早于生效（存档）。
    NotYetEffective { effective_at_ms: u64 },
    /// 已过期（存档）。
    Expired { expires_at_ms: u64 },
    /// 未登记的产品绑定（product/region → 点映射缺失）。
    UnboundProduct { product: u32, region: u32 },
    /// 事件平面结构化拒绝**原样透传**（RateExceeded/Overflow/序列等——
    /// 不伪造原因；统一 reject reason，[07] §3.7）。
    ModuleReject(eneros_ingestion_core::IngestReject),
}

/// 存档原因（迟到/过期/旧修订——不进当前目标的可见性面）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveReason {
    NotYetEffective,
    Expired,
    StaleRevision,
}

/// 有界存档条目。
#[derive(Debug, Clone)]
pub struct ArchiveEntry {
    pub envelope: MarketEnvelope,
    pub reason: ArchiveReason,
}

// ===========================================================================
// MarketEventAdapter
// ===========================================================================

/// 市场事实接入 adapter（push 模式；事件经 module.ingest 进事件平面）。
pub struct MarketEventAdapter {
    source_id: u32,
    /// 签名密钥注册/验签/撤销/修订水位纪律单源（keyring——蓝图 62 卷
    /// 62.3 S5 收敛；key_id → 公钥 + revision 水位表都在 keyring）。
    keys: SignerKeyring,
    /// 产品/区域 → 站内点位绑定（签名配置编译）。
    bindings: BTreeMap<(u32, u32), u32>,
    /// 有界存档（迟到/过期/旧修订；满 64 丢最旧并计数——不静默）。
    archive: Vec<ArchiveEntry>,
    archive_dropped: u64,
    // —— 计数（u64；u8 禁用）——
    pub accepted_total: u64,
    pub rejected_total: u64,
    pub archived_total: u64,
}

const ARCHIVE_CAP: usize = 64;

impl MarketEventAdapter {
    /// 装配（密钥 keyring + 产品绑定）。
    pub fn new(source_id: u32, keys: SignerKeyring, bindings: BTreeMap<(u32, u32), u32>) -> Self {
        Self {
            source_id,
            keys,
            bindings,
            archive: Vec::new(),
            archive_dropped: 0,
            accepted_total: 0,
            rejected_total: 0,
            archived_total: 0,
        }
    }

    /// 接入一封市场事实（[07] §3.4 全纪律单点）。
    pub fn accept(
        &mut self,
        module: &mut IngestionModule,
        env: &MarketEnvelope,
        now_ms: u64,
    ) -> Result<IngestOutcome, MarketReject> {
        // 1. 摘要比对（canonical → SM3；防 payload/签名错配）。
        let digest = env.compute_digest();
        if digest != env.payload_digest {
            self.rejected_total += 1;
            return Err(MarketReject::DigestMismatch);
        }
        // 2. SM2 验签（keyring 类型化纪律单源：未知 key/撤销/验伪统一
        // 结构化拒绝——调用方不触碰底层验签布尔返回）。
        match self
            .keys
            .verify_authenticated(env.key_id, &env.canonical_bytes(), &env.signature)
        {
            Ok(_) => {}
            Err(VerifyReject::UnknownKey { key_id }) => {
                self.rejected_total += 1;
                return Err(MarketReject::UnknownKey { key_id });
            }
            Err(_) => {
                self.rejected_total += 1;
                return Err(MarketReject::BadSignature);
            }
        }
        // 3. 有限性（[07] §3.4：price 必须 finite；volume 同纪律）。
        if !env.price.is_finite() || !env.volume.is_finite() {
            self.rejected_total += 1;
            return Err(MarketReject::NotFinite);
        }
        // 4. 修订水位（同 external_id 严格递增；旧修订 = 存档；水位表
        // 在 keyring 单源）。
        if let Some(seen) = self.keys.revision(&env.external_id) {
            if env.revision <= seen {
                self.archive_entry(env.clone(), ArchiveReason::StaleRevision);
                return Err(MarketReject::StaleRevision {
                    revision: env.revision,
                    seen,
                });
            }
        }
        // 5. 时间窗（迟到/过期/早到 = 存档——不能进入当前目标）。
        if now_ms > env.expires_at_ms {
            self.archive_entry(env.clone(), ArchiveReason::Expired);
            return Err(MarketReject::Expired {
                expires_at_ms: env.expires_at_ms,
            });
        }
        if now_ms < env.effective_at_ms {
            self.archive_entry(env.clone(), ArchiveReason::NotYetEffective);
            return Err(MarketReject::NotYetEffective {
                effective_at_ms: env.effective_at_ms,
            });
        }
        // 6. 产品绑定 → 站内点。
        let Some(&point_id) = self.bindings.get(&(env.product, env.region)) else {
            self.rejected_total += 1;
            return Err(MarketReject::UnboundProduct {
                product: env.product,
                region: env.region,
            });
        };
        // 7. 事件平面准入（身份由 module PointPlan 富化——raw 最小化；
        // 单位透传信封——canonical 已锁，杜绝中途调包）。
        let raw = RawObservation {
            point_id,
            value: ObservationValue::F64(env.price),
            event_time_ns: Some(env.published_at_ms.saturating_mul(1_000_000).max(1)),
            quality_bits: quality_bits::VALID,
            // 签名已验 → Authenticated（[02] 完整性等级 1）。
            integrity: IntegrityTag::Authenticated,
            flags: 0,
            schema_id: 0,
            schema_version: 0,
            unit_code: env.unit_code,
            asset_id: 0,
            mapping_version: 0,
            device_id: 0,
        };
        let outcome = module
            .ingest(self.source_id, raw, now_ms.saturating_mul(1_000_000))
            .map_err(MarketReject::ModuleReject)?;
        // 提交水位（ingest 成功才推进修订水位；步骤 4 已保证严格递增，
        // NotAdvancing 在此不可达——防御臂如实计拒不静默）。
        if let Err(WatermarkReject::NotAdvancing { seen }) =
            self.keys.advance_revision(&env.external_id, env.revision)
        {
            self.rejected_total += 1;
            return Err(MarketReject::StaleRevision {
                revision: env.revision,
                seen,
            });
        }
        self.accepted_total += 1;
        Ok(outcome)
    }

    fn archive_entry(&mut self, envelope: MarketEnvelope, reason: ArchiveReason) {
        self.archived_total += 1;
        if self.archive.len() >= ARCHIVE_CAP {
            self.archive.remove(0);
            self.archive_dropped += 1;
        }
        self.archive.push(ArchiveEntry { envelope, reason });
    }

    /// 存档审计迭代（控制路径零消费——只读审计面）。
    pub fn archive(&self) -> &[ArchiveEntry] {
        &self.archive
    }

    /// 存档因有界而丢弃的条数。
    pub fn archive_dropped(&self) -> u64 {
        self.archive_dropped
    }
}

/// 拒绝的诊断文本（E3 证据行用）。
pub fn reject_reason(r: &MarketReject) -> String {
    format!("{r:?}")
}

// ===========================================================================
// E3：签名 sandbox（测试——真实 SM2 签名 + 修订/过期纪律）
// ===========================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::collections::BTreeMap;

    use eneros_crypto::sm2::keypair::{Sm2KeyPair, Sm2PrivateKey, Sm2PublicKey};
    use eneros_crypto::sm2::sign::Sm2Signer;
    use eneros_ingestion_core::{
        EventQos, InMemorySink, MemEpochStore, OperationalProfile, PointPlan, SourceDescriptor,
    };

    use super::*;

    /// 测试密钥对（固定标量——sandbox 可复现）。
    fn test_keypair() -> (Sm2PrivateKey, Sm2PublicKey) {
        let mut sk_bytes = [0u8; 32];
        sk_bytes[31] = 0x2A;
        let sk = Sm2PrivateKey::from_bytes(&sk_bytes).unwrap();
        let pair = Sm2KeyPair::from_private_key(&sk).unwrap();
        (sk, pair.public_key)
    }

    fn envelope() -> MarketEnvelope {
        MarketEnvelope {
            market: 1,
            operator: 1,
            product: 100,
            region: 1,
            delivery_start_ms: 1_800_000_000_000,
            duration_ns: 900_000_000_000,
            published_at_ms: 1_799_999_000_000,
            effective_at_ms: 1_799_999_000_000,
            expires_at_ms: 1_800_000_000_000,
            revision: 1,
            external_id: [7; 16],
            sequence: 1,
            price: 412.5,
            volume: 10.0,
            unit_code: 0,
            volume_unit_code: 0,
            key_id: *b"testkey1",
            signature: [0; 64],
            payload_digest: [0; 32],
        }
    }

    /// 签名并回填（digest + signature）。
    fn seal(env: &mut MarketEnvelope, sk: &Sm2PrivateKey, pk: &Sm2PublicKey) {
        env.payload_digest = env.compute_digest();
        let signer = Sm2Signer::new();
        let sig = signer
            .sign_deterministic(&env.canonical_bytes(), sk, pk)
            .unwrap();
        env.signature = sig.to_bytes();
    }

    fn module_with_point(point_id: u32) -> IngestionModule {
        let mut m = IngestionModule::new(
            Box::new(InMemorySink::new(256)),
            Box::new(MemEpochStore::default()),
            OperationalProfile::reference(),
        );
        m.register(SourceDescriptor {
            source_id: 5,
            tenant_id: 1,
            site_id: 1,
            allow_control: false,
            require_integrity: true,
            point_range: (0, 0),
            max_events_per_s: 1000,
            allowed_schemas: alloc::vec![],
            clock_id: 3,
            enabled: true,
        });
        m.open(5, 0).unwrap();
        let plan = PointPlan {
            source_id: 5,
            point_id,
            device_id: 0,
            period_ns: 0, // 事件驱动——市场事实不轮询
            phase_offset_ns: 0,
            deadline_ns: 0,
            deadband_abs: 0.0,
            deadband_rel: 0.0,
            qos: EventQos::Q2,
            suppressible: true,
            timeout_ns: 0,
            max_retries: 0,
            schema_id: 0,
            schema_version: 0,
            unit_code: 0,
            asset_id: 0,
            mapping_version: 1,
            scale: 1.0,
            offset: 0.0,
            func_code: 0,
            reg_addr: 0,
            slave_addr: 0,
        };
        m.install_points(1, alloc::vec![plan]).unwrap();
        m
    }

    fn adapter_with(pk: &Sm2PublicKey) -> MarketEventAdapter {
        let mut keys = SignerKeyring::new();
        keys.register(*b"testkey1", pk);
        let mut bindings = BTreeMap::new();
        bindings.insert((100u32, 1u32), 9001u32);
        MarketEventAdapter::new(5, keys, bindings)
    }

    /// E3-①：有效签名 → 事件平面接受。
    #[test]
    fn signed_envelope_accepted() {
        let (sk, pk) = test_keypair();
        let mut env = envelope();
        seal(&mut env, &sk, &pk);
        let mut m = module_with_point(9001);
        let mut a = adapter_with(&pk);
        let out = a.accept(&mut m, &env, 1_799_999_500_000).unwrap();
        assert!(matches!(out, IngestOutcome::Appended(_)));
        assert_eq!(a.accepted_total, 1);
        assert_eq!(m.accepted_total, 1);
    }

    /// E3-②：摘要篡改 → DigestMismatch。
    #[test]
    fn digest_tamper_rejected() {
        let (sk, pk) = test_keypair();
        let mut env = envelope();
        seal(&mut env, &sk, &pk);
        env.price = 999.0; // 改载荷不重签
        let mut m = module_with_point(9001);
        let mut a = adapter_with(&pk);
        assert_eq!(
            a.accept(&mut m, &env, 1_799_999_500_000).unwrap_err(),
            MarketReject::DigestMismatch
        );
    }

    /// E3-③：签名无效（换 key 验）→ BadSignature；未知 key → UnknownKey。
    #[test]
    fn bad_signature_and_unknown_key() {
        let (sk, pk) = test_keypair();
        let mut env = envelope();
        seal(&mut env, &sk, &pk);
        // 未知 key id（digest 保持一致——先过摘要门，命中 key 门）。
        let mut env_unknown = envelope();
        env_unknown.key_id = *b"otherkey";
        env_unknown.payload_digest = env_unknown.compute_digest();
        let mut m = module_with_point(9001);
        let mut a = adapter_with(&pk);
        assert!(matches!(
            a.accept(&mut m, &env_unknown, 1_799_999_500_000)
                .unwrap_err(),
            MarketReject::UnknownKey { .. }
        ));
        // 有效 key 但签名对不上（载荷变过——digest 也重算，签名仍旧）。
        let mut env_resigned = envelope();
        env_resigned.price = 500.0;
        env_resigned.payload_digest = env_resigned.compute_digest();
        assert!(matches!(
            a.accept(&mut m, &env_resigned, 1_799_999_500_000)
                .unwrap_err(),
            MarketReject::BadSignature
        ));
    }

    /// E3-④：修订纪律——同事实 revision 递增 = 新事件；回退 = 存档。
    #[test]
    fn revision_chain() {
        let (sk, pk) = test_keypair();
        let mut m = module_with_point(9001);
        let mut a = adapter_with(&pk);
        let mut rev1 = envelope();
        seal(&mut rev1, &sk, &pk);
        a.accept(&mut m, &rev1, 1_799_999_500_000).unwrap();
        // revision 2（价格修订）→ 新事件。
        let mut rev2 = envelope();
        rev2.revision = 2;
        rev2.price = 430.0;
        seal(&mut rev2, &sk, &pk);
        let out = a.accept(&mut m, &rev2, 1_799_999_600_000).unwrap();
        assert!(matches!(out, IngestOutcome::Appended(_)));
        assert_eq!(m.accepted_total, 2, "修订 = 新事件（不原地改值）");
        // revision 1 重放 → StaleRevision 存档。
        let mut stale = envelope();
        stale.revision = 1;
        stale.sequence = 99;
        seal(&mut stale, &sk, &pk);
        assert!(matches!(
            a.accept(&mut m, &stale, 1_799_999_700_000).unwrap_err(),
            MarketReject::StaleRevision { .. }
        ));
        assert_eq!(a.archived_total, 1);
        assert_eq!(a.archive()[0].reason, ArchiveReason::StaleRevision);
        assert_eq!(m.accepted_total, 2, "存档不进事件平面");
    }

    /// E3-⑤：过期/早到 → 存档（不能进入当前目标）。
    #[test]
    fn expiry_window_archives() {
        let (sk, pk) = test_keypair();
        let mut m = module_with_point(9001);
        let mut a = adapter_with(&pk);
        let mut env = envelope();
        seal(&mut env, &sk, &pk);
        // 过期（now > expires_at）。
        assert!(matches!(
            a.accept(&mut m, &env, 1_800_000_100_000).unwrap_err(),
            MarketReject::Expired { .. }
        ));
        // 早到（now < effective_at）。
        assert!(matches!(
            a.accept(&mut m, &env, 1_799_900_000_000).unwrap_err(),
            MarketReject::NotYetEffective { .. }
        ));
        assert_eq!(a.archived_total, 2);
        assert!(m.accepted_total == 0, "存档零事件平面写入");
        // 窗口内 → 接受。
        a.accept(&mut m, &env, 1_799_999_500_000).unwrap();
        assert_eq!(m.accepted_total, 1);
    }

    /// E3-⑥：非有限价格拒绝。
    #[test]
    fn non_finite_price_rejected() {
        let (sk, pk) = test_keypair();
        let mut m = module_with_point(9001);
        let mut a = adapter_with(&pk);
        let mut env = envelope();
        env.price = f64::NAN;
        seal(&mut env, &sk, &pk);
        assert_eq!(
            a.accept(&mut m, &env, 1_799_999_500_000).unwrap_err(),
            MarketReject::NotFinite
        );
    }

    /// E3-⑦：未绑定产品拒绝。
    #[test]
    fn unbound_product_rejected() {
        let (sk, pk) = test_keypair();
        let mut m = module_with_point(9001);
        let mut a = adapter_with(&pk);
        let mut env = envelope();
        env.product = 404;
        seal(&mut env, &sk, &pk);
        assert!(matches!(
            a.accept(&mut m, &env, 1_799_999_500_000).unwrap_err(),
            MarketReject::UnboundProduct { .. }
        ));
    }
}
