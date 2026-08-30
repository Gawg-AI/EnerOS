//! 蓝图 61 v0.314.28：旧 proposal → CandidatePlan 迁移 adapter（[06]
//! §3.5.3"严格保留 status/digest"）+ dispatch 侧资源注册表只读视图。
//!
//! 边界纪律：proposal（旧形态）在 dispatch 边界经本 adapter 进入
//! CandidatePlan——链内（validate/authorize/persist/deliver）只消费
//! typed 产物，不再读裸 proposal 字段。facade reader 保留至 .32 删除
//! （[17] .28/.32 行）。

use alloc::vec;

use eneros_decision_contract::{
    Assignment, CandidatePlan, PlanId, ResourceRegistry, SnapshotId, SolveStatusKind,
};
use eneros_dual_brain::decision::ProposedControlPlan;

use crate::DispatchError;

/// proposal → CandidatePlan（严格迁移；任何丢失/变形即拒绝）。
///
/// 身份派生（迁移窗口确定性规则，ADR-0048）：
/// - `PlanId` = UTF-8(plan_id) 前 16B 零填充——与 `correlation_of` 同派生
///   （Phase 0 幂等/journal 链逐字节不变）；
/// - `SnapshotId` = input_digest 前 16B（provenance 即快照绑定）；
/// - `objective_digest` = 全零（迁移期无 ObjectiveSpec——.36 卷接入）；
/// - 分配 = 单 Assignment（asset 绑定来自 dispatch profile——非硬编码）。
pub fn proposal_to_candidate(
    p: &ProposedControlPlan,
    asset_binding: (u64, u16, u32, u16),
    unit_code: u16,
    catalog_version: u64,
) -> Result<CandidatePlan, DispatchError> {
    // solve_status 门（双保险——上游已 gate；adapter 不放行非成功状态）。
    let status = match p.schedule.solve_status {
        eneros_energy_lp_model::result::SolveStatus::Optimal => SolveStatusKind::Optimal,
        eneros_energy_lp_model::result::SolveStatus::Suboptimal => SolveStatusKind::Suboptimal,
        _ => {
            return Err(DispatchError::ProposalInvalid(
                "solve_status 非成功（adapter 拒绝）".into(),
            ))
        }
    };
    // plan_id → PlanId（correlation 同派生）。
    let mut id_bytes = [0u8; 16];
    let pb = p.plan_id.as_bytes();
    let n = core::cmp::min(16, pb.len());
    id_bytes[..n].copy_from_slice(&pb[..n]);
    let plan_id = PlanId::new(id_bytes)
        .map_err(|_| DispatchError::ProposalInvalid("plan_id 派生零 ID".into()))?;
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
    let (asset_id, device_id, point_id, _) = asset_binding;
    CandidatePlan::new(
        plan_id,
        SnapshotId(sid),
        [0u8; 32], // objective_digest（迁移期无 ObjectiveSpec）
        catalog_version,
        p.issued_at_ms,
        0, // step_ns（单时段）
        1, // step_count
        status,
        p.schedule.objective_value,
        0.0, // gap（heuristic 解不声明最优性界）
        0.0, // residual
        0,   // elapsed（迁移期未测——审计字段，0 = 未提供）
        p.model_digest.unwrap_or([0u8; 32]),
        digest,
        vec![Assignment {
            asset_id,
            device_id,
            point_id,
            power_kw: p.power_kw,
            unit_code,
            step_index: 1,
        }],
        p.issued_at_ms,
        u64::from(p.ttl_secs.max(1)) * 1000,
    )
    .map_err(|e| DispatchError::ProposalInvalid(alloc::format!("candidate: {e:?}")))
}

/// dispatch profile 背书的资源注册表只读视图（[06] §3.3 CommandBatch
/// 构造第二参数；.35 卷 resource-registry 落地前由 profile 单源充当）。
pub(crate) struct ProfileResourceView<'a> {
    pub(crate) profile: &'a eneros_control_gateway::profile::ControlProfile,
    pub(crate) catalog_version: u64,
}

impl ResourceRegistry for ProfileResourceView<'_> {
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

    use super::*;

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
        let c = proposal_to_candidate(&p, (1, 1, 10, 6), 6, 1).unwrap();
        // 身份：PlanId = correlation 同派生。
        let mut expected_id = [0u8; 16];
        expected_id[..6].copy_from_slice(b"heur-1");
        assert_eq!(c.id().0, expected_id);
        // 快照绑定 = input_digest 前 16B。
        assert_eq!(c.snapshot_id().0, [0xABu8; 16]);
        // status 保留。
        assert_eq!(c.solver_status(), SolveStatusKind::Suboptimal);
        // 分配保留功率与绑定。
        assert_eq!(c.assignments().len(), 1);
        assert_eq!(c.assignments()[0].power_kw, -50.0);
        assert_eq!(c.assignments()[0].point_id, 10);
        // ttl 保留（60s → 60000ms）。
        assert_eq!(c.ttl_ms(), 60_000);
    }

    #[test]
    fn adapter_rejects_non_success_status() {
        let mut p = proposal();
        p.schedule.solve_status = eneros_energy_lp_model::result::SolveStatus::Timeout;
        assert!(proposal_to_candidate(&p, (1, 1, 10, 6), 6, 1).is_err());
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
