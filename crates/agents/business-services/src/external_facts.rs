//! 蓝图 61 Phase 6 §5.3/§5.4/§5.5/§5.8：工艺/企业/市场（补充）外部事实
//! 接入层。
//!
//! 唯一不变量（[11] §5.1）：外部市场、工艺和企业系统只能发布**认证、
//! 版本化、有效期明确**的事实/目标草案；它们不能构造命令或旁路站内
//! 安全。三类事实都能追到外部 revision 和签名（[11] §5.9 退出 gate）。
//!
//! 本模块与 [`crate::market_ingest::MarketEventAdapter`] 同构但面向
//! **约束/目标事实**（工艺边界、维护窗、企业计划/合同/碳预算/班次）：
//! 不产生控制快照观测（unit_code=0 纪律——事实不进控制快照），经
//! [`FactEventSink`] 进入事件平面（target 装配接 StoreSink/event-store）。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_crypto::sm2::keypair::Sm2PublicKey;
use eneros_signer_keyring::{SignerKeyring, VerifyReject, WatermarkReject};

/// 事实分类码（信封 class 域；1=市场 2=工艺 3=企业——与
/// [`FactPayload::class`] 一一对应）。
pub mod class {
    pub const MARKET: u8 = 1;
    pub const PROCESS: u8 = 2;
    pub const ENTERPRISE: u8 = 3;
}

/// 企业事实种类（[11] §5.4）。
pub mod enterprise_kind {
    pub const PRODUCTION_PLAN: u16 = 1;
    pub const MAINTENANCE_ORDER: u16 = 2;
    pub const ENERGY_CONTRACT: u16 = 3;
    pub const CARBON_BUDGET: u16 = 4;
    pub const SHIFT_APPROVAL: u16 = 5;
}

/// 工艺阶段模式（[11] §5.3：startup/shutdown/run/cleaning）。
pub mod process_mode {
    pub const STARTUP: u8 = 1;
    pub const RUN: u8 = 2;
    pub const SHUTDOWN: u8 = 3;
    pub const CLEANING: u8 = 4;
}

/// 事实头（外部身份 + 时效 + 认证材料；[11] §5.1）。
#[derive(Debug, Clone, PartialEq)]
pub struct FactHeader {
    /// 外部事实 ID（修订链身份）。
    pub external_id: [u8; 16],
    /// 修订号（同 external_id 严格递增——每次修订新事实）。
    pub revision: u32,
    /// 外部系统注册号（SOURCE 注册表）。
    pub source_id: u32,
    /// 数据分类（telemetry Classification 码；Restricted 字段不上模型/云）。
    pub classification: u8,
    pub published_at_ms: u64,
    pub effective_from_ms: u64,
    pub expires_at_ms: u64,
}

impl FactHeader {
    /// 基本纪律：修订 ≥ 1、窗口合法。
    pub fn validate(&self) -> Result<(), FactReject> {
        if self.revision == 0 {
            return Err(FactReject::BadRevision);
        }
        if self.external_id == [0; 16] {
            return Err(FactReject::ZeroExternalId);
        }
        if self.effective_from_ms >= self.expires_at_ms {
            return Err(FactReject::BadWindow);
        }
        if self.classification > 3 {
            return Err(FactReject::BadClassification);
        }
        Ok(())
    }
}

/// 事实载荷 trait（canonical 与语义校验由各事实类实现）。
pub trait FactPayload: Clone + core::fmt::Debug + PartialEq {
    fn class(&self) -> u8;
    /// 语义字段 canonical（定长小端；不参与签名/摘要的派生字段除外）。
    fn payload_canonical(&self) -> Vec<u8>;
    /// 语义校验（finite/范围/枚举/窗口——[11] §5.3/§5.8）。
    fn validate_semantics(&self) -> Result<(), FactReject>;
}

// ---------------------------------------------------------------------------
// 三类事实
// ---------------------------------------------------------------------------

/// 市场事实（[11] §5.2 inbound：价格/时段/出清/调度指令/ACK/取消/结算
/// 修订的约束面——本卷价格/时段/电量）。
#[derive(Debug, Clone, PartialEq)]
pub struct MarketFact {
    pub product: u32,
    pub region: u32,
    /// 交割时段（起点 ms + 时长 ns）。
    pub delivery_start_ms: u64,
    pub duration_ns: u64,
    /// 价格（YuanPerMwh 语义；必须 finite）。
    pub price: f64,
    /// 电量（MWh 语义；必须 finite）。
    pub volume: f64,
    /// ISO 4217 币种码（0 = 未注明 → 拒；[11] §5.2 币种明确）。
    pub currency_code: u16,
    /// 时区偏移（分钟；[11] §5.2 时区明确）。
    pub tz_offset_min: i16,
}

impl FactPayload for MarketFact {
    fn class(&self) -> u8 {
        class::MARKET
    }

    fn payload_canonical(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(56);
        b.extend_from_slice(&self.product.to_le_bytes());
        b.extend_from_slice(&self.region.to_le_bytes());
        b.extend_from_slice(&self.delivery_start_ms.to_le_bytes());
        b.extend_from_slice(&self.duration_ns.to_le_bytes());
        b.extend_from_slice(&self.price.to_le_bytes());
        b.extend_from_slice(&self.volume.to_le_bytes());
        b.extend_from_slice(&self.currency_code.to_le_bytes());
        b.extend_from_slice(&self.tz_offset_min.to_le_bytes());
        b
    }

    fn validate_semantics(&self) -> Result<(), FactReject> {
        if !self.price.is_finite() || !self.volume.is_finite() {
            return Err(FactReject::NotFinite);
        }
        if self.currency_code == 0 {
            return Err(FactReject::BadCurrency);
        }
        if !(-840..=840).contains(&self.tz_offset_min) {
            return Err(FactReject::BadTimezone);
        }
        if self.duration_ns == 0 {
            return Err(FactReject::BadWindow);
        }
        Ok(())
    }
}

/// 工艺事实（[11] §5.3：process_unit/batch/recipe/phase/interlock/
/// 边界/维护窗）。
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessFact {
    pub process_unit: u32,
    pub batch: u32,
    pub recipe: u32,
    pub recipe_version: u32,
    pub phase: u16,
    /// 联锁掩码（位域；由 PLC 语义层解释——EnerOS 只做规划前额外约束）。
    pub interlock_mask: u64,
    pub mode: u8,
    pub rate_min: f64,
    pub rate_max: f64,
    pub temp_min: f64,
    pub temp_max: f64,
    pub pressure_min: f64,
    pub pressure_max: f64,
    pub flow_min: f64,
    pub flow_max: f64,
    pub quality_min: f64,
    /// 维护窗（None = 无；用于 capability 收紧）。
    pub maintenance: Option<u64>,
}

impl FactPayload for ProcessFact {
    fn class(&self) -> u8 {
        class::PROCESS
    }

    fn payload_canonical(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(128);
        b.extend_from_slice(&self.process_unit.to_le_bytes());
        b.extend_from_slice(&self.batch.to_le_bytes());
        b.extend_from_slice(&self.recipe.to_le_bytes());
        b.extend_from_slice(&self.recipe_version.to_le_bytes());
        b.extend_from_slice(&self.phase.to_le_bytes());
        b.extend_from_slice(&self.interlock_mask.to_le_bytes());
        b.push(self.mode);
        for f in [
            self.rate_min,
            self.rate_max,
            self.temp_min,
            self.temp_max,
            self.pressure_min,
            self.pressure_max,
            self.flow_min,
            self.flow_max,
            self.quality_min,
        ] {
            b.extend_from_slice(&f.to_le_bytes());
        }
        match self.maintenance {
            Some(window) => {
                b.push(1);
                b.extend_from_slice(&window.to_le_bytes());
            }
            None => b.push(0),
        }
        b
    }

    fn validate_semantics(&self) -> Result<(), FactReject> {
        for v in [
            self.rate_min,
            self.rate_max,
            self.temp_min,
            self.temp_max,
            self.pressure_min,
            self.pressure_max,
            self.flow_min,
            self.flow_max,
            self.quality_min,
        ] {
            if !v.is_finite() {
                return Err(FactReject::NotFinite);
            }
        }
        if self.rate_min > self.rate_max
            || self.temp_min > self.temp_max
            || self.pressure_min > self.pressure_max
            || self.flow_min > self.flow_max
        {
            return Err(FactReject::BadBounds);
        }
        if !(1..=4).contains(&self.mode) {
            return Err(FactReject::BadMode);
        }
        if self.maintenance == Some(0) {
            return Err(FactReject::BadWindow);
        }
        Ok(())
    }
}

/// 企业事实（[11] §5.4：计划/工单/合同/碳预算/班次审批）。
#[derive(Debug, Clone, PartialEq)]
pub struct EnterpriseFact {
    pub kind: u16,
    pub subject_id: u32,
    pub priority: u8,
    /// 窗口（生效→过期；与 Header 结合：窗口 ⊂ 有效期）。
    pub window_from_ms: u64,
    pub window_to_ms: u64,
    /// 数值域（碳因子 tCO2/MWh、合同需量 kW 等；语义随 kind；有限性门）。
    pub value: f64,
    /// 是否要求人工审批（[11] §5.4：企业文本先转草案，人工/规则验证）。
    pub approval_required: u8,
}

impl FactPayload for EnterpriseFact {
    fn class(&self) -> u8 {
        class::ENTERPRISE
    }

    fn payload_canonical(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(48);
        b.extend_from_slice(&self.kind.to_le_bytes());
        b.extend_from_slice(&self.subject_id.to_le_bytes());
        b.push(self.priority);
        b.extend_from_slice(&self.window_from_ms.to_le_bytes());
        b.extend_from_slice(&self.window_to_ms.to_le_bytes());
        b.extend_from_slice(&self.value.to_le_bytes());
        b.push(self.approval_required);
        b
    }

    fn validate_semantics(&self) -> Result<(), FactReject> {
        if !(1..=5).contains(&self.kind) {
            return Err(FactReject::BadKind);
        }
        if !self.value.is_finite() {
            return Err(FactReject::NotFinite);
        }
        if self.window_from_ms >= self.window_to_ms {
            return Err(FactReject::BadWindow);
        }
        if self.approval_required > 1 {
            return Err(FactReject::BadApprovalFlag);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 信封与拒因
// ---------------------------------------------------------------------------

/// 接入拒绝（认证/纪律面）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FactReject {
    DigestMismatch,
    UnknownKey {
        key_id: [u8; 8],
    },
    BadSignature,
    BadRevision,
    ZeroExternalId,
    BadWindow,
    BadClassification,
    NotFinite,
    BadBounds,
    BadMode,
    BadKind,
    BadCurrency,
    BadTimezone,
    BadApprovalFlag,
    StaleRevision {
        revision: u32,
        seen: u32,
    },
    NotYetEffective {
        effective_from_ms: u64,
    },
    Expired {
        expires_at_ms: u64,
    },
    /// 事件面存储故障（sink 满/落库失败）——认证已通过，非纪律拒绝；
    /// 修订水位不推进，可重试。
    Sink(&'static str),
}

/// 签名事实信封（[11] §5.1 认证、版本化、有效期明确）。
#[derive(Debug, Clone, PartialEq)]
pub struct FactEnvelope<F: FactPayload> {
    pub header: FactHeader,
    pub payload: F,
    /// 签名者密钥 id + SM2 签名（r‖s 64B）。
    pub key_id: [u8; 8],
    pub signature: [u8; 64],
    /// canonical 载荷 SM3 摘要（防 payload/签名错配）。
    pub payload_digest: [u8; 32],
}

impl<F: FactPayload> FactEnvelope<F> {
    /// canonical（header canonical ‖ fact class ‖ payload canonical；
    /// 签名/摘要不参与）。
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(64 + 8 + self.payload.payload_canonical().len());
        b.extend_from_slice(&self.header.external_id);
        b.extend_from_slice(&self.header.revision.to_le_bytes());
        b.extend_from_slice(&self.header.source_id.to_le_bytes());
        b.push(self.header.classification);
        b.extend_from_slice(&self.header.published_at_ms.to_le_bytes());
        b.extend_from_slice(&self.header.effective_from_ms.to_le_bytes());
        b.extend_from_slice(&self.header.expires_at_ms.to_le_bytes());
        b.push(self.payload.class());
        b.extend_from_slice(&self.payload.payload_canonical());
        b
    }

    pub fn compute_digest(&self) -> [u8; 32] {
        eneros_crypto::sm3::hash(&self.canonical_bytes())
    }

    /// 全门校验（语义 → 摘要 → 签名——顺序固定；[11] §5.8）。验签
    /// 纪律走 keyring 单源（类型化——调用方不触碰底层验签布尔）。
    pub fn validate(&self, keys: &SignerKeyring) -> Result<(), FactReject> {
        self.header.validate()?;
        self.payload.validate_semantics()?;
        if self.compute_digest() != self.payload_digest {
            return Err(FactReject::DigestMismatch);
        }
        match keys.verify_authenticated(self.key_id, &self.canonical_bytes(), &self.signature) {
            Ok(_) => Ok(()),
            Err(VerifyReject::UnknownKey { key_id }) => Err(FactReject::UnknownKey { key_id }),
            Err(_) => Err(FactReject::BadSignature),
        }
    }
}

// ---------------------------------------------------------------------------
// 事件面 sink 与 adapter
// ---------------------------------------------------------------------------

/// 事实事件 sink（target 装配接 StoreSink/event-store；host 测试用记忆
/// 实现）。条目保留完整 canonical（含签名）——可追溯外部 revision 与
/// 签名（[11] §5.9）。
pub trait FactEventSink {
    fn append(
        &mut self,
        class: u8,
        external_id: &[u8; 16],
        revision: u32,
        canonical: &[u8],
        now_ns: u64,
    ) -> Result<u64, &'static str>;
}

/// 事件面条目（class, external_id, revision, canonical, now_ns）。
pub type FactEventEntry = (u8, [u8; 16], u32, Vec<u8>, u64);

/// 记忆 sink（测试/沙箱；有界 256 条）。
#[derive(Debug, Clone, Default)]
pub struct MemFactEventSink {
    pub entries: Vec<FactEventEntry>,
}

impl FactEventSink for MemFactEventSink {
    fn append(
        &mut self,
        class: u8,
        external_id: &[u8; 16],
        revision: u32,
        canonical: &[u8],
        now_ns: u64,
    ) -> Result<u64, &'static str> {
        if self.entries.len() >= 256 {
            return Err("sink full");
        }
        self.entries
            .push((class, *external_id, revision, canonical.to_vec(), now_ns));
        Ok(self.entries.len() as u64 - 1)
    }
}

/// 外部事实接入 adapter（push 模式；认证 + 修订水位 + 时效门 →
/// 事件面）。
pub struct FactAdapter {
    /// 签名密钥注册/验签/撤销/修订水位纪律单源（keyring——蓝图 62 卷
    /// 62.3 S5 收敛）。
    keys: SignerKeyring,
    pub accepted_total: u64,
    pub rejected_total: u64,
}

impl FactAdapter {
    pub fn new(keys: SignerKeyring) -> Self {
        Self {
            keys,
            accepted_total: 0,
            rejected_total: 0,
        }
    }

    pub fn revision_of(&self, external_id: &[u8; 16]) -> Option<u32> {
        self.keys.revision(external_id)
    }

    /// 接入一封已认证事实（[11] §5.1 纪律单点）。
    pub fn accept<F: FactPayload>(
        &mut self,
        sink: &mut dyn FactEventSink,
        env: &FactEnvelope<F>,
        now_ms: u64,
    ) -> Result<u64, FactReject> {
        // 认证门（语义 → 摘要 → 签名）；拒绝计数（审计面）。
        if let Err(r) = env.validate(&self.keys) {
            self.rejected_total += 1;
            return Err(r);
        }
        // 修订水位（严格递增；旧修订拒——不原地改值，[11] §5.5 append
        // 新 revision；水位表在 keyring 单源）。
        if let Some(seen) = self.keys.revision(&env.header.external_id) {
            if env.header.revision <= seen {
                self.rejected_total += 1;
                return Err(FactReject::StaleRevision {
                    revision: env.header.revision,
                    seen,
                });
            }
        }
        // 时效窗。
        if now_ms > env.header.expires_at_ms {
            self.rejected_total += 1;
            return Err(FactReject::Expired {
                expires_at_ms: env.header.expires_at_ms,
            });
        }
        if now_ms < env.header.effective_from_ms {
            self.rejected_total += 1;
            return Err(FactReject::NotYetEffective {
                effective_from_ms: env.header.effective_from_ms,
            });
        }
        // 事件面（canonical 全量保留——外部 revision 与签名可追溯）。
        // 存储面故障显式上报为 Sink——不得冒充窗口语义（BadWindow）；
        // 计数与水位均不动（认证已通过，可重试）。
        let canonical = env.canonical_bytes();
        let offset = sink
            .append(
                env.payload.class(),
                &env.header.external_id,
                env.header.revision,
                &canonical,
                now_ms.saturating_mul(1_000_000),
            )
            .map_err(FactReject::Sink)?;
        // 水位推进（sink 成功才推进；水位检查已保证严格递增——
        // NotAdvancing 不可达，防御臂如实计拒不静默）。
        if let Err(WatermarkReject::NotAdvancing { seen }) = self
            .keys
            .advance_revision(&env.header.external_id, env.header.revision)
        {
            self.rejected_total += 1;
            return Err(FactReject::StaleRevision {
                revision: env.header.revision,
                seen,
            });
        }
        self.accepted_total += 1;
        Ok(offset)
    }
}

/// 测试密钥（固定标量；与全仓 sandbox 密钥同源）。
pub fn test_keypair() -> (eneros_crypto::sm2::keypair::Sm2PrivateKey, Sm2PublicKey) {
    let mut sk_bytes = [0u8; 32];
    sk_bytes[31] = 0x2A;
    let sk = eneros_crypto::sm2::keypair::Sm2PrivateKey::from_bytes(&sk_bytes).unwrap();
    let pair = eneros_crypto::sm2::keypair::Sm2KeyPair::from_private_key(&sk).unwrap();
    (sk, pair.public_key)
}

/// 封印（sandbox/测试；生产私钥在外部系统侧）。
pub fn seal<F: FactPayload>(
    env: &mut FactEnvelope<F>,
    sk: &eneros_crypto::sm2::keypair::Sm2PrivateKey,
    pk: &Sm2PublicKey,
) {
    env.payload_digest = env.compute_digest();
    let signer = eneros_crypto::sm2::sign::Sm2Signer::new();
    let sig = signer
        .sign_deterministic(&env.canonical_bytes(), sk, pk)
        .expect("deterministic sign");
    env.signature = sig.to_bytes();
}

/// 拒绝诊断文本。
pub fn reject_reason(r: &FactReject) -> String {
    alloc::format!("{r:?}")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    fn header(external_id: [u8; 16], revision: u32) -> FactHeader {
        FactHeader {
            external_id,
            revision,
            source_id: 9,
            classification: 1,
            published_at_ms: 1000,
            effective_from_ms: 1000,
            expires_at_ms: 1_000_000,
        }
    }

    fn market_fact() -> MarketFact {
        MarketFact {
            product: 100,
            region: 1,
            delivery_start_ms: 1_000_000_000,
            duration_ns: 900_000_000_000,
            price: 412.5,
            volume: 10.0,
            currency_code: 156, // CNY
            tz_offset_min: 480,
        }
    }

    fn process_fact() -> ProcessFact {
        ProcessFact {
            process_unit: 7,
            batch: 3,
            recipe: 11,
            recipe_version: 2,
            phase: 5,
            interlock_mask: 0b1010,
            mode: process_mode::RUN,
            rate_min: 0.0,
            rate_max: 50.0,
            temp_min: 20.0,
            temp_max: 80.0,
            pressure_min: 0.0,
            pressure_max: 1.6,
            flow_min: 0.0,
            flow_max: 100.0,
            quality_min: 0.9,
            maintenance: None,
        }
    }

    fn enterprise_fact() -> EnterpriseFact {
        EnterpriseFact {
            kind: enterprise_kind::MAINTENANCE_ORDER,
            subject_id: 42,
            priority: 5,
            window_from_ms: 2_000,
            window_to_ms: 9_999,
            value: 0.0,
            approval_required: 1,
        }
    }

    fn sealed<F: FactPayload>(header: FactHeader, payload: F) -> FactEnvelope<F> {
        let mut env = FactEnvelope {
            header,
            payload,
            key_id: *b"testkey1",
            signature: [0; 64],
            payload_digest: [0; 32],
        };
        let (sk, pk) = test_keypair();
        seal(&mut env, &sk, &pk);
        env
    }

    fn adapter() -> FactAdapter {
        let (_sk, pk) = test_keypair();
        let mut keys = SignerKeyring::new();
        keys.register(*b"testkey1", &pk);
        FactAdapter::new(keys)
    }

    /// 合同套件（[11] §5.8）：三类事实共用同一纪律面。
    fn run_contract_suite<F: FactPayload>(name: &str, payload: F) {
        let mut a = adapter();
        let mut sink = MemFactEventSink::default();
        let env = sealed(header([7; 16], 1), payload.clone());
        // 正向：验签 + 修订水位 + 事件面落库（可追溯 revision + 签名）。
        let off = a.accept(&mut sink, &env, 5000).unwrap();
        assert_eq!(off, 0);
        assert_eq!(sink.entries.len(), 1);
        assert_eq!(sink.entries[0].2, 1, "{name} revision 可追溯");
        assert_eq!(a.revision_of(&[7; 16]), Some(1));
        // 旧修订 → 拒。
        let stale = sealed(header([7; 16], 1), payload.clone());
        assert!(matches!(
            a.accept(&mut sink, &stale, 5000),
            Err(FactReject::StaleRevision {
                revision: 1,
                seen: 1
            })
        ));
        // 新修订通过。
        let newer = sealed(header([7; 16], 2), payload.clone());
        a.accept(&mut sink, &newer, 5000).unwrap();
        assert_eq!(a.revision_of(&[7; 16]), Some(2));
        // 过期 → 拒。
        let expired = sealed(header([8; 16], 1), payload.clone());
        assert!(matches!(
            a.accept(&mut sink, &expired, 2_000_000),
            Err(FactReject::Expired { .. })
        ));
        // 未生效 → 拒。
        let early = sealed(header([9; 16], 1), payload.clone());
        assert!(matches!(
            a.accept(&mut sink, &early, 500),
            Err(FactReject::NotYetEffective { .. })
        ));
        // 摘要篡改 → 拒。
        let mut tampered = sealed(header([10; 16], 1), payload.clone());
        tampered.payload_digest[0] ^= 0x01;
        assert_eq!(
            a.accept(&mut sink, &tampered, 5000),
            Err(FactReject::DigestMismatch)
        );
        // 签名篡改 → 拒。
        let mut badsig = sealed(header([11; 16], 1), payload.clone());
        badsig.signature[10] ^= 0xFF;
        assert!(matches!(
            a.accept(&mut sink, &badsig, 5000),
            Err(FactReject::BadSignature)
        ));
        // 未知 key → 拒。
        let mut ukey = sealed(header([12; 16], 1), payload.clone());
        ukey.key_id = *b"unknown1";
        assert!(matches!(
            a.accept(&mut sink, &ukey, 5000),
            Err(FactReject::UnknownKey { .. })
        ));
    }

    #[test]
    fn sink_full_not_misreported_as_badwindow() {
        // 存储面故障不是窗口语义：sink 满 → 结构化 Sink 错误（历史实现
        // map_err(|_| BadWindow) 把「sink full」谎报成 BadWindow，审计
        // 面失真）。拒绝计数只反映认证/纪律面；水位不推进（可重试）。
        let mut a = adapter();
        let mut sink = MemFactEventSink::default();
        for i in 0..256u32 {
            a.accept(
                &mut sink,
                &sealed(header([7; 16], i + 1), market_fact()),
                5000,
            )
            .unwrap();
        }
        assert_eq!(sink.entries.len(), 256, "sink 恰好打满");
        let rejected_before = a.rejected_total;
        let res = a.accept(
            &mut sink,
            &sealed(header([7; 16], 257), market_fact()),
            5000,
        );
        assert!(matches!(res, Err(FactReject::Sink("sink full"))));
        assert_eq!(a.rejected_total, rejected_before);
        assert_eq!(a.revision_of(&[7; 16]), Some(256), "水位未推进");
    }

    #[test]
    fn market_fact_contract_suite() {
        run_contract_suite("market", market_fact());
    }

    #[test]
    fn process_fact_contract_suite() {
        run_contract_suite("process", process_fact());
    }

    #[test]
    fn enterprise_fact_contract_suite() {
        run_contract_suite("enterprise", enterprise_fact());
    }

    #[test]
    fn semantics_rejected_per_class() {
        // 市场：非有限价格/未注币种/时区越界。
        let mut m = market_fact();
        m.price = f64::NAN;
        assert_eq!(m.validate_semantics(), Err(FactReject::NotFinite));
        let mut m = market_fact();
        m.currency_code = 0;
        assert_eq!(m.validate_semantics(), Err(FactReject::BadCurrency));
        let mut m = market_fact();
        m.tz_offset_min = 900;
        assert_eq!(m.validate_semantics(), Err(FactReject::BadTimezone));
        // 工艺：越界边界/未知模式。
        let mut p = process_fact();
        p.temp_min = 90.0;
        assert_eq!(p.validate_semantics(), Err(FactReject::BadBounds));
        let mut p = process_fact();
        p.mode = 9;
        assert_eq!(p.validate_semantics(), Err(FactReject::BadMode));
        // 企业：未知 kind/窗口倒置/审批标志非法。
        let mut e = enterprise_fact();
        e.kind = 99;
        assert_eq!(e.validate_semantics(), Err(FactReject::BadKind));
        let mut e = enterprise_fact();
        e.window_from_ms = 9_999;
        e.window_to_ms = 2_000;
        assert_eq!(e.validate_semantics(), Err(FactReject::BadWindow));
        let mut e = enterprise_fact();
        e.approval_required = 2;
        assert_eq!(e.validate_semantics(), Err(FactReject::BadApprovalFlag));
    }

    #[test]
    fn header_discipline() {
        let h = header([7; 16], 1);
        assert!(h.validate().is_ok());
        let mut h = header([7; 16], 1);
        h.revision = 0;
        assert_eq!(h.validate(), Err(FactReject::BadRevision));
        let h = header([0; 16], 1);
        assert_eq!(h.validate(), Err(FactReject::ZeroExternalId));
        let mut h = header([7; 16], 1);
        h.expires_at_ms = h.effective_from_ms;
        assert_eq!(h.validate(), Err(FactReject::BadWindow));
    }

    #[test]
    fn classification_bound() {
        let mut e = sealed(header([7; 16], 1), enterprise_fact());
        e.header.classification = 4;
        assert_eq!(e.header.validate(), Err(FactReject::BadClassification));
    }
}
