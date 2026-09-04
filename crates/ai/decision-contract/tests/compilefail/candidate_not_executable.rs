//! CandidatePlan 不能传给执行：执行入口只接受 AuthorizedCommand
//! （telemetry-model::command）；候选类型没有 device write 语义——
//! 编译期封阶段（[17] .28 行"编译期封阶段"）。
use eneros_decision_contract::{
    Assignment, CandidatePlan, CandidatePlanParts, PlanId, SnapshotId, SolveStatusKind,
};
use eneros_telemetry_model::command::AuthorizedCommand;

fn main() {
    let plan = CandidatePlan::from_parts(CandidatePlanParts {
        id: PlanId::new([1; 16]).unwrap(),
        snapshot_id: SnapshotId([2; 16]),
        objective_digest: [0; 32],
        resource_catalog_version: 1,
        horizon_start_ms: 0,
        step_ns: 0,
        step_count: 1,
        solver_status: SolveStatusKind::Optimal,
        objective_value: 0.0,
        relative_gap: 0.0,
        max_primal_residual: 0.0,
        elapsed_ms: 0,
        model_digest: [3; 32],
        input_digest: [4; 32],
        assignments: vec![Assignment {
            asset_id: 1,
            device_id: 1,
            point_id: 1,
            power_kw: 1.0,
            unit_code: 6,
            step_index: 1,
        }],
        issued_at_ms: 0,
        ttl_ms: 1,
    })
    .unwrap();
    // 期望失败：CandidatePlan 与 AuthorizedCommand 是不可互换类型。
    let _exec: &AuthorizedCommand = &plan;
}
