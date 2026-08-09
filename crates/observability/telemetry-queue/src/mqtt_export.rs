//! 22 §2.W4.1：metrics/telemetry MQTT 导出器——真通道 + 背压。
//!
//! `OtlpExporter` 的注入闭包在此接**真 MQTT 通道**（21-B B2 总线）：
//! metrics 经 topic `eneros/{device}/metrics` 周期发布；网络断时进
//! **有界环形缓冲**（满 DropOldest + 计数），连上后 `flush_pending`
//! 回传（背压语义与 W3.3 同款，锁定 2）。

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;

/// MQTT 导出器（发送闭包注入：`FnMut(topic, payload)`）。
pub struct MqttMetricsExporter<F> {
    /// 设备标识（topic 前缀 `eneros/{device}/metrics`）。
    device_id: String,
    /// 发送闭包（真 MQTT publish；Err = 网络断/通道不可用）。
    send_fn: F,
    /// 有界缓冲（网络断时排队；容量配置化）。
    buffer: VecDeque<String>,
    /// 缓冲容量。
    capacity: usize,
    /// 丢弃计数（缓冲满 DropOldest；指标 `metrics.export.dropped`）。
    dropped: u64,
    /// 成功导出计数。
    exported: u64,
}

impl<F> MqttMetricsExporter<F>
where
    F: FnMut(&str, &str) -> Result<(), ()>,
{
    /// 新建（`capacity` = 断网缓冲上限；0 回退 1）。
    pub fn new(device_id: &str, send_fn: F, capacity: usize) -> Self {
        Self {
            device_id: String::from(device_id),
            send_fn,
            buffer: VecDeque::with_capacity(capacity.max(1)),
            capacity: capacity.max(1),
            dropped: 0,
            exported: 0,
        }
    }

    /// metrics topic（`eneros/{device}/metrics`）。
    pub fn metrics_topic(&self) -> String {
        format!("eneros/{}/metrics", self.device_id)
    }

    /// alerts topic（`eneros/{device}/alerts`；W4.4 告警 sink 共用）。
    pub fn alerts_topic(&self) -> String {
        format!("eneros/{}/alerts", self.device_id)
    }

    /// 导出一条 OTLP 行：先直发；失败（网络断）→ 进有界缓冲（满
    /// DropOldest + 计数）。返回是否直发成功。
    pub fn export(&mut self, line: &str) -> Result<(), ()> {
        let topic = self.metrics_topic();
        match (self.send_fn)(&topic, line) {
            Ok(()) => {
                self.exported += 1;
                Ok(())
            }
            Err(()) => {
                // 网络断：缓冲（有界；满 DropOldest 保最新）。
                if self.buffer.len() >= self.capacity {
                    self.buffer.pop_front();
                    self.dropped += 1;
                }
                self.buffer.push_back(String::from(line));
                Err(())
            }
        }
    }

    /// 连上后回传缓冲（网络恢复时调用；返回本次回传条数）。
    pub fn flush_pending(&mut self) -> usize {
        let mut flushed = 0;
        while let Some(line) = self.buffer.pop_front() {
            let topic = self.metrics_topic();
            match (self.send_fn)(&topic, &line) {
                Ok(()) => {
                    self.exported += 1;
                    flushed += 1;
                }
                Err(()) => {
                    // 又断了：放回队首（保序），下次再 flush。
                    self.buffer.push_front(line);
                    break;
                }
            }
        }
        flushed
    }

    /// 待回传条数（指标：`metrics.export.pending`）。
    pub fn pending(&self) -> usize {
        self.buffer.len()
    }

    /// 丢弃计数（指标：`metrics.export.dropped`）。
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// 成功导出计数。
    pub fn exported_count(&self) -> u64 {
        self.exported
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 收集发送的闭包（模拟真 MQTT broker）；返回 (共享收集器, 发送闭包)。
    fn collector() -> (
        std::rc::Rc<core::cell::RefCell<Vec<(String, String)>>>,
        impl FnMut(&str, &str) -> Result<(), ()>,
    ) {
        let state: std::rc::Rc<core::cell::RefCell<Vec<(String, String)>>> =
            std::rc::Rc::new(core::cell::RefCell::new(Vec::new()));
        let s2 = std::rc::Rc::clone(&state);
        let send = move |topic: &str, payload: &str| {
            s2.borrow_mut()
                .push((topic.to_string(), payload.to_string()));
            Ok(())
        };
        (state, send)
    }

    /// 验收：metrics 真能离开设备（MQTT 发布）——topic 与 payload 正确。
    #[test]
    fn test_export_publishes_to_mqtt() {
        let (sent, send) = collector();
        let mut ex = MqttMetricsExporter::new("dev-1", send, 64);
        ex.export("{\"metric\":1}").expect("直发成功");
        assert_eq!(ex.exported_count(), 1);
        assert_eq!(ex.pending(), 0);
        let s = sent.borrow();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].0, "eneros/dev-1/metrics");
        assert_eq!(s[0].1, "{\"metric\":1}");
    }

    /// 验收：网络断 → 缓冲（有界 + 丢弃计数）；连上 → flush 回传。
    #[test]
    fn test_network_break_buffers_and_flushes() {
        // 断网模拟：前 3 次失败，之后成功。
        let fail_count = core::cell::Cell::new(3);
        let send = move |_t: &str, _p: &str| -> Result<(), ()> {
            if fail_count.get() > 0 {
                fail_count.set(fail_count.get() - 1);
                Err(())
            } else {
                Ok(())
            }
        };
        let mut ex = MqttMetricsExporter::new("dev-2", send, 64);
        // 断网：3 条入缓冲。
        for i in 0..3 {
            assert!(ex.export(&format!("line{i}")).is_err());
        }
        assert_eq!(ex.pending(), 3);
        assert_eq!(ex.dropped(), 0);
        // 网络恢复：flush 回传 3 条。
        let flushed = ex.flush_pending();
        assert_eq!(flushed, 3);
        assert_eq!(ex.pending(), 0);
        assert_eq!(ex.exported_count(), 3);
    }

    /// 验收：缓冲满 DropOldest + 计数（背压不无界）。
    #[test]
    fn test_buffer_bounded_drop_oldest() {
        let fail = |_t: &str, _p: &str| -> Result<(), ()> { Err(()) };
        let mut ex = MqttMetricsExporter::new("dev-3", fail, 2);
        for i in 0..5 {
            let _ = ex.export(&format!("line{i}"));
        }
        assert_eq!(ex.pending(), 2, "缓冲必须有界");
        assert_eq!(ex.dropped(), 3, "满时 DropOldest + 计数");
    }

    /// 验收：alerts topic 契约（W4.4 共用）。
    #[test]
    fn test_alerts_topic() {
        let (_sent, send) = collector();
        let ex = MqttMetricsExporter::new("dev-4", send, 8);
        assert_eq!(ex.alerts_topic(), "eneros/dev-4/alerts");
    }
}
