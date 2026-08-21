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
    /// 31 PR-12：in-flight 报文缓存（QoS1/2 超时重传用；ack 后移除）。
    inflight: BTreeMap<u16, PublishPacket>,
    /// QoS 1/2 在途消息持久化（v0.221.0；None = 不持久化）。
    pub qos_store: Option<Box<dyn QosStore>>,
}

impl MqttClient {
    /// 构造客户端（初始状态 Disconnected，无传输层）.
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
            inflight: BTreeMap::new(),
            qos_store: None,
        }
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
            username: None,
            password: None,
            will: self.last_will.clone(),
            keep_alive_secs,
            clean_session: true,
        };
        let bytes = packet::encode(&MqttPacket::Connect(connect));
        // 31 PR-10：will payload 超限（编码失败）→ 拒绝连接（fail-closed，
        // 不发送损坏报文）。
        if bytes.is_empty() {
            return Err(MqttError::PacketEncode);
        }
        transport
            .send(&bytes)
            .map_err(|_| MqttError::TransportError)?;
        // 等待 CONNACK（同步 poll 一次）
        let connack_bytes = transport.recv().map_err(|_| MqttError::BrokerUnreachable)?;
        let pkt = packet::decode(&connack_bytes).map_err(|_| MqttError::PacketDecodeError)?;
        match pkt {
            MqttPacket::Connack(c) => {
                if c.return_code == 0 {
                    self.state = ConnectionState::Connected;
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
        // 加入订阅列表（重连恢复用）
        self.subscriptions.push((String::from(topic), qos));
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
        self.pending_acks.clear();
        self.inflight.clear(); // 31-review：断开清在途表（防重连后 id 复用错发）。
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
                self.ping(now_ms)?;
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
            MqttPacket::Publish(_) => {
                let v = vec![pkt];
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
        self.pending_acks.clear();
        self.inflight.clear(); // 31-review：同 disconnect。
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
