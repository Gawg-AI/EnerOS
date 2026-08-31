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
    Assignment, CandidatePlan, ObjectiveSpec, PlanId, ReasonCode, SnapshotId, SolveStatusKind,
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
    ObjectiveSpec::new(
        SnapshotId([7; 16]),
        hard,
        vec![],
        ObjectiveKind::MaxRevenue,
        1,
        ObjectiveSource::RuleBundle,
        ReasonCode::PriceArbitrage,
        0,
        u64::MAX,
    )
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
    }
}

/// 手工计划构造（负例直接构造违约轨迹）。
fn plan(assignments: Vec<Assignment>) -> CandidatePlan {
    CandidatePlan::new(
        PlanId::new([1; 16]).unwrap(),
        SnapshotId([7; 16]),
        [2; 32],
        1,
        1_000,
        STEP_NS_HOUR,
        2,
        SolveStatusKind::Optimal,
        0.0,
        0.0,
        0.0,
        10,
        [3; 32],
        [4; 32],
        assignments,
        1_000,
        60_000,
    )
    .unwrap()
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
    let snap = snapshot();
    let sp = base_spec();
    let envelopes = vec![(1, env_battery()), (10, env_grid())];
    let batteries = vec![battery()];
    let context = ctx(&snap, &sp, &envelopes, &batteries);
    let p = plan(assignments);
    CertifierModule::certify(&context, &p, 2_000)
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
    match CertifierModule::certify(&context, &p, 2_000) {
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
    let r = CandidatePlan::new(
        PlanId::new([1; 16]).unwrap(),
        SnapshotId([7; 16]),
        [2; 32],
        1,
        1_000,
        STEP_NS_HOUR,
        2,
        SolveStatusKind::Optimal,
        0.0,
        0.0,
        0.0,
        10,
        [3; 32],
        [4; 32],
        vec![asg(1, 1, f64::NAN)],
        1_000,
        60_000,
    );
    assert_eq!(
        r,
        Err(eneros_decision_contract::ContractError::OutOfRange(
            "assignment.power_kw"
        ))
    );
    // 缺步（电池缺 t2）。
    let a = vec![asg(1, 1, -1.0), asg(10, 1, 1.0), asg(10, 2, -5.0)];
    match certify(a) {
        Err(CertifierError::MissingAssignment { asset_id, step }) => {
            assert_eq!(asset_id, 1);
            assert_eq!(step, 2);
        }
        other => panic!("{other:?}"),
    }
    // 重复分配。
    let a = vec![asg(1, 1, 1.0), asg(1, 1, 2.0)];
    assert_eq!(
        certify(a),
        Err(CertifierError::MalformedAssignment("duplicate"))
    );
    // 越步：合同构造层即拒（step_index ≤ step_count）。
    let r = CandidatePlan::new(
        PlanId::new([1; 16]).unwrap(),
        SnapshotId([7; 16]),
        [2; 32],
        1,
        1_000,
        STEP_NS_HOUR,
        2,
        SolveStatusKind::Optimal,
        0.0,
        0.0,
        0.0,
        10,
        [3; 32],
        [4; 32],
        vec![asg(1, 3, 1.0)],
        1_000,
        60_000,
    );
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
    match CertifierModule::certify(&context, &p, 2_000) {
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
    assert!(CertifierModule::certify(&context, &p, 2_000).is_ok());
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
    match CertifierModule::certify(&context, &p, 2_000_000) {
        Err(CertifierError::PlanExpired) => {}
        other => panic!("{other:?}"),
    }
    // 快照不匹配。
    let mut other_snap = snapshot();
    other_snap.snapshot_id = [9; 16];
    let context_bad = ctx(&other_snap, &sp, &envelopes, &batteries);
    match CertifierModule::certify(&context_bad, &p, 2_000) {
        Err(CertifierError::PlanMismatch("snapshot_id")) => {}
        other => panic!("{other:?}"),
    }
    // horizon 不匹配。
    let mut c2 = ctx(&snap, &sp, &envelopes, &batteries);
    c2.horizon_step_count = 3;
    match CertifierModule::certify(&c2, &p, 2_000) {
        Err(CertifierError::PlanMismatch("horizon")) => {}
        other => panic!("{other:?}"),
    }
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
    match CertifierModule::certify(&context, &p, 2_000) {
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
    match CertifierModule::certify(&context, &plan(a), 2_000) {
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
    let mut out = certify(valid_assignments()).expect("certify");
    let mut ring = CertifierKeyRing::new(1, b"station-cert-key");
    CertifierModule::seal_key_envelope(&mut out, &ring).expect("seal");
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
    ring.rotate(2, b"station-cert-key-v2");
    let mut out2 = certify(valid_assignments()).unwrap();
    CertifierModule::seal_key_envelope(&mut out2, &ring).unwrap();
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
// 5. shadow 认证 HiGHS 全用例（对比 solver 残差与独立残差）
// ---------------------------------------------------------------------------

#[test]
fn shadow_certify_highs_golden_plan() {
    use eneros_planner::builder::{BatterySpec, GridSpec, PlanInput};
    use eneros_planner::{AssetBinding, Planner};
    use eneros_solver_core::highs::HighsSolver;
    #[allow(unused_imports)]
    use eneros_solver_core::solver::Solver;

    let spec_pl = ObjectiveSpec::new(
        SnapshotId([7; 16]),
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
        ObjectiveKind::MaxRevenue,
        1,
        ObjectiveSource::RuleBundle,
        ReasonCode::PriceArbitrage,
        0,
        u64::MAX,
    )
    .unwrap();
    let obj = {
        let c = eneros_objective_engine::CompiledObjective {
            spec: spec_pl.clone(),
            prices_per_kwh: vec![1.0, 10.0],
            min_on_steps: 0,
            min_off_steps: 0,
            registry_generation: 1,
            bundle_version: 1,
            bundle_digest: [9; 32],
            spec_digest: [0; 32],
            horizon_step_ns: STEP_NS_HOUR,
            horizon_step_count: 2,
        };
        let d = eneros_objective_engine::compiler::digest_compiled(
            &c.spec,
            &c.prices_per_kwh,
            &c.bundle_digest,
            c.registry_generation,
        );
        eneros_objective_engine::CompiledObjective {
            spec_digest: d,
            ..c
        }
    };
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
        eneros_decision_contract::DecisionRequest::new([1; 16], obj.spec.clone(), 1, 10_000, 1)
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
        spec: &obj.spec,
        envelopes: &envelopes,
        batteries: &batteries,
        horizon_step_ns: STEP_NS_HOUR,
        horizon_step_count: 2,
        horizon_start_ms: 1_000,
        grid_asset_id: 10,
        tolerance_profile_id: 1,
    };
    let out = CertifierModule::certify(&context, &plan, 2_000)
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

#[test]
fn seal_issuance_feature_rg_gate() {
    let toml = include_str!("../Cargo.toml");
    assert!(
        toml.contains("features = [\"certifier\"]"),
        "本 crate 必须显式启用 certifier feature（seal 发放面）"
    );
    // 全仓唯一性：其他 crate 不得启用（transition 面为 dispatch 过渡窗，
    // ADR-051 登记）。
    for (path, ok) in [
        ("crates/ai/planner/Cargo.toml", false),
        ("crates/ai/objective-engine/Cargo.toml", false),
        ("crates/protocols/control-dispatch/Cargo.toml", false),
    ] {
        let src = std::fs::read_to_string(path).unwrap_or_default();
        assert!(
            !src.contains("features = [\"certifier\"]"),
            "{path} 不得启用 certifier feature（发放面唯一性）"
        );
        let _ = ok;
    }
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
