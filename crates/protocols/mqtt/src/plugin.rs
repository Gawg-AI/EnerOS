//! MQTT 协议插件（蓝图 39 WP-39.11）。
//!
//! 与其他插件同构：`Box<dyn TransportHandle>` 实现 [`MqttTransport`]
//! （本地 trait 实现外来类型）；插件持有 [`MqttClient`]，poll 时惰性
//! connect（事件循环语义）并驱动协议栈。安全通道组合（
//! [`crate::real::SecureMqttTransport`]）由装配层按 channel 策略注入。

use alloc::borrow::ToOwned;
use alloc::boxed::Box;

use eneros_protocol_abstract::write::PointWriteIntent;
use eneros_protocol_abstract::{
    AdapterState, InteropLevel, PointRegistry, Progress, ProtocolConfig, ProtocolDescriptor,
    ProtocolError, ProtocolHealth, ProtocolPlugin, ProtocolRole, ResourceBudget, SecurityProfile,
    TransportFactory, TransportHandle, TransportKind, TransportSpec, WorkBudget,
};

use crate::client::MqttClient;
use crate::error::MqttError;
use crate::packet::MqttPacket;
use crate::transport::MqttTransport;

/// 插件 id。
pub const MQTT_PLUGIN_ID: &str = "mqtt";

/// `Box<dyn TransportHandle>` 实现 [`MqttTransport`]。
///
/// connect(host, port)：工厂 open() 已按 spec.peer 建立传输连接（连接
/// 参数来自配置，不猜）；此处仅确认句柄在线——不在线 → NotConnected
/// （fail-closed，不伪装成功）。
impl MqttTransport for Box<dyn TransportHandle> {
    fn connect(&mut self, _host: &str, _port: u16) -> Result<(), MqttError> {
        if self.is_connected() {
            Ok(())
        } else {
            Err(MqttError::NotConnected)
        }
    }
    fn send(&mut self, data: &[u8]) -> Result<(), MqttError> {
        TransportHandle::send(self.as_mut(), data).map_err(|_| MqttError::TransportError)
    }
    fn recv(&mut self) -> Result<alloc::vec::Vec<u8>, MqttError> {
        let mut buf = [0u8; 4096];
        let n = TransportHandle::recv(self.as_mut(), &mut buf)
            .map_err(|_| MqttError::PublishTimeout)?;
        Ok(buf[..n].to_vec())
    }
    fn close(&mut self) -> Result<(), MqttError> {
        TransportHandle::close(self.as_mut()).map_err(|_| MqttError::TransportError)
    }
    fn is_connected(&self) -> bool {
        TransportHandle::is_connected(self.as_ref())
    }
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

/// 已知配置键。
const KNOWN_KEYS: &[&str] = &[
    "broker",
    "client_id",
    "keep_alive_ms",
    "qos",
    "will_topic",
    // 47（WP-47.1 协议入站写）：下行写命令订阅主题。缺省 = 不订阅
    //（方向一禁用——诚实：未配置不假装能收写命令）。
    "control_topic",
];

/// MQTT 插件。
pub struct MqttPlugin {
    client: Option<MqttClient>,
    state: AdapterState,
    /// 连接状态（插件内跟踪；poll 惰性连接）。
    connected: bool,
    last_poll_ms: u64,
    error_count: u64,
    last_error: Option<ProtocolError>,
    last_progress: Progress,
    /// 45.4：注入点表路由点数（观测；服务层按协议过滤注入）。
    routed_count: usize,
    /// 47（WP-47.1）：下行写命令主题（可选；未配置 = 不收写）。
    control_topic: Option<alloc::string::String>,
    /// 47（WP-47.1）：已解析待导出写意图（poll → drain）。
    pending_intents: alloc::vec::Vec<PointWriteIntent>,
    /// 47（WP-47.1）：意图序号（每协议单调；重放判定由 ControlDispatch）。
    intent_seq: u64,
    /// 47（WP-47.1）：已收到但无法解析的下行负载计数（观测；不 panic）。
    bad_downlink_count: u64,
}

impl MqttPlugin {
    pub fn new() -> Self {
        Self {
            client: None,
            state: AdapterState::Uninitialized,
            connected: false,
            last_poll_ms: 0,
            error_count: 0,
            last_error: None,
            last_progress: Progress::Idle,
            routed_count: 0,
            control_topic: None,
            pending_intents: alloc::vec::Vec::new(),
            intent_seq: 0,
            bad_downlink_count: 0,
        }
    }

    /// 45.4：注入点表路由点数（观测）。
    pub fn routed_count(&self) -> usize {
        self.routed_count
    }

    /// 47（WP-47.1）：不可解析下行负载计数（观测）。
    pub fn bad_downlink_count(&self) -> u64 {
        self.bad_downlink_count
    }

    /// 47（WP-47.1）：解析一条下行写命令负载为写意图。
    ///
    /// 负载文本格式（外部 SCADA 约定）：`SET <device_id> <point_id> <value>`
    /// ——ASCII 空白分隔；value = 十进制浮点（kW 语义）。格式不符 →
    /// 计数 + 丢弃（不 panic、不伪造）；主题不符 → 忽略。
    /// **只导出意图，禁止直接写设备**（WP-47.1 红线）。
    fn on_downlink(&mut self, topic: &str, payload: &[u8], now_ms: u64) {
        let Some(ctl) = self.control_topic.as_ref() else {
            return;
        };
        if topic != ctl {
            return;
        }
        let text = match core::str::from_utf8(payload) {
            Ok(t) => t.trim(),
            Err(_) => {
                self.bad_downlink_count += 1;
                return;
            }
        };
        let mut parts = text.split_whitespace();
        let cmd = parts.next().unwrap_or("");
        let (Some(dev), Some(point), Some(val)) = (parts.next(), parts.next(), parts.next()) else {
            self.bad_downlink_count += 1;
            return;
        };
        if parts.next().is_some() {
            // 多余字段 → 格式不符（防错发）。
            self.bad_downlink_count += 1;
            return;
        }
        if cmd != "SET" {
            return; // 其他命令字：非本插件写协议（忽略，不计数为坏负载）。
        }
        let (Ok(device_id), Ok(point_id), Ok(value)) =
            (dev.parse::<u32>(), point.parse::<u32>(), val.parse::<f64>())
        else {
            self.bad_downlink_count += 1;
            return;
        };
        if !value.is_finite() || device_id == 0 {
            self.bad_downlink_count += 1;
            return;
        }
        self.intent_seq = self.intent_seq.saturating_add(1);
        self.pending_intents.push(PointWriteIntent {
            protocol_id: alloc::string::String::from(MQTT_PLUGIN_ID),
            device_id: device_id as u16,
            point_id,
            value: eneros_upa_model::PointValue::Float(value),
            sequence: self.intent_seq,
            issued_ms: now_ms,
        });
    }
}

impl Default for MqttPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolPlugin for MqttPlugin {
    fn descriptor(&self) -> &ProtocolDescriptor {
        static D: ProtocolDescriptor = ProtocolDescriptor {
            id: MQTT_PLUGIN_ID,
            version: "1.0",
            role: ProtocolRole::Client,
            transport_kinds: &[TransportKind::Tcp],
            security_profile: SecurityProfile::Tls12,
            point_types: &["float", "int", "bool", "string"],
            resource_budget: ResourceBudget {
                max_connections: 1,
                max_poll_ms: 50,
                max_frame: 4096,
            },
            // 46.5：QEMU/EL0 + 独立对端 L2 闭环（evidence/46/WP-46.5.md）。
            interop_level: InteropLevel::TargetPeerVerified,
        };
        &D
    }

    fn validate_config(&self, cfg: &ProtocolConfig) -> Result<(), ProtocolError> {
        eneros_protocol_abstract::plugin::reject_unknown_keys(cfg, KNOWN_KEYS)?;
        // broker 必须可解析（host[:port]）且 ≤ 128 字节（TransportSpec.peer
        // 容量；39 收尾复验 P2-1：超长不得留到 start 内 panic）。
        match cfg.get("broker") {
            Some(broker) if !broker.is_empty() && broker.len() <= 128 => Ok(()),
            _ => Err(ProtocolError::InvalidConfig),
        }
    }

    fn start(
        &mut self,
        cfg: &ProtocolConfig,
        transports: &mut dyn TransportFactory,
        points: &mut dyn PointRegistry,
    ) -> Result<(), ProtocolError> {
        // 45.4 点表驱动路由：消费注入点表（插件不自造地址）。服务层按协议
        // 过滤注入；此处登记路由点数（非本协议地址点跳过，不阻断装配）。
        self.routed_count =
            eneros_protocol_abstract::plugin::count_matching_mappings(points, |m| {
                matches!(
                    m.protocol_addr,
                    eneros_protocol_abstract::ProtocolAddress::Mqtt { .. }
                )
            });
        if self.client.is_some() {
            return Err(ProtocolError::ProtocolInit);
        }
        if !transports.supports(TransportKind::Tcp) {
            return Err(ProtocolError::Unsupported);
        }
        // 39 收尾复验（P0）：消费 validate_config 校验过的配置本体
        //（旧实现此处重建 default 空配置——broker 永远 127.0.0.1:1883，
        // 配置链整体断裂）；缺省值仅用于未提供的可选键。
        let broker = cfg
            .get("broker")
            .filter(|b| !b.is_empty() && b.len() <= 128)
            .ok_or(ProtocolError::InvalidConfig)?
            .to_owned();
        let client_id = cfg.get("client_id").unwrap_or("eneros-agent").to_owned();
        let keep_alive_ms: u64 =
            eneros_protocol_abstract::plugin::config_u64(cfg, "keep_alive_ms")?.unwrap_or(60_000);
        let mut spec = TransportSpec {
            kind: TransportKind::Tcp,
            peer: [0; 128],
            peer_len: 0,
            mtu: 4096,
            connect_timeout_ms: 2000,
            recv_timeout_ms: 1000,
        };
        let bytes = broker.as_bytes();
        spec.peer[..bytes.len()].copy_from_slice(bytes);
        spec.peer_len = bytes.len() as u8;
        let handle = transports
            .open(&spec)
            .map_err(|_| ProtocolError::ProtocolInit)?;
        let transport: Box<dyn MqttTransport> = Box::new(handle);
        let mut client = MqttClient::new(&broker, &client_id, keep_alive_ms);
        client.set_transport(transport);
        // 47（WP-47.1）：下行写命令主题（可选）——配置了才订阅。
        if let Some(topic) = cfg.get("control_topic").filter(|t| !t.is_empty()) {
            match client.subscribe(topic, crate::qos::QoS::AtMostOnce) {
                Ok(()) => {
                    self.control_topic = Some(topic.to_owned());
                }
                Err(_) => {
                    // 订阅失败 → 不装配写通道（fail-closed：不假装能收写）。
                    return Err(ProtocolError::ProtocolInit);
                }
            }
        }
        self.client = Some(client);
        self.state = AdapterState::Running;
        Ok(())
    }

    fn poll(&mut self, now_ms: u64, _budget: WorkBudget) -> Result<Progress, ProtocolError> {
        let Some(client) = self.client.as_mut() else {
            return Err(ProtocolError::ProtocolNotStarted);
        };
        self.last_poll_ms = now_ms;
        // 惰性连接（事件循环语义：首次 poll 建立 CONNECT 握手）。
        if !self.connected {
            match client.connect(now_ms) {
                Ok(()) => self.connected = true,
                Err(_) => {
                    self.error_count += 1;
                    self.last_error = Some(ProtocolError::ReadFailed);
                    self.last_progress = Progress::Degraded;
                    return Err(ProtocolError::ReadFailed);
                }
            }
        }
        match client.poll(now_ms) {
            Ok(pkts) => {
                // 47（WP-47.1）：下行 PUBLISH（control_topic）→ 解析为写意图
                //（只导出；设备写由 ControlDispatch 授权后执行）。
                for pkt in pkts {
                    if let MqttPacket::Publish(pub_) = pkt {
                        self.on_downlink(&pub_.topic, &pub_.payload, now_ms);
                    }
                }
                self.last_progress = Progress::Completed;
                Ok(Progress::Completed)
            }
            Err(_) => {
                self.error_count += 1;
                self.last_error = Some(ProtocolError::ReadFailed);
                self.last_progress = Progress::Degraded;
                Err(ProtocolError::ReadFailed)
            }
        }
    }

    fn health(&self) -> ProtocolHealth {
        ProtocolHealth {
            state: self.state,
            last_poll_ms: self.last_poll_ms,
            error_count: self.error_count,
            last_error: self.last_error.clone(),
            last_progress: self.last_progress,
        }
    }

    fn stop(&mut self) -> Result<(), ProtocolError> {
        if let Some(client) = self.client.as_mut() {
            let _ = client.disconnect();
        }
        self.client = None;
        self.state = AdapterState::Stopped;
        Ok(())
    }

    fn drain_write_intents(&mut self, out: &mut alloc::vec::Vec<PointWriteIntent>) {
        out.append(&mut self.pending_intents);
    }
}

#[cfg(test)]
mod tests {
    use eneros_protocol_abstract::EmptyPointRegistry;
    use eneros_protocol_abstract::TransportError;

    use super::*;

    /// 模拟 MQTT broker 的最小传输：CONNECT → CONNACK(0)，其余 echo。
    #[derive(Default)]
    struct MqttBrokerTransport {
        last: core::cell::RefCell<alloc::vec::Vec<u8>>,
    }

    impl TransportHandle for MqttBrokerTransport {
        fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
            *self.last.borrow_mut() = data.to_vec();
            Ok(())
        }
        fn recv(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
            let data = self.last.borrow().clone();
            // CONNECT 包（0x10 开头）→ 应答 CONNACK。
            let reply: alloc::vec::Vec<u8> = if data.first() == Some(&0x10) {
                alloc::vec![0x20, 0x02, 0x00, 0x00]
            } else {
                data
            };
            let n = reply.len().min(buf.len());
            buf[..n].copy_from_slice(&reply[..n]);
            Ok(n)
        }
        fn close(&mut self) -> Result<(), TransportError> {
            Ok(())
        }
        fn is_connected(&self) -> bool {
            true
        }
        fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
            self
        }
    }

    struct TestTcpFactory;

    impl TransportFactory for TestTcpFactory {
        fn open(
            &mut self,
            _spec: &TransportSpec,
        ) -> Result<Box<dyn TransportHandle>, TransportError> {
            Ok(Box::new(MqttBrokerTransport::default()))
        }
        fn supports(&self, kind: TransportKind) -> bool {
            kind == TransportKind::Tcp
        }
    }

    #[test]
    fn validate_config_rejects_unknown_and_empty_broker() {
        let plugin = MqttPlugin::new();
        let mut cfg = ProtocolConfig::default();
        cfg.insert("broker", "127.0.0.1:1883");
        assert!(plugin.validate_config(&cfg).is_ok());
        cfg.insert("broker", "");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
        cfg.insert("broker", "127.0.0.1:1883");
        cfg.insert("evil", "1");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
    }

    #[test]
    fn start_poll_stop_lifecycle() {
        let mut plugin = MqttPlugin::new();
        let mut factory = TestTcpFactory;
        let mut cfg = ProtocolConfig::default();
        cfg.insert("broker", "127.0.0.1:1883");
        plugin
            .start(&cfg, &mut factory, &mut EmptyPointRegistry)
            .expect("start");
        assert_eq!(plugin.health().state, AdapterState::Running);
        plugin
            .poll(
                1000,
                WorkBudget {
                    max_work_ms: 50,
                    max_polls: 1,
                },
            )
            .unwrap();
        assert!(plugin.health().last_poll_ms >= 1000);
        plugin.stop().unwrap();
        assert_eq!(
            plugin.poll(
                2000,
                WorkBudget {
                    max_work_ms: 50,
                    max_polls: 1
                }
            ),
            Err(ProtocolError::ProtocolNotStarted)
        );
    }

    /// 39 收尾复验（P0）：配置的 broker 真正进入传输 spec（配置链
    /// 不再断裂；旧实现恒用硬编码 127.0.0.1:1883）。
    #[test]
    fn start_consumes_configured_broker() {
        /// 捕获 open() 收到的 spec。
        struct CapturingFactory {
            peer: core::cell::RefCell<alloc::vec::Vec<u8>>,
        }
        impl TransportFactory for CapturingFactory {
            fn open(
                &mut self,
                spec: &TransportSpec,
            ) -> Result<Box<dyn TransportHandle>, TransportError> {
                *self.peer.borrow_mut() = spec.peer[..spec.peer_len as usize].to_vec();
                Ok(Box::new(MqttBrokerTransport::default()))
            }
            fn supports(&self, kind: TransportKind) -> bool {
                kind == TransportKind::Tcp
            }
        }
        let mut plugin = MqttPlugin::new();
        let mut factory = CapturingFactory {
            peer: core::cell::RefCell::new(alloc::vec::Vec::new()),
        };
        let mut cfg = ProtocolConfig::default();
        cfg.insert("broker", "10.7.0.42:8883");
        plugin
            .start(&cfg, &mut factory, &mut EmptyPointRegistry)
            .expect("start");
        assert_eq!(*factory.peer.borrow(), b"10.7.0.42:8883");
    }

    /// 39 收尾复验（P2-1）：broker 超 128 字节 → 校验拒绝（不留到
    /// start 内 panic）。
    #[test]
    fn validate_config_rejects_oversized_broker() {
        let plugin = MqttPlugin::new();
        let mut cfg = ProtocolConfig::default();
        cfg.insert(
            "broker",
            &alloc::string::String::from_utf8(alloc::vec![b'a'; 200]).unwrap(),
        );
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
    }

    // ================= 47（WP-47.1）协议入站写 =================

    /// 47（WP-47.1）：control_topic 下行 SET 负载 → 写意图导出
    ///（只导出；序列单调；drain 清空缓冲）。
    #[test]
    fn downlink_set_parses_to_write_intent() {
        let mut plugin = MqttPlugin::new();
        plugin.control_topic = Some(alloc::string::String::from("eneros/ctl"));
        plugin.on_downlink("eneros/ctl", b"SET 1 10 50.0", 1_700_000_000_010);
        plugin.on_downlink("eneros/ctl", b"SET 1 10 -20.5", 1_700_000_000_020);
        let mut out = alloc::vec::Vec::new();
        plugin.drain_write_intents(&mut out);
        assert_eq!(out.len(), 2, "两条下行都必须导出");
        assert_eq!(out[0].protocol_id, MQTT_PLUGIN_ID);
        assert_eq!(out[0].device_id, 1);
        assert_eq!(out[0].point_id, 10);
        assert_eq!(out[0].value, eneros_upa_model::PointValue::Float(50.0));
        assert_eq!(out[0].sequence, 1, "序号单调");
        assert_eq!(out[1].sequence, 2);
        assert_eq!(out[1].issued_ms, 1_700_000_000_020);
        // drain 后缓冲清空（第二只读为空）。
        let mut again = alloc::vec::Vec::new();
        plugin.drain_write_intents(&mut again);
        assert!(again.is_empty());
    }

    /// 47（WP-47.1）：主题不符 / 坏格式 / 非 SET 命令 → 不导出意图；
    /// 坏下行计数（不 panic、不伪造）。
    #[test]
    fn downlink_bad_payloads_counted_and_ignored() {
        let mut plugin = MqttPlugin::new();
        plugin.control_topic = Some(alloc::string::String::from("eneros/ctl"));
        plugin.on_downlink("other/topic", b"SET 1 10 50", 1); // 主题不符
        plugin.on_downlink("eneros/ctl", b"SET 1 10", 2); // 字段不足
        plugin.on_downlink("eneros/ctl", b"SET 1 x 50", 3); // 点非数字
        plugin.on_downlink("eneros/ctl", b"SET 0 10 50", 4); // device 0
        plugin.on_downlink("eneros/ctl", b"SET 1 10 NaN", 5); // 非有限值
        plugin.on_downlink("eneros/ctl", b"SET 1 10 50 extra", 6); // 多余字段
        plugin.on_downlink("eneros/ctl", b"GET 1 10 50", 7); // 非 SET 命令
        let mut out = alloc::vec::Vec::new();
        plugin.drain_write_intents(&mut out);
        assert!(out.is_empty(), "全部坏负载不得导出意图: {out:?}");
        assert_eq!(
            plugin.bad_downlink_count, 5,
            "5 条格式不符计数（主题不符与非 SET 命令不计数）"
        );
    }

    /// 47（WP-47.1）：意图线码往返（svc 间 EIPC 传输同源）。
    #[test]
    fn downlink_intent_survives_wire_roundtrip() {
        let mut plugin = MqttPlugin::new();
        plugin.control_topic = Some(alloc::string::String::from("eneros/ctl"));
        plugin.on_downlink("eneros/ctl", b"SET 1 10 72.25", 99);
        let mut out = alloc::vec::Vec::new();
        plugin.drain_write_intents(&mut out);
        assert_eq!(out.len(), 1);
        let wire = eneros_protocol_abstract::write::encode_intent(&out[0]);
        let back = eneros_protocol_abstract::write::decode_intent(&wire).expect("decode");
        assert_eq!(back, out[0]);
    }
}
