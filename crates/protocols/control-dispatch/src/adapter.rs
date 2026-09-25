//! 蓝图 61 v0.314.28：旧 proposal → CandidatePlan 迁移 adapter（[06]
//! §3.5.3"严格保留 status/digest"）+ dispatch 侧资源注册表只读视图。
//!
//! 边界纪律：proposal（旧形态）在 dispatch 边界经本 adapter 进入
//! CandidatePlan——链内（validate/authorize/persist/deliver）只消费
//! typed 产物，不再读裸 proposal 字段。2026-09-24 C2 收口：legacy
//! facade 入口（`engine::run`/trait `submit`）与位置实参四元组绑定退役，
//! 本模块只余**未绑定迁移形态**（v1 wire 臂/stub 后端）与**绑定形态**
//! （生产可信输入入口）。

use alloc::vec;

use eneros_decision_contract::{
    Assignment, CandidatePlan, CandidatePlanParts, PlanId, SnapshotId, SolveStatusKind,
};
use eneros_dual_brain::decision::ProposedControlPlan;

use crate::DispatchError;

/// 候选身份输入（蓝图 70.8 WP-70.8.4）：候选只能由**可信输入**构造。
///
/// 构造方（ai-worker 启发式/远程后端）从冻结快照身份派生本结构；验证方
/// （dispatch 引擎 / certifier）用同一公式重算 `objective_digest` 并比对
/// `snapshot_id` / `catalog_version` / 时域——入口**只验证，不改写**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateInputs {
    /// 冻结快照 ID（全零 = 未绑定 → 拒绝）。
    pub snapshot_id: [u8; 16],
    /// 目标文档摘要（= `decision_contract::objective_binding_v1(...)`；
    /// 结构化计划落地后升级为 `ObjectiveSpec::digest_v1()`——同一字段）。
    pub objective_digest: [u8; 32],
    /// 计划时域起点（ms；取冻结快照产出时刻——同代事实）。
    pub horizon_start_ms: u64,
    /// 单步时长（ns；0 = 单时段迁移形态，仅限 step_count == 1）。
    pub step_ns: u64,
    /// 步数（≥ 1）。
    pub step_count: u32,
    /// 目录合同版本（与快照身份携带的 `catalog_version` 必须一致）。
    pub catalog_version: u64,
}

impl CandidateInputs {
    /// 由冻结快照身份构造（单点派生：摘要公式与验证方共用）。
    pub fn from_snapshot(
        snapshot_id: [u8; 16],
        catalog_version: u64,
        horizon_start_ms: u64,
        step_ns: u64,
        step_count: u32,
    ) -> Self {
        Self {
            snapshot_id,
            objective_digest: eneros_decision_contract::objective_binding_v1(
                &snapshot_id,
                catalog_version,
                horizon_start_ms,
                step_ns,
                step_count,
            ),
            horizon_start_ms,
            step_ns,
            step_count,
            catalog_version,
        }
    }

    /// 自检（构造侧与验证侧同一判据）：身份非零、摘要与公式一致、时域
    /// 自洽。任何一项不成立 → 该候选不具备新控制资格。
    pub fn validate(&self) -> Result<(), DispatchError> {
        if self.snapshot_id == [0u8; 16] {
            return Err(DispatchError::ProposalInvalid(
                "candidate-inputs: 未绑定快照身份（snapshot_id 全零）".into(),
            ));
        }
        if self.catalog_version == 0 {
            return Err(DispatchError::ProposalInvalid(
                "candidate-inputs: 目录合同版本为零".into(),
            ));
        }
        if self.step_count == 0 {
            return Err(DispatchError::ProposalInvalid(
                "candidate-inputs: step_count 为零".into(),
            ));
        }
        if self.step_ns == 0 && self.step_count != 1 {
            return Err(DispatchError::ProposalInvalid(
                "candidate-inputs: 零步长只允许单时段（step_count == 1）".into(),
            ));
        }
        let expect = eneros_decision_contract::objective_binding_v1(
            &self.snapshot_id,
            self.catalog_version,
            self.horizon_start_ms,
            self.step_ns,
            self.step_count,
        );
        if expect != self.objective_digest {
            return Err(DispatchError::ProposalInvalid(
                "candidate-inputs: objective_digest 与绑定公式不符".into(),
            ));
        }
        Ok(())
    }
}

/// proposal → CandidatePlan（**绑定形态**；蓝图 70.8 WP-70.8.4 生产唯一
/// 入口）。身份全部来自可信输入，不再伪造：
/// - `SnapshotId` = 冻结快照 ID；
/// - `objective_digest` = 绑定公式输出（验证方重算即判同代）；
/// - 时域 = 快照产出时刻 + 步长/步数（迁移窗单步：step_ns = 0,
///   step_count = 1）；
/// - `resource_catalog_version` = 可信输入携带的目录合同版本。
pub fn proposal_to_candidate_bound(
    p: &ProposedControlPlan,
    inputs: &CandidateInputs,
    asset_binding: (u64, u16, u32, u16),
    unit_code: u16,
) -> Result<CandidatePlan, DispatchError> {
    inputs.validate()?;
    let status = solve_status_kind(p)?;
    let plan_id = plan_id_of(p)?;
    let (asset_id, device_id, point_id, _) = asset_binding;
    CandidatePlan::from_parts(CandidatePlanParts {
        id: plan_id,
        snapshot_id: SnapshotId(inputs.snapshot_id),
        objective_digest: inputs.objective_digest,
        resource_catalog_version: inputs.catalog_version,
        horizon_start_ms: inputs.horizon_start_ms,
        step_ns: inputs.step_ns,
        step_count: inputs.step_count,
        solver_status: status,
        objective_value: p.schedule.objective_value,
        relative_gap: 0.0,
        max_primal_residual: 0.0,
        elapsed_ms: 0,
        model_digest: p.model_digest.unwrap_or([0u8; 32]),
        input_digest: p.input_digest.unwrap_or([0u8; 32]),
        assignments: vec![Assignment {
            asset_id,
            device_id,
            point_id,
            power_kw: p.power_kw,
            unit_code,
            step_index: 1,
        }],
        issued_at_ms: p.issued_at_ms,
        ttl_ms: u64::from(p.ttl_secs.max(1)) * 1000,
    })
    .map_err(|e| DispatchError::ProposalInvalid(alloc::format!("candidate: {e:?}")))
}

/// 未绑定分配解析（`asset_id == 0` → `default_asset_id`；其余保持不变）。
///
/// 这是**声明式绑定规则**（不是身份改写）：快照/目录/时域/目标摘要字段
/// 零改动，只有占位资产被替换。引擎在认证与编译之前一次性解析——证书与
/// 命令批因此绑定同一份计划 wire。
pub fn resolve_unbound_assets(
    plan: &CandidatePlan,
    default_asset_id: u64,
) -> Result<CandidatePlan, DispatchError> {
    if default_asset_id == 0 || plan.assignments().iter().all(|a| a.asset_id != 0) {
        return Ok(plan.clone());
    }
    let assignments: alloc::vec::Vec<Assignment> = plan
        .assignments()
        .iter()
        .map(|a| {
            let mut a = *a;
            if a.asset_id == 0 {
                a.asset_id = default_asset_id;
            }
            a
        })
        .collect();
    CandidatePlan::from_parts(CandidatePlanParts {
        id: *plan.id(),
        snapshot_id: plan.snapshot_id(),
        objective_digest: *plan.objective_digest(),
        resource_catalog_version: plan.resource_catalog_version(),
        horizon_start_ms: plan.horizon_start_ms(),
        step_ns: plan.step_ns(),
        step_count: plan.step_count(),
        solver_status: plan.solver_status(),
        objective_value: plan.objective_value(),
        relative_gap: plan.relative_gap(),
        max_primal_residual: plan.max_primal_residual(),
        elapsed_ms: plan.elapsed_ms(),
        model_digest: *plan.model_digest(),
        input_digest: *plan.input_digest(),
        assignments,
        issued_at_ms: plan.issued_at_ms(),
        ttl_ms: plan.ttl_ms(),
    })
    .map_err(|e| DispatchError::ProposalInvalid(alloc::format!("resolve-unbound: {e:?}")))
}

/// 求解状态门（两入口共用；adapter 不放行非成功状态）。
fn solve_status_kind(p: &ProposedControlPlan) -> Result<SolveStatusKind, DispatchError> {
    match p.schedule.solve_status {
        eneros_energy_lp_model::result::SolveStatus::Optimal => Ok(SolveStatusKind::Optimal),
        eneros_energy_lp_model::result::SolveStatus::Suboptimal => Ok(SolveStatusKind::Suboptimal),
        _ => Err(DispatchError::ProposalInvalid(
            "solve_status 非成功（adapter 拒绝）".into(),
        )),
    }
}

/// plan_id → PlanId（UTF-8 前 16B 零填充；correlation 同派生）。
fn plan_id_of(p: &ProposedControlPlan) -> Result<PlanId, DispatchError> {
    let mut id_bytes = [0u8; 16];
    let pb = p.plan_id.as_bytes();
    let n = core::cmp::min(16, pb.len());
    id_bytes[..n].copy_from_slice(&pb[..n]);
    PlanId::new(id_bytes).map_err(|_| DispatchError::ProposalInvalid("plan_id 派生零 ID".into()))
}

/// proposal → CandidatePlan（**未绑定迁移形态**；v1 wire 臂与 stub 决策
/// 后端的构造器）。
///
/// 2026-09-24 C2 收口：legacy `engine::run`（facade submit 臂）退役，本
/// adapter 的**位置实参四元组** `(asset, device, point, unit)` 随之退役
///——迁移期唯一的真绑定消费者就是 run（routes[0] 注入）；剩余调用方
/// （v1 wire 臂 / stub 后端 / 测试）全部以全零绑定进入，asset/device/
/// point/unit 由 dispatch 编译期经 ResourceRegistry 解析
/// （`resolve_unbound_assets` + compiler 绑定查找）。身份仍为迁移形态
/// （input_digest/plan_id 派生伪快照、objective_digest 全零）——引擎在
/// `TrustedInputPolicy::Required` 下拒绝本形态产物；`LegacyMigration` 轨
/// 放行（typed_chain 差分锁的负例正是该语义）。
pub fn proposal_to_candidate_unbound(
    p: &ProposedControlPlan,
    catalog_version: u64,
) -> Result<CandidatePlan, DispatchError> {
    // solve_status 门（R16-Q，蓝图 75 WP-75.1.3：收编入 solve_status_kind
    // 单源——原此处内联重写同一 match；plan_id_of 的同型收编先例见
    // 下文 :259 注释）。双保险语义保持：上游已 gate，adapter 不放行
    // 非成功状态（错误词形单源自动同源）。
    let status = solve_status_kind(p)?;
    // 2026-09-24 复查修复：legacy `validate_proposal` 的两道拒绝防线在
    // 本咽喉延续（v1 wire 臂 / ai_worker fallback / EMA 迁移轨共同入口，
    // 输入域宽于上游已滤除的生产 coordinator）。错误词形与 HEAD
    // validate_proposal 逐字一致。power_kw 非有限/超上限由下游既有防线
    // 承担（compiler `!value.is_finite()` → ValueOutOfRange；引擎逐意图
    // max_power_kw 检查），不在此重复。
    if p.ttl_secs == 0 {
        return Err(DispatchError::ProposalInvalid(
            "ttl_secs 为 0（防零有效期）".into(),
        ));
    }
    if p.schedule.schedule.is_empty() {
        return Err(DispatchError::ProposalInvalid("空调度不可投递".into()));
    }
    // plan_id → PlanId（R14-N：收编入 plan_id_of 单源——原此处内联重写
    // 同 6 行派生，UTF-8 前 16B 零填充共两份）。
    let plan_id = plan_id_of(p)?;
    // input_digest → SnapshotId（前 16B = 真实 provenance 绑定；迁移期
    // None（测试/旧后端）→ plan_id 派生伪快照身份——确定性、可复算）。
    let digest = p.input_digest.unwrap_or_else(|| {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        h.update(b"eneros.legacy-plan-snapshot.v1");
        h.update(p.plan_id.as_bytes());
        h.finalize().into()
    });
    let mut sid = [0u8; 16];
    sid.copy_from_slice(&digest[..16]);
    // 蓝图 65 WP-65.2：命名字段 Parts 构造（builder 退役）——objective_
    // value/ttl 槽位在字段名上自证（槽位断言见 dispatch 集成测试）。
    CandidatePlan::from_parts(CandidatePlanParts {
        id: plan_id,
        snapshot_id: SnapshotId(sid),
        objective_digest: [0u8; 32],
        resource_catalog_version: catalog_version,
        horizon_start_ms: p.issued_at_ms,
        step_ns: 0, // 单时段
        step_count: 1,
        solver_status: status,
        objective_value: p.schedule.objective_value,
        relative_gap: 0.0,
        max_primal_residual: 0.0,
        elapsed_ms: 0,
        model_digest: p.model_digest.unwrap_or([0u8; 32]),
        input_digest: digest,
        assignments: vec![Assignment {
            asset_id: 0,
            device_id: 0,
            point_id: 0,
            power_kw: p.power_kw,
            unit_code: 0,
            step_index: 1,
        }],
        issued_at_ms: p.issued_at_ms,
        ttl_ms: u64::from(p.ttl_secs.max(1)) * 1000,
    })
    .map_err(|e| DispatchError::ProposalInvalid(alloc::format!("candidate: {e:?}")))
}
/// dispatch profile 背书的资源注册表只读视图（[06] §3.3 CommandBatch
/// 构造参数；.35 卷 resource-registry 落地后生产装配注入 PlannerView，
/// 本视图降为内部测试 DTO——不再有任何生产构造点）。
#[cfg(test)]
pub(crate) struct ProfileResourceView<'a> {
    pub(crate) profile: &'a eneros_control_gateway::profile::ControlProfile,
    pub(crate) catalog_version: u64,
}

#[cfg(test)]
impl eneros_decision_contract::ResourceRegistry for ProfileResourceView<'_> {
    fn asset_exists(&self, asset_id: u64) -> bool {
        asset_id == u64::from(self.profile.device_id)
    }

    fn asset_control_binding(&self, asset_id: u64) -> Option<(u16, u32, u16)> {
        if asset_id == u64::from(self.profile.device_id) {
            Some((
                u16::try_from(self.profile.device_id).unwrap_or(0),
                self.profile.point_id,
                eneros_telemetry_model::unit_catalog::code_of(self.profile.unit),
            ))
        } else {
            None
        }
    }

    fn catalog_version(&self) -> u64 {
        self.catalog_version
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use eneros_decision_contract::ResourceRegistry;

    use super::*;

    /// R16-Q 源扫描守卫（蓝图 75 WP-75.1.3）：solve-status 门单源 =
    /// `solve_status_kind`——adapter.rs 内 Optimal/Suboptimal 放行
    /// match 臂至多 1 处（单源函数本臂；bound 形态走函数调用、unbound
    /// 形态同律）。针运行期拼接（防守卫自咬——doc 不得复现完整针形）。
    /// 变异验证：unbound 臂回填内联 match ⇒ 本测红（臂计数 2）。
    #[test]
    fn solve_status_gate_is_single_sourced() {
        let src = include_str!("adapter.rs");
        assert!(
            src.matches(concat!("SolveStatus::", "Optimal =>")).count() <= 1,
            "solve-status 门必须经 solve_status_kind 单源（内联 match 回潮）"
        );
    }

    fn proposal() -> ProposedControlPlan {
        ProposedControlPlan::single_entry(
            String::from("heur-1"),
            eneros_dual_brain::DecisionPath::Fast,
            -50.0,
            0.45,
            eneros_energy_lp_model::result::SolveStatus::Suboptimal,
            60,
            2_000,
            Some([0xAB; 32]),
        )
    }

    #[test]
    fn adapter_preserves_identity_and_status() {
        let p = proposal();
        let c = proposal_to_candidate_unbound(&p, 1).unwrap();
        // 身份：PlanId = correlation 同派生。
        let mut expected_id = [0u8; 16];
        expected_id[..6].copy_from_slice(b"heur-1");
        assert_eq!(c.id().0, expected_id);
        // 快照绑定 = input_digest 前 16B。
        assert_eq!(c.snapshot_id().0, [0xABu8; 16]);
        // status 保留。
        assert_eq!(c.solver_status(), SolveStatusKind::Suboptimal);
        // 分配保留功率；绑定全零（C2 收口——asset/device/point/unit 由
        // dispatch 编译期经 ResourceRegistry 解析，不再经位置实参注入）。
        assert_eq!(c.assignments().len(), 1);
        assert_eq!(c.assignments()[0].power_kw, -50.0);
        assert_eq!(c.assignments()[0].asset_id, 0, "未绑定形态");
        assert_eq!(c.resource_catalog_version(), 1, "目录版本透传");
        // ttl 保留（60s → 60000ms）。
        assert_eq!(c.ttl_ms(), 60_000);
    }

    #[test]
    fn adapter_rejects_non_success_status() {
        let mut p = proposal();
        p.schedule.solve_status = eneros_energy_lp_model::result::SolveStatus::Timeout;
        assert!(proposal_to_candidate_unbound(&p, 1).is_err());
    }

    /// C2 复查修复（2026-09-24）：legacy `validate_proposal` 随 facade 退役，
    /// 但其两道**拒绝**防线（ttl_secs=0 / 空调度）在 adapter 侧必须延续
    ///——`proposal_to_candidate_unbound` 是 v1 wire 臂/ai_worker fallback/
    /// EMA 迁移轨的共同咽喉，输入域宽于上游已滤除的生产 coordinator。
    /// 旧实现把 ttl=0 经 `max(1)` 静默收编为 1s、空调度计划照常进链——
    /// 均为 HEAD 语义收缩。错误词形与 HEAD validate_proposal 逐字一致。
    #[test]
    fn adapter_rejects_zero_ttl_and_empty_schedule() {
        // ttl_secs = 0 → 拒绝（防零有效期；不得静默收编为 1s）。
        let mut p = proposal();
        p.ttl_secs = 0;
        assert_eq!(
            proposal_to_candidate_unbound(&p, 1),
            Err(DispatchError::ProposalInvalid(alloc::string::String::from(
                "ttl_secs 为 0（防零有效期）"
            )))
        );
        // 空调度 → 拒绝（空调度不可投递）。
        let mut p = proposal();
        p.schedule.schedule = alloc::vec::Vec::new();
        assert_eq!(
            proposal_to_candidate_unbound(&p, 1),
            Err(DispatchError::ProposalInvalid(alloc::string::String::from(
                "空调度不可投递"
            )))
        );
    }

    #[test]
    fn profile_view_scopes_asset() {
        let profile =
            eneros_control_gateway::ControlProfile::ai_pcs_setpoint(1, 10, 4, [7u8; 16], 1);
        let view = ProfileResourceView {
            profile: &profile,
            catalog_version: 1,
        };
        assert!(view.asset_exists(1));
        assert!(!view.asset_exists(2));
        assert_eq!(view.asset_control_binding(1), Some((1, 10, 6)));
    }
}
