//! MQTT 协议插件（蓝图 39 WP-39.11）。
//!
//! 与其他插件同构：`Box<dyn TransportHandle>` 实现 [`MqttTransport`]
//! （本地 trait 实现外来类型）；插件持有 [`MqttClient`]，poll 时惰性
//! connect（事件循环语义）并驱动协议栈。安全通道组合（
//! [`crate::real::SecureMqttTransport`]）由装配层按 channel 策略注入。

use alloc::borrow::ToOwned;
use alloc::boxed::Box;

use eneros_protocol_abstract::{
    AdapterState, InteropLevel, PointRegistry, Progress, ProtocolConfig, ProtocolDescriptor,
    ProtocolError, ProtocolHealth, ProtocolPlugin, ProtocolRole, ResourceBudget, SecurityProfile,
    TransportFactory, TransportHandle, TransportKind, TransportSpec, WorkBudget,
};

use crate::client::MqttClient;
use crate::error::MqttError;
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
const KNOWN_KEYS: &[&str] = &["broker", "client_id", "keep_alive_ms", "qos", "will_topic"];

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
        }
    }

    /// 45.4：注入点表路由点数（观测）。
    pub fn routed_count(&self) -> usize {
        self.routed_count
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
            interop_level: InteropLevel::CodecReady,
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
            Ok(_) => {
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
}
