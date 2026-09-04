//! IEC 61850 MMS 协议插件（蓝图 39 WP-39.11）.
//!
//! 服务装配统一入口：经 [`TransportFactory`] 获取 TCP
//! `Box<dyn TransportHandle>`（本地 [`MmsTransport`] trait 由外来
//! trait object 实现——插件与网卡解耦），组合现有 [`MmsClient`]
//! codec（COTP CR/CC + ACSE AARQ/AARE 握手 + Read/Write 服务），
//! 不重新实现任何编解码。
//!
//! # 配置键
//!
//! - `peer` → 必填 `ip:port`（`parse_ip_port` 校验；进入传输 spec 与
//!   `MmsClient::connect` 关联时序）
//! - `timeout_ms` → 可选（默认 3000；进入传输超时与 `MmsClient` 超时预算）
//! - `ap_title` → 可选（默认 "1.1.1.999"，本 crate 测试惯例；进入 AARQ）
//!
//! # 关联/重连接线实况（诚实声明）
//!
//! | 蓝图要求 | 覆盖状态 |
//! |----------|----------|
//! | TCP 连接 | codec 经 `MmsTransport::connect` 接线；spec.peer 由工厂建连 |
//! | COTP/ACSE 关联 | codec 已覆盖（start 中 `MmsClient::connect`，重试 ≤3） |
//! | 断链后重关联 | poll 接线：`ConnState::Error/Idle` → 重发握手（codec 重试语义） |
//! | Read/Write 服务 | codec 已覆盖，经 [`MmsPlugin::client_mut`] 供集成层发起 |
//! | 点表映射 | 读写值入点表属集成层接缝，**待后续**（point_types 空切片） |
//! | IEC 62351 安全 | **待后续**（descriptor security_profile = None） |
//! | TCP 断链重建 | 插件层无工厂句柄，底层 socket 重开归集成层；codec 重连仅重发握手 |

use alloc::boxed::Box;
use alloc::string::String;

use eneros_protocol_abstract::{
    parse_ip_port, AdapterState, InteropLevel, PointRegistry, Progress, ProtocolConfig,
    ProtocolDescriptor, ProtocolError, ProtocolHealth, ProtocolPlugin, ProtocolRole,
    ResourceBudget, SecurityProfile, TransportError, TransportFactory, TransportHandle,
    TransportKind, TransportSpec, WorkBudget,
};

use crate::{ConnState, MmsClient, MmsError, MmsTransport};

/// 插件 id。
pub const MMS_PLUGIN_ID: &str = "iec61850-mms";

/// `timeout_ms` 未配置时的默认超时（毫秒）。
const DEFAULT_TIMEOUT_MS: u64 = 3000;

/// `ap_title` 未配置时的默认 AP-title（本 crate 测试惯例，非猜测兜底）。
const DEFAULT_AP_TITLE: &str = "1.1.1.999";

/// 接收缓冲区大小（与 mms_client 内 RECV_BUF_LEN 同口径）。
const MAX_FRAME: usize = 4096;

/// 传输错误映射（超时语义保留，其余归传输错误）。
fn map_transport_err(e: TransportError) -> MmsError {
    if e == TransportError::Timeout {
        MmsError::Timeout
    } else {
        MmsError::TransportError
    }
}

/// `Box<dyn TransportHandle>` 实现本地 [`MmsTransport`]（外来类型实现
/// 本地 trait——合法；插件与网卡解耦）。
impl MmsTransport for Box<dyn TransportHandle> {
    fn connect(&mut self, _addr: &str, _port: u16) -> Result<(), MmsError> {
        // 工厂已按 spec.peer（配置的 ip:port）建连；此处仅校验连接状态。
        // TCP 断链后的 socket 重开归集成层（模块文档缺口声明）。
        if self.is_connected() {
            Ok(())
        } else {
            Err(MmsError::NotConnected)
        }
    }
    fn send(&mut self, pdu: &[u8]) -> Result<(), MmsError> {
        TransportHandle::send(self.as_mut(), pdu).map_err(map_transport_err)
    }
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, MmsError> {
        match TransportHandle::recv(self.as_mut(), buf) {
            // 无数据到达 → 超时语义（codec 按 D11 重试）。
            Ok(0) => Err(MmsError::Timeout),
            Ok(n) => Ok(n),
            Err(e) => Err(map_transport_err(e)),
        }
    }
}

/// 已知配置键（蓝图 64 WP-64.1.2 起单源引用 `eneros-protocol-keys`）。
const KNOWN_KEYS: &[&str] = eneros_protocol_keys::MMS;

/// MMS 插件（客户端，Client）。
pub struct MmsPlugin {
    client: Option<MmsClient<Box<dyn TransportHandle>>>,
    /// 关联目标 IP（poll 重关联复用配置值，不重读配置）。
    peer_ip: [u8; 4],
    /// 关联目标端口。
    peer_port: u16,
    last_poll_ms: u64,
    error_count: u64,
    last_error: Option<ProtocolError>,
    last_progress: Progress,
    /// 45.4：注入点表路由点数（观测；服务层按协议过滤注入）。
    routed_count: usize,
}

impl MmsPlugin {
    pub fn new() -> Self {
        Self {
            client: None,
            peer_ip: [0; 4],
            peer_port: 0,
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

    /// 只读访问底层客户端（集成层状态观察用）。
    pub fn client(&self) -> Option<&MmsClient<Box<dyn TransportHandle>>> {
        self.client.as_ref()
    }

    /// 可变访问底层客户端（集成层发起 Read/Write 服务用）。
    pub fn client_mut(&mut self) -> Option<&mut MmsClient<Box<dyn TransportHandle>>> {
        self.client.as_mut()
    }

    /// 当前连接状态（未启动 → `Idle`）。
    pub fn conn_state(&self) -> ConnState {
        self.client
            .as_ref()
            .map_or(ConnState::Idle, |c| c.conn_state())
    }
}

impl Default for MmsPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolPlugin for MmsPlugin {
    fn descriptor(&self) -> &ProtocolDescriptor {
        static D: ProtocolDescriptor = ProtocolDescriptor {
            id: MMS_PLUGIN_ID,
            version: "1.0",
            role: ProtocolRole::Client,
            transport_kinds: &[TransportKind::Tcp],
            security_profile: SecurityProfile::None,
            // Read/Write 值入点表属集成层接缝（模块文档），无静态点表。
            point_types: &[],
            resource_budget: ResourceBudget {
                max_connections: 1,
                max_poll_ms: 50,
                max_frame: MAX_FRAME,
            },
            // 46.5：QEMU/EL0 + 独立对端 L2 闭环（evidence/46/WP-46.5.md）。
            interop_level: InteropLevel::TargetPeerVerified,
        };
        &D
    }

    fn validate_config(&self, cfg: &ProtocolConfig) -> Result<(), ProtocolError> {
        eneros_protocol_abstract::plugin::reject_unknown_keys(cfg, KNOWN_KEYS)?;
        // peer 必填且可解析为 ip:port（不猜）。
        match cfg.get("peer") {
            Some(peer) if parse_ip_port(peer).is_some() => {}
            _ => return Err(ProtocolError::InvalidConfig),
        }
        if let Some(v) = eneros_protocol_abstract::plugin::config_u64(cfg, "timeout_ms")? {
            if v == 0 || v > u64::from(u32::MAX) {
                return Err(ProtocolError::InvalidConfig);
            }
        }
        if let Some(s) = cfg.get("ap_title") {
            if s.is_empty() {
                return Err(ProtocolError::InvalidConfig);
            }
        }
        Ok(())
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
                    eneros_protocol_abstract::ProtocolAddress::Mms { .. }
                )
            });
        if self.client.is_some() {
            return Err(ProtocolError::ProtocolInit);
        }
        if !transports.supports(TransportKind::Tcp) {
            return Err(ProtocolError::Unsupported);
        }
        // 39 收尾复验纪律：全部运行参数来自校验过的配置本体，禁止 default 顶替。
        let peer = String::from(cfg.get("peer").ok_or(ProtocolError::InvalidConfig)?);
        let (ip, port) = parse_ip_port(&peer).ok_or(ProtocolError::InvalidConfig)?;
        let timeout_ms = eneros_protocol_abstract::plugin::config_u64(cfg, "timeout_ms")?
            .unwrap_or(DEFAULT_TIMEOUT_MS);
        if timeout_ms == 0 || timeout_ms > u64::from(u32::MAX) {
            return Err(ProtocolError::InvalidConfig);
        }
        let ap_title = match cfg.get("ap_title") {
            Some(s) if !s.is_empty() => String::from(s),
            Some(_) => return Err(ProtocolError::InvalidConfig),
            None => String::from(DEFAULT_AP_TITLE),
        };
        // 配置真消费：peer 进入传输 spec（工厂据此建连）；
        // timeout_ms 进入传输超时 + MmsClient 超时预算；ap_title 进入 AARQ。
        let mut spec = TransportSpec {
            kind: TransportKind::Tcp,
            peer: [0; 128],
            peer_len: 0,
            mtu: MAX_FRAME,
            connect_timeout_ms: timeout_ms,
            recv_timeout_ms: timeout_ms,
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
        // 关联时序接线：COTP CR/CC + ACSE AARQ/AARE（codec 内重试 ≤3，D11）。
        let mut client = MmsClient::new(handle, &ap_title, timeout_ms as u32);
        client
            .connect(&dotted_ip(ip), port)
            .map_err(|_| ProtocolError::ProtocolInit)?;
        self.peer_ip = ip;
        self.peer_port = port;
        self.client = Some(client);
        self.last_progress = Progress::Completed;
        Ok(())
    }

    fn poll(&mut self, now_ms: u64, _budget: WorkBudget) -> Result<Progress, ProtocolError> {
        let client = self
            .client
            .as_mut()
            .ok_or(ProtocolError::ProtocolNotStarted)?;
        self.last_poll_ms = now_ms;
        match client.conn_state() {
            // 请求驱动客户端：已连接时无自发工作（Read/Write 经 client_mut 发起）。
            ConnState::Connected => {
                self.last_progress = Progress::Idle;
                Ok(Progress::Idle)
            }
            // 断链/未关联 → 重关联尝试（接线 MmsClient::connect 重试语义）。
            ConnState::Error | ConnState::Idle | ConnState::Connecting => {
                match client.connect(&dotted_ip(self.peer_ip), self.peer_port) {
                    Ok(()) => {
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
        }
    }

    fn health(&self) -> ProtocolHealth {
        let state = match &self.client {
            Some(c) => match c.conn_state() {
                ConnState::Connected => AdapterState::Running,
                ConnState::Connecting => AdapterState::Initialized,
                ConnState::Idle => AdapterState::Uninitialized,
                ConnState::Error => AdapterState::Error,
            },
            None => AdapterState::Uninitialized,
        };
        ProtocolHealth {
            state,
            last_poll_ms: self.last_poll_ms,
            error_count: self.error_count,
            last_error: self.last_error.clone(),
            last_progress: self.last_progress,
        }
    }

    fn stop(&mut self) -> Result<(), ProtocolError> {
        if let Some(client) = self.client.as_mut() {
            client.disconnect();
        }
        self.client = None;
        self.last_progress = Progress::Idle;
        Ok(())
    }
}

/// IP → 点分十进制（`MmsClient::connect` 地址参数用）。
fn dotted_ip(ip: [u8; 4]) -> String {
    alloc::format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::collections::VecDeque;
    use alloc::rc::Rc;
    use alloc::vec::Vec;
    use core::cell::{Cell, RefCell};

    use eneros_protocol_abstract::EmptyPointRegistry;

    use super::*;

    /// COTP CC（acse::decode_cotp_cc 接受：第 2 字节 0xD0）。
    const CC: &[u8] = &[0x09, 0xD0, 0x00, 0x01, 0x00, 0x01, 0x00, 0xC0, 0x01, 0x0A];
    /// ACSE AARE 接受（decode_aare：tag 0x61 + result 0）。
    const AARE_OK: &[u8] = &[0x61, 0x03, 0x02, 0x01, 0x00];
    /// ACSE AARE 拒绝（result 非 0）。
    const AARE_REFUSE: &[u8] = &[0x61, 0x03, 0x02, 0x01, 0x01];

    /// 脚本化 TCP 句柄（共享 rx 队列 + sent 记录——握手响应可预载）。
    struct ScriptedTcp {
        rx: Rc<RefCell<VecDeque<Vec<u8>>>>,
        sent: Rc<RefCell<Vec<Vec<u8>>>>,
        connected: Cell<bool>,
    }

    impl TransportHandle for ScriptedTcp {
        fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
            self.sent.borrow_mut().push(data.to_vec());
            Ok(())
        }
        fn recv(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
            let Some(frame) = self.rx.borrow_mut().pop_front() else {
                return Ok(0);
            };
            let n = frame.len().min(buf.len());
            buf[..n].copy_from_slice(&frame[..n]);
            Ok(n)
        }
        fn close(&mut self) -> Result<(), TransportError> {
            self.connected.set(false);
            Ok(())
        }
        fn is_connected(&self) -> bool {
            self.connected.get()
        }
        fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
            self
        }
    }

    /// 捕获 spec + 共享 rx/sent 的工厂（配置真消费断言用）。
    struct ScriptedFactory {
        rx: Rc<RefCell<VecDeque<Vec<u8>>>>,
        sent: Rc<RefCell<Vec<Vec<u8>>>>,
        specs: RefCell<Vec<(TransportKind, Vec<u8>)>>,
    }

    impl ScriptedFactory {
        fn new() -> Self {
            Self {
                rx: Rc::new(RefCell::new(VecDeque::new())),
                sent: Rc::new(RefCell::new(Vec::new())),
                specs: RefCell::new(Vec::new()),
            }
        }
    }

    impl TransportFactory for ScriptedFactory {
        fn open(
            &mut self,
            spec: &TransportSpec,
        ) -> Result<Box<dyn TransportHandle>, TransportError> {
            if spec.kind != TransportKind::Tcp {
                return Err(TransportError::Unsupported);
            }
            self.specs
                .borrow_mut()
                .push((spec.kind, spec.peer[..spec.peer_len as usize].to_vec()));
            Ok(Box::new(ScriptedTcp {
                rx: Rc::clone(&self.rx),
                sent: Rc::clone(&self.sent),
                connected: Cell::new(true),
            }))
        }
        fn supports(&self, kind: TransportKind) -> bool {
            kind == TransportKind::Tcp
        }
    }

    /// 基础合法配置。
    fn base_cfg() -> ProtocolConfig {
        let mut cfg = ProtocolConfig::default();
        cfg.insert("peer", "192.168.1.10:102");
        cfg
    }

    #[test]
    fn validate_config_positive_and_unknown_key_rejected() {
        let plugin = MmsPlugin::new();
        let mut cfg = base_cfg();
        assert!(plugin.validate_config(&cfg).is_ok());
        cfg.insert("timeout_ms", "5000");
        cfg.insert("ap_title", "1.1.1.999");
        assert!(plugin.validate_config(&cfg).is_ok());
        cfg.insert("unknown", "x");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
    }

    #[test]
    fn validate_config_peer_and_ranges() {
        let plugin = MmsPlugin::new();
        // 缺 peer → InvalidConfig。
        assert_eq!(
            plugin.validate_config(&ProtocolConfig::default()),
            Err(ProtocolError::InvalidConfig)
        );
        // peer 格式非法 → InvalidConfig。
        let mut cfg = ProtocolConfig::default();
        cfg.insert("peer", "nothostport");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
        // timeout_ms = 0 / 非数字 → InvalidConfig。
        let mut cfg = base_cfg();
        cfg.insert("timeout_ms", "0");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
        cfg.insert("timeout_ms", "abc");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
        // ap_title 空串 → InvalidConfig。
        let mut cfg = base_cfg();
        cfg.insert("ap_title", "");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
    }

    #[test]
    fn start_poll_stop_lifecycle() {
        let mut plugin = MmsPlugin::new();
        let mut factory = ScriptedFactory::new();
        // 预载握手响应：CC + AARE 接受。
        factory.rx.borrow_mut().push_back(CC.to_vec());
        factory.rx.borrow_mut().push_back(AARE_OK.to_vec());
        plugin
            .start(&base_cfg(), &mut factory, &mut EmptyPointRegistry)
            .expect("start");
        assert_eq!(plugin.health().state, AdapterState::Running);
        assert_eq!(plugin.conn_state(), ConnState::Connected);
        // 已连接 → 请求驱动，poll 无自发工作。
        let progress = plugin
            .poll(
                500,
                WorkBudget {
                    max_work_ms: 20,
                    max_polls: 4,
                },
            )
            .unwrap();
        assert_eq!(progress, Progress::Idle);
        assert!(plugin.health().last_poll_ms >= 500);
        plugin.stop().unwrap();
        assert_eq!(
            plugin.poll(
                600,
                WorkBudget {
                    max_work_ms: 20,
                    max_polls: 4
                }
            ),
            Err(ProtocolError::ProtocolNotStarted)
        );
        assert_eq!(plugin.health().state, AdapterState::Uninitialized);
    }

    /// 39 收尾复验（P0）：配置真消费——peer 进入传输 spec；ap_title 进入
    /// AARQ；缺 peer → InvalidConfig（默认值不伪健康）。
    #[test]
    fn start_consumes_configured_peer_and_ap_title() {
        let mut plugin = MmsPlugin::new();
        let mut factory = ScriptedFactory::new();
        factory.rx.borrow_mut().push_back(CC.to_vec());
        factory.rx.borrow_mut().push_back(AARE_OK.to_vec());
        let mut cfg = ProtocolConfig::default();
        cfg.insert("peer", "10.7.0.9:102");
        cfg.insert("ap_title", "9.8.7.666");
        plugin
            .start(&cfg, &mut factory, &mut EmptyPointRegistry)
            .expect("start");
        // 工厂捕获 spec：kind Tcp + peer 即配置的 ip:port。
        let specs = factory.specs.borrow();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].0, TransportKind::Tcp);
        assert_eq!(specs[0].1, b"10.7.0.9:102");
        drop(specs);
        // 握手时序：先发 COTP CR，再发 AARQ（含配置的 ap_title）。
        let sent = factory.sent.borrow();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0][1], 0xE0); // COTP CR
        assert_eq!(&sent[1][..3], &[0x02, 0xF0, 0x80]); // COTP DT 头
        assert_eq!(sent[1][3], 0x60); // AARQ
        assert!(sent[1].windows(9).any(|w| w == b"9.8.7.666"));
        drop(sent);

        // 缺 peer → InvalidConfig。
        let mut plugin2 = MmsPlugin::new();
        assert_eq!(
            plugin2.start(
                &ProtocolConfig::default(),
                &mut factory,
                &mut EmptyPointRegistry
            ),
            Err(ProtocolError::InvalidConfig)
        );
    }

    /// 关联被拒（AARE result 非 0）→ start 报 ProtocolInit（不虚标连接成功）。
    #[test]
    fn start_association_refused_protocol_init() {
        let mut plugin = MmsPlugin::new();
        let mut factory = ScriptedFactory::new();
        factory.rx.borrow_mut().push_back(CC.to_vec());
        factory.rx.borrow_mut().push_back(AARE_REFUSE.to_vec());
        assert_eq!(
            plugin.start(&base_cfg(), &mut factory, &mut EmptyPointRegistry),
            Err(ProtocolError::ProtocolInit)
        );
        assert_eq!(plugin.health().state, AdapterState::Uninitialized);
    }

    #[test]
    fn start_rejects_unsupported_transport() {
        let mut plugin = MmsPlugin::new();
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
            plugin.start(&base_cfg(), &mut SerialOnly, &mut EmptyPointRegistry),
            Err(ProtocolError::Unsupported)
        );
    }

    /// 蓝图 64 WP-64.1.3 双面引用锁（插件侧）：KNOWN_KEYS = 键表单源
    /// `eneros-protocol-keys::MMS`；对侧锁在 config-bundle protocol.rs
    /// r64_ref_lock_mms。
    #[test]
    fn r64_ref_lock_mms() {
        assert_eq!(KNOWN_KEYS, eneros_protocol_keys::MMS);
    }
}

#[cfg(test)]
mod required_keys_face_lock {
    /// 蓝图 66 WP-66.4.2（C9 插件运行面第三面锁）：插件运行面实际必填
    /// 检查键集（测试内**字面镜像**，变异敏感——改字面即红）== 叶
    /// `eneros_protocol_keys::REQUIRED_MMS`。三方锁链：叶内容锁（叶内）→ 本
    /// 字面镜像锁（运行面）→ config-bundle 行为表锁（编译面）。
    /// 维护纪律：start 的 peer 必填检查（`cfg.get("peer")…ok_or` + parse_ip_port）增删必填检查时必须同步本字面与叶表
    /// （protocol-keys/src/lib.rs），三面任一失配即红。
    #[test]
    fn start_required_keys_locked_to_leaf_table() {
        const START_REQUIRED: &[&str] = &["peer"];
        assert_eq!(
            START_REQUIRED,
            eneros_protocol_keys::REQUIRED_MMS,
            "插件运行面必填检查集漂移——须同步叶表与本字面（蓝图 66 C9）"
        );
    }
}
