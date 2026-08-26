//! MQTT 协议插件（蓝图 39 WP-39.11）。
//!
//! 与其他插件同构：`Box<dyn TransportHandle>` 实现 [`MqttTransport`]
//! （本地 trait 实现外来类型）；插件持有 [`MqttClient`]，poll 时惰性
//! connect（事件循环语义）并驱动协议栈。安全通道组合（
//! [`crate::real::SecureMqttTransport`]）由装配层按 channel 策略注入。

use alloc::borrow::ToOwned;
use alloc::boxed::Box;

use eneros_protocol_abstract::write::PointWriteIntent;
use eneros_protocol_abstract::{
    AdapterState, InteropLevel, PointRegistry, Progress, ProtocolConfig, ProtocolDescriptor,
    ProtocolError, ProtocolHealth, ProtocolPlugin, ProtocolRole, ResourceBudget, SecurityProfile,
    TransportFactory, TransportHandle, TransportKind, TransportSpec, WorkBudget,
};

use crate::client::MqttClient;
use crate::error::MqttError;
use crate::packet::MqttPacket;
use crate::transport::MqttTransport;

/// 插件 id。
pub const MQTT_PLUGIN_ID: &str = "mqtt";

/// `Box<dyn TransportHandle>` 实现 [`MqttTransport`]。
///
/// connect(host, port)：工厂 open() 已按 spec.peer 建立传输连接（连接
/// 参数来自配置，不猜）；此处仅确认句柄在线——不在线 → NotConnected
/// （fail-closed，不伪装成功）。
impl MqttTransport for Box<dyn TransportHandle> {
    fn connect(&mut self, host: &str, port: u16) -> Result<(), MqttError> {
        // 50（WP-50.1 实弹修复）：MQTT connect **一律重建 TCP 连接**。
        // 半开 TCP（QEMU slirp 不向 guest 传播 host 侧关闭）下
        // is_connected 恒真——信任它会把 CONNECT 写进死链且 CONNACK
        // 永不到来。重建由 TcpStreamHandle::reconnect 承担（初始 open
        // 后首次 connect 多一次握手，代价可忽略）；随后 MqttClient 完成
        // CONNECT/CONNACK 握手。
        let peer = alloc::format!("{host}:{port}");
        TransportHandle::reconnect(self.as_mut(), &peer).map_err(|_| MqttError::NotConnected)
    }
    fn send(&mut self, data: &[u8]) -> Result<(), MqttError> {
        TransportHandle::send(self.as_mut(), data).map_err(|_| MqttError::TransportError)
    }
    fn recv(&mut self) -> Result<alloc::vec::Vec<u8>, MqttError> {
        let mut buf = [0u8; 4096];
        // 50（WP-50.1）：失败统一 PublishTimeout——插件侧以**连续失败
        // 计数**判定断连（半开 TCP 下 is_connected 恒真，无法依赖底层）。
        let n = TransportHandle::recv(self.as_mut(), &mut buf)
            .map_err(|_| MqttError::PublishTimeout)?;
        Ok(buf[..n].to_vec())
    }
    fn close(&mut self) -> Result<(), MqttError> {
        TransportHandle::close(self.as_mut()).map_err(|_| MqttError::TransportError)
    }
    fn is_connected(&self) -> bool {
        TransportHandle::is_connected(self.as_ref())
    }
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

/// 已知配置键。
const KNOWN_KEYS: &[&str] = &[
    "broker",
    "client_id",
    "keep_alive_ms",
    "qos",
    "will_topic",
    // 47（WP-47.1 协议入站写）：下行写命令订阅主题。缺省 = 不订阅
    //（方向一禁用——诚实：未配置不假装能收写命令）。
    "control_topic",
    // 49（WP-49.2 远程 dual-brain 传输面）：AI 请求发布主题 + 应答订阅
    // 主题。缺省 = AI 链路关闭（诚实：未配置不假装能收发 peer 消息）。
    "ai_request_topic",
    "ai_response_topic",
    // 50（WP-50.1 exporter 真通道）：遥测/指标/日志批量帧发布主题。
    // 缺省 = 遥测出口关闭（诚实：未配置不假装能导出遥测）。
    "telemetry_topic",
    // 50（WP-50.2 mgmt 通道）：管理请求订阅 / 应答发布主题。缺省 =
    // 管理通道关闭（诚实：未配置不假装能收发管理命令）。
    "mgmt_request_topic",
    "mgmt_response_topic",
];

/// 49：peer 应答收件箱上限（有界缓冲；满则丢弃新消息并计数）。
pub const AI_INBOX_CAP: usize = 8;
/// 49：app 发布 outbox 上限。
pub const APP_OUTBOX_CAP: usize = 4;
/// 49：单条 app 消息负载上限（契约层 PEER_REQUEST_MAX_LEN 同量级；
/// 插件不依赖契约 crate——传输面只认字节）。
pub const APP_MSG_MAX_LEN: usize = 1024;

/// MQTT 插件。
pub struct MqttPlugin {
    client: Option<MqttClient>,
    state: AdapterState,
    /// 连接状态（插件内跟踪；poll 惰性连接）。
    connected: bool,
    last_poll_ms: u64,
    error_count: u64,
    last_error: Option<ProtocolError>,
    last_progress: Progress,
    /// 45.4：注入点表路由点数（观测；服务层按协议过滤注入）。
    routed_count: usize,
    /// 47（WP-47.1）：下行写命令主题（可选；未配置 = 不收写）。
    control_topic: Option<alloc::string::String>,
    /// 47：control_topic 待订阅标志（connect 后惰性订阅——start 期传输
    /// 未连接，直接 subscribe 会在客户端未握手时失败）。
    pending_subscribe: bool,
    /// 47：control_topic 已订阅。
    subscribed: bool,
    /// 47（WP-47.1）：已解析待导出写意图（poll → drain）。
    pending_intents: alloc::vec::Vec<PointWriteIntent>,
    /// 47（WP-47.1）：意图序号（每协议单调；重放判定由 ControlDispatch）。
    intent_seq: u64,
    /// 47（WP-47.1）：已收到但无法解析的下行负载计数（观测；不 panic）。
    bad_downlink_count: u64,
    /// 49（WP-49.2）：AI 请求发布主题（可选；未配置 = AI 链路关闭）。
    ai_request_topic: Option<alloc::string::String>,
    /// 49：AI 应答订阅主题（可选）。
    ai_response_topic: Option<alloc::string::String>,
    /// 49：ai_response_topic 待订阅标志（connect 后惰性订阅）。
    ai_pending_subscribe: bool,
    /// 49：ai_response_topic 已订阅。
    ai_subscribed: bool,
    /// 50（WP-50.1）：遥测批量帧发布主题（可选；未配置 = 遥测出口关闭）。
    telemetry_topic: Option<alloc::string::String>,
    /// 50：遥测帧被 svc-mqtt 拒绝计数（坏帧/未配置/断连 fail-closed）。
    pub telemetry_rejected_count: u64,
    /// 50：已发布遥测帧计数（观测）。
    pub telemetry_published_count: u64,
    /// 50（WP-50.2）：管理请求订阅主题（可选；未配置 = 管理通道关闭）。
    mgmt_request_topic: Option<alloc::string::String>,
    /// 50（WP-50.2）：管理应答发布主题（可选）。
    mgmt_response_topic: Option<alloc::string::String>,
    /// 50：mgmt_request_topic 待订阅标志（connect 后惰性订阅）。
    mgmt_pending_subscribe: bool,
    /// 50：mgmt_request_topic 已订阅。
    mgmt_subscribed: bool,
    /// 50：已收到待转发管理请求（原始 MGT1 帧；服务层 drain 后经 EIPC
    /// 推送 svc-mgmt——插件不解释内容）。有界；满则丢新 + 计数。
    mgmt_inbox: alloc::vec::Vec<alloc::vec::Vec<u8>>,
    /// 50：mgmt inbox 满 / 超限丢弃计数（观测）。
    pub mgmt_dropped_count: u64,

    /// 49：待发布 app 消息（topic, payload）——服务层入队，poll 时 QoS1 发布。
    app_outbox: alloc::vec::Vec<(alloc::string::String, alloc::vec::Vec<u8>)>,
    /// 49：已收到待转发的 peer 应答（原始字节；服务层 drain 后经 EIPC 推送
    /// svc-ai-worker——插件不解释内容）。
    peer_inbox: alloc::vec::Vec<alloc::vec::Vec<u8>>,
    /// 49：inbox 满 / 超限丢弃计数（观测）。
    pub ai_dropped_count: u64,
}

impl MqttPlugin {
    pub fn new() -> Self {
        Self {
            client: None,
            state: AdapterState::Uninitialized,
            connected: false,
            last_poll_ms: 0,
            error_count: 0,
            last_error: None,
            last_progress: Progress::Idle,
            routed_count: 0,
            control_topic: None,
            pending_subscribe: false,
            subscribed: false,
            pending_intents: alloc::vec::Vec::new(),
            intent_seq: 0,
            bad_downlink_count: 0,
            ai_request_topic: None,
            ai_response_topic: None,
            ai_pending_subscribe: false,
            ai_subscribed: false,
            telemetry_topic: None,
            telemetry_rejected_count: 0,
            telemetry_published_count: 0,
            mgmt_request_topic: None,
            mgmt_response_topic: None,
            mgmt_pending_subscribe: false,
            mgmt_subscribed: false,
            mgmt_inbox: alloc::vec::Vec::new(),
            mgmt_dropped_count: 0,
            app_outbox: alloc::vec::Vec::new(),
            peer_inbox: alloc::vec::Vec::new(),
            ai_dropped_count: 0,
        }
    }

    /// 45.4：注入点表路由点数（观测）。
    pub fn routed_count(&self) -> usize {
        self.routed_count
    }

    /// 47（WP-47.1）：不可解析下行负载计数（观测）。
    pub fn bad_downlink_count(&self) -> u64 {
        self.bad_downlink_count
    }

    // ================= 49（WP-49.2）远程 dual-brain 传输面 =================

    /// 主题是否为 AI 应答订阅主题。
    fn is_ai_response_topic(&self, topic: &str) -> bool {
        match self.ai_response_topic.as_ref() {
            Some(t) => topic == t,
            None => false,
        }
    }

    /// peer 应答原始字节入收件箱（有界；满则丢新计数——不阻塞不伪造）。
    fn on_peer_message_inbox(&mut self, payload: alloc::vec::Vec<u8>) {
        if self.peer_inbox.len() >= AI_INBOX_CAP {
            self.ai_dropped_count = self.ai_dropped_count.saturating_add(1);
            return;
        }
        self.peer_inbox.push(payload);
    }

    /// 服务层入队一条 AI 请求（poll 时经 QoS1 发布到 ai_request_topic）。
    ///
    /// 未配置 ai_request_topic / 超限（条数或字节数）→ 显式拒绝（fail-closed，
    /// 调用方计数）——不假装接受。
    pub fn enqueue_app_publish(
        &mut self,
        topic: &str,
        payload: &[u8],
    ) -> Result<(), ProtocolError> {
        if self.ai_request_topic.as_deref() != Some(topic) {
            return Err(ProtocolError::InvalidConfig);
        }
        if payload.len() > APP_MSG_MAX_LEN || self.app_outbox.len() >= APP_OUTBOX_CAP {
            self.ai_dropped_count = self.ai_dropped_count.saturating_add(1);
            return Err(ProtocolError::InvalidConfig);
        }
        self.app_outbox.push((
            alloc::string::String::from(topic),
            alloc::vec::Vec::from(payload),
        ));
        Ok(())
    }

    /// 服务层取走全部已收 peer 应答（原始字节；顺序保持）。
    pub fn drain_peer_messages(&mut self, out: &mut alloc::vec::Vec<alloc::vec::Vec<u8>>) {
        out.append(&mut self.peer_inbox);
    }

    /// 50（WP-50.1）：服务层入队一帧遥测批量帧（poll 时经 QoS1 发布到
    /// `telemetry_topic`）。未配置 telemetry_topic / 超限（条数或字节数）→
    /// 显式拒绝 + 计数（fail-closed，svc-mqtt 据此回 ST_UNAVAILABLE——调用
    /// 方 durable outbox 兜底补传）。
    pub fn enqueue_telemetry_publish(&mut self, payload: &[u8]) -> Result<(), ProtocolError> {
        let Some(topic) = self.telemetry_topic.clone() else {
            self.telemetry_rejected_count = self.telemetry_rejected_count.saturating_add(1);
            return Err(ProtocolError::InvalidConfig);
        };
        if payload.len() > APP_MSG_MAX_LEN || self.app_outbox.len() >= APP_OUTBOX_CAP {
            self.telemetry_rejected_count = self.telemetry_rejected_count.saturating_add(1);
            return Err(ProtocolError::InvalidConfig);
        }
        self.app_outbox
            .push((topic, alloc::vec::Vec::from(payload)));
        Ok(())
    }

    /// 50：遥测发布主题（svc-mqtt 入队校验/观测用；None = 遥测出口关闭）。
    pub fn telemetry_topic(&self) -> Option<&str> {
        self.telemetry_topic.as_deref()
    }

    /// 50：遥测帧拒绝/发布计数（观测）。
    pub fn telemetry_rejected(&self) -> u64 {
        self.telemetry_rejected_count
    }
    pub fn telemetry_published(&self) -> u64 {
        self.telemetry_published_count
    }

    /// 50（WP-50.1）：broker 连接状态（svc-mqtt 边界判据——断连时拒绝
    /// 遥测帧 → 调用方 durable outbox 兜底补传）。取**底层传输真实状态**
    /// （smoltcp socket 是否 connected），而非插件缓存标志——空闲 recv
    /// 超时不是断连。
    pub fn is_connected(&self) -> bool {
        self.client
            .as_ref()
            .and_then(|c| c.transport.as_ref())
            .map(|t| t.is_connected())
            .unwrap_or(false)
    }

    // ================= 50（WP-50.2）mgmt 通道传输面 =================

    /// 主题是否为管理请求订阅主题。
    fn is_mgmt_request_topic(&self, topic: &str) -> bool {
        match self.mgmt_request_topic.as_ref() {
            Some(t) => topic == t,
            None => false,
        }
    }

    /// 管理请求原始帧入收件箱（有界；满则丢新 + 计数——不阻塞不伪造）。
    fn on_mgmt_request_inbox(&mut self, payload: alloc::vec::Vec<u8>) {
        if self.mgmt_inbox.len() >= AI_INBOX_CAP {
            self.mgmt_dropped_count = self.mgmt_dropped_count.saturating_add(1);
            return;
        }
        self.mgmt_inbox.push(payload);
    }

    /// 服务层取走全部已收管理请求（原始 MGT1 帧；顺序保持）。
    pub fn drain_mgmt_requests(&mut self, out: &mut alloc::vec::Vec<alloc::vec::Vec<u8>>) {
        out.append(&mut self.mgmt_inbox);
    }

    /// 服务层入队一条管理应答（poll 时经 QoS1 发布到 mgmt_response_topic）。
    /// 未配置主题 / 超限 → 显式拒绝（fail-closed，调用方计数）。
    pub fn enqueue_mgmt_response(&mut self, payload: &[u8]) -> Result<(), ProtocolError> {
        let Some(topic) = self.mgmt_response_topic.clone() else {
            self.mgmt_dropped_count = self.mgmt_dropped_count.saturating_add(1);
            return Err(ProtocolError::InvalidConfig);
        };
        if payload.len() > APP_MSG_MAX_LEN || self.app_outbox.len() >= APP_OUTBOX_CAP {
            self.mgmt_dropped_count = self.mgmt_dropped_count.saturating_add(1);
            return Err(ProtocolError::InvalidConfig);
        }
        self.app_outbox
            .push((topic, alloc::vec::Vec::from(payload)));
        Ok(())
    }

    /// 50：管理请求主题（bin 订阅校验/观测用；None = 管理通道关闭）。
    pub fn mgmt_request_topic(&self) -> Option<&str> {
        self.mgmt_request_topic.as_deref()
    }

    /// 50：管理消息丢弃计数（观测）。
    pub fn mgmt_dropped(&self) -> u64 {
        self.mgmt_dropped_count
    }

    /// 49：AI 消息丢弃计数（观测）。
    pub fn ai_dropped(&self) -> u64 {
        self.ai_dropped_count
    }

    /// 49：AI 请求发布主题（bin 入队校验用；None = AI 链路关闭）。
    pub fn ai_request_topic(&self) -> Option<&str> {
        self.ai_request_topic.as_deref()
    }

    /// 49：outbox 积压数（观测/测试）。
    pub fn app_outbox_len(&self) -> usize {
        self.app_outbox.len()
    }

    /// 47（WP-47.1）：解析一条下行写命令负载为写意图。
    ///
    /// 负载文本格式（外部 SCADA 约定）：`SET <device_id> <point_id> <value>`
    /// ——ASCII 空白分隔；value = 十进制浮点（kW 语义）。格式不符 →
    /// 计数 + 丢弃（不 panic、不伪造）；主题不符 → 忽略。
    /// **只导出意图，禁止直接写设备**（WP-47.1 红线）。
    fn on_downlink(&mut self, topic: &str, payload: &[u8], now_ms: u64) {
        let Some(ctl) = self.control_topic.as_ref() else {
            return;
        };
        if topic != ctl {
            return;
        }
        let text = match core::str::from_utf8(payload) {
            Ok(t) => t.trim(),
            Err(_) => {
                self.bad_downlink_count += 1;
                return;
            }
        };
        let mut parts = text.split_whitespace();
        let cmd = parts.next().unwrap_or("");
        let (Some(dev), Some(point), Some(val)) = (parts.next(), parts.next(), parts.next()) else {
            self.bad_downlink_count += 1;
            return;
        };
        if parts.next().is_some() {
            // 多余字段 → 格式不符（防错发）。
            self.bad_downlink_count += 1;
            return;
        }
        if cmd != "SET" {
            return; // 其他命令字：非本插件写协议（忽略，不计数为坏负载）。
        }
        let (Ok(device_id), Ok(point_id), Ok(value)) =
            (dev.parse::<u32>(), point.parse::<u32>(), val.parse::<f64>())
        else {
            self.bad_downlink_count += 1;
            return;
        };
        if !value.is_finite() || device_id == 0 {
            self.bad_downlink_count += 1;
            return;
        }
        self.intent_seq = self.intent_seq.saturating_add(1);
        self.pending_intents.push(PointWriteIntent {
            protocol_id: alloc::string::String::from(MQTT_PLUGIN_ID),
            device_id: device_id as u16,
            point_id,
            value: eneros_upa_model::PointValue::Float(value),
            sequence: self.intent_seq,
            issued_ms: now_ms,
        });
    }
}

impl Default for MqttPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolPlugin for MqttPlugin {
    fn descriptor(&self) -> &ProtocolDescriptor {
        static D: ProtocolDescriptor = ProtocolDescriptor {
            id: MQTT_PLUGIN_ID,
            version: "1.0",
            role: ProtocolRole::Client,
            transport_kinds: &[TransportKind::Tcp],
            security_profile: SecurityProfile::Tls12,
            point_types: &["float", "int", "bool", "string"],
            resource_budget: ResourceBudget {
                max_connections: 1,
                max_poll_ms: 50,
                max_frame: 4096,
            },
            // 46.5：QEMU/EL0 + 独立对端 L2 闭环（evidence/46/WP-46.5.md）。
            interop_level: InteropLevel::TargetPeerVerified,
        };
        &D
    }

    fn validate_config(&self, cfg: &ProtocolConfig) -> Result<(), ProtocolError> {
        eneros_protocol_abstract::plugin::reject_unknown_keys(cfg, KNOWN_KEYS)?;
        // broker 必须可解析（host[:port]）且 ≤ 128 字节（TransportSpec.peer
        // 容量；39 收尾复验 P2-1：超长不得留到 start 内 panic）。
        match cfg.get("broker") {
            Some(broker) if !broker.is_empty() && broker.len() <= 128 => Ok(()),
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
                    eneros_protocol_abstract::ProtocolAddress::Mqtt { .. }
                )
            });
        if self.client.is_some() {
            return Err(ProtocolError::ProtocolInit);
        }
        if !transports.supports(TransportKind::Tcp) {
            return Err(ProtocolError::Unsupported);
        }
        // 39 收尾复验（P0）：消费 validate_config 校验过的配置本体
        //（旧实现此处重建 default 空配置——broker 永远 127.0.0.1:1883，
        // 配置链整体断裂）；缺省值仅用于未提供的可选键。
        let broker = cfg
            .get("broker")
            .filter(|b| !b.is_empty() && b.len() <= 128)
            .ok_or(ProtocolError::InvalidConfig)?
            .to_owned();
        let client_id = cfg.get("client_id").unwrap_or("eneros-agent").to_owned();
        let keep_alive_ms: u64 =
            eneros_protocol_abstract::plugin::config_u64(cfg, "keep_alive_ms")?.unwrap_or(60_000);
        let mut spec = TransportSpec {
            kind: TransportKind::Tcp,
            peer: [0; 128],
            peer_len: 0,
            mtu: 4096,
            connect_timeout_ms: 2000,
            // 50（WP-50.1 实弹登记）：维持 1000ms——实测 150ms 使空闲 poll
            // 错误率放大 ~6×，与断连判定/重连节拍交互导致启动期整体爬慢
            // （long5/long6/final6 三连复现；long4 @1000ms 全程正常）。
            // 入站数据由 rx_buf 残留缓冲续收，不丢。
            recv_timeout_ms: 1000,
        };
        let bytes = broker.as_bytes();
        spec.peer[..bytes.len()].copy_from_slice(bytes);
        spec.peer_len = bytes.len() as u8;
        let handle = transports
            .open(&spec)
            .map_err(|_| ProtocolError::ProtocolInit)?;
        let transport: Box<dyn MqttTransport> = Box::new(handle);
        let mut client = MqttClient::new(&broker, &client_id, keep_alive_ms);
        client.set_transport(transport);
        // 47（WP-47.1）：下行写命令主题（可选）——配置了才订阅；订阅
        // 延后到首次 poll connect 之后（start 期传输未握手，直调
        // subscribe 必失败）。
        self.control_topic = cfg
            .get("control_topic")
            .filter(|t| !t.is_empty())
            .map(|t| t.to_owned());
        if self.control_topic.is_some() {
            self.pending_subscribe = true;
        }
        // 49（WP-49.2）：AI 请求/应答主题（可选）——应答主题 connect 后
        // 惰性订阅；请求主题仅作发布目标（不订阅）。
        self.ai_response_topic = cfg
            .get("ai_response_topic")
            .filter(|t| !t.is_empty())
            .map(|t| t.to_owned());
        if self.ai_response_topic.is_some() {
            self.ai_pending_subscribe = true;
        }
        self.ai_request_topic = cfg
            .get("ai_request_topic")
            .filter(|t| !t.is_empty())
            .map(|t| t.to_owned());
        // 50（WP-50.1）：遥测发布主题（可选；未配置 = 遥测出口关闭）。
        self.telemetry_topic = cfg
            .get("telemetry_topic")
            .filter(|t| !t.is_empty())
            .map(|t| t.to_owned());
        // 50（WP-50.2）：管理请求订阅 / 应答发布主题（可选）。
        self.mgmt_response_topic = cfg
            .get("mgmt_response_topic")
            .filter(|t| !t.is_empty())
            .map(|t| t.to_owned());
        self.mgmt_request_topic = cfg
            .get("mgmt_request_topic")
            .filter(|t| !t.is_empty())
            .map(|t| t.to_owned());
        if self.mgmt_request_topic.is_some() {
            self.mgmt_pending_subscribe = true;
        }
        self.client = Some(client);
        self.state = AdapterState::Running;
        Ok(())
    }

    fn poll(&mut self, now_ms: u64, _budget: WorkBudget) -> Result<Progress, ProtocolError> {
        let Some(client) = self.client.as_mut() else {
            return Err(ProtocolError::ProtocolNotStarted);
        };
        self.last_poll_ms = now_ms;
        // 惰性连接（事件循环语义：首次 poll 建立 CONNECT 握手）。
        if !self.connected {
            match client.connect(now_ms) {
                Ok(()) => self.connected = true,
                Err(_) => {
                    self.error_count += 1;
                    self.last_error = Some(ProtocolError::ReadFailed);
                    self.last_progress = Progress::Degraded;
                    return Err(ProtocolError::ReadFailed);
                }
            }
        }
        // 47：connect 后惰性订阅 control_topic（若上轮失败本轮重试）。
        if self.connected && self.pending_subscribe && !self.subscribed {
            if let Some(topic) = self.control_topic.clone() {
                if client
                    .subscribe(&topic, crate::qos::QoS::AtMostOnce)
                    .is_ok()
                {
                    self.pending_subscribe = false;
                    self.subscribed = true;
                }
            }
        }
        // 49（WP-49.2）：connect 后惰性订阅 ai_response_topic（QoS1——
        // 蓝图传输判据；失败下轮重试）。
        if self.connected && self.ai_pending_subscribe && !self.ai_subscribed {
            if let Some(topic) = self.ai_response_topic.clone() {
                if client
                    .subscribe(&topic, crate::qos::QoS::AtLeastOnce)
                    .is_ok()
                {
                    self.ai_pending_subscribe = false;
                    self.ai_subscribed = true;
                }
            }
        }
        // 50（WP-50.2）：connect 后惰性订阅 mgmt_request_topic（失败下轮重试）。
        if self.connected && self.mgmt_pending_subscribe && !self.mgmt_subscribed {
            if let Some(topic) = self.mgmt_request_topic.clone() {
                if client
                    .subscribe(&topic, crate::qos::QoS::AtLeastOnce)
                    .is_ok()
                {
                    self.mgmt_pending_subscribe = false;
                    self.mgmt_subscribed = true;
                }
            }
        }
        // 49：outbox 冲刷（每轮排空至多 APP_OUTBOX_CAP 条——积压会使
        // 应答相对请求错相，49 第六轮实弹根因；单条失败保留待下轮）。
        if self.connected && !self.app_outbox.is_empty() {
            while !self.app_outbox.is_empty() {
                let Some((topic, payload)) = self.app_outbox.first() else {
                    break;
                };
                let topic = topic.clone();
                let payload = payload.clone();
                if client
                    .publish(&topic, &payload, crate::qos::QoS::AtLeastOnce, now_ms)
                    .is_ok()
                {
                    self.app_outbox.remove(0);
                    // 50（WP-50.1）：遥测帧发布计数（观测）。
                    if Some(topic.as_str()) == self.telemetry_topic.as_deref() {
                        self.telemetry_published_count =
                            self.telemetry_published_count.saturating_add(1);
                    }
                } else {
                    self.error_count += 1;
                    break;
                }
            }
        }
        match client.poll(now_ms) {
            Ok(pkts) => {
                // 47（WP-47.1）：下行 PUBLISH（control_topic）→ 解析为写意图
                //（只导出；设备写由 ControlDispatch 授权后执行）。
                // 49（WP-49.2）：ai_response_topic PUBLISH → 原始字节入
                // peer_inbox（不解释内容——契约解释权在 svc-ai-worker）。
                for pkt in pkts {
                    if let MqttPacket::Publish(pub_) = pkt {
                        if self.is_ai_response_topic(&pub_.topic) {
                            self.on_peer_message_inbox(pub_.payload);
                        } else if self.is_mgmt_request_topic(&pub_.topic) {
                            // 50（WP-50.2）：管理请求原始帧入收件箱（有界；
                            // 满则丢新 + 计数——服务层 drain 后经 EIPC 推送
                            // svc-mgmt，插件不解释内容）。
                            self.on_mgmt_request_inbox(pub_.payload);
                        } else {
                            self.on_downlink(&pub_.topic, &pub_.payload, now_ms);
                        }
                    }
                }
                self.last_progress = Progress::Completed;
                // 50（WP-50.1）：底层真断连（Closed→NotConnected→client 已
                // mark_disconnected）→ 同步插件缓存状态 + 复位惰性订阅标志，
                // 下轮 poll 走 `!self.connected` 重连 seam。空闲超时
                // （PublishTimeout）不会进入此分支——不是断连。
                let client_connected = matches!(
                    self.client.as_ref().map(|c| c.state),
                    Some(crate::client::ConnectionState::Connected)
                );
                if !client_connected {
                    self.connected = false;
                    self.subscribed = false;
                    self.pending_subscribe = self.control_topic.is_some();
                    self.ai_subscribed = false;
                    self.ai_pending_subscribe = self.ai_response_topic.is_some();
                    self.mgmt_subscribed = false;
                    self.mgmt_pending_subscribe = self.mgmt_request_topic.is_some();
                }
                Ok(Progress::Completed)
            }
            Err(_) => {
                self.error_count += 1;
                self.last_error = Some(ProtocolError::ReadFailed);
                self.last_progress = Progress::Degraded;
                // 50（WP-50.1）：**仅当底层传输真正断开**（socket 关闭——
                // broker 死 → FIN/RST）才标记断线 + 重置惰性订阅标志；空闲
                // recv 超时（无入站数据）不是断连。下轮 poll 走
                // `!self.connected` 分支重连（TcpStreamHandle::reconnect seam）。
                // 半开 TCP（slirp 不传 FIN）下 is_connected 恒真——断连由
                // client 侧 keepalive PING 无应答 mark_disconnected 判定
                // （50 实弹修复）；此处经 client 状态同步插件缓存。
                let client_down = client.state != crate::client::ConnectionState::Connected;
                let transport_down = client
                    .transport
                    .as_ref()
                    .map(|t| !t.is_connected())
                    .unwrap_or(true)
                    || client_down;
                if transport_down {
                    client.mark_disconnected();
                    self.connected = false;
                    self.subscribed = false;
                    self.pending_subscribe = self.control_topic.is_some();
                    self.ai_subscribed = false;
                    self.ai_pending_subscribe = self.ai_response_topic.is_some();
                    self.mgmt_subscribed = false;
                    self.mgmt_pending_subscribe = self.mgmt_request_topic.is_some();
                }
                Err(ProtocolError::ReadFailed)
            }
        }
    }

    fn health(&self) -> ProtocolHealth {
        ProtocolHealth {
            state: self.state,
            last_poll_ms: self.last_poll_ms,
            error_count: self.error_count,
            last_error: self.last_error.clone(),
            last_progress: self.last_progress,
        }
    }

    fn stop(&mut self) -> Result<(), ProtocolError> {
        if let Some(client) = self.client.as_mut() {
            let _ = client.disconnect();
        }
        self.client = None;
        self.state = AdapterState::Stopped;
        Ok(())
    }

    fn drain_write_intents(&mut self, out: &mut alloc::vec::Vec<PointWriteIntent>) {
        out.append(&mut self.pending_intents);
    }
}

#[cfg(test)]
mod tests {

    use eneros_protocol_abstract::{EmptyPointRegistry, TransportError};

    //（TransportError 仅测试面使用——lib 面经 Box<dyn TransportHandle> 映射消化。）
    use super::*;

    /// 模拟 MQTT broker 的最小传输：CONNECT → CONNACK(0)，其余 echo。
    #[derive(Default)]
    struct MqttBrokerTransport {
        last: core::cell::RefCell<alloc::vec::Vec<u8>>,
    }

    impl TransportHandle for MqttBrokerTransport {
        fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
            *self.last.borrow_mut() = data.to_vec();
            Ok(())
        }
        fn recv(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
            let data = self.last.borrow().clone();
            // CONNECT 包（0x10 开头）→ 应答 CONNACK。
            let reply: alloc::vec::Vec<u8> = if data.first() == Some(&0x10) {
                alloc::vec![0x20, 0x02, 0x00, 0x00]
            } else {
                data
            };
            let n = reply.len().min(buf.len());
            buf[..n].copy_from_slice(&reply[..n]);
            Ok(n)
        }
        fn close(&mut self) -> Result<(), TransportError> {
            Ok(())
        }
        fn is_connected(&self) -> bool {
            true
        }
        // 50：connect 一律重建语义下，测试 echo 传输的重建 = 无操作 Ok。
        fn reconnect(&mut self, _peer: &str) -> Result<(), TransportError> {
            Ok(())
        }
        fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
            self
        }
    }

    struct TestTcpFactory;

    impl TransportFactory for TestTcpFactory {
        fn open(
            &mut self,
            _spec: &TransportSpec,
        ) -> Result<Box<dyn TransportHandle>, TransportError> {
            Ok(Box::new(MqttBrokerTransport::default()))
        }
        fn supports(&self, kind: TransportKind) -> bool {
            kind == TransportKind::Tcp
        }
    }

    #[test]
    fn validate_config_rejects_unknown_and_empty_broker() {
        let plugin = MqttPlugin::new();
        let mut cfg = ProtocolConfig::default();
        cfg.insert("broker", "127.0.0.1:1883");
        assert!(plugin.validate_config(&cfg).is_ok());
        cfg.insert("broker", "");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
        cfg.insert("broker", "127.0.0.1:1883");
        cfg.insert("evil", "1");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
    }

    #[test]
    fn start_poll_stop_lifecycle() {
        let mut plugin = MqttPlugin::new();
        let mut factory = TestTcpFactory;
        let mut cfg = ProtocolConfig::default();
        cfg.insert("broker", "127.0.0.1:1883");
        plugin
            .start(&cfg, &mut factory, &mut EmptyPointRegistry)
            .expect("start");
        assert_eq!(plugin.health().state, AdapterState::Running);
        plugin
            .poll(
                1000,
                WorkBudget {
                    max_work_ms: 50,
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
                    max_work_ms: 50,
                    max_polls: 1
                }
            ),
            Err(ProtocolError::ProtocolNotStarted)
        );
    }

    /// 39 收尾复验（P0）：配置的 broker 真正进入传输 spec（配置链
    /// 不再断裂；旧实现恒用硬编码 127.0.0.1:1883）。
    #[test]
    fn start_consumes_configured_broker() {
        /// 捕获 open() 收到的 spec。
        struct CapturingFactory {
            peer: core::cell::RefCell<alloc::vec::Vec<u8>>,
        }
        impl TransportFactory for CapturingFactory {
            fn open(
                &mut self,
                spec: &TransportSpec,
            ) -> Result<Box<dyn TransportHandle>, TransportError> {
                *self.peer.borrow_mut() = spec.peer[..spec.peer_len as usize].to_vec();
                Ok(Box::new(MqttBrokerTransport::default()))
            }
            fn supports(&self, kind: TransportKind) -> bool {
                kind == TransportKind::Tcp
            }
        }
        let mut plugin = MqttPlugin::new();
        let mut factory = CapturingFactory {
            peer: core::cell::RefCell::new(alloc::vec::Vec::new()),
        };
        let mut cfg = ProtocolConfig::default();
        cfg.insert("broker", "10.7.0.42:8883");
        plugin
            .start(&cfg, &mut factory, &mut EmptyPointRegistry)
            .expect("start");
        assert_eq!(*factory.peer.borrow(), b"10.7.0.42:8883");
    }

    /// 39 收尾复验（P2-1）：broker 超 128 字节 → 校验拒绝（不留到
    /// start 内 panic）。
    #[test]
    fn validate_config_rejects_oversized_broker() {
        let plugin = MqttPlugin::new();
        let mut cfg = ProtocolConfig::default();
        cfg.insert(
            "broker",
            &alloc::string::String::from_utf8(alloc::vec![b'a'; 200]).unwrap(),
        );
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
    }

    // ================= 47（WP-47.1）协议入站写 =================

    /// 47（WP-47.1）：control_topic 下行 SET 负载 → 写意图导出
    ///（只导出；序列单调；drain 清空缓冲）。
    #[test]
    fn downlink_set_parses_to_write_intent() {
        let mut plugin = MqttPlugin::new();
        plugin.control_topic = Some(alloc::string::String::from("eneros/ctl"));
        plugin.on_downlink("eneros/ctl", b"SET 1 10 50.0", 1_700_000_000_010);
        plugin.on_downlink("eneros/ctl", b"SET 1 10 -20.5", 1_700_000_000_020);
        let mut out = alloc::vec::Vec::new();
        plugin.drain_write_intents(&mut out);
        assert_eq!(out.len(), 2, "两条下行都必须导出");
        assert_eq!(out[0].protocol_id, MQTT_PLUGIN_ID);
        assert_eq!(out[0].device_id, 1);
        assert_eq!(out[0].point_id, 10);
        assert_eq!(out[0].value, eneros_upa_model::PointValue::Float(50.0));
        assert_eq!(out[0].sequence, 1, "序号单调");
        assert_eq!(out[1].sequence, 2);
        assert_eq!(out[1].issued_ms, 1_700_000_000_020);
        // drain 后缓冲清空（第二只读为空）。
        let mut again = alloc::vec::Vec::new();
        plugin.drain_write_intents(&mut again);
        assert!(again.is_empty());
    }

    /// 47（WP-47.1）：主题不符 / 坏格式 / 非 SET 命令 → 不导出意图；
    /// 坏下行计数（不 panic、不伪造）。
    #[test]
    fn downlink_bad_payloads_counted_and_ignored() {
        let mut plugin = MqttPlugin::new();
        plugin.control_topic = Some(alloc::string::String::from("eneros/ctl"));
        plugin.on_downlink("other/topic", b"SET 1 10 50", 1); // 主题不符
        plugin.on_downlink("eneros/ctl", b"SET 1 10", 2); // 字段不足
        plugin.on_downlink("eneros/ctl", b"SET 1 x 50", 3); // 点非数字
        plugin.on_downlink("eneros/ctl", b"SET 0 10 50", 4); // device 0
        plugin.on_downlink("eneros/ctl", b"SET 1 10 NaN", 5); // 非有限值
        plugin.on_downlink("eneros/ctl", b"SET 1 10 50 extra", 6); // 多余字段
        plugin.on_downlink("eneros/ctl", b"GET 1 10 50", 7); // 非 SET 命令
        let mut out = alloc::vec::Vec::new();
        plugin.drain_write_intents(&mut out);
        assert!(out.is_empty(), "全部坏负载不得导出意图: {out:?}");
        assert_eq!(
            plugin.bad_downlink_count, 5,
            "5 条格式不符计数（主题不符与非 SET 命令不计数）"
        );
    }

    /// 47（WP-47.1）：意图线码往返（svc 间 EIPC 传输同源）。
    #[test]
    fn downlink_intent_survives_wire_roundtrip() {
        let mut plugin = MqttPlugin::new();
        plugin.control_topic = Some(alloc::string::String::from("eneros/ctl"));
        plugin.on_downlink("eneros/ctl", b"SET 1 10 72.25", 99);
        let mut out = alloc::vec::Vec::new();
        plugin.drain_write_intents(&mut out);
        assert_eq!(out.len(), 1);
        let wire = eneros_protocol_abstract::write::encode_intent(&out[0]);
        let back = eneros_protocol_abstract::write::decode_intent(&wire).expect("decode");
        assert_eq!(back, out[0]);
    }

    // ================= 49（WP-49.2）远程 dual-brain 传输面 =================

    /// 49：应答主题 PUBLISH → 原始字节入 inbox（不解释）；drain 清空；
    /// control_topic 解析不受影响（双主题并存路由正确）。
    #[test]
    fn ai_response_routed_to_inbox_and_control_still_parsed() {
        let mut plugin = MqttPlugin::new();
        plugin.control_topic = Some(alloc::string::String::from("eneros/ctl"));
        plugin.ai_response_topic = Some(alloc::string::String::from("eneros/brain/response"));
        // AI 应答（任意字节——含非 UTF-8，插件不解释）。
        let raw = alloc::vec![0x7b, 0x22, 0xff, 0x7d];
        plugin.on_peer_message_inbox(raw.clone());
        // control_topic 写命令照常解析。
        plugin.on_downlink("eneros/ctl", b"SET 1 10 50.0", 1);
        let mut msgs = alloc::vec::Vec::new();
        plugin.drain_peer_messages(&mut msgs);
        assert_eq!(msgs, alloc::vec![raw], "AI 应答原样入箱");
        let mut intents = alloc::vec::Vec::new();
        plugin.drain_write_intents(&mut intents);
        assert_eq!(intents.len(), 1, "control 链不受影响");
    }

    /// 49：inbox 有界——满后丢新消息并计数。
    #[test]
    fn ai_inbox_bounded_drops_new_and_counts() {
        let mut plugin = MqttPlugin::new();
        plugin.ai_response_topic = Some(alloc::string::String::from("r"));
        for i in 0..AI_INBOX_CAP {
            plugin.on_peer_message_inbox(alloc::vec![i as u8]);
        }
        plugin.on_peer_message_inbox(alloc::vec![0xEE]);
        assert_eq!(plugin.peer_inbox.len(), AI_INBOX_CAP);
        assert_eq!(plugin.ai_dropped(), 1);
        let mut msgs = alloc::vec::Vec::new();
        plugin.drain_peer_messages(&mut msgs);
        assert!(msgs.iter().all(|m| m[0] != 0xEE), "新消息须被丢弃");
    }

    /// 49：enqueue 校验 ai_request_topic 配置 + 超限拒绝（fail-closed）。
    #[test]
    fn enqueue_app_publish_validates_topic_and_limits() {
        let mut plugin = MqttPlugin::new();
        // 未配置请求主题 → 拒绝。
        assert!(plugin.enqueue_app_publish("t", b"x").is_err());
        plugin.ai_request_topic = Some(alloc::string::String::from("eneros/brain/request"));
        // 主题不匹配 → 拒绝。
        assert!(plugin.enqueue_app_publish("other", b"x").is_err());
        // 正常入队。
        assert!(plugin
            .enqueue_app_publish("eneros/brain/request", b"{\"req_id\":1}")
            .is_ok());
        assert_eq!(plugin.app_outbox_len(), 1);
        // 超长 → 拒绝 + 计数。
        let big = alloc::vec![0u8; APP_MSG_MAX_LEN + 1];
        assert!(plugin
            .enqueue_app_publish("eneros/brain/request", &big)
            .is_err());
        assert_eq!(plugin.ai_dropped(), 1);
        // 积压上限 → 拒绝 + 计数。
        for i in 0..APP_OUTBOX_CAP - 1 {
            assert!(plugin
                .enqueue_app_publish("eneros/brain/request", alloc::format!("{{{i}}}").as_bytes())
                .is_ok());
        }
        assert!(plugin
            .enqueue_app_publish("eneros/brain/request", b"{}")
            .is_err());
        assert_eq!(plugin.ai_dropped(), 2);
    }

    /// 49：validate_config 接受 AI 主题键（KNOWN_KEYS 扩展回归——未知键
    /// 拒绝语义保持）。
    #[test]
    fn validate_config_accepts_ai_topics_rejects_unknown() {
        let plugin = MqttPlugin::new();
        let mut cfg = ProtocolConfig::default();
        cfg.insert("broker", "10.0.2.2:1883");
        cfg.insert("ai_request_topic", "eneros/brain/request");
        cfg.insert("ai_response_topic", "eneros/brain/response");
        assert_eq!(plugin.validate_config(&cfg), Ok(()));
        cfg.insert("ai_evil_key", "x");
        assert_eq!(
            plugin.validate_config(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
    }

    // ================= 50（WP-50.1）遥测出口 =================

    /// 50：telemetry_topic 配置被 validate_config 接受（KNOWN_KEYS 扩展）。
    #[test]
    fn validate_config_accepts_telemetry_topic() {
        let plugin = MqttPlugin::new();
        let mut cfg = ProtocolConfig::default();
        cfg.insert("broker", "10.0.2.2:1883");
        cfg.insert("telemetry_topic", "eneros/dev-1/telemetry");
        assert_eq!(plugin.validate_config(&cfg), Ok(()));
    }

    /// 50：enqueue_telemetry_publish——未配置主题拒绝+计数；配置后入队；
    /// 超限拒绝（fail-closed）。
    #[test]
    fn enqueue_telemetry_publish_validates_and_counts() {
        let mut plugin = MqttPlugin::new();
        assert!(plugin.enqueue_telemetry_publish(b"OBSF").is_err());
        assert_eq!(plugin.telemetry_rejected(), 1);
        plugin.telemetry_topic = Some(alloc::string::String::from("eneros/dev-1/telemetry"));
        assert!(plugin.enqueue_telemetry_publish(b"OBSF\x01").is_ok());
        assert_eq!(plugin.app_outbox_len(), 1);
        assert_eq!(plugin.telemetry_topic(), Some("eneros/dev-1/telemetry"));
        // 超长拒绝 + 计数（不入队）。
        let big = alloc::vec![0u8; APP_MSG_MAX_LEN + 1];
        assert!(plugin.enqueue_telemetry_publish(&big).is_err());
        assert_eq!(plugin.telemetry_rejected(), 2);
        assert_eq!(plugin.app_outbox_len(), 1);
    }

    /// 50（WP-50.1 DoD② 组件级证明）：断连→重连→恢复全周期——
    /// 半开 TCP（is_connected 恒真）下 keepalive PING 无应答触发断连标记，
    /// 下一次 poll 走 connect 一律重建 seam（reconnect 计数 +1），
    /// 存量遥测帧恢复发布。
    ///
    /// SPEC-GAP-CLOSURE w1 转正登记：检测钩子已于 client.poll 复产
    /// （auto-PING 无应答 → mark_disconnected；裁剪版，CONNECT 握手期
    /// 不经此路径，规避 50 卷回退时登记的启动期挂起交互）。组件级全周期
    /// 通过；QEMU 窗口下的节拍复验仍属本 spec WP-1 C4/C5 卷判据。
    #[test]
    fn disconnect_reconnect_cycle_recovers_publishing() {
        use std::rc::Rc;

        #[derive(Default)]
        struct CycleTransport {
            alive: Rc<core::cell::Cell<bool>>,
            reconnects: Rc<core::cell::Cell<u32>>,
            last: core::cell::RefCell<alloc::vec::Vec<u8>>,
        }
        impl TransportHandle for CycleTransport {
            fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
                if !self.alive.get() {
                    return Err(TransportError::Closed);
                }
                *self.last.borrow_mut() = data.to_vec();
                Ok(())
            }
            fn recv(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
                if !self.alive.get() {
                    return Err(TransportError::Closed);
                }
                let data = self.last.borrow().clone();
                let reply: alloc::vec::Vec<u8> = match data.first() {
                    Some(&0x10) => alloc::vec![0x20, 0x02, 0x00, 0x00], // CONNECT→CONNACK
                    Some(&0xC0) => alloc::vec![0xD0, 0x00],             // PINGREQ→PINGRESP
                    // PUBLISH（QoS1 头 0x32）→ PUBACK 回显 packet id。
                    Some(b) if b & 0xF0 == 0x30 => {
                        let tlen = usize::from(data[2]) * 256 + usize::from(data[3]);
                        let off = 4 + tlen;
                        let pid = data
                            .get(off..off + 2)
                            .map(|w| [w[0], w[1]])
                            .unwrap_or([0, 1]);
                        alloc::vec![0x40, 0x02, pid[0], pid[1]]
                    }
                    _ => alloc::vec![],
                };
                let n = reply.len().min(buf.len());
                buf[..n].copy_from_slice(&reply[..n]);
                Ok(n)
            }
            fn close(&mut self) -> Result<(), TransportError> {
                Ok(())
            }
            fn is_connected(&self) -> bool {
                true // 半开 TCP：恒真（slirp 不传播 host 侧关闭）
            }
            fn reconnect(&mut self, _peer: &str) -> Result<(), TransportError> {
                self.alive.set(true);
                self.reconnects.set(self.reconnects.get() + 1);
                Ok(())
            }
            fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
                self
            }
        }

        struct CycleFactory {
            transport: Rc<core::cell::RefCell<CycleTransport>>,
        }
        impl TransportFactory for CycleFactory {
            fn open(
                &mut self,
                _spec: &TransportSpec,
            ) -> Result<Box<dyn TransportHandle>, TransportError> {
                Ok(Box::new(CycleTransport {
                    alive: self.transport.borrow().alive.clone(),
                    reconnects: self.transport.borrow().reconnects.clone(),
                    last: core::cell::RefCell::new(alloc::vec::Vec::new()),
                }))
            }
            fn supports(&self, kind: TransportKind) -> bool {
                kind == TransportKind::Tcp
            }
        }

        let alive = Rc::new(core::cell::Cell::new(true));
        let reconnects = Rc::new(core::cell::Cell::new(0));
        let mut factory = CycleFactory {
            transport: Rc::new(core::cell::RefCell::new(CycleTransport {
                alive: alive.clone(),
                reconnects: reconnects.clone(),
                last: core::cell::RefCell::new(alloc::vec::Vec::new()),
            })),
        };
        let mut plugin = MqttPlugin::new();
        let mut cfg = ProtocolConfig::default();
        cfg.insert("broker", "127.0.0.1:1883");
        cfg.insert("telemetry_topic", "eneros/dev/telemetry");
        // 短 keepalive：PING 无应答是半开 TCP 下唯一可靠的断连检测点。
        cfg.insert("keep_alive_ms", "100");
        plugin
            .start(&cfg, &mut factory, &mut EmptyPointRegistry)
            .expect("start");
        let budget = WorkBudget {
            max_work_ms: 50,
            max_polls: 1,
        };
        plugin.poll(1000, budget).expect("initial poll");
        assert_eq!(reconnects.get(), 1, "首次 connect 即重建（一律重建语义）");

        // 入队一帧遥测并发布成功。
        plugin
            .enqueue_telemetry_publish(b"OBSF-frame-1")
            .expect("enqueue");
        plugin.poll(2000, budget).expect("publish poll");
        assert_eq!(plugin.app_outbox_len(), 0, "在线期帧应发布");

        // 断网：alive=false（半开——is_connected 仍恒真）。连续 ≥3 次失败
        // 后必须标记断连；期间入队的帧滞留 outbox。
        alive.set(false);
        plugin
            .enqueue_telemetry_publish(b"OBSF-frame-2")
            .expect("enqueue");
        for t in 3000..3400 {
            let _ = plugin.poll(t, budget);
        }
        assert_eq!(reconnects.get(), 2, "断连后下一次 poll 必须走重连 seam");

        // 重连成功（模拟 broker 恢复 + TCP 重建）→ 存量帧补传。
        plugin.poll(5000, budget).expect("recovery poll");
        assert_eq!(plugin.app_outbox_len(), 0, "恢复后存量帧应补传发布");
        assert!(plugin.telemetry_published() >= 1, "发布计数应含补传");
    }

    /// 50：is_connected() 反映**底层传输真实状态**（socket 关闭 = 断连；
    /// 空闲不是断连）——svc-mqtt 边界判据据此 fail-closed。
    #[test]
    fn is_connected_reflects_transport_state() {
        let mut plugin = MqttPlugin::new();
        assert!(!plugin.is_connected(), "无 client → 未连接");
        let mut mock = crate::transport::MockTransport::new();
        mock.set_connected(true);
        let mut client = MqttClient::new("10.0.2.2:1883", "eneros-agent", 60_000);
        client.transport = Some(Box::new(mock));
        client.state = crate::client::ConnectionState::Connected;
        plugin.client = Some(client);
        assert!(plugin.is_connected(), "传输 connected → 真连接");
        // 传输断开（socket 关闭）→ is_connected 立即反映。
        if let Some(c) = &mut plugin.client {
            if let Some(t) = &mut c.transport {
                let _ = t.close();
            }
        }
        assert!(!plugin.is_connected(), "socket 关闭 → 断连");
    }

    /// 50（WP-50.2）：管理请求入收件箱（有界）→ drain；应答入 outbox
    /// （未配置应答主题拒绝——fail-closed）。
    #[test]
    fn mgmt_request_inbox_and_response_outbox() {
        let mut plugin = MqttPlugin::new();
        // 未配置请求主题 → 下行不路由进 inbox（on_downlink 语义）。
        assert!(plugin.mgmt_request_topic().is_none());
        assert!(plugin.enqueue_mgmt_response(b"resp").is_err());
        assert_eq!(plugin.mgmt_dropped(), 1);
        // 配置后：请求帧原样入箱（不解释内容）。
        plugin.mgmt_request_topic = Some(alloc::string::String::from("eneros/mgmt/request"));
        let frame = alloc::vec![b'M', b'G', b'T', b'1', 0x01];
        plugin.on_mgmt_request_inbox(frame.clone());
        let mut reqs = alloc::vec::Vec::new();
        plugin.drain_mgmt_requests(&mut reqs);
        assert_eq!(reqs, alloc::vec![frame]);
        // 应答主题配置后可发布。
        plugin.mgmt_response_topic = Some(alloc::string::String::from("eneros/mgmt/response"));
        assert!(plugin.enqueue_mgmt_response(b"\x01status:ok").is_ok());
        assert_eq!(plugin.app_outbox_len(), 1);
        assert_eq!(plugin.app_outbox[0].0, "eneros/mgmt/response");
    }
}
