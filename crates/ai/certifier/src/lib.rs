//! EnerOS 蓝图 61 [09] §4 —— v0.314.38 独立 CertifierModule。
//!
//! # 唯一不变量（[09] §4.1）
//!
//! 只有 CertifierModule 能生成 [`CertifiedPlan`]；它**独立重算全部硬
//! 约束**，并拒绝 solver、规则、LLM 或手工伪造的"安全"声明。
//!
//! # 独立性边界（[09] §4.2，类型与依赖面共同执行）
//!
//! - 不依赖 planner（Cargo 依赖面即无；不复用 solver 的 validate 或
//!   约束矩阵 pass/fail——评估器只消费计划轨迹 + 物理参数）；
//! - 不接受 CandidatePlan 的任何"安全声明"（合同类型上不存在该字段）；
//! - 不使用生成器 default-fill 的状态（缺值即拒绝）；
//! - 不修改/钳位候选计划（评估只读）；
//! - 不访问设备 writer（无 writer 依赖）。
//!
//! # 证书（[09] §4.3）
//!
//! [`CertifiedPlan`] 按 [02] 固定字段；proof Blob 含约束 id、评估
//! lhs/rhs、单位、容差、pass/fail、输入 refs、certifier code/config
//! digest。证书以站内 certifier key 的 HMAC 绑定（[`proof::
//! CertificateEnvelope`]）；验签只证明认证产物来源，**不替代 Gateway
//! 权限裁决**。
//!
//! # 偏离登记（[09] §4.7 门内如实声明）
//!
//! - 充放/购售互斥在净功率合同面上**不可观测**（Assignment 只携带净
//!   功率）：同向抵消无物理危害（设备命令为净量）、能量守恒仍被独立
//!   校验——互斥由 planner 模型 binary 约束保证，登记为模型侧性质。
//! - 发放面（蓝图 70.9 起，复查订正旧文）：**唯一**发放点 = 本 crate 的
//!   [`CertifierModule::certify`]（`CertifiedPlan::new_certified` +
//!   `CertifierSeal::issue`）；`control-dispatch` 的过渡发放面
//!   （`transition-certifier` / `CertifierSeal::issue_transition`）已按
//!   ADR-051 出口条件**删除**。生产消费者 = `svc-control-dispatch`
//!   （经 `CertificateSource` 接缝：发放 `TrustedInputCertifier`、验真
//!   `CertifierKeyRing`），故"生产零消费者"不再是事实。
//!   注意：`CertifierSeal` 是 **feature 门**（Cargo feature 在同图内
//!   合并）——凡依赖本 crate 的构建图都会打开 `certifier` feature，
//!   于是 `issue()` 在语法上可达；发放面唯一性由**源码扫描门**执法
//!   （`tests/integration.rs::seal_issuance_source_scan_rejects_third_party_callers`），
//!   不是类型系统事实。
//! - min on/off 与工艺状态机类约束：净功率面可校验包络与能量，启停
//!   状态机依赖合同拓扑外的历史状态（登记待 Phase 5/6 输入源）。
//! - 控制合同与可读回条件（[09] §4.2）：回读合格性由 registry 合同
//!   面在包络求交时承载（[08] §4.4 readback_ok），命令级读回确认在
//!   dispatch 执行链——评估器输入合同中无逐命令读回语义，登记待
//!   Phase 5 CommandIntent 前置条件接入。
//!
//! # 依赖纪律（[16] §3.9）
//!
//! 禁止依赖 planner internals、LLM、writer。no_std + alloc（可移植到
//! 目标侧 svc-certifier）。

#![no_std]

// 蓝图 69.4 WP-69.4.3（方案 B1）：原 `#![cfg_attr(not(test), allow(dead_code))]`
// 全 crate 豁免已删——「暂不接电」不该永久静音编译器。
// 蓝图 70.9（复查订正）：本 crate **已接电**（发放 TrustedInputCertifier /
// 验真 CertifierKeyRing，生产消费者 = svc-control-dispatch）。此后新增
// dead_code 即真实腐坏信号：以 `cargo build -p eneros-certifier 2>&1 |
// grep dead_code` 复核，不得以"暂不接电"为由放行。

extern crate alloc;

// 蓝图 70.9 WP-70.9.2：可信输入 → 认证上下文装配（纯函数）。
pub mod context;
pub mod eval;
pub mod proof;

use alloc::vec::Vec;

use eneros_decision_contract::{CandidatePlan, CertificateId, CertifiedPlan, SnapshotId};
use eneros_resource_registry::capability::CapabilityEnvelope;
use eneros_snapshot_service::TrustedSnapshot;

/// certifier 语义版本（证书 certifier_version 字段来源）。
pub const CERTIFIER_VERSION: u32 = 1;

/// 储能物理参数（独立评估的 SOC 推进输入；与 planner 输入同源不同型
/// ——certifier 不 import planner 类型）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EvalBattery {
    pub asset_id: u64,
    /// 额定容量（kWh；>0）。
    pub capacity_kwh: f64,
    /// 充/放电效率 (0,1]。
    pub eta_c: f64,
    pub eta_d: f64,
    /// 初始能量（kWh）。
    pub initial_kwh: f64,
}

/// 评估输入（[09] §4.2：同一签名 catalog/snapshot 只读视图）。
pub struct EvalContext<'a> {
    pub snapshot: &'a TrustedSnapshot,
    pub spec: &'a eneros_decision_contract::ObjectiveSpec,
    pub envelopes: &'a [(u64, CapabilityEnvelope)],
    pub batteries: &'a [EvalBattery],
    /// 计划 horizon（与 CandidatePlan 字段一致性校验）。
    pub horizon_step_ns: u64,
    pub horizon_step_count: u32,
    pub horizon_start_ms: u64,
    /// 并网点资产 id（0 = 无并网点；站级功率/承诺/备用约束的落点）。
    pub grid_asset_id: u64,
    /// 容差档 id（1=L2，2=L3；表见 [`eval::tolerance_for`]）。
    pub tolerance_profile_id: u32,
    /// 约束/资源目录版本（计划 resource_catalog_version 同源；证据引用）。
    pub constraint_catalog_version: u64,
}

impl EvalContext<'_> {
    /// 绑定校验判据（WP-64.3.3.2；自 [`CertifierModule::certify`] 步骤 0
    /// 原样迁入——计划 ↔ 输入 ↔ horizon 五对 + TTL；错误变体/词形/比较
    /// 松紧零变化，仅位置迁移：判据与字段同居一处，已登记未修的
    /// "snapshot 包络输入源"字段未来只改这里）。
    pub fn bind(&self, plan: &CandidatePlan, now_ms: u64) -> Result<(), CertifierError> {
        if plan.snapshot_id().0 != self.snapshot.snapshot_id {
            return Err(CertifierError::PlanMismatch("snapshot_id"));
        }
        if plan.snapshot_id().0 != self.spec.snapshot_id().0 {
            return Err(CertifierError::PlanMismatch("spec_snapshot"));
        }
        // 步长比对走**有效步长**（单源 [`effective_step_ns`]）：迁移窗
        // 单时段（step_ns = 0）以 TTL 为等效步长，上下文必须同值。
        if self.horizon_step_ns != effective_step_ns(plan)
            || plan.step_count() != self.horizon_step_count
        {
            return Err(CertifierError::PlanMismatch("horizon"));
        }
        if plan.horizon_start_ms() != self.horizon_start_ms {
            return Err(CertifierError::PlanMismatch("horizon_start"));
        }
        // 证据引用的目录版本与计划绑定目录必须同源——否则证书声称按
        // A 目录认证、证据引用 B 目录，且分叉不可检测。
        if self.constraint_catalog_version != plan.resource_catalog_version() {
            return Err(CertifierError::PlanMismatch("catalog_version"));
        }
        if !plan.is_valid_at(now_ms) {
            return Err(CertifierError::PlanExpired);
        }
        Ok(())
    }
}

/// **有效步长**（迁移窗单时段语义单源）：`step_ns == 0` 表示"单步覆盖
/// 计划有效期"，评估器以计划 TTL 为步长（与 dispatch 编译期的 legacy
/// 窗口语义同域）。严格轨迹形态（step_ns > 0）原值返回。
pub fn effective_step_ns(plan: &eneros_decision_contract::CandidatePlan) -> u64 {
    if plan.step_ns() == 0 {
        plan.ttl_ms().max(1).saturating_mul(1_000_000)
    } else {
        plan.step_ns()
    }
}

/// 认证结果（证书 + 证据）。
#[derive(Debug, Clone, PartialEq)]
pub struct CertifiedOutcome {
    pub certificate: CertifiedPlan,
    /// 逐约束证据（proof Blob 规范编码的展开）。
    pub records: Vec<proof::ProofRecord>,
    /// 证据摘要（CertifiedPlan.proof_digest 同源）。
    pub proof_digest: [u8; 32],
    /// key 绑定信封（Gateway 验证输入）。
    pub key_envelope: proof::CertificateEnvelope,
    /// 计划 wire SHA-256（绑定校验输入）。
    pub plan_sha256: [u8; 32],
}

/// 认证错误（结构化拒绝——一切不合格可观测）。
#[derive(Debug, Clone, PartialEq)]
pub enum CertifierError {
    /// 计划与评估输入不绑定（快照/horizon/步数不一致）。
    PlanMismatch(&'static str),
    /// 计划已过期 / 窗口倒挂。
    PlanExpired,
    /// 缺分配（结构性缺值——不 default-fill）。
    MissingAssignment { asset_id: u64, step: u32 },
    /// 分配非法（越步/重复）。
    MalformedAssignment(&'static str),
    /// 非有限值。
    NonFinite(&'static str),
    /// 包络缺失/不可用/不在有效期。
    Envelope { asset_id: u64 },
    /// 轨迹违约（逐条证据引用 record 序号；**随错误携带全部已产生证据**
    /// ——诊断重放面，兑现"记录保留供诊断重放"承诺，[09] §4.3）。
    Violated {
        constraint_kind: u16,
        asset_id: u64,
        record_index: usize,
        /// 违约时已产生的全部证据记录（含本条 fail 记录）。
        records: Vec<proof::ProofRecord>,
    },
    /// 评估输入不合法（容量/效率/步长）。
    InvalidInput(&'static str),
    /// 摘要/绑定失败。
    DigestMismatch(&'static str),
}

/// 独立认证器（深模块；无状态——可并发复用）。
pub struct CertifierModule;

impl CertifierModule {
    /// 唯一认证入口（[16] §3.9 公开面）。
    ///
    /// `ring` 必须提供——key 绑定信封在认证内**就地签发**（fail-closed：
    /// 不存在未绑定 MAC 的"裸证书"流出，[09] §4.3）。
    pub fn certify(
        ctx: &EvalContext<'_>,
        plan: &CandidatePlan,
        now_ms: u64,
        ring: &proof::CertifierKeyRing,
    ) -> Result<CertifiedOutcome, CertifierError> {
        // 0. 绑定校验（WP-64.3.3.2：判据迁 [`EvalContext::bind`]）。
        ctx.bind(plan, now_ms)?;

        // 1. 独立 forward evaluator（[09] §4.2 全清单的本卷实现）。
        let records = eval::evaluate(ctx, plan)?;

        // 2. 证据摘要 + 证书（唯一 seal 发放点）。
        // 62 v0.315.0 WP-62.1.2：派生单源 = decision-contract
        // `plan_wire_sha256`（原手写 SHA-256 删除）。
        let plan_sha: [u8; 32] = eneros_decision_contract::CandidatePlan::plan_wire_sha256(plan);
        let proof_digest = proof::digest_records(&records, &plan_sha, CERTIFIER_VERSION);
        let certificate_id = CertificateId::new(plan_sha[..16].try_into().expect("16B"))
            .map_err(|_| CertifierError::DigestMismatch("cert-id"))?;
        let cert = CertifiedPlan::new_certified(
            eneros_decision_contract::CertifierSeal::issue(),
            eneros_decision_contract::CertifiedPlanFields {
                certificate_id,
                plan_id: *plan.id(),
                plan_sha256: plan_sha,
                snapshot_id: SnapshotId(plan.snapshot_id().0),
                certifier_version: CERTIFIER_VERSION,
                constraint_catalog_version: plan.resource_catalog_version(),
                tolerance_profile_id: ctx.tolerance_profile_id,
                certified_at_ms: now_ms,
                valid_until_ms: plan.issued_at_ms().saturating_add(plan.ttl_ms()),
                hard_constraint_count: records.len() as u32,
                proof_digest,
            },
        )
        .map_err(|_| CertifierError::PlanMismatch("certificate"))?;

        // 3. key 绑定（fail-closed：无活跃 key → 认证失败，零证书流出；
        // 不存在未绑定 MAC 的"裸证书"，[09] §4.3；空 key 拒绝——
        // keyed.rs 生产调用方纪律在本发放面执行）。
        let (key_id, key_version, key) = ring
            .active()
            .ok_or(CertifierError::InvalidInput("no active key"))?;
        if key.is_empty() {
            return Err(CertifierError::InvalidInput("empty key"));
        }
        // MAC 覆盖证书自身规范 wire（有效期/计数/引用不可离 MAC 篡改）
        // ‖ plan_sha ‖ proof_digest。
        let input = proof::mac_input(&cert.encode_v1(), &plan_sha, &proof_digest);
        let key_envelope = proof::CertificateEnvelope {
            key_id,
            key_version,
            mac: eneros_decision_contract::keyed::hmac_sha256(key, &input),
        };
        Ok(CertifiedOutcome {
            certificate: cert,
            records,
            proof_digest,
            key_envelope,
            plan_sha256: plan_sha,
        })
    }

    /// Gateway 侧验证（[09] §4.3：key id、版本、有效期、plan digest）。
    ///
    /// 只证明认证产物来源——**不替代 Gateway 权限裁决**。
    pub fn verify(
        cert: &CertifiedPlan,
        plan_wire: &[u8],
        key_envelope: &proof::CertificateEnvelope,
        ring: &proof::CertifierKeyRing,
        now_ms: u64,
    ) -> Result<(), CertifierError> {
        if !cert.is_valid_at(now_ms) {
            return Err(CertifierError::PlanExpired);
        }
        let plan_sha: [u8; 32] =
            eneros_decision_contract::CandidatePlan::plan_wire_sha256_of_wire(plan_wire);
        if plan_sha != *cert.plan_sha256() {
            return Err(CertifierError::DigestMismatch("plan_sha"));
        }
        let Some((_, version, key)) = ring.key(key_envelope.key_id) else {
            return Err(CertifierError::DigestMismatch("unknown key"));
        };
        if version != key_envelope.key_version {
            return Err(CertifierError::DigestMismatch("key version"));
        }
        if key.is_empty() {
            return Err(CertifierError::InvalidInput("empty key"));
        }
        // 绑定 = HMAC(cert_wire ‖ plan_sha ‖ proof)——证书字段（含有效
        // 期）参与 MAC，篡改任意字段即失配。
        let input = proof::mac_input(&cert.encode_v1(), &plan_sha, cert.proof_digest());
        let expect = eneros_decision_contract::keyed::hmac_sha256(key, &input);
        if eneros_decision_contract::keyed::mac_eq(&expect, &key_envelope.mac) {
            Ok(())
        } else {
            Err(CertifierError::DigestMismatch("mac"))
        }
    }
}

// ============================================================================
// 测试缝：真实评估器驱动的证书夹具（`test-util` 门）
// ============================================================================

/// 夹具证书（与 `control-dispatch::CertifierReply` 同形；本 crate 不依赖
/// control-dispatch——调用方映射）。
#[cfg(any(test, feature = "test-util"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureReply {
    pub certificate: alloc::vec::Vec<u8>,
    pub key_id: u16,
    pub key_version: u32,
    pub mac: [u8; 32],
}

/// 单资产夹具认证器（**真实** `CertifierModule::certify`——证据/约束记录
/// /MAC 全真；输入是夹具值）。生产代码不得使用：由 `test-util` feature
/// 门控（未启用时本模块不存在）。
///
/// 身份输入**全部取自被认证的计划**（快照 ID / 目录版本 / 时域三元组），
/// 因此一个夹具实例可服务任意计划；夹具固定：单资产（asset 1）、容量
/// `capacity_kwh`、功率窗 `[p_min, p_max]`、初始 SOC `soc`。
#[cfg(any(test, feature = "test-util"))]
pub struct FixtureCertifier {
    capacity_kwh: f64,
    p_min_kw: f64,
    p_max_kw: f64,
    soc: f64,
    ring: proof::CertifierKeyRing,
    tolerance_profile_id: u32,
}

#[cfg(any(test, feature = "test-util"))]
impl FixtureCertifier {
    /// 构造（key 材 + 资产参数）。
    pub fn new(key: &[u8], capacity_kwh: f64, p_min_kw: f64, p_max_kw: f64, soc: f64) -> Self {
        Self {
            capacity_kwh,
            p_min_kw,
            p_max_kw,
            soc,
            ring: proof::CertifierKeyRing::new(1, key),
            tolerance_profile_id: 2,
        }
    }

    /// 验方 key 环（引擎验真用）。
    pub fn ring(&self) -> &proof::CertifierKeyRing {
        &self.ring
    }

    /// 认证（真实评估器；身份取自计划）。
    pub fn certify(
        &self,
        plan: &eneros_decision_contract::CandidatePlan,
        now_ms: u64,
    ) -> Result<FixtureReply, CertifierError> {
        use eneros_decision_contract::{
            Constraint, ConstraintKind, ObjectiveKind, ObjectiveSource, ObjectiveSpec, ReasonCode,
            SnapshotId,
        };
        use eneros_resource_registry::capability::{Availability, CapabilityEnvelope};
        use eneros_snapshot_service::{CompletenessReport, TrustedSnapshot};
        let snapshot_id = plan.snapshot_id().0;
        let now_ns = plan.horizon_start_ms().saturating_mul(1_000_000);
        let snapshot = TrustedSnapshot {
            snapshot_id,
            tenant_id: 1,
            site_id: 1,
            cut_time_ns: now_ns,
            produced_at_ns: now_ns,
            policy_version: 1,
            version_set: eneros_semantic_catalog::VersionSet::ZERO,
            observations: alloc::vec::Vec::new(),
            source_watermarks: alloc::vec::Vec::new(),
            completeness: CompletenessReport {
                required: alloc::vec::Vec::new(),
                accepted: alloc::vec::Vec::new(),
                missing: alloc::vec::Vec::new(),
                stale: alloc::vec::Vec::new(),
                bad: alloc::vec::Vec::new(),
                conflict: alloc::vec::Vec::new(),
                substituted: alloc::vec::Vec::new(),
            },
            conflicts: alloc::vec::Vec::new(),
            derived: alloc::vec::Vec::new(),
            provenance_root: [0x11; 32],
        };
        let envelope = CapabilityEnvelope {
            asset_id: 1,
            valid_from_ns: 0,
            valid_until_ns: u64::MAX / 2,
            p_min_kw: self.p_min_kw,
            p_max_kw: self.p_max_kw,
            q_min_kvar: 0.0,
            q_max_kvar: 0.0,
            energy_min_kwh: 0.0,
            energy_max_kwh: self.capacity_kwh,
            ramp_up_kw_s: 1_000_000.0,
            ramp_down_kw_s: 1_000_000.0,
            min_on_ns: 0,
            min_off_ns: 0,
            availability: Availability::Full,
            derating_reason: eneros_resource_registry::capability::DeratingReason::None,
            source_snapshot_id: snapshot_id,
            tightenings: alloc::vec::Vec::new(),
            generation: 1,
        };
        let battery = EvalBattery {
            asset_id: 1,
            capacity_kwh: self.capacity_kwh,
            eta_c: 0.95,
            eta_d: 0.95,
            initial_kwh: self.capacity_kwh * self.soc.clamp(0.0, 1.0),
        };
        // 有效步长单源（迁移窗单时段 ⇒ TTL 等效步长）。
        let step_ns = effective_step_ns(plan);
        let spec = ObjectiveSpec::builder(SnapshotId(snapshot_id))
            .constraints(
                alloc::vec![
                    Constraint {
                        kind: ConstraintKind::MinSoc,
                        bound: 0.05,
                    },
                    Constraint {
                        kind: ConstraintKind::MaxSoc,
                        bound: 0.95,
                    },
                ],
                alloc::vec::Vec::new(),
            )
            .objective(
                ObjectiveKind::SelfConsume,
                1,
                ObjectiveSource::Contract,
                ReasonCode::PriceArbitrage,
            )
            .window(
                plan.horizon_start_ms().max(1),
                plan.horizon_start_ms()
                    .max(1)
                    .saturating_add(plan.ttl_ms().max(1)),
            )
            .build()
            .map_err(|_| CertifierError::InvalidInput("fixture-spec"))?;
        let envelopes = [(1u64, envelope)];
        let batteries = [battery];
        // 步骤域对齐：`EvalContext::bind` 逐字段比对计划的 step_ns/count。
        // 计划 step_ns = 0（迁移单时段）时以等效步长评估，但 bind 会因
        // 步长不等拒绝——故夹具对该形态直接以计划值构造 ctx（评估器对
        // step_ns = 0 拒绝 InvalidInput，故此处改为声明式：把等效步长
        // 写回计划**不可行**（wire 绑定）——因此夹具只服务 step_ns > 0
        // 的形态；迁移单时段形态由调用方先归一）。
        let ctx = EvalContext {
            snapshot: &snapshot,
            spec: &spec,
            envelopes: &envelopes,
            batteries: &batteries,
            horizon_step_ns: step_ns,
            horizon_step_count: plan.step_count(),
            horizon_start_ms: plan.horizon_start_ms(),
            grid_asset_id: 0,
            tolerance_profile_id: self.tolerance_profile_id,
            constraint_catalog_version: plan.resource_catalog_version(),
        };
        let out = CertifierModule::certify(&ctx, plan, now_ms, &self.ring)?;
        Ok(FixtureReply {
            certificate: out.certificate.encode_v1(),
            key_id: out.key_envelope.key_id,
            key_version: out.key_envelope.key_version,
            mac: out.key_envelope.mac,
        })
    }
}

/// 夹具证书（**只供编译器/接线测试**：不做独立重算，`hard_constraint_count`
/// 为给定值）。引擎验真会拒（零约束记录/摘要不符）——这正是测试要区分
/// 的"结构合法证书"与"真认证证书"。
#[cfg(any(test, feature = "test-util"))]
pub fn fixture_certificate(
    plan: &eneros_decision_contract::CandidatePlan,
    plan_sha: [u8; 32],
    now_ms: u64,
    hard_constraint_count: u32,
) -> Result<eneros_decision_contract::CertifiedPlan, CertifierError> {
    use eneros_decision_contract::{
        CertificateId, CertifiedPlan, CertifiedPlanFields, CertifierSeal,
    };
    let certificate_id = CertificateId::new(
        plan_sha[..16]
            .try_into()
            .map_err(|_| CertifierError::InvalidInput("cert-id"))?,
    )
    .map_err(|_| CertifierError::InvalidInput("cert-id"))?;
    CertifiedPlan::new_certified(
        CertifierSeal::issue(),
        CertifiedPlanFields {
            certificate_id,
            plan_id: *plan.id(),
            plan_sha256: plan_sha,
            snapshot_id: plan.snapshot_id(),
            certifier_version: CERTIFIER_VERSION,
            constraint_catalog_version: plan.resource_catalog_version(),
            tolerance_profile_id: 2,
            certified_at_ms: now_ms,
            valid_until_ms: now_ms.saturating_add(plan.ttl_ms()),
            hard_constraint_count,
            proof_digest: [0xCD; 32],
        },
    )
    .map_err(|_| CertifierError::InvalidInput("cert-fields"))
}
