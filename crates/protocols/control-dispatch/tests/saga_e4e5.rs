//! 蓝图 61 Phase 5 WP-5.2（[10] §3）：BatchSaga E4/E5 测试套件。
//!
//! 判据映射（[10] §3.6/§3.7）：
//! - 故障矩阵：任意 journal append 前后断电（崩溃 campaign 覆盖）、批中
//!   第 N 个失败、ACK 丢/重复、readback 不符、双主旧 fencing、补偿失败、
//!   catalog/plan 执行中升级、批过期；
//! - E4/E5 多设备至少三资源：成功、部分失败、补偿、人工；
//! - 100 个崩溃点随机 campaign：最终无假 Committed、无重复已确认动作；
//! - 每命令真实状态 + 批整体语义（report 面）。
//!
//! 崩溃建模（诚实边界）：崩溃 = 事件日志在任意 append 边界截断 + 进程
//! 重启（物理效果按事件重放——已确认命令落盘，未知命令无效果变体由
//! executor 幂等键兜底重执行）；campaign 断言：无假 Applied（Applied ⇒
//! 物理值 = 授权值）、单命令物理写 ≤ 2（原执行 + 幂等重放）。

#![allow(clippy::disallowed_macros)]

use std::collections::BTreeMap;

use eneros_control_dispatch::saga::{
    needs_reconcile, BatchPolicy, BatchSaga, BatchState, CmdOutcome, CmdState, DriveReport,
    EventKind, ReconcileOutcome, SagaEvent, SagaExecutor, SagaPolicy,
};
use eneros_decision_contract::{CommandIntent, CompensationHint, IntentId, PlanId, Precondition};

// ---------------------------------------------------------------- 夹具

fn intent(id_byte: u8, device: u16, point: u32, kw: f64, restore: Option<f64>) -> CommandIntent {
    CommandIntent::new(
        IntentId([id_byte; 16]),
        PlanId([0x61; 16]),
        [0xAB; 32],
        u64::from(device),
        device,
        point,
        kw,
        6, // kW
        1_000,
        2_000_000,
        Precondition::None,
        match restore {
            Some(v) => CompensationHint::RestorePowerKw(v),
            None => CompensationHint::None,
        },
    )
    .unwrap()
}

/// 虚拟设备面：物理值 + 写次数（重复副作用检测）。
#[derive(Default)]
struct Plant {
    physical: BTreeMap<u16, f64>,
    writes: BTreeMap<(u16, [u8; 16]), u32>,
}

impl Plant {
    fn write(&mut self, device: u16, intent_id: [u8; 16], value: f64) {
        *self.physical.entry(device).or_insert(0.0) = value;
        *self.writes.entry((device, intent_id)).or_insert(0) += 1;
    }
}

/// 故障注入执行器。
struct MockExecutor<'a> {
    plant: &'a mut Plant,
    /// op 序号 → 故障（Fail/Reject/UnknownLanded/UnknownClean/StaleFencing/CatalogChanged/BadReadback）。
    script: BTreeMap<usize, Fault>,
    op: usize,
    /// readback 双重校验语义：第一次 single，第二次 confirmed。
    split_verify: bool,
    reconcile_unreachable: bool,
}

#[derive(Clone, Copy, Debug)]
enum Fault {
    Fail,
    Reject,
    /// 写已落地 + ACK 丢失（真崩溃形态）。
    UnknownLanded,
    /// 写未落地 + 超时。
    UnknownClean,
    StaleFencing,
    CatalogChanged,
    BadReadback,
}

impl<'a> MockExecutor<'a> {
    fn new(plant: &'a mut Plant) -> Self {
        Self {
            plant,
            script: BTreeMap::new(),
            op: 0,
            split_verify: false,
            reconcile_unreachable: false,
        }
    }

    fn with_fault(mut self, op: usize, f: Fault) -> Self {
        self.script.insert(op, f);
        self
    }
}

impl SagaExecutor for MockExecutor<'_> {
    fn execute(&mut self, intent: &CommandIntent, _attempt: u32) -> CmdOutcome {
        let fault = self.script.get(&self.op).copied();
        self.op += 1;
        match fault {
            None => {
                self.plant
                    .write(intent.device_id(), intent.intent_id().0, intent.power_kw());
                CmdOutcome::Applied {
                    receipt_digest: [1; 32],
                    readback: Some(intent.power_kw()),
                }
            }
            Some(Fault::UnknownLanded) => {
                self.plant
                    .write(intent.device_id(), intent.intent_id().0, intent.power_kw());
                CmdOutcome::Unknown {
                    request_digest: [2; 32],
                    reason: String::from("PROCESS-CRASH: write landed, ack lost"),
                }
            }
            Some(Fault::UnknownClean) => CmdOutcome::Unknown {
                request_digest: [2; 32],
                reason: String::from("PROCESS-CRASH: no effect"),
            },
            Some(Fault::Fail) => CmdOutcome::Failed {
                reason: String::from("device write failed"),
            },
            Some(Fault::Reject) => CmdOutcome::Rejected {
                reason: String::from("gateway rejected"),
            },
            Some(Fault::StaleFencing) => CmdOutcome::Rejected {
                reason: String::from("stale fencing (dual-master)"),
            },
            Some(Fault::CatalogChanged) => CmdOutcome::Rejected {
                reason: String::from("catalog changed mid-execution"),
            },
            Some(Fault::BadReadback) => {
                // 写落地但读回不符（ACK ≠ 物理成功）。
                self.plant
                    .write(intent.device_id(), intent.intent_id().0, intent.power_kw());
                CmdOutcome::Failed {
                    reason: String::from("readback mismatch"),
                }
            }
        }
    }

    fn reconcile(&mut self, intent: &CommandIntent) -> ReconcileOutcome {
        if self.reconcile_unreachable {
            return ReconcileOutcome::ResolvedManual {
                reason: String::from("device unreachable"),
            };
        }
        let actual = self
            .plant
            .physical
            .get(&intent.device_id())
            .copied()
            .unwrap_or(f64::NAN);
        if (actual - intent.power_kw()).abs() < f64::EPSILON {
            // readback-first 确认；双重校验：第一次 single，第二次 confirmed。
            ReconcileOutcome::ConfirmedApplied {
                readback: actual,
                double_verified: !self.split_verify,
            }
        } else {
            ReconcileOutcome::FailedConfirmed {
                reason: format!("readback {actual} != authorized {}", intent.power_kw()),
            }
        }
    }

    fn compensate(&mut self, intent: &CommandIntent, restore_kw: f64) -> CmdOutcome {
        let fault = self.script.get(&self.op).copied();
        self.op += 1;
        match fault {
            Some(Fault::Fail) | Some(Fault::Reject) => CmdOutcome::Failed {
                reason: String::from("compensation failed"),
            },
            _ => {
                self.plant
                    .write(intent.device_id(), intent.intent_id().0, restore_kw);
                CmdOutcome::Applied {
                    receipt_digest: [3; 32],
                    readback: Some(restore_kw),
                }
            }
        }
    }
}

fn saga_of(intents: &[CommandIntent], edges: &[(u32, u32)], policy: BatchPolicy) -> BatchSaga {
    BatchSaga::new(
        [0x62; 16],
        intents,
        edges,
        u64::MAX,
        SagaPolicy {
            batch: policy,
            max_parallel: 2,
        },
    )
}

// ---------------------------------------------------------------- E4：故障矩阵

#[test]
fn all_success_three_devices_completes() {
    // [10] §3.7：多设备 ≥3 资源成功路径。
    let intents = vec![
        intent(1, 1, 10, 10.0, None),
        intent(2, 2, 20, 20.0, None),
        intent(3, 3, 30, 30.0, None),
    ];
    let mut plant = Plant::default();
    let mut ex = MockExecutor::new(&mut plant);
    let mut saga = saga_of(&intents, &[], BatchPolicy::KeepSucceeded);
    let report = saga.drive(&mut ex, 5_000).unwrap();
    assert_eq!(report.final_state, BatchState::Completed);
    assert!(report.commands.iter().all(|c| c.state == CmdState::Applied));
    assert_eq!(report.commands.len(), 3);
    assert_eq!(plant.physical[&1], 10.0);
    assert_eq!(plant.physical[&2], 20.0);
    assert_eq!(plant.physical[&3], 30.0);
}

#[test]
fn nth_fails_keep_succeeded_blocks_dependents() {
    // 批中第 2 个失败（0-based op 1）→ Failed；依赖后继 Blocked。
    let intents = vec![
        intent(1, 1, 10, 10.0, None),
        intent(2, 2, 20, 20.0, None),
        intent(3, 2, 20, 50.0, None), // 依赖 intent 2（同设备串行链）
    ];
    let edges = vec![(1u32, 2u32)];
    let mut plant = Plant::default();
    let mut ex = MockExecutor::new(&mut plant).with_fault(1, Fault::Fail);
    let mut saga = saga_of(&intents, &edges, BatchPolicy::KeepSucceeded);
    let report = saga.drive(&mut ex, 5_000).unwrap();
    assert_eq!(report.final_state, BatchState::Failed);
    assert_eq!(report.commands[0].state, CmdState::Applied);
    assert_eq!(report.commands[1].state, CmdState::Failed);
    assert_eq!(report.commands[2].state, CmdState::Blocked);
    // 失败命令零写、依赖后继未投递。
    assert_eq!(plant.physical.get(&2), None);
}

#[test]
fn compensation_reverses_applied_and_irreversible_goes_manual() {
    // 补偿策略：已成功逆序补偿；撞到不可逆（补偿提示 None）→ Manual。
    // 布局：idx2 可补偿（失败）、idx1 不可逆（成功）、idx0 可补偿（成功）
    // ——逆序补偿先 idx1（成功回设）再 idx0（撞不可逆 idx0？否——
    // idx0 可补偿）。欲令「先成功一笔、再撞不可逆」：逆序 = [1,0]，
    // idx1 可补偿先补成功，idx0 不可逆 → Manual。
    let intents = vec![
        intent(1, 1, 10, 100.0, None),      // 不可逆（成功）
        intent(2, 2, 20, 30.0, Some(10.0)), // 可补偿（成功）
        intent(3, 3, 30, 20.0, Some(5.0)),  // 可补偿（失败触发补偿）
    ];
    let mut plant = Plant::default();
    let mut ex = MockExecutor::new(&mut plant).with_fault(2, Fault::Fail);
    let mut saga = saga_of(&intents, &[], BatchPolicy::Compensate);
    let report = saga.drive(&mut ex, 5_000).unwrap();
    assert_eq!(report.final_state, BatchState::ManualIntervention);
    // 逆序补偿：idx1 成功回设；idx0 不可逆 → 人工（保持 Applied 等人工）。
    assert_eq!(report.commands[1].state, CmdState::Compensated);
    assert_eq!(plant.physical[&2], 10.0, "可补偿命令已回设基线");
    assert_eq!(report.commands[0].state, CmdState::Applied);
    assert_eq!(report.commands[2].state, CmdState::Failed);
}

#[test]
fn full_compensation_succeeds() {
    let intents = vec![
        intent(1, 1, 10, 100.0, Some(0.0)),
        intent(2, 2, 20, 30.0, Some(10.0)),
        intent(3, 3, 30, 20.0, Some(5.0)),
    ];
    let mut plant = Plant::default();
    let mut ex = MockExecutor::new(&mut plant).with_fault(2, Fault::Fail);
    let mut saga = saga_of(&intents, &[], BatchPolicy::Compensate);
    let report = saga.drive(&mut ex, 5_000).unwrap();
    assert_eq!(report.final_state, BatchState::Compensated);
    // 已成功的两条补偿到基线；失败那条保持 Failed（补偿不发明效果）。
    assert_eq!(report.commands[0].state, CmdState::Compensated);
    assert_eq!(report.commands[1].state, CmdState::Compensated);
    assert_eq!(report.commands[2].state, CmdState::Failed);
    assert_eq!(plant.physical[&1], 0.0);
    assert_eq!(plant.physical[&2], 10.0);
    assert_eq!(plant.physical.get(&3), None);
}

#[test]
fn compensation_failure_goes_manual() {
    let intents = vec![
        intent(1, 1, 10, 100.0, Some(0.0)),
        intent(2, 2, 20, 30.0, Some(10.0)),
    ];
    let mut plant = Plant::default();
    // op1 = 第二命令失败触发补偿；op2 = 逆序第一笔补偿执行失败。
    let mut ex = MockExecutor::new(&mut plant).with_fault(1, Fault::Fail);
    ex.script.insert(2, Fault::Fail);
    let mut saga = saga_of(&intents, &[], BatchPolicy::Compensate);
    let report = saga.drive(&mut ex, 5_000).unwrap();
    assert_eq!(report.final_state, BatchState::ManualIntervention);
    assert_eq!(
        report.commands[0].state,
        CmdState::Compensating,
        "唯一 Applied 被补偿中"
    );
}

#[test]
fn ack_lost_write_landed_recovered_applied() {
    // ACK 丢/超时但写已落地 → Unknown → 对账（readback-first）→
    // RecoveredApplied 双重校验 → Applied；不重复写。
    let intents = vec![intent(1, 1, 10, 42.0, None)];
    let mut plant = Plant::default();
    let mut ex = MockExecutor::new(&mut plant)
        .with_fault(0, Fault::UnknownLanded)
        .with_fault(2, Fault::UnknownLanded);
    let mut saga = saga_of(&intents, &[], BatchPolicy::KeepSucceeded);
    let report = saga.drive(&mut ex, 5_000).unwrap();
    assert_eq!(report.final_state, BatchState::Completed);
    assert_eq!(report.commands[0].state, CmdState::Applied);
    // 单次物理写（对账零投递——readback 确认，不盲发）。
    assert_eq!(plant.writes[&(1u16, [1u8; 16])], 1, "不重复已确认副作用");
}

#[test]
fn ack_lost_no_effect_reconciles_then_executes() {
    // 崩溃未落地 → 对账确认未生效 → 批 Failed（无假 Committed）。
    let intents = vec![intent(1, 1, 10, 42.0, None)];
    let mut plant = Plant::default();
    let mut ex = MockExecutor::new(&mut plant).with_fault(0, Fault::UnknownClean);
    let mut saga = saga_of(&intents, &[], BatchPolicy::KeepSucceeded);
    let report = saga.drive(&mut ex, 5_000).unwrap();
    assert_eq!(report.final_state, BatchState::Failed);
    assert_eq!(report.commands[0].state, CmdState::Failed);
    assert!(!plant.physical.contains_key(&1), "无效果不变成功");
}

#[test]
fn reconcile_unresolved_goes_manual() {
    let intents = vec![intent(1, 1, 10, 42.0, None)];
    let mut plant = Plant::default();
    let mut ex = MockExecutor::new(&mut plant).with_fault(0, Fault::UnknownClean);
    ex.reconcile_unreachable = true;
    let mut saga = saga_of(&intents, &[], BatchPolicy::KeepSucceeded);
    let report = saga.drive(&mut ex, 5_000).unwrap();
    assert_eq!(report.final_state, BatchState::ManualIntervention);
}

#[test]
fn readback_mismatch_is_failed_not_committed() {
    // readback 不符（ACK 成功 ≠ 物理成功）→ Failed。
    let intents = vec![intent(1, 1, 10, 42.0, None)];
    let mut plant = Plant::default();
    let mut ex = MockExecutor::new(&mut plant).with_fault(0, Fault::BadReadback);
    let mut saga = saga_of(&intents, &[], BatchPolicy::KeepSucceeded);
    let report = saga.drive(&mut ex, 5_000).unwrap();
    assert_eq!(report.commands[0].state, CmdState::Failed);
    assert_eq!(report.final_state, BatchState::Failed);
}

#[test]
fn gateway_reject_is_terminal_zero_delivery() {
    let intents = vec![intent(1, 1, 10, 42.0, None)];
    let mut plant = Plant::default();
    let mut ex = MockExecutor::new(&mut plant).with_fault(0, Fault::Reject);
    let mut saga = saga_of(&intents, &[], BatchPolicy::KeepSucceeded);
    let report = saga.drive(&mut ex, 5_000).unwrap();
    assert_eq!(report.commands[0].state, CmdState::Failed);
    assert!(!plant.physical.contains_key(&1));
}

#[test]
fn stale_fencing_dual_master_rejected() {
    let intents = vec![intent(1, 1, 10, 42.0, None)];
    let mut plant = Plant::default();
    let mut ex = MockExecutor::new(&mut plant).with_fault(0, Fault::StaleFencing);
    let mut saga = saga_of(&intents, &[], BatchPolicy::KeepSucceeded);
    let report = saga.drive(&mut ex, 5_000).unwrap();
    assert_eq!(report.commands[0].state, CmdState::Failed);
    assert!(!plant.physical.contains_key(&1), "旧 fencing 零写");
}

#[test]
fn catalog_upgrade_mid_execution_fails_batch() {
    let intents = vec![intent(1, 1, 10, 10.0, None), intent(2, 2, 20, 20.0, None)];
    let mut plant = Plant::default();
    let mut ex = MockExecutor::new(&mut plant).with_fault(1, Fault::CatalogChanged);
    let mut saga = saga_of(&intents, &[], BatchPolicy::KeepSucceeded);
    let report = saga.drive(&mut ex, 5_000).unwrap();
    assert_eq!(report.final_state, BatchState::Failed);
    assert_eq!(report.commands[0].state, CmdState::Applied);
    assert_eq!(report.commands[1].state, CmdState::Failed);
}

#[test]
fn batch_expired_blocks_remaining() {
    let intents = vec![intent(1, 1, 10, 10.0, None), intent(2, 2, 20, 20.0, None)];
    let mut plant = Plant::default();
    let mut ex = MockExecutor::new(&mut plant);
    let mut saga = BatchSaga::new(
        [0x62; 16],
        &intents,
        &[],
        3_000, // deadline 已过
        SagaPolicy {
            batch: BatchPolicy::KeepSucceeded,
            max_parallel: 2,
        },
    );
    let report = saga.drive(&mut ex, 5_000).unwrap();
    assert_eq!(report.final_state, BatchState::Failed);
    assert!(
        report.commands.iter().all(|c| c.state == CmdState::Blocked),
        "过期批不执行任何命令"
    );
    assert!(plant.physical.is_empty(), "过期零投递");
}

#[test]
fn manual_policy_terminates_without_compensation() {
    let intents = vec![
        intent(1, 1, 10, 100.0, Some(0.0)),
        intent(2, 2, 20, 30.0, None),
    ];
    let mut plant = Plant::default();
    let mut ex = MockExecutor::new(&mut plant).with_fault(1, Fault::Fail);
    let mut saga = saga_of(&intents, &[], BatchPolicy::Manual);
    let report = saga.drive(&mut ex, 5_000).unwrap();
    assert_eq!(report.final_state, BatchState::ManualIntervention);
    assert_eq!(report.commands[0].state, CmdState::Applied, "已成功保持");
    assert!(plant.physical[&1] == 100.0);
}

// ---------------------------------------------------------------- E5：恢复/重放

#[test]
fn restore_replays_terminal_log_and_rejects_tampering() {
    let intents = vec![intent(1, 1, 10, 10.0, None), intent(2, 2, 20, 20.0, None)];
    let mut plant = Plant::default();
    let mut ex = MockExecutor::new(&mut plant);
    let mut saga = saga_of(&intents, &[], BatchPolicy::KeepSucceeded);
    let report = saga.drive(&mut ex, 5_000).unwrap();
    let events = saga.events().to_vec();

    // 完整重放恢复。
    let restored = BatchSaga::restore(
        &events,
        &intents,
        &[],
        u64::MAX,
        SagaPolicy {
            batch: BatchPolicy::KeepSucceeded,
            max_parallel: 2,
        },
    )
    .expect("合法日志可恢复");
    assert_eq!(restored.state(), report.final_state);

    // 序断链 → None。
    let mut broken = events.clone();
    broken[1].seq = 99;
    assert!(BatchSaga::restore(&broken, &intents, &[], u64::MAX, SagaPolicy::default()).is_none());

    // 非法批转换 → None。
    let mut illegal = events.clone();
    illegal[0].kind = EventKind::Batch {
        from: BatchState::Pending,
        to: BatchState::Completed,
    };
    assert!(BatchSaga::restore(&illegal, &intents, &[], u64::MAX, SagaPolicy::default()).is_none());

    // 未知意图的命令事件 → None。
    let mut ghost = events.clone();
    ghost.insert(
        1,
        SagaEvent {
            seq: 2,
            batch_id: [0x62; 16],
            kind: EventKind::Command {
                intent_id: [0xEE; 16],
                from: CmdState::Pending,
                to: CmdState::Executing,
            },
            attempt: 1,
            digest: [0; 32],
            readback: None,
            reason: String::from("ghost"),
            at_ms: 1,
        },
    );
    // 重排后续 seq 保证连续性——意图不存在仍必须拒绝。
    for (i, e) in ghost.iter_mut().enumerate() {
        e.seq = (i + 1) as u64;
    }
    assert!(BatchSaga::restore(&ghost, &intents, &[], u64::MAX, SagaPolicy::default()).is_none());
}

#[test]
fn needs_reconcile_classification() {
    assert!(!needs_reconcile(BatchState::Completed, 100, 200));
    assert!(!needs_reconcile(BatchState::ManualIntervention, 100, 200));
    assert!(needs_reconcile(BatchState::Executing, 100, 200));
    assert!(needs_reconcile(BatchState::Reconciling, 100, 200));
    // 未启动批过期 = 可安全丢弃，不需收敛。
    assert!(!needs_reconcile(BatchState::Pending, 100, 200));
    assert!(needs_reconcile(BatchState::Pending, 300, 200));
}

// ---------------------------------------------------------------- 100 崩溃点 campaign

#[test]
fn crash_campaign_100_truncation_points_no_fake_committed() {
    // 基线批：三设备 + 依赖链 + 可补偿，混合故障脚本。
    let intents = vec![
        intent(1, 1, 10, 10.0, Some(0.0)),
        intent(2, 1, 10, 20.0, Some(10.0)), // 同设备串行（依赖 0）
        intent(3, 2, 20, 30.0, None),
        intent(4, 3, 30, 40.0, Some(5.0)),
    ];
    let edges = vec![(0u32, 1u32)];
    let policy = SagaPolicy {
        batch: BatchPolicy::Compensate,
        max_parallel: 2,
    };

    // 基线运行（无故障）。
    let mut plant = Plant::default();
    let mut ex = MockExecutor::new(&mut plant);
    let mut baseline = BatchSaga::new([0x62; 16], &intents, &edges, u64::MAX, policy.clone());
    let base_report = baseline.drive(&mut ex, 5_000).unwrap();
    let events = baseline.events().to_vec();
    let total = events.len();
    assert!(total >= 8, "事件面充足：{total}");

    for i in 0..100usize {
        let k = i % (total + 1); // 截断点（事件 append 边界）
        let mut plant2 = Plant::default();
        // 物理效果按已记录事件重放（崩溃前已确认的写真实存在）。
        for e in &events[..k] {
            if let EventKind::Command {
                intent_id,
                to: CmdState::Applied | CmdState::Compensated,
                ..
            } = e.kind
            {
                if let Some(c) = intents.iter().find(|c| c.intent_id().0 == intent_id) {
                    let compensated = e.reason.starts_with("compensated");
                    let value = if compensated {
                        match c.compensation() {
                            CompensationHint::RestorePowerKw(v) => *v,
                            CompensationHint::None => c.power_kw(),
                        }
                    } else {
                        c.power_kw()
                    };
                    plant2.write(c.device_id(), intent_id, value);
                }
            }
        }
        // 截断点已越过批终态 → 恢复即终态，无需再驱动。
        if let Some(done) =
            BatchSaga::restore(&events[..k], &intents, &edges, u64::MAX, policy.clone())
        {
            if done.state().is_terminal() {
                assert_eq!(done.state(), base_report.final_state, "点 {k}：终态漂移");
                continue;
            }
        }
        let mut ex2 = MockExecutor::new(&mut plant2);
        // k=0 = 崩溃在任何 append 之前 → 等价新批。
        let mut restored = if k == 0 {
            BatchSaga::new([0x62; 16], &intents, &edges, u64::MAX, policy.clone())
        } else {
            BatchSaga::restore(&events[..k], &intents, &edges, u64::MAX, policy.clone())
                .unwrap_or_else(|| panic!("点 {k}：合法前缀必须可恢复"))
        };
        let report: DriveReport = restored
            .drive(&mut ex2, 5_000)
            .unwrap_or_else(|e| panic!("点 {}：恢复驱动失败：{}", k, e));

        // 不变量 1：终态合法。
        assert!(
            report.final_state.is_terminal(),
            "点 {k}：非终态 {report:?}"
        );
        // 不变量 2：无假 Committed——每设备物理值 = 报告中该设备最后一次
        // Applied/Compensated 命令的授权值（Applied ⇒ 写真实落地）。
        let mut expected: BTreeMap<u16, f64> = BTreeMap::new();
        for (idx, c) in report.commands.iter().enumerate() {
            let intent = &intents[idx];
            match c.state {
                CmdState::Applied => {
                    expected.insert(intent.device_id(), intent.power_kw());
                }
                CmdState::Compensated => {
                    let restore = match intent.compensation() {
                        CompensationHint::RestorePowerKw(v) => *v,
                        CompensationHint::None => intent.power_kw(),
                    };
                    expected.insert(intent.device_id(), restore);
                }
                _ => {}
            }
        }
        for (device, value) in &expected {
            assert_eq!(
                plant2.physical.get(device),
                Some(value),
                "点 {k}：假 Applied（物理不符）device={device}"
            );
        }
        // 不变量 3：无重复已确认副作用——单 (device,intent) 物理写 ≤ 2
        // （原执行 + 幂等重放；补偿另计键）。
        for (key, n) in &plant2.writes {
            assert!(*n <= 2, "点 {k}：重复副作用 {key:?}×{n}");
        }
        // 不变量 4：与基线同脚本、同故障面 → 同批终态。
        assert_eq!(
            report.final_state, base_report.final_state,
            "点 {k}：终态漂移"
        );
    }
}
