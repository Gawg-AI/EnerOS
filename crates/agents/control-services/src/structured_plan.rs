//! 蓝图 70.10 WP-70.10.2——**结构化计划 → 候选**装配。
//!
//! # 责任边界
//!
//! host（planner-peer）经 v2 通道返回**全量分配**；本模块把它变成执行链
//! 能消费的 `CandidatePlan`，并保证：
//!
//! 1. **绑定以请求为准**：应答回带的 `device_id/point_id/unit_code` 不被
//!    采信——分配的设备/点/单位一律取自目标侧在途请求的规格（peer 是不
//!    受信输入，"echo 回来的绑定"不是证据）。
//! 2. **资产集合闭合**：应答分配必须落在请求的资产集合内（未知资产 =
//!    协议违规，整批拒绝），不得凭空新增可控对象。
//! 3. **身份同代**：快照/目录/时域/目标摘要由 [`CandidateInputs`] 单点
//!    派生（与 dispatch 入口的准入公式同一函数）——生成者无法自造身份。
//! 4. 步序/条数/数值在契约层已校验（`validate_response_v2`）；本模块只做
//!    绑定归一与装配，不重复放宽。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_control_dispatch::adapter::CandidateInputs;
use eneros_control_dispatch::DispatchError;
use eneros_decision_contract::{Assignment, CandidatePlan, CandidatePlanParts, PlanId, SnapshotId};
use eneros_intent_contract::peer::{PeerAssetSpec, PeerPlanV2};

/// 结构化计划 → 候选（生产装配点；`bindings` = 在途请求的资产规格）。
pub fn candidate_from_structured_plan(
    plan: &PeerPlanV2,
    inputs: &CandidateInputs,
    bindings: &[PeerAssetSpec],
) -> Result<CandidatePlan, String> {
    inputs
        .validate()
        .map_err(|e: DispatchError| format!("{e:?}"))?;
    if bindings.is_empty() {
        return Err(String::from("无请求侧资产规格（不可装配绑定）"));
    }
    let status = match plan.solver_status.as_str() {
        "optimal" => eneros_decision_contract::SolveStatusKind::Optimal,
        "suboptimal" => eneros_decision_contract::SolveStatusKind::Suboptimal,
        other => return Err(format!("求解状态不在闭集: {other}")),
    };
    let mut assignments: Vec<Assignment> = Vec::with_capacity(plan.assignments.len());
    for a in &plan.assignments {
        let spec = bindings
            .iter()
            .find(|s| s.asset_id == a.asset_id)
            .ok_or_else(|| format!("分配资产 {} 不在请求集合内", a.asset_id))?;
        assignments.push(Assignment {
            asset_id: a.asset_id,
            // 绑定字段**取自请求**（不受信应答不回传权威）。
            device_id: spec.device_id,
            point_id: spec.point_id,
            unit_code: spec.unit_code,
            power_kw: a.power_kw,
            step_index: a.step_index,
        });
    }
    // plan id：由请求身份 + 首条分配派生（同一输入同一 id——重放不产生
    // 新计划身份；幂等键仍由 dispatch 的 correlation 链承载）。
    let mut id_bytes = [0u8; 16];
    id_bytes[..8].copy_from_slice(&plan.horizon_start_ms.to_le_bytes());
    id_bytes[8..12].copy_from_slice(&plan.step_count.to_le_bytes());
    id_bytes[12..16].copy_from_slice(&(plan.assignments.len() as u32).to_le_bytes());
    let plan_id = PlanId::new(id_bytes).map_err(|_| String::from("plan id 派生零 ID"))?;
    let model_digest = decode_digest(&plan.model_digest_hex)?;
    let input_digest = decode_digest(&plan.input_digest_hex)?;
    CandidatePlan::from_parts(CandidatePlanParts {
        id: plan_id,
        snapshot_id: SnapshotId(inputs.snapshot_id),
        objective_digest: inputs.objective_digest,
        resource_catalog_version: inputs.catalog_version,
        horizon_start_ms: inputs.horizon_start_ms,
        step_ns: inputs.step_ns,
        step_count: inputs.step_count,
        solver_status: status,
        objective_value: plan.objective_value,
        relative_gap: plan.relative_gap,
        max_primal_residual: 0.0,
        elapsed_ms: plan.elapsed_ms,
        model_digest,
        input_digest,
        assignments,
        issued_at_ms: plan.horizon_start_ms,
        ttl_ms: u64::from(plan.ttl_secs.max(1)) * 1_000,
    })
    .map_err(|e| format!("candidate: {e:?}"))
}

/// 64 小写 hex → 32B（契约层已校验形态；此处只做长度复核）。
fn decode_digest(hex: &str) -> Result<[u8; 32], String> {
    let bytes = eneros_hex::decode_vec(hex).ok_or_else(|| String::from("digest 非 hex"))?;
    if bytes.len() != 32 {
        return Err(format!("digest {}B ≠ 32B", bytes.len()));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use eneros_intent_contract::peer::{hex16, hex32, PeerAssignment};

    use super::*;

    fn spec(asset_id: u64, device: u16, point: u32) -> PeerAssetSpec {
        PeerAssetSpec {
            asset_id,
            capacity_kwh: 100.0,
            eta_c: 0.95,
            eta_d: 0.95,
            initial_kwh: 60.0,
            soc_min: 0.05,
            soc_max: 0.95,
            p_min_kw: -500.0,
            p_max_kw: 500.0,
            ramp_up_kw_s: 3.0,
            ramp_down_kw_s: 3.0,
            device_id: device,
            point_id: point,
            unit_code: 6,
        }
    }

    fn plan(steps: u32) -> PeerPlanV2 {
        PeerPlanV2 {
            snapshot_id_hex: hex16(&[0x5A; 16]),
            catalog_version: 3,
            horizon_start_ms: 1_700_000_000_000,
            step_ns: 60_000_000_000,
            step_count: steps,
            solver_status: String::from("optimal"),
            model_digest_hex: hex32(&[0x11; 32]),
            input_digest_hex: hex32(&[0x22; 32]),
            objective_value: -3.5,
            relative_gap: 0.0,
            elapsed_ms: 9,
            assignments: (1..=steps)
                .map(|s| PeerAssignment {
                    asset_id: 1,
                    // 应答回带的绑定故意写错（不受信）：必须被请求侧覆盖。
                    device_id: 999,
                    point_id: 999,
                    unit_code: 999,
                    step_index: s,
                    power_kw: -2.5,
                })
                .collect(),
            target_device: String::from("pcs"),
            ttl_secs: 60,
        }
    }

    fn inputs() -> CandidateInputs {
        CandidateInputs::from_snapshot([0x5A; 16], 3, 1_700_000_000_000, 60_000_000_000, 4)
    }

    /// 正例：全量分配装配；**绑定字段取自请求**（不受信应答的 echo 被覆盖）。
    #[test]
    fn structured_plan_binds_from_request_not_from_response() {
        let cand =
            candidate_from_structured_plan(&plan(4), &inputs(), &[spec(1, 2, 10)]).expect("装配");
        assert_eq!(cand.assignments().len(), 4);
        assert_eq!(cand.snapshot_id().0, [0x5A; 16]);
        assert_eq!(cand.resource_catalog_version(), 3);
        assert_eq!(cand.step_count(), 4);
        assert_eq!(
            cand.solver_status(),
            eneros_decision_contract::SolveStatusKind::Optimal
        );
        for a in cand.assignments() {
            assert_eq!(
                a.device_id, 2,
                "设备绑定必须取自请求（应答回带 999 不可信）"
            );
            assert_eq!(a.point_id, 10);
            assert_eq!(a.unit_code, 6);
        }
        assert_eq!(cand.model_digest(), &[0x11; 32]);
        assert_eq!(cand.input_digest(), &[0x22; 32]);
        // 目标摘要 = 生成者/验证者同一公式（dispatch 入口重算即判同代）。
        assert_eq!(
            cand.objective_digest(),
            &eneros_decision_contract::objective_binding_v1(
                &[0x5A; 16],
                3,
                1_700_000_000_000,
                60_000_000_000,
                4
            )
        );
    }

    /// 负例：分配落在请求资产集合之外 → 整批拒绝（不得凭空新增可控对象）。
    #[test]
    fn structured_plan_rejects_unknown_asset() {
        let mut p = plan(2);
        p.assignments[1].asset_id = 77;
        match candidate_from_structured_plan(&p, &inputs(), &[spec(1, 2, 10)]) {
            Err(ref s) => assert!(s.contains("不在请求集合内"), "{s}"),
            Ok(_) => panic!("未知资产必须拒绝"),
        }
    }

    /// 负例：无请求侧规格（不可装配绑定）与状态闭集。
    #[test]
    fn structured_plan_rejects_missing_bindings_and_bad_status() {
        assert!(candidate_from_structured_plan(&plan(2), &inputs(), &[]).is_err());
        let mut bad = plan(2);
        bad.solver_status = String::from("infeasible");
        assert!(candidate_from_structured_plan(&bad, &inputs(), &[spec(1, 2, 10)]).is_err());
    }
}
