//! IEC 61850-9-2 SV 协议插件（蓝图 39 WP-39.11）.
//!
//! 服务装配统一入口：经 [`TransportFactory`] 获取 raw Ethernet
//! `Box<dyn TransportHandle>`（本地 [`L2Transport`] trait 由外来
//! trait object 实现——插件与网卡解耦），组合现有 [`SvSubscriber`]
//! codec（EtherType 0x88BA 过滤 + BER 解码 + smpCnt 连续性检测），
//! 不重新实现任何编解码。
//!
//! # 配置键与 sv_rx 实际参数对应（诚实声明）
//!
//! - `app_id` → [`SvSubscriber::new`] 的 APPID 过滤（必填，1..=0xFFFF）
//! - `dst_mac` → 目的组播 MAC 过滤（可选，默认 IEC 61850-9-2 SV 组播基址）
//! - `buffer_size` → 环形缓冲容量（可选，默认 64）
//!
//! 蓝图提及的 `sample_rate` **不是 sv_rx 的构造参数**：采样率语义由
//! smpCnt 连续性检测（`SampleStatus::New/Duplicate/SmpJump`）在 codec
//! 内部承担，本插件在 poll 中接线该状态（SmpJump → 错误计数 + 降级）。
//!
//! # 覆盖实况（不虚标）
//!
//! | 蓝图要求 | 覆盖状态 |
//! |----------|----------|
//! | raw Ethernet | codec 已覆盖（EtherType 0x88BA 直帧接收） |
//! | VLAN（802.1Q） | **待后续**——codec 按固定偏移解析以太网头，不带 VLAN tag |
//! | smpCnt 连续性 | codec 已覆盖，插件接线（poll 中消费 `SampleStatus`） |
//! | 采样数据上游 | 插件 poll 排空缓冲计数；采样值入点表属集成层接缝，**待后续** |

use alloc::boxed::Box;
use alloc::string::String;

use eneros_protocol_abstract::{
    AdapterState, InteropLevel, PointRegistry, Progress, ProtocolConfig, ProtocolDescriptor,
    ProtocolError, ProtocolHealth, ProtocolPlugin, ProtocolRole, ResourceBudget, SecurityProfile,
    TransportFactory, TransportHandle, TransportKind, TransportSpec, WorkBudget,
};

use crate::sv_rx::{SampleStatus, SvSubscriber};
use crate::{L2Transport, SvError};

/// 插件 id。
pub const SV_PLUGIN_ID: &str = "iec61850-sv";

/// IEC 61850-9-2 SV 组播基址（`dst_mac` 未配置时的标准默认，非猜测兜底）。
pub const DEFAULT_SV_DST_MAC: [u8; 6] = [0x01, 0x0C, 0xCD, 0x04, 0x00, 0x00];

/// `buffer_size` 未配置时的默认环形缓冲容量。
const DEFAULT_BUFFER_SIZE: u64 = 64;

/// 以太网最大帧（含头）。
const MAX_FRAME: usize = 1518;

/// `Box<dyn TransportHandle>` 实现本地 [`L2Transport`]（外来类型实现
/// 本地 trait——合法；插件与网卡解耦）。
impl L2Transport for Box<dyn TransportHandle> {
    fn send(&mut self, frame: &[u8]) -> Result<(), SvError> {
        TransportHandle::send(self.as_mut(), frame).map_err(|_| SvError::TransportError)
    }
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, SvError> {
        TransportHandle::recv(self.as_mut(), buf).map_err(|_| SvError::TransportError)
    }
}

/// 已知配置键。
const KNOWN_KEYS: &[&str] = &["app_id", "dst_mac", "buffer_size"];

/// SV 插件（订阅者，Sub）。
pub struct SvPlugin {
    subscriber: Option<SvSubscriber<Box<dyn TransportHandle>>>,
    /// 累计接收的有效采样数。
    samples_received: u64,
    /// 累计 smpCnt 跳变次数（采样丢失，codec 连续性检测结果）。
    sample_jumps: u64,
    last_poll_ms: u64,
    error_count: u64,
    last_error: Option<ProtocolError>,
    last_progress: Progress,
}

impl SvPlugin {
    pub fn new() -> Self {
        Self {
            subscriber: None,
            samples_received: 0,
            sample_jumps: 0,
            last_poll_ms: 0,
            error_count: 0,
            last_error: None,
            last_progress: Progress::Idle,
        }
    }

    /// 累计接收的有效采样数。
    pub fn samples_received(&self) -> u64 {
        self.samples_received
    }

    /// 累计 smpCnt 跳变次数（丢样检测接线证据）。
    pub fn sample_jumps(&self) -> u64 {
        self.sample_jumps
    }

    /// 最近一次接收的 smpCnt（未收到 → 0）。
    pub fn last_smp_cnt(&self) -> u16 {
        self.subscriber.as_ref().map_or(0, |s| s.last_smp_cnt())
    }
}

impl Default for SvPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolPlugin for SvPlugin {
    fn descriptor(&self) -> &ProtocolDescriptor {
        static D: ProtocolDescriptor = ProtocolDescriptor {
            id: SV_PLUGIN_ID,
            version: "1.0",
            role: ProtocolRole::Sub,
            transport_kinds: &[TransportKind::RawEthernet],
            security_profile: SecurityProfile::None,
            // SV 为高速采样流（f32 通道），当前无静态点表映射（见模块文档）。
            point_types: &[],
            resource_budget: ResourceBudget {
                max_connections: 1,
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
        if let Some(mac) = cfg.get("dst_mac") {
            if parse_mac(mac).is_none() {
                return Err(ProtocolError::InvalidConfig);
            }
        }
        if let Some(v) = eneros_protocol_abstract::plugin::config_u64(cfg, "buffer_size")? {
            if v == 0 || v > u64::from(u16::MAX) {
                return Err(ProtocolError::InvalidConfig);
            }
        }
        Ok(())
    }

    fn start(
        &mut self,
        cfg: &ProtocolConfig,
        transports: &mut dyn TransportFactory,
        _points: &mut dyn PointRegistry,
    ) -> Result<(), ProtocolError> {
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
        let dst_mac = match cfg.get("dst_mac") {
            Some(s) => parse_mac(s).ok_or(ProtocolError::InvalidConfig)?,
            None => DEFAULT_SV_DST_MAC,
        };
        let buffer_size = eneros_protocol_abstract::plugin::config_u64(cfg, "buffer_size")?
            .unwrap_or(DEFAULT_BUFFER_SIZE);
        if buffer_size == 0 || buffer_size > u64::from(u16::MAX) {
            return Err(ProtocolError::InvalidConfig);
        }
        // 配置真消费：app_id/dst_mac 进入 SvSubscriber 过滤器构造，
        // buffer_size 进入环形缓冲容量；dst_mac 同时进入传输 spec.peer。
        let handle = open_raw(transports, &mac_hex_string(dst_mac))?;
        let subscriber = SvSubscriber::new(app_id, dst_mac, buffer_size as usize, handle)
            .map_err(|_| ProtocolError::ProtocolInit)?;
        self.subscriber = Some(subscriber);
        self.samples_received = 0;
        self.sample_jumps = 0;
        Ok(())
    }

    fn poll(&mut self, now_ms: u64, budget: WorkBudget) -> Result<Progress, ProtocolError> {
        let subscriber = self
            .subscriber
            .as_mut()
            .ok_or(ProtocolError::ProtocolNotStarted)?;
        self.last_poll_ms = now_ms;
        let mut buf = [0u8; MAX_FRAME];
        let max_frames = budget.max_polls.max(1);
        for _ in 0..max_frames {
            let n = match subscriber.transport_mut().recv(&mut buf) {
                Ok(n) => n,
                Err(_) => {
                    self.error_count += 1;
                    self.last_error = Some(ProtocolError::ReadFailed);
                    self.last_progress = Progress::Degraded;
                    return Err(ProtocolError::ReadFailed);
                }
            };
            if n == 0 {
                break;
            }
            match subscriber.receive(&buf[..n]) {
                Ok(_) => {}
                Err(_) => {
                    // 通过 EtherType/MAC/APPID 过滤后的畸形 PDU → 解码错误。
                    self.error_count += 1;
                    self.last_error = Some(ProtocolError::ReadFailed);
                    self.last_progress = Progress::Degraded;
                    return Err(ProtocolError::ReadFailed);
                }
            }
        }
        // 排空缓冲并接线 smpCnt 连续性检测（codec 内 SampleStatus 判定）。
        let samples = subscriber.take_samples();
        let got_any = !samples.is_empty();
        let mut degraded = false;
        let mut fresh = 0u64;
        for sample in &samples {
            if sample.status == SampleStatus::SmpJump {
                self.sample_jumps += 1;
                self.error_count += 1;
                degraded = true;
            }
            // 39 §6.3 负例（重复）：Duplicate 样本幂等丢弃——不计入
            // samples_received（fail-closed 防御：重复帧不重复上报）。
            if sample.status != SampleStatus::Duplicate {
                fresh += 1;
            }
        }
        self.samples_received += fresh;
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

/// 解析组播 MAC：12 位十六进制（"010ccd040001"）或冒号分隔
/// （"01:0c:cd:04:00:01"）；其余格式 → None（不猜）。
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

    use eneros_protocol_abstract::test_support::EchoTransport;
    use eneros_protocol_abstract::{EmptyPointRegistry, TransportError};

    use super::*;

    /// SV EtherType（IEC 61850-9-2；sv_rx 内为私有常量，测试侧按标准值构帧）。
    const ETHERTYPE_SV: u16 = 0x88BA;

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
        cfg.insert("app_id", "16385");
        cfg
    }

    /// 构造标准 SV 帧（以太网头 + APPID + 四字节头 + smpCnt/timestamp/channels TLV）。
    fn sv_frame(dst: [u8; 6], app_id: u16, smp_cnt: u16, channels: &[f32]) -> Vec<u8> {
        let mut f = Vec::new();
        f.extend_from_slice(&dst);
        f.extend_from_slice(&[0x02, 0x00, 0x00, 0x00, 0x00, 0x02]);
        f.extend_from_slice(&ETHERTYPE_SV.to_be_bytes());
        f.extend_from_slice(&app_id.to_be_bytes());
        let payload_len = 4 + 10 + 2 + channels.len() * 4;
        f.extend_from_slice(&(6u16 + payload_len as u16).to_be_bytes());
        f.extend_from_slice(&[0, 0]);
        f.extend_from_slice(&[0, 0]);
        f.extend_from_slice(&[0x80, 0x02]);
        f.extend_from_slice(&smp_cnt.to_be_bytes());
        f.extend_from_slice(&[0x81, 0x08]);
        f.extend_from_slice(&0u64.to_be_bytes());
        f.push(0x82);
        f.push((channels.len() * 4) as u8);
        for c in channels {
            f.extend_from_slice(&c.to_be_bytes());
        }
        f
    }

    #[test]
    fn validate_config_positive_and_unknown_key_rejected() {
        let plugin = SvPlugin::new();
        let mut cfg = base_cfg();
        assert!(plugin.validate_config(&cfg).is_ok());
        cfg.insert("dst_mac", "010ccd040001");
        cfg.insert("buffer_size", "32");
        assert!(plugin.validate_config(&cfg).is_ok());
        cfg.insert("unknown", "x");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
    }

    #[test]
    fn validate_config_ranges_and_required_keys() {
        let plugin = SvPlugin::new();
        // 缺 app_id → InvalidConfig。
        let cfg = ProtocolConfig::default();
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
        // app_id = 0 与超 u16 范围 → InvalidConfig。
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
        // buffer_size = 0 / 非数字 → InvalidConfig。
        let mut cfg = base_cfg();
        cfg.insert("buffer_size", "0");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
        cfg.insert("buffer_size", "abc");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
        // dst_mac 非法 → InvalidConfig。
        let mut cfg = base_cfg();
        cfg.insert("dst_mac", "zz");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
    }

    #[test]
    fn start_poll_stop_lifecycle() {
        let mut plugin = SvPlugin::new();
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

    /// 39 收尾复验（P0）：配置真消费——app_id/dst_mac 进入传输 spec 与
    /// codec 过滤器（匹配帧收下、失配帧丢弃）；缺 app_id → InvalidConfig。
    #[test]
    fn start_consumes_configured_app_id() {
        let mut plugin = SvPlugin::new();
        let mut factory = ScriptedFactory::new();
        let mut cfg = base_cfg(); // app_id = 16385 (0x4001)
        cfg.insert("dst_mac", "01:0c:cd:04:00:11");
        plugin
            .start(&cfg, &mut factory, &mut EmptyPointRegistry)
            .expect("start");
        let specs = factory.specs.borrow();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].0, TransportKind::RawEthernet);
        assert_eq!(specs[0].1, b"010ccd040011");
        drop(specs);
        // 匹配帧 → 采样计数 +1。
        let dst = [0x01, 0x0C, 0xCD, 0x04, 0x00, 0x11];
        factory
            .shared
            .borrow_mut()
            .push_back(sv_frame(dst, 0x4001, 100, &[1.5, -2.5]));
        let progress = plugin
            .poll(
                100,
                WorkBudget {
                    max_work_ms: 20,
                    max_polls: 8,
                },
            )
            .unwrap();
        assert_eq!(progress, Progress::Completed);
        assert_eq!(plugin.samples_received(), 1);
        assert_eq!(plugin.last_smp_cnt(), 100);
        // 失配 app_id 帧 → 静默丢弃。
        factory
            .shared
            .borrow_mut()
            .push_back(sv_frame(dst, 0x4002, 101, &[1.0]));
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
        assert_eq!(plugin.samples_received(), 1);

        // 缺 app_id → InvalidConfig（默认值不伪健康）。
        let mut plugin2 = SvPlugin::new();
        assert_eq!(
            plugin2.start(
                &ProtocolConfig::default(),
                &mut factory,
                &mut EmptyPointRegistry
            ),
            Err(ProtocolError::InvalidConfig)
        );
    }

    /// smpCnt 连续性检查接线：跳变（丢样）→ SmpJump → Degraded + 计数。
    #[test]
    fn poll_wires_sample_counter_continuity() {
        let mut plugin = SvPlugin::new();
        let mut factory = ScriptedFactory::new();
        plugin
            .start(&base_cfg(), &mut factory, &mut EmptyPointRegistry)
            .expect("start");
        let dst = DEFAULT_SV_DST_MAC;
        // smpCnt 100 → 103（101/102 丢失）。
        factory
            .shared
            .borrow_mut()
            .push_back(sv_frame(dst, 0x4001, 100, &[1.0]));
        factory
            .shared
            .borrow_mut()
            .push_back(sv_frame(dst, 0x4001, 103, &[1.0]));
        let progress = plugin
            .poll(
                100,
                WorkBudget {
                    max_work_ms: 20,
                    max_polls: 8,
                },
            )
            .unwrap();
        assert_eq!(progress, Progress::Degraded);
        assert_eq!(plugin.samples_received(), 2);
        assert_eq!(plugin.sample_jumps(), 1);
        assert_eq!(plugin.health().error_count, 1);
    }

    /// 39 收尾复验（§6.3 重复）：同一 smpCnt 的 SV 帧重复注入——
    /// 幂等丢弃：不重复计入 samples_received、不告警、不 panic。
    #[test]
    fn test_39_duplicate_sample_idempotent() {
        let mut plugin = SvPlugin::new();
        let mut factory = ScriptedFactory::new();
        plugin
            .start(&base_cfg(), &mut factory, &mut EmptyPointRegistry)
            .expect("start");
        let dst = DEFAULT_SV_DST_MAC;
        let frame = sv_frame(dst, 0x4001, 100, &[1.0]);
        factory.shared.borrow_mut().push_back(frame.clone());
        factory.shared.borrow_mut().push_back(frame); // 重复
        let progress = plugin
            .poll(
                100,
                WorkBudget {
                    max_work_ms: 20,
                    max_polls: 8,
                },
            )
            .unwrap();
        assert_eq!(progress, Progress::Completed);
        assert_eq!(plugin.samples_received(), 1, "重复样本不得重复计数");
        assert_eq!(plugin.sample_jumps(), 0);
        assert_eq!(plugin.health().error_count, 0);
    }

    /// 39 收尾复验（§6.3 乱序）：smpCnt 回跳（旧帧乱序）——codec 归
    /// SmpJump → Degraded + sample_jumps/error_count 可检测（钉住现有
    /// fail-closed 语义；回跳与丢样共用 SmpJump 类别，无独立变体）。
    #[test]
    fn test_39_smp_cnt_rewind_detected_as_jump() {
        let mut plugin = SvPlugin::new();
        let mut factory = ScriptedFactory::new();
        plugin
            .start(&base_cfg(), &mut factory, &mut EmptyPointRegistry)
            .expect("start");
        let dst = DEFAULT_SV_DST_MAC;
        // 正常 100 → 101，随后回跳旧帧 99。
        factory
            .shared
            .borrow_mut()
            .push_back(sv_frame(dst, 0x4001, 100, &[1.0]));
        factory
            .shared
            .borrow_mut()
            .push_back(sv_frame(dst, 0x4001, 101, &[1.0]));
        factory
            .shared
            .borrow_mut()
            .push_back(sv_frame(dst, 0x4001, 99, &[1.0])); // 回跳
        let progress = plugin
            .poll(
                100,
                WorkBudget {
                    max_work_ms: 20,
                    max_polls: 8,
                },
            )
            .unwrap();
        assert_eq!(progress, Progress::Degraded, "smpCnt 回跳必须可检测");
        assert_eq!(plugin.sample_jumps(), 1);
        assert_eq!(plugin.health().error_count, 1);
        // 回跳帧也计入 samples_received？——不：它是 SmpJump 非 Duplicate，
        // 按现行语义（非重复即收）计数 3，回跳的可检测性由 jumps 承担。
        assert_eq!(plugin.samples_received(), 3);
    }

    #[test]
    fn start_rejects_unsupported_transport() {
        let mut plugin = SvPlugin::new();
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
}
