//! 22 §2.W2.4：设备拒绝/异常结果处理——分类 + 策略 + 防无限重试。
//!
//! 决策系统收到设备拒绝（W2.2 的 `DualBrainError::DispatchError`）后：
//! ①分类（离线/越限/安全阻断/通信失败）→ ②选择策略（有界重试 with
//! 调整 / 降级 / 上报人工）→ ③绝不静默丢弃、绝不无限重试。
//! 每类拒绝计数入指标（`decision.rejected.<reason>` 源数据）。

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;

use crate::error::DualBrainError;

/// 拒绝原因分类（`decision.rejected.<reason>` 标签）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RejectionKind {
    /// 设备离线/不可达。
    Offline,
    /// 值越限（设备拒绝功率/越限）。
    OutOfRange,
    /// 安全阻断（ControlChain Unsafe / 安全校验 Fatal）。
    SafetyBlocked,
    /// 通信失败/超时。
    CommFailed,
}

impl RejectionKind {
    /// 指标标签（`decision.rejected.<label>`）。
    pub const fn metric_label(&self) -> &'static str {
        match self {
            Self::Offline => "offline",
            Self::OutOfRange => "out_of_range",
            Self::SafetyBlocked => "safety_blocked",
            Self::CommFailed => "comm_failed",
        }
    }
}

/// 拒绝处理策略（绝不含"无限重试"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectionAction {
    /// 有界重试（`attempt` = 第几次；调用方按退避后重发调整后的命令）。
    Retry { attempt: u32 },
    /// 降级（fast-path / 维持上次成功决策）。
    Degrade,
    /// 上报人工（重试预算耗尽或安全阻断——标记告警，不自动继续）。
    EscalateToHuman,
}

/// 拒绝处理器（每类计数 + 重试预算，防无限重试）。
#[derive(Debug, Clone)]
pub struct RejectionHandler {
    /// 每类拒绝计数（指标源：`decision.rejected.<label>`）。
    pub counts: [u64; 4],
    /// 重试预算（`target|reason` → 已重试次数；超预算 → EscalateToHuman）。
    retries: BTreeMap<String, u32>,
    /// 每目标最大重试次数（防无限重试）。
    pub max_retries_per_target: u32,
}

impl Default for RejectionHandler {
    fn default() -> Self {
        Self::new(3)
    }
}

impl RejectionHandler {
    /// 新建（`max_retries` = 每目标重试预算，默认 3）。
    pub fn new(max_retries: u32) -> Self {
        Self {
            counts: [0; 4],
            retries: BTreeMap::new(),
            max_retries_per_target: max_retries,
        }
    }

    /// 分类拒绝错误（`DispatchError` 载荷；无法分类 → CommFailed 兜底，
    /// fail-closed：不静默丢弃）。
    pub fn classify(err: &DualBrainError) -> RejectionKind {
        match err {
            DualBrainError::DispatchError(msg) => {
                if msg.contains("unsafe") || msg.contains("safety") {
                    RejectionKind::SafetyBlocked
                } else if msg.contains("Offline") || msg.contains("offline") {
                    RejectionKind::Offline
                } else if msg.contains("Rejected")
                    || msg.contains("over-limit")
                    || msg.contains("limit")
                    || msg.contains("越限")
                {
                    RejectionKind::OutOfRange
                } else {
                    // 网关拒绝/通信失败等 → CommFailed 兜底（不静默）。
                    RejectionKind::CommFailed
                }
            }
            DualBrainError::SafetyFatal(_) => RejectionKind::SafetyBlocked,
            _ => RejectionKind::CommFailed,
        }
    }

    /// 处理一次拒绝：计数 + 预算检查 → 策略。
    ///
    /// - SafetyBlocked：立即 `EscalateToHuman`（安全阻断不重试——重试
    ///   无意义且可能绕过安全语义）。
    /// - 其他：预算内 `Retry{attempt}`；超预算 → `EscalateToHuman`
    ///   （绝不无限重试）。
    pub fn handle(&mut self, err: &DualBrainError, target: &str) -> RejectionAction {
        let kind = Self::classify(err);
        self.counts[kind as usize] += 1;
        if kind == RejectionKind::SafetyBlocked {
            return RejectionAction::EscalateToHuman;
        }
        let key = format!("{target}|{}", kind.metric_label());
        let attempts = self.retries.entry(key).or_insert(0);
        *attempts += 1;
        if *attempts > self.max_retries_per_target {
            RejectionAction::EscalateToHuman
        } else {
            RejectionAction::Retry { attempt: *attempts }
        }
    }

    /// 重试成功/放弃后重置预算（调用方在成功或 Escalate 后调用）。
    pub fn reset_retries(&mut self, target: &str) {
        let keys: alloc::vec::Vec<String> = self
            .retries
            .keys()
            .filter(|k| k.starts_with(&format!("{target}|")))
            .cloned()
            .collect();
        for k in keys {
            self.retries.remove(&k);
        }
    }

    /// 拒绝总计数（指标汇总）。
    pub fn total_rejections(&self) -> u64 {
        self.counts.iter().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dispatch_err(kind: RejectionKind) -> DualBrainError {
        let msg = match kind {
            RejectionKind::Offline => "device-exec: Offline",
            RejectionKind::OutOfRange => "device rejected: over-limit",
            RejectionKind::SafetyBlocked => "unsafe: soc below minimum",
            RejectionKind::CommFailed => "gateway: NoPolicy",
        };
        DualBrainError::DispatchError(String::from(msg))
    }

    /// 四类拒绝各有独立计数与分类。
    #[test]
    fn test_four_rejection_kinds_classified_and_counted() {
        let mut h = RejectionHandler::new(3);
        for kind in [
            RejectionKind::Offline,
            RejectionKind::OutOfRange,
            RejectionKind::SafetyBlocked,
            RejectionKind::CommFailed,
        ] {
            let _ = h.handle(&dispatch_err(kind), "pcs");
        }
        assert_eq!(h.counts[RejectionKind::Offline as usize], 1);
        assert_eq!(h.counts[RejectionKind::OutOfRange as usize], 1);
        assert_eq!(h.counts[RejectionKind::SafetyBlocked as usize], 1);
        assert_eq!(h.counts[RejectionKind::CommFailed as usize], 1);
        assert_eq!(h.total_rejections(), 4);
    }

    /// 预算内重试；超预算 → EscalateToHuman（绝不无限重试）。
    #[test]
    fn test_retry_budget_prevents_infinite_retries() {
        let mut h = RejectionHandler::new(3);
        let err = dispatch_err(RejectionKind::Offline);
        for attempt in 1..=3 {
            assert_eq!(h.handle(&err, "pcs"), RejectionAction::Retry { attempt });
        }
        // 第 4 次（> 预算）→ 上报人工。
        assert_eq!(h.handle(&err, "pcs"), RejectionAction::EscalateToHuman);
        // 永不回退为无限重试。
        assert_eq!(h.handle(&err, "pcs"), RejectionAction::EscalateToHuman);
    }

    /// 安全阻断立即上报（不重试）。
    #[test]
    fn test_safety_blocked_escalates_immediately() {
        let mut h = RejectionHandler::new(3);
        let err = dispatch_err(RejectionKind::SafetyBlocked);
        assert_eq!(h.handle(&err, "pcs"), RejectionAction::EscalateToHuman);
        assert_eq!(
            h.counts[RejectionKind::SafetyBlocked as usize],
            1,
            "安全阻断必须计数（不静默）"
        );
    }

    /// 预算按目标隔离 + 重置。
    #[test]
    fn test_retry_budget_per_target_and_reset() {
        let mut h = RejectionHandler::new(2);
        let err = dispatch_err(RejectionKind::CommFailed);
        // 目标 A 用满预算 → Escalate；目标 B 独立预算。
        let _ = h.handle(&err, "pcs-a");
        let _ = h.handle(&err, "pcs-a");
        assert_eq!(h.handle(&err, "pcs-a"), RejectionAction::EscalateToHuman);
        assert_eq!(
            h.handle(&err, "pcs-b"),
            RejectionAction::Retry { attempt: 1 }
        );
        // 重置 A 后重新可重试（成功/人工介入后）。
        h.reset_retries("pcs-a");
        assert_eq!(
            h.handle(&err, "pcs-a"),
            RejectionAction::Retry { attempt: 1 }
        );
    }

    /// 未分类错误 → CommFailed 兜底（fail-closed 不静默）。
    #[test]
    fn test_unclassified_error_falls_back_to_comm() {
        let mut h = RejectionHandler::new(3);
        let err = DualBrainError::LlmError(String::from("some engine error"));
        let _ = h.handle(&err, "pcs");
        assert_eq!(h.counts[RejectionKind::CommFailed as usize], 1);
    }
}
