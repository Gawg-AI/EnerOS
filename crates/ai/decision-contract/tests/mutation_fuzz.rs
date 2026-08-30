//! 蓝图 61 v0.314.29 §4.5.2：决策产物解码器确定性 mutation fuzz。

use eneros_decision_contract::{Assignment, CandidatePlan, PlanId, SnapshotId, SolveStatusKind};

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

fn intent() -> eneros_decision_contract::CommandIntent {
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
