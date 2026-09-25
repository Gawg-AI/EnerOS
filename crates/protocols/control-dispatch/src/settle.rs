//! 结算决策表（62 v0.315.0 WP-62.1.3，S3；C1 拆分 2026-09-24 自 engine.rs
//! 整块迁出——纯函数表 + 触发/结局类型族 + 表级单测同迁，编排面
//! `SettleDispatch`（engine 内聚实现）留 engine.rs）。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_dual_brain::audit::{ExecOutcome, SafetyOutcome};

use crate::dispatch::DispatchState;
use crate::journal::ExecutionStatus;
// ==========================================================================
// 62 v0.315.0 WP-62.1.3（S3）：结算决策表。
//
// 每条命令的结局（journal 状态转移 + durable intent 状态 + 回执形态 +
// audit 文案）由纯函数 [`settle`] 单点决定；[`SettleDispatch::settle_command`]
// 是结算路径上 `journal.transition` / `intent_log.update_state` / `audit`
// 的唯一编排点（stage 推进另经 [`SettleDispatch::push_stage`]）。
// 文案经 [`AuditText`] 惰性渲染，串口/audit 词形逐字节不变（audit_chain
// 与 45–47/61 冻结证据链依赖）。写序不变量（61 v0.314.24：Applied =
// durable 终态 ack 先行，成功后才 journal AppliedConfirmed）由表内
// `durable` 序表达，表级单测 `settle_table_*` 锁定。
// ==========================================================================

/// 结算触发事件（execute_core 前置拒绝 + deliver_and_settle 全部分类臂）。
///
/// 新增一个触发（如假想 OutcomeCode）的成本 = 本枚举一臂 + `settle`
/// 表一行 + 表测试一用例 + 流程分类点一行——不动任何既有臂。
#[derive(Debug)]
pub(crate) enum SettleTrigger {
    /// 电气护栏拒绝（步骤 4.5，授权后零投递）。
    SafetyReject { reason: String },
    /// 速率预算拒绝（步骤 5；现状无 audit——保持）。
    BudgetRate,
    /// 能量预算拒绝（步骤 5；现状无 audit——保持）。
    BudgetEnergy,
    /// durable intent append 失败（步骤 6，零投递终局）。
    IntentAppendFailed { err: String },
    /// journal Authorized→Persisted 推进失败（步骤 6.5）。
    JournalPersistFailed { err: String },
    /// 投递 Ok 但 journal Persisted→Sent 推进失败（步骤 7）。
    SentAdvanceFailed { err: String },
    /// 投递 Ok 但 durable mark-sent 失败（步骤 7）。
    MarkDeliveredFailed,
    /// 投递超时（写可能已发生）。
    DeliveryTimeout,
    /// 通道不可达（可证明未投递 → 终局 Failed）。
    DeliveryUnreachable,
    /// 投递通用错误（写可能已发生）。
    DeliveryError { err: String },
    /// 回执帧解码失败（写已发生、结果未知）。
    ResponseDecodeFailed { err: String },
    /// ACK 已解码但 journal Sent→TransportAcked 推进失败。
    AckAdvanceFailed { err: String },
    /// 对端 Applied（两相成功，见 [`Settlement`] 写序）。
    Applied,
    /// Applied 但 durable 终态落盘失败（嵌套触发）。
    AppliedDurableFailed,
    /// Applied 但 journal AppliedConfirmed 推进失败（嵌套触发）。
    AppliedAdvanceFailed { err: String },
    /// 对端幂等命中（本进程不持原结果，绝不升级 Committed）。
    PeerIdempotentHit,
    /// 对端 dry-run（未写设备）。
    PeerDryRun,
    /// 对端拒绝。
    PeerDenied { class: u8 },
    /// 写可能已发生但读回不可用。
    ReadbackUnavailable,
    /// ACK 成功但读回不匹配。
    ReadbackMismatch,
    /// 对端执行失败。
    PeerFailed,
    /// 恢复对账终局（蓝图 75 WP-75.1.1，R16-B）：held saga 恢复路径的
    /// AppliedConfirmed 推进（ReadOnlyRecovery::applied_durable →
    /// `settle_saga_applied` 身份预检后收表）。见 settle 表
    /// RecoveredApplied 行注释（有意差异的裁决面）。
    RecoveredApplied,
}

/// 单笔 durable 写（结算表行内元素）。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SettleOp {
    /// journal 转移（Applied 行的 journal 写携带 sm3(回执) 摘要——
    /// 由 `settle_command` 的 Applied 分支供给；其余行摘要恒 None）。
    Journal {
        from: ExecutionStatus,
        to: ExecutionStatus,
    },
    /// durable intent 状态推进。
    Intent { state: DispatchState },
}

/// audit 文案（惰性渲染：states 调试串在编排点按终态拼接）。
#[derive(Debug)]
pub(crate) enum AuditText {
    /// 预格式文案（含动态 err/class 内容，不含 states）。
    Formatted(String),
    /// `states={states:?};{suffix}`（suffix 形如 `note=…`/`safety-reject=…`）。
    StatesSuffix(String),
    /// `states={states:?}`（Applied 成功终审）。
    States,
}

impl AuditText {
    pub(crate) fn render(&self, states: &[DispatchState]) -> String {
        match self {
            AuditText::Formatted(s) => s.clone(),
            AuditText::StatesSuffix(suffix) => alloc::format!("states={states:?};{suffix}"),
            AuditText::States => alloc::format!("states={states:?}"),
        }
    }
}

/// 回执形态（结算表第四元）。
#[derive(Debug, Clone)]
pub(crate) enum SettleReceipt {
    /// 成功收尾（调用侧做记账/爬坡基线/计数后回 Committed 回执）。
    Committed,
    /// Ok 回执 + 终态（Rejected/Failed）。
    Terminal {
        final_state: DispatchState,
        note: String,
    },
    /// Ok 回执 + Indeterminate（对账，不猜成败）。
    Indeterminate { note: String },
    /// `Err(DeliveryUnreachable)`。
    Unreachable,
    /// `Err(SafetyRejected)`。
    SafetyReject { reason: String },
    /// `Err(BudgetExceeded)`。
    BudgetExceeded { kind: &'static str },
    /// `Err(IntentPersistFailed)`。
    IntentPersistFailed { err: String },
}

/// 结算结局（触发 → 四元组：durable 写序 + audit + 回执形态）。
#[derive(Debug)]
pub(crate) struct Settlement {
    /// durable 写序（按序执行）。Applied 行 = `[Intent(Committed),
    /// Journal(TransportAcked→AppliedConfirmed)]`——durable 终态 ack
    /// 先行（61 v0.314.24 写序不变量的表内表达）。
    pub(crate) durable: Vec<SettleOp>,
    /// audit（None = 该触发现状不写 audit——rate/energy 预算拒绝）。
    pub(crate) audit: Option<(ExecOutcome, SafetyOutcome, AuditText)>,
    pub(crate) receipt: SettleReceipt,
}

/// 结算决策表（纯函数）：触发事件 → 结局。
///
/// 表级单测（`unit_tests::settle_table_*`）逐触发断言四元组与
/// 文案逐字节词形；新增触发事件只需本 match 加一臂。
pub(crate) fn settle(trigger: &SettleTrigger) -> Settlement {
    let failed_pass = (ExecOutcome::Failed, SafetyOutcome::Passed);
    let (durable, audit, receipt) = match trigger {
        SettleTrigger::SafetyReject { reason } => (
            alloc::vec![SettleOp::Journal {
                from: ExecutionStatus::Authorized,
                to: ExecutionStatus::Failed,
            }],
            Some((
                ExecOutcome::Failed,
                SafetyOutcome::Blocked,
                AuditText::StatesSuffix(alloc::format!("safety-reject={reason}")),
            )),
            SettleReceipt::SafetyReject {
                reason: reason.clone(),
            },
        ),
        SettleTrigger::BudgetRate => (
            alloc::vec![SettleOp::Journal {
                from: ExecutionStatus::Authorized,
                to: ExecutionStatus::Failed,
            }],
            None,
            SettleReceipt::BudgetExceeded { kind: "rate" },
        ),
        SettleTrigger::BudgetEnergy => (
            alloc::vec![SettleOp::Journal {
                from: ExecutionStatus::Authorized,
                to: ExecutionStatus::Failed,
            }],
            None,
            SettleReceipt::BudgetExceeded { kind: "energy" },
        ),
        SettleTrigger::IntentAppendFailed { err } => (
            alloc::vec![SettleOp::Journal {
                from: ExecutionStatus::Authorized,
                to: ExecutionStatus::Failed,
            }],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(alloc::format!(
                    "intent persist failed: {err}（零投递，终局 Failed）"
                )),
            )),
            SettleReceipt::IntentPersistFailed { err: err.clone() },
        ),
        SettleTrigger::JournalPersistFailed { err } => (
            alloc::vec![],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(alloc::format!(
                    "execution-journal Authorized→Persisted failed: {err}"
                )),
            )),
            SettleReceipt::IntentPersistFailed {
                err: alloc::format!("execution-journal Authorized→Persisted failed: {err}"),
            },
        ),
        SettleTrigger::SentAdvanceFailed { err } => (
            alloc::vec![SettleOp::Intent {
                state: DispatchState::Indeterminate,
            }],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(alloc::format!(
                    "delivery-ok 但 journal Sent 推进失败 {err}——对账"
                )),
            )),
            SettleReceipt::Indeterminate {
                note: alloc::format!("delivery-ok 但 journal Sent 推进失败 {err}——对账"),
            },
        ),
        SettleTrigger::MarkDeliveredFailed => (
            alloc::vec![],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(String::from("delivery-ok 但 durable mark-sent 失败——对账")),
            )),
            SettleReceipt::Indeterminate {
                note: String::from("delivery-ok 但 durable mark-sent 失败——对账"),
            },
        ),
        SettleTrigger::DeliveryTimeout => (
            alloc::vec![
                SettleOp::Journal {
                    from: ExecutionStatus::Persisted,
                    to: ExecutionStatus::Indeterminate,
                },
                SettleOp::Intent {
                    state: DispatchState::Indeterminate,
                },
            ],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(String::from(
                    "delivery-timeout → Indeterminate（先读回/查询状态，不盲重试）",
                )),
            )),
            SettleReceipt::Indeterminate {
                note: String::from("delivery-timeout：写可能已发生，需对账"),
            },
        ),
        SettleTrigger::DeliveryUnreachable => (
            alloc::vec![
                SettleOp::Journal {
                    from: ExecutionStatus::Persisted,
                    to: ExecutionStatus::Failed,
                },
                SettleOp::Intent {
                    state: DispatchState::Failed,
                },
            ],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(String::from("delivery-unreachable")),
            )),
            SettleReceipt::Unreachable,
        ),
        SettleTrigger::DeliveryError { err } => (
            alloc::vec![
                SettleOp::Journal {
                    from: ExecutionStatus::Persisted,
                    to: ExecutionStatus::Indeterminate,
                },
                SettleOp::Intent {
                    state: DispatchState::Indeterminate,
                },
            ],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(alloc::format!(
                    "delivery: {err} → Indeterminate（写可能已发生，对账）"
                )),
            )),
            SettleReceipt::Indeterminate {
                note: alloc::format!("delivery-error {err}：物理结果未知，需对账"),
            },
        ),
        SettleTrigger::ResponseDecodeFailed { err } => (
            alloc::vec![
                SettleOp::Journal {
                    from: ExecutionStatus::Sent,
                    to: ExecutionStatus::Indeterminate,
                },
                SettleOp::Intent {
                    state: DispatchState::Indeterminate,
                },
            ],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(alloc::format!(
                    "response-decode: {err} → Indeterminate（对账）"
                )),
            )),
            SettleReceipt::Indeterminate {
                note: String::from("response-decode-failed：物理结果未知，需对账"),
            },
        ),
        SettleTrigger::AckAdvanceFailed { err } => (
            alloc::vec![SettleOp::Intent {
                state: DispatchState::Indeterminate,
            }],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(alloc::format!(
                    "ack-decoded 但 journal TransportAcked 推进失败 {err}——对账"
                )),
            )),
            SettleReceipt::Indeterminate {
                note: alloc::format!("ack-decoded 但 journal TransportAcked 推进失败 {err}——对账"),
            },
        ),
        SettleTrigger::Applied => (
            // 写序不变量：durable 终态 ack 先行，成功后才
            // AppliedConfirmed → 外部 Committed（61 v0.314.24）。
            alloc::vec![
                SettleOp::Intent {
                    state: DispatchState::Committed,
                },
                SettleOp::Journal {
                    from: ExecutionStatus::TransportAcked,
                    to: ExecutionStatus::AppliedConfirmed,
                },
            ],
            Some((
                ExecOutcome::Succeeded,
                SafetyOutcome::Passed,
                AuditText::States,
            )),
            SettleReceipt::Committed,
        ),
        SettleTrigger::AppliedDurableFailed => (
            alloc::vec![SettleOp::Journal {
                from: ExecutionStatus::TransportAcked,
                to: ExecutionStatus::Indeterminate,
            }],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(String::from(
                    "applied-confirmed 但 durable 终态落盘失败——对账（不伪造成功）",
                )),
            )),
            SettleReceipt::Indeterminate {
                note: String::from("applied-confirmed 但 durable 终态落盘失败——对账（不伪造成功）"),
            },
        ),
        SettleTrigger::AppliedAdvanceFailed { err } => (
            alloc::vec![],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(alloc::format!(
                    "applied-confirmed 但 journal 推进失败 {err}——对账"
                )),
            )),
            SettleReceipt::Indeterminate {
                note: alloc::format!("applied-confirmed 但 journal 推进失败 {err}——对账"),
            },
        ),
        SettleTrigger::PeerIdempotentHit => (
            alloc::vec![
                SettleOp::Journal {
                    from: ExecutionStatus::TransportAcked,
                    to: ExecutionStatus::Reconciling,
                },
                SettleOp::Intent {
                    state: DispatchState::Indeterminate,
                },
            ],
            Some((
                failed_pass.0,
                failed_pass.1,
                AuditText::Formatted(String::from(
                    "对端幂等命中：原结果未知——对账，不升级 Committed",
                )),
            )),
            SettleReceipt::Indeterminate {
                note: String::from("对端幂等命中：原结果未知——对账，不升级 Committed"),
            },
        ),
        SettleTrigger::PeerDryRun => {
            let note = String::from("对端 dry-run（未写设备）");
            (
                alloc::vec![
                    SettleOp::Journal {
                        from: ExecutionStatus::TransportAcked,
                        to: ExecutionStatus::Rejected,
                    },
                    SettleOp::Intent {
                        state: DispatchState::Rejected,
                    },
                ],
                Some((
                    failed_pass.0,
                    failed_pass.1,
                    AuditText::StatesSuffix(alloc::format!("note={note}")),
                )),
                SettleReceipt::Terminal {
                    final_state: DispatchState::Rejected,
                    note,
                },
            )
        }
        SettleTrigger::PeerDenied { class } => {
            let note = alloc::format!("device-denied class={class}");
            (
                alloc::vec![
                    SettleOp::Journal {
                        from: ExecutionStatus::TransportAcked,
                        to: ExecutionStatus::Failed,
                    },
                    SettleOp::Intent {
                        state: DispatchState::Failed,
                    },
                ],
                Some((
                    failed_pass.0,
                    failed_pass.1,
                    AuditText::StatesSuffix(alloc::format!("note={note}")),
                )),
                SettleReceipt::Terminal {
                    final_state: DispatchState::Failed,
                    note,
                },
            )
        }
        SettleTrigger::ReadbackUnavailable => {
            let note = String::from("readback-unavailable：物理状态未确认，需对账");
            (
                alloc::vec![
                    SettleOp::Journal {
                        from: ExecutionStatus::TransportAcked,
                        to: ExecutionStatus::Indeterminate,
                    },
                    SettleOp::Intent {
                        state: DispatchState::Indeterminate,
                    },
                ],
                Some((
                    failed_pass.0,
                    failed_pass.1,
                    AuditText::StatesSuffix(alloc::format!("note={note}")),
                )),
                SettleReceipt::Indeterminate { note },
            )
        }
        SettleTrigger::ReadbackMismatch => {
            let note = String::from("readback-mismatch：ACK 成功但物理状态未达目标");
            (
                alloc::vec![
                    SettleOp::Journal {
                        from: ExecutionStatus::TransportAcked,
                        to: ExecutionStatus::Failed,
                    },
                    SettleOp::Intent {
                        state: DispatchState::Failed,
                    },
                ],
                Some((
                    ExecOutcome::Failed,
                    SafetyOutcome::Blocked,
                    AuditText::StatesSuffix(alloc::format!("note={note}")),
                )),
                SettleReceipt::Terminal {
                    final_state: DispatchState::Failed,
                    note,
                },
            )
        }
        SettleTrigger::PeerFailed => {
            let note = String::from("device-exec-failed");
            (
                alloc::vec![
                    SettleOp::Journal {
                        from: ExecutionStatus::TransportAcked,
                        to: ExecutionStatus::Failed,
                    },
                    SettleOp::Intent {
                        state: DispatchState::Failed,
                    },
                ],
                Some((
                    failed_pass.0,
                    failed_pass.1,
                    AuditText::StatesSuffix(alloc::format!("note={note}")),
                )),
                SettleReceipt::Terminal {
                    final_state: DispatchState::Failed,
                    note,
                },
            )
        }
        // R16-B（蓝图 75 WP-75.1.1）：恢复对账终局表行——held saga 已持久
        // 两份独立读回证据（ReadOnlyRecovery 路径），本行把 journal/intent
        // 推进到终局。**有意差异在此裁决**（原手写路径无注释、无测试说明
        // 是否有意）：
        // - 无回执摘要（receipt=Committed 无 note）——丢失的 ACK 就保持
        //   丢失，不制造 transport 摘要（原 :653-654 注释语义）；
        // - audit = None，**不触发 gate.record / 不增 committed_count /
        //   不推 states**（恢复不重复记账——正常 Applied 行的记账面不在
        //   本行重放；差异锁测试 typed_chain
        //   `recovered_applied_settle_does_not_double_book` 防回潮）。
        // 条件转移 →Reconciling（journal 记录非 Reconciling 时先归一）是
        // 表行表达不了的预检归一化，留在调用方
        // `held_engine::settle_saga_applied`。
        SettleTrigger::RecoveredApplied => (
            // 写序与正常 Applied 同律：durable 终态 ack 先行，成功后才
            // journal AppliedConfirmed（61 v0.314.24 写序不变量的表内
            // 表达；与原手写序 :650-668 逐字一致）。
            alloc::vec![
                SettleOp::Intent {
                    state: DispatchState::Committed,
                },
                SettleOp::Journal {
                    from: ExecutionStatus::Reconciling,
                    to: ExecutionStatus::AppliedConfirmed,
                },
            ],
            None,
            SettleReceipt::Committed,
        ),
    };
    Settlement {
        durable,
        audit,
        receipt,
    }
}

/// stage 推进（流程继续型 durable 写，区别于终局结算）。
#[derive(Debug, Clone, Copy)]
pub(crate) enum SettleStage {
    /// journal Authorized→Persisted（步骤 6.5）。
    JournalPersisted,
    /// journal Persisted→Sent（投递 Ok 后，步骤 7）。
    JournalSent,
    /// durable intent → Delivered（步骤 7）。
    IntentDelivered,
    /// journal Sent→TransportAcked（ACK 解码后，步骤 8）。
    JournalTransportAcked,
}

/// 结算不变参（蓝图 63 WP-63.1.3）：一次命令结算路径上**不随触发变化**
/// 的上下文——`started`/`command_id`/`correlation`/`energy`/
/// `setpoint_kw`/`response`。`states` 是唯一随臂推进的可变参，仍逐调用
/// 传 `&mut`。
///
/// `response` 两段式（非变异）：投递帧未到位（前置结算）→ `None`；
/// 帧解码后调用点以 `Some(&帧)` 重建 ctx 拷贝。Copy 语义——绝不在
/// 编排点内原地改 ctx。
#[derive(Clone, Copy)]
pub(crate) struct ExecutionCtx<'a> {
    pub(crate) started: u64,
    pub(crate) command_id: [u8; 16],
    pub(crate) correlation: [u8; 16],
    /// 对端回执原文（Applied 行 sm3 摘要供给；前置结算 None）。
    pub(crate) response: Option<&'a [u8]>,
    /// 记账能量（kWh；前置结算路径 0.0——与 execute_core 现状一致）。
    pub(crate) energy: f64,
    /// 连续 setpoint 值（AppliedConfirmed 时更新爬坡基线；离散 None）。
    pub(crate) setpoint_kw: Option<f64>,
    /// 蓝图 70 R10：命令语义目标 (device, point)——AppliedConfirmed 爬坡
    /// 基线按键（多资产各键独立；单资产形态 = 旧 (profile.device,profile.point)
    /// 逐字节一致）。
    pub(crate) target: (u32, u32),
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;
    use crate::dispatch::DispatchState;
    use crate::journal::ExecutionStatus;
    // ===== 62 v0.315.0 WP-62.1.3 结算决策表表级单测 =====
    //
    // 逐触发断言四元组（durable 写序 / audit 形态与逐字节文案 / 回执形态）。
    // 新增触发事件 = SettleTrigger 一臂 + settle 一臂 + 本节一用例。

    /// ACK 后典型状态路径（OutcomeCode 各臂的 audit states 调试串来源）。
    fn ack_states() -> Vec<DispatchState> {
        alloc::vec![
            DispatchState::Persisted,
            DispatchState::Delivered,
            DispatchState::TransportAcked,
        ]
    }

    /// IntentAppendFailed / JournalPersistFailed 触发点的真实流程位
    /// （execute_core：Authorized/Reserved 已推、Persisted 未推——pre-
    /// persist 阶段）。夹具须与真实位一致，否则未来该触发改 StatesSuffix
    /// 形态时测试夹具会掩盖词形漂移（62 复查 62.1-B 收口）。
    fn pre_persist_states() -> Vec<DispatchState> {
        alloc::vec![
            DispatchState::Proposed,
            DispatchState::Validated,
            DispatchState::Authorized,
            DispatchState::Reserved,
        ]
    }

    fn audit_of(s: &Settlement, states: &[DispatchState]) -> (ExecOutcome, SafetyOutcome, String) {
        let (exec, safety, text) = s.audit.as_ref().expect("audit 存在");
        (*exec, *safety, text.render(states))
    }

    fn journal_op(from: ExecutionStatus, to: ExecutionStatus) -> SettleOp {
        SettleOp::Journal { from, to }
    }

    fn intent_op(state: DispatchState) -> SettleOp {
        SettleOp::Intent { state }
    }

    #[test]
    fn settle_table_safety_reject() {
        let states = alloc::vec![DispatchState::Authorized];
        let s = settle(&SettleTrigger::SafetyReject {
            reason: String::from("ov"),
        });
        assert_eq!(
            s.durable,
            alloc::vec![journal_op(
                ExecutionStatus::Authorized,
                ExecutionStatus::Failed
            )]
        );
        let (exec, safety, text) = audit_of(&s, &states);
        assert!(matches!(exec, ExecOutcome::Failed));
        assert!(matches!(safety, SafetyOutcome::Blocked));
        assert_eq!(text, format!("states={states:?};safety-reject=ov"));
        assert!(matches!(s.receipt, SettleReceipt::SafetyReject { .. }));
    }

    #[test]
    fn settle_table_budget_rate_no_audit() {
        let s = settle(&SettleTrigger::BudgetRate);
        assert_eq!(
            s.durable,
            alloc::vec![journal_op(
                ExecutionStatus::Authorized,
                ExecutionStatus::Failed
            )]
        );
        assert!(s.audit.is_none(), "rate 预算拒绝现状无 audit");
        assert!(matches!(
            s.receipt,
            SettleReceipt::BudgetExceeded { kind: "rate" }
        ));
    }

    #[test]
    fn settle_table_budget_energy_no_audit() {
        let s = settle(&SettleTrigger::BudgetEnergy);
        assert_eq!(
            s.durable,
            alloc::vec![journal_op(
                ExecutionStatus::Authorized,
                ExecutionStatus::Failed
            )]
        );
        assert!(s.audit.is_none(), "energy 预算拒绝现状无 audit");
        assert!(matches!(
            s.receipt,
            SettleReceipt::BudgetExceeded { kind: "energy" }
        ));
    }

    #[test]
    fn settle_table_intent_append_failed() {
        let s = settle(&SettleTrigger::IntentAppendFailed {
            err: String::from("DiskFull"),
        });
        assert_eq!(
            s.durable,
            alloc::vec![journal_op(
                ExecutionStatus::Authorized,
                ExecutionStatus::Failed
            )]
        );
        let (exec, safety, text) = audit_of(&s, &pre_persist_states());
        assert!(matches!(exec, ExecOutcome::Failed));
        assert!(matches!(safety, SafetyOutcome::Passed));
        assert_eq!(
            text,
            "intent persist failed: DiskFull（零投递，终局 Failed）"
        );
        assert!(matches!(
            s.receipt,
            SettleReceipt::IntentPersistFailed { .. }
        ));
    }

    #[test]
    fn settle_table_journal_persist_failed() {
        let s = settle(&SettleTrigger::JournalPersistFailed {
            err: String::from("NotAuthorized"),
        });
        assert!(s.durable.is_empty(), "推进已试败，表内无补偿写");
        let (_, _, text) = audit_of(&s, &pre_persist_states());
        assert_eq!(
            text,
            "execution-journal Authorized→Persisted failed: NotAuthorized"
        );
        assert!(matches!(
            s.receipt,
            SettleReceipt::IntentPersistFailed { .. }
        ));
    }

    #[test]
    fn settle_table_sent_advance_failed() {
        let states = ack_states();
        let s = settle(&SettleTrigger::SentAdvanceFailed {
            err: String::from("NotAuthorized"),
        });
        assert_eq!(
            s.durable,
            alloc::vec![intent_op(DispatchState::Indeterminate)]
        );
        let (_, _, text) = audit_of(&s, &states);
        assert_eq!(
            text,
            "delivery-ok 但 journal Sent 推进失败 NotAuthorized——对账"
        );
        assert!(matches!(
            s.receipt,
            SettleReceipt::Indeterminate { ref note }
                if note == "delivery-ok 但 journal Sent 推进失败 NotAuthorized——对账"
        ));
    }

    #[test]
    fn settle_table_mark_delivered_failed() {
        let s = settle(&SettleTrigger::MarkDeliveredFailed);
        assert!(s.durable.is_empty());
        let (_, _, text) = audit_of(&s, &ack_states());
        assert_eq!(text, "delivery-ok 但 durable mark-sent 失败——对账");
        assert!(matches!(s.receipt, SettleReceipt::Indeterminate { .. }));
    }

    #[test]
    fn settle_table_delivery_timeout() {
        let s = settle(&SettleTrigger::DeliveryTimeout);
        assert_eq!(
            s.durable,
            alloc::vec![
                journal_op(ExecutionStatus::Persisted, ExecutionStatus::Indeterminate),
                intent_op(DispatchState::Indeterminate),
            ]
        );
        let (_, _, text) = audit_of(&s, &ack_states());
        assert_eq!(
            text,
            "delivery-timeout → Indeterminate（先读回/查询状态，不盲重试）"
        );
        assert!(matches!(
            s.receipt,
            SettleReceipt::Indeterminate { ref note }
                if note == "delivery-timeout：写可能已发生，需对账"
        ));
    }

    #[test]
    fn settle_table_delivery_unreachable() {
        let s = settle(&SettleTrigger::DeliveryUnreachable);
        assert_eq!(
            s.durable,
            alloc::vec![
                journal_op(ExecutionStatus::Persisted, ExecutionStatus::Failed),
                intent_op(DispatchState::Failed),
            ]
        );
        let (_, _, text) = audit_of(&s, &ack_states());
        assert_eq!(text, "delivery-unreachable");
        assert!(matches!(s.receipt, SettleReceipt::Unreachable));
    }

    #[test]
    fn settle_table_delivery_error() {
        let states = ack_states();
        let s = settle(&SettleTrigger::DeliveryError {
            err: String::from("Protocol(\"x\")"),
        });
        assert_eq!(
            s.durable,
            alloc::vec![
                journal_op(ExecutionStatus::Persisted, ExecutionStatus::Indeterminate),
                intent_op(DispatchState::Indeterminate),
            ]
        );
        let (_, _, text) = audit_of(&s, &states);
        assert_eq!(
            text,
            "delivery: Protocol(\"x\") → Indeterminate（写可能已发生，对账）"
        );
        assert!(matches!(
            s.receipt,
            SettleReceipt::Indeterminate { ref note }
                if note == "delivery-error Protocol(\"x\")：物理结果未知，需对账"
        ));
    }

    #[test]
    fn settle_table_response_decode_failed() {
        let s = settle(&SettleTrigger::ResponseDecodeFailed {
            err: String::from("Truncated"),
        });
        assert_eq!(
            s.durable,
            alloc::vec![
                journal_op(ExecutionStatus::Sent, ExecutionStatus::Indeterminate),
                intent_op(DispatchState::Indeterminate),
            ]
        );
        let (_, _, text) = audit_of(&s, &ack_states());
        assert_eq!(text, "response-decode: Truncated → Indeterminate（对账）");
        assert!(matches!(
            s.receipt,
            SettleReceipt::Indeterminate { ref note }
                if note == "response-decode-failed：物理结果未知，需对账"
        ));
    }

    #[test]
    fn settle_table_ack_advance_failed() {
        let s = settle(&SettleTrigger::AckAdvanceFailed {
            err: String::from("NotAuthorized"),
        });
        assert_eq!(
            s.durable,
            alloc::vec![intent_op(DispatchState::Indeterminate)]
        );
        let (_, _, text) = audit_of(&s, &ack_states());
        assert_eq!(
            text,
            "ack-decoded 但 journal TransportAcked 推进失败 NotAuthorized——对账"
        );
        assert!(matches!(s.receipt, SettleReceipt::Indeterminate { .. }));
    }

    #[test]
    fn settle_table_applied_write_order_invariant() {
        // 写序不变量：durable 终态 ack 先行，成功后才 journal AppliedConfirmed。
        let s = settle(&SettleTrigger::Applied);
        assert_eq!(
            s.durable,
            alloc::vec![
                intent_op(DispatchState::Committed),
                journal_op(
                    ExecutionStatus::TransportAcked,
                    ExecutionStatus::AppliedConfirmed
                ),
            ]
        );
        let (exec, safety, text) = audit_of(&s, &ack_states());
        assert!(matches!(exec, ExecOutcome::Succeeded));
        assert!(matches!(safety, SafetyOutcome::Passed));
        assert_eq!(text, format!("states={:?}", ack_states()));
        assert!(matches!(s.receipt, SettleReceipt::Committed));
    }

    #[test]
    fn settle_table_applied_durable_failed() {
        let s = settle(&SettleTrigger::AppliedDurableFailed);
        assert_eq!(
            s.durable,
            alloc::vec![journal_op(
                ExecutionStatus::TransportAcked,
                ExecutionStatus::Indeterminate
            )]
        );
        let (_, _, text) = audit_of(&s, &ack_states());
        assert_eq!(
            text,
            "applied-confirmed 但 durable 终态落盘失败——对账（不伪造成功）"
        );
        assert!(matches!(s.receipt, SettleReceipt::Indeterminate { .. }));
    }

    #[test]
    fn settle_table_applied_advance_failed() {
        let s = settle(&SettleTrigger::AppliedAdvanceFailed {
            err: String::from("NotAuthorized"),
        });
        assert!(s.durable.is_empty());
        let (_, _, text) = audit_of(&s, &ack_states());
        assert_eq!(
            text,
            "applied-confirmed 但 journal 推进失败 NotAuthorized——对账"
        );
        assert!(matches!(s.receipt, SettleReceipt::Indeterminate { .. }));
    }

    #[test]
    fn settle_table_peer_idempotent_hit() {
        let s = settle(&SettleTrigger::PeerIdempotentHit);
        assert_eq!(
            s.durable,
            alloc::vec![
                journal_op(
                    ExecutionStatus::TransportAcked,
                    ExecutionStatus::Reconciling
                ),
                intent_op(DispatchState::Indeterminate),
            ]
        );
        let (_, _, text) = audit_of(&s, &ack_states());
        assert_eq!(text, "对端幂等命中：原结果未知——对账，不升级 Committed");
        assert!(matches!(
            s.receipt,
            SettleReceipt::Indeterminate { ref note }
                if note == "对端幂等命中：原结果未知——对账，不升级 Committed"
        ));
    }

    #[test]
    fn settle_table_peer_dry_run() {
        let states = ack_states();
        let s = settle(&SettleTrigger::PeerDryRun);
        assert_eq!(
            s.durable,
            alloc::vec![
                journal_op(ExecutionStatus::TransportAcked, ExecutionStatus::Rejected),
                intent_op(DispatchState::Rejected),
            ]
        );
        let (_, _, text) = audit_of(&s, &states);
        assert_eq!(
            text,
            format!("states={states:?};note=对端 dry-run（未写设备）")
        );
        assert!(matches!(
            s.receipt,
            SettleReceipt::Terminal {
                final_state: DispatchState::Rejected,
                ..
            }
        ));
    }

    #[test]
    fn settle_table_peer_denied() {
        let states = ack_states();
        let s = settle(&SettleTrigger::PeerDenied { class: 3 });
        assert_eq!(
            s.durable,
            alloc::vec![
                journal_op(ExecutionStatus::TransportAcked, ExecutionStatus::Failed),
                intent_op(DispatchState::Failed),
            ]
        );
        let (_, _, text) = audit_of(&s, &states);
        assert_eq!(
            text,
            format!("states={states:?};note=device-denied class=3")
        );
        assert!(matches!(
            s.receipt,
            SettleReceipt::Terminal {
                final_state: DispatchState::Failed,
                ..
            }
        ));
    }

    #[test]
    fn settle_table_readback_unavailable() {
        let states = ack_states();
        let s = settle(&SettleTrigger::ReadbackUnavailable);
        assert_eq!(
            s.durable,
            alloc::vec![
                journal_op(
                    ExecutionStatus::TransportAcked,
                    ExecutionStatus::Indeterminate
                ),
                intent_op(DispatchState::Indeterminate),
            ]
        );
        let (_, _, text) = audit_of(&s, &states);
        assert_eq!(
            text,
            format!("states={states:?};note=readback-unavailable：物理状态未确认，需对账")
        );
        assert!(matches!(s.receipt, SettleReceipt::Indeterminate { .. }));
    }

    #[test]
    fn settle_table_readback_mismatch_blocked() {
        let states = ack_states();
        let s = settle(&SettleTrigger::ReadbackMismatch);
        assert_eq!(
            s.durable,
            alloc::vec![
                journal_op(ExecutionStatus::TransportAcked, ExecutionStatus::Failed),
                intent_op(DispatchState::Failed),
            ]
        );
        let (exec, safety, text) = audit_of(&s, &states);
        assert!(matches!(exec, ExecOutcome::Failed));
        assert!(
            matches!(safety, SafetyOutcome::Blocked),
            "读回不符必须 Blocked"
        );
        assert_eq!(
            text,
            format!("states={states:?};note=readback-mismatch：ACK 成功但物理状态未达目标")
        );
        assert!(matches!(
            s.receipt,
            SettleReceipt::Terminal {
                final_state: DispatchState::Failed,
                ..
            }
        ));
    }

    #[test]
    fn settle_table_peer_failed() {
        let states = ack_states();
        let s = settle(&SettleTrigger::PeerFailed);
        assert_eq!(
            s.durable,
            alloc::vec![
                journal_op(ExecutionStatus::TransportAcked, ExecutionStatus::Failed),
                intent_op(DispatchState::Failed),
            ]
        );
        let (_, _, text) = audit_of(&s, &states);
        assert_eq!(text, format!("states={states:?};note=device-exec-failed"));
        assert!(matches!(
            s.receipt,
            SettleReceipt::Terminal {
                final_state: DispatchState::Failed,
                ..
            }
        ));
    }

    #[test]
    fn settle_table_recovered_applied() {
        // R16-B（蓝图 75 WP-75.1.1）：恢复对账终局表行——写序同 Applied
        // 两相（durable ack 先行）；audit=None、receipt 无摘要（有意差异
        // 见表行注释：恢复不重复记账，丢失的 ACK 保持丢失）。
        let s = settle(&SettleTrigger::RecoveredApplied);
        assert_eq!(
            s.durable,
            alloc::vec![
                intent_op(DispatchState::Committed),
                journal_op(
                    ExecutionStatus::Reconciling,
                    ExecutionStatus::AppliedConfirmed
                ),
            ]
        );
        assert!(s.audit.is_none(), "恢复终局无 audit（差异显式化）");
        assert!(
            matches!(s.receipt, SettleReceipt::Committed),
            "receipt=Committed 无摘要（调用方丢弃，纯内部携带）"
        );
    }
}
