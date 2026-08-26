//! AI tick 生命周期策略单源（蓝图 57 WP-57.1，体检 C2 合核）。
//!
//! 历史：tick 编排的记账逻辑（连续错误预算/未知观测计数/升级裁决）只在
//! 进程内 [`EnergyAgent`](能量侧) 手写实现，EL0 上真正运行的
//! EnergyAgentService 薄副本没有任何预算语义——特性 WP 反复修补不被执行
//! 的模块。本模块把记账收拢为两侧共同消费的单源：
//!
//! - 富侧（energy-market-agent）：迁移后行为逐位保持（蓝图 57 D1）；
//! - 目标侧（control-services）：获得同源预算语义，熔断冷却为登记的
//!   行为增益（蓝图 57 D2）；学习钩子不入目标侧（wire 无诚实学习输入，
//!   蓝图 57 D3/A3）。

use alloc::string::String;

use crate::rejection::{RejectionAction, RejectionHandler, RejectionKind};

/// 一次 AI tick 的抽象结果——两侧 adapter 各自映射入本词汇，策略核心只认
/// 本词汇（进程内 typed 结果与 EIPC wire 回执在 adapter 层归一）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TickOutcome {
    /// 提交且回执 Committed（唯一可学习结果）。
    Committed,
    /// 提交成功但回执非 Committed（Failed/Indeterminate/Rejected）。
    DispatchRejected {
        /// 回执 note（分类输入）。
        note: String,
    },
    /// 前置失败（校验/授权/预算/持久化/不可达）：零投递。
    PreDispatchError {
        /// 错误详情（分类输入）。
        detail: String,
    },
    /// 决策端明确不动作（死区无动作/空调度/降级无基线）——审计但不下发。
    NoAction,
    /// 结构化拒绝（协调器裁决拒绝）。
    Rejected {
        /// 拒绝原因（分类输入）。
        reason: String,
    },
    /// 决策后端不可用（worker 掉线/加载失败）——计入失败预算（熔断后
    /// 冷却期跳过推理正是此场景的止损）。
    Unavailable,
    /// 可信输入缺失（遥测被评估器拒绝）——零推理零新写，不计失败预算，
    /// 只进未知观测计数（与富侧信任门路径一致）。
    NoTrustedInput,
}

/// 单次记账裁决。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickVerdict {
    /// 正常推进。
    Proceed,
    /// 进入错误态（安全阻断升级或连续错误达上限）——adapter 按各自恢复
    /// 路径处置（富侧：心跳 Dead；目标侧：熔断冷却，蓝图 57 D2）。
    Escalated,
}

/// 最近一次失败的分类与策略（adapter 诊断/审计面透传）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastRejection {
    /// 分类。
    pub kind: RejectionKind,
    /// 策略（Retry/Degrade/EscalateToHuman）。
    pub action: RejectionAction,
}

/// AI tick 错误预算/状态记账单源。
///
/// 内嵌 [`RejectionHandler`] 复用既有分类与每目标重试预算——不另造分类词
/// 汇（CONTEXT.md 同义词纪律）。
#[derive(Debug)]
pub struct TickPolicy {
    max_consecutive_errors: u32,
    consecutive_errors: u32,
    unknown_outcomes: u64,
    in_error: bool,
    last_rejection: Option<LastRejection>,
    rejections: RejectionHandler,
}

impl Default for TickPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl TickPolicy {
    /// 连续错误上限（32 AG-01：Transient 错误 N 次连续才转 Error——
    /// 承 energy-market-agent 原 MAX_CONSECUTIVE_ERRORS 常量）。
    pub const MAX_CONSECUTIVE_ERRORS: u32 = 3;

    /// 新建（内嵌默认 RejectionHandler，重试预算 3）。
    pub fn new() -> Self {
        Self::with_rejections(RejectionHandler::new(3))
    }

    /// 新建（注入既有 RejectionHandler——富侧迁移保留其原 handler 实例）。
    pub fn with_rejections(rejections: RejectionHandler) -> Self {
        Self {
            max_consecutive_errors: Self::MAX_CONSECUTIVE_ERRORS,
            consecutive_errors: 0,
            unknown_outcomes: 0,
            in_error: false,
            last_rejection: None,
            rejections,
        }
    }

    /// 记录一次 tick 结果 → 裁决。语义与富侧原手写实现逐位一致（D1）：
    /// - `Committed` / `NoAction`：复位连续错误、解除错误态；
    /// - `DispatchRejected` / `PreDispatchError` / `Rejected` /
    ///   `Unavailable`：分类入 RejectionHandler + 连续错误 +1；
    ///   EscalateToHuman 或达上限 → [`TickVerdict::Escalated`]；
    /// - `NoTrustedInput`：仅未知观测计数（零推理路径，不计失败）。
    pub fn record(&mut self, outcome: &TickOutcome) -> TickVerdict {
        match outcome {
            TickOutcome::Committed | TickOutcome::NoAction => {
                self.consecutive_errors = 0;
                self.in_error = false;
                TickVerdict::Proceed
            }
            TickOutcome::NoTrustedInput => {
                self.unknown_outcomes += 1;
                TickVerdict::Proceed
            }
            TickOutcome::DispatchRejected { note } => self.note_failure(note),
            TickOutcome::PreDispatchError { detail } => self.note_failure(detail),
            TickOutcome::Rejected { reason } => self.note_failure(reason),
            TickOutcome::Unavailable => self.note_failure("ai-backend-unavailable"),
        }
    }

    /// 可信门直接拒绝（评估器拒绝发生在构造 TickOutcome 之前的路径）——
    /// 仅未知计数，不影响预算。
    pub fn note_unknown(&mut self) {
        self.unknown_outcomes += 1;
    }

    /// 是否处于错误态（升级后为 true，直至 Committed/NoAction 复位）。
    pub fn in_error(&self) -> bool {
        self.in_error
    }

    /// 当前连续错误数。
    pub fn consecutive_errors(&self) -> u32 {
        self.consecutive_errors
    }

    /// 未知观测计数（NoTrustedInput 路径指标）。
    pub fn unknown_outcomes(&self) -> u64 {
        self.unknown_outcomes
    }

    /// 最近一次失败的分类与策略。
    pub fn last_rejection(&self) -> Option<LastRejection> {
        self.last_rejection
    }

    /// 内嵌拒绝处理器只读视图（诊断/指标面）。
    pub fn rejections(&self) -> &RejectionHandler {
        &self.rejections
    }

    /// 内嵌拒绝处理器可变访问（测试注入/预算运维面）。
    pub fn rejections_mut(&mut self) -> &mut RejectionHandler {
        &mut self.rejections
    }

    fn note_failure(&mut self, note: &str) -> TickVerdict {
        let err = crate::error::DualBrainError::DispatchError(String::from(note));
        let kind = RejectionHandler::classify(&err);
        let action = self.rejections.handle(&err, "pcs");
        self.last_rejection = Some(LastRejection { kind, action });
        self.consecutive_errors += 1;
        if action == RejectionAction::EscalateToHuman
            || self.consecutive_errors >= self.max_consecutive_errors
        {
            self.in_error = true;
            return TickVerdict::Escalated;
        }
        TickVerdict::Proceed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejected(note: &str) -> TickOutcome {
        TickOutcome::DispatchRejected {
            note: String::from(note),
        }
    }

    #[test]
    fn committed_and_no_action_reset_budget_and_error_state() {
        let mut p = TickPolicy::new();
        for _ in 0..TickPolicy::MAX_CONSECUTIVE_ERRORS {
            p.record(&rejected("device-denied"));
        }
        assert_eq!(p.record(&rejected("x")), TickVerdict::Escalated);
        assert!(p.in_error());
        assert_eq!(
            p.record(&TickOutcome::Committed),
            TickVerdict::Proceed,
            "Committed 复位"
        );
        assert!(!p.in_error());
        assert_eq!(p.consecutive_errors(), 0);
        // 升级后再 NoAction 同样复位（富侧 :346-358 语义）。
        for _ in 0..TickPolicy::MAX_CONSECUTIVE_ERRORS {
            p.record(&rejected("x"));
        }
        assert!(p.in_error());
        p.record(&TickOutcome::NoAction);
        assert!(!p.in_error());
    }

    #[test]
    fn consecutive_failures_escalate_at_limit() {
        let mut p = TickPolicy::new();
        assert_eq!(p.record(&rejected("a")), TickVerdict::Proceed);
        assert_eq!(p.record(&rejected("b")), TickVerdict::Proceed);
        assert_eq!(p.record(&rejected("c")), TickVerdict::Escalated);
        assert_eq!(p.consecutive_errors(), 3);
        // 未达上限前保持 Proceed（32 AG-01：Transient 容忍）。
        let mut q = TickPolicy::new();
        assert_eq!(q.record(&rejected("a")), TickVerdict::Proceed);
        q.record(&TickOutcome::Committed);
        assert_eq!(q.consecutive_errors(), 0, "成功打断连续序列");
    }

    #[test]
    fn safety_blocked_escalates_immediately() {
        let mut p = TickPolicy::new();
        let v = p.record(&rejected("unsafe: over safe_max"));
        assert_eq!(v, TickVerdict::Escalated, "SafetyBlocked 立即升级");
        let lr = p.last_rejection().expect("last_rejection");
        assert_eq!(lr.action, RejectionAction::EscalateToHuman);
    }

    #[test]
    fn unavailable_counts_toward_budget() {
        let mut p = TickPolicy::new();
        p.record(&TickOutcome::Unavailable);
        p.record(&TickOutcome::Unavailable);
        assert_eq!(
            p.record(&TickOutcome::Unavailable),
            TickVerdict::Escalated,
            "后端持续不可用必须进熔断（目标侧止损场景）"
        );
    }

    #[test]
    fn no_trusted_input_only_counts_unknowns() {
        let mut p = TickPolicy::new();
        p.record(&TickOutcome::NoTrustedInput);
        p.record(&TickOutcome::NoTrustedInput);
        p.note_unknown();
        assert_eq!(p.unknown_outcomes(), 3);
        assert_eq!(p.consecutive_errors(), 0, "信任门拒绝不计失败预算");
        assert!(!p.in_error());
    }

    #[test]
    fn pre_dispatch_error_and_rejected_are_failures() {
        let mut p = TickPolicy::new();
        p.record(&TickOutcome::PreDispatchError {
            detail: String::from("rate-limited"),
        });
        p.record(&TickOutcome::Rejected {
            reason: String::from("gate-rate-limited"),
        });
        assert_eq!(p.consecutive_errors(), 2);
        assert!(p.last_rejection().is_some());
    }
}
