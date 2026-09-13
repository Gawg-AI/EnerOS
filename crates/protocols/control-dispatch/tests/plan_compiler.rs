//! 蓝图 61 Phase 5 WP-5.1（[10] §2）：PlanCompiler 测试套件。
//!
//! 判据映射（[10] §2.6）：
//! - 确定性 digest / 排序 / 单位与分辨率 / DAG 与串行 / 分批上限；
//! - assignment 未知合同拒绝指名、计划-证书绑定（篡改）、catalog 代次；
//! - 同设备串行链、跨设备并行上限、不可逆人工批准登记；
//! - mutation：任意值变化 ⇒ intent/batch 摘要漂移（Gateway 域外即可检）。

use eneros_control_dispatch::compiler::{
    compile, CompileReject, CompilerPolicy, MapContractView, NoAppliedState, PlanContract,
    SkipReason,
};
use eneros_decision_contract::{
    Assignment, CandidatePlan, CandidatePlanParts, CertifiedPlan, ContractError, PlanId,
    ResourceRegistry, SnapshotId, SolveStatusKind,
};

// ---------------------------------------------------------------- 夹具

fn contract(class: u16, reversible: u8, min: f64, max: f64, compensable: bool) -> PlanContract {
    PlanContract {
        class,
        readback_point_id: 1001,
        reversible,
        min,
        max,
        settle_ms: 0,
        compensable,
    }
}

/// 双资产注册表：asset 1 → 设备 1/点 10；asset 2 → 设备 2/点 20（单位 kW）。
struct TwoAssetRegistry {
    catalog_version: u64,
}

impl ResourceRegistry for TwoAssetRegistry {
    fn asset_exists(&self, asset_id: u64) -> bool {
        matches!(asset_id, 1 | 2)
    }
    fn asset_control_binding(&self, asset_id: u64) -> Option<(u16, u32, u16)> {
        match asset_id {
            1 => Some((1, 10, 6)),
            2 => Some((2, 20, 6)),
            _ => None,
        }
    }
    fn catalog_version(&self) -> u64 {
        self.catalog_version
    }
}

/// 多资产候选计划（assignments 逐条给定）。
#[allow(clippy::too_many_arguments)]
fn plan_with(
    id_byte: u8,
    issued_at_ms: u64,
    ttl_ms: u64,
    step_ns: u64,
    step_count: u32,
    assignments: Vec<Assignment>,
) -> CandidatePlan {
    CandidatePlan::from_parts(CandidatePlanParts {
        id: PlanId([id_byte; 16]),
        snapshot_id: SnapshotId([2u8; 16]),
        objective_digest: [0xAA; 32],
        resource_catalog_version: 1,
        horizon_start_ms: issued_at_ms,
        step_ns,
        step_count,
        solver_status: SolveStatusKind::Optimal,
        objective_value: 1.0,
        relative_gap: 0.0,
        max_primal_residual: 0.0,
        elapsed_ms: 10,
        model_digest: [0xBB; 32],
        input_digest: [0xCC; 32],
        assignments,
        issued_at_ms,
        ttl_ms,
    })
    .expect("合法计划")
}

fn cert_of(plan: &CandidatePlan, plan_sha: [u8; 32], now_ms: u64) -> CertifiedPlan {
    // 蓝图 70.9：过渡发放面已删除——测试契据改走 certifier crate 的
    // test-util 夹具（结构合法的 CertifiedPlan；引擎验真另判）。
    eneros_certifier::fixture_certificate(plan, plan_sha, now_ms, 4).expect("fixture cert")
}

fn plan_sha(plan: &CandidatePlan) -> [u8; 32] {
    // WP-64.3.2.2：测试改调单源辅助（原本地 Sha256::digest 公式重演删）。
    eneros_decision_contract::CandidatePlan::plan_wire_sha256_of_wire(&plan.encode_v1())
}

fn policy() -> CompilerPolicy {
    CompilerPolicy {
        compiler_version: 1,
        max_commands_per_batch: 8,
        max_parallel: 2,
        merge_below_kw: 0.0,
        min_window_ms: 1_000,
    }
}

fn two_contracts() -> MapContractView {
    let mut v = MapContractView::new();
    v.insert(1, contract(2, 1, -500.0, 500.0, false));
    v.insert(2, contract(2, 1, -500.0, 500.0, false));
    v
}

// ---------------------------------------------------------------- 确定性

#[test]
fn deterministic_digest_same_inputs() {
    let plan = plan_with(
        1,
        1_000,
        60_000,
        10_000_000_000,
        2,
        vec![
            Assignment {
                asset_id: 1,
                device_id: 1,
                point_id: 10,
                power_kw: 50.0,
                unit_code: 6,
                step_index: 1,
            },
            Assignment {
                asset_id: 1,
                device_id: 1,
                point_id: 10,
                power_kw: -30.0,
                unit_code: 6,
                step_index: 2,
            },
        ],
    );
    let cert = cert_of(&plan, plan_sha(&plan), 1_000);
    let reg = TwoAssetRegistry { catalog_version: 1 };
    let a = compile(
        &cert,
        &plan,
        &reg,
        &two_contracts(),
        &NoAppliedState,
        &policy(),
        1_000,
        0,
    )
    .expect("编译成功");
    let b = compile(
        &cert,
        &plan,
        &reg,
        &two_contracts(),
        &NoAppliedState,
        &policy(),
        1_000,
        0,
    )
    .expect("编译成功");
    assert_eq!(a.compiler_digest, b.compiler_digest);
    assert_eq!(a.batches.len(), b.batches.len());
    for (ba, bb) in a.batches.iter().zip(&b.batches) {
        assert_eq!(ba.batch_digest(), bb.batch_digest());
        assert_eq!(ba.encode_v1(), bb.encode_v1());
    }
    // 单意图 intent_id 可追溯 assignment（asset=1, step=1）。
    let i0 = a.batches[0].intents()[0];
    let expected = {
        use sha2::Digest as _;
        let mut buf = Vec::new();
        buf.extend_from_slice(b"eneros.intent-id.v1");
        buf.extend_from_slice(&plan_sha(&plan));
        buf.extend_from_slice(&1u64.to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes());
        let d: [u8; 32] = sha2::Sha256::digest(&buf).into();
        [d[0], d[1], d[2], d[3]]
    };
    assert_eq!(&i0.intent_id().0[..4], &expected);
}

#[test]
fn golden_batch_digest_stable() {
    // 固定夹具 golden（摘要锚：策略/派生规则漂移即失败——[10] §2.7
    // 「Batch golden」判据的 host 侧形态；首锚 = v0.314.41 编译器 v1）。
    let plan = plan_with(
        7,
        1_000,
        60_000,
        0, // 迁移期单时段形态
        1,
        vec![Assignment {
            asset_id: 1,
            device_id: 1,
            point_id: 10,
            power_kw: -40.0,
            unit_code: 6,
            step_index: 1,
        }],
    );
    let cert = cert_of(&plan, plan_sha(&plan), 1_000);
    let reg = TwoAssetRegistry { catalog_version: 1 };
    let out = compile(
        &cert,
        &plan,
        &reg,
        &two_contracts(),
        &NoAppliedState,
        &policy(),
        1_000,
        0,
    )
    .expect("编译成功");
    assert_eq!(out.batches.len(), 1);
    let digest = out.batches[0].batch_digest();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        hex, "abb1df57af6427f3af70524cd731c6590acef86f694db89e8ec2a496150a7c45",
        "batch golden 漂移——编译器派生规则或策略入码变化（须显式重锚）。\
         重锚记录（蓝图 70.9 WP-70.9.4）：旧值 01cd04a2… 锚定**过渡发放**契据\
         （certifier_version 1 / tolerance 1 / proof_digest [0xCD;32] / 硬约束 4）；\
         过渡发放面删除后契据改由 certifier crate 夹具签发（certifier_version 1、\
         tolerance 2、同一 proof_digest 与计数）——批摘要随契据字段变化，\
         非编译器规则变化"
    );
}

// ---------------------------------------------------------------- 排序/方向

#[test]
fn safer_directions_execute_first() {
    // 资产 1：50 → 10（|目标|<|当前|：释放约束，方向类 0）
    // 资产 2：0 → 80（增加出力，方向类 1）
    let plan = plan_with(
        1,
        1_000,
        60_000,
        0,
        1,
        vec![
            Assignment {
                asset_id: 2,
                device_id: 2,
                point_id: 20,
                power_kw: 80.0,
                unit_code: 6,
                step_index: 1,
            },
            Assignment {
                asset_id: 1,
                device_id: 1,
                point_id: 10,
                power_kw: 10.0,
                unit_code: 6,
                step_index: 1,
            },
        ],
    );
    let cert = cert_of(&plan, plan_sha(&plan), 1_000);
    let reg = TwoAssetRegistry { catalog_version: 1 };
    let applied = |a: u64| -> Option<f64> {
        if a == 1 {
            Some(50.0)
        } else {
            None
        }
    };
    let out = compile(
        &cert,
        &plan,
        &reg,
        &two_contracts(),
        &applied,
        &policy(),
        1_000,
        0,
    )
    .expect("编译成功");
    let intents: Vec<_> = out.batches.iter().flat_map(|b| b.intents()).collect();
    assert_eq!(intents.len(), 2);
    // 先 10（资产 1 降风险），后 80（资产 2 增出力）——尽管 assignment
    // 输入序相反。
    assert_eq!(intents[0].power_kw(), 10.0);
    assert_eq!(intents[1].power_kw(), 80.0);
}

// ---------------------------------------------------------------- 单位/分辨率

#[test]
fn unit_conversion_and_incompatible_rejected() {
    // 绑定单位 MW（code 7；kW=6）：assignment kW=1500 → MW=1.5。
    struct MwBinding;
    impl ResourceRegistry for MwBinding {
        fn asset_exists(&self, a: u64) -> bool {
            a == 1
        }
        fn asset_control_binding(&self, a: u64) -> Option<(u16, u32, u16)> {
            (a == 1).then_some((1, 10, 7)) // 7 = MW
        }
        fn catalog_version(&self) -> u64 {
            1
        }
    }
    let plan = plan_with(
        1,
        1_000,
        60_000,
        0,
        1,
        vec![Assignment {
            asset_id: 1,
            device_id: 1,
            point_id: 10,
            power_kw: 1500.0,
            unit_code: 6, // kW
            step_index: 1,
        }],
    );
    let cert = cert_of(&plan, plan_sha(&plan), 1_000);
    let out = compile(
        &cert,
        &plan,
        &MwBinding,
        &two_contracts(),
        &NoAppliedState,
        &policy(),
        1_000,
        0,
    )
    .expect("编译成功");
    assert_eq!(out.batches[0].intents()[0].power_kw(), 1.5);
    assert_eq!(out.batches[0].intents()[0].unit_code(), 7);

    // 量纲不兼容（绑定 0=未知单位 → 拒绝，不猜）。
    struct BadUnit;
    impl ResourceRegistry for BadUnit {
        fn asset_exists(&self, a: u64) -> bool {
            a == 1
        }
        fn asset_control_binding(&self, a: u64) -> Option<(u16, u32, u16)> {
            (a == 1).then_some((1, 10, 0)) // 0 = 未知单位
        }
        fn catalog_version(&self) -> u64 {
            1
        }
    }
    assert_eq!(
        compile(
            &cert,
            &plan,
            &BadUnit,
            &two_contracts(),
            &NoAppliedState,
            &policy(),
            1_000,
            0,
        )
        .err(),
        Some(CompileReject::UnitIncompatible { asset_id: 1 })
    );
}

#[test]
fn below_resolution_merges_with_evidence() {
    let reg = TwoAssetRegistry { catalog_version: 1 };
    let applied = |_: u64| -> Option<f64> { Some(50.0) };
    let mut pol = policy();
    pol.merge_below_kw = 1.0;
    // 部分合并：50.4→50 合并留证（80 正常成命令）。
    let plan2 = plan_with(
        1,
        1_000,
        60_000,
        0,
        1,
        vec![
            Assignment {
                asset_id: 1,
                device_id: 1,
                point_id: 10,
                power_kw: 50.4,
                unit_code: 6,
                step_index: 1,
            },
            Assignment {
                asset_id: 2,
                device_id: 2,
                point_id: 20,
                power_kw: 80.0,
                unit_code: 6,
                step_index: 1,
            },
        ],
    );
    let cert2 = cert_of(&plan2, plan_sha(&plan2), 1_000);
    let out2 = compile(
        &cert2,
        &plan2,
        &reg,
        &two_contracts(),
        &applied,
        &pol,
        1_000,
        0,
    )
    .expect("编译成功");
    assert_eq!(out2.skipped.len(), 1, "50.4→50 合并留证");
    assert_eq!(out2.skipped[0].reason, SkipReason::BelowResolution);
    assert_eq!(out2.skipped[0].asset_id, 1);
    assert_eq!(out2.batches[0].intents()[0].power_kw(), 80.0);

    // 全量合并：无剩余命令 → 计划级拒绝（空批 fail-closed，不伪造动作）。
    let plan1 = plan_with(
        1,
        1_000,
        60_000,
        0,
        1,
        vec![Assignment {
            asset_id: 1,
            device_id: 1,
            point_id: 10,
            power_kw: 50.4,
            unit_code: 6,
            step_index: 1,
        }],
    );
    let cert1 = cert_of(&plan1, plan_sha(&plan1), 1_000);
    let out1 = compile(
        &cert1,
        &plan1,
        &reg,
        &two_contracts(),
        &applied,
        &pol,
        1_000,
        0,
    );
    assert!(
        matches!(
            out1,
            Err(CompileReject::BatchReject(ContractError::MissingField(
                "intents"
            )))
        ),
        "全量合并 ⇒ 无命令可编译：{out1:?}"
    );
}

#[test]
fn expired_history_steps_skipped_on_recompile() {
    // 跨 tick 重编译：步 1 窗已过 → WindowExpired 跳过（不阻塞步 2 编译；
    // 键不入编译 = 永不重投）。
    let reg = TwoAssetRegistry { catalog_version: 1 };
    let plan = plan_with(
        1,
        1_000,
        60_000,
        10_000_000_000,
        2,
        vec![
            Assignment {
                asset_id: 1,
                device_id: 1,
                point_id: 10,
                power_kw: 10.0,
                unit_code: 6,
                step_index: 1,
            },
            Assignment {
                asset_id: 1,
                device_id: 1,
                point_id: 10,
                power_kw: 20.0,
                unit_code: 6,
                step_index: 2,
            },
        ],
    );
    // 证书在 t=15s 重发（tick 2 的 run_typed 行为）；步 1 窗 [1s,11s) 已过，
    // 步 2 窗 [11s,21s) 中段仍可执行。
    let cert = cert_of(&plan, plan_sha(&plan), 15_000);
    let out = compile(
        &cert,
        &plan,
        &reg,
        &two_contracts(),
        &NoAppliedState,
        &policy(),
        15_000,
        0,
    )
    .expect("步 2 仍可编译");
    assert_eq!(out.skipped.len(), 1);
    assert_eq!(out.skipped[0].reason, SkipReason::WindowExpired);
    assert_eq!(out.skipped[0].step_index, 1);
    assert_eq!(out.batches[0].intents().len(), 1);
    assert_eq!(out.batches[0].intents()[0].power_kw(), 20.0);
}

// ---------------------------------------------------------------- 拒绝面

#[test]
fn unknown_contract_named_and_cert_mismatch_rejected() {
    // assignment 指向无合同资产 3。
    struct OneAssetReg;
    impl ResourceRegistry for OneAssetReg {
        fn asset_exists(&self, a: u64) -> bool {
            a == 1
        }
        fn asset_control_binding(&self, a: u64) -> Option<(u16, u32, u16)> {
            (a == 1).then_some((1, 10, 6))
        }
        fn catalog_version(&self) -> u64 {
            1
        }
    }
    let plan = plan_with(
        1,
        1_000,
        60_000,
        0,
        1,
        vec![
            Assignment {
                asset_id: 1,
                device_id: 1,
                point_id: 10,
                power_kw: 1.0,
                unit_code: 6,
                step_index: 1,
            },
            Assignment {
                asset_id: 3,
                device_id: 3,
                point_id: 30,
                power_kw: 2.0,
                unit_code: 6,
                step_index: 1,
            },
        ],
    );
    let cert = cert_of(&plan, plan_sha(&plan), 1_000);
    assert_eq!(
        compile(
            &cert,
            &plan,
            &OneAssetReg,
            &two_contracts(),
            &NoAppliedState,
            &policy(),
            1_000,
            0,
        )
        .err(),
        Some(CompileReject::UnknownContract { asset_id: 3 })
    );

    // 证书篡改：cert 绑定另一 plan digest。
    let plan_ok = plan_with(
        1,
        1_000,
        60_000,
        0,
        1,
        vec![Assignment {
            asset_id: 1,
            device_id: 1,
            point_id: 10,
            power_kw: 1.0,
            unit_code: 6,
            step_index: 1,
        }],
    );
    let forged: [u8; 32] = [0xEE; 32];
    let bad_cert = cert_of(&plan_ok, forged, 1_000);
    assert_eq!(
        compile(
            &bad_cert,
            &plan_ok,
            &TwoAssetRegistry { catalog_version: 1 },
            &two_contracts(),
            &NoAppliedState,
            &policy(),
            1_000,
            0,
        )
        .err(),
        Some(CompileReject::PlanCertMismatch)
    );
}

#[test]
fn catalog_generation_change_rejected() {
    let plan = plan_with(
        1,
        1_000,
        60_000,
        0,
        1,
        vec![Assignment {
            asset_id: 1,
            device_id: 1,
            point_id: 10,
            power_kw: 1.0,
            unit_code: 6,
            step_index: 1,
        }],
    );
    let cert = cert_of(&plan, plan_sha(&plan), 1_000);
    // registry 代次 2 ≠ 证书 catalog_version 1 → batch 构造拒绝。
    let out = compile(
        &cert,
        &plan,
        &TwoAssetRegistry { catalog_version: 2 },
        &two_contracts(),
        &NoAppliedState,
        &policy(),
        1_000,
        0,
    )
    .err();
    assert!(
        matches!(out, Some(CompileReject::BatchReject(_))),
        "catalog 代次变化必须拒绝：{out:?}"
    );
}

#[test]
fn window_and_range_and_readback_negatives() {
    let reg = TwoAssetRegistry { catalog_version: 1 };
    // 过短窗：min_window 30s > ttl 10s（step_ns=0 → 窗=证书窗）。
    let plan = plan_with(
        1,
        1_000,
        10_000,
        0,
        1,
        vec![Assignment {
            asset_id: 1,
            device_id: 1,
            point_id: 10,
            power_kw: 1.0,
            unit_code: 6,
            step_index: 1,
        }],
    );
    let cert = cert_of(&plan, plan_sha(&plan), 1_000);
    let mut pol = policy();
    pol.min_window_ms = 30_000;
    assert_eq!(
        compile(
            &cert,
            &plan,
            &reg,
            &two_contracts(),
            &NoAppliedState,
            &pol,
            1_000,
            0
        )
        .err(),
        Some(CompileReject::WindowTooShort)
    );

    // 越合同域（500 上限；750 拒绝，不钳位）。
    let plan_hi = plan_with(
        1,
        1_000,
        60_000,
        0,
        1,
        vec![Assignment {
            asset_id: 1,
            device_id: 1,
            point_id: 10,
            power_kw: 750.0,
            unit_code: 6,
            step_index: 1,
        }],
    );
    let cert_hi = cert_of(&plan_hi, plan_sha(&plan_hi), 1_000);
    assert_eq!(
        compile(
            &cert_hi,
            &plan_hi,
            &reg,
            &two_contracts(),
            &NoAppliedState,
            &policy(),
            1_000,
            0
        )
        .err(),
        Some(CompileReject::ValueOutOfRange {
            asset_id: 1,
            step_index: 1
        })
    );

    // 无回读合同拒绝。
    let mut no_rb = MapContractView::new();
    no_rb.insert(
        1,
        PlanContract {
            readback_point_id: 0,
            ..contract(2, 1, -500.0, 500.0, false)
        },
    );
    assert_eq!(
        compile(
            &cert,
            &plan,
            &reg,
            &no_rb,
            &NoAppliedState,
            &policy(),
            1_000,
            0
        )
        .err(),
        Some(CompileReject::NoReadback { asset_id: 1 })
    );
}

// ---------------------------------------------------------------- DAG/分批

#[test]
fn same_device_serial_and_multi_batch_split() {
    // 双资产各 2 步链，批上限 2 → 两批（切块只发生在资产链边界——
    // 链完整性不可让）；批内边链、批序 = 资产序。
    let plan = plan_with(
        1,
        1_000,
        3_600_000,
        10_000_000_000,
        2,
        vec![
            Assignment {
                asset_id: 1,
                device_id: 1,
                point_id: 10,
                power_kw: 10.0,
                unit_code: 6,
                step_index: 1,
            },
            Assignment {
                asset_id: 1,
                device_id: 1,
                point_id: 10,
                power_kw: 20.0,
                unit_code: 6,
                step_index: 2,
            },
            Assignment {
                asset_id: 2,
                device_id: 2,
                point_id: 20,
                power_kw: 30.0,
                unit_code: 6,
                step_index: 1,
            },
            Assignment {
                asset_id: 2,
                device_id: 2,
                point_id: 20,
                power_kw: 40.0,
                unit_code: 6,
                step_index: 2,
            },
        ],
    );
    let cert = cert_of(&plan, plan_sha(&plan), 1_000);
    let reg = TwoAssetRegistry { catalog_version: 1 };
    let mut pol = policy();
    pol.max_commands_per_batch = 2;
    let out = compile(
        &cert,
        &plan,
        &reg,
        &two_contracts(),
        &NoAppliedState,
        &pol,
        1_000,
        0,
    )
    .expect("编译成功");
    assert_eq!(out.batches.len(), 2);
    assert_eq!(out.batches[0].intents().len(), 2);
    assert_eq!(out.batches[1].intents().len(), 2);
    // 批内串行链 0→1（同设备恒串行）。
    assert_eq!(out.batches[0].dependency_edges(), [(0u32, 1u32)]);
    assert_eq!(out.batches[1].dependency_edges(), [(0u32, 1u32)]);
    // 批 1 = 资产 1，批 2 = 资产 2；时段序保持。
    assert_eq!(out.batches[0].intents()[0].asset_id(), 1);
    assert_eq!(out.batches[1].intents()[0].asset_id(), 2);
    assert_eq!(out.batches[0].intents()[0].power_kw(), 10.0);
    assert_eq!(out.batches[1].intents()[1].power_kw(), 40.0);
    // 批 id 确定性且随序号变化。
    assert_ne!(out.batches[0].batch_id(), out.batches[1].batch_id());
}

#[test]
fn chain_over_batch_cap_rejected() {
    let assignments: Vec<Assignment> = (1..=3)
        .map(|s| Assignment {
            asset_id: 1,
            device_id: 1,
            point_id: 10,
            power_kw: 10.0 * s as f64,
            unit_code: 6,
            step_index: s,
        })
        .collect();
    let plan = plan_with(1, 1_000, 3_600_000, 10_000_000_000, 3, assignments);
    let cert = cert_of(&plan, plan_sha(&plan), 1_000);
    let reg = TwoAssetRegistry { catalog_version: 1 };
    let mut pol = policy();
    pol.max_commands_per_batch = 2;
    assert_eq!(
        compile(
            &cert,
            &plan,
            &reg,
            &two_contracts(),
            &NoAppliedState,
            &pol,
            1_000,
            0
        )
        .err(),
        Some(CompileReject::ChainExceedsBatch {
            asset_id: 1,
            len: 3
        })
    );
}

// ---------------------------------------------------------------- 不可逆/补偿

#[test]
fn irreversible_gets_manual_flag_and_no_compensation() {
    let plan = plan_with(
        1,
        1_000,
        60_000,
        0,
        1,
        vec![Assignment {
            asset_id: 1,
            device_id: 1,
            point_id: 10,
            power_kw: 100.0,
            unit_code: 6,
            step_index: 1,
        }],
    );
    let cert = cert_of(&plan, plan_sha(&plan), 1_000);
    let reg = TwoAssetRegistry { catalog_version: 1 };
    let mut contracts = MapContractView::new();
    contracts.insert(1, contract(2, 0, -500.0, 500.0, true)); // 不可逆
    let applied = |_: u64| -> Option<f64> { Some(10.0) };
    let out = compile(
        &cert,
        &plan,
        &reg,
        &contracts,
        &applied,
        &policy(),
        1_000,
        0,
    )
    .expect("编译成功");
    assert_eq!(out.manual_approval.len(), 1);
    assert_eq!(
        out.batches[0].intents()[0].compensation(),
        &eneros_decision_contract::CompensationHint::None,
        "不可逆动作补偿强制 None"
    );
}

#[test]
fn compensable_contract_restores_baseline() {
    let plan = plan_with(
        1,
        1_000,
        60_000,
        0,
        1,
        vec![Assignment {
            asset_id: 1,
            device_id: 1,
            point_id: 10,
            power_kw: 100.0,
            unit_code: 6,
            step_index: 1,
        }],
    );
    let cert = cert_of(&plan, plan_sha(&plan), 1_000);
    let reg = TwoAssetRegistry { catalog_version: 1 };
    let mut contracts = MapContractView::new();
    contracts.insert(1, contract(2, 1, -500.0, 500.0, true)); // 可补偿
    let applied = |_: u64| -> Option<f64> { Some(10.0) };
    let out = compile(
        &cert,
        &plan,
        &reg,
        &contracts,
        &applied,
        &policy(),
        1_000,
        0,
    )
    .expect("编译成功");
    assert!(out.manual_approval.is_empty());
    assert_eq!(
        out.batches[0].intents()[0].compensation(),
        &eneros_decision_contract::CompensationHint::RestorePowerKw(10.0),
        "补偿 = 计划前已确认基线"
    );
}

// ---------------------------------------------------------------- 幂等键

#[test]
fn plan_command_correlation_legacy_compat_and_stable() {
    use eneros_control_dispatch::compiler::plan_command_correlation;
    let pid = PlanId([7u8; 16]);
    let iid = eneros_decision_contract::IntentId([9u8; 16]);
    // legacy 单命令形态 = PlanId 字节（Phase 0 单 PCS 幂等记录兼容）。
    assert_eq!(plan_command_correlation(&pid, &iid, true), pid.0);
    // 多步形态 = intent_id 派生：跨重放稳定、异 intent 互异。
    let a = plan_command_correlation(&pid, &iid, false);
    assert_eq!(a, plan_command_correlation(&pid, &iid, false), "重放稳定");
    let iid2 = eneros_decision_contract::IntentId([8u8; 16]);
    assert_ne!(a, plan_command_correlation(&pid, &iid2, false));
    assert_ne!(a, pid.0);
}

#[test]
fn unbound_assignment_resolved_via_default_asset() {
    // [06] §3.3：后端侧 unbound 形态（asset=0）——dispatch 编译期按
    // profile 完成绑定（default_asset_id=1）；0 = 严格拒绝。
    let plan = plan_with(
        1,
        1_000,
        60_000,
        0,
        1,
        vec![Assignment {
            asset_id: 0,
            device_id: 0,
            point_id: 0,
            power_kw: 25.0,
            unit_code: 6,
            step_index: 1,
        }],
    );
    let cert = cert_of(&plan, plan_sha(&plan), 1_000);
    let reg = TwoAssetRegistry { catalog_version: 1 };
    let out = compile(
        &cert,
        &plan,
        &reg,
        &two_contracts(),
        &NoAppliedState,
        &policy(),
        1_000,
        1,
    )
    .expect("绑定完成可编译");
    let intent = out.batches[0].intents()[0];
    assert_eq!(intent.asset_id(), 1);
    assert_eq!(intent.device_id(), 1);
    assert_eq!(intent.point_id(), 10);
    assert_eq!(intent.power_kw(), 25.0);
    // 严格模式（default=0）：unbound 拒绝。
    assert_eq!(
        compile(
            &cert,
            &plan,
            &reg,
            &two_contracts(),
            &NoAppliedState,
            &policy(),
            1_000,
            0,
        )
        .err(),
        Some(CompileReject::UnknownContract { asset_id: 0 })
    );
}
