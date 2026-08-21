//! IEC 60870-5-104 协议插件（蓝图 39 WP-39.11）。
//!
//! 与 RTU/TCP 插件同构：`Box<dyn TransportHandle>` 实现
//! [`MasterTransport`]，`Iec104Adapter` 直接消费；对端地址由配置
//! `peer`（ip:port）注入，工厂 open 时连接（生产路径禁止字面量 IP）。

use alloc::borrow::ToOwned;
use alloc::boxed::Box;
use alloc::vec::Vec;

use eneros_protocol_abstract::{
    parse_ip_port, AdapterState, InteropLevel, PointRegistry, Progress, ProtocolAdapter,
    ProtocolConfig, ProtocolDescriptor, ProtocolError, ProtocolHealth, ProtocolPlugin,
    ProtocolRole, ResourceBudget, SecurityProfile, TransportFactory, TransportHandle,
    TransportKind, TransportSpec, WorkBudget,
};

use crate::adapter::{Iec104Adapter, IecPointMapping};
use crate::config::MasterConfig;
use crate::error::MasterError;
use crate::transport::{ConnId, MasterTransport};

/// 插件 id。
pub const IEC104_PLUGIN_ID: &str = "iec104-master";

/// 生产 MasterTransport：把 `Box<dyn TransportHandle>`（TCP 字节流）包成
/// **粘包重组**传输——46.5（QEMU 实弹根因）：`Box<dyn TransportHandle>`
/// 直接实现 MasterTransport 时，`recv` 返回的是 TCP 流任意切片（STARTDT_CON
/// `68 04 0B 00 00 00` 被切成 5+1 字节），半帧交给 process_rx 解码
/// `InvalidFrame` → 连接停在 StartDtPending，t1 超时断链（对端 transcript
/// 只有 STARTDT_CON + 20s READ_TIMEOUT）。本 wrapper 累积到完整 APDU
/// （`68 len` + 4B 控制 + ASDU）才返回一帧，粘包余量保留下次。
struct StreamMasterTransport {
    inner: Box<dyn TransportHandle>,
    /// 粘包缓冲：read 一次消费多帧时暂存剩余字节（与 loopback 测试
    /// `StdMasterTransport` 同约定）。
    pending: Vec<u8>,
}

impl StreamMasterTransport {
    fn new(inner: Box<dyn TransportHandle>) -> Self {
        Self {
            inner,
            pending: Vec::new(),
        }
    }

    /// 从 `pending` 取出一个完整 APDU（0x68 打头 + 长度一致）→ 余量留下。
    fn take_frame(&mut self) -> Option<Vec<u8>> {
        if self.pending.len() >= 2 && self.pending[0] == 0x68 {
            let frame_len = 2 + self.pending[1] as usize;
            if self.pending.len() >= frame_len {
                let frame = self.pending[..frame_len].to_vec();
                self.pending.drain(..frame_len);
                return Some(frame);
            }
        }
        // 非 0x68 打头（残留垃圾/错位）→ 丢弃首字节继续扫描，防无限累积。
        if self.pending.len() > 2 && self.pending[0] != 0x68 {
            self.pending.drain(..1);
            return self.take_frame();
        }
        // 半帧：保留等待补齐。
        None
    }
}

impl MasterTransport for StreamMasterTransport {
    fn connect(&mut self, _ip: [u8; 4], _port: u16) -> Result<ConnId, MasterError> {
        if self.inner.is_connected() {
            Ok(1)
        } else {
            Err(MasterError::ConnectFailed)
        }
    }
    fn send(&mut self, _conn: ConnId, data: &[u8]) -> Result<(), MasterError> {
        TransportHandle::send(self.inner.as_mut(), data).map_err(|_| MasterError::SendFailed)
    }
    fn recv(&mut self, _conn: ConnId) -> Result<Option<Vec<u8>>, MasterError> {
        // 先消化粘包缓冲（上一轮读到的整帧）。
        if let Some(frame) = self.take_frame() {
            return Ok(Some(frame));
        }
        let mut buf = [0u8; 1024];
        match TransportHandle::recv(self.inner.as_mut(), &mut buf) {
            Ok(0) => Ok(None),
            Ok(n) => {
                self.pending.extend_from_slice(&buf[..n]);
                Ok(self.take_frame())
            }
            // 39 收尾复验（P1-4）：断连对状态机必须可见——旧实现把
            // Closed/Io 一并映射为 Ok(None)（断链只能靠 t3 间接兜底）。
            // 超时/无数据 = 无帧（poll 语义）；Closed/Io = 接收失败。
            Err(eneros_protocol_abstract::TransportError::Timeout) => {
                // 超时前已读到的完整帧仍返回（不丢数据）。
                Ok(self.take_frame())
            }
            Err(eneros_protocol_abstract::TransportError::Closed)
            | Err(eneros_protocol_abstract::TransportError::Io) => Err(MasterError::RecvFailed),
            Err(_) => Ok(self.take_frame()),
        }
    }
    fn close(&mut self, _conn: ConnId) -> Result<(), MasterError> {
        self.pending.clear();
        TransportHandle::close(self.inner.as_mut()).map_err(|_| MasterError::StateError)
    }
    fn now_ms(&self) -> u64 {
        eneros_time::get_monotonic_ns() / 1_000_000
    }
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self.inner.as_any_mut()
    }
}

/// 已知配置键。
const KNOWN_KEYS: &[&str] = &[
    "peer",
    "clock_sync_interval_ms",
    "t3_timeout_ms",
    "t1_timeout_ms",
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
    /// 45.4：注入点表路由点数（观测；服务层按协议过滤注入）。
    routed_count: usize,
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
            routed_count: 0,
        }
    }

    /// 45.4：注入点表路由点数（观测）。
    pub fn routed_count(&self) -> usize {
        self.routed_count
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
            // 46.5：QEMU/EL0 + 独立对端 L2 闭环（evidence/46/WP-46.5.md）。
            interop_level: InteropLevel::TargetPeerVerified,
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
        points: &mut dyn PointRegistry,
    ) -> Result<(), ProtocolError> {
        // 45.4 点表驱动路由：消费注入点表（插件不自造地址）。服务层按协议
        // 过滤注入；此处登记路由点数（非本协议地址点跳过，不阻断装配）。
        self.routed_count =
            eneros_protocol_abstract::plugin::count_matching_mappings(points, |m| {
                matches!(
                    m.protocol_addr,
                    eneros_protocol_abstract::ProtocolAddress::Iec104 { .. }
                )
            });
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
        // 46（WP-46.4）：t1 连接确认超时（STARTDT_CON 等待上限）。
        let t1_timeout_ms = eneros_protocol_abstract::plugin::config_u64(cfg, "t1_timeout_ms")?
            .unwrap_or(15_000)
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
            t1_timeout_ms,
            poll_interval_ms,
            default_port: port,
            peer_ip: ip,
            peer_port: port,
        };
        // 46.5：流式粘包重组传输（TCP 字节流切帧，见 `StreamMasterTransport`）。
        let transport: Box<dyn MasterTransport> = Box::new(StreamMasterTransport::new(handle));
        // 46.5（点表→104 映射接线）：从注入点表构建设备映射（45.4 接口；
        // 服务层经 svc-config 取点表后注入本插件）。映射 → adapter 设备表
        // → STARTDT/总召唤驱动（空映射 = 不连设备 = 无 104 握手；点表缺失
        // 如实降级不伪造）。
        let mut point_ids = alloc::vec::Vec::new();
        points.collect_ids(&mut point_ids);
        let mut mappings: alloc::vec::Vec<IecPointMapping> = alloc::vec::Vec::new();
        for id in &point_ids {
            if let Some(m) = points.resolve(*id) {
                if let eneros_protocol_abstract::ProtocolAddress::Iec104 {
                    common_addr, ioa, ..
                } = &m.protocol_addr
                {
                    mappings.push(IecPointMapping {
                        point_id: *id,
                        common_addr: *common_addr,
                        ioa: *ioa as u32,
                    });
                }
            }
        }
        let mut adapter = Iec104Adapter::new(master_config, transport).with_mappings(mappings);
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
        // 46.5（QEMU 实弹根因）：总召唤调度时钟域必须与 `master.connect` 的
        // 调度时钟一致。connect 用 `transport.now_ms()`（= eneros_time
        // monotonic，服务 time_init 后的小时钟），而旧实现把服务侧大时钟
        // `now_ms`（boot 相对）直接传给 `adapter.poll` → 两时钟域混用：
        // 调度在首轮（STARTDT_CON 尚未消费）触发一次 StateError 后
        // update_next 推到 大时钟+5s，永不再次到期 → 总召唤不发（对端
        // transcript 只有 STARTDT_CON，20s READ_TIMEOUT）。修正：adapter.poll
        // 一律用传输同源时钟 `eneros_time::get_monotonic_ns()/1e6`；
        // `last_poll_ms` 保留调用方 now_ms（健康观测，host 测试语义不变）。
        let t_ms = eneros_time::get_monotonic_ns() / 1_000_000;
        self.last_poll_ms = now_ms;
        match adapter.poll(t_ms) {
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

    fn debug_line(&self) -> Option<alloc::string::String> {
        self.adapter.as_ref().map(|a| a.debug_conn())
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

    /// 46.5 回归：流式粘包重组——TCP 把 `68 04 0B 00 00 00` 切成 5+1 字节，
    /// 旧实现（`Box<dyn TransportHandle>` 直接 recv）把 5B 半帧交给
    /// process_rx 解码失败 → 停在 StartDtPending。`StreamMasterTransport`
    /// 必须攒齐完整 APDU（2+len）才返回。
    #[test]
    fn stream_transport_assembles_partial_apdu() {
        struct Chunked {
            queue: core::cell::RefCell<alloc::vec::Vec<Vec<u8>>>,
        }
        impl TransportHandle for Chunked {
            fn send(&mut self, _data: &[u8]) -> Result<(), TransportError> {
                Ok(())
            }
            fn recv(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
                let mut q = self.queue.borrow_mut();
                match q.pop() {
                    Some(chunk) => {
                        buf[..chunk.len()].copy_from_slice(&chunk);
                        Ok(chunk.len())
                    }
                    None => Err(TransportError::Timeout),
                }
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
        // STARTDT_CON = 68 04 0B 00 00 00（6 字节），先喂 5 字节再喂 1 字节。
        let startdt_con = [0x68u8, 0x04, 0x0B, 0x00, 0x00, 0x00];
        let mut t = StreamMasterTransport::new(Box::new(Chunked {
            queue: core::cell::RefCell::new(alloc::vec![
                // Vec::pop 取末元素 → 先喂 5B 半帧，再喂 1B 补齐。
                startdt_con[5..].to_vec(),
                startdt_con[..5].to_vec(),
            ]),
        }));
        // 首段 5B：不足完整 APDU → Ok(None)（半帧保留，不交给解码器）。
        assert_eq!(t.recv(1).unwrap(), None, "5B 半帧不得作为完整帧返回");
        // 第二段 1B 补齐 → 返回完整 6B STARTDT_CON。
        let frame = t.recv(1).unwrap().expect("补齐后应返回完整 APDU");
        assert_eq!(&frame[..], &startdt_con, "粘包重组须逐字节还原");
    }
}
