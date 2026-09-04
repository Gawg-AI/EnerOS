//! 蓝图 61 v0.314.29 §4.5.2：决策产物解码器确定性 mutation fuzz。

use eneros_decision_contract::{
    Assignment, CandidatePlan, CandidatePlanParts, CommandIntentParts, PlanId, SnapshotId,
    SolveStatusKind,
};

struct Xorshift(u64);

impl Xorshift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

fn mutate(seed: u64, base: &[u8], rounds: usize, f: &mut dyn FnMut(&[u8])) {
    let mut rng = Xorshift(seed);
    for _ in 0..rounds {
        let mut buf = base.to_vec();
        let n = 1 + (rng.next() as usize) % 8;
        for _ in 0..n {
            let idx = (rng.next() as usize) % buf.len();
            buf[idx] = (rng.next() & 0xFF) as u8;
        }
        match rng.next() % 8 {
            0..=2 => {
                let keep = 1 + (rng.next() as usize) % buf.len();
                buf.truncate(keep);
            }
            3 => buf.extend_from_slice(&vec![0u8; 8192]),
            _ => {}
        }
        f(&buf);
    }
}

fn plan() -> CandidatePlan {
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

fn intent() -> eneros_decision_contract::CommandIntent {
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

#[test]
fn candidate_decode_never_panics() {
    let base = plan().encode_v1();
    mutate(0xC0DE0001, &base, 50_000, &mut |buf| {
        let _ = CandidatePlan::decode_v1(buf);
    });
}

#[test]
fn intent_decode_never_panics() {
    let base = intent().encode_v1();
    mutate(0xC1DE0002, &base, 50_000, &mut |buf| {
        let _ = eneros_decision_contract::CommandIntent::decode_v1(buf);
    });
}

#[test]
fn cert_decode_never_panics() {
    let base: Vec<u8> = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/vectors/blueprint61/certified-plan-v1.golden"
    ))
    .expect("golden")
    .trim()
    .as_bytes()
    .chunks(2)
    .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
    .collect();
    mutate(0xCEDE0003, &base, 50_000, &mut |buf| {
        let _ = eneros_decision_contract::CertifiedPlan::decode_v1(buf);
    });
}
