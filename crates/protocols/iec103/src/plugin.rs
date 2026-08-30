//! IEC 103 插件（`ProtocolPlugin` 合同实现——蓝图 48 WP-48.3）.
//!
//! 结构与 `dlt645::plugin` 同型（Serial 工厂路径）；ASDU 自有格式 codec
//! BLOCKED → 数据面诚实 `Unsupported`；descriptor `interop_level =
//! SpecBlocked`。范围裁剪：扰动数据仅登记读取（无录波解析）。

use alloc::boxed::Box;

use eneros_protocol_abstract::{
    AdapterState, InteropLevel, PointRegistry, Progress, ProtocolConfig, ProtocolDescriptor,
    ProtocolError, ProtocolHealth, ProtocolPlugin, ProtocolRole, ResourceBudget, SecurityProfile,
    TransportFactory, TransportHandle, TransportKind, TransportSpec, WorkBudget,
};

use crate::config::{validate, Iec103Config};

/// 插件 id。
pub const IEC103_PLUGIN_ID: &str = "iec103";

/// IEC 103 master 插件。
pub struct Iec103Plugin {
    handle: Option<Box<dyn TransportHandle>>,
    cfg: Option<Iec103Config>,
    last_poll_ms: u64,
    error_count: u64,
    last_error: Option<ProtocolError>,
    last_progress: Progress,
    routed_count: usize,
}

impl Iec103Plugin {
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

    /// codec BLOCKED 登记行。
    pub fn spec_blocked_note() -> &'static str {
        "FT1.2 + proprietary ASDU BLOCKED (owner input: IEC 60870-5-103 spec text); disturbance data = registry-read only"
    }
}

impl Default for Iec103Plugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolPlugin for Iec103Plugin {
    fn descriptor(&self) -> &ProtocolDescriptor {
        static D: ProtocolDescriptor = ProtocolDescriptor {
            id: IEC103_PLUGIN_ID,
            version: "0.1",
            role: ProtocolRole::Master,
            transport_kinds: &[TransportKind::Serial],
            security_profile: SecurityProfile::None,
            // 范围裁剪（蓝图 WP-48.3）：扰动数据块传输仅登记读取，
            // 不做完整录波解析——点类型边界不含录波形数据。
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
                    eneros_protocol_abstract::ProtocolAddress::Iec103 { .. }
                )
            });
        if self.handle.is_some() {
            return Err(ProtocolError::ProtocolInit);
        }
        if !transports.supports(TransportKind::Serial) {
            return Err(ProtocolError::Unsupported);
        }
        let c = Iec103Config::from_validated(cfg)?;
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
            "iec103 routed={} {}",
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

    fn cfg_with_retries(retries: &str) -> ProtocolConfig {
        let mut cfg = ProtocolConfig::default();
        cfg.insert("common_addr", "1");
        cfg.insert("retries", retries);
        cfg
    }

    #[test]
    fn descriptor_is_honestly_spec_blocked() {
        let p = Iec103Plugin::new();
        assert_eq!(p.descriptor().id, "iec103");
        assert_eq!(p.descriptor().interop_level, InteropLevel::SpecBlocked);
        assert_eq!(p.descriptor().role, ProtocolRole::Master);
    }

    #[test]
    fn validate_rejects_unknown_keys_via_plugin() {
        let p = Iec103Plugin::new();
        assert_eq!(p.validate_config(&cfg_with_retries("2")), Ok(()));
        let mut bad = cfg_with_retries("2");
        bad.insert("evil", "x");
        assert_eq!(p.validate_config(&bad), Err(ProtocolError::InvalidConfig));
        // retries 非数字 → InvalidConfig。
        let nan = cfg_with_retries("abc");
        assert_eq!(p.validate_config(&nan), Err(ProtocolError::InvalidConfig));
    }

    #[test]
    fn lifecycle_consumes_config_and_reports_running_then_stopped() {
        let mut p = Iec103Plugin::new();
        p.start(
            &cfg_with_retries("3"),
            &mut SerialOkFactory,
            &mut EmptyPointRegistry,
        )
        .expect("start");
        assert_eq!(p.health().state, AdapterState::Running);
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
        assert_eq!(p.health().state, AdapterState::Uninitialized);
    }

    #[test]
    fn routed_count_counts_only_iec103_addresses() {
        let mappings = vec![ProtocolPointMapping {
            point_id: 7001,
            device_id: 4,
            protocol_addr: ProtocolAddress::Iec103 {
                common_addr: 1,
                fun: 160,
                inf: 90,
            },
            data_type: eneros_upa_model::PointType::Analog,
            scale: 1.0,
            offset: 0.0,
            identity: eneros_protocol_abstract::PointIdentityBinding::UNKNOWN,
        }];
        let mut reg = eneros_protocol_abstract::VecPointRegistry::new(mappings);
        let mut p = Iec103Plugin::new();
        p.start(&cfg_with_retries("2"), &mut SerialOkFactory, &mut reg)
            .expect("start");
        assert_eq!(p.routed_count(), 1);
        let line = p.debug_line().unwrap_or_default();
        assert!(line.contains("BLOCKED"));
        assert!(line.contains("registry-read only"));
    }
}
