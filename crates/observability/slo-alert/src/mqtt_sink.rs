//! 22 §2.W4.4：告警通知真出口——MQTT sink。
//!
//! `NotificationSink` 的注入闭包在此接真 MQTT 通道（21-B B2 总线）：
//! 告警经 topic `eneros/{device}/alerts` 发布（与 W4.1 metrics 同契约）。
//! payload 为轻量 `key=value` 行（no_std + alloc，无 serde 依赖）；
//! 发送失败返回 `Err(())`——触发引擎保留状态、下一轮重试（trigger.rs:22-29
//! 契约），并累计失败计数供指标观测。

use alloc::format;
use alloc::string::String;

use crate::trigger::{NotificationSink, TriggeredAlert};

/// 告警 MQTT sink（真出口：`eneros/{device}/alerts`）。
#[allow(clippy::result_unit_err)]
pub struct MqttAlertSink<F> {
    /// 设备标识（topic 前缀）。
    device_id: String,
    /// 发送闭包（真 MQTT publish；Err = 网络断/通道不可用）。
    send_fn: F,
    /// 成功发送计数。
    sent: u64,
    /// 失败计数（指标：`alerts.sink.failed`）。
    failed: u64,
}

impl<F> MqttAlertSink<F>
where
    F: FnMut(&str, &str) -> Result<(), ()>,
{
    /// 新建告警 sink（`send_fn` 与 W4.1 `MqttMetricsExporter` 同契约，
    /// 可共用同一 MQTT publish 闭包）。
    pub fn new(device_id: &str, send_fn: F) -> Self {
        Self {
            device_id: String::from(device_id),
            send_fn,
            sent: 0,
            failed: 0,
        }
    }

    /// alerts topic（`eneros/{device}/alerts`）。
    pub fn alerts_topic(&self) -> String {
        format!("eneros/{}/alerts", self.device_id)
    }

    /// 成功发送计数。
    pub fn sent(&self) -> u64 {
        self.sent
    }

    /// 失败计数（网络断/通道不可用）。
    pub fn failed(&self) -> u64 {
        self.failed
    }
}

impl<F> NotificationSink for MqttAlertSink<F>
where
    F: FnMut(&str, &str) -> Result<(), ()>,
{
    fn notify(&mut self, alert: &TriggeredAlert) -> Result<(), ()> {
        let payload = format!(
            "id={} state={:?} value_ns={} threshold_ns={} at_unix={}",
            alert.id, alert.state, alert.value_ns, alert.threshold_ns, alert.at_unix
        );
        let topic = self.alerts_topic();
        match (self.send_fn)(&topic, &payload) {
            Ok(()) => {
                self.sent += 1;
                Ok(())
            }
            Err(()) => {
                self.failed += 1;
                Err(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use core::cell::RefCell;

    use crate::trigger::{AlertState, TriggerEngine};

    use super::*;

    /// 收集发送的闭包（模拟真 MQTT broker）。
    #[allow(clippy::type_complexity)]
    fn collector() -> (
        std::rc::Rc<RefCell<Vec<(String, String)>>>,
        impl FnMut(&str, &str) -> Result<(), ()>,
    ) {
        let state: std::rc::Rc<RefCell<Vec<(String, String)>>> =
            std::rc::Rc::new(RefCell::new(Vec::new()));
        let s2 = std::rc::Rc::clone(&state);
        let send = move |topic: &str, payload: &str| {
            s2.borrow_mut()
                .push((topic.to_string(), payload.to_string()));
            Ok(())
        };
        (state, send)
    }

    /// 验收：SLO 触发的告警真经 MQTT 发出——topic 与 payload 契约。
    #[test]
    fn test_alert_published_via_mqtt() {
        let (sent, send) = collector();
        let sink = MqttAlertSink::new("dev-1", send);
        let mut engine = TriggerEngine::new(sink);
        // Warning 转换 → 通知。
        assert!(engine.evaluate("ipc", 3_000_000, 2_000_000, 5_000_000, 1000));
        let s = sent.borrow();
        assert_eq!(s.len(), 1, "告警必须发出 1 条");
        assert_eq!(s[0].0, "eneros/dev-1/alerts", "topic 契约");
        assert!(s[0].1.contains("state=Warning"), "payload 含状态: {}", s[0].1);
        assert!(s[0].1.contains("id=ipc-1"), "payload 含告警 id");
        assert!(s[0].1.contains("value_ns=3000000"), "payload 含实测值");
    }

    /// 验收：发送失败 → Err + 失败计数；状态机仍推进（trigger 契约）。
    #[test]
    fn test_send_failure_counts_and_advances() {
        let fail = |_t: &str, _p: &str| -> Result<(), ()> { Err(()) };
        let mut sink = MqttAlertSink::new("dev-2", fail);
        assert!(sink.notify(&TriggeredAlert {
            id: String::from("ipc-1"),
            state: AlertState::Critical,
            value_ns: 6_000_000,
            threshold_ns: 5_000_000,
            at_unix: 1000,
        })
        .is_err());
        assert_eq!(sink.failed(), 1);
        assert_eq!(sink.sent(), 0);

        // 与 TriggerEngine 组合：通知失败不阻断状态机。
        let mut engine = TriggerEngine::new(MqttAlertSink::new("dev-2", |_t, _p| Err(())));
        engine.evaluate("ipc", 6_000_000, 2_000_000, 5_000_000, 1000);
        assert_eq!(engine.state(), AlertState::Critical);
    }

    /// 验收：恢复通知也经 MQTT 发出（运维侧能收到恢复事件）。
    #[test]
    fn test_recovery_published() {
        let (sent, send) = collector();
        let sink = MqttAlertSink::new("dev-3", send);
        let mut engine = TriggerEngine::new(sink);
        engine.evaluate("ipc", 3_000_000, 2_000_000, 5_000_000, 1000);
        assert!(engine.evaluate("ipc", 100_000, 2_000_000, 5_000_000, 2000));
        let s = sent.borrow();
        assert_eq!(s.len(), 2);
        assert!(s[1].1.contains("state=Normal"), "恢复事件: {}", s[1].1);
        assert_eq!(engine.state(), AlertState::Normal);
    }
}
