//! EnerOS 蓝图 61 [09] §3 —— v0.314.37 多资源 Planner 与真实 HiGHS。
//!
//! # 唯一不变量（[09] §3.1）
//!
//! 多资源 CandidatePlan 来自**真实求解 backend**，并完整报告求解状态、
//! gap、残差、变量映射、模型 digest 和输入证据；任何不合格结果在
//! parse 前拒绝。
//!
//! # 部署裁决（[09] §3.2 / ADR-040）
//!
//! HiGHS 1.7.2 为 C++ 静态库，当前只有 x86-64 构建；aarch64/seL4 目标
//! 侧无 C runtime/内存预算支撑——生产 planner 运行在**受控站内计算分区
//! （host）**，通过 DecisionRequest 接入；它没有 protocol writer、
//! Gateway key 或 seL4 device capability（类型面：本 crate 无任何
//! writer/dispatch 依赖）。wire、签名、deadline、input digest 与
//! Certifier 与部署位置无关。
//!
//! # 公开面（[16] §3.8）
//!
//! [`Planner::plan`] 是唯一入口：`(DecisionRequest, PlanInput) ->
//! CandidatePlan`。ModelBuilder/contributors/变量目录/backend 客户端
//! 全部私有（深模块）。
//!
//! # 求解 gate（[09] §3.5）
//!
//! backend error / Infeasible / Unbounded / Error / 无 incumbent 超时 →
//! 拒绝；Optimal → 残差 gate；Feasible/Suboptimal → gap+残差+deadline
//! gate（[`tolerance::ToleranceProfile`]，L2 默认只接受 Optimal）。
//! 变量缺失/非有限一律 [`PlanError::MalformedSolution`]——**禁止
//! `unwrap_or(0)`**。
//!
//! # 偏离登记（[09] §3.9 门内如实声明）
//!
//! - assignments BlobRef 依赖存储句柄移交（Phase 2/3 已登记 BLOCKED）：
//!   inline 上限 64 分配，超出 → [`PlanError::AssignmentsExceedInline`]；
//!   超大规模验证走 [`Planner::solve_document`]（不产出 CandidatePlan）。
//! - 启动次数/最小启停（状态机类）：min on/off 由包络 min_on/off 携带
//!   时生效；启动次数依赖跨 horizon durable 状态（无输入源，登记待
//!   Phase 5/6）。
//! - 软约束当前不进模型（合同面单一 ObjectiveKind；多目标加权待
//!   Phase 6 业务接入）；目标项归一化参数随 bundle 携带、权重在
//!   CompiledObjective 证据链中。

#![cfg_attr(not(test), allow(dead_code))]

pub mod builder;
pub mod catalog;
pub mod gate;
pub mod tolerance;

use std::collections::BTreeMap;
use std::vec::Vec;

use catalog::VariableCatalog;
use eneros_decision_contract::{
    Assignment, CandidatePlan, DecisionRequest, PlanId, SnapshotId, SolveStatusKind,
};
use eneros_resource_registry::capability::CapabilityEnvelope;
use gate::SolveGate;
use sha2::Digest;
use tolerance::ToleranceProfile;

/// 单资源解（变量目录投影；数值全部来自解向量，缺失即错）。
#[derive(Debug, Clone)]
pub struct ResourceSolution {
    pub asset_id: u64,
    /// 每步净功率（kW；正=注入/放电）。
    pub net_kw: Vec<f64>,
}

/// 求解产出（plan 与 document 两种形态共用）。
#[derive(Debug, Clone)]
pub struct SolveOutcome {
    pub resources: Vec<ResourceSolution>,
    pub objective_value: f64,
    pub relative_gap: f64,
    pub max_primal_residual: f64,
    pub max_integrality_residual: f64,
    pub solver_status: SolveStatusKind,
    pub model_digest: [u8; 32],
    pub catalog: VariableCatalog,
    pub elapsed_ms: u64,
    pub horizon_start_ms: u64,
    pub step_ns: u64,
    pub step_count: u32,
}

/// planner 错误（结构化；一切拒绝可观测）。
#[derive(Debug, Clone, PartialEq)]
pub enum PlanError {
    /// 输入与 DecisionRequest 不绑定（snapshot/registry 不一致）。
    RequestMismatch(&'static str),
    /// CandidatePlan 合同构造失败。
    CandidateInvalid(String),
    /// 模型构建失败（缺规格资源/非法约束）。
    ModelBuild(builder::ModelError),
    /// backend 调用失败。
    Backend(String),
    /// backend 拒绝求解（Infeasible/Unbounded/Error/无 incumbent 超时）。
    Rejected(String),
    /// 解向量长度/取值不合格（缺列/NaN——不 default-fill）。
    MalformedSolution(&'static str),
    /// 残差超容差。
    ResidualExceeded { primal: f64, limit: f64 },
    /// 整数性残差超容差。
    IntegralityExceeded { residual: f64, limit: f64 },
    /// 相对 gap 超策略。
    GapExceeded { gap: f64, limit: f64 },
    /// 求解 + 下游预算超期。
    DeadlineExceeded { elapsed_ms: u64, budget_ms: u32 },
    /// 分配超 inline 上限（BlobRef 待存储句柄移交）。
    AssignmentsExceedInline { needed: usize, cap: usize },
    /// 计划资产缺 device/point 绑定。
    MissingBinding { asset_id: u64 },
}

/// planner 配置。
pub struct Planner {
    profile: ToleranceProfile,
    /// 下游最坏时延预留（ms；deadline = budget - margin）。
    downstream_margin_ms: u32,
    /// 上轮热启动（model digest → 解向量；维度/digest 一致才注入）。
    warm: Option<([u8; 32], Vec<f64>)>,
}

impl Planner {
    pub fn new(profile: ToleranceProfile, downstream_margin_ms: u32) -> Self {
        Self {
            profile,
            downstream_margin_ms,
            warm: None,
        }
    }

    /// 唯一入口（[16] §3.8）：DecisionRequest + PlanInput → CandidatePlan。
    pub fn plan(
        &mut self,
        backend: &mut dyn eneros_solver_core::solver::Solver,
        req: &DecisionRequest,
        input: &builder::PlanInput,
    ) -> Result<CandidatePlan, PlanError> {
        let out = self.solve_document(backend, req, input)?;
        // —— CandidatePlan 装配 ——
        let mut assignments = Vec::new();
        for res in &out.resources {
            let bind = input
                .bindings
                .iter()
                .find(|b| b.asset_id == res.asset_id)
                .ok_or(PlanError::MissingBinding {
                    asset_id: res.asset_id,
                })?;
            for (t, &p) in res.net_kw.iter().enumerate() {
                assignments.push(Assignment {
                    asset_id: res.asset_id,
                    device_id: bind.device_id,
                    point_id: bind.point_id,
                    power_kw: p,
                    unit_code: bind.unit_code,
                    step_index: t as u32 + 1,
                });
            }
        }
        if assignments.len() > eneros_decision_contract::candidate::MAX_INLINE_ASSIGNMENTS {
            return Err(PlanError::AssignmentsExceedInline {
                needed: assignments.len(),
                cap: eneros_decision_contract::candidate::MAX_INLINE_ASSIGNMENTS,
            });
        }
        if assignments.is_empty() {
            return Err(PlanError::MissingBinding { asset_id: 0 });
        }
        // plan_id 确定性派生（请求 + 模型 + 输入 + 签发时刻）。
        let mut h = sha2::Sha256::new();
        h.update(b"eneros.plan-id.v1");
        h.update(req.request_id());
        h.update(out.model_digest);
        h.update(input.objective.spec_digest);
        h.update(input.provenance_root);
        h.update(input.now_ms.to_le_bytes());
        let digest: [u8; 32] = h.finalize().into();
        let plan_id = PlanId::new(digest[..16].try_into().expect("16B"))
            .map_err(|_| PlanError::RequestMismatch("plan-id"))?;
        let ttl_ms = req.budget_ms().max(1_000) as u64;
        let issued = input.now_ms;
        CandidatePlan::new(
            plan_id,
            SnapshotId(input.snapshot_id),
            input.objective.spec_digest,
            input.registry_generation,
            out.horizon_start_ms,
            out.step_ns,
            out.step_count,
            out.solver_status,
            out.objective_value,
            out.relative_gap,
            out.max_primal_residual,
            out.elapsed_ms,
            out.model_digest,
            input.provenance_root,
            assignments,
            issued,
            ttl_ms,
        )
        .map_err(|e| PlanError::CandidateInvalid(format!("candidate:{e:?}")))
    }

    /// 求解文档（不装配 CandidatePlan；C1 规模验证入口）。
    pub fn solve_document(
        &mut self,
        backend: &mut dyn eneros_solver_core::solver::Solver,
        req: &DecisionRequest,
        input: &builder::PlanInput,
    ) -> Result<SolveOutcome, PlanError> {
        // 0. 请求绑定（同一快照 + 同一 registry 世代 + 同一规格）。
        if req.objective().snapshot_id().0 != input.snapshot_id {
            return Err(PlanError::RequestMismatch("snapshot_id"));
        }
        if req.resource_catalog_version() != input.registry_generation {
            return Err(PlanError::RequestMismatch("registry_generation"));
        }
        if req.objective() != &input.objective.spec {
            return Err(PlanError::RequestMismatch("objective_spec"));
        }
        if !req.objective().is_valid_at(input.now_ms) {
            return Err(PlanError::RequestMismatch("spec expired"));
        }

        // 1. 建模。
        let built = builder::ModelBuilder::build(input).map_err(PlanError::ModelBuild)?;
        // 模型 digest 一致才注入热启动。
        if let Some((digest, sol)) = &self.warm {
            if *digest == built.model_digest {
                let _ = backend.set_warm_start(sol);
            }
        }
        // 2. 时间预算：backend 原生 time_limit + 外层 deadline 记账。
        let started = std::time::Instant::now();
        let budget = req.budget_ms().saturating_sub(self.downstream_margin_ms);
        let limit_s = (budget.max(1) as f64 / 1000.0).min(3600.0);
        let _ = backend.set_param("time_limit", &format!("{}", limit_s));
        let result = backend
            .solve(&built.problem, input.now_ms)
            .map_err(|e| PlanError::Backend(format!("{e}")));
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let result = result?;

        // 3. 求解 gate（[09] §3.5）。
        let gate = SolveGate::new(self.profile);
        let verdict = gate.evaluate(&result, &built, elapsed_ms, budget)?;
        // 4. 热启动记忆（同 digest 复用）。
        self.warm = Some((built.model_digest, result.solution.clone()));
        let resources = builder::project_solution(&built, &result.solution)?;
        Ok(SolveOutcome {
            resources,
            objective_value: verdict.objective_value,
            relative_gap: verdict.relative_gap,
            max_primal_residual: verdict.max_primal_residual,
            max_integrality_residual: verdict.max_integrality_residual,
            solver_status: verdict.status,
            model_digest: built.model_digest,
            catalog: built.catalog,
            elapsed_ms,
            horizon_start_ms: input.now_ms,
            step_ns: input.objective.horizon_step_ns,
            step_count: input.objective.horizon_step_count,
        })
    }
}

/// 资产绑定记录（装配面从 PlannerView 解析注入）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AssetBinding {
    pub asset_id: u64,
    pub device_id: u16,
    pub point_id: u32,
    pub unit_code: u16,
}

/// 包络查询（asset → 包络；BTreeMap 薄包装）。
pub struct EnvelopeIndex<'a> {
    by_asset: BTreeMap<u64, &'a CapabilityEnvelope>,
}

impl<'a> EnvelopeIndex<'a> {
    pub fn new(envs: &'a [(u64, CapabilityEnvelope)]) -> Self {
        Self {
            by_asset: envs.iter().map(|(id, e)| (*id, e)).collect(),
        }
    }
    pub fn get(&self, asset_id: u64) -> Option<&'a CapabilityEnvelope> {
        self.by_asset.get(&asset_id).copied()
    }
}
