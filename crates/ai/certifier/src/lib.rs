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
//! - control-dispatch 的过渡 seal 发放面（`transition-certifier`）
//!   在目标侧快照/包络输入具备前保留（Phase 5 PlanCompiler 替换），
//!   见 ADR-051；本 crate 是 CertifiedPlan 的**规范生产者**。
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
#![cfg_attr(not(test), allow(dead_code))]

extern crate alloc;

pub mod eval;
pub mod proof;

use alloc::vec::Vec;

use eneros_decision_contract::{CandidatePlan, CertificateId, CertifiedPlan, SnapshotId};
use eneros_resource_registry::capability::CapabilityEnvelope;
use eneros_snapshot_service::TrustedSnapshot;
use sha2::Digest;

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
        // 0. 绑定校验（计划 ↔ 输入 ↔ horizon）。
        if plan.snapshot_id().0 != ctx.snapshot.snapshot_id {
            return Err(CertifierError::PlanMismatch("snapshot_id"));
        }
        if plan.snapshot_id().0 != ctx.spec.snapshot_id().0 {
            return Err(CertifierError::PlanMismatch("spec_snapshot"));
        }
        if plan.step_ns() != ctx.horizon_step_ns || plan.step_count() != ctx.horizon_step_count {
            return Err(CertifierError::PlanMismatch("horizon"));
        }
        if plan.horizon_start_ms() != ctx.horizon_start_ms {
            return Err(CertifierError::PlanMismatch("horizon_start"));
        }
        // 证据引用的目录版本与计划绑定目录必须同源——否则证书声称按
        // A 目录认证、证据引用 B 目录，且分叉不可检测。
        if ctx.constraint_catalog_version != plan.resource_catalog_version() {
            return Err(CertifierError::PlanMismatch("catalog_version"));
        }
        if !plan.is_valid_at(now_ms) {
            return Err(CertifierError::PlanExpired);
        }

        // 1. 独立 forward evaluator（[09] §4.2 全清单的本卷实现）。
        let records = eval::evaluate(ctx, plan)?;

        // 2. 证据摘要 + 证书（唯一 seal 发放点）。
        // 62 v0.315.0 WP-62.1.2：派生单源 = decision-contract
        // `plan_wire_sha256`（原手写 SHA-256 删除）。
        let plan_sha: [u8; 32] = eneros_decision_contract::CandidatePlan::plan_wire_sha256(plan);
        let proof_digest = proof::digest_records(&records, &plan_sha, CERTIFIER_VERSION);
        let cert = CertifiedPlan::new_certified(
            eneros_decision_contract::CertifierSeal::issue(),
            CertificateId::new(plan_sha[..16].try_into().expect("16B"))
                .map_err(|_| CertifierError::DigestMismatch("cert-id"))?,
            *plan.id(),
            plan_sha,
            SnapshotId(plan.snapshot_id().0),
            CERTIFIER_VERSION,
            plan.resource_catalog_version(),
            ctx.tolerance_profile_id,
            now_ms,
            plan.issued_at_ms().saturating_add(plan.ttl_ms()),
            records.len() as u32,
            proof_digest,
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
        let plan_sha: [u8; 32] = sha2::Sha256::digest(plan_wire).into();
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
