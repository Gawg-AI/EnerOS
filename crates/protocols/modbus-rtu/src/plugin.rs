//! Modbus RTU 协议插件（蓝图 39 WP-39.11：ProductManifest 声明的协议以
//! 独立 service + 生产 transport 运行——本合同是服务装配的统一入口）。
//!
//! 插件**硬件无关**：经 [`TransportFactory`] 获取 `Box<dyn TransportHandle>`
//! （RS485/UART 由工厂注入），`Box<dyn TransportHandle>` 实现
//! [`RtuTransport`]（本地 trait 实现外来类型——合法），适配器直接消费。
//! 新增串口硬件只换工厂，不修改插件/适配器。

use alloc::boxed::Box;

use eneros_driver_framework::DriverError;
use eneros_protocol_abstract::{
    AdapterState, InteropLevel, PointRead, PointRegistry, Progress, ProtocolAdapter,
    ProtocolAddress, ProtocolConfig, ProtocolDescriptor, ProtocolError, ProtocolHealth,
    ProtocolPlugin, ProtocolPointMapping, ProtocolRole, ResourceBudget, SecurityProfile,
    TransportFactory, TransportHandle, TransportKind, TransportSpec, WorkBudget,
};

use crate::adapter::ModbusRtuAdapter;
use crate::master::RtuTransport;

/// 插件 id（descriptor 与装配/证据共用）。
pub const RTU_PLUGIN_ID: &str = "modbus-rtu";

/// `Box<dyn TransportHandle>` 实现 [`RtuTransport`]——插件与硬件解耦。
impl RtuTransport for Box<dyn TransportHandle> {
    fn send(&mut self, data: &[u8]) -> Result<(), DriverError> {
        TransportHandle::send(self.as_mut(), data).map_err(|_| DriverError::InitFailed)
    }
    fn recv(&mut self, _timeout_ms: u32) -> Result<alloc::vec::Vec<u8>, DriverError> {
        // 有界接收：RTU 帧 ≤ 256 B；错误映射见 map_transport_recv_error
        //（39 §6.3：断连与超时可区分，断连不重试 fail-closed）。
        let mut buf = [0u8; 512];
        let n = TransportHandle::recv(self.as_mut(), &mut buf).map_err(map_transport_recv_error)?;
        Ok(buf[..n].to_vec())
    }
}

/// 39 §6.3 负例（断连 vs 超时）：recv 错误映射。
///
/// `DriverError` 无 LinkDown 变体（枚举不强行新增——登记于本注释）；
/// `TransportError::Closed`（链路已关闭/失效）映射为 `InvalidState`
/// 作为"断连"等价物：它不是 Timeout，`send_request_with_retry` 对其
/// 立即失败不重试（fail-closed）；其余错误（Timeout/Io/...）映射为
/// Timeout，重试语义由 master 接管。
fn map_transport_recv_error(e: eneros_protocol_abstract::TransportError) -> DriverError {
    use eneros_protocol_abstract::TransportError;
    match e {
        TransportError::Closed => DriverError::InvalidState,
        TransportError::Timeout
        | TransportError::Io
        | TransportError::ConnectFailed
        | TransportError::Unsupported => DriverError::Timeout,
    }
}

/// 已知配置键（未知键 → `InvalidConfig`，WP-39.10 不变量；蓝图 64
/// WP-64.1.2 起单源引用 `eneros-protocol-keys`，双源同步义务随收口消灭）。
///
/// SPEC-D WP-D.2 键语义与真消费面（配置零消费禁令）：
/// - `baud` / `parity` / `clock_hz`：svc-modbus-rtu 装配
///   [`eneros_protocol_services::net_transport::SerialTransportFactory`] /
///   `Pl011UartHw` 时解析注入；缺失/非法 → 能力壳 fail-closed，不猜
///   9600 默认值冒充活路。
/// - `slave_id` / `timeout_ms` / `retries`：插件 start 消费（39 既有）。
const KNOWN_KEYS: &[&str] = eneros_protocol_keys::MODBUS_RTU;

/// Modbus RTU 插件（服务级合同实现）。
pub struct RtuPlugin {
    adapter: Option<ModbusRtuAdapter<Box<dyn TransportHandle>>>,
    started: bool,
    last_poll_ms: u64,
    error_count: u64,
    last_error: Option<ProtocolError>,
    last_progress: Progress,
    /// 45.4：注入点表路由点数（观测；服务层按协议过滤注入）。
    routed_count: usize,
    /// SPEC-D WP-D.2：read_point 序号（单调；重放判定/观测，同 TcpPlugin）。
    point_seq: u64,
}

impl RtuPlugin {
    pub fn new() -> Self {
        Self {
            adapter: None,
            started: false,
            last_poll_ms: 0,
            error_count: 0,
            last_error: None,
            last_progress: Progress::Idle,
            routed_count: 0,
            point_seq: 0,
        }
    }

    /// 45.4：注入点表路由点数（观测）。
    pub fn routed_count(&self) -> usize {
        self.routed_count
    }

    fn config_timeout(cfg: &ProtocolConfig) -> u32 {
        cfg.get("timeout_ms")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1000)
    }

    fn config_retries(cfg: &ProtocolConfig) -> u8 {
        cfg.get("retries").and_then(|v| v.parse().ok()).unwrap_or(2)
    }
}

impl Default for RtuPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolPlugin for RtuPlugin {
    fn descriptor(&self) -> &ProtocolDescriptor {
        static D: ProtocolDescriptor = ProtocolDescriptor {
            id: RTU_PLUGIN_ID,
            version: "1.0",
            role: ProtocolRole::Master,
            transport_kinds: &[TransportKind::Serial],
            security_profile: SecurityProfile::None,
            point_types: &["float", "int", "bool"],
            resource_budget: ResourceBudget {
                max_connections: 1,
                max_poll_ms: 50,
                max_frame: 256,
            },
            interop_level: InteropLevel::CodecReady,
        };
        &D
    }

    fn validate_config(&self, cfg: &ProtocolConfig) -> Result<(), ProtocolError> {
        eneros_protocol_abstract::plugin::reject_unknown_keys(cfg, KNOWN_KEYS)
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
                    eneros_protocol_abstract::ProtocolAddress::Modbus { .. }
                )
            });
        if self.adapter.is_some() {
            return Err(ProtocolError::ProtocolInit);
        }
        if !transports.supports(TransportKind::Serial) {
            return Err(ProtocolError::Unsupported);
        }
        // 39 收尾复验（P0）：timeout/retries/slave_id 来自校验过的配置
        // 本体（旧实现此处重建 default 空配置——配置零消费）。
        let slave_id =
            eneros_protocol_abstract::plugin::config_u64(cfg, "slave_id")?.unwrap_or(1) as u8;
        let spec = TransportSpec {
            kind: TransportKind::Serial,
            peer: [0; 128],
            peer_len: 0,
            mtu: 256,
            connect_timeout_ms: Self::config_timeout(cfg) as u64,
            recv_timeout_ms: Self::config_timeout(cfg) as u64,
        };
        let handle = transports
            .open(&spec)
            .map_err(|_| ProtocolError::ProtocolInit)?;
        // 适配器直接消费 `Box<dyn TransportHandle>`（RtuTransport impl 在上）。
        let mut adapter =
            ModbusRtuAdapter::new(handle, Self::config_timeout(cfg), Self::config_retries(cfg));
        // 完整生命周期：init（配置绑定）→ start（Running）。
        let adapter_cfg = eneros_protocol_abstract::AdapterConfig {
            name: alloc::string::String::from(RTU_PLUGIN_ID),
            protocol_type: eneros_protocol_abstract::ProtocolType::ModbusRtu,
            device_configs: alloc::vec![eneros_protocol_abstract::DeviceConfig {
                device_id: 1,
                name: alloc::string::String::from("rtu-1"),
                address: eneros_protocol_abstract::ProtocolAddress::Modbus {
                    slave_addr: slave_id,
                    reg_addr: 0,
                    func_code: 3,
                },
            }],
        };
        adapter.init(&adapter_cfg)?;
        adapter.start()?;
        self.adapter = Some(adapter);
        self.started = true;
        Ok(())
    }

    /// SPEC-D WP-D.2（C1 全链）：点表驱动单点真读——RTU 侧 FC03 保持寄存器
    /// 单字（FC04 输入寄存器未接 master 读面 → `Unsupported` 诚实上抛，
    /// 不以保持寄存器帧冒充输入寄存器）。**只读**——授权语义与 TcpPlugin
    /// 相同：地址来自注入点表，本方法不自造地址。
    fn read_point(
        &mut self,
        _now_ms: u64,
        mapping: &ProtocolPointMapping,
    ) -> Result<PointRead, ProtocolError> {
        let Some(adapter) = self.adapter.as_mut() else {
            return Err(ProtocolError::ProtocolNotStarted);
        };
        let (slave_addr, reg_addr, func_code) = match mapping.protocol_addr {
            ProtocolAddress::Modbus {
                slave_addr,
                reg_addr,
                func_code,
            } => (slave_addr, reg_addr, func_code),
            _ => return Err(ProtocolError::AddrTypeMismatch),
        };
        if func_code != 3 {
            return Err(ProtocolError::Unsupported);
        }
        let regs = adapter
            .read_raw_registers(slave_addr, reg_addr, 1)
            .map_err(crate::adapter::modbus_to_protocol)?;
        let raw = regs.first().copied().ok_or(ProtocolError::ReadFailed)? as i64;
        self.point_seq = self.point_seq.saturating_add(1);
        Ok(PointRead {
            raw,
            ok: true,
            seq: self.point_seq,
        })
    }

    /// SPEC-D WP-D.2（C1 全链）：点表驱动 FC06 单寄存器写。工程值经映射
    /// 逆变换（scale/offset；越界/非数值 → 明确错误，不截断静默）。
    /// **只执行**——授权已由 ControlDispatch 九步链完成（不双头授权）。
    fn write_point(
        &mut self,
        _now_ms: u64,
        mapping: &ProtocolPointMapping,
        value: eneros_upa_model::PointValue,
    ) -> Result<(), ProtocolError> {
        let Some(adapter) = self.adapter.as_mut() else {
            return Err(ProtocolError::ProtocolNotStarted);
        };
        let (slave_addr, reg_addr) = match mapping.protocol_addr {
            ProtocolAddress::Modbus {
                slave_addr,
                reg_addr,
                ..
            } => (slave_addr, reg_addr),
            _ => return Err(ProtocolError::AddrTypeMismatch),
        };
        if mapping.scale == 0.0 {
            return Err(ProtocolError::InvalidConfig);
        }
        let raw_f = match value {
            eneros_upa_model::PointValue::Float(f) if f.is_finite() => f,
            eneros_upa_model::PointValue::Int(i) => i as f64,
            eneros_upa_model::PointValue::Bool(b) => {
                if b {
                    1.0
                } else {
                    0.0
                }
            }
            eneros_upa_model::PointValue::Enum(e) => e as f64,
            _ => return Err(ProtocolError::Unsupported),
        };
        let raw_i = mapping.from_engineering(raw_f);
        let raw = u16::try_from(raw_i).map_err(|_| ProtocolError::WriteFailed)?;
        adapter
            .write_register_raw(slave_addr, reg_addr, raw)
            .map_err(crate::adapter::modbus_to_protocol)
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
    use eneros_protocol_abstract::EmptyPointRegistry;
    use eneros_protocol_abstract::TransportError;
    use eneros_rs485::mock::MockUartHw;

    use super::*;

    /// 测试串口工厂（MockUartHw → Rs485RtuTransport → 句柄）。
    struct TestSerialFactory {
        uart: Option<MockUartHw>,
    }

    impl TestSerialFactory {
        fn new() -> Self {
            Self {
                uart: Some(MockUartHw::default()),
            }
        }
    }

    impl TransportFactory for TestSerialFactory {
        fn open(
            &mut self,
            spec: &TransportSpec,
        ) -> Result<alloc::boxed::Box<dyn TransportHandle>, TransportError> {
            if spec.kind != TransportKind::Serial {
                return Err(TransportError::Unsupported);
            }
            let uart = self.uart.take().ok_or(TransportError::Closed)?;
            let transport = crate::transport::Rs485RtuTransport::new(uart, 9600);
            Ok(alloc::boxed::Box::new(transport))
        }
        fn supports(&self, kind: TransportKind) -> bool {
            kind == TransportKind::Serial
        }
    }

    impl TransportHandle for crate::transport::Rs485RtuTransport<MockUartHw> {
        fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
            crate::master::RtuTransport::send(self, data).map_err(|_| TransportError::Io)
        }
        fn recv(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
            let data = crate::master::RtuTransport::recv(self, 100)
                .map_err(|_| TransportError::Timeout)?;
            let n = data.len().min(buf.len());
            buf[..n].copy_from_slice(&data[..n]);
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

    #[test]
    fn validate_config_rejects_unknown_keys() {
        let plugin = RtuPlugin::new();
        let mut cfg = ProtocolConfig::default();
        cfg.insert("slave_id", "1");
        assert!(plugin.validate_config(&cfg).is_ok());
        cfg.insert("evil", "x");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
    }

    /// SPEC-D WP-D.2 键表锁定（蓝图 64 WP-64.1.3 迁移为引用锁）：
    /// KNOWN_KEYS = 键表单源 `eneros-protocol-keys::MODBUS_RTU`。迁移前
    /// 6 键字面锁 diff 见执行卡（键集合不变）；对侧锁在 config-bundle
    /// protocol.rs d_rtu_known_keys_reference_lock。
    #[test]
    fn d_known_keys_reference_lock() {
        assert_eq!(KNOWN_KEYS, eneros_protocol_keys::MODBUS_RTU);
    }

    #[test]
    fn start_poll_stop_lifecycle() {
        let mut plugin = RtuPlugin::new();
        let mut factory = TestSerialFactory::new();
        let cfg = ProtocolConfig::default();
        plugin
            .start(&cfg, &mut factory, &mut EmptyPointRegistry)
            .expect("start");
        assert_eq!(plugin.health().state, AdapterState::Running);
        let progress = plugin.poll(
            1000,
            WorkBudget {
                max_work_ms: 50,
                max_polls: 1,
            },
        );
        assert!(progress.is_ok());
        assert!(plugin.health().last_poll_ms >= 1000);
        plugin.stop().expect("stop");
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

    /// 39 收尾复验（§6.3 断连 vs 超时）：recv 错误映射单测——
    /// Closed → InvalidState（断连等价物，DriverError 无 LinkDown，
    /// 登记于 map_transport_recv_error 注释）；Timeout/Io → Timeout。
    #[test]
    fn test_39_recv_error_mapping_distinguishes_closed_from_timeout() {
        assert_eq!(
            map_transport_recv_error(TransportError::Closed),
            DriverError::InvalidState
        );
        assert_eq!(
            map_transport_recv_error(TransportError::Timeout),
            DriverError::Timeout
        );
        assert_eq!(
            map_transport_recv_error(TransportError::Io),
            DriverError::Timeout
        );
    }

    /// recv 注入固定错误的句柄（断连/超时注入用）。
    struct ErrHandle(TransportError);

    impl TransportHandle for ErrHandle {
        fn send(&mut self, _data: &[u8]) -> Result<(), TransportError> {
            Ok(())
        }
        fn recv(&mut self, _buf: &mut [u8]) -> Result<usize, TransportError> {
            Err(self.0)
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

    /// 39 收尾复验（§6.3 断连）：`RtuTransport for Box<dyn TransportHandle>`
    /// 的 recv 经真实句柄路径——链路关闭（Closed）→ InvalidState 而非
    /// Timeout；超时仍 → Timeout。两者可区分，断连不会被 master 当作
    /// 可重试超时（fail-closed）。
    #[test]
    fn test_39_rtu_recv_closed_vs_timeout_via_handle() {
        let mut h: alloc::boxed::Box<dyn TransportHandle> =
            alloc::boxed::Box::new(ErrHandle(TransportError::Closed));
        assert_eq!(
            crate::master::RtuTransport::recv(&mut h, 100),
            Err(DriverError::InvalidState)
        );
        let mut h: alloc::boxed::Box<dyn TransportHandle> =
            alloc::boxed::Box::new(ErrHandle(TransportError::Timeout));
        assert_eq!(
            crate::master::RtuTransport::recv(&mut h, 100),
            Err(DriverError::Timeout)
        );
    }

    #[test]
    fn start_rejects_missing_serial_support() {
        let mut plugin = RtuPlugin::new();
        // 只支持 TCP 的工厂 → Unsupported（fail-closed）。
        struct TcpOnly;
        impl TransportFactory for TcpOnly {
            fn open(
                &mut self,
                _spec: &TransportSpec,
            ) -> Result<alloc::boxed::Box<dyn TransportHandle>, TransportError> {
                Err(TransportError::Unsupported)
            }
            fn supports(&self, kind: TransportKind) -> bool {
                kind == TransportKind::Tcp
            }
        }
        assert_eq!(
            plugin.start(
                &ProtocolConfig::default(),
                &mut TcpOnly,
                &mut EmptyPointRegistry
            ),
            Err(ProtocolError::Unsupported)
        );
    }
}
