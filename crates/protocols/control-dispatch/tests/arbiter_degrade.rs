//! 蓝图 61 Phase 5 WP-5.3（[10] §4）：CommandArbiter 与 DegradationLease
//! 测试套件。
//!
//! 判据映射（[10] §4.6/§4.7）：
//! - 所有来源两两冲突（9×9 组合：优先级/抢占/合并/拒绝判定矩阵）；
//! - 抢占（紧急抢占在途；同级先到先执行）与人工模式（自动拒绝、切回
//!   自动必须 fresh snapshot）；
//! - lease：签名、过期、revoked（撤销纪元）、超功率/能量/次数/scope；
//! - 降级状态机抖动/恢复窗口/重启计数；联网恢复旧外部目标过期；D5
//!   资源从 planner capability 移除；
//! - 全仓唯一生产 Arbiter（源码审计测试）。

#![allow(clippy::disallowed_macros)]

use eneros_control_dispatch::arbiter::{
    ArbitrationRequest, CommandArbiter, CommandSource, ControlMode, Decision, ResourceContext,
};
use eneros_control_dispatch::degrade::{
    DegradationAuthority, DegradationLease, DegradationLevel, DegradationMachine, LeaseUsage,
};

const ALL_SOURCES: [CommandSource; 9] = [
    CommandSource::EmergencyLocalProtection,
    CommandSource::LocalSafetyController,
    CommandSource::LocalOperator,
    CommandSource::GridOperatorConfirmed,
    CommandSource::ProcessController,
    CommandSource::CommittedMarketPlan,
    CommandSource::SiteOptimizer,
    CommandSource::FederationObjective,
    CommandSource::ModelSuggestion,
];

fn req(source: CommandSource) -> ArbitrationRequest {
    ArbitrationRequest {
        source,
        device_id: 1,
        lease_valid: true,
        valid_now: true,
        preemptible_target: true,
        in_flight_sent_or_unknown: false,
    }
}

fn ctx_auto() -> ResourceContext {
    ResourceContext {
        mode: ControlMode::Auto,
        saga_in_flight: false,
        fresh_snapshot_after_manual: true,
    }
}

// ---------------------------------------------------------------- 两两冲突矩阵

#[test]
fn pairwise_source_conflicts_follow_precedence() {
    // 9×9 组合：在途来源 vs 新意图来源。判定规则：
    // - ModelSuggestion 新意图恒拒绝；
    // - 在途优先级更高于新意图 → Reject；
    // - 同级 → Reject（先到先执行，互斥）；
    // - 新意图优先级更高 → Preempt（reconcile-first）。
    for &flying in ALL_SOURCES.iter() {
        for &incoming in ALL_SOURCES.iter() {
            let mut arb = CommandArbiter::new(64);
            arb.register_in_flight(1, flying);
            let d = arb.decide(
                &req(incoming),
                &ResourceContext {
                    mode: ControlMode::Auto,
                    saga_in_flight: true,
                    fresh_snapshot_after_manual: true,
                },
            );
            if incoming == CommandSource::ModelSuggestion {
                assert!(matches!(d, Decision::Reject(_)), "{incoming:?} 必须拒绝");
                continue;
            }
            let stackable = |s: CommandSource| {
                matches!(
                    s,
                    CommandSource::SiteOptimizer | CommandSource::FederationObjective
                )
            };
            if stackable(flying) && stackable(incoming) {
                assert!(
                    matches!(d, Decision::MergeInSolver(_)),
                    "{flying:?}×{incoming:?} 可叠加服务 → solver"
                );
                continue;
            }
            if flying.precedence() <= incoming.precedence() {
                assert!(
                    matches!(d, Decision::Reject(_)),
                    "{incoming:?} 不得抢占 {flying:?}"
                );
            } else {
                assert!(
                    matches!(d, Decision::Preempt(..)),
                    "{incoming:?} 应抢占 {flying:?}"
                );
            }
        }
    }
}

#[test]
fn no_conflict_proceeds_and_losers_recorded() {
    let mut arb = CommandArbiter::new(64);
    let d = arb.decide(&req(CommandSource::SiteOptimizer), &ctx_auto());
    assert_eq!(d, Decision::Proceed);
    // 冲突后败者入审计日志。
    let _ = arb.decide(&req(CommandSource::ModelSuggestion), &ctx_auto());
    assert_eq!(arb.loser_log().len(), 1);
    assert_eq!(arb.loser_log()[0].1.source, CommandSource::ModelSuggestion);
}

// ---------------------------------------------------------------- 抢占/人工

#[test]
fn emergency_preempts_in_flight_optimizer() {
    let mut arb = CommandArbiter::new(64);
    arb.register_in_flight(1, CommandSource::SiteOptimizer);
    let d = arb.decide(
        &req(CommandSource::EmergencyLocalProtection),
        &ResourceContext {
            mode: ControlMode::Auto,
            saga_in_flight: true,
            fresh_snapshot_after_manual: true,
        },
    );
    match d {
        Decision::Preempt(_, losers) => {
            assert_eq!(losers[0].source, CommandSource::SiteOptimizer);
            assert!(losers[0].reason.contains("reconcile") || losers[0].reason.contains("取消"));
        }
        other => panic!("紧急必须抢占在途：{other:?}"),
    }
}

#[test]
fn manual_mode_rejects_auto_and_resume_requires_fresh_snapshot() {
    let mut arb = CommandArbiter::new(64);
    let ctx_manual = ResourceContext {
        mode: ControlMode::Manual,
        saga_in_flight: false,
        fresh_snapshot_after_manual: false,
    };
    // 自动来源全拒（紧急除外——紧急仍可行动）。
    for &s in ALL_SOURCES.iter() {
        let d = arb.decide(&req(s), &ctx_manual);
        if s.is_emergency() {
            assert!(
                !matches!(d, Decision::Reject(_)),
                "紧急不受人工模式阻绝：{s:?}"
            );
        } else if s != CommandSource::ModelSuggestion {
            assert!(matches!(d, Decision::Reject(_)), "人工模式拒绝 {s:?}");
        }
    }
    // 切回自动：无 fresh snapshot 拒绝。
    assert!(arb.resume_auto(false).is_err());
    assert!(arb.resume_auto(true).is_ok());
    // 检修隔离同理拒绝非紧急。
    let ctx_maint = ResourceContext {
        mode: ControlMode::Maintenance,
        saga_in_flight: false,
        fresh_snapshot_after_manual: false,
    };
    assert!(matches!(
        arb.decide(&req(CommandSource::SiteOptimizer), &ctx_maint),
        Decision::Reject(_)
    ));
}

#[test]
fn invalid_window_and_lease_rejected() {
    let mut arb = CommandArbiter::new(64);
    let mut r = req(CommandSource::SiteOptimizer);
    r.valid_now = false;
    assert!(matches!(arb.decide(&r, &ctx_auto()), Decision::Reject(_)));
    let mut r2 = req(CommandSource::GridOperatorConfirmed);
    r2.lease_valid = false;
    assert!(matches!(arb.decide(&r2, &ctx_auto()), Decision::Reject(_)));
}

// ---------------------------------------------------------------- lease 面

fn issue_lease(strategy: [u8; 32]) -> DegradationLease {
    DegradationLease::issue(
        &[7u8; 32],
        [9; 16],
        1,
        DegradationLevel::SolverFallback,
        strategy,
        [0xBB; 32],
        10.0, // max_abs_power_kw（heuristic 5kW 双倍裕度内）
        10.0,
        10.0,
        3,
        1_000,
        60_000,
        1,
        0,
    )
    .unwrap()
}

#[test]
fn lease_gate_signature_expiry_revocation_and_bounds() {
    let strategy = [0xAA; 32];
    let lease = issue_lease(strategy);
    let mut auth = DegradationAuthority::new(&[7u8; 32], 0);
    // 合法授权 + 计数。
    auth.authorize(&lease, 2_000, &strategy, 5.0, 5.0, 5.0)
        .unwrap();
    auth.authorize(&lease, 2_000, &strategy, 5.0, 5.0, 5.0)
        .unwrap();
    // 超功率限额。
    assert!(auth
        .authorize(&lease, 2_000, &strategy, 10.1, 5.0, 5.0)
        .is_err());
    // 超能量/爬坡。
    assert!(auth
        .authorize(&lease, 2_000, &strategy, 5.0, 10.1, 5.0)
        .is_err());
    assert!(auth
        .authorize(&lease, 2_000, &strategy, 5.0, 5.0, 10.1)
        .is_err());
    // scope：策略摘要不在白名单。
    assert!(auth
        .authorize(&lease, 2_000, &[0xCC; 32], 5.0, 5.0, 5.0)
        .is_err());
    // 过期。
    assert!(auth
        .authorize(&lease, 60_001, &strategy, 5.0, 5.0, 5.0)
        .is_err());
    // 次数耗尽（max_uses 3：已成功 2 次，再 1 次后第 4 次拒绝）。
    auth.authorize(&lease, 2_000, &strategy, 5.0, 5.0, 5.0)
        .unwrap();
    assert!(auth
        .authorize(&lease, 2_000, &strategy, 5.0, 5.0, 5.0)
        .is_err());
    // 撤销纪元：revoke 后旧 lease 全灭。
    let mut auth2 = DegradationAuthority::new(&[7u8; 32], 0);
    auth2
        .authorize(&lease, 2_000, &strategy, 5.0, 5.0, 5.0)
        .unwrap();
    let epoch = auth2.revoke_all();
    assert_eq!(epoch, 1);
    assert!(auth2
        .authorize(&lease, 2_000, &strategy, 5.0, 5.0, 5.0)
        .is_err());
    // 篡改签名拒绝。
    let mut forged = lease.clone();
    forged.max_abs_power_kw = 9000.0;
    assert!(auth2
        .authorize(&forged, 2_000, &strategy, 5.0, 5.0, 5.0)
        .is_err());
    // LeaseUsage 直连记账。
    let mut u = LeaseUsage::new(lease.lease_id);
    u.record_use(&lease).unwrap();
    assert_eq!(u.uses, 1);
}

// ---------------------------------------------------------------- 降级面 E5

#[test]
fn external_off_stale_objectives_expire_on_reconnect() {
    // D2 ExternalOff：冻结窗口内签发的外部承诺 lease；联网恢复后
    // （窗口过期）旧外部目标全部失效——authority 拒绝。
    let strategy = [0xDD; 32];
    let lease = DegradationLease::issue(
        &[7u8; 32],
        [8; 16],
        1,
        DegradationLevel::ExternalOff,
        strategy,
        [0xBB; 32],
        50.0,
        50.0,
        5.0,
        10,
        1_000,
        5_000, // 冻结窗到 t=5s
        1,
        0,
    )
    .unwrap();
    let mut auth = DegradationAuthority::new(&[7u8; 32], 0);
    auth.authorize(&lease, 2_000, &strategy, 5.0, 5.0, 5.0)
        .unwrap();
    // 恢复后（t>5s）：旧目标过期。
    assert!(auth
        .authorize(&lease, 5_001, &strategy, 5.0, 5.0, 5.0)
        .is_err());
}

#[test]
fn d5_removes_resource_from_planner_capability() {
    let mut m = DegradationMachine::new(2);
    assert!(m.resource_capable());
    m.escalate(DegradationLevel::SnapshotDegraded);
    assert!(m.resource_capable(), "D4 仍可规划（降额）");
    m.escalate(DegradationLevel::ExecutionUnknown);
    assert!(!m.resource_capable(), "D5 执行未知 ⇒ 资源移出规划池");
    // 逐级恢复后重回能力池。
    while m.level() != DegradationLevel::Normal {
        m.healthy_tick();
        m.healthy_tick();
    }
    assert!(m.resource_capable());
}

// ---------------------------------------------------------------- 唯一 Arbiter 审计

#[test]
fn single_production_arbiter_in_crate() {
    // [10] §4.7：全仓只有一个生产 Arbiter——源码级审计（排除注释行）：
    // `struct CommandArbiter` 只允许出现在 arbiter.rs；gateway/saga/
    // compiler 不得定义第二仲裁结构。
    let files: [(&str, &str); 8] = [
        ("arbiter.rs", include_str!("../src/arbiter.rs")),
        ("engine.rs", include_str!("../src/engine.rs")),
        ("saga.rs", include_str!("../src/saga.rs")),
        ("compiler.rs", include_str!("../src/compiler.rs")),
        ("degrade.rs", include_str!("../src/degrade.rs")),
        ("dispatch.rs", include_str!("../src/dispatch.rs")),
        ("journal.rs", include_str!("../src/journal.rs")),
        ("lib.rs", include_str!("../src/lib.rs")),
    ];
    for (name, src) in files {
        for line in src.lines() {
            let t = line.trim_start();
            if t.starts_with("//") {
                continue;
            }
            let defines = (t.contains("struct ") && t.contains("Arbiter"))
                || (t.contains("struct ") && t.contains("Arbitrator"));
            if name != "arbiter.rs" {
                assert!(!defines, "{name} 出现第二 Arbiter 定义：{line}");
            } else {
                assert!(
                    t.contains("pub struct CommandArbiter") || !defines,
                    "arbiter.rs 权威定义漂移：{line}"
                );
            }
        }
    }
}
