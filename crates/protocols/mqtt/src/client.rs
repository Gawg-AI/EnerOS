//! MQTT 客户端状态机 + QoS 1/2 状态跟踪 + 指数退避重连.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::error::MqttError;
use crate::packet::{self, MqttPacket, PublishPacket, SubscribePacket, UnsubscribePacket};
use crate::qos::QoS;
#[cfg(not(any(feature = "fs-persist", test)))]
use crate::qos_store::{PendingMessage, QosStore};
#[cfg(any(feature = "fs-persist", test))]
use crate::qos_store::{PendingMessage, QosStore};
use crate::transport::MqttTransport;
use crate::will::LastWill;

/// 客户端连接状态.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// 已断开（初始状态或显式 disconnect）.
    Disconnected,
    /// 正在连接（已发送 CONNECT，等待 CONNACK）.
    Connecting,
    /// 已连接（CONNACK 接受）.
    Connected,
    /// 重连中（断线后指数退避重连）.
    Reconnecting,
}

/// 重连状态（指数退避）.
#[derive(Debug, Clone, Copy)]
pub struct ReconnectState {
    /// 已尝试重连次数.
    pub attempt_count: u32,
    /// 下次可重连的时间戳（ms）.
    pub next_retry_ms: u64,
    /// 当前退避时间（ms）.
    pub backoff_ms: u32,
}

impl ReconnectState {
    /// 初始值：attempt_count=0, next_retry_ms=0, backoff_ms=1000（D15：初始 1s）.
    pub fn new() -> Self {
        Self {
            attempt_count: 0,
            next_retry_ms: 0,
            backoff_ms: 1000,
        }
    }

    /// 计算下次退避：倍增 backoff_ms，封顶 30000（30s），自增 attempt_count，
    /// 设置 next_retry_ms = now + backoff_ms（参数为当前时间戳）.
    pub fn next_backoff(&mut self, now_ms: u64) -> u32 {
        self.attempt_count += 1;
        let current = self.backoff_ms;
        // 倍增并封顶 30s（D15）
        self.backoff_ms = (self.backoff_ms.saturating_mul(2)).min(30_000);
        self.next_retry_ms = now_ms.saturating_add(current as u64);
        current
    }

    /// 重置为初始状态（重连成功后调用）.
    pub fn reset(&mut self) {
        self.attempt_count = 0;
        self.next_retry_ms = 0;
        self.backoff_ms = 1000;
    }
}

impl Default for ReconnectState {
    fn default() -> Self {
        Self::new()
    }
}

/// QoS 1/2 在途消息状态.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingAck {
    /// QoS 1：等待 PUBACK.
    WaitingPuback,
    /// QoS 2 第 2 步：等待 PUBREC（收到后发 PUBREL）.
    WaitingPubrec { packet_id: u16 },
    /// QoS 2 第 4 步：等待 PUBCOMP.
    WaitingPubcomp { packet_id: u16 },
}

/// MQTT 客户端.
pub struct MqttClient {
    /// Broker 地址（host:port 形式或仅 host）.
    pub broker: String,
    /// 客户端 ID.
    pub client_id: String,
    /// 连接状态.
    pub state: ConnectionState,
    /// 已订阅列表（重连后恢复用）.
    pub subscriptions: Vec<(String, QoS)>,
    /// 遗嘱消息.
    pub last_will: Option<LastWill>,
    /// 心跳间隔（ms）.
    pub keep_alive_ms: u64,
    /// 46（WP-46.4 keepalive）：最近一次协议活动时间（ms；connect 成功 /
    /// 任意出站活动置位）——poll 据此在达到 keep_alive 阈值时自动 PINGREQ。
    pub last_activity_ms: u64,
    /// 传输层.
    pub transport: Option<Box<dyn MqttTransport>>,
    /// 重连状态.
    pub reconnect_state: ReconnectState,
    /// 下一个待分配的 QoS 1/2 packet ID.
    pub pending_publish_packet_id: u16,
    /// 在途 QoS 1/2 消息（packet_id → 状态）.
    pub pending_acks: BTreeMap<u16, PendingAck>,
    /// WP-53.4 切片 D：入站 QoS2 握手中途状态（已发 PUBREC、等 PUBREL）.
    inbound_qos2: BTreeMap<u16, ()>,
    /// 31 PR-12：in-flight 报文缓存（QoS1/2 超时重传用；ack 后移除）。
    inflight: BTreeMap<u16, PublishPacket>,
    /// QoS 1/2 在途消息持久化（v0.221.0；None = 不持久化）。
    pub qos_store: Option<Box<dyn QosStore>>,
    /// WP-53.4 切片 D：CONNECT clean_session 标志（默认 true = 旧行为；
    /// false = 持久会话——断线保留在途状态，CONNACK session_present 回读）。
    pub clean_session: bool,
    /// WP-53.4 切片 D：最近一次 CONNACK 的 session_present 位
    /// （clean_session=false 时 broker 据此恢复会话）。
    pub session_present: bool,
    /// SPEC-GAP-CLOSURE WP-3：CONNECT 凭据（None=匿名，与旧行为逐字节一致；
    /// 值经 [`Self::set_credentials`] 注入后随每次 CONNECT 编码发送）。
    /// 管道不做政策——release 下是否强制凭据属 owner 裁决（OQ-2）。
    pub credentials: Option<(String, Vec<u8>)>,
}

impl MqttClient {
    /// 构造客户端（初始状态 Disconnected，无传输层；clean_session 默认 true）.
    pub fn new(broker: &str, client_id: &str, keep_alive_ms: u64) -> Self {
        Self {
            broker: String::from(broker),
            client_id: String::from(client_id),
            state: ConnectionState::Disconnected,
            subscriptions: Vec::new(),
            last_will: None,
            keep_alive_ms,
            last_activity_ms: 0,
            transport: None,
            reconnect_state: ReconnectState::new(),
            pending_publish_packet_id: 1,
            pending_acks: BTreeMap::new(),
            inbound_qos2: BTreeMap::new(),
            inflight: BTreeMap::new(),
            qos_store: None,
            clean_session: true,
            session_present: false,
            credentials: None,
        }
    }

    /// 设置 CONNECT clean_session 标志（WP-53.4 切片 D；connect 前调用）.
    ///
    /// `false` = 持久会话：断线时保留 pending/inflight 状态以便重连续传，
    /// 且 CONNACK 的 `session_present` 会写入 [`Self::session_present`]。
    pub fn set_clean_session(&mut self, clean: bool) {
        self.clean_session = clean;
    }

    /// 设置 QoS 持久化存储（v0.221.0）。
    pub fn set_qos_store(&mut self, store: Box<dyn QosStore>) {
        self.qos_store = Some(store);
    }

    /// 启动时恢复在途消息（重连补传）。
    ///
    /// 返回恢复的未确认消息列表；调用方应逐条重新 publish（dup=true）。
    pub fn restore_pending(&mut self) -> Vec<PendingMessage> {
        match &mut self.qos_store {
            Some(store) => store.load_all(),
            None => Vec::new(),
        }
    }

    /// 设置遗嘱消息（connect 前调用）.
    pub fn set_will(&mut self, will: LastWill) {
        self.last_will = Some(will);
    }

    /// 设置 CONNECT 凭据（WP-3 管道层；connect 前调用，随重连持续生效）.
    ///
    /// MQTT 3.1.1 字符串/二进制字段长度前缀均为 u16：超长凭据在注入时即
    /// 拒绝（fail-closed），不等 wire 编码期静默截断。传空串/空切片合法
    /// （显式匿名语义），用 `credentials = None` 才回到旧编码路径。
    pub fn set_credentials(&mut self, username: &str, password: &[u8]) -> Result<(), MqttError> {
        const WIRE_U16_MAX: usize = 65_535;
        if username.len() > WIRE_U16_MAX || password.len() > WIRE_U16_MAX {
            return Err(MqttError::PacketEncode);
        }
        self.credentials = Some((String::from(username), Vec::from(password)));
        Ok(())
    }

    /// 注入传输层实现.
    pub fn set_transport(&mut self, transport: Box<dyn MqttTransport>) {
        self.transport = Some(transport);
    }

    /// 分配下一个 packet ID（1~65535 循环）.
    fn allocate_packet_id(&mut self) -> u16 {
        let id = self.pending_publish_packet_id;
        self.pending_publish_packet_id = self.pending_publish_packet_id.wrapping_add(1);
        if self.pending_publish_packet_id == 0 {
            self.pending_publish_packet_id = 1;
        }
        id
    }

    /// 建立连接：发送 CONNECT，等待 CONNACK.
    pub fn connect(&mut self, now_ms: u64) -> Result<(), MqttError> {
        let _ = now_ms;
        let transport = self.transport.as_mut().ok_or(MqttError::NotConnected)?;
        // 解析 broker 为 host:port（默认 1883）
        let (host, port) = parse_broker(&self.broker);
        transport
            .connect(&host, port)
            .map_err(|_| MqttError::BrokerUnreachable)?;
        self.state = ConnectionState::Connecting;
        // 构造 CONNECT 报文
        let keep_alive_secs = self.keep_alive_ms.div_ceil(1000) as u16;
        let connect = packet::ConnectPacket {
            client_id: self.client_id.clone(),
            username: self.credentials.as_ref().map(|(u, _)| u.clone()),
            password: self.credentials.as_ref().map(|(_, p)| p.clone()),
            will: self.last_will.clone(),
            keep_alive_secs,
            clean_session: self.clean_session,
        };
        let bytes = packet::encode(&MqttPacket::Connect(connect));
        // 31 PR-10：will payload 超限（编码失败）→ 拒绝连接（fail-closed，
        // 不发送损坏报文）。
        if bytes.is_empty() {
            // WP-1/C3（四个失败臂之一）：编码失败同样不得把对端观测面留在
            // Connecting 中间态——F-A1 卡死链在此的入口。
            self.state = ConnectionState::Disconnected;
            return Err(MqttError::PacketEncode);
        }
        // WP-1/F-A1 修复：send / recv / decode 三个失败臂此前只返回错误、
        // 不迁移状态，与 try_reconnect 的状态门（F-A2）叠加构成"首次握手
        // 失败后永不重连"死锁链；现统一打回 Disconnected 交上层编排恢复。
        if transport.send(&bytes).is_err() {
            self.state = ConnectionState::Disconnected;
            return Err(MqttError::TransportError);
        }
        // 等待 CONNACK（同步 poll 一次）
        let connack_bytes = match transport.recv() {
            Ok(b) => b,
            Err(_) => {
                self.state = ConnectionState::Disconnected;
                return Err(MqttError::BrokerUnreachable);
            }
        };
        let pkt = match packet::decode(&connack_bytes) {
            Ok(p) => p,
            Err(_) => {
                self.state = ConnectionState::Disconnected;
                return Err(MqttError::PacketDecodeError);
            }
        };
        match pkt {
            MqttPacket::Connack(c) => {
                if c.return_code == 0 {
                    self.state = ConnectionState::Connected;
                    // WP-53.4 切片 D：消费 session_present 位（此前解析后丢弃）。
                    self.session_present = c.session_present;
                    self.last_activity_ms = now_ms;
                    Ok(())
                } else {
                    self.state = ConnectionState::Disconnected;
                    Err(MqttError::BrokerUnreachable)
                }
            }
            _ => {
                self.state = ConnectionState::Disconnected;
                Err(MqttError::UnexpectedPacket)
            }
        }
    }

    /// 发布消息（QoS 0/1/2）.
    pub fn publish(
        &mut self,
        topic: &str,
        payload: &[u8],
        qos: QoS,
        now_ms: u64,
    ) -> Result<(), MqttError> {
        if self.state != ConnectionState::Connected {
            return Err(MqttError::NotConnected);
        }
        // 先分配 packet_id（避免与 transport 借用冲突）
        let (packet_id, pending) = match qos {
            QoS::AtMostOnce => (None, None),
            QoS::AtLeastOnce => {
                let id = self.allocate_packet_id();
                (Some(id), Some(PendingAck::WaitingPuback))
            }
            QoS::ExactlyOnce => {
                let id = self.allocate_packet_id();
                (Some(id), Some(PendingAck::WaitingPubrec { packet_id: id }))
            }
        };
        let publish = PublishPacket {
            topic: String::from(topic),
            packet_id,
            qos,
            retain: false,
            dup: false,
            payload: Vec::from(payload),
        };
        // 31 PR-12：in-flight 缓存（重传用）。
        if let Some(id) = packet_id {
            self.inflight.insert(id, publish.clone());
        }
        let bytes = packet::encode(&MqttPacket::Publish(publish));
        // v0.221.0：QoS1/2 发送前持久化（断网/重启可补传）。
        if let (Some(id), Some(state)) = (packet_id, pending) {
            if let Some(store) = &mut self.qos_store {
                let pm = PendingMessage {
                    packet_id: id,
                    topic: String::from(topic),
                    payload: payload.to_vec(),
                    qos: qos.as_u8(),
                    state,
                };
                let _ = store.save(&pm);
            }
        }
        {
            let transport = self.transport.as_mut().ok_or(MqttError::NotConnected)?;
            transport
                .send(&bytes)
                .map_err(|_| MqttError::TransportError)?;
        }
        let _ = now_ms;
        match pending {
            None => Ok(()),
            Some(PendingAck::WaitingPuback) => {
                let pid = packet_id.unwrap_or(0);
                self.pending_acks.insert(pid, PendingAck::WaitingPuback);
                // 同步等待 PUBACK。31-review：失败（超时/错包/断连）→ 清 inflight，
                // 防 u16 回绕后 id 复用、DUP 重发陈旧 payload（旧实现所有错误路径泄漏）。
                if let Err(e) = self.wait_for_ack(pid, now_ms) {
                    self.inflight.remove(&pid);
                    return Err(e);
                }
                Ok(())
            }
            Some(PendingAck::WaitingPubrec { packet_id: pid }) => {
                self.pending_acks
                    .insert(pid, PendingAck::WaitingPubrec { packet_id: pid });
                // QoS 2：等待 PUBREC → 发 PUBREL → 等待 PUBCOMP（同上错误清理）。
                if let Err(e) = self.wait_for_qos2(pid, now_ms) {
                    self.inflight.remove(&pid);
                    return Err(e);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// 测试访问：传输层（downcast 用）。
    #[cfg(test)]
    pub fn transport_mut(&mut self) -> &mut dyn crate::transport::MqttTransport {
        self.transport.as_mut().expect("transport").as_mut()
    }

    /// 等待 PUBACK（QoS 1）.
    ///
    /// 31 PR-12：transport recv 超时（PublishTimeout）→ DUP=1 同 packet id
    /// 重发（MQTT 3.1.1 §4.4 超时重传），最多 2 次；仍超时 → Err。
    fn wait_for_ack(&mut self, packet_id: u16, _now_ms: u64) -> Result<(), MqttError> {
        for attempt in 0..=2 {
            let transport = self.transport.as_mut().ok_or(MqttError::NotConnected)?;
            let bytes = match transport.recv() {
                Ok(b) => b,
                // 31 PR-12：recv 错误（超时/无数据）→ DUP 重传（≤2 次）。
                Err(_) if attempt < 2 => {
                    // 重发（DUP=1，同 packet id）。
                    let mut publish = self
                        .inflight
                        .get(&packet_id)
                        .cloned()
                        .ok_or(MqttError::PublishTimeout)?;
                    publish.dup = true;
                    let bytes = packet::encode(&MqttPacket::Publish(publish));
                    self.transport
                        .as_mut()
                        .ok_or(MqttError::NotConnected)?
                        .send(&bytes)
                        .map_err(|_| MqttError::TransportError)?;
                    continue;
                }
                Err(e) => return Err(e),
            };
            let pkt = packet::decode(&bytes).map_err(|_| MqttError::PacketDecodeError)?;
            match pkt {
                MqttPacket::Puback(id) if id == packet_id => {
                    self.pending_acks.remove(&packet_id);
                    self.inflight.remove(&packet_id);
                    // v0.221.0：确认后清除持久化记录。
                    if let Some(store) = &mut self.qos_store {
                        let _ = store.remove(packet_id);
                    }
                    return Ok(());
                }
                _ => return Err(MqttError::UnexpectedPacket),
            }
        }
        Err(MqttError::PublishTimeout)
    }

    /// QoS 2 四次握手：等待 PUBREC → 发 PUBREL → 等待 PUBCOMP.
    fn wait_for_qos2(&mut self, packet_id: u16, _now_ms: u64) -> Result<(), MqttError> {
        // 步骤 1：等待 PUBREC
        let transport = self.transport.as_mut().ok_or(MqttError::NotConnected)?;
        let bytes = transport.recv().map_err(|_| MqttError::PublishTimeout)?;
        let pkt = packet::decode(&bytes).map_err(|_| MqttError::PacketDecodeError)?;
        match pkt {
            MqttPacket::Pubrec(id) if id == packet_id => {}
            _ => return Err(MqttError::UnexpectedPacket),
        }
        // 更新状态为 WaitingPubcomp
        self.pending_acks
            .insert(packet_id, PendingAck::WaitingPubcomp { packet_id });
        // 步骤 2：发送 PUBREL
        let pubrel_bytes = packet::encode(&MqttPacket::Pubrel(packet_id));
        transport
            .send(&pubrel_bytes)
            .map_err(|_| MqttError::TransportError)?;
        // 步骤 3：等待 PUBCOMP
        let bytes = transport.recv().map_err(|_| MqttError::PublishTimeout)?;
        let pkt = packet::decode(&bytes).map_err(|_| MqttError::PacketDecodeError)?;
        match pkt {
            MqttPacket::Pubcomp(id) if id == packet_id => {
                self.pending_acks.remove(&packet_id);
                // 31-review：QoS2 成功路径漏清 inflight（旧实现永不删 → 每条 QoS2 publish 泄漏）。
                self.inflight.remove(&packet_id);
                // v0.221.0：确认后清除持久化记录。
                if let Some(store) = &mut self.qos_store {
                    let _ = store.remove(packet_id);
                }
                Ok(())
            }
            _ => Err(MqttError::UnexpectedPacket),
        }
    }

    /// 订阅 Topic（QoS 1 报文 ID 分配，等待 SUBACK）.
    pub fn subscribe(&mut self, topic: &str, qos: QoS) -> Result<(), MqttError> {
        if self.state != ConnectionState::Connected {
            return Err(MqttError::NotConnected);
        }
        // 加入订阅列表（重连恢复用）——同 topic 去重（WP-53.4 切片 D：
        // 旧实现 push 累积，重连恢复订阅时列表翻倍且重复发送订阅报文）。
        match self.subscriptions.iter_mut().find(|(t, _)| t == topic) {
            Some(entry) => entry.1 = qos,
            None => self.subscriptions.push((String::from(topic), qos)),
        }
        // 先分配 packet_id（避免与 transport 借用冲突）
        let packet_id = self.allocate_packet_id();
        let sub = SubscribePacket {
            packet_id,
            topics: vec![(String::from(topic), qos)],
        };
        let bytes = packet::encode(&MqttPacket::Subscribe(sub));
        {
            let transport = self.transport.as_mut().ok_or(MqttError::NotConnected)?;
            transport
                .send(&bytes)
                .map_err(|_| MqttError::SubscribeFailed)?;
        }
        // 等待 SUBACK
        let transport = self.transport.as_mut().ok_or(MqttError::NotConnected)?;
        let bytes = transport.recv().map_err(|_| MqttError::SubscribeFailed)?;
        let pkt = packet::decode(&bytes).map_err(|_| MqttError::PacketDecodeError)?;
        match pkt {
            MqttPacket::Suback(s) if s.packet_id == packet_id => {
                // 任一 return_code >= 128 表示失败
                if s.return_codes.iter().any(|&c| c >= 128) {
                    return Err(MqttError::SubscribeFailed);
                }
                Ok(())
            }
            _ => Err(MqttError::UnexpectedPacket),
        }
    }

    /// 取消订阅.
    pub fn unsubscribe(&mut self, topic: &str) -> Result<(), MqttError> {
        if self.state != ConnectionState::Connected {
            return Err(MqttError::NotConnected);
        }
        // 从订阅列表移除
        self.subscriptions.retain(|(t, _)| t != topic);
        // 先分配 packet_id（避免与 transport 借用冲突）
        let packet_id = self.allocate_packet_id();
        let unsub = UnsubscribePacket {
            packet_id,
            topics: vec![String::from(topic)],
        };
        let bytes = packet::encode(&MqttPacket::Unsubscribe(unsub));
        {
            let transport = self.transport.as_mut().ok_or(MqttError::NotConnected)?;
            transport
                .send(&bytes)
                .map_err(|_| MqttError::UnsubscribeFailed)?;
        }
        // 等待 UNSUBACK
        let transport = self.transport.as_mut().ok_or(MqttError::NotConnected)?;
        let bytes = transport.recv().map_err(|_| MqttError::UnsubscribeFailed)?;
        let pkt = packet::decode(&bytes).map_err(|_| MqttError::PacketDecodeError)?;
        match pkt {
            MqttPacket::Unsuback(id) if id == packet_id => Ok(()),
            _ => Err(MqttError::UnexpectedPacket),
        }
    }

    /// 心跳：发送 PINGREQ，等待 PINGRESP.
    pub fn ping(&mut self, _now_ms: u64) -> Result<(), MqttError> {
        if self.state != ConnectionState::Connected {
            return Err(MqttError::NotConnected);
        }
        let transport = self.transport.as_mut().ok_or(MqttError::NotConnected)?;
        let bytes = packet::encode(&MqttPacket::Pingreq);
        transport
            .send(&bytes)
            .map_err(|_| MqttError::TransportError)?;
        let resp = transport.recv().map_err(|_| MqttError::PublishTimeout)?;
        let pkt = packet::decode(&resp).map_err(|_| MqttError::PacketDecodeError)?;
        match pkt {
            MqttPacket::Pingresp => Ok(()),
            _ => Err(MqttError::UnexpectedPacket),
        }
    }

    /// 主动断开：发送 DISCONNECT，关闭传输层.
    pub fn disconnect(&mut self) -> Result<(), MqttError> {
        if let Some(transport) = self.transport.as_mut() {
            if self.state == ConnectionState::Connected {
                let bytes = packet::encode(&MqttPacket::Disconnect);
                let _ = transport.send(&bytes);
            }
            let _ = transport.close();
        }
        self.state = ConnectionState::Disconnected;
        // WP-53.4 切片 D：clean_session=false（持久会话）时保留在途状态，
        // 重连后可续传；true 维持旧行为（清空防 id 复用错发）。
        if self.clean_session {
            self.pending_acks.clear();
            self.inflight.clear();
            self.inbound_qos2.clear();
        }
        Ok(())
    }

    /// 轮询入站报文（非阻塞：无数据返回空 Vec）.
    ///
    /// 处理 PUBACK/PUBREC/PUBCOMP（移除 pending_acks），返回入站 PUBLISH 列表。
    pub fn poll(&mut self, now_ms: u64) -> Result<Vec<MqttPacket>, MqttError> {
        // 46（WP-46.4 keepalive）：空闲 ≥ keep_alive_ms 时自动 PINGREQ（有界
        // 同步等待 PINGRESP——与 ping() 同语义；now_ms 被真实消费，不再忽略）。
        if self.state == ConnectionState::Connected {
            if now_ms.saturating_sub(self.last_activity_ms) >= self.keep_alive_ms {
                // WP-1/F-A3 复产（50 卷回退登记的裁剪版）：自动保活 PING
                // 无应答是半开 TCP 下唯一可靠断连信号——此处标记断连交由
                // 上层编排恢复（plugin 惰性 connect 分支）。首连期 CONNECT
                // 握手不经此路径，无 50 卷启动期挂起交互；空闲 recv 超时
                // 仍不视作断连（语义不变）。
                if let Err(e) = self.ping(now_ms) {
                    self.mark_disconnected();
                    return Err(e);
                }
                self.last_activity_ms = now_ms;
            }
        } else {
            self.last_activity_ms = now_ms;
        }
        let transport = self.transport.as_mut().ok_or(MqttError::NotConnected)?;
        let bytes = match transport.recv() {
            Ok(b) => b,
            Err(MqttError::NotConnected) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let pkt = packet::decode(&bytes).map_err(|_| MqttError::PacketDecodeError)?;
        match pkt {
            MqttPacket::Puback(id) => {
                self.pending_acks.remove(&id);
                Ok(Vec::new())
            }
            MqttPacket::Pubrec(id) => {
                // 收到 PUBREC → 发 PUBREL
                self.pending_acks
                    .insert(id, PendingAck::WaitingPubcomp { packet_id: id });
                let pubrel_bytes = packet::encode(&MqttPacket::Pubrel(id));
                transport
                    .send(&pubrel_bytes)
                    .map_err(|_| MqttError::TransportError)?;
                Ok(Vec::new())
            }
            MqttPacket::Pubcomp(id) => {
                self.pending_acks.remove(&id);
                Ok(Vec::new())
            }
            // WP-53.4 切片 D：入站 PUBREL（broker→client 的 QoS2 第 3 步）
            // 必须回 PUBCOMP——旧实现落入 `_` 分支被静默吞掉，握手悬挂。
            MqttPacket::Pubrel(id) => {
                let comp = packet::encode(&MqttPacket::Pubcomp(id));
                transport
                    .send(&comp)
                    .map_err(|_| MqttError::TransportError)?;
                self.inbound_qos2.remove(&id);
                Ok(Vec::new())
            }
            MqttPacket::Publish(p) => {
                // WP-53.4 切片 D：入站 PUBLISH 必须按 QoS 确认
                // （MQTT 3.1.1 §4.3；旧实现原样透传不回 ACK，
                // QoS1 下 broker 无限重发同一消息）。
                match p.qos {
                    QoS::AtMostOnce => {}
                    QoS::AtLeastOnce => {
                        if let Some(id) = p.packet_id {
                            let ack = packet::encode(&MqttPacket::Puback(id));
                            transport
                                .send(&ack)
                                .map_err(|_| MqttError::TransportError)?;
                        }
                    }
                    QoS::ExactlyOnce => {
                        if let Some(id) = p.packet_id {
                            let rec = packet::encode(&MqttPacket::Pubrec(id));
                            transport
                                .send(&rec)
                                .map_err(|_| MqttError::TransportError)?;
                            self.inbound_qos2.insert(id, ());
                        }
                    }
                }
                let v = vec![MqttPacket::Publish(p)];
                Ok(v)
            }
            _ => Ok(Vec::new()),
        }
    }

    /// 触发重连（指数退避，D15：初始 1s，最大 30s）.
    ///
    /// - 若状态非 Reconnecting/Disconnected：返回 Ok（无需重连）
    /// - 若 now_ms < next_retry_ms：返回 Err(NotConnected)（仍在退避中）
    /// - 否则调用 connect() 重连
    ///   - 成功：恢复所有订阅，重置 reconnect_state
    ///   - 失败：调用 next_backoff()，返回 Err
    pub fn try_reconnect(&mut self, now_ms: u64) -> Result<(), MqttError> {
        if self.state != ConnectionState::Reconnecting
            && self.state != ConnectionState::Disconnected
        {
            return Ok(());
        }
        if now_ms < self.reconnect_state.next_retry_ms {
            return Err(MqttError::NotConnected);
        }
        self.state = ConnectionState::Reconnecting;
        match self.connect(now_ms) {
            Ok(()) => {
                // 恢复订阅
                let subs = self.subscriptions.clone();
                for (topic, qos) in subs {
                    let _ = self.subscribe(&topic, qos);
                }
                self.reconnect_state.reset();
                Ok(())
            }
            Err(e) => {
                self.reconnect_state.next_backoff(now_ms);
                self.state = ConnectionState::Reconnecting;
                Err(e)
            }
        }
    }

    /// 标记断线（外部检测到传输断开时调用）.
    pub fn mark_disconnected(&mut self) {
        self.state = ConnectionState::Reconnecting;
        // WP-53.4 切片 D：持久会话保留在途状态（同 disconnect 语义）。
        if self.clean_session {
            self.pending_acks.clear();
            self.inflight.clear();
            self.inbound_qos2.clear();
        }
    }

    /// 返回当前在途 QoS 1/2 消息数.
    pub fn pending_count(&self) -> usize {
        self.pending_acks.len()
    }
}

/// 解析 broker 字符串为 (host, port).
///
/// 支持形式：
/// - "host" → ("host", 1883)
/// - "host:port" → ("host", port)
fn parse_broker(broker: &str) -> (String, u16) {
    if let Some(idx) = broker.rfind(':') {
        let host = &broker[..idx];
        let port_str = &broker[idx + 1..];
        if let Ok(port) = port_str.parse::<u16>() {
            return (String::from(host), port);
        }
    }
    (String::from(broker), 1883)
}

#[cfg(test)]
mod tests {
    use crate::transport::MockTransport;

    /// 31 PR-12 判据：mock 不返回 PUBACK（recv 空 → PublishTimeout）→
    /// DUP=1 同 packet id 重发（MQTT 3.1.1 §4.4）。
    #[test]
    fn test_qos1_timeout_retransmits_dup() {
        let mut transport = MockTransport::new();
        transport.set_connected(true);
        let mut client = crate::client::MqttClient::new("eneros-test", "client-1", 30);
        client.set_transport(Box::new(transport));
        // 直接构造 Connected 状态发布。
        client.state = crate::client::ConnectionState::Connected;
        // mock recv 空队列 → PublishTimeout → DUP 重传路径触发。
        let result = client.publish("t", b"payload", crate::qos::QoS::AtLeastOnce, 0);
        let _ = result;
        let sent = client
            .transport_mut()
            .as_any_mut()
            .downcast_ref::<MockTransport>()
            .expect("downcast")
            .sent_packets()
            .to_vec();
        let dup_frames = sent
            .iter()
            .filter(|b| b.first() == Some(&0x3A)) // PUBLISH QoS1 DUP=1 (0b00111010)
            .count();
        assert!(dup_frames >= 1, "超时后必须 DUP=1 重发（31 PR-12 判据）");
    }

    use super::*;

    #[test]
    fn test_reconnect_state_initial() {
        let s = ReconnectState::new();
        assert_eq!(s.attempt_count, 0);
        assert_eq!(s.next_retry_ms, 0);
        assert_eq!(s.backoff_ms, 1000);
    }

    #[test]
    fn test_reconnect_state_backoff_doubles() {
        let mut s = ReconnectState::new();
        let b1 = s.next_backoff(0);
        assert_eq!(b1, 1000);
        assert_eq!(s.attempt_count, 1);
        assert_eq!(s.next_retry_ms, 1000);
        assert_eq!(s.backoff_ms, 2000);

        let b2 = s.next_backoff(1000);
        assert_eq!(b2, 2000);
        assert_eq!(s.attempt_count, 2);
        assert_eq!(s.next_retry_ms, 3000);
        assert_eq!(s.backoff_ms, 4000);
    }

    #[test]
    fn test_reconnect_state_backoff_cap() {
        let mut s = ReconnectState::new();
        s.backoff_ms = 16_000;
        let b = s.next_backoff(0);
        assert_eq!(b, 16_000);
        assert_eq!(s.backoff_ms, 30_000); // 封顶 30s
    }

    #[test]
    fn test_parse_broker() {
        assert_eq!(parse_broker("localhost"), (String::from("localhost"), 1883));
        assert_eq!(
            parse_broker("broker.example.com:8883"),
            (String::from("broker.example.com"), 8883)
        );
        assert_eq!(
            parse_broker("192.168.1.1:1883"),
            (String::from("192.168.1.1"), 1883)
        );
    }

    #[test]
    fn test_allocate_packet_id_wraps() {
        let mut c = MqttClient::new("localhost", "id", 60_000);
        assert_eq!(c.allocate_packet_id(), 1);
        assert_eq!(c.allocate_packet_id(), 2);
        c.pending_publish_packet_id = 65535;
        assert_eq!(c.allocate_packet_id(), 65535);
        assert_eq!(c.allocate_packet_id(), 1); // 回绕
    }

    // ===== 39 §6.3 负例：流量突发/backpressure + 坏报文突发 =====

    /// 构造已连接状态的客户端（mock 预置 PUBACK 可选）。
    fn connected_client(transport: MockTransport) -> MqttClient {
        let mut client = MqttClient::new("eneros-test", "client-1", 60_000);
        client.set_transport(Box::new(transport));
        client.state = ConnectionState::Connected;
        client
    }

    /// 读取 mock 已发送报文数（downcast）。
    fn sent_count(client: &mut MqttClient) -> usize {
        client
            .transport_mut()
            .as_any_mut()
            .downcast_ref::<MockTransport>()
            .expect("downcast")
            .sent_count()
    }

    /// 39 收尾复验（§6.3 流量突发/backpressure）：客户端为同步 publish
    /// 模型（window=1，无 inflight 窗口上限）——实际边界即"每次 publish
    /// 同步等 ACK"。突发三条 QoS1 publish（broker 逐条回 PUBACK）全部
    /// 确认、无积压、不 panic；ACK 缺失时不静默丢：DUP 重传 ≤2 次后
    /// 返回 PublishTimeout，消息保留在 pending_acks（at-least-once 语义，
    /// 可由调用方补传），而非静默吞掉。
    #[test]
    fn test_39_qos1_burst_all_acked_and_missing_ack_not_silently_dropped() {
        // 突发路径：三条 publish 各自预置 PUBACK（pid 1/2/3）。
        let mut transport = MockTransport::new();
        transport.set_connected(true);
        for pid in 1u16..=3 {
            transport.enqueue_recv(vec![0x40, 0x02, (pid >> 8) as u8, pid as u8]);
        }
        let mut client = connected_client(transport);
        for i in 0..3u8 {
            client
                .publish("t", &[i], crate::qos::QoS::AtLeastOnce, 0)
                .expect("突发 publish 必须逐条确认");
        }
        assert_eq!(client.pending_count(), 0, "全部 ACK 后无积压");
        assert_eq!(sent_count(&mut client), 3);

        // 无 ACK 路径：不静默丢——DUP 重传后 Err，消息仍在途表。
        let mut transport = MockTransport::new();
        transport.set_connected(true);
        let mut client = connected_client(transport);
        let result = client.publish("t", b"x", crate::qos::QoS::AtLeastOnce, 0);
        // 不静默成功：无 ACK 时必须报错（PublishTimeout，或 mock 空队列
        // 语义下第三次 recv 的 NotConnected——两者均为显式失败路径）。
        assert!(
            result == Err(MqttError::PublishTimeout) || result == Err(MqttError::NotConnected),
            "无 ACK 不得静默成功: {result:?}"
        );
        // 首发 1 + DUP 重传 2 = 3 帧（31 PR-12 重传语义，非静默丢）。
        assert_eq!(sent_count(&mut client), 3);
        // 未确认消息保留在 pending_acks（不得静默吞 ACK 语义）。
        assert_eq!(client.pending_count(), 1);
    }

    /// 39 收尾复验（§6.3 坏报文突发）：一次 recv 注入超长垃圾字节流
    ///（远超典型报文），解码路径拒绝（PacketDecodeError）且不 panic；
    /// 客户端状态保持可继续运行。
    #[test]
    fn test_39_garbage_byte_burst_rejected_without_panic() {
        let mut transport = MockTransport::new();
        transport.set_connected(true);
        // 超长垃圾流：非法报文类型 + 剩余长度编码损坏。
        let garbage = vec![0xFFu8; 16_384];
        transport.enqueue_recv(garbage);
        // 再注一段伪 PUBLISH（剩余长度声称远超实际）。
        transport.enqueue_recv(vec![0x30, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]);
        let mut client = connected_client(transport);
        let r1 = client.poll(0);
        assert_eq!(r1, Err(MqttError::PacketDecodeError));
        let r2 = client.poll(0);
        assert_eq!(r2, Err(MqttError::PacketDecodeError));
        // 解码拒绝后状态不损坏：队列排空后 poll 返回空（不 panic）。
        let r3 = client.poll(0);
        assert!(r3.is_ok());
        assert!(r3.unwrap().is_empty());
    }

    // ===== WP-53.4 切片 D：入站 PUBLISH 确认 / 持久会话 / 订阅去重 =====

    use crate::qos::QoS;

    /// 构造入站 PUBLISH 报文字节.
    fn inbound_publish(qos: QoS, pid: u16) -> Vec<u8> {
        packet::encode(&MqttPacket::Publish(PublishPacket {
            topic: String::from("inbound"),
            packet_id: if qos == QoS::AtMostOnce {
                None
            } else {
                Some(pid)
            },
            qos,
            retain: false,
            dup: false,
            payload: vec![b'x'],
        }))
    }

    /// WP-53.4：QoS0 入站不回任何 ACK。
    #[test]
    fn inbound_qos0_publish_needs_no_ack() {
        let mut transport = MockTransport::new();
        transport.set_connected(true);
        transport.enqueue_recv(inbound_publish(QoS::AtMostOnce, 0));
        let mut client = connected_client(transport);
        let got = client.poll(0).expect("poll");
        assert_eq!(got.len(), 1);
        assert_eq!(sent_count(&mut client), 0, "QoS0 入站不得回 ACK");
    }

    /// WP-53.4：QoS1 入站必须回 PUBACK（旧实现透传不确认，
    /// broker 会按重传语义无限重发同一消息）。
    #[test]
    fn inbound_qos1_publish_gets_puback() {
        let mut transport = MockTransport::new();
        transport.set_connected(true);
        transport.enqueue_recv(inbound_publish(QoS::AtLeastOnce, 7));
        let mut client = connected_client(transport);
        let got = client.poll(0).expect("poll");
        assert_eq!(got.len(), 1);
        let sent = {
            let t = client
                .transport_mut()
                .as_any_mut()
                .downcast_ref::<MockTransport>()
                .expect("downcast");
            t.sent_packets().to_vec()
        };
        assert_eq!(
            sent,
            vec![vec![0x40, 0x02, 0x00, 0x07]],
            "必须回 PUBACK(packet_id=7)：固定头 0x40"
        );
    }

    /// WP-53.4：QoS2 入站完整握手 PUBLISH→PUBREC，PUBREL→PUBCOMP。
    #[test]
    fn inbound_qos2_full_handshake() {
        let mut transport = MockTransport::new();
        transport.set_connected(true);
        transport.enqueue_recv(inbound_publish(QoS::ExactlyOnce, 9));
        let mut client = connected_client(transport);

        // 第一步：PUBLISH → 回 PUBREC
        let got = client.poll(0).expect("poll step1");
        assert_eq!(got.len(), 1);
        {
            let t = client
                .transport_mut()
                .as_any_mut()
                .downcast_ref::<MockTransport>()
                .expect("downcast");
            let sent = t.sent_packets();
            assert_eq!(
                sent.last().map(|b| b.as_slice()),
                Some(&[0x50, 0x02, 0x00, 0x09][..]),
                "必须回 PUBREC(packet_id=9)：固定头 0x50"
            );
        }

        // 第二步：broker 发 PUBREL → 必须回 PUBCOMP（旧实现静默吞掉）
        {
            let t = client
                .transport_mut()
                .as_any_mut()
                .downcast_mut::<MockTransport>()
                .expect("downcast");
            t.enqueue_recv(vec![0x62, 0x02, 0x00, 0x09]); // PUBREL 固定头 0x62
        }
        let _ = client.poll(0).expect("poll step2");
        let t = client
            .transport_mut()
            .as_any_mut()
            .downcast_ref::<MockTransport>()
            .expect("downcast");
        let sent = t.sent_packets();
        assert_eq!(
            sent.last().map(|b| b.as_slice()),
            Some(&[0x70, 0x02, 0x00, 0x09][..]),
            "收到 PUBREL 必须回 PUBCOMP(packet_id=9)：固定头 0x70"
        );
    }

    /// WP-53.4：clean_session=false 持久会话——断线保留在途状态；
    /// 默认 true 维持旧行为（清空）。
    #[test]
    fn clean_session_flag_controls_state_retention() {
        // 默认 true：断线清空（旧行为不变）。
        let mut c = MqttClient::new("b", "id", 60_000);
        c.pending_acks.insert(1, PendingAck::WaitingPuback);
        c.mark_disconnected();
        assert_eq!(c.pending_count(), 0, "clean_session=true 断线清空");

        // false：断线保留 pending_acks/inflight。
        let mut p = MqttClient::new("b", "id", 60_000);
        p.clean_session = false;
        p.pending_acks.insert(1, PendingAck::WaitingPuback);
        p.mark_disconnected();
        assert_eq!(p.pending_count(), 1, "持久会话断线必须保留在途状态");
    }

    /// WP-53.4：session_present 位从 CONNACK 消费（旧实现解析后丢弃）。
    #[test]
    fn session_present_captured_from_connack() {
        let mut transport = MockTransport::new();
        transport.set_connected(true);
        let connack = packet::encode(&MqttPacket::Connack(crate::packet::ConnackPacket {
            session_present: true,
            return_code: 0,
        }));
        transport.enqueue_recv(connack);
        let mut client = MqttClient::new("eneros-test", "sess-1", 60_000);
        client.clean_session = false;
        client.set_transport(Box::new(transport));
        client.connect(0).expect("connect");
        assert!(client.session_present, "CONNACK session_present 必须回读");
    }

    /// WP-53.4：同 topic 重复订阅去重——订阅列表长度恒定，
    /// 重连恢复后不再翻倍。
    #[test]
    fn subscribe_dedup_keeps_list_constant_across_reconnects() {
        let mut client = MqttClient::new("eneros-test", "dedup-1", 60_000);
        let mut transport = MockTransport::new();
        transport.set_connected(true);

        // 辅助：向客户端内嵌 mock 入队一段应答。
        fn feed(client: &mut MqttClient, bytes: Vec<u8>) {
            client
                .transport_mut()
                .as_any_mut()
                .downcast_mut::<MockTransport>()
                .expect("downcast")
                .enqueue_recv(bytes);
        }

        // 初始连接（消费 1 条 CONNACK）。
        client.set_transport(Box::new(transport));
        feed(
            &mut client,
            packet::encode(&MqttPacket::Connack(crate::packet::ConnackPacket {
                session_present: false,
                return_code: 0,
            })),
        );
        client.connect(0).expect("initial connect");

        // 两次同 topic 订阅（SUBACK pid 1、2 依次入队）。
        for pid in 1u16..=2 {
            feed(
                &mut client,
                packet::encode(&MqttPacket::Suback(crate::packet::SubackPacket {
                    packet_id: pid,
                    return_codes: vec![0],
                })),
            );
        }
        client.subscribe("a/b", QoS::AtLeastOnce).expect("sub#1");
        client
            .subscribe("a/b", QoS::AtMostOnce)
            .expect("sub#2 同 topic 去重");
        assert_eq!(client.subscriptions.len(), 1, "同 topic 不得累积两条订阅");

        // 断线→重连两轮：恢复订阅不改列表长度。
        // 每轮预置该轮的 CONNACK + SUBACK（packet_id 随全局计数器递增）。
        for (round, pid) in [(0u32, 3u16), (1, 4)] {
            client.mark_disconnected();
            feed(
                &mut client,
                packet::encode(&MqttPacket::Connack(crate::packet::ConnackPacket {
                    session_present: false,
                    return_code: 0,
                })),
            );
            feed(
                &mut client,
                packet::encode(&MqttPacket::Suback(crate::packet::SubackPacket {
                    packet_id: pid,
                    return_codes: vec![0],
                })),
            );
            client
                .try_reconnect(10_000 + u64::from(round) * 60_000)
                .expect("reconnect");
            assert_eq!(
                client.subscriptions.len(),
                1,
                "重连恢复订阅后列表长度必须恒定"
            );
        }
    }
}

#[cfg(test)]
mod keepalive_tests {
    use super::ConnectionState;
    use crate::client::MqttClient;
    use crate::packet::{self, MqttPacket};
    use crate::transport::MockTransport;

    /// 46（WP-46.4）keepalive：poll 周期按 keep_alive_ms 自动 PINGREQ。
    #[test]
    fn keepalive_poll_sends_pingreq_after_keep_alive_interval() {
        let mut transport = MockTransport::new();
        transport.set_connected(true);
        let mut client = MqttClient::new("eneros-test", "keep-1", 5_000);
        client.set_transport(Box::new(transport));
        client.last_activity_ms = 1_000;
        client.state = ConnectionState::Connected;
        // 阈值内：不发送 PINGREQ（无入站 → 空）。
        let got = client.poll(4_000).expect("poll");
        assert!(got.is_empty());
        let sent = {
            let t = client
                .transport
                .as_mut()
                .unwrap()
                .as_any_mut()
                .downcast_mut::<MockTransport>()
                .unwrap();
            t.sent_count()
        };
        assert_eq!(sent, 0, "阈值内不得主动 PINGREQ");
        // 超过阈值：poll 触发 PINGREQ + 等待 PINGRESP（预置应答）→ 刷新活动时间。
        {
            let t = client
                .transport
                .as_mut()
                .unwrap()
                .as_any_mut()
                .downcast_mut::<MockTransport>()
                .unwrap();
            t.enqueue_recv(packet::encode(&MqttPacket::Pingresp));
        }
        let _ = client.poll(7_000).expect("poll with ping");
        {
            let t = client
                .transport
                .as_mut()
                .unwrap()
                .as_any_mut()
                .downcast_mut::<MockTransport>()
                .unwrap();
            assert_eq!(t.sent_count(), 1, "达到 keep_alive 阈值必须 PINGREQ");
            assert!(t.sent_packets()[0].starts_with(&[0xC0, 0x00])); // PINGREQ 固定头
        }
        assert_eq!(client.last_activity_ms, 7_000, "PINGREQ 后刷新活动时间");
    }
}

// ================= SPEC-GAP-CLOSURE-2026-08-26 =================
// WP-1：C1 握手负例组 + 半开检测补产；WP-3：凭据 seam 三判据。
// C3 完备性：以下用例共同覆盖 connect() 全部失败臂——encode 空 /
// send / recv / decode / return_code≠0 / 非 Connack 报文，加上
// mark_disconnected 与 disconnect() 的既有出边，ConnectionState 四个
// 变体均有非测试路径出口，不存在"进入后无法离开"的状态。

#[cfg(test)]
mod gap_closure_tests {
    use super::ConnectionState;
    use super::MqttClient;
    use crate::error::MqttError;
    use crate::packet::{self, MqttPacket};
    use crate::qos::QoS;
    use crate::transport::MockTransport;
    use crate::will::LastWill;

    const CONNACK_OK: [u8; 4] = [0x20, 0x02, 0x00, 0x00];

    fn ready_client() -> MqttClient {
        let mut t = MockTransport::new();
        t.set_connected(true);
        let mut c = MqttClient::new("eneros-test", "cid", 60_000);
        c.set_transport(Box::new(t));
        c
    }

    fn transport_of(client: &mut MqttClient) -> &mut MockTransport {
        client
            .transport
            .as_mut()
            .expect("transport")
            .as_any_mut()
            .downcast_mut::<MockTransport>()
            .expect("downcast")
    }

    /// WP-1/C1-a：无连接可建（mock 未连接）→ 显式失败且状态回 Disconnected，
    /// 不留在 Connecting。
    #[test]
    fn wp1_connect_unreachable_leaves_disconnected() {
        let mut c = MqttClient::new("eneros-test", "cid", 60_000);
        let mut t = MockTransport::new(); // 默认未连接
        t.set_connected(false);
        c.set_transport(Box::new(t));
        assert!(c.connect(0).is_err());
        assert_eq!(
            c.state,
            ConnectionState::Disconnected,
            "握手失败臂不得停留 Connecting（F-A1）"
        );
        // F-A2 联动锁：Disconnected 态 try_reconnect 必须是可推进的
        // （不再被状态门直接放行返回 Ok）。
        assert!(matches!(
            c.try_reconnect(c.reconnect_state.next_retry_ms),
            Err(MqttError::NotConnected) | Err(_)
        ));
    }

    /// WP-1/C1-b：CONNACK 等待超时（队列为空）→ BrokerUnreachable +
    /// Disconnected。
    #[test]
    fn wp1_connack_timeout_resets_disconnected() {
        let mut c = ready_client();
        assert_eq!(c.connect(0), Err(MqttError::BrokerUnreachable));
        assert_eq!(c.state, ConnectionState::Disconnected);
    }

    /// WP-1/C1-c：对端回垃圾帧 → PacketDecodeError + Disconnected。
    #[test]
    fn wp1_connack_garbage_resets_disconnected() {
        let mut c = ready_client();
        transport_of(&mut c).enqueue_recv(vec![0xFF, 0xFF]);
        assert_eq!(c.connect(0), Err(MqttError::PacketDecodeError));
        assert_eq!(c.state, ConnectionState::Disconnected);
    }

    /// WP-1/C1-d（回归锁定既有语义）：CONNACK return_code≠0 → 已是
    /// Disconnected，锁死不回归。
    #[test]
    fn wp1_connack_return_code_nonzero_disconnects() {
        let mut c = ready_client();
        transport_of(&mut c).enqueue_recv(vec![0x20, 0x02, 0x00, 0x05]);
        assert_eq!(c.connect(0), Err(MqttError::BrokerUnreachable));
        assert_eq!(c.state, ConnectionState::Disconnected);
    }

    /// WP-1/C1-e（回归锁定）：收到合法但非 Connack 报文 →
    /// UnexpectedPacket + Disconnected。
    #[test]
    fn wp1_unexpected_packet_disconnects() {
        let mut c = ready_client();
        transport_of(&mut c).enqueue_recv(vec![0x40, 0x02, 0x00, 0x01]); // PUBACK
        assert_eq!(c.connect(0), Err(MqttError::UnexpectedPacket));
        assert_eq!(c.state, ConnectionState::Disconnected);
    }

    /// WP-1/C1-f：will payload 超 u16（31 PR-10 编码拒绝臂）→
    /// PacketEncode + Disconnected。
    #[test]
    fn wp1_oversized_will_encode_fails_and_disconnects() {
        let mut c = ready_client();
        c.set_will(LastWill {
            topic: String::from("t"),
            payload: alloc::vec![0u8; 70_000],
            qos: QoS::AtMostOnce,
            retain: false,
        });
        assert_eq!(c.connect(0), Err(MqttError::PacketEncode));
        assert_eq!(c.state, ConnectionState::Disconnected);
    }

    /// WP-1/C5 前置（半开检测补产）：已连接后空闲越过 keep_alive →
    /// 自动 PINGREQ 且无 PINGRESP → PublishTimeout 传播 + 状态进入
    /// Reconnecting（mark_disconnected 语义），插件层据此走惰性重连。
    #[test]
    fn wp1_halfopen_ping_timeout_marks_reconnecting() {
        let mut t = MockTransport::new();
        t.set_connected(true);
        let mut c = MqttClient::new("eneros-test", "cid", 5_000);
        c.set_transport(Box::new(t));
        c.last_activity_ms = 1_000;
        c.state = ConnectionState::Connected;
        // 不预置 PINGRESP：模拟半开 TCP 对端无应答。
        let r = c.poll(7_000);
        assert_eq!(r, Err(MqttError::PublishTimeout));
        assert_eq!(
            c.state,
            ConnectionState::Reconnecting,
            "PING 无应答必须标记断连（plugin :676 注释所宣称的语义）"
        );
        let sent = transport_of(&mut c).sent_packets();
        assert!(
            sent.iter().any(|f| f.starts_with(&[0xC0, 0x00])),
            "检测必须经由真实 PINGREQ 路径"
        );
    }

    /// WP-3/C1 golden：set_credentials 后 CONNECT 帧与规约直构逐字节一致
    /// （clientId="cid"、user="u"、pass="p"、clean_session、keepalive=60s：
    /// flags=0xC2、剩余长度=21）。
    #[test]
    fn wp3_golden_frame_with_credentials() {
        let mut c = ready_client();
        c.set_credentials("u", b"p").expect("seam");
        transport_of(&mut c).enqueue_recv(CONNACK_OK.to_vec());
        c.connect(0).expect("带凭据握手成功");
        assert_eq!(c.state, ConnectionState::Connected);
        let expected: Vec<Vec<u8>> = vec![vec![
            0x10, 0x15, 0x00, 0x04, b'M', b'Q', b'T', b'T', 0x04, 0xC2, 0x00, 0x3C, 0x00, 0x03,
            b'c', b'i', b'd', 0x00, 0x01, b'u', 0x00, 0x01, b'p',
        ]];
        assert_eq!(transport_of(&mut c).sent_packets(), expected);
    }

    /// WP-3/C3 回归锁：None 凭据路径的 CONNECT 帧 与 seam 引入前逐字节
    /// 一致（flags 仅 clean_session=0x02、payload 只含 clientId）。
    #[test]
    fn wp3_none_credentials_frame_byte_unchanged() {
        let mut c = ready_client();
        transport_of(&mut c).enqueue_recv(CONNACK_OK.to_vec());
        c.connect(0).expect("匿名握手成功");
        let expected: Vec<Vec<u8>> = vec![vec![
            0x10, 0x0F, 0x00, 0x04, b'M', b'Q', b'T', b'T', 0x04, 0x02, 0x00, 0x3C, 0x00, 0x03,
            b'c', b'i', b'd',
        ]];
        assert_eq!(transport_of(&mut c).sent_packets(), expected);
    }

    /// WP-3/C2 负例：超过 u16 长度前缀上限的凭据在注入时即拒绝
    /// （fail-closed，不到 wire 层静默截断），且不污染既有凭据态。
    #[test]
    fn wp3_oversized_credentials_rejected_at_seam() {
        let mut c = ready_client();
        assert_eq!(
            c.set_credentials(&"u".repeat(70_000), b"p"),
            Err(MqttError::PacketEncode)
        );
        assert!(c.credentials.is_none(), "被拒绝的注入不得留半成品状态");
        assert_eq!(
            c.set_credentials("ok-user", &[0u8; 70_001]),
            Err(MqttError::PacketEncode)
        );
        assert!(c.set_credentials("u2", b"p2").is_ok());
        assert!(
            matches!(&c.credentials, Some((u, p)) if u == "u2" && p == b"p2"),
            "合法注入覆盖生效"
        );
    }

    /// 保险丝：encode 出口对超长 username 同样返回 None（seam 层失守时
    /// wire 层仍 fail-closed——与 31 PR-10 will 同型防线）。
    #[test]
    fn wp3_wire_layer_rejects_oversized_username_as_backstop() {
        let pkt = MqttPacket::Connect(packet::ConnectPacket {
            client_id: String::from("cid"),
            username: Some("x".repeat(70_000)),
            password: None,
            will: None,
            keep_alive_secs: 60,
            clean_session: true,
        });
        let bytes = packet::encode(&pkt);
        assert!(
            bytes.is_empty(),
            "wire 层对 >u16 的 username 必须 fail-closed"
        );
    }
}
