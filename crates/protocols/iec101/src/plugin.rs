//! IEC 101 插件（`ProtocolPlugin` 合同实现——蓝图 48 WP-48.2）.
//!
//! 结构与 `dlt645::plugin` 同型（Serial 工厂路径）；数据面因 FT1.2
//! BLOCKED 返回诚实 `Unsupported`；descriptor `interop_level = SpecBlocked`。
//! slave 侧插件（主从成对）随链路层一并 BLOCKED——登记于 crate 文档。

use alloc::boxed::Box;

use eneros_protocol_abstract::{
    AdapterState, InteropLevel, PointRegistry, Progress, ProtocolConfig, ProtocolDescriptor,
    ProtocolError, ProtocolHealth, ProtocolPlugin, ProtocolRole, ResourceBudget, SecurityProfile,
    TransportFactory, TransportHandle, TransportKind, TransportSpec, WorkBudget,
};

use crate::config::{validate, Iec101Config};

/// 插件 id。
pub const IEC101_PLUGIN_ID: &str = "iec101";

/// IEC 101 master 插件。
pub struct Iec101Plugin {
    handle: Option<Box<dyn TransportHandle>>,
    cfg: Option<Iec101Config>,
    last_poll_ms: u64,
    error_count: u64,
    last_error: Option<ProtocolError>,
    last_progress: Progress,
    routed_count: usize,
}

impl Iec101Plugin {
    pub fn new() -> Self {
        Self {
            handle: None,
            cfg: None,
            last_poll_ms: 0,
            error_count: 0,
            last_error: None,
            last_progress: Progress::Idle,
            routed_count: 0,
        }
    }

    /// 路由点数（观测）。
    pub fn routed_count(&self) -> usize {
        self.routed_count
    }

    /// 已消费配置（观测/诊断）。
    pub fn active_config(&self) -> Option<&Iec101Config> {
        self.cfg.as_ref()
    }

    /// codec BLOCKED 登记行。
    pub fn spec_blocked_note() -> &'static str {
        "FT1.2 link layer BLOCKED (owner input: IEC 60870-5-101 spec text)"
    }
}

impl Default for Iec101Plugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolPlugin for Iec101Plugin {
    fn descriptor(&self) -> &ProtocolDescriptor {
        static D: ProtocolDescriptor = ProtocolDescriptor {
            id: IEC101_PLUGIN_ID,
            version: "0.1",
            role: ProtocolRole::Master,
            transport_kinds: &[TransportKind::Serial],
            security_profile: SecurityProfile::None,
            point_types: &["float", "int", "bool"],
            resource_budget: ResourceBudget {
                max_connections: 1,
                max_poll_ms: 50,
                max_frame: 255,
            },
            interop_level: InteropLevel::SpecBlocked,
        };
        &D
    }

    fn validate_config(&self, cfg: &ProtocolConfig) -> Result<(), ProtocolError> {
        validate(cfg)
    }

    fn start(
        &mut self,
        cfg: &ProtocolConfig,
        transports: &mut dyn TransportFactory,
        points: &mut dyn PointRegistry,
    ) -> Result<(), ProtocolError> {
        self.routed_count =
            eneros_protocol_abstract::plugin::count_matching_mappings(points, |m| {
                matches!(
                    m.protocol_addr,
                    eneros_protocol_abstract::ProtocolAddress::Iec101 { .. }
                )
            });
        if self.handle.is_some() {
            return Err(ProtocolError::ProtocolInit);
        }
        if !transports.supports(TransportKind::Serial) {
            return Err(ProtocolError::Unsupported);
        }
        let c = Iec101Config::from_validated(cfg)?;
        let spec = TransportSpec {
            kind: TransportKind::Serial,
            peer: [0; 128],
            peer_len: 0,
            mtu: 255,
            connect_timeout_ms: c.timeout_ms,
            recv_timeout_ms: c.timeout_ms,
        };
        let handle = transports
            .open(&spec)
            .map_err(|_| ProtocolError::ProtocolInit)?;
        self.cfg = Some(c);
        self.handle = Some(handle);
        Ok(())
    }

    fn poll(&mut self, now_ms: u64, _budget: WorkBudget) -> Result<Progress, ProtocolError> {
        if self.handle.is_none() {
            return Err(ProtocolError::ProtocolNotStarted);
        }
        self.last_poll_ms = now_ms;
        // FT1.2 BLOCKED：链路就绪但无帧可编解码——诚实 Idle。
        self.last_progress = Progress::Idle;
        Ok(Progress::Idle)
    }

    fn read_point(
        &mut self,
        _now_ms: u64,
        _mapping: &eneros_protocol_abstract::ProtocolPointMapping,
    ) -> Result<eneros_protocol_abstract::PointRead, ProtocolError> {
        Err(ProtocolError::Unsupported)
    }

    fn health(&self) -> ProtocolHealth {
        let state = if self.handle.is_some() {
            AdapterState::Running
        } else {
            AdapterState::Uninitialized
        };
        ProtocolHealth {
            state,
            last_poll_ms: self.last_poll_ms,
            error_count: self.error_count,
            last_error: self.last_error.clone(),
            last_progress: self.last_progress,
        }
    }

    fn debug_line(&self) -> Option<alloc::string::String> {
        Some(alloc::format!(
            "iec101 routed={} {}",
            self.routed_count,
            Self::spec_blocked_note()
        ))
    }

    fn stop(&mut self) -> Result<(), ProtocolError> {
        self.handle = None;
        self.cfg = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use eneros_protocol_abstract::{
        AdapterState, EmptyPointRegistry, InteropLevel, ProtocolAddress, ProtocolPointMapping,
        TransportError,
    };

    use super::*;

    struct SerialOkFactory;
    impl TransportFactory for SerialOkFactory {
        fn open(
            &mut self,
            _spec: &TransportSpec,
        ) -> Result<Box<dyn TransportHandle>, TransportError> {
            Ok(Box::new(NullHandle))
        }
        fn supports(&self, kind: TransportKind) -> bool {
            kind == TransportKind::Serial
        }
    }

    struct TcpOnlyFactory;
    impl TransportFactory for TcpOnlyFactory {
        fn open(
            &mut self,
            _spec: &TransportSpec,
        ) -> Result<Box<dyn TransportHandle>, TransportError> {
            Err(TransportError::Unsupported)
        }
        fn supports(&self, kind: TransportKind) -> bool {
            kind == TransportKind::Tcp
        }
    }

    struct NullHandle;
    impl TransportHandle for NullHandle {
        fn send(&mut self, _data: &[u8]) -> Result<(), TransportError> {
            Ok(())
        }
        fn recv(&mut self, _buf: &mut [u8]) -> Result<usize, TransportError> {
            Ok(0)
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

    fn full_cfg() -> ProtocolConfig {
        let mut cfg = ProtocolConfig::default();
        cfg.insert("link_addr", "7");
        cfg.insert("common_addr", "1");
        cfg.insert("link_addr_size", "1");
        cfg.insert("common_addr_size", "2");
        cfg.insert("t0_ms", "30");
        cfg.insert("t1_ms", "300");
        cfg.insert("t2_ms", "200");
        cfg.insert("t3_ms", "20000");
        cfg.insert("k", "12");
        cfg.insert("w", "8");
        cfg
    }

    #[test]
    fn descriptor_is_honestly_spec_blocked() {
        let p = Iec101Plugin::new();
        assert_eq!(p.descriptor().id, "iec101");
        assert_eq!(p.descriptor().interop_level, InteropLevel::SpecBlocked);
        assert!(p
            .descriptor()
            .transport_kinds
            .contains(&TransportKind::Serial));
    }

    #[test]
    fn validate_and_start_consume_config() {
        let mut p = Iec101Plugin::new();
        p.start(&full_cfg(), &mut SerialOkFactory, &mut EmptyPointRegistry)
            .expect("start");
        assert_eq!(p.health().state, AdapterState::Running);
        let c = p.active_config().expect("consumed");
        assert_eq!(c.link_addr, 7);
        assert_eq!(c.common_addr_size, 2);
        // 超时组缺失 → start 拒绝（fail-closed）。
        let mut p2 = Iec101Plugin::new();
        let mut thin = ProtocolConfig::default();
        thin.insert("link_addr", "7");
        assert_eq!(
            p2.start(&thin, &mut SerialOkFactory, &mut EmptyPointRegistry),
            Err(ProtocolError::InvalidConfig)
        );
    }

    #[test]
    fn lifecycle_and_non_serial_rejection() {
        let mut p = Iec101Plugin::new();
        p.start(&full_cfg(), &mut SerialOkFactory, &mut EmptyPointRegistry)
            .expect("start");
        assert_eq!(
            p.poll(
                1000,
                WorkBudget {
                    max_work_ms: 10,
                    max_polls: 1
                }
            ),
            Ok(Progress::Idle)
        );
        p.stop().expect("stop");
        assert_eq!(
            p.poll(
                2000,
                WorkBudget {
                    max_work_ms: 10,
                    max_polls: 1
                }
            ),
            Err(ProtocolError::ProtocolNotStarted)
        );
        let mut q = Iec101Plugin::new();
        assert_eq!(
            q.start(&full_cfg(), &mut TcpOnlyFactory, &mut EmptyPointRegistry),
            Err(ProtocolError::Unsupported)
        );
    }

    #[test]
    fn routed_count_counts_only_iec101_addresses() {
        let mappings = vec![
            ProtocolPointMapping {
                point_id: 5001,
                device_id: 2,
                protocol_addr: ProtocolAddress::Iec101 {
                    link_addr: 7,
                    common_addr: 1,
                    ioa: 100,
                },
                data_type: eneros_upa_model::PointType::Analog,
                scale: 1.0,
                offset: 0.0,
                identity: eneros_protocol_abstract::PointIdentityBinding::UNKNOWN,
            },
            ProtocolPointMapping {
                point_id: 5002,
                device_id: 2,
                protocol_addr: ProtocolAddress::Iec104 {
                    common_addr: 1,
                    ioa: 100,
                    type_id: 9,
                },
                data_type: eneros_upa_model::PointType::Analog,
                scale: 1.0,
                offset: 0.0,
                identity: eneros_protocol_abstract::PointIdentityBinding::UNKNOWN,
            },
        ];
        let mut reg = eneros_protocol_abstract::VecPointRegistry::new(mappings);
        let mut p = Iec101Plugin::new();
        p.start(&full_cfg(), &mut SerialOkFactory, &mut reg)
            .expect("start");
        assert_eq!(p.routed_count(), 1);
    }

    #[test]
    fn read_point_unsupported_and_debug_line_registers_blocked() {
        let mut p = Iec101Plugin::new();
        p.start(&full_cfg(), &mut SerialOkFactory, &mut EmptyPointRegistry)
            .expect("start");
        let m = ProtocolPointMapping {
            point_id: 5001,
            device_id: 2,
            protocol_addr: ProtocolAddress::Iec101 {
                link_addr: 7,
                common_addr: 1,
                ioa: 100,
            },
            data_type: eneros_upa_model::PointType::Analog,
            scale: 1.0,
            offset: 0.0,
            identity: eneros_protocol_abstract::PointIdentityBinding::UNKNOWN,
        };
        assert_eq!(p.read_point(0, &m), Err(ProtocolError::Unsupported));
        let line = p.debug_line().unwrap_or_default();
        assert!(line.contains("BLOCKED"));
        assert!(line.contains("IEC 60870-5-101"));
    }
}
