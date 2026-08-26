//! DL/T 645 插件（`ProtocolPlugin` 六方法合同实现——蓝图 48 WP-48.1）.
//!
//! 结构与 `modbus-rtu::plugin` 同型（Serial 工厂路径，46.1 结论）；
//! 数据面（读/写点）因 codec BLOCKED 返回诚实 `Unsupported`——
//! descriptor `interop_level = SpecBlocked` 是唯一真实声称。

use alloc::boxed::Box;

use eneros_protocol_abstract::{
    AdapterState, InteropLevel, PointRegistry, Progress, ProtocolConfig, ProtocolDescriptor,
    ProtocolError, ProtocolHealth, ProtocolPlugin, ProtocolRole, ResourceBudget, SecurityProfile,
    TransportFactory, TransportHandle, TransportKind, TransportSpec, WorkBudget,
};

use crate::config::{validate, Cj188Config};

/// 插件 id（descriptor 与装配/证据共用）。
pub const CJ188_PLUGIN_ID: &str = "cj188";

/// DL/T 645 master 插件。
pub struct Cj188Plugin {
    handle: Option<Box<dyn TransportHandle>>,
    cfg: Option<Cj188Config>,
    last_poll_ms: u64,
    error_count: u64,
    last_error: Option<ProtocolError>,
    last_progress: Progress,
    /// 45.4：注入点表路由点数（观测；服务层按协议过滤注入）。
    routed_count: usize,
}

impl Cj188Plugin {
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

    /// codec BLOCKED 登记行（debug 观测；服务侧 debug_line 透出）。
    pub fn spec_blocked_note() -> &'static str {
        "codec BLOCKED (owner input: DL/T 645-2007 spec text)"
    }
}

impl Default for Cj188Plugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolPlugin for Cj188Plugin {
    fn descriptor(&self) -> &ProtocolDescriptor {
        static D: ProtocolDescriptor = ProtocolDescriptor {
            id: CJ188_PLUGIN_ID,
            version: "0.1",
            role: ProtocolRole::Master,
            transport_kinds: &[TransportKind::Serial],
            security_profile: SecurityProfile::None,
            point_types: &["float", "int", "bool"],
            resource_budget: ResourceBudget {
                max_connections: 1,
                max_poll_ms: 50,
                max_frame: 36,
            },
            // 诚实分级：codec 字节层 BLOCKED（规约文本未提供）。
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
        _points: &mut dyn PointRegistry,
    ) -> Result<(), ProtocolError> {
        // F-3 登记口径（SPEC-GAP-CLOSURE）：CJ188 线码变体未进
        // ProtocolAddress 枚举（spec 明确 OUT），点表路由暂置 0——挂载日随
        // 线码扩展启用计数；其余 seam 照常，不阻断装配纪律。
        self.routed_count = 0;
        if self.handle.is_some() {
            return Err(ProtocolError::ProtocolInit);
        }
        if !transports.supports(TransportKind::Serial) {
            return Err(ProtocolError::Unsupported);
        }
        let c = Cj188Config::from_validated(cfg)?;
        let spec = TransportSpec {
            kind: TransportKind::Serial,
            peer: [0; 128],
            peer_len: 0,
            mtu: 36,
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
        // codec BLOCKED：链路已就绪但无帧可编解码——诚实 Idle（不伪造
        // Completed/Degraded；错误计数不动）。
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
            "cj188 routed={} {}",
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

    /// 测试用外来地址映射（DLT645 变体借用——CJ188 线码未挂载前的替身）。
    fn foreign_addr(addr: u64) -> ProtocolPointMapping {
        ProtocolPointMapping {
            point_id: 9001,
            device_id: 7,
            protocol_addr: ProtocolAddress::Dlt645 { addr_bcd: addr },
            data_type: eneros_upa_model::PointType::Analog,
            scale: 0.1,
            offset: 0.0,
        }
    }

    #[test]
    fn descriptor_is_honestly_spec_blocked() {
        let p = Cj188Plugin::new();
        assert_eq!(p.descriptor().id, "cj188");
        assert_eq!(p.descriptor().interop_level, InteropLevel::SpecBlocked);
        assert_eq!(p.descriptor().role, ProtocolRole::Master);
        assert!(p
            .descriptor()
            .transport_kinds
            .contains(&TransportKind::Serial));
    }

    #[test]
    fn validate_rejects_unknown_and_out_of_range() {
        let p = Cj188Plugin::new();
        let mut cfg = ProtocolConfig::default();
        cfg.insert("baud", "2400");
        assert_eq!(p.validate_config(&cfg), Ok(()));
        cfg.insert("unknown_k", "v");
        assert_eq!(p.validate_config(&cfg), Err(ProtocolError::InvalidConfig));
        let mut bad = ProtocolConfig::default();
        bad.insert("baud", "115200");
        assert_eq!(p.validate_config(&bad), Err(ProtocolError::InvalidConfig));
    }

    #[test]
    fn start_consumes_config_and_reports_running() {
        let mut p = Cj188Plugin::new();
        let mut cfg = ProtocolConfig::default();
        cfg.insert("addr_bcd", "123456789012");
        cfg.insert("timeout_ms", "500");
        p.start(&cfg, &mut SerialOkFactory, &mut EmptyPointRegistry)
            .expect("start");
        assert_eq!(p.health().state, AdapterState::Running);
        // poll：codec BLOCKED → 诚实 Idle。
        let progress = p.poll(
            1000,
            WorkBudget {
                max_work_ms: 10,
                max_polls: 1,
            },
        );
        assert_eq!(progress, Ok(Progress::Idle));
        assert_eq!(p.health().last_poll_ms, 1000);
        p.stop().expect("stop");
        assert_eq!(p.health().state, AdapterState::Uninitialized);
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
    }

    #[test]
    fn start_rejects_non_serial_factory() {
        let mut p = Cj188Plugin::new();
        assert_eq!(
            p.start(
                &ProtocolConfig::default(),
                &mut TcpOnlyFactory,
                &mut EmptyPointRegistry
            ),
            Err(ProtocolError::Unsupported)
        );
    }

    #[test]
    fn routed_count_pends_cj188_line_code_mount() {
        let mut p = Cj188Plugin::new();
        let mappings = vec![
            foreign_addr(1),
            ProtocolPointMapping {
                point_id: 1002,
                device_id: 1,
                protocol_addr: ProtocolAddress::Modbus {
                    slave_addr: 1,
                    reg_addr: 1,
                    func_code: 3,
                },
                data_type: eneros_upa_model::PointType::Analog,
                scale: 1.0,
                offset: 0.0,
            },
            foreign_addr(999999999999),
        ];
        let mut reg = eneros_protocol_abstract::VecPointRegistry::new(mappings);
        let cfg = ProtocolConfig::default();
        p.start(&cfg, &mut SerialOkFactory, &mut reg)
            .expect("start");
        // 线码变体未挂载 → 计数恒 0（F-3 登记口径锁定）。
        assert_eq!(p.routed_count(), 0);
    }

    #[test]
    fn read_point_is_unsupported_until_codec_lands() {
        let mut p = Cj188Plugin::new();
        let cfg = ProtocolConfig::default();
        p.start(&cfg, &mut SerialOkFactory, &mut EmptyPointRegistry)
            .expect("start");
        let m = foreign_addr(1);
        assert_eq!(p.read_point(0, &m), Err(ProtocolError::Unsupported));
    }

    #[test]
    fn debug_line_carries_blocked_registration() {
        let p = Cj188Plugin::new();
        let line = p.debug_line().unwrap_or_default();
        assert!(line.contains("BLOCKED"));
        assert!(line.contains("DL/T 645-2007"));
    }
}
