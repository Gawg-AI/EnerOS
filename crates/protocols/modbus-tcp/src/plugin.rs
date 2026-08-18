//! Modbus TCP 协议插件（蓝图 39 WP-39.11）。
//!
//! 与 RTU 插件同构：硬件无关，经 [`TransportFactory`] 获取
//! `Box<dyn TransportHandle>`；`Box<dyn TransportHandle>` 实现
//! [`TcpTransport`]，适配器直接消费。

use alloc::borrow::ToOwned;
use alloc::boxed::Box;

use eneros_protocol_abstract::{
    parse_ip_port, AdapterState, InteropLevel, PointRegistry, Progress, ProtocolAdapter,
    ProtocolConfig, ProtocolDescriptor, ProtocolError, ProtocolHealth, ProtocolPlugin,
    ProtocolRole, ResourceBudget, SecurityProfile, TransportFactory, TransportHandle,
    TransportKind, TransportSpec, WorkBudget,
};

use crate::adapter::ModbusTcpAdapter;
use crate::device::TcpDevice;
use crate::error::ModbusTcpError;
use crate::transport::TcpTransport;

/// 插件 id。
pub const TCP_PLUGIN_ID: &str = "modbus-tcp";

/// `Box<dyn TransportHandle>` 实现 [`TcpTransport`]（本地 trait 实现
/// 外来类型——合法；插件与网卡解耦）。
impl TcpTransport for Box<dyn TransportHandle> {
    fn send(&mut self, data: &[u8]) -> Result<(), ModbusTcpError> {
        TransportHandle::send(self.as_mut(), data).map_err(|_| ModbusTcpError::ConnectionFailed)
    }
    fn recv(&mut self, _timeout_ms: u32) -> Result<alloc::vec::Vec<u8>, ModbusTcpError> {
        let mut buf = [0u8; 1024];
        let n =
            TransportHandle::recv(self.as_mut(), &mut buf).map_err(|_| ModbusTcpError::Timeout)?;
        Ok(buf[..n].to_vec())
    }
    fn connect(&mut self, device: &TcpDevice) -> Result<(), ModbusTcpError> {
        // 工厂 open() 已按 spec.peer 建立连接；此处仅验证一致性。
        if self.is_connected(device) {
            Ok(())
        } else {
            Err(ModbusTcpError::ConnectionFailed)
        }
    }
    fn is_connected(&self, device: &TcpDevice) -> bool {
        let _ = device;
        TransportHandle::is_connected(self.as_ref())
    }
}

/// 已知配置键。
const KNOWN_KEYS: &[&str] = &["peer", "unit_id", "retries", "timeout_ms"];

/// Modbus TCP 插件。
pub struct TcpPlugin {
    adapter: Option<ModbusTcpAdapter<Box<dyn TransportHandle>>>,
    started: bool,
    last_poll_ms: u64,
    error_count: u64,
    last_error: Option<ProtocolError>,
    last_progress: Progress,
}

impl TcpPlugin {
    pub fn new() -> Self {
        Self {
            adapter: None,
            started: false,
            last_poll_ms: 0,
            error_count: 0,
            last_error: None,
            last_progress: Progress::Idle,
        }
    }
}

impl Default for TcpPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolPlugin for TcpPlugin {
    fn descriptor(&self) -> &ProtocolDescriptor {
        static D: ProtocolDescriptor = ProtocolDescriptor {
            id: TCP_PLUGIN_ID,
            version: "1.0",
            role: ProtocolRole::Master,
            transport_kinds: &[TransportKind::Tcp],
            security_profile: SecurityProfile::None,
            point_types: &["float", "int", "bool"],
            resource_budget: ResourceBudget {
                max_connections: 1,
                max_poll_ms: 50,
                max_frame: 260,
            },
            interop_level: InteropLevel::CodecReady,
        };
        &D
    }

    fn validate_config(&self, cfg: &ProtocolConfig) -> Result<(), ProtocolError> {
        eneros_protocol_abstract::plugin::reject_unknown_keys(cfg, KNOWN_KEYS)?;
        // peer 必须可解析为 ip:port（不猜）。
        match cfg.get("peer") {
            Some(peer) if parse_ip_port(peer).is_some() => Ok(()),
            _ => Err(ProtocolError::InvalidConfig),
        }
    }

    fn start(
        &mut self,
        cfg: &ProtocolConfig,
        transports: &mut dyn TransportFactory,
        _points: &mut dyn PointRegistry,
    ) -> Result<(), ProtocolError> {
        if self.adapter.is_some() {
            return Err(ProtocolError::ProtocolInit);
        }
        if !transports.supports(TransportKind::Tcp) {
            return Err(ProtocolError::Unsupported);
        }
        // 39 收尾复验（P0）：peer/unit_id/timeout/retries 来自校验过的
        // 配置本体（旧实现此处重建 default 空配置 + 硬编码 0.0.0.0:502）。
        let peer = cfg
            .get("peer")
            .ok_or(ProtocolError::InvalidConfig)?
            .to_owned();
        let (ip, port) = parse_ip_port(&peer).ok_or(ProtocolError::InvalidConfig)?;
        let unit_id =
            eneros_protocol_abstract::plugin::config_u64(cfg, "unit_id")?.unwrap_or(1) as u8;
        let timeout_ms =
            eneros_protocol_abstract::plugin::config_u64(cfg, "timeout_ms")?.unwrap_or(1000) as u32;
        let retries =
            eneros_protocol_abstract::plugin::config_u64(cfg, "retries")?.unwrap_or(2) as u8;
        let mut spec = TransportSpec {
            kind: TransportKind::Tcp,
            peer: [0; 128],
            peer_len: 0,
            mtu: 260,
            connect_timeout_ms: 2000,
            recv_timeout_ms: timeout_ms as u64,
        };
        let bytes = peer.as_bytes();
        if bytes.len() > spec.peer.len() {
            return Err(ProtocolError::InvalidConfig);
        }
        spec.peer[..bytes.len()].copy_from_slice(bytes);
        spec.peer_len = bytes.len() as u8;
        let handle = transports
            .open(&spec)
            .map_err(|_| ProtocolError::ProtocolInit)?;
        let device = TcpDevice {
            ip,
            port,
            unit_id,
            timeout_ms,
        };
        let mut adapter = ModbusTcpAdapter::new(handle, device, retries);
        let acfg = eneros_protocol_abstract::AdapterConfig {
            name: alloc::string::String::from(TCP_PLUGIN_ID),
            protocol_type: eneros_protocol_abstract::ProtocolType::ModbusTcp,
            device_configs: alloc::vec![eneros_protocol_abstract::DeviceConfig {
                device_id: 1,
                name: alloc::string::String::from("tcp-1"),
                address: eneros_protocol_abstract::ProtocolAddress::Modbus {
                    slave_addr: 1,
                    reg_addr: 0,
                    func_code: 3,
                },
            }],
        };
        adapter.init(&acfg)?;
        adapter.start()?;
        self.adapter = Some(adapter);
        self.started = true;
        Ok(())
    }

    fn poll(&mut self, now_ms: u64, _budget: WorkBudget) -> Result<Progress, ProtocolError> {
        let Some(adapter) = self.adapter.as_mut() else {
            return Err(ProtocolError::ProtocolNotStarted);
        };
        self.last_poll_ms = now_ms;
        match adapter.poll(now_ms) {
            Ok(()) => {
                self.last_progress = Progress::Completed;
                Ok(Progress::Completed)
            }
            Err(e) => {
                self.error_count += 1;
                self.last_error = Some(e.clone());
                self.last_progress = Progress::Degraded;
                Err(e)
            }
        }
    }

    fn health(&self) -> ProtocolHealth {
        let (state, progress) = match &self.adapter {
            Some(a) => (a.state(), self.last_progress),
            None => (AdapterState::Uninitialized, Progress::Idle),
        };
        ProtocolHealth {
            state,
            last_poll_ms: self.last_poll_ms,
            error_count: self.error_count,
            last_error: self.last_error.clone(),
            last_progress: progress,
        }
    }

    fn stop(&mut self) -> Result<(), ProtocolError> {
        self.adapter = None;
        self.started = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use eneros_protocol_abstract::test_support::EchoTransport;
    use eneros_protocol_abstract::EmptyPointRegistry;
    use eneros_protocol_abstract::TransportError;

    use super::*;

    struct TestTcpFactory;

    impl TransportFactory for TestTcpFactory {
        fn open(
            &mut self,
            _spec: &TransportSpec,
        ) -> Result<Box<dyn TransportHandle>, TransportError> {
            Ok(Box::new(EchoTransport::new()))
        }
        fn supports(&self, kind: TransportKind) -> bool {
            kind == TransportKind::Tcp
        }
    }

    #[test]
    fn validate_config_requires_parseable_peer() {
        let plugin = TcpPlugin::new();
        let mut cfg = ProtocolConfig::default();
        cfg.insert("peer", "192.168.1.10:502");
        assert!(plugin.validate_config(&cfg).is_ok());
        cfg.insert("peer", "not-an-ip");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
        cfg.insert("peer", "10.0.0.1:502");
        cfg.insert("unknown", "x");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
    }

    #[test]
    fn start_poll_stop_lifecycle() {
        let mut plugin = TcpPlugin::new();
        let mut factory = TestTcpFactory;
        let mut cfg = ProtocolConfig::default();
        cfg.insert("peer", "192.168.1.10:502");
        plugin
            .start(&cfg, &mut factory, &mut EmptyPointRegistry)
            .expect("start");
        assert_eq!(plugin.health().state, AdapterState::Running);
        plugin
            .poll(
                500,
                WorkBudget {
                    max_work_ms: 50,
                    max_polls: 1,
                },
            )
            .unwrap();
        assert!(plugin.health().last_poll_ms >= 500);
        plugin.stop().unwrap();
        assert_eq!(
            plugin.poll(
                600,
                WorkBudget {
                    max_work_ms: 50,
                    max_polls: 1
                }
            ),
            Err(ProtocolError::ProtocolNotStarted)
        );
    }

    /// 39 收尾复验（P0）：配置的 peer 真正进入传输 spec（旧实现恒用
    /// 硬编码 0.0.0.0:502）；缺 peer → InvalidConfig（不再默认值兜底）。
    #[test]
    fn start_consumes_configured_peer() {
        struct CapturingFactory {
            peer: core::cell::RefCell<alloc::vec::Vec<u8>>,
        }
        impl TransportFactory for CapturingFactory {
            fn open(
                &mut self,
                spec: &TransportSpec,
            ) -> Result<Box<dyn TransportHandle>, TransportError> {
                *self.peer.borrow_mut() = spec.peer[..spec.peer_len as usize].to_vec();
                Ok(Box::new(EchoTransport::new()))
            }
            fn supports(&self, kind: TransportKind) -> bool {
                kind == TransportKind::Tcp
            }
        }
        let mut plugin = TcpPlugin::new();
        let mut factory = CapturingFactory {
            peer: core::cell::RefCell::new(alloc::vec::Vec::new()),
        };
        let mut cfg = ProtocolConfig::default();
        cfg.insert("peer", "10.7.0.9:1502");
        plugin
            .start(&cfg, &mut factory, &mut EmptyPointRegistry)
            .expect("start");
        assert_eq!(*factory.peer.borrow(), b"10.7.0.9:1502");

        // 缺 peer → InvalidConfig（默认值不伪健康）。
        let mut plugin2 = TcpPlugin::new();
        assert_eq!(
            plugin2.start(
                &ProtocolConfig::default(),
                &mut factory,
                &mut EmptyPointRegistry
            ),
            Err(ProtocolError::InvalidConfig)
        );
    }

    #[test]
    fn start_rejects_unsupported_transport() {
        let mut plugin = TcpPlugin::new();
        struct SerialOnly;
        impl TransportFactory for SerialOnly {
            fn open(
                &mut self,
                _spec: &TransportSpec,
            ) -> Result<Box<dyn TransportHandle>, TransportError> {
                Err(TransportError::Unsupported)
            }
            fn supports(&self, kind: TransportKind) -> bool {
                kind == TransportKind::Serial
            }
        }
        assert_eq!(
            plugin.start(
                &ProtocolConfig::default(),
                &mut SerialOnly,
                &mut EmptyPointRegistry
            ),
            Err(ProtocolError::Unsupported)
        );
    }
}
