//! Modbus TCP 协议插件（蓝图 39 WP-39.11）。
//!
//! 与 RTU 插件同构：硬件无关，经 [`TransportFactory`] 获取
//! `Box<dyn TransportHandle>`；`Box<dyn TransportHandle>` 实现
//! [`TcpTransport`]，适配器直接消费。

use alloc::borrow::ToOwned;
use alloc::boxed::Box;

use eneros_protocol_abstract::{
    parse_ip_port, AdapterState, InteropLevel, PointRead, PointRegistry, Progress, ProtocolAdapter,
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

/// 已知配置键（蓝图 64 WP-64.1.2 起单源引用 `eneros-protocol-keys`；
/// 键序取 config-bundle 旧序——retries/timeout_ms 互换无行为语义）。
const KNOWN_KEYS: &[&str] = eneros_protocol_keys::MODBUS_TCP;

/// Modbus TCP 插件。
pub struct TcpPlugin {
    adapter: Option<ModbusTcpAdapter<Box<dyn TransportHandle>>>,
    started: bool,
    last_poll_ms: u64,
    error_count: u64,
    last_error: Option<ProtocolError>,
    last_progress: Progress,
    /// 45.4：注入点表中可寻址（Modbus 地址）点计数（观测）。
    routed_count: usize,
    /// 45.4：read_point 序号（单调；重放判定/观测）。
    point_seq: u64,
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
            routed_count: 0,
            point_seq: 0,
        }
    }

    /// 蓝图 61 v0.314.30（[07] §2.3）：寄存器组读——相邻寄存器合并读取
    /// 的传输原语（`start..start+words`；words ∈ [1,125] Modbus 上限）。
    ///
    /// 组内任何异常按点质量展开（调用方处理），不把整组假装成功。
    pub fn read_group(
        &mut self,
        reg_addr: u16,
        func_code: u8,
        words: u16,
    ) -> Result<alloc::vec::Vec<u16>, ProtocolError> {
        let Some(adapter) = self.adapter.as_mut() else {
            return Err(ProtocolError::ProtocolNotStarted);
        };
        adapter.read_raw(reg_addr, func_code, words)
    }

    /// 45.4：注入点表路由点数（观测）。
    pub fn routed_count(&self) -> usize {
        self.routed_count
    }

    /// 45.4：read_point 单调序号（观测）。
    pub fn point_seq(&self) -> u64 {
        self.point_seq
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
            // 46.5：QEMU/EL0 + 独立对端 L2 闭环（evidence/46/WP-46.5.md）。
            interop_level: InteropLevel::TargetPeerVerified,
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
        points: &mut dyn PointRegistry,
    ) -> Result<(), ProtocolError> {
        if self.adapter.is_some() {
            return Err(ProtocolError::ProtocolInit);
        }
        if !transports.supports(TransportKind::Tcp) {
            return Err(ProtocolError::Unsupported);
        }
        // 45.4 点表驱动路由：消费注入的点表（插件不自造地址）。服务层按
        // 协议过滤后注入；此处统计本插件可寻址（Modbus 地址）点数——
        // 非 Modbus 地址属于其他协议插件，跳过（不阻断装配）。
        {
            let mut ids: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
            points.collect_ids(&mut ids);
            self.routed_count = ids
                .iter()
                .filter_map(|id| points.resolve(*id))
                .filter(|m| {
                    matches!(
                        m.protocol_addr,
                        eneros_protocol_abstract::ProtocolAddress::Modbus { .. }
                    )
                })
                .count();
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

    /// 45.4 点表驱动单点真读（modbus-tcp 打样）：按点映射地址读保持/
    /// 输入寄存器 1 字（func 3/4）；线圈/多字 → 46 卷深度项（Unsupported）。
    fn read_point(
        &mut self,
        _now_ms: u64,
        mapping: &eneros_protocol_abstract::ProtocolPointMapping,
    ) -> Result<PointRead, ProtocolError> {
        let Some(adapter) = self.adapter.as_mut() else {
            return Err(ProtocolError::ProtocolNotStarted);
        };
        let (reg_addr, func_code) = match mapping.protocol_addr {
            eneros_protocol_abstract::ProtocolAddress::Modbus {
                slave_addr: _slave,
                reg_addr,
                func_code,
            } => (reg_addr, func_code),
            _ => return Err(ProtocolError::AddrTypeMismatch),
        };
        match func_code {
            3 | 4 => {}
            _ => return Err(ProtocolError::Unsupported),
        }
        let regs = adapter.read_raw(reg_addr, func_code, 1)?;
        let raw = regs.first().copied().ok_or(ProtocolError::ReadFailed)? as i64;
        self.point_seq = self.point_seq.saturating_add(1);
        Ok(PointRead {
            raw,
            ok: true,
            seq: self.point_seq,
        })
    }

    /// 47（WP-47.1 协议出站写）：点表驱动单点写入（FC06 单寄存器；读
    /// 功能码 3/4 映射同寄存器写）。工程值经映射逆变换（scale/offset；
    /// 越界/非数值 → 明确错误，不截断静默）。**只执行**——授权由
    /// ControlDispatch 九步链完成。
    fn write_point(
        &mut self,
        _now_ms: u64,
        mapping: &eneros_protocol_abstract::ProtocolPointMapping,
        value: eneros_upa_model::PointValue,
    ) -> Result<(), ProtocolError> {
        let Some(adapter) = self.adapter.as_mut() else {
            return Err(ProtocolError::ProtocolNotStarted);
        };
        let (reg_addr, func_code) = match mapping.protocol_addr {
            eneros_protocol_abstract::ProtocolAddress::Modbus {
                slave_addr: _slave,
                reg_addr,
                func_code,
            } => (reg_addr, func_code),
            _ => return Err(ProtocolError::AddrTypeMismatch),
        };
        if matches!(func_code, 1 | 2 | 4) {
            // 输入寄存器/线圈映射：不可写（诚实；本地写路径仅保持寄存器）。
            return Err(ProtocolError::Unsupported);
        }
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
        let raw: u16 = u16::try_from(raw_i).map_err(|_| ProtocolError::WriteFailed)?;
        adapter.write_raw(reg_addr, func_code, raw)
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

    // ===== 45.4 打样：点表驱动 start 消费 + read_point 真读（正负例）=====

    use eneros_protocol_abstract::ProtocolAddress;
    use eneros_protocol_abstract::ProtocolPointMapping;
    use eneros_protocol_abstract::VecPointRegistry;

    fn modbus_map(point_id: u32, reg_addr: u16, func_code: u8) -> ProtocolPointMapping {
        ProtocolPointMapping {
            point_id,
            device_id: 2,
            protocol_addr: ProtocolAddress::Modbus {
                slave_addr: 2,
                reg_addr,
                func_code,
            },
            data_type: eneros_upa_model::PointType::Analog,
            scale: 0.1,
            offset: 0.0,
            identity: eneros_protocol_abstract::PointIdentityBinding::UNKNOWN,
        }
    }

    /// 脚本化传输：应答固定 Modbus TCP 读保持寄存器帧（txn=0, value=0x1234）。
    struct ScriptedHandle {
        last_sent: core::cell::RefCell<alloc::vec::Vec<u8>>,
        connected: core::cell::Cell<bool>,
    }
    impl ScriptedHandle {
        fn new() -> Self {
            Self {
                last_sent: core::cell::RefCell::new(alloc::vec::Vec::new()),
                connected: core::cell::Cell::new(true),
            }
        }
    }
    impl eneros_protocol_abstract::TransportHandle for ScriptedHandle {
        fn send(&mut self, data: &[u8]) -> Result<(), eneros_protocol_abstract::TransportError> {
            *self.last_sent.borrow_mut() = data.to_vec();
            Ok(())
        }
        fn recv(
            &mut self,
            buf: &mut [u8],
        ) -> Result<usize, eneros_protocol_abstract::TransportError> {
            // MBAP(txn0,proto0,len5) + unit1 + func3 + bytecount2 + value 0x1234
            let resp: [u8; 12] = [
                0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x01, 0x03, 0x02, 0x12, 0x34, 0x00,
            ];
            let n = resp.len().min(buf.len());
            buf[..n].copy_from_slice(&resp[..n]);
            // 真实读出为 11 字节（0x1234）；补 tail 无需——master 按 len 读取。
            Ok(11)
        }
        fn is_connected(&self) -> bool {
            self.connected.get()
        }
        fn close(&mut self) -> Result<(), eneros_protocol_abstract::TransportError> {
            self.connected.set(false);
            Ok(())
        }
        fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
            self
        }
    }

    #[test]
    fn start_consumes_point_registry_and_counts_routed() {
        let mut plugin = TcpPlugin::new();
        struct ScriptedFactory;
        impl TransportFactory for ScriptedFactory {
            fn open(
                &mut self,
                _spec: &TransportSpec,
            ) -> Result<Box<dyn TransportHandle>, TransportError> {
                Ok(Box::new(ScriptedHandle::new()))
            }
            fn supports(&self, kind: TransportKind) -> bool {
                kind == TransportKind::Tcp
            }
        }
        let mut cfg = ProtocolConfig::default();
        cfg.insert("peer", "10.0.0.1:502");
        // 注册表：2 个 Modbus 点 + 1 个 MQTT（非本协议，跳过）。
        let mut reg = VecPointRegistry::new(vec![
            modbus_map(1001, 4097, 3),
            modbus_map(1002, 4098, 3),
            ProtocolPointMapping {
                point_id: 7001,
                device_id: 7,
                protocol_addr: ProtocolAddress::Mqtt {
                    topic: alloc::string::String::from("plant/pv/power"),
                },
                data_type: eneros_upa_model::PointType::Analog,
                scale: 1.0,
                offset: 0.0,
                identity: eneros_protocol_abstract::PointIdentityBinding::UNKNOWN,
            },
        ]);
        plugin
            .start(&cfg, &mut ScriptedFactory, &mut reg)
            .expect("start");
        assert_eq!(plugin.routed_count(), 2, "Modbus 点被路由，MQTT 点跳过");
    }

    #[test]
    fn read_point_rejects_non_modbus_mapping_and_coils() {
        let mut plugin = TcpPlugin::new();
        struct ScriptedFactory;
        impl TransportFactory for ScriptedFactory {
            fn open(
                &mut self,
                _spec: &TransportSpec,
            ) -> Result<Box<dyn TransportHandle>, TransportError> {
                Ok(Box::new(ScriptedHandle::new()))
            }
            fn supports(&self, kind: TransportKind) -> bool {
                kind == TransportKind::Tcp
            }
        }
        let mut cfg = ProtocolConfig::default();
        cfg.insert("peer", "10.0.0.1:502");
        let mut empty_reg = EmptyPointRegistry;
        plugin
            .start(&cfg, &mut ScriptedFactory, &mut empty_reg)
            .expect("start");
        // 非 Modbus 地址 → AddrTypeMismatch。
        let mqtt_map = ProtocolPointMapping {
            point_id: 7001,
            device_id: 7,
            protocol_addr: ProtocolAddress::Mqtt {
                topic: alloc::string::String::from("t"),
            },
            data_type: eneros_upa_model::PointType::Analog,
            scale: 1.0,
            offset: 0.0,
            identity: eneros_protocol_abstract::PointIdentityBinding::UNKNOWN,
        };
        assert_eq!(
            plugin.read_point(0, &mqtt_map),
            Err(ProtocolError::AddrTypeMismatch)
        );
        // 线圈（func 1）→ Unsupported（46 深度项）。
        let coil = modbus_map(1004, 4100, 1);
        assert_eq!(plugin.read_point(0, &coil), Err(ProtocolError::Unsupported));
        // 未启动 → ProtocolNotStarted。
        let mut unstarted = TcpPlugin::new();
        assert_eq!(
            unstarted.read_point(0, &modbus_map(1001, 4097, 3)),
            Err(ProtocolError::ProtocolNotStarted)
        );
    }

    /// 打样正例：func 3 保持寄存器真读 → 应答 0x1234 → raw=0x1234。
    #[test]
    fn read_point_reads_holding_register_real() {
        let mut plugin = TcpPlugin::new();
        struct ScriptedFactory;
        impl TransportFactory for ScriptedFactory {
            fn open(
                &mut self,
                _spec: &TransportSpec,
            ) -> Result<Box<dyn TransportHandle>, TransportError> {
                Ok(Box::new(ScriptedHandle::new()))
            }
            fn supports(&self, kind: TransportKind) -> bool {
                kind == TransportKind::Tcp
            }
        }
        let mut cfg = ProtocolConfig::default();
        cfg.insert("peer", "10.0.0.1:502");
        let mut reg = VecPointRegistry::new(vec![modbus_map(1001, 4097, 3)]);
        plugin
            .start(&cfg, &mut ScriptedFactory, &mut reg)
            .expect("start");
        let mapping = reg.resolve(1001).expect("point routed").clone();
        let read = plugin.read_point(0, &mapping).expect("read");
        assert!(read.ok);
        assert_eq!(read.raw, 0x1234); // 0x1234 = 4660
        assert_eq!(read.seq, 1);
        // 工程量变换复用 mapping 原语（45 红线：禁重造）。
        let eng = mapping.to_engineering(read.raw);
        assert!((eng - 466.0).abs() < 1e-9);
    }

    /// 蓝图 64 WP-64.1.3 双面引用锁（插件侧）：KNOWN_KEYS = 键表单源
    /// `eneros-protocol-keys::MODBUS_TCP`（回退本地字面量必红；对侧锁在
    /// config-bundle protocol.rs r64_ref_lock_modbus_tcp）。
    #[test]
    fn r64_ref_lock_modbus_tcp() {
        assert_eq!(KNOWN_KEYS, eneros_protocol_keys::MODBUS_TCP);
    }
}

#[cfg(test)]
mod required_keys_face_lock {
    /// 蓝图 66 WP-66.4.2（C9 插件运行面第三面锁）：插件运行面实际必填
    /// 检查键集（测试内**字面镜像**，变异敏感——改字面即红）== 叶
    /// `eneros_protocol_keys::REQUIRED_MODBUS_TCP`。三方锁链：叶内容锁（叶内）→ 本
    /// 字面镜像锁（运行面）→ config-bundle 行为表锁（编译面）。
    /// 维护纪律：start 的 peer 必填检查（`cfg.get("peer")…ok_or` + parse_ip_port）增删必填检查时必须同步本字面与叶表
    /// （protocol-keys/src/lib.rs），三面任一失配即红。
    #[test]
    fn start_required_keys_locked_to_leaf_table() {
        const START_REQUIRED: &[&str] = &["peer"];
        assert_eq!(
            START_REQUIRED,
            eneros_protocol_keys::REQUIRED_MODBUS_TCP,
            "插件运行面必填检查集漂移——须同步叶表与本字面（蓝图 66 C9）"
        );
    }
}
