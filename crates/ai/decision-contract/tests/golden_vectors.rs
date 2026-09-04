//! 蓝图 61 v0.314.29 §4.5.1：决策产物 golden vectors。

use eneros_decision_contract::{
    Assignment, CandidatePlan, CandidatePlanParts, CommandIntentParts, PlanId, SnapshotId,
    SolveStatusKind,
};

const VEC_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../tests/vectors/blueprint61"
);

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|x| format!("{:02x}", x)).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn read(name: &str) -> String {
    std::fs::read_to_string(format!("{VEC_DIR}/{name}"))
        .expect("golden file")
        .trim()
        .to_string()
}

fn canonical_plan() -> CandidatePlan {
    CandidatePlan::from_parts(CandidatePlanParts {
        id: PlanId::new([1; 16]).unwrap(),
        snapshot_id: SnapshotId([2; 16]),
        objective_digest: [3; 32],
        resource_catalog_version: 1,
        horizon_start_ms: 1_700_000_000_000,
        step_ns: 1_000,
        step_count: 1,
        solver_status: SolveStatusKind::Suboptimal,
        objective_value: 12.5,
        relative_gap: 0.01,
        max_primal_residual: 0.001,
        elapsed_ms: 30,
        model_digest: [4; 32],
        input_digest: [5; 32],
        assignments: vec![Assignment {
            asset_id: 1,
            device_id: 1,
            point_id: 10,
            power_kw: -50.0,
            unit_code: 6,
            step_index: 1,
        }],
        issued_at_ms: 1_700_000_001_000,
        ttl_ms: 60_000,
    })
    .unwrap()
}

fn canonical_intent() -> eneros_decision_contract::CommandIntent {
    eneros_decision_contract::CommandIntent::from_parts(CommandIntentParts {
        intent_id: eneros_decision_contract::IntentId([3; 16]),
        plan_id: PlanId([1; 16]),
        plan_sha256: [0xAB; 32],
        asset_id: 1,
        device_id: 1,
        point_id: 10,
        power_kw: -50.0,
        unit_code: 6,
        valid_from_ms: 1_700_000_000_000,
        valid_until_ms: 1_700_000_060_000,
        precondition: eneros_decision_contract::Precondition::None,
        compensation: eneros_decision_contract::CompensationHint::None,
    })
    .unwrap()
}

#[cfg(feature = "transition-certifier")]
fn canonical_cert() -> eneros_decision_contract::CertifiedPlan {
    eneros_decision_contract::CertifiedPlan::decode_v1(&unhex(&read("certified-plan-v1.golden")))
        .expect("cert golden decode")
}

#[test]
fn candidate_plan_v1_golden() {
    let p = canonical_plan();
    assert_eq!(hex(&p.encode_v1()), read("candidate-plan-v1.golden"));
    assert_eq!(
        CandidatePlan::decode_v1(&unhex(&read("candidate-plan-v1.golden"))).unwrap(),
        p
    );
}

#[test]
fn command_intent_v1_golden() {
    let i = canonical_intent();
    assert_eq!(hex(&i.encode_v1()), read("command-intent-v1.golden"));
    assert_eq!(
        eneros_decision_contract::CommandIntent::decode_v1(&unhex(&read(
            "command-intent-v1.golden"
        )))
        .unwrap(),
        i
    );
}

#[test]
#[cfg(feature = "transition-certifier")]
fn certified_plan_v1_golden_and_binding() {
    let c = canonical_cert();
    // 语义锁定：证书字段与规范值一致。
    assert_eq!(c.certificate_id().0, [9; 16]);
    assert_eq!(c.plan_id().0, [1; 16]);
    assert_eq!(c.plan_sha256(), &[0xAB; 32]);
    assert_eq!(c.certifier_version(), 1);
    assert!(c.is_valid_at(1_700_000_059_000));
    // 绑定：证书 ↔ (plan_id, plan_sha256)。
    assert!(c.verify_binding(c.plan_id(), c.plan_sha256()));
    assert!(!c.verify_binding(c.plan_id(), &[0; 32]));
}
