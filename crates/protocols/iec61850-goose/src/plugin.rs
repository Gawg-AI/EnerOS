//! IEC 61850 GOOSE 协议插件（蓝图 39 WP-39.11）.
//!
//! 服务装配统一入口：经 [`TransportFactory`] 获取 raw Ethernet
//! `Box<dyn TransportHandle>`（本地 [`L2Transport`] trait 由外来
//! trait object 实现——插件与网卡解耦），组合现有
//! [`GoosePublisher`]（TX）与 [`GooseSubscriber`]（RX）codec，
//! 不重新实现任何编解码。
//!
//! # 蓝图 WP-39.11 覆盖实况（诚实声明，不虚标）
//!
//! | 蓝图要求 | 覆盖状态 |
//! |----------|----------|
//! | raw Ethernet | codec 已覆盖（EtherType 0x88B8 直帧收发） |
//! | VLAN（802.1Q） | **待后续**——codec 按固定偏移解析以太网头，不带 VLAN tag；传输层可自行剥 tag |
//! | appID | codec 已覆盖（TX 组帧 + RX 构造期过滤，配置值经 `app_id` 传入构造） |
//! | confRev | TX 编码已覆盖（恒 1）；**RX 解码跳过 0x88 字段，`conf_rev` 配置值仅留存为期望值（[`GoosePlugin::expected_conf_rev`]），逐帧校验待 codec 后续暴露** |
//! | replay（重放/重复） | codec 已覆盖（RX st_num/sq_num 分类 New/Duplicate/StJump） |
//! | timeAllowedToLive | TX 编码已覆盖（配置值进入控制块 max_time → 帧 TTL 字段 + 重传心跳周期）；RX 侧为**插件层 TTL 监视**（超时无新事件 → Degraded），codec 本身不解码该字段 |
//! | service | codec 已覆盖（发布/订阅双路径） |

use alloc::boxed::Box;
use alloc::string::String;

use eneros_protocol_abstract::{
    AdapterState, InteropLevel, PointRegistry, Progress, ProtocolConfig, ProtocolDescriptor,
    ProtocolError, ProtocolHealth, ProtocolPlugin, ProtocolRole, ResourceBudget, SecurityProfile,
    TransportFactory, TransportHandle, TransportKind, TransportSpec, WorkBudget,
};

use crate::goose_rx::{GooseSubscriber, RxStatus};
use crate::goose_tx::{GooseControlBlock, GoosePublisher};
use crate::{GooseError, L2Transport};

/// 插件 id。
pub const GOOSE_PLUGIN_ID: &str = "iec61850-goose";

/// IEC 61850-8-1 GOOSE 组播基址（`dst_mac` 未配置时的标准默认，非猜测兜底）。
pub const DEFAULT_GOOSE_DST_MAC: [u8; 6] = [0x01, 0x0C, 0xCD, 0x01, 0x00, 0x00];

/// `min_time_ms` 未配置时的默认快重传间隔（毫秒）。
const DEFAULT_MIN_TIME_MS: u64 = 10;

/// 以太网最大帧（含头）。
const MAX_FRAME: usize = 1518;

/// `Box<dyn TransportHandle>` 实现本地 [`L2Transport`]（外来类型实现
/// 本地 trait——合法；插件与网卡解耦）。
impl L2Transport for Box<dyn TransportHandle> {
    fn send(&mut self, frame: &[u8]) -> Result<(), GooseError> {
        TransportHandle::send(self.as_mut(), frame).map_err(|_| GooseError::TransportError)
    }
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, GooseError> {
        TransportHandle::recv(self.as_mut(), buf).map_err(|_| GooseError::TransportError)
    }
}

/// 已知配置键。
const KNOWN_KEYS: &[&str] = &[
    "app_id",
    "conf_rev",
    "time_allowed_to_live_ms",
    "min_time_ms",
    "go_cb_ref",
    "dataset_ref",
    "dst_mac",
];

/// GOOSE 插件（发布 + 订阅，PubSub）。
pub struct GoosePlugin {
    publisher: Option<GoosePublisher<Box<dyn TransportHandle>>>,
    subscriber: Option<GooseSubscriber<Box<dyn TransportHandle>>>,
    /// 期望 confRev（RX codec 暂不解码 confRev 字段，留存待后续接线——模块文档）。
    expected_conf_rev: Option<u32>,
    /// timeAllowedToLive（毫秒；TX 帧 TTL + RX 插件层监视阈值）。
    ttl_ms: u64,
    /// 最近一次收到新事件的 poll 时刻（TTL 监视基线；None = 尚未收到）。
    last_rx_ms: Option<u64>,
    /// 累计收到的新事件数（New/StJump；Duplicate 不计）。
    events: u64,
    last_poll_ms: u64,
    error_count: u64,
    last_error: Option<ProtocolError>,
    last_progress: Progress,
    /// 45.4：注入点表路由点数（观测；服务层按协议过滤注入）。
    routed_count: usize,
}

impl GoosePlugin {
    pub fn new() -> Self {
        Self {
            publisher: None,
            subscriber: None,
            expected_conf_rev: None,
            ttl_ms: 0,
            last_rx_ms: None,
            events: 0,
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

    /// 期望 confRev（配置值留存；RX codec 暂不逐帧校验——模块文档缺口声明）。
    pub fn expected_conf_rev(&self) -> Option<u32> {
        self.expected_conf_rev
    }

    /// 配置的 timeAllowedToLive（毫秒）。
    pub fn time_allowed_to_live_ms(&self) -> u64 {
        self.ttl_ms
    }

    /// 累计收到的新事件数。
    pub fn received_events(&self) -> u64 {
        self.events
    }

    /// 最近一次 RX 的 st_num（未启动/未收到 → 0）。
    pub fn last_st_num(&self) -> u32 {
        self.subscriber.as_ref().map_or(0, |s| s.last_st_num())
    }
}

impl Default for GoosePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolPlugin for GoosePlugin {
    fn descriptor(&self) -> &ProtocolDescriptor {
        static D: ProtocolDescriptor = ProtocolDescriptor {
            id: GOOSE_PLUGIN_ID,
            version: "1.0",
            role: ProtocolRole::PubSub,
            transport_kinds: &[TransportKind::RawEthernet],
            security_profile: SecurityProfile::None,
            // GOOSE 为事件驱动（数据集值经 PDU 到达），无静态点表。
            point_types: &[],
            resource_budget: ResourceBudget {
                // 发布 + 订阅各一条传输句柄。
                max_connections: 2,
                max_poll_ms: 20,
                max_frame: MAX_FRAME,
            },
            interop_level: InteropLevel::CodecReady,
        };
        &D
    }

    fn validate_config(&self, cfg: &ProtocolConfig) -> Result<(), ProtocolError> {
        eneros_protocol_abstract::plugin::reject_unknown_keys(cfg, KNOWN_KEYS)?;
        // app_id 必填且 1..=0xFFFF（0 为保留非法，codec 构造期亦拒绝）。
        let app_id = eneros_protocol_abstract::plugin::config_u64(cfg, "app_id")?
            .ok_or(ProtocolError::InvalidConfig)?;
        if app_id == 0 || app_id > u64::from(u16::MAX) {
            return Err(ProtocolError::InvalidConfig);
        }
        // conf_rev 可选数字（≤ u32::MAX）。
        if let Some(v) = eneros_protocol_abstract::plugin::config_u64(cfg, "conf_rev")? {
            if v > u64::from(u32::MAX) {
                return Err(ProtocolError::InvalidConfig);
            }
        }
        // time_allowed_to_live_ms 必填（GOOSE 时序安全相关，不猜），u16 域。
        let ttl = eneros_protocol_abstract::plugin::config_u64(cfg, "time_allowed_to_live_ms")?
            .ok_or(ProtocolError::InvalidConfig)?;
        if ttl == 0 || ttl > u64::from(u16::MAX) {
            return Err(ProtocolError::InvalidConfig);
        }
        if let Some(v) = eneros_protocol_abstract::plugin::config_u64(cfg, "min_time_ms")? {
            if v == 0 || v > u64::from(u16::MAX) {
                return Err(ProtocolError::InvalidConfig);
            }
        }
        // 控制块身份字段必填非空（不伪造 IED 身份）。
        match cfg.get("go_cb_ref") {
            Some(s) if !s.is_empty() => {}
            _ => return Err(ProtocolError::InvalidConfig),
        }
        match cfg.get("dataset_ref") {
            Some(s) if !s.is_empty() => {}
            _ => return Err(ProtocolError::InvalidConfig),
        }
        if let Some(mac) = cfg.get("dst_mac") {
            if parse_mac(mac).is_none() {
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
            eneros_protocol_abstract::plugin::count_matching_mappings(points, |_| true);
        if self.subscriber.is_some() {
            return Err(ProtocolError::ProtocolInit);
        }
        if !transports.supports(TransportKind::RawEthernet) {
            return Err(ProtocolError::Unsupported);
        }
        // 39 收尾复验纪律：全部运行参数来自校验过的配置本体，禁止 default 顶替。
        let app_id = eneros_protocol_abstract::plugin::config_u64(cfg, "app_id")?
            .ok_or(ProtocolError::InvalidConfig)?;
        if app_id == 0 || app_id > u64::from(u16::MAX) {
            return Err(ProtocolError::InvalidConfig);
        }
        let app_id = app_id as u16;
        let conf_rev = match eneros_protocol_abstract::plugin::config_u64(cfg, "conf_rev")? {
            Some(v) if v <= u64::from(u32::MAX) => Some(v as u32),
            Some(_) => return Err(ProtocolError::InvalidConfig),
            None => None,
        };
        let ttl = eneros_protocol_abstract::plugin::config_u64(cfg, "time_allowed_to_live_ms")?
            .ok_or(ProtocolError::InvalidConfig)?;
        if ttl == 0 || ttl > u64::from(u16::MAX) {
            return Err(ProtocolError::InvalidConfig);
        }
        let min_time = eneros_protocol_abstract::plugin::config_u64(cfg, "min_time_ms")?
            .unwrap_or(DEFAULT_MIN_TIME_MS);
        if min_time == 0 || min_time > u64::from(u16::MAX) {
            return Err(ProtocolError::InvalidConfig);
        }
        let go_cb_ref = String::from(cfg.get("go_cb_ref").ok_or(ProtocolError::InvalidConfig)?);
        if go_cb_ref.is_empty() {
            return Err(ProtocolError::InvalidConfig);
        }
        let dataset_ref = String::from(cfg.get("dataset_ref").ok_or(ProtocolError::InvalidConfig)?);
        if dataset_ref.is_empty() {
            return Err(ProtocolError::InvalidConfig);
        }
        let dst_mac = match cfg.get("dst_mac") {
            Some(s) => parse_mac(s).ok_or(ProtocolError::InvalidConfig)?,
            None => DEFAULT_GOOSE_DST_MAC,
        };
        // 配置真消费：app_id 进入 RX 过滤器构造 + TX 帧 appID；
        // dst_mac 进入 RX MAC 过滤 + TX 目的地址 + 传输 spec.peer。
        let peer = mac_hex_string(dst_mac);
        let handle_tx = open_raw(transports, &peer)?;
        let handle_rx = open_raw(transports, &peer)?;
        let cb = GooseControlBlock {
            go_cb_ref,
            app_id,
            dst_addr: dst_mac,
            min_time: min_time as u16,
            // max_time 同时作为帧 timeAllowedToLive 字段（goose_tx 语义）。
            max_time: ttl as u16,
            st_num: 0,
            sq_num: 0,
            dataset_ref,
            needs_retransmit: false,
        };
        let publisher =
            GoosePublisher::new(cb, handle_tx).map_err(|_| ProtocolError::ProtocolInit)?;
        let subscriber = GooseSubscriber::new(app_id, dst_mac, handle_rx)
            .map_err(|_| ProtocolError::ProtocolInit)?;
        self.publisher = Some(publisher);
        self.subscriber = Some(subscriber);
        self.expected_conf_rev = conf_rev;
        self.ttl_ms = ttl;
        self.last_rx_ms = None;
        self.events = 0;
        Ok(())
    }

    fn poll(&mut self, now_ms: u64, budget: WorkBudget) -> Result<Progress, ProtocolError> {
        let subscriber = self
            .subscriber
            .as_mut()
            .ok_or(ProtocolError::ProtocolNotStarted)?;
        self.last_poll_ms = now_ms;
        let mut degraded = false;
        let mut got_any = false;
        let max_frames = budget.max_polls.max(1);
        for _ in 0..max_frames {
            match subscriber.poll() {
                Ok(Some((pdu, status))) => {
                    got_any = true;
                    // replay/重传语义接线：Duplicate 不计新事件；
                    // StJump（st_num 跳变 = 事件丢失）→ 错误计数 + 降级。
                    if status != RxStatus::Duplicate {
                        self.events += 1;
                        self.last_rx_ms = Some(now_ms);
                    }
                    if status == RxStatus::StJump {
                        self.error_count += 1;
                        degraded = true;
                    }
                    // 39 §6.3 负例（乱序/重放）：st_num 回跳可检测。
                    // codec 对旧帧乱序归 Duplicate 且不更新状态（静默）；
                    // 插件层比较当前基线（Duplicate 不推进 last_st_num，
                    // 故完全重复 st == 基线不告警，回跳 st < 基线告警）→
                    // 错误计数 + 降级（fail-closed 防御，不改 codec 判定）。
                    if status == RxStatus::Duplicate && pdu.st_num < subscriber.last_st_num() {
                        self.error_count += 1;
                        degraded = true;
                    }
                }
                Ok(None) => break,
                Err(_) => {
                    self.error_count += 1;
                    self.last_error = Some(ProtocolError::ReadFailed);
                    self.last_progress = Progress::Degraded;
                    return Err(ProtocolError::ReadFailed);
                }
            }
        }
        // TX 重传/心跳（goose_tx 现有状态机：前 3 次 min_time，其后 max_time 周期）。
        if let Some(publisher) = self.publisher.as_mut() {
            let _ = publisher.retransmit_if_needed(now_ms);
        }
        // timeAllowedToLive 插件层监视（RX codec 不解码 TTL 字段——模块文档）：
        // 已建立接收基线后，超时无新事件 → 降级（不报错，订阅侧超时语义）。
        if let Some(last) = self.last_rx_ms {
            if now_ms.saturating_sub(last) > self.ttl_ms {
                degraded = true;
            }
        }
        let progress = if degraded {
            Progress::Degraded
        } else if got_any {
            Progress::Completed
        } else {
            Progress::Idle
        };
        self.last_progress = progress;
        Ok(progress)
    }

    fn health(&self) -> ProtocolHealth {
        let state = if self.subscriber.is_some() {
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

    fn stop(&mut self) -> Result<(), ProtocolError> {
        self.publisher = None;
        self.subscriber = None;
        self.last_progress = Progress::Idle;
        Ok(())
    }
}

/// 打开 raw Ethernet 传输（spec.peer = 组播 MAC 十六进制描述）。
fn open_raw(
    transports: &mut dyn TransportFactory,
    peer: &str,
) -> Result<Box<dyn TransportHandle>, ProtocolError> {
    let mut spec = TransportSpec {
        kind: TransportKind::RawEthernet,
        peer: [0; 128],
        peer_len: 0,
        mtu: MAX_FRAME,
        connect_timeout_ms: 1000,
        recv_timeout_ms: 1000,
    };
    let bytes = peer.as_bytes();
    if bytes.len() > spec.peer.len() {
        return Err(ProtocolError::InvalidConfig);
    }
    spec.peer[..bytes.len()].copy_from_slice(bytes);
    spec.peer_len = bytes.len() as u8;
    transports
        .open(&spec)
        .map_err(|_| ProtocolError::ProtocolInit)
}

/// 解析组播 MAC：12 位十六进制（"010ccd010001"）或冒号分隔
/// （"01:0c:cd:01:00:01"）；其余格式 → None（不猜）。
fn parse_mac(text: &str) -> Option<[u8; 6]> {
    if text.len() == 12 && !text.contains(':') {
        let mut out = [0u8; 6];
        for i in 0..6 {
            out[i] = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
        }
        return Some(out);
    }
    if text.len() == 17 {
        let mut out = [0u8; 6];
        let mut idx = 0usize;
        for part in text.split(':') {
            if idx >= 6 || part.len() != 2 {
                return None;
            }
            out[idx] = u8::from_str_radix(part, 16).ok()?;
            idx += 1;
        }
        if idx == 6 {
            return Some(out);
        }
    }
    None
}

/// MAC → 小写十六进制字符串（spec.peer 描述用）。
fn mac_hex_string(mac: [u8; 6]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(12);
    for b in mac {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use alloc::rc::Rc;
    use alloc::vec::Vec;

    use eneros_iec61850_model::DaValue;
    use eneros_protocol_abstract::test_support::EchoTransport;
    use eneros_protocol_abstract::{EmptyPointRegistry, TransportError};

    use super::*;

    /// 支持 RawEthernet 的回环工厂（生命周期测试用）。
    struct RawEchoFactory;

    impl TransportFactory for RawEchoFactory {
        fn open(
            &mut self,
            spec: &TransportSpec,
        ) -> Result<Box<dyn TransportHandle>, TransportError> {
            if spec.kind != TransportKind::RawEthernet {
                return Err(TransportError::Unsupported);
            }
            Ok(Box::new(EchoTransport::new()))
        }
        fn supports(&self, kind: TransportKind) -> bool {
            kind == TransportKind::RawEthernet
        }
    }

    /// 脚本化 L2 句柄（共享 rx 队列——测试可在 start 后推帧）。
    struct ScriptedL2 {
        rx: Rc<core::cell::RefCell<alloc::collections::VecDeque<Vec<u8>>>>,
        connected: core::cell::Cell<bool>,
    }

    impl TransportHandle for ScriptedL2 {
        fn send(&mut self, _data: &[u8]) -> Result<(), TransportError> {
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

    /// 捕获 spec + 共享 rx 队列的工厂（配置真消费断言用）。
    struct ScriptedFactory {
        shared: Rc<core::cell::RefCell<alloc::collections::VecDeque<Vec<u8>>>>,
        specs: core::cell::RefCell<Vec<(TransportKind, Vec<u8>)>>,
    }

    impl ScriptedFactory {
        fn new() -> Self {
            Self {
                shared: Rc::new(core::cell::RefCell::new(alloc::collections::VecDeque::new())),
                specs: core::cell::RefCell::new(Vec::new()),
            }
        }
    }

    impl TransportFactory for ScriptedFactory {
        fn open(
            &mut self,
            spec: &TransportSpec,
        ) -> Result<Box<dyn TransportHandle>, TransportError> {
            if spec.kind != TransportKind::RawEthernet {
                return Err(TransportError::Unsupported);
            }
            self.specs
                .borrow_mut()
                .push((spec.kind, spec.peer[..spec.peer_len as usize].to_vec()));
            Ok(Box::new(ScriptedL2 {
                rx: Rc::clone(&self.shared),
                connected: core::cell::Cell::new(true),
            }))
        }
        fn supports(&self, kind: TransportKind) -> bool {
            kind == TransportKind::RawEthernet
        }
    }

    /// 基础合法配置。
    fn base_cfg() -> ProtocolConfig {
        let mut cfg = ProtocolConfig::default();
        cfg.insert("app_id", "4097");
        cfg.insert("time_allowed_to_live_ms", "5000");
        cfg.insert("go_cb_ref", "IED1LD/LLN0$GO$gocb1");
        cfg.insert("dataset_ref", "IED1LD/LLN0$ds1");
        cfg
    }

    /// 用 crate 现有 TX codec 生成一帧（init_st 为发布前 st_num）。
    fn goose_frame(app_id: u16, dst: [u8; 6], init_st: u32) -> Vec<u8> {
        let cb = GooseControlBlock {
            go_cb_ref: String::from("IED1LD/LLN0$GO$gocb1"),
            app_id,
            dst_addr: dst,
            min_time: 10,
            max_time: 5000,
            st_num: init_st,
            sq_num: 0,
            dataset_ref: String::from("IED1LD/LLN0$ds1"),
            needs_retransmit: false,
        };
        let mut p = GoosePublisher::new(cb, crate::MockL2::new()).unwrap();
        p.update_value("A", DaValue::Bool(true));
        p.publish(0).unwrap();
        p.transport().tx_frames()[0].clone()
    }

    #[test]
    fn validate_config_positive_and_unknown_key_rejected() {
        let plugin = GoosePlugin::new();
        let mut cfg = base_cfg();
        assert!(plugin.validate_config(&cfg).is_ok());
        cfg.insert("conf_rev", "1");
        cfg.insert("dst_mac", "010ccd010001");
        assert!(plugin.validate_config(&cfg).is_ok());
        cfg.insert("unknown", "x");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
    }

    #[test]
    fn validate_config_ranges_and_required_keys() {
        let plugin = GoosePlugin::new();
        // 缺 app_id → InvalidConfig。
        let mut cfg = base_cfg();
        cfg.entries.retain(|(k, _)| k != "app_id");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
        // app_id = 0（保留非法）与超 u16 范围 → InvalidConfig。
        let mut cfg = base_cfg();
        cfg.insert("app_id", "0");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
        cfg.insert("app_id", "70000");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
        // conf_rev 非数字 → InvalidConfig。
        let mut cfg = base_cfg();
        cfg.insert("conf_rev", "abc");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
        // 缺 time_allowed_to_live_ms → InvalidConfig（必填不猜）。
        let mut cfg = base_cfg();
        cfg.entries.retain(|(k, _)| k != "time_allowed_to_live_ms");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
        // dst_mac 非法格式 → InvalidConfig。
        let mut cfg = base_cfg();
        cfg.insert("dst_mac", "zz");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
        // 缺 go_cb_ref → InvalidConfig。
        let mut cfg = base_cfg();
        cfg.entries.retain(|(k, _)| k != "go_cb_ref");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
    }

    #[test]
    fn start_poll_stop_lifecycle() {
        let mut plugin = GoosePlugin::new();
        let mut factory = RawEchoFactory;
        let cfg = base_cfg();
        plugin
            .start(&cfg, &mut factory, &mut EmptyPointRegistry)
            .expect("start");
        assert_eq!(plugin.health().state, AdapterState::Running);
        plugin
            .poll(
                500,
                WorkBudget {
                    max_work_ms: 20,
                    max_polls: 8,
                },
            )
            .unwrap();
        assert!(plugin.health().last_poll_ms >= 500);
        plugin.stop().unwrap();
        assert_eq!(
            plugin.poll(
                600,
                WorkBudget {
                    max_work_ms: 20,
                    max_polls: 8
                }
            ),
            Err(ProtocolError::ProtocolNotStarted)
        );
        assert_eq!(plugin.health().state, AdapterState::Uninitialized);
    }

    /// 39 收尾复验（P0）：配置真消费——app_id/dst_mac/TTL 进入传输 spec
    /// 与 codec 构造；缺必填键 → InvalidConfig（不默认值兜底）。
    #[test]
    fn start_consumes_configured_values() {
        let mut plugin = GoosePlugin::new();
        let mut factory = ScriptedFactory::new();
        let mut cfg = base_cfg();
        cfg.insert("conf_rev", "7");
        cfg.insert("dst_mac", "01:0c:cd:01:00:2a");
        plugin
            .start(&cfg, &mut factory, &mut EmptyPointRegistry)
            .expect("start");
        // 发布 + 订阅各开一条 raw Ethernet 传输，peer 均为配置的组播 MAC。
        let specs = factory.specs.borrow();
        assert_eq!(specs.len(), 2);
        for (kind, peer) in specs.iter() {
            assert_eq!(*kind, TransportKind::RawEthernet);
            assert_eq!(peer, b"010ccd01002a");
        }
        drop(specs);
        // TTL/conf_rev 配置值被插件持有（TTL 进入 TX 控制块 max_time）。
        assert_eq!(plugin.time_allowed_to_live_ms(), 5000);
        assert_eq!(plugin.expected_conf_rev(), Some(7));

        // 缺 app_id → InvalidConfig（默认值不伪健康）。
        let mut plugin2 = GoosePlugin::new();
        let mut cfg2 = base_cfg();
        cfg2.entries.retain(|(k, _)| k != "app_id");
        assert_eq!(
            plugin2.start(&cfg2, &mut factory, &mut EmptyPointRegistry),
            Err(ProtocolError::InvalidConfig)
        );
        // 缺 time_allowed_to_live_ms → InvalidConfig。
        let mut plugin3 = GoosePlugin::new();
        let mut cfg3 = base_cfg();
        cfg3.entries.retain(|(k, _)| k != "time_allowed_to_live_ms");
        assert_eq!(
            plugin3.start(&cfg3, &mut factory, &mut EmptyPointRegistry),
            Err(ProtocolError::InvalidConfig)
        );
    }

    /// 配置的 app_id 真正进入 RX 过滤：匹配帧收下，失配帧丢弃，
    /// st_num 跳变 → Degraded（replay/丢失检测接线）。
    #[test]
    fn start_filters_by_configured_app_id() {
        let mut plugin = GoosePlugin::new();
        let mut factory = ScriptedFactory::new();
        let cfg = base_cfg(); // app_id = 4097
        plugin
            .start(&cfg, &mut factory, &mut EmptyPointRegistry)
            .expect("start");
        let dst = DEFAULT_GOOSE_DST_MAC;
        // 匹配 app_id 的两帧：st=1 与 st=5（中间丢失 → StJump）。
        factory
            .shared
            .borrow_mut()
            .push_back(goose_frame(4097, dst, 0));
        factory
            .shared
            .borrow_mut()
            .push_back(goose_frame(4097, dst, 4));
        let progress = plugin
            .poll(
                100,
                WorkBudget {
                    max_work_ms: 20,
                    max_polls: 8,
                },
            )
            .unwrap();
        assert_eq!(progress, Progress::Degraded); // StJump 降级
        assert_eq!(plugin.received_events(), 2);
        assert_eq!(plugin.last_st_num(), 5);
        // 失配 app_id 帧 → 静默丢弃（不增事件）。
        factory
            .shared
            .borrow_mut()
            .push_back(goose_frame(0x2222, dst, 9));
        let progress = plugin
            .poll(
                110,
                WorkBudget {
                    max_work_ms: 20,
                    max_polls: 8,
                },
            )
            .unwrap();
        assert_eq!(progress, Progress::Idle);
        assert_eq!(plugin.received_events(), 2);
    }

    #[test]
    fn start_rejects_unsupported_transport() {
        let mut plugin = GoosePlugin::new();
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

    #[test]
    fn parse_mac_formats() {
        assert_eq!(
            parse_mac("010ccd01002a"),
            Some([0x01, 0x0C, 0xCD, 0x01, 0x00, 0x2A])
        );
        assert_eq!(
            parse_mac("01:0C:CD:01:00:2A"),
            Some([0x01, 0x0C, 0xCD, 0x01, 0x00, 0x2A])
        );
        assert_eq!(parse_mac("010ccd01002"), None);
        assert_eq!(parse_mac("zz0ccd01002a"), None);
    }

    /// 39 收尾复验（§6.3 重复/重放）：同一 stNum/sqNum 的 GOOSE 帧重复
    /// 注入——第二次幂等丢弃：不重复上报（events 不增）、不告警
    ///（error_count 不变）、不 panic。
    #[test]
    fn test_39_replayed_frame_idempotent_no_double_report() {
        let mut plugin = GoosePlugin::new();
        let mut factory = ScriptedFactory::new();
        plugin
            .start(&base_cfg(), &mut factory, &mut EmptyPointRegistry)
            .expect("start");
        let dst = DEFAULT_GOOSE_DST_MAC;
        let frame = goose_frame(4097, dst, 0); // st=1, sq=0
        factory.shared.borrow_mut().push_back(frame.clone());
        factory.shared.borrow_mut().push_back(frame); // 重放
        let progress = plugin
            .poll(
                100,
                WorkBudget {
                    max_work_ms: 20,
                    max_polls: 8,
                },
            )
            .unwrap();
        // 首帧 New → 事件 1；重放帧 Duplicate → 幂等丢弃。
        assert_eq!(plugin.received_events(), 1, "重复帧不得重复上报");
        assert_eq!(plugin.health().error_count, 0, "完全重复不告警");
        assert_eq!(progress, Progress::Completed);
    }

    /// 39 收尾复验（§6.3 乱序）：stNum 回跳（旧帧乱序到达）——
    /// 旧行为是静默丢弃不可检测；已修成可检测（插件层基线比较，
    /// 不改 codec 判定）：回跳帧不上报且触发 Degraded + 错误计数。
    #[test]
    fn test_39_st_num_rewind_detected_and_degraded() {
        let mut plugin = GoosePlugin::new();
        let mut factory = ScriptedFactory::new();
        plugin
            .start(&base_cfg(), &mut factory, &mut EmptyPointRegistry)
            .expect("start");
        let dst = DEFAULT_GOOSE_DST_MAC;
        // 正常序列 st=1 → st=2，随后乱序旧帧 st=1 回跳。
        factory
            .shared
            .borrow_mut()
            .push_back(goose_frame(4097, dst, 0)); // st=1
        factory
            .shared
            .borrow_mut()
            .push_back(goose_frame(4097, dst, 1)); // st=2
        factory
            .shared
            .borrow_mut()
            .push_back(goose_frame(4097, dst, 0)); // st=1（回跳）
        let progress = plugin
            .poll(
                100,
                WorkBudget {
                    max_work_ms: 20,
                    max_polls: 8,
                },
            )
            .unwrap();
        assert_eq!(progress, Progress::Degraded, "stNum 回跳必须可检测");
        assert_eq!(plugin.received_events(), 2, "回跳帧不得上报");
        assert_eq!(plugin.health().error_count, 1);
        // 回跳帧不污染基线（codec Duplicate 不更新状态）。
        assert_eq!(plugin.last_st_num(), 2);
    }
}
