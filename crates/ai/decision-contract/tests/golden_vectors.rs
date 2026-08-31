//! 蓝图 61 v0.314.29 §4.5.1：决策产物 golden vectors。

use eneros_decision_contract::{Assignment, CandidatePlan, PlanId, SnapshotId, SolveStatusKind};

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
    CandidatePlan::new(
        PlanId::new([1; 16]).unwrap(),
        SnapshotId([2; 16]),
        [3; 32],
        1,
        1_700_000_000_000,
        1_000,
        1,
        SolveStatusKind::Suboptimal,
        12.5,
        0.01,
        0.001,
        30,
        [4; 32],
        [5; 32],
        vec![Assignment {
            asset_id: 1,
            device_id: 1,
            point_id: 10,
            power_kw: -50.0,
            unit_code: 6,
            step_index: 1,
        }],
        1_700_000_001_000,
        60_000,
    )
    .unwrap()
}

fn canonical_intent() -> eneros_decision_contract::CommandIntent {
    eneros_decision_contract::CommandIntent::new(
        eneros_decision_contract::IntentId([3; 16]),
        PlanId([1; 16]),
        [0xAB; 32],
        1,
        1,
        10,
        -50.0,
        6,
        1_700_000_000_000,
        1_700_000_060_000,
        eneros_decision_contract::Precondition::None,
        eneros_decision_contract::CompensationHint::None,
    )
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
