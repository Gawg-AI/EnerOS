//! SLO 告警触发引擎（v0.241.0，B5-04）。
//!
//! - [`TriggerEngine`] — 阈值求值 + 告警状态机（Normal → Warning →
//!   Critical → 恢复），防重复告警（状态转换才通知）；
//! - [`NotificationSink`] — 通知出口抽象（注入闭包：上报/日志/外部
//!   通知通道）。

use alloc::string::String;

/// 告警状态机状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertState {
    /// 正常（无告警）。
    Normal,
    /// 警告（超 p99 阈值）。
    Warning,
    /// 严重（超 p999 阈值）。
    Critical,
}

/// 通知出口。
pub trait NotificationSink {
    /// 发送告警通知。
    ///
    /// `Err(())` 表示投递失败（触发引擎将保留告警状态并在下一轮重试）；
    /// 无错误载荷是刻意设计——通知通道失败原因由注入实现记录。
    #[allow(clippy::result_unit_err)]
    fn notify(&mut self, alert: &TriggeredAlert) -> Result<(), ()>;
}

/// 已触发告警。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggeredAlert {
    /// 告警 ID（baseline id + 编号）。
    pub id: String,
    /// 状态。
    pub state: AlertState,
    /// 实测值（ns）。
    pub value_ns: u64,
    /// 阈值（ns）。
    pub threshold_ns: u64,
    /// 触发时间（Unix 秒）。
    pub at_unix: u64,
}

/// 触发引擎：阈值求值 + 状态机。
///
/// 每次 `evaluate` 输入实测延迟（ns），按基线 p99/p999 阈值分级；
/// 仅在状态转换时发出通知（Normal→Warning→Critical 或恢复），
/// 避免重复告警风暴。
pub struct TriggerEngine<S> {
    /// 通知出口。
    sink: S,
    /// 当前状态。
    state: AlertState,
    /// 累计触发次数。
    triggers: u64,
}

impl<S: NotificationSink> TriggerEngine<S> {
    /// 创建触发引擎。
    pub fn new(sink: S) -> Self {
        Self {
            sink,
            state: AlertState::Normal,
            triggers: 0,
        }
    }

    /// 求值一次实测延迟。
    ///
    /// `baseline_id` 用于构造告警 ID；返回本次是否触发了通知
    ///（仅状态转换时）。
    pub fn evaluate(
        &mut self,
        baseline_id: &str,
        value_ns: u64,
        p99_ns: u64,
        p999_ns: u64,
        now_unix: u64,
    ) -> bool {
        let next = if value_ns > p999_ns {
            AlertState::Critical
        } else if value_ns > p99_ns {
            AlertState::Warning
        } else {
            AlertState::Normal
        };
        if next == self.state {
            return false; // 无状态转换：不通知（防重复）
        }
        self.state = next;
        self.triggers += 1;
        let threshold = match next {
            AlertState::Critical => p999_ns,
            AlertState::Warning => p99_ns,
            AlertState::Normal => 0,
        };
        let alert = TriggeredAlert {
            id: alloc::format!("{}-{}", baseline_id, self.triggers),
            state: next,
            value_ns,
            threshold_ns: threshold,
            at_unix: now_unix,
        };
        self.sink.notify(&alert).is_ok()
    }

    /// 当前状态。
    pub fn state(&self) -> AlertState {
        self.state
    }

    /// 累计触发次数。
    pub fn trigger_count(&self) -> u64 {
        self.triggers
    }
}

/// 闭包通知出口（注入真实上报通道）。
pub struct FnNotificationSink<F> {
    /// 通知闭包。
    notify_fn: F,
}

impl<F> FnNotificationSink<F>
where
    F: FnMut(&TriggeredAlert) -> Result<(), ()>,
{
    /// 创建闭包通知出口。
    pub fn new(notify_fn: F) -> Self {
        Self { notify_fn }
    }
}

impl<F> NotificationSink for FnNotificationSink<F>
where
    F: FnMut(&TriggeredAlert) -> Result<(), ()>,
{
    fn notify(&mut self, alert: &TriggeredAlert) -> Result<(), ()> {
        (self.notify_fn)(alert)
    }
}

// ============================================================================
// Tests（阈值求值 + 状态机 + 防重复）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn test_evaluate_normal_no_notify() {
        let notified: core::cell::RefCell<Vec<String>> = core::cell::RefCell::new(Vec::new());
        let sink = FnNotificationSink::new(|alert: &TriggeredAlert| {
            notified.borrow_mut().push(alert.id.clone());
            Ok(())
        });
        let mut engine = TriggerEngine::new(sink);
        // 初始 Normal → 无转换 → 无通知。
        assert!(!engine.evaluate("ipc", 100, 2_000_000, 5_000_000, 1000));
        assert_eq!(engine.trigger_count(), 0);
    }

    #[test]
    fn test_warning_transition_notifies() {
        let notified: core::cell::RefCell<Vec<String>> = core::cell::RefCell::new(Vec::new());
        let sink = FnNotificationSink::new(|alert: &TriggeredAlert| {
            notified.borrow_mut().push(alert.id.clone());
            Ok(())
        });
        let mut engine = TriggerEngine::new(sink);
        assert!(engine.evaluate("ipc", 3_000_000, 2_000_000, 5_000_000, 1000));
        assert_eq!(engine.state(), AlertState::Warning);
        assert_eq!(notified.borrow().len(), 1);
        // 持续超 p99 但未转 Critical → 不再通知（防重复）。
        assert!(!engine.evaluate("ipc", 3_500_000, 2_000_000, 5_000_000, 1001));
        assert_eq!(notified.borrow().len(), 1);
    }

    #[test]
    fn test_critical_escalation_notifies() {
        let notified: core::cell::RefCell<Vec<String>> = core::cell::RefCell::new(Vec::new());
        let sink = FnNotificationSink::new(|alert: &TriggeredAlert| {
            notified.borrow_mut().push(alert.id.clone());
            Ok(())
        });
        let mut engine = TriggerEngine::new(sink);
        engine.evaluate("ipc", 3_000_000, 2_000_000, 5_000_000, 1000);
        assert!(engine.evaluate("ipc", 6_000_000, 2_000_000, 5_000_000, 1001));
        assert_eq!(engine.state(), AlertState::Critical);
        assert_eq!(notified.borrow().len(), 2);
    }

    #[test]
    fn test_recovery_notifies() {
        let notified: core::cell::RefCell<Vec<String>> = core::cell::RefCell::new(Vec::new());
        let sink = FnNotificationSink::new(|alert: &TriggeredAlert| {
            notified.borrow_mut().push(alert.id.clone());
            Ok(())
        });
        let mut engine = TriggerEngine::new(sink);
        engine.evaluate("ipc", 3_000_000, 2_000_000, 5_000_000, 1000);
        // 恢复 → Normal 转换 → 通知。
        assert!(engine.evaluate("ipc", 100_000, 2_000_000, 5_000_000, 2000));
        assert_eq!(engine.state(), AlertState::Normal);
        assert_eq!(notified.borrow().len(), 2);
    }

    #[test]
    fn test_notify_error_swallowed_but_state_advances() {
        let mut engine = TriggerEngine::new(FnNotificationSink::new(|_a: &TriggeredAlert| Err(())));
        // 通知失败不阻断状态机。
        engine.evaluate("ipc", 3_000_000, 2_000_000, 5_000_000, 1000);
        assert_eq!(engine.state(), AlertState::Warning);
    }

    #[test]
    fn test_alert_id_sequence() {
        let notified: core::cell::RefCell<Vec<String>> = core::cell::RefCell::new(Vec::new());
        let sink = FnNotificationSink::new(|alert: &TriggeredAlert| {
            notified.borrow_mut().push(alert.id.clone());
            Ok(())
        });
        let mut engine = TriggerEngine::new(sink);
        engine.evaluate("ipc", 3_000_000, 2_000_000, 5_000_000, 1000);
        engine.evaluate("ipc", 6_000_000, 2_000_000, 5_000_000, 1001);
        let ids = notified.borrow();
        assert_eq!(ids[0], "ipc-1");
        assert_eq!(ids[1], "ipc-2");
    }
}
