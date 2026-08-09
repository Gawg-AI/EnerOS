//! 22 §2.G-R4：可运维闸门——远程能看/能管/能升级的端到端载体。
//!
//! 本 crate 是 **闸门证据** 而非产品模块：
//!
//! - [`MqttBroker`] — 模拟 MQTT broker（topic → FIFO 有界队列），
//!   publish/subscribe 闭包与 W4.1/4.2/4.4 通道同契约，把五条通道
//!   （metrics / alerts / cmd-resp / OTA / WS）串进同一运维会话；
//! - `tests/gr4_remote_ops.rs` — 完整运维会话：**能看**（metrics+告警）、
//!   **能管**（mgmt-api RPC 经 RBAC/限流/幂等）、**能升级**（OTA 回路）、
//!   **能看实时**（RFC 6455 WS 帧推送），全经同一 broker。
//!
//! 通过即满足 G-R4 判据「W4.1-W4.5 验收全 PASS + 远程能看/能管/能升级」。

#![no_std]

extern crate alloc;

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use core::cell::RefCell;

/// 模拟 MQTT broker：topic → FIFO 队列（有界，满 DropOldest——背压语义
/// 与 W3.3 同款；模拟真实 broker 的 QoS 0 语义）。
#[derive(Debug)]
pub struct MqttBroker {
    /// topic → 待消费消息（FIFO）。
    topics: RefCell<BTreeMap<String, VecDeque<String>>>,
    /// 每 topic 队列上限。
    capacity: usize,
    /// 丢弃计数。
    dropped: RefCell<u64>,
}

impl MqttBroker {
    /// 新建 broker（`capacity` = 每 topic 队列上限，0 回退 1）。
    pub fn new(capacity: usize) -> Self {
        Self {
            topics: RefCell::new(BTreeMap::new()),
            capacity: capacity.max(1),
            dropped: RefCell::new(0),
        }
    }

    /// 发布消息（模拟 broker 转发：入队）。
    pub fn publish(&self, topic: &str, payload: &str) -> Result<(), ()> {
        let mut topics = self.topics.borrow_mut();
        let q = topics.entry(String::from(topic)).or_default();
        if q.len() >= self.capacity {
            q.pop_front();
            *self.dropped.borrow_mut() += 1;
        }
        q.push_back(String::from(payload));
        Ok(())
    }

    /// 订阅读取（FIFO；空 → None=超时/无消息）。
    pub fn subscribe(&self, topic: &str) -> Option<String> {
        self.topics.borrow_mut().get_mut(topic)?.pop_front()
    }

    /// 某 topic 的未消费消息数。
    pub fn pending(&self, topic: &str) -> usize {
        self.topics.borrow().get(topic).map_or(0, VecDeque::len)
    }

    /// 丢弃计数（背压丢弃）。
    pub fn dropped(&self) -> u64 {
        *self.dropped.borrow()
    }

    /// 构造 publish 闭包（与 W4.1/4.2/4.4 通道契约一致，委托 `publish`）。
    pub fn publish_closure(&self) -> impl FnMut(&str, &str) -> Result<(), ()> + '_ {
        let this = self;
        move |t: &str, p: &str| this.publish(t, p)
    }

    /// 构造 subscribe 闭包（与 W4.2 mgmt-api 传输契约一致）。
    pub fn subscribe_closure(&self) -> impl FnMut(&str) -> Option<String> + '_ {
        let this = self;
        move |t: &str| this.subscribe(t)
    }
}

/// 运维会话：把五条通道挂到同一 broker 上（G-R4 证据载体）。
///
/// 设备侧通道（真代码）+ broker（模拟链路）组合；集成层将来用真
/// MQTT broker 替换 `MqttBroker` 即得完整远程运维栈。
pub struct RemoteOpsSession {
    /// broker（模拟链路）。
    pub broker: MqttBroker,
    /// 设备标识。
    pub device_id: String,
}

impl RemoteOpsSession {
    /// 新建运维会话（`device_id` 决定全部 topic 前缀）。
    pub fn new(device_id: &str) -> Self {
        Self {
            broker: MqttBroker::new(128),
            device_id: String::from(device_id),
        }
    }

    /// 设备侧 metrics 导出器（W4.1）挂到本会话。
    pub fn metrics_exporter(
        &self,
    ) -> eneros_telemetry_queue::mqtt_export::MqttMetricsExporter<
        impl FnMut(&str, &str) -> Result<(), ()> + use<'_>,
    > {
        let device = self.device_id.clone();
        let broker = &self.broker;
        eneros_telemetry_queue::mqtt_export::MqttMetricsExporter::new(
            &device,
            move |t: &str, p: &str| broker.publish(t, p),
            64,
        )
    }

    /// 设备侧告警 sink（W4.4）挂到本会话。
    pub fn alert_sink(
        &self,
    ) -> eneros_slo_alert::MqttAlertSink<impl FnMut(&str, &str) -> Result<(), ()> + use<'_>> {
        let device = self.device_id.clone();
        let broker = &self.broker;
        eneros_slo_alert::MqttAlertSink::new(&device, move |t: &str, p: &str| broker.publish(t, p))
    }

    /// 运维侧读取：metrics topic 行（能看）。
    pub fn take_metrics(&self) -> Option<String> {
        self.broker
            .subscribe(&alloc::format!("eneros/{}/metrics", self.device_id))
    }

    /// 运维侧读取：alerts topic 行（能看告警）。
    pub fn take_alert(&self) -> Option<String> {
        self.broker
            .subscribe(&alloc::format!("eneros/{}/alerts", self.device_id))
    }

    /// 运维侧读取：resp topic 行（远程命令响应）。
    pub fn take_response(&self) -> Option<String> {
        self.broker
            .subscribe(&alloc::format!("eneros/{}/resp", self.device_id))
    }
}

/// 设备侧 mgmt-api RPC 传输（W4.2）挂到会话 broker 上（能管）。
/// 返回类型复杂（双闭包），以 type alias 保持可读。
#[allow(clippy::type_complexity)]
pub fn attach_rpc_transport<'s>(
    session: &'s RemoteOpsSession,
) -> eneros_mgmt_api::transport::MqttRpcTransport<
    impl FnMut(&str, &str) -> Result<(), ()> + 's,
    impl FnMut(&str) -> Option<String> + 's,
> {
    let device = session.device_id.clone();
    let broker = &session.broker;
    let pub_fn = move |t: &str, p: &str| broker.publish(t, p);
    let sub_fn = move |t: &str| broker.subscribe(t);
    eneros_mgmt_api::transport::MqttRpcTransport::new(&device, pub_fn, sub_fn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broker_fifo_and_bounded() {
        let b = MqttBroker::new(2);
        for i in 0..5 {
            b.publish("t", &alloc::format!("m{i}")).unwrap();
        }
        assert_eq!(b.pending("t"), 2, "有界");
        assert_eq!(b.dropped(), 3, "DropOldest");
        assert_eq!(b.subscribe("t").as_deref(), Some("m3"), "保最新");
        assert_eq!(b.subscribe("t").as_deref(), Some("m4"));
        assert_eq!(b.subscribe("t"), None, "空队列");
    }

    #[test]
    fn session_topic_contracts() {
        use eneros_slo_alert::NotificationSink;
        let s = RemoteOpsSession::new("dev-x");
        s.metrics_exporter().export("row").unwrap();
        s.alert_sink().notify(&eneros_slo_alert::TriggeredAlert {
            id: alloc::string::String::from("a1"),
            state: eneros_slo_alert::AlertState::Critical,
            value_ns: 1,
            threshold_ns: 2,
            at_unix: 3,
        })
        .unwrap();
        assert!(s.take_metrics().unwrap().contains("row"));
        assert!(s.take_alert().unwrap().contains("a1"));
    }
}
