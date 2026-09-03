//! CandidatePlan 不能传给执行：执行入口只接受 AuthorizedCommand
//! （telemetry-model::command）；候选类型没有 device write 语义——
//! 编译期封阶段（[17] .28 行"编译期封阶段"）。
use eneros_decision_contract::{Assignment, CandidatePlan, PlanId, SnapshotId, SolveStatusKind};
use eneros_telemetry_model::command::AuthorizedCommand;

fn main() {
    let plan = CandidatePlan::builder(
        PlanId::new([1; 16]).unwrap(),
        SnapshotId([2; 16]),
        [0; 32],
        1,
    )
    .solve(SolveStatusKind::Optimal, 0.0, 0.0, 0.0, 0)
    .horizons(0, 0, 1)
    .digests([3; 32], [4; 32])
    .validity(0, 1)
    .assignments(vec![Assignment {
        asset_id: 1,
        device_id: 1,
        point_id: 1,
        power_kw: 1.0,
        unit_code: 6,
        step_index: 1,
    }])
    .build()
    .unwrap();
    // 期望失败：CandidatePlan 与 AuthorizedCommand 是不可互换类型。
    let _exec: &AuthorizedCommand = &plan;
}
