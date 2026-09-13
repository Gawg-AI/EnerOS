//! certifier 集成测试（[09] §4.5/§4.6/§4.7）：
//! 1. malicious CandidatePlan 集合先行（每类约束至少一个违约样本）；
//! 2. 独立评估 + 证据记录；
//! 3. 手算差分；
//! 4. 证书构造权 + Gateway 验证（key id/版本/有效期/plan digest）；
//! 5. shadow 认证 HiGHS 全用例（对比 solver 残差与独立残差）；
//! 6. 变异测试（删/改每类硬约束必须被拦）；
//! 7. key 生命周期与回滚；1000 资源 × 96 步性能。

#![allow(clippy::disallowed_macros)]

use std::collections::BTreeMap;
use std::vec;
use std::vec::Vec;

use eneros_certifier::proof::CertifierKeyRing;
use eneros_certifier::{CertifierError, CertifierModule, EvalBattery, EvalContext};
use eneros_decision_contract::objective::{
    Constraint, ConstraintKind, ObjectiveKind, ObjectiveSource,
};
use eneros_decision_contract::{
    Assignment, CandidatePlan, CandidatePlanParts, ObjectiveSpec, PlanId, ReasonCode, SnapshotId,
    SolveStatusKind,
};
use eneros_resource_registry::capability::{Availability, CapabilityEnvelope, DeratingReason};
use eneros_snapshot_service::TrustedSnapshot;

const STEP_NS_HOUR: u64 = 3_600_000_000_000;

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

fn snapshot() -> TrustedSnapshot {
    TrustedSnapshot {
        snapshot_id: [7; 16],
        tenant_id: 1,
        site_id: 1,
        cut_time_ns: 1_000_000_000_000,
        produced_at_ns: 1_000_000_000_000,
        policy_version: 1,
        version_set: eneros_semantic_catalog::VersionSet::ZERO,
        observations: vec![],
        source_watermarks: vec![],
        completeness: eneros_snapshot_service::CompletenessReport {
            required: vec![],
            accepted: vec![],
            missing: vec![],
            stale: vec![],
            bad: vec![],
            conflict: vec![],
            substituted: vec![],
        },
        conflicts: vec![],
        derived: vec![],
        provenance_root: [3; 32],
    }
}

fn env_battery() -> CapabilityEnvelope {
    CapabilityEnvelope {
        asset_id: 1,
        valid_from_ns: 0,
        valid_until_ns: u64::MAX,
        p_min_kw: -5.0,
        p_max_kw: 5.0,
        q_min_kvar: 0.0,
        q_max_kvar: 0.0,
        energy_min_kwh: 0.0,
        energy_max_kwh: 10.0,
        ramp_up_kw_s: 1000.0,
        ramp_down_kw_s: 1000.0,
        min_on_ns: 0,
        min_off_ns: 0,
        availability: Availability::Full,
        derating_reason: DeratingReason::None,
        source_snapshot_id: [1; 16],
        tightenings: vec![],
        generation: 1,
    }
}

fn env_grid() -> CapabilityEnvelope {
    CapabilityEnvelope {
        asset_id: 10,
        p_min_kw: -20.0,
        p_max_kw: 20.0,
        ..env_battery()
    }
}

fn battery() -> EvalBattery {
    EvalBattery {
        asset_id: 1,
        capacity_kwh: 10.0,
        eta_c: 1.0,
        eta_d: 1.0,
        initial_kwh: 5.0,
    }
}

fn spec(hard: Vec<Constraint>) -> ObjectiveSpec {
    ObjectiveSpec::builder(SnapshotId([7; 16]))
        .constraints(hard, vec![])
        .objective(
            ObjectiveKind::MaxRevenue,
            1,
            ObjectiveSource::RuleBundle,
            ReasonCode::PriceArbitrage,
        )
        .window(0, u64::MAX)
        .build()
        .unwrap()
}

fn base_spec() -> ObjectiveSpec {
    spec(vec![
        Constraint {
            kind: ConstraintKind::MinSoc,
            bound: 0.1,
        },
        Constraint {
            kind: ConstraintKind::MaxSoc,
            bound: 0.9,
        },
    ])
}

fn ctx<'a>(
    snapshot: &'a TrustedSnapshot,
    spec: &'a ObjectiveSpec,
    envelopes: &'a [(u64, CapabilityEnvelope)],
    batteries: &'a [EvalBattery],
) -> EvalContext<'a> {
    EvalContext {
        snapshot,
        spec,
        envelopes,
        batteries,
        horizon_step_ns: STEP_NS_HOUR,
        horizon_step_count: 2,
        horizon_start_ms: 1_000,
        grid_asset_id: 10,
        tolerance_profile_id: 1,
        constraint_catalog_version: 1,
    }
}

/// 手工计划构造（合法轨迹；WP-64.3.1 起违约轨迹在构造点即拒——
/// certifier 的 duplicate/MissingAssignment 独立复检分支保留为纵深，
/// 公开构造面不可达，见本文件 malicious_nan_and_missing_rejected）。
fn plan(assignments: Vec<Assignment>) -> CandidatePlan {
    plan_raw(assignments).expect("合法计划")
}

/// 负例构造基座（判据现居构造点——from_parts 唯一验证通道）。
fn plan_raw(
    assignments: Vec<Assignment>,
) -> Result<CandidatePlan, eneros_decision_contract::ContractError> {
    CandidatePlan::from_parts(CandidatePlanParts {
        id: PlanId::new([1; 16]).unwrap(),
        snapshot_id: SnapshotId([7; 16]),
        objective_digest: [2; 32],
        resource_catalog_version: 1,
        horizon_start_ms: 1_000,
        step_ns: STEP_NS_HOUR,
        step_count: 2,
        solver_status: SolveStatusKind::Optimal,
        objective_value: 0.0,
        relative_gap: 0.0,
        max_primal_residual: 0.0,
        elapsed_ms: 10,
        model_digest: [3; 32],
        input_digest: [4; 32],
        assignments,
        issued_at_ms: 1_000,
        ttl_ms: 60_000,
    })
}

fn asg(asset: u64, step: u32, kw: f64) -> Assignment {
    Assignment {
        asset_id: asset,
        device_id: 2,
        point_id: 6,
        power_kw: kw,
        unit_code: 6,
        step_index: step,
    }
}

/// 合法基线轨迹：t0 充 1（buy 1），t1 放 5（sell 5）。
/// SOC: 5 → 6 → 1 ✓（窗 [1,9]）。
fn valid_assignments() -> Vec<Assignment> {
    vec![
        asg(1, 1, -1.0),
        asg(1, 2, 5.0),
        asg(10, 1, 1.0),
        asg(10, 2, -5.0),
    ]
}

fn certify(
    assignments: Vec<Assignment>,
) -> Result<eneros_certifier::CertifiedOutcome, CertifierError> {
    certify_with(assignments, &CertifierKeyRing::new(1, b"station-cert-key"))
}

fn certify_with(
    assignments: Vec<Assignment>,
    ring: &CertifierKeyRing,
) -> Result<eneros_certifier::CertifiedOutcome, CertifierError> {
    let snap = snapshot();
    let sp = base_spec();
    let envelopes = vec![(1, env_battery()), (10, env_grid())];
    let batteries = vec![battery()];
    let context = ctx(&snap, &sp, &envelopes, &batteries);
    let p = plan(assignments);
    CertifierModule::certify(&context, &p, 2_000, ring)
}

// ---------------------------------------------------------------------------
// 1-2. 正例 + 证据链
// ---------------------------------------------------------------------------

#[test]
fn certify_valid_plan_with_proofs() {
    let out = certify(valid_assignments()).expect("certify");
    assert!(out.certificate.hard_constraint_count() > 0);
    assert!(out.records.iter().all(|r| r.pass));
    // 绑定：证书 ↔ plan wire。
    let p = plan(valid_assignments());
    let wire = p.encode_v1();
    let sha: [u8; 32] = {
        use sha2::Digest;
        sha2::Sha256::digest(&wire).into()
    };
    assert!(out.certificate.verify_binding(p.id(), &sha));
    assert_eq!(out.plan_sha256, sha);
    // 证据摘要一致性。
    let redigest = eneros_certifier::proof::digest_records(
        &out.records,
        &sha,
        eneros_certifier::CERTIFIER_VERSION,
    );
    assert_eq!(redigest, out.proof_digest);
    assert_eq!(out.certificate.proof_digest(), &out.proof_digest);
}

// ---------------------------------------------------------------------------
// 3. 手算差分：SOC 推进 vs 高精度参考
// ---------------------------------------------------------------------------

#[test]
fn hand_computed_soc_progression() {
    // η=1、dt=1h：t0 充 1 → 6；t1 放 5 → 1。末态证据 lhs = 1.0。
    let out = certify(valid_assignments()).unwrap();
    let soc_records: Vec<_> = out
        .records
        .iter()
        .filter(|r| r.kind == ConstraintKind::MinSoc.code() && r.asset_id == 1)
        .collect();
    assert_eq!(soc_records.len(), 1);
    assert!(
        (soc_records[0].lhs - 1.0).abs() < 1e-12,
        "末态 {}",
        soc_records[0].lhs
    );
    // η_d=0.5：放 5 → 耗 10 → 末态 5+1−10 < 0 → MinSoc 违约。
    let batteries = vec![EvalBattery {
        eta_d: 0.5,
        ..battery()
    }];
    let snap = snapshot();
    let sp = base_spec();
    let envelopes = vec![(1, env_battery()), (10, env_grid())];
    let context = ctx(&snap, &sp, &envelopes, &batteries);
    let p = plan(valid_assignments());
    let ring = CertifierKeyRing::new(1, b"station-cert-key");
    match CertifierModule::certify(&context, &p, 2_000, &ring) {
        Err(CertifierError::Violated {
            constraint_kind,
            asset_id,
            ..
        }) => {
            assert_eq!(constraint_kind, ConstraintKind::MinSoc.code());
            assert_eq!(asset_id, 1);
        }
        other => panic!("expected soc violation, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// malicious 集合（[09] §4.5 步骤 1：先写违约样本）
// ---------------------------------------------------------------------------

#[test]
fn malicious_power_over_envelope_rejected() {
    let mut a = valid_assignments();
    a[0].power_kw = 5.5; // 电池 pmax 5
    match certify(a) {
        Err(CertifierError::Violated {
            constraint_kind, ..
        }) => {
            assert_eq!(constraint_kind, ConstraintKind::MaxPowerKw.code())
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn malicious_power_under_envelope_rejected() {
    let mut a = valid_assignments();
    a[1].power_kw = -6.5; // 电池 pmin −5（充电方向超限）
    match certify(a) {
        Err(CertifierError::Violated {
            constraint_kind, ..
        }) => {
            assert_eq!(constraint_kind, ConstraintKind::MinPowerKw.code())
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn malicious_soc_drain_rejected() {
    let mut a = valid_assignments();
    a[1].power_kw = 4.0; // t1 放 4：末态 5+1−4=2 ✓；改 t0 也放 → 末态更低
    a[0].power_kw = 4.0; // t0 放 4 → soc1 = 1 < 窗下限…；t1 再放 4 → 末态 −3
                         // t0: 5−4=1 ✓（边界）；t1: 1−4=−3 ✗ MinSoc。
    let mut grid = a.clone();
    grid.truncate(2);
    let a = vec![a[0], a[1], asg(10, 1, 4.0), asg(10, 2, 4.0)];
    let _ = grid;
    match certify(a) {
        Err(CertifierError::Violated {
            constraint_kind,
            asset_id,
            ..
        }) => {
            assert_eq!(constraint_kind, ConstraintKind::MinSoc.code());
            assert_eq!(asset_id, 1);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn malicious_nan_and_missing_rejected() {
    // NaN 在合同构造层即拒（CandidatePlan 校验构造器；evaluator 的
    // NonFinite 分支为 decode 侧纵深防御）。
    let r = plan_raw(vec![asg(1, 1, f64::NAN)]);
    assert_eq!(
        r,
        Err(eneros_decision_contract::ContractError::OutOfRange(
            "assignment.power_kw"
        ))
    );
    // 缺步（电池缺 t2）：WP-64.3.1 轨迹不变量构造点强制——判据自
    // certifier MissingAssignment 前移构造点（ADR-051 独立复检分支保留
    // 为纵深；公开构造面不可达，certifier 不再是唯一守门者）。
    let a = vec![asg(1, 1, -1.0), asg(10, 1, 1.0), asg(10, 2, -5.0)];
    assert_eq!(
        plan_raw(a),
        Err(eneros_decision_contract::ContractError::Malformed(
            "assignment_trajectory_gap"
        ))
    );
    // 重复分配：同上，构造点强制。
    let a = vec![asg(1, 1, 1.0), asg(1, 1, 2.0)];
    assert_eq!(
        plan_raw(a),
        Err(eneros_decision_contract::ContractError::Malformed(
            "duplicate_assignment"
        ))
    );
    // 越步：合同构造层即拒（step_index ≤ step_count）。
    let r = plan_raw(vec![asg(1, 3, 1.0)]);
    assert_eq!(
        r,
        Err(eneros_decision_contract::ContractError::OutOfRange(
            "assignment.step_index"
        ))
    );
}

#[test]
fn malicious_commitment_shortfall_rejected() {
    let hard = vec![
        Constraint {
            kind: ConstraintKind::MinSoc,
            bound: 0.1,
        },
        Constraint {
            kind: ConstraintKind::MaxSoc,
            bound: 0.9,
        },
        Constraint {
            kind: ConstraintKind::EnergyCommitmentKwh,
            bound: 4.0,
        },
    ];
    let snap = snapshot();
    let sp = spec(hard);
    let envelopes = vec![(1, env_battery()), (10, env_grid())];
    let batteries = vec![battery()];
    let context = ctx(&snap, &sp, &envelopes, &batteries);
    // 只卖 2（承诺 4）→ 违约。
    let a = vec![
        asg(1, 1, -1.0),
        asg(1, 2, 3.0),
        asg(10, 1, 1.0),
        asg(10, 2, -3.0),
    ];
    let p = plan(a);
    let ring = CertifierKeyRing::new(1, b"station-cert-key");
    match CertifierModule::certify(&context, &p, 2_000, &ring) {
        Err(CertifierError::Violated {
            constraint_kind, ..
        }) => {
            assert_eq!(constraint_kind, ConstraintKind::EnergyCommitmentKwh.code())
        }
        other => panic!("{other:?}"),
    }
    // 卖足 4 → 通过（t1 净放 4 = 售 4）。
    let a = vec![
        asg(1, 1, -1.0),
        asg(1, 2, 4.0),
        asg(10, 1, 1.0),
        asg(10, 2, -4.0),
    ];
    let p = plan(a);
    let ring = CertifierKeyRing::new(1, b"station-cert-key");
    assert!(CertifierModule::certify(&context, &p, 2_000, &ring).is_ok());
}

#[test]
fn binding_and_window_rejections() {
    // 过期。
    let out = certify(valid_assignments()).unwrap();
    let _ = out;
    let snap = snapshot();
    let sp = base_spec();
    let envelopes = vec![(1, env_battery()), (10, env_grid())];
    let batteries = vec![battery()];
    let context = ctx(&snap, &sp, &envelopes, &batteries);
    let p = plan(valid_assignments());
    // now 远超 ttl（issued 1000 + ttl 60000）。
    let ring = CertifierKeyRing::new(1, b"k");
    match CertifierModule::certify(&context, &p, 2_000_000, &ring) {
        Err(CertifierError::PlanExpired) => {}
        other => panic!("{other:?}"),
    }
    // 快照不匹配。
    let mut other_snap = snapshot();
    other_snap.snapshot_id = [9; 16];
    let context_bad = ctx(&other_snap, &sp, &envelopes, &batteries);
    match CertifierModule::certify(&context_bad, &p, 2_000, &ring) {
        Err(CertifierError::PlanMismatch("snapshot_id")) => {}
        other => panic!("{other:?}"),
    }
    // horizon 不匹配。
    let mut c2 = ctx(&snap, &sp, &envelopes, &batteries);
    c2.horizon_step_count = 3;
    match CertifierModule::certify(&c2, &p, 2_000, &ring) {
        Err(CertifierError::PlanMismatch("horizon")) => {}
        other => panic!("{other:?}"),
    }
    // WP-64.3.3.2：spec_snapshot / horizon_start 两对负例（bind 判据
    // 每对一个负例）。
    // 规格快照与计划快照不同源（spec 绑定别的快照）。
    let spec_other_snap = ObjectiveSpec::builder(SnapshotId([8; 16]))
        .constraints(sp.hard().to_vec(), vec![])
        .objective(
            ObjectiveKind::MaxRevenue,
            1,
            ObjectiveSource::RuleBundle,
            ReasonCode::PriceArbitrage,
        )
        .window(0, u64::MAX)
        .build()
        .unwrap();
    let c3 = ctx(&snap, &spec_other_snap, &envelopes, &batteries);
    match CertifierModule::certify(&c3, &p, 2_000, &ring) {
        Err(CertifierError::PlanMismatch("spec_snapshot")) => {}
        other => panic!("{other:?}"),
    }
    // horizon 起点不匹配。
    let mut c4 = ctx(&snap, &sp, &envelopes, &batteries);
    c4.horizon_start_ms = 2_000;
    match CertifierModule::certify(&c4, &p, 2_000, &ring) {
        Err(CertifierError::PlanMismatch("horizon_start")) => {}
        other => panic!("{other:?}"),
    }
}

/// WP-64.3.5 复查补：certifier 独立复检分支的负例保留——构造点强制后
/// 公开构造面不可达违约轨迹，但 ADR-051 复检分支必须保有自己的直接
/// 触达测试（evaluate_trajectories pub 面，绕开构造点注入违约 traj）。
#[test]
fn independent_recheck_trajectory_branches_still_live() {
    use std::collections::BTreeMap;
    let snap = snapshot();
    let sp = base_spec();
    let envelopes = vec![(1, env_battery()), (10, env_grid())];
    let batteries = vec![battery()];
    let context = ctx(&snap, &sp, &envelopes, &batteries);
    let ring = CertifierKeyRing::new(1, b"k");
    let p = plan(valid_assignments());

    // 缺档：asset 1 缺 step2 → MissingAssignment { asset_id: 1, step: 2 }。
    // （MalformedAssignment("duplicate") 臂经 evaluate 的 traj.entry 路径
    // 注入需违约 plan——构造点强制后公开面不可达：拒绝语义已前移至
    // 构造点 duplicate_assignment 拒绝（本文件 malicious_nan_and_missing_
    // rejected 断言），eval 分支保留为纵深，登记执行卡。）
    let mut gap: BTreeMap<u64, Vec<Option<f64>>> = BTreeMap::new();
    gap.insert(1, vec![Some(-1.0), None]);
    gap.insert(10, vec![Some(1.0), Some(-5.0)]);
    match eneros_certifier::eval::evaluate_trajectories(&gap, &context) {
        Err(CertifierError::MissingAssignment { asset_id, step }) => {
            assert_eq!(asset_id, 1);
            assert_eq!(step, 2);
        }
        other => panic!("{other:?}"),
    }
    // 良构 traj：复检通过（同 certify 正例的 evaluator 半边）。
    let mut ok: BTreeMap<u64, Vec<Option<f64>>> = BTreeMap::new();
    ok.insert(1, vec![Some(-1.0), Some(-2.0)]);
    ok.insert(10, vec![Some(1.0), Some(-5.0)]);
    let records =
        eneros_certifier::eval::evaluate_trajectories(&ok, &context).expect("良构 traj 复检通过");
    let _ = (records, &p, &ring);
}

// ---------------------------------------------------------------------------
// 6. 变异测试：逐类硬约束变异必须被独立拦截（[09] §4.7）
// ---------------------------------------------------------------------------

#[test]
fn mutation_each_hard_constraint_class_intercepted() {
    // 基线合法计划 + 逐类变异。
    let cases: Vec<(&'static str, Vec<Assignment>)> = vec![
        (
            "max power",
            vec![
                asg(1, 1, 5.5),
                asg(1, 2, 0.0),
                asg(10, 1, 5.5),
                asg(10, 2, 0.0),
            ],
        ),
        (
            "min power",
            vec![
                asg(1, 1, -5.5),
                asg(1, 2, 0.0),
                asg(10, 1, -5.5),
                asg(10, 2, 0.0),
            ],
        ),
        (
            "soc min",
            vec![
                asg(1, 1, 4.5),
                asg(1, 2, 4.5),
                asg(10, 1, 4.5),
                asg(10, 2, 4.5),
            ],
        ),
        (
            "energy window (overcharge)",
            vec![
                asg(1, 1, -5.0),
                asg(1, 2, 0.0),
                asg(10, 1, -5.0),
                asg(10, 2, 0.0),
            ],
        ),
    ];
    for (tag, mutated) in cases {
        let r = certify(mutated);
        assert!(r.is_err(), "变异 {tag} 必须被拦截");
    }
    // 终值 SOC 变异。
    let hard = vec![
        Constraint {
            kind: ConstraintKind::MinSoc,
            bound: 0.1,
        },
        Constraint {
            kind: ConstraintKind::MaxSoc,
            bound: 0.9,
        },
        Constraint {
            kind: ConstraintKind::SocFinal,
            bound: 0.8,
        },
    ];
    let snap = snapshot();
    let sp = spec(hard);
    let envelopes = vec![(1, env_battery()), (10, env_grid())];
    let batteries = vec![battery()];
    let context = ctx(&snap, &sp, &envelopes, &batteries);
    let p = plan(valid_assignments()); // 末态 1 ≠ 8
    let ring = CertifierKeyRing::new(1, b"station-cert-key");
    match CertifierModule::certify(&context, &p, 2_000, &ring) {
        Err(CertifierError::Violated {
            constraint_kind, ..
        }) => {
            assert_eq!(constraint_kind, ConstraintKind::SocFinal.code())
        }
        other => panic!("soc final: {other:?}"),
    }
    // 备用容量变异：峰购超过 cap − R。
    let hard = vec![
        Constraint {
            kind: ConstraintKind::MinSoc,
            bound: 0.1,
        },
        Constraint {
            kind: ConstraintKind::MaxSoc,
            bound: 0.9,
        },
        Constraint {
            kind: ConstraintKind::ReserveCapacityKw,
            bound: 18.0,
        }, // 允许峰 2
    ];
    let sp = spec(hard);
    let context = ctx(&snap, &sp, &envelopes, &batteries);
    let a = vec![
        asg(1, 1, -1.0),
        asg(1, 2, 5.0),
        asg(10, 1, 10.0),
        asg(10, 2, -5.0),
    ];
    let ring = CertifierKeyRing::new(1, b"k");
    match CertifierModule::certify(&context, &plan(a), 2_000, &ring) {
        Err(CertifierError::Violated {
            constraint_kind, ..
        }) => {
            assert_eq!(constraint_kind, ConstraintKind::ReserveCapacityKw.code())
        }
        other => panic!("reserve: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 4+7. 证书构造权 + Gateway 验证 + key 生命周期/回滚
// ---------------------------------------------------------------------------

#[test]
fn gateway_verify_and_key_lifecycle() {
    let mut ring = CertifierKeyRing::new(1, b"station-cert-key");
    let out = certify(valid_assignments()).expect("certify");
    let wire = plan(valid_assignments()).encode_v1();
    // 正常验证。
    assert!(
        CertifierModule::verify(&out.certificate, &wire, &out.key_envelope, &ring, 2_000).is_ok()
    );
    // 篡改 plan wire → digest 失配。
    let mut tampered = wire.clone();
    tampered[20] ^= 0xFF;
    assert!(
        CertifierModule::verify(&out.certificate, &tampered, &out.key_envelope, &ring, 2_000)
            .is_err()
    );
    // 错 key id。
    let mut bad_env = out.key_envelope;
    bad_env.key_id = 42;
    assert!(CertifierModule::verify(&out.certificate, &wire, &bad_env, &ring, 2_000).is_err());
    // 轮换：新 key 签发可验；旧 key（前代）签发仍可验（N-1 窗口）。
    ring.rotate(2, b"station-cert-key-v2").unwrap();
    let out2 = certify_with(valid_assignments(), &ring).unwrap();
    assert_eq!(out2.key_envelope.key_id, 2);
    assert!(
        CertifierModule::verify(&out2.certificate, &wire, &out2.key_envelope, &ring, 2_000).is_ok()
    );
    // 回滚（retire 活跃 key 2）→ 其证书立即不可验。
    assert!(ring.retire(2));
    assert!(
        CertifierModule::verify(&out2.certificate, &wire, &out2.key_envelope, &ring, 2_000)
            .is_err()
    );
    // key1（前代）证书仍可验（未被 retire）。
    assert!(
        CertifierModule::verify(&out.certificate, &wire, &out.key_envelope, &ring, 2_000).is_ok()
    );
    // 过期证书拒绝。
    assert!(CertifierModule::verify(
        &out.certificate,
        &wire,
        &out.key_envelope,
        &ring,
        10_000_000
    )
    .is_err());
}

// ---------------------------------------------------------------------------
// 复查修复回归：日内 SOC 最坏值 + 证书字段 MAC 覆盖 + 输入 refs 绑定
// ---------------------------------------------------------------------------

/// 日内越界但末态回窗：旧实现只查末态 → 漏认证；修复后按轨迹最坏值拒。
#[test]
fn intraday_soc_excursion_rejected_even_if_final_in_window() {
    // t0 放 4.5：5 → 0.5（跌破窗下限 1）；t1 充 4：0.5 → 4.5（末态合规）。
    let a = vec![
        asg(1, 1, 4.5),
        asg(1, 2, -4.0),
        asg(10, 1, 4.5),
        asg(10, 2, -4.0),
    ];
    match certify(a) {
        Err(CertifierError::Violated {
            constraint_kind,
            asset_id,
            ..
        }) => {
            assert_eq!(constraint_kind, ConstraintKind::MinSoc.code());
            assert_eq!(asset_id, 1);
        }
        other => panic!("日内越界必须被拒，got {other:?}"),
    }
    // 反向：t0 充 4.5：5 → 9.5（超上限 9）；t1 放 4：9.5 → 5.5（末态合规）。
    let a = vec![
        asg(1, 1, -4.5),
        asg(1, 2, 4.0),
        asg(10, 1, -4.5),
        asg(10, 2, 4.0),
    ];
    match certify(a) {
        Err(CertifierError::Violated {
            constraint_kind, ..
        }) => {
            assert_eq!(constraint_kind, ConstraintKind::MaxSoc.code());
        }
        other => panic!("日内越上限必须被拒，got {other:?}"),
    }
    // MinSoc 证据 lhs = 轨迹最坏值（0.5），非末态（4.5）。
    let out = certify(valid_assignments()).unwrap();
    let soc_min = out
        .records
        .iter()
        .find(|r| r.kind == ConstraintKind::MinSoc.code() && r.asset_id == 1)
        .unwrap();
    assert!((soc_min.lhs - 1.0).abs() < 1e-12, "最坏值 {}", soc_min.lhs);
}

/// 证书自身字段（有效期）篡改：MAC 覆盖证书 wire → 验证必拒。
#[test]
fn certificate_field_tampering_rejected_by_mac() {
    let ring = CertifierKeyRing::new(1, b"station-cert-key");
    let out = certify(valid_assignments()).expect("certify");
    let wire = plan(valid_assignments()).encode_v1();
    assert!(ring.active().is_some());
    // 合法证书 + 合法 wire：通过。
    assert!(
        CertifierModule::verify(&out.certificate, &wire, &out.key_envelope, &ring, 2_000).is_ok()
    );
    // 篡改证书有效期字段（wire 内 valid_until_ms 域）后 decode：字段
    // 合法性校验仍通过，但 MAC 失配 → 拒绝（延长有效期不可绕过）。
    let mut cert_wire = out.certificate.encode_v1();
    // 布局：4 magic + 2 ver + 2 flags + 32 ids + 32 plan_sha + 16 snap
    // + 4 certifier_version + 8 catalog + 4 tolerance + 8 certified_at
    // → valid_until_ms 起点 = 4+2+2+32+32+16+4+8+4+8 = 112。
    cert_wire[112..120].copy_from_slice(&u64::MAX.to_le_bytes());
    let extended = eneros_decision_contract::CertifiedPlan::decode_v1(&cert_wire)
        .expect("篡改有效期后的 wire 仍通过字段校验（正是 MAC 要防的）");
    assert_eq!(extended.valid_until_ms(), u64::MAX);
    assert!(
        CertifierModule::verify(&extended, &wire, &out.key_envelope, &ring, 2_000).is_err(),
        "MAC 未覆盖证书字段时此断言失败（永续漏洞）"
    );
}

/// 输入目录版本与计划目录版本分叉 → 拒绝（证据引用一致性）。
#[test]
fn catalog_version_divergence_rejected() {
    let snap = snapshot();
    let sp = base_spec();
    let envelopes = vec![(1, env_battery()), (10, env_grid())];
    let batteries = vec![battery()];
    let mut context = ctx(&snap, &sp, &envelopes, &batteries);
    context.constraint_catalog_version = 99; // plan 声称 v1
    let p = plan(valid_assignments());
    let ring = CertifierKeyRing::new(1, b"station-cert-key");
    match CertifierModule::certify(&context, &p, 2_000, &ring) {
        Err(CertifierError::PlanMismatch("catalog_version")) => {}
        other => panic!("{other:?}"),
    }
}

/// Violated 错误携带全部已产生证据（诊断重放面——记录不再随 Err 丢弃）。
#[test]
fn violated_error_carries_records() {
    let mut a = valid_assignments();
    a[1].power_kw = 6.5; // 电池 pmax 5：step2 MaxPowerKw 违约（非首条记录）
    match certify(a) {
        Err(CertifierError::Violated {
            record_index,
            records,
            ..
        }) => {
            assert!(record_index < records.len(), "序号越界");
            assert!(!records[record_index].pass, "指向的记录必为 fail");
            assert!(
                records.iter().take(record_index).all(|r| r.pass),
                "违约前的记录已随错误保留"
            );
        }
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 5. shadow 认证 HiGHS 全用例（对比 solver 残差与独立残差）
// ---------------------------------------------------------------------------

#[test]
fn shadow_certify_highs_golden_plan() {
    use eneros_planner::builder::{BatterySpec, GridSpec, PlanInput};
    use eneros_planner::{AssetBinding, Planner};
    use eneros_solver_core::highs::HighsSolver;
    #[allow(unused_imports)]
    use eneros_solver_core::solver::Solver;

    let spec_pl = ObjectiveSpec::builder(SnapshotId([7; 16]))
        .constraints(
            vec![
                Constraint {
                    kind: ConstraintKind::MinSoc,
                    bound: 0.1,
                },
                Constraint {
                    kind: ConstraintKind::MaxSoc,
                    bound: 0.9,
                },
                Constraint {
                    kind: ConstraintKind::MaxPowerKw,
                    bound: 15.0,
                },
            ],
            vec![],
        )
        .objective(
            ObjectiveKind::MaxRevenue,
            1,
            ObjectiveSource::RuleBundle,
            ReasonCode::PriceArbitrage,
        )
        .window(0, u64::MAX)
        .build()
        .unwrap();
    // WP-64.3.2.1：CompiledObjective 密封后经 test-util fixture 缝装配。
    let obj = eneros_objective_engine::CompiledObjective::compile_fixture(
        eneros_objective_engine::compiler::CompiledObjectiveParts {
            spec: spec_pl.clone(),
            prices_per_kwh: vec![1.0, 10.0],
            min_on_steps: 0,
            min_off_steps: 0,
            registry_generation: 1,
            bundle_version: 1,
            bundle_digest: [9; 32],
            horizon_step_ns: STEP_NS_HOUR,
            horizon_step_count: 2,
        },
    );
    let input = PlanInput {
        objective: &obj,
        snapshot_id: [7; 16],
        provenance_root: [0xAB; 32],
        registry_generation: 1,
        now_ms: 1_000,
        site_node_id: 1,
        envelopes: vec![(1, env_battery()), (10, env_grid())],
        batteries: vec![BatterySpec {
            asset_id: 1,
            capacity_kwh: 10.0,
            eta_c: 1.0,
            eta_d: 1.0,
            initial_kwh: 5.0,
        }],
        renewables: vec![],
        loads: vec![],
        grid: Some(GridSpec { asset_id: 10 }),
        dispatchable: vec![],
        bindings: vec![
            AssetBinding {
                asset_id: 1,
                device_id: 2,
                point_id: 6,
                unit_code: 6,
            },
            AssetBinding {
                asset_id: 10,
                device_id: 3,
                point_id: 1,
                unit_code: 6,
            },
        ],
    };
    let req =
        eneros_decision_contract::DecisionRequest::new([1; 16], obj.spec().clone(), 1, 10_000, 1)
            .unwrap();
    let mut hb = HighsSolver::new().unwrap();
    let mut planner = Planner::new(eneros_planner::tolerance::ToleranceProfile::l2(), 100);
    let plan = planner.plan(&mut hb, &req, &input).expect("plan");
    // solver 自述残差小。
    assert!(plan.max_primal_residual() < 1e-6);
    // 独立认证：全部硬约束重算通过。
    let snap = snapshot();
    let envelopes = vec![(1, env_battery()), (10, env_grid())];
    let batteries = vec![battery()];
    let context = EvalContext {
        snapshot: &snap,
        spec: obj.spec(),
        envelopes: &envelopes,
        batteries: &batteries,
        horizon_step_ns: STEP_NS_HOUR,
        horizon_step_count: 2,
        horizon_start_ms: 1_000,
        grid_asset_id: 10,
        tolerance_profile_id: 1,
        constraint_catalog_version: 1,
    };
    let ring = CertifierKeyRing::new(1, b"station-cert-key");
    let out = CertifierModule::certify(&context, &plan, 2_000, &ring)
        .expect("独立评估器必须放行 HiGHS 最优计划");
    assert!(out.records.iter().all(|r| r.pass));
    // 独立残差（证据中的最坏违约量，按判定方向）≤ 各记录容差。
    let worst_violation = out
        .records
        .iter()
        .map(|r| match r.cmp {
            eneros_certifier::proof::Cmp::Ge => (r.rhs - r.lhs).max(0.0),
            eneros_certifier::proof::Cmp::Le => (r.lhs - r.rhs).max(0.0),
            eneros_certifier::proof::Cmp::Eq => (r.lhs - r.rhs).abs(),
        })
        .fold(0.0f64, f64::max);
    assert!(worst_violation <= 1e-3, "独立违约量 {worst_violation}");
    // MaxPowerKw 证据（站级 → PCC）：轨迹最坏值 = 峰购 1。
    let site_max = out
        .records
        .iter()
        .find(|r| r.kind == ConstraintKind::MaxPowerKw.code() && r.asset_id == 10)
        .expect("site MaxPower 证据存在");
    assert!((site_max.lhs - 1.0).abs() < 1e-6, "峰购 {}", site_max.lhs);
}

// ---------------------------------------------------------------------------
// 性能（[09] §4.6：1000 资源 × 96 时段满足预算）
// ---------------------------------------------------------------------------

#[test]
fn perf_1000_resources_96_steps() {
    let snap = snapshot();
    // 物理一致夹具：零轨迹（SOC 恒 5、功率 0 在包络内）——记录数即
    // 结构性工作量（1000 资源 × 96 步 × 每步 2 包络记录 + SOC/斜率）。
    let sp = spec(vec![Constraint {
        kind: ConstraintKind::MaxPowerKw,
        bound: 2000.0,
    }]);
    let mut grid_env = env_grid();
    grid_env.p_min_kw = -2000.0;
    grid_env.p_max_kw = 2000.0;
    let mut envelopes = vec![(10, grid_env)];
    let mut batteries = vec![];
    let mut traj: BTreeMap<u64, Vec<Option<f64>>> = BTreeMap::new();
    for id in 100..1100u64 {
        let mut e = env_battery();
        e.asset_id = id;
        envelopes.push((id, e));
        batteries.push(EvalBattery {
            asset_id: id,
            capacity_kwh: 10.0,
            eta_c: 0.95,
            eta_d: 0.95,
            initial_kwh: 5.0,
        });
        traj.insert(id, vec![Some(0.0); 96]);
    }
    traj.insert(10, vec![Some(0.0); 96]);
    let mut context = ctx(&snap, &sp, &envelopes, &batteries);
    context.horizon_step_count = 96;
    let started = std::time::Instant::now();
    let records = eneros_certifier::eval::evaluate_trajectories(&traj, &context).expect("eval");
    let elapsed = started.elapsed();
    assert!(records.iter().all(|r| r.pass));
    // L2 预算（保守 1s；host 单线程）。
    assert!(elapsed.as_millis() < 1000, "eval took {:?}", elapsed);
}

// ---------------------------------------------------------------------------
// 发放面 rg 门（ADR-051）：certifier feature 仅本 crate 启用
// ---------------------------------------------------------------------------

/// WP-64.3.1.5：seal 发放面 feature 全仓唯一性 walk（读不到 = panic
/// 语义保持——`unwrap_or_default` 会把"读不到"变"恒通过"，假阳性门）。
/// 返回违规 crate 清单；`self_manifest` 豁免（正面断言单列）。
/// 匹配面：Cargo.toml 非注释行中的引号字面 `"certifier"`——覆盖
/// `features = ["certifier"]`/多元素数组/紧凑写法等全部激活形态
/// （裸 `certifier = []` 声明不算激活，不误报）。
fn certifier_feature_offenders(
    roots: &[&std::path::Path],
    self_manifest: &std::path::Path,
    min_checked: usize,
) -> Vec<std::path::PathBuf> {
    fn walk(
        dir: &std::path::Path,
        self_manifest: &std::path::Path,
        offenders: &mut Vec<std::path::PathBuf>,
        checked: &mut usize,
    ) {
        let entries =
            std::fs::read_dir(dir).unwrap_or_else(|e| panic!("rg 门必须读到目录 {dir:?}: {e}"));
        for entry in entries {
            let entry = entry.unwrap_or_else(|e| panic!("rg 门目录项读取失败: {e}"));
            let path = entry.path();
            if path.is_dir() {
                walk(&path, self_manifest, offenders, checked);
            } else if path.file_name().is_some_and(|n| n == "Cargo.toml") {
                if path == self_manifest {
                    continue;
                }
                let src = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("rg 门必须读到 {path:?}: {e}"));
                *checked += 1;
                if src.lines().any(|l| {
                    let t = l.trim();
                    !t.starts_with('#') && t.contains("\"certifier\"")
                }) {
                    offenders.push(path);
                }
            }
        }
    }
    let mut offenders = Vec::new();
    let mut checked = 0usize;
    for root in roots {
        walk(root, self_manifest, &mut offenders, &mut checked);
    }
    assert!(
        checked >= min_checked,
        "walk 只覆盖了 {checked} 个 crate（下限 {min_checked}）——遍历失效假绿防护"
    );
    offenders
}

/// WP-64.3.1.5 红测（临时目录形态，不污染真实 crate）：第四 crate 启用
/// certifier feature → 门红；干净树 → 通过。
#[test]
fn seal_gate_walk_rejects_fourth_crate() {
    let tmp = std::env::temp_dir().join(format!("r64-seal-gate-{}", std::process::id()));
    let crates = tmp.join("crates");
    for rel in [
        "ai/certifier",
        "ai/planner",
        "ai/objective-engine",
        "protocols/control-dispatch",
    ] {
        let dir = crates.join(rel);
        std::fs::create_dir_all(&dir).expect("temp tree");
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").expect("temp toml");
    }
    // 干净树：零违规。
    assert!(
        certifier_feature_offenders(&[&crates], &crates.join("ai/certifier/Cargo.toml"), 3)
            .is_empty()
    );
    // 第四 crate 启用 feature → 门红。
    std::fs::write(
        crates.join("protocols/control-dispatch/Cargo.toml"),
        "[features]\ncertifier = []\nfeatures = [\"certifier\"]\n",
    )
    .expect("temp toml");
    let offenders =
        certifier_feature_offenders(&[&crates], &crates.join("ai/certifier/Cargo.toml"), 3);
    assert_eq!(
        offenders.len(),
        1,
        "第四 crate 启用 certifier feature 必须被 walk 抓到"
    );
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn seal_issuance_feature_rg_gate() {
    let toml = include_str!("../Cargo.toml");
    assert!(
        toml.contains("features = [\"certifier\"]"),
        "本 crate 必须显式启用 certifier feature（seal 发放面）"
    );
    // 全仓唯一性（WP-64.3.1.5）：其他 crate 不得启用（transition 面为
    // dispatch 过渡窗，ADR-051 登记）——遍历 workspace crates/**/Cargo.toml
    // （原 3 路径清单的推广；读不到即 panic）。
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("workspace root")
        .to_path_buf();
    let offenders = certifier_feature_offenders(
        &[&workspace.join("crates"), &workspace.join("xtask")],
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("Cargo.toml")
            .as_path(),
        101,
    );
    assert!(
        offenders.is_empty(),
        "以下 crate 不得启用 certifier feature（发放面唯一性）: {offenders:?}"
    );
    // seal 构造器不在无 feature 面（编译期事实由 decision-contract
    // compile-fail 锁定；此处锁文档面：issue() 只在 certifier feature）。
    let cert_src = include_str!("../../../ai/decision-contract/src/certificate.rs");
    let issue_count = cert_src
        .lines()
        .filter(|l| l.contains("pub fn issue()"))
        .count();
    assert_eq!(issue_count, 1, "issue() 唯一");
    let issue_line = cert_src
        .lines()
        .find(|l| l.contains("pub fn issue()"))
        .unwrap();
    // issue() 定义必须位于 certifier feature 门内（其前最近的 cfg 行）。
    let idx = cert_src
        .lines()
        .position(|l| l.contains("pub fn issue()"))
        .unwrap();
    let gated = cert_src
        .lines()
        .take(idx)
        .filter(|l| l.trim_start().starts_with("#[cfg(feature"))
        .last()
        .map(|l| l.contains("certifier"))
        .unwrap_or(false);
    assert!(gated, "issue() 必须被 certifier feature 门控：{issue_line}");
}

/// 方向 5 出口「生成者拿不到证书发放能力」的**可执行守卫**（复查补口）。
///
/// 事实订正：`CertifierSeal` 的门是 **Cargo feature 门**，而 feature 在同一
/// 构建图内**合并**——70.9 起 `eneros-control-dispatch` 依赖
/// `eneros-certifier`（验证面：key ring + verify），该图内 `certifier`
/// feature 必然打开，于是 `CertifierSeal::issue()` /
/// `CertifiedPlan::new_certified` 在**语法上可达**。所以
/// 「发放面唯一性」不能只靠 compile-fail（它只覆盖无 feature 的图）。
///
/// 本测把规则降为源码扫描（rg 门同律）：除**定义面**
///（`ai/decision-contract`，seal 类型与构造器本体）与**唯一发放面**
///（`ai/certifier`）外，任何 crate 的源码不得出现发放构造调用。
///
/// 变异靶：在 planner/control-dispatch 任一源文件写入
/// `CertifierSeal::issue()` → 本测红（`new_certified(` 同理）。
#[test]
fn seal_issuance_source_scan_rejects_third_party_callers() {
    use std::path::{Path, PathBuf};

    /// 调用形态字面（**带括号**——注释/文档里提到符号名不算调用；
    /// `certify.rs` 的过渡面缺席锁以拼接构造针，也不含这两个字面）。
    const NEEDLES: [&str; 2] = ["CertifierSeal::issue()", "new_certified("];

    fn walk_src(dir: &Path, offenders: &mut Vec<(PathBuf, &'static str)>, checked: &mut usize) {
        let entries =
            std::fs::read_dir(dir).unwrap_or_else(|e| panic!("rg 门必须读到目录 {dir:?}: {e}"));
        for entry in entries {
            let path = entry
                .unwrap_or_else(|e| panic!("rg 门目录项读取失败: {e}"))
                .path();
            if path.is_dir() {
                walk_src(&path, offenders, checked);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let src = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("rg 门必须读到 {path:?}: {e}"));
                *checked += 1;
                for needle in NEEDLES {
                    if src.contains(needle) {
                        offenders.push((path.clone(), needle));
                    }
                }
            }
        }
    }

    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("workspace root")
        .to_path_buf();
    // 豁免：定义面（类型/构造器本体 + 其自身单测）与唯一发放面。
    let exempt = [
        workspace.join("crates/ai/decision-contract"),
        workspace.join("crates/ai/certifier"),
    ];
    let mut offenders: Vec<(PathBuf, &'static str)> = Vec::new();
    let mut checked = 0usize;
    for root in [workspace.join("crates"), workspace.join("xtask")] {
        walk_src(&root, &mut offenders, &mut checked);
    }
    assert!(
        checked >= 400,
        "walk 只覆盖了 {checked} 个源文件——遍历失效假绿防护"
    );
    let offenders: Vec<_> = offenders
        .into_iter()
        .filter(|(p, _)| !exempt.iter().any(|e| p.starts_with(e)))
        .collect();
    assert!(
        offenders.is_empty(),
        "发放构造只能在 eneros-certifier（唯一发放面）与 decision-contract（定义面）内出现；\
         生成者/验证者一律不得构造：{offenders:?}"
    );
}

// ---------------------------------------------------------------------------
// WP-65.2.5 组合链测（蓝图 65）：签名 RuleBundle → **真**
// ObjectiveCompiler::compile → Planner::plan → CertifierModule::certify。
// test-util fixture seam 不动（服务负例）；本测走唯一真编译入口，验证
// compile→plan→certify 三段可组合且三元绑定一致。
// ---------------------------------------------------------------------------

#[test]
fn combo_chain_real_compile_plan_certify() {
    use eneros_decision_contract::objective::ConstraintKind;
    use eneros_objective_engine::compiler::SignatureVerifier;
    use eneros_objective_engine::spec::MarketFacts;
    use eneros_objective_engine::{
        layer, normalization, term_kind, CompileInputs, ConstraintSpec, FactSet, HorizonPlan,
        ObjectiveCompiler, ObjectiveTerm, RuleBundle, ScopeRef,
    };
    use eneros_planner::builder::{BatterySpec, GridSpec, PlanInput};
    use eneros_planner::tolerance::ToleranceProfile;
    use eneros_planner::{AssetBinding, Planner};

    const CUT_NS: u64 = 1_000_000_000_000;
    let unit =
        |u: eneros_telemetry_model::PhysicalUnit| eneros_telemetry_model::unit_catalog::code_of(u);
    let rule = |id: u64,
                kind: ConstraintKind,
                layer_p: u16,
                unit_code: u16,
                lo: f64,
                hi: f64|
     -> ConstraintSpec {
        ConstraintSpec::new(
            id,
            kind.code(),
            0,
            layer_p,
            ScopeRef::SITE,
            unit_code,
            lo,
            hi,
            0,
            u64::MAX,
            [1; 16],
        )
        .unwrap()
    };

    // 1. 签名 RuleBundle（MinSoc/MaxSoc/MaxPowerKw + 价格缩放经济项；
    //    地平线 2 步 × 1h——与 certifier 既有 EvalContext 夹具同形）。
    let mut b = RuleBundle::builder([9; 16], 5)
        .validity_window(0, u64::MAX)
        .horizon(HorizonPlan::new(STEP_NS_HOUR, 2).unwrap())
        .site_rating_kw(100.0);
    for c in [
        rule(
            1,
            ConstraintKind::MinSoc,
            layer::DEVICE_SAFETY,
            unit(eneros_telemetry_model::PhysicalUnit::Dimensionless),
            0.1,
            0.1,
        ),
        rule(
            2,
            ConstraintKind::MaxSoc,
            layer::DEVICE_SAFETY,
            unit(eneros_telemetry_model::PhysicalUnit::Dimensionless),
            0.9,
            0.9,
        ),
        rule(
            3,
            ConstraintKind::MaxPowerKw,
            layer::GRID_CODE,
            unit(eneros_telemetry_model::PhysicalUnit::Kilowatts),
            -100.0,
            100.0,
        ),
    ] {
        b = b.add_constraint(c);
    }
    b = b.add_term(
        ObjectiveTerm::new(
            10,
            term_kind::ECONOMIC,
            1.0,
            unit(eneros_telemetry_model::PhysicalUnit::Dimensionless),
            normalization::PRICE_SCALED,
            layer::ECONOMIC,
            [1; 16],
        )
        .unwrap(),
    );
    let mut bundle = b.build().unwrap();
    bundle.seal(1, b"test-key");

    // 2. 真 compile（签名验证材料在输入）。
    let facts = FactSet {
        as_of_ns: CUT_NS,
        market: MarketFacts {
            prices_per_kwh: vec![1.0, 10.0],
            source_event_id: [1; 16],
        },
        ..Default::default()
    };
    let compiled = ObjectiveCompiler::new(60_000_000_000, 1, true)
        .compile(CompileInputs {
            snapshot: &snapshot(),
            registry_generation: 1,
            bundle: &bundle,
            facts: &facts,
            intent: None,
            verify: Some(SignatureVerifier {
                key_id: 1,
                key: b"test-key",
            }),
        })
        .expect("真编译（compile）");
    assert_eq!(compiled.horizon_step_count(), 2);

    // 3. 真 plan（HiGHS 后端；包络/电池/绑定与 certifier 侧同资产）。
    let input = PlanInput {
        objective: &compiled,
        snapshot_id: [7; 16],
        provenance_root: [0xAB; 32],
        registry_generation: 1,
        // now_ms = CUT_NS/1e6（真编译 spec 窗起点——窗内绑定）。
        now_ms: 1_000_000,
        site_node_id: 1,
        envelopes: vec![(1, env_battery()), (10, env_grid())],
        batteries: vec![BatterySpec {
            asset_id: 1,
            capacity_kwh: 10.0,
            eta_c: 1.0,
            eta_d: 1.0,
            initial_kwh: 5.0,
        }],
        renewables: vec![],
        loads: vec![],
        grid: Some(GridSpec { asset_id: 10 }),
        dispatchable: vec![],
        bindings: vec![
            AssetBinding {
                asset_id: 1,
                device_id: 2,
                point_id: 6,
                unit_code: 6,
            },
            AssetBinding {
                asset_id: 10,
                device_id: 3,
                point_id: 1,
                unit_code: 6,
            },
        ],
    };
    let req = eneros_decision_contract::DecisionRequest::new(
        [1u8; 16],
        compiled.spec().clone(),
        1,
        10_000,
        1,
    )
    .unwrap();
    let mut hb = eneros_solver_core::highs::HighsSolver::new().expect("Highs");
    let mut planner = Planner::new(ToleranceProfile::l2(), 100);
    let plan = planner.plan(&mut hb, &req, &input).expect("真 plan");

    // 4. 真 certify（EvalContext 消费**真编译**的 spec）。
    let envelopes = vec![(1, env_battery()), (10, env_grid())];
    let batteries = vec![battery()];
    let snap = snapshot();
    // 内联 EvalContext（ctx() 夹具 horizon_start_ms=1_000 服务既有用例；
    // 本测与真编译 spec 窗一致 = CUT_NS/1e6）。
    let context = EvalContext {
        snapshot: &snap,
        spec: compiled.spec(),
        envelopes: &envelopes,
        batteries: &batteries,
        horizon_step_ns: STEP_NS_HOUR,
        horizon_step_count: 2,
        horizon_start_ms: 1_000_000,
        grid_asset_id: 10,
        tolerance_profile_id: 1,
        constraint_catalog_version: 1,
    };
    let ring = CertifierKeyRing::new(1, b"station-cert-key");
    let out = CertifierModule::certify(&context, &plan, 1_005_000, &ring).expect("真 certify");

    // 5. 三元一致断言：plan↔cert 的 plan_sha/plan_id + objective_digest
    //    归真编译（证书无 objective_digest 字段——[00] §4 推迟证据；
    //    此处证 plan 与引擎 spec_digest 同源、证书经 certify 绑定二者）。
    assert_eq!(out.certificate.plan_id(), plan.id(), "plan_id 绑定");
    assert_eq!(
        out.certificate.plan_sha256(),
        &eneros_decision_contract::CandidatePlan::plan_wire_sha256(&plan),
        "plan_sha256 = 计划 wire SHA-256"
    );
    assert_eq!(
        plan.objective_digest(),
        compiled.spec_digest(),
        "objective_digest = 真编译 spec_digest"
    );
    assert!(out.records.iter().all(|r| r.pass), "独立评估全过");
}

// ---------------------------------------------------------------------------
// WP-68.4.4（stretch）：certifier 输入可达性——真实 SnapshotModule 产物
// 喂 shadow 认证路径（只验证不接电；生产消费归 68.5 后 certifier 接电卷）。
// ---------------------------------------------------------------------------

/// 真实 SnapshotModule（prod catalog + 覆盖点政策 + LocalTrustedEdge 采集
/// 形事件）构建 TrustedSnapshot——替代手搓 fixture 的产物级输入。
fn snapshot_module_product() -> eneros_snapshot_service::TrustedSnapshot {
    use eneros_semantic_catalog::SemanticCatalog;
    use eneros_snapshot_service::conflict::ConflictRule;
    use eneros_snapshot_service::{
        ConflictResolver, ProjectorCapacity, SnapshotPolicy, SnapshotPolicyRegistry,
        SnapshotRequest,
    };

    let toml = include_str!("../../../../configs/catalog.toml");
    let compiled =
        eneros_config_bundle::toml_compile::compile_catalog(toml).expect("compile catalog");
    let repo = eneros_semantic_catalog::open(&compiled.payload).expect("decode catalog");

    let coverage: Vec<u32> = vec![1, 2, 3, 4, 5, 6, 7];
    let mut policies = SnapshotPolicyRegistry::new();
    policies.register(SnapshotPolicy {
        purpose_id: 1,
        policy_version: 1,
        required: coverage.clone(),
        optional: vec![],
        allowed_lateness_ns: 600_000_000_000,
        max_age_ns: 600_000_000_000,
        min_quality_bits_ok: true,
        // 生产采集轨 integrity = LocalTrustedEdge（builder 门只认
        // Authenticated）——政策取生产事实（68.4 执行卡"快照政策"）。
        require_integrity: false,
        degraded_ok: false,
        allowed_sources: None,
        substitution: None,
        catalog_version: repo.version_set().contract_version,
    });
    let mut module = eneros_snapshot_service::module_from_catalog(
        repo,
        ProjectorCapacity {
            max_points: 64,
            max_events_per_point: 4,
        },
        policies,
        ConflictResolver::new(ConflictRule::PrimaryBackup),
        0,
    );
    // 采集形事件：7 覆盖点 × LocalTrustedEdge（生产 acq 唯一 integrity 形）。
    for point in coverage.iter() {
        module.apply_event(eneros_telemetry_model::event::ObservationEvent {
            tenant_id: 1,
            site_id: 1,
            source_id: 1,
            source_epoch: 1,
            sequence: u64::from(*point),
            event_time_ns: 900_000_000,
            ingest_time_ns: 900_000_000,
            asset_id: 1,
            device_id: 2,
            unit_code: 0,
            point_id: *point,
            schema_id: 1,
            schema_version: 1,
            quality_bits: eneros_telemetry_model::event::quality_bits::VALID,
            mapping_version: 1,
            value: eneros_telemetry_model::event::ObservationValue::F64(100.0),
            integrity: eneros_telemetry_model::event::IntegrityTag::LocalTrustedEdge,
            clock_id: 1,
            flags: 0,
        });
    }
    module
        .build(
            SnapshotRequest {
                tenant_id: 1,
                site_id: 1,
                purpose_id: 1,
                as_of_ns: 1_000_000_000,
                max_wait_ns: 0,
                policy_version: 1,
            },
            1_000_000_000,
        )
        .expect("真实模块必须产出快照（SM 解冻面）")
}

#[test]
fn wp6844_certifier_reachable_from_real_snapshot_module_product() {
    use eneros_decision_contract::SnapshotId;

    // 1. 真实产物：模块构建的 TrustedSnapshot（含 provenance/watermarks）。
    let snap = snapshot_module_product();
    assert!(!snap.observations.is_empty(), "产物须含覆盖点观测");
    assert!(!snap.source_watermarks.is_empty(), "产物须含来源水位");

    // 2. 五对一致输入：spec/plan 以**产物 snapshot_id** 绑定（bind 契约
    //    certifier/src/lib.rs:102-124）。
    let sid = SnapshotId(snap.snapshot_id);
    let sp = ObjectiveSpec::builder(sid)
        .constraints(
            vec![
                Constraint {
                    kind: ConstraintKind::MinSoc,
                    bound: 0.1,
                },
                Constraint {
                    kind: ConstraintKind::MaxSoc,
                    bound: 0.9,
                },
            ],
            vec![],
        )
        .objective(
            ObjectiveKind::MaxRevenue,
            1,
            ObjectiveSource::RuleBundle,
            ReasonCode::PriceArbitrage,
        )
        .window(0, u64::MAX)
        .build()
        .unwrap();
    let envelopes = vec![(1, env_battery()), (10, env_grid())];
    let batteries = vec![battery()];
    let context = ctx(&snap, &sp, &envelopes, &batteries);
    let mut parts = plan_parts_template();
    parts.snapshot_id = sid;
    let p = CandidatePlan::from_parts(parts).expect("合法计划");

    // 3. 可达性冒烟：bind（五对）+ certify（独立重算 + seal）全通。
    context
        .bind(&p, 2_000)
        .expect("bind 五对一致（可达性成立）");
    let ring = CertifierKeyRing::new(1, b"station-cert-key");
    let out = CertifierModule::certify(&context, &p, 2_000, &ring).expect("shadow certify");
    assert!(out.records.iter().all(|r| r.pass));
}

/// plan fixture 的可变 id 变体（既有 plan_raw 的 parts 模板）。
fn plan_parts_template() -> eneros_decision_contract::CandidatePlanParts {
    eneros_decision_contract::CandidatePlanParts {
        id: eneros_decision_contract::PlanId::new([1; 16]).unwrap(),
        snapshot_id: eneros_decision_contract::SnapshotId([7; 16]),
        objective_digest: [2; 32],
        resource_catalog_version: 1,
        horizon_start_ms: 1_000,
        step_ns: STEP_NS_HOUR,
        step_count: 2,
        solver_status: SolveStatusKind::Optimal,
        objective_value: 0.0,
        relative_gap: 0.0,
        max_primal_residual: 0.0,
        elapsed_ms: 10,
        model_digest: [3; 32],
        input_digest: [4; 32],
        assignments: valid_assignments(),
        issued_at_ms: 1_000,
        ttl_ms: 60_000,
    }
}
