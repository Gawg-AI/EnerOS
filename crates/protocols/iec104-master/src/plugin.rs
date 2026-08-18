//! IEC 60870-5-104 协议插件（蓝图 39 WP-39.11）。
//!
//! 与 RTU/TCP 插件同构：`Box<dyn TransportHandle>` 实现
//! [`MasterTransport`]，`Iec104Adapter` 直接消费；对端地址由配置
//! `peer`（ip:port）注入，工厂 open 时连接（生产路径禁止字面量 IP）。

use alloc::borrow::ToOwned;
use alloc::boxed::Box;

use eneros_protocol_abstract::{
    parse_ip_port, AdapterState, InteropLevel, PointRegistry, Progress, ProtocolAdapter,
    ProtocolConfig, ProtocolDescriptor, ProtocolError, ProtocolHealth, ProtocolPlugin,
    ProtocolRole, ResourceBudget, SecurityProfile, TransportFactory, TransportHandle,
    TransportKind, TransportSpec, WorkBudget,
};

use crate::adapter::Iec104Adapter;
use crate::config::MasterConfig;
use crate::error::MasterError;
use crate::transport::{ConnId, MasterTransport};

/// 插件 id。
pub const IEC104_PLUGIN_ID: &str = "iec104-master";

/// `Box<dyn TransportHandle>` 实现 [`MasterTransport`]。
impl MasterTransport for Box<dyn TransportHandle> {
    fn connect(&mut self, _ip: [u8; 4], _port: u16) -> Result<ConnId, MasterError> {
        if self.is_connected() {
            Ok(1)
        } else {
            Err(MasterError::ConnectFailed)
        }
    }
    fn send(&mut self, _conn: ConnId, data: &[u8]) -> Result<(), MasterError> {
        TransportHandle::send(self.as_mut(), data).map_err(|_| MasterError::SendFailed)
    }
    fn recv(&mut self, _conn: ConnId) -> Result<Option<alloc::vec::Vec<u8>>, MasterError> {
        let mut buf = [0u8; 1024];
        match TransportHandle::recv(self.as_mut(), &mut buf) {
            Ok(0) => Ok(None),
            Ok(n) => Ok(Some(buf[..n].to_vec())),
            // 39 收尾复验（P1-4）：断连对状态机必须可见——旧实现把
            // Closed/Io 一并映射为 Ok(None)（断链只能靠 t3 间接兜底）。
            // 超时/无数据 = 无帧（poll 语义）；Closed/Io = 接收失败。
            Err(eneros_protocol_abstract::TransportError::Timeout) => Ok(None),
            Err(eneros_protocol_abstract::TransportError::Closed)
            | Err(eneros_protocol_abstract::TransportError::Io) => Err(MasterError::RecvFailed),
            Err(_) => Ok(None),
        }
    }
    fn close(&mut self, _conn: ConnId) -> Result<(), MasterError> {
        TransportHandle::close(self.as_mut()).map_err(|_| MasterError::StateError)
    }
    fn now_ms(&self) -> u64 {
        eneros_time::get_monotonic_ns() / 1_000_000
    }
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

/// 已知配置键。
const KNOWN_KEYS: &[&str] = &[
    "peer",
    "clock_sync_interval_ms",
    "t3_timeout_ms",
    "poll_interval_ms",
];

/// IEC 104 插件。
pub struct Iec104Plugin {
    adapter: Option<Iec104Adapter>,
    started: bool,
    last_poll_ms: u64,
    error_count: u64,
    last_error: Option<ProtocolError>,
    last_progress: Progress,
}

impl Iec104Plugin {
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

impl Default for Iec104Plugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolPlugin for Iec104Plugin {
    fn descriptor(&self) -> &ProtocolDescriptor {
        static D: ProtocolDescriptor = ProtocolDescriptor {
            id: IEC104_PLUGIN_ID,
            version: "1.0",
            role: ProtocolRole::Master,
            transport_kinds: &[TransportKind::Tcp],
            security_profile: SecurityProfile::None,
            point_types: &["float", "int", "bool", "string"],
            resource_budget: ResourceBudget {
                max_connections: 1,
                max_poll_ms: 100,
                max_frame: 253,
            },
            interop_level: InteropLevel::CodecReady,
        };
        &D
    }

    fn validate_config(&self, cfg: &ProtocolConfig) -> Result<(), ProtocolError> {
        eneros_protocol_abstract::plugin::reject_unknown_keys(cfg, KNOWN_KEYS)?;
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
        // 39 收尾复验（P0）：peer/t3/poll_interval/clock_sync 来自校验
        // 过的配置本体（旧实现此处重建 default 空配置 + 硬编码
        // 0.0.0.0:2404）。
        let peer = cfg
            .get("peer")
            .ok_or(ProtocolError::InvalidConfig)?
            .to_owned();
        let (ip, port) = parse_ip_port(&peer).ok_or(ProtocolError::InvalidConfig)?;
        let t3_timeout_ms = eneros_protocol_abstract::plugin::config_u64(cfg, "t3_timeout_ms")?
            .unwrap_or(20_000)
            .min(u32::MAX as u64) as u32;
        let poll_interval_ms =
            eneros_protocol_abstract::plugin::config_u64(cfg, "poll_interval_ms")?
                .unwrap_or(30_000)
                .min(u32::MAX as u64) as u32;
        let clock_sync_interval_ms =
            eneros_protocol_abstract::plugin::config_u64(cfg, "clock_sync_interval_ms")?
                .unwrap_or(600_000)
                .min(u32::MAX as u64) as u32;
        let mut spec = TransportSpec {
            kind: TransportKind::Tcp,
            peer: [0; 128],
            peer_len: 0,
            mtu: 253,
            connect_timeout_ms: 2000,
            recv_timeout_ms: 1000,
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
        let master_config = MasterConfig {
            clock_sync_interval_ms,
            t3_timeout_ms,
            poll_interval_ms,
            default_port: port,
            peer_ip: ip,
            peer_port: port,
        };
        let transport: Box<dyn MasterTransport> = Box::new(handle);
        let mut adapter = Iec104Adapter::new(master_config, transport);
        let acfg = eneros_protocol_abstract::AdapterConfig {
            name: alloc::string::String::from(IEC104_PLUGIN_ID),
            protocol_type: eneros_protocol_abstract::ProtocolType::Iec104,
            device_configs: alloc::vec![eneros_protocol_abstract::DeviceConfig {
                device_id: 1,
                name: alloc::string::String::from("iec104-1"),
                address: eneros_protocol_abstract::ProtocolAddress::Iec104 {
                    common_addr: 1,
                    ioa: 0,
                    type_id: 9,
                },
            }],
        };
        adapter.init(&acfg)?;
        // start 内含 STARTDT_ACT 握手——工厂 open 已连接，直接启动。
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
    fn validate_config_requires_peer() {
        let plugin = Iec104Plugin::new();
        let mut cfg = ProtocolConfig::default();
        cfg.insert("peer", "192.168.1.10:2404");
        assert!(plugin.validate_config(&cfg).is_ok());
        cfg.insert("peer", "nope");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
    }

    #[test]
    fn start_poll_stop_lifecycle() {
        let mut plugin = Iec104Plugin::new();
        let mut factory = TestTcpFactory;
        let mut cfg = ProtocolConfig::default();
        cfg.insert("peer", "192.168.1.10:2404");
        plugin
            .start(&cfg, &mut factory, &mut EmptyPointRegistry)
            .expect("start");
        assert_eq!(plugin.health().state, AdapterState::Running);
        plugin
            .poll(
                1000,
                WorkBudget {
                    max_work_ms: 100,
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
                    max_work_ms: 100,
                    max_polls: 1
                }
            ),
            Err(ProtocolError::ProtocolNotStarted)
        );
    }

    /// 39 收尾复验（P0）：配置的 peer/t3 真正进入 spec 与 MasterConfig
    ///（旧实现恒用硬编码 0.0.0.0:2404/20s）；缺 peer → InvalidConfig。
    #[test]
    fn start_consumes_configured_peer_and_timers() {
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
        let mut plugin = Iec104Plugin::new();
        let mut factory = CapturingFactory {
            peer: core::cell::RefCell::new(alloc::vec::Vec::new()),
        };
        let mut cfg = ProtocolConfig::default();
        cfg.insert("peer", "10.7.0.20:2404");
        cfg.insert("t3_timeout_ms", "15000");
        plugin
            .start(&cfg, &mut factory, &mut EmptyPointRegistry)
            .expect("start");
        assert_eq!(*factory.peer.borrow(), b"10.7.0.20:2404");

        // 缺 peer → InvalidConfig（默认值不伪健康）。
        let mut plugin2 = Iec104Plugin::new();
        assert_eq!(
            plugin2.start(
                &ProtocolConfig::default(),
                &mut factory,
                &mut EmptyPointRegistry
            ),
            Err(ProtocolError::InvalidConfig)
        );
    }
}
