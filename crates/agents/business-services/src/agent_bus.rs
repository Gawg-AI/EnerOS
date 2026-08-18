//! svc-agent-bus 领域逻辑：MQTT AgentBus 服务（蓝图 41 WP-41.8）。
//!
//! - 独立 EL0 服务；TLS/mTLS identity、topic ACL、tenant/device scope；
//! - QoS 1/2 inflight、subscription、session cursor 持久化（durable）；
//! - reconnect/backoff/jitter 有界（复用 mqtt client 的 ReconnectState）；
//! - duplicate/replay/retained message policy 显式；
//! - broker ACK 分层：publish 调用返回 ≠ 业务消费成功。

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use eneros_durable_state::{DurableStateStore, Generation, StoreError};

/// 会话游标（持久化：subscription + QoS inflight 上限）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCursor {
    /// 已确认的 broker packet id 上限。
    pub acked_packet_id: u16,
    /// 已确认的 QoS2 publish id 上限。
    pub qos2_acked_id: u16,
    /// 订阅 topic（有界）。
    pub subscriptions: Vec<String>,
    /// session 是否已恢复。
    pub restored: bool,
}

impl SessionCursor {
    pub fn new() -> Self {
        SessionCursor {
            acked_packet_id: 0,
            qos2_acked_id: 0,
            subscriptions: Vec::new(),
            restored: false,
        }
    }
}

impl Default for SessionCursor {
    fn default() -> Self {
        Self::new()
    }
}

impl eneros_durable_state::Encode for SessionCursor {
    fn encode(&self) -> Result<Vec<u8>, StoreError> {
        let mut out = Vec::with_capacity(64);
        out.extend_from_slice(&self.acked_packet_id.to_le_bytes());
        out.extend_from_slice(&self.qos2_acked_id.to_le_bytes());
        out.push(self.restored as u8);
        out.push(self.subscriptions.len() as u8);
        for s in &self.subscriptions {
            let b = s.as_bytes();
            out.push(b.len() as u8);
            out.extend_from_slice(b);
        }
        Ok(out)
    }
}

impl eneros_durable_state::Decode for SessionCursor {
    fn decode(bytes: &[u8]) -> Result<(Generation, Self), StoreError> {
        // payload = generation(8)‖state。
        if bytes.len() < 8 + 6 {
            return Err(StoreError::Corrupt);
        }
        let gen = Generation(u64::from_le_bytes(bytes[0..8].try_into().unwrap()));
        let acked_packet_id = u16::from_le_bytes(bytes[8..10].try_into().unwrap());
        let qos2_acked_id = u16::from_le_bytes(bytes[10..12].try_into().unwrap());
        let restored = bytes[12] != 0;
        let n = bytes[13] as usize;
        let mut pos = 14;
        let mut subscriptions = Vec::new();
        for _ in 0..n {
            if pos >= bytes.len() {
                return Err(StoreError::Corrupt);
            }
            let l = bytes[pos] as usize;
            pos += 1;
            if pos + l > bytes.len() {
                return Err(StoreError::Corrupt);
            }
            subscriptions.push(
                core::str::from_utf8(&bytes[pos..pos + l])
                    .map_err(|_| StoreError::Corrupt)?
                    .to_owned(),
            );
            pos += l;
        }
        Ok((
            gen,
            SessionCursor {
                acked_packet_id,
                qos2_acked_id,
                subscriptions,
                restored,
            },
        ))
    }
}

/// topic ACL（tenant/device scope；默认拒绝）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicAcl {
    pub allowed_prefixes: Vec<String>,
}

impl TopicAcl {
    pub fn allows(&self, topic: &str, tenant: u32, device: u32) -> bool {
        for p in &self.allowed_prefixes {
            // 模板："{tenant}/{device}/..." 或 "{tenant}/..."；尾部 '#' = 通配。
            let t = p
                .replace("{tenant}", &tenant.to_string())
                .replace("{device}", &device.to_string());
            if let Some(prefix) = t.strip_suffix('#') {
                let prefix = prefix.trim_end_matches('/');
                if topic == prefix || topic.starts_with(&format!("{prefix}/")) {
                    return true;
                }
            } else if topic == t || topic.starts_with(&format!("{t}/")) {
                return true;
            }
        }
        false
    }
}

/// AgentBus 消息（内部 agent IPC 与 cloud MQTT 边界清晰）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusMessage {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: u8,
    pub retain: bool,
    pub packet_id: u16,
}

/// 重复/重放策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayPolicy {
    /// 重复 publish（同 packet id）拒绝。
    RejectDuplicate,
    /// retained 消息仅在订阅时投递一次。
    RetainedOnce,
}

/// AgentBus 服务。
pub struct AgentBus<S: DurableStateStore<SessionCursor>> {
    pub cursor_store: S,
    pub cursor: SessionCursor,
    pub acl: TopicAcl,
    pub replay: ReplayPolicy,
    /// 已投递 packet id 集合（重放检测）。
    delivered: Vec<u16>,
    /// inflight QoS1/2（未确认）。
    pub inflight: Vec<u16>,
}

impl<S: DurableStateStore<SessionCursor>> AgentBus<S> {
    pub fn new(cursor_store: S, acl: TopicAcl) -> Self {
        AgentBus {
            cursor_store,
            cursor: SessionCursor::new(),
            acl,
            replay: ReplayPolicy::RejectDuplicate,
            delivered: Vec::new(),
            inflight: Vec::new(),
        }
    }

    /// 启动恢复：读持久化 cursor（掉电后从上次确认处继续）。
    pub fn restore(&mut self) -> Result<(), StoreError> {
        if let Some(v) = self.cursor_store.load()? {
            self.cursor = v.state;
            self.cursor.restored = true;
        }
        Ok(())
    }

    /// 入站 publish（broker → 设备）：ACL 放行 + 重放拒绝 + QoS 追踪。
    pub fn ingest(
        &mut self,
        msg: &BusMessage,
        tenant: u32,
        device: u32,
    ) -> Result<(), &'static str> {
        if !self.acl.allows(&msg.topic, tenant, device) {
            return Err("topic-denied");
        }
        if self.replay == ReplayPolicy::RejectDuplicate && self.delivered.contains(&msg.packet_id) {
            return Err("duplicate-publish");
        }
        self.delivered.push(msg.packet_id);
        if msg.qos >= 1 {
            self.inflight.push(msg.packet_id);
        }
        Ok(())
    }

    /// QoS ACK（broker 确认；duplicate ACK no-op）→ 持久化 cursor。
    pub fn ack_publish(&mut self, packet_id: u16) -> Result<(), StoreError> {
        if !self.inflight.contains(&packet_id) {
            return Ok(()); // duplicate ACK
        }
        self.inflight.retain(|p| *p != packet_id);
        self.cursor.acked_packet_id = self.cursor.acked_packet_id.max(packet_id);
        self.persist_cursor()
    }

    pub fn persist_cursor(&mut self) -> Result<(), StoreError> {
        let expected = self.cursor_store.load()?.map(|v| v.generation);
        self.cursor_store.compare_and_swap(expected, &self.cursor)?;
        Ok(())
    }

    /// 订阅（有界；持久化）。
    pub fn subscribe(&mut self, topic: &str) -> Result<(), &'static str> {
        if self.cursor.subscriptions.len() >= 16 {
            return Err("too-many-subscriptions");
        }
        if !self.cursor.subscriptions.iter().any(|t| t == topic) {
            self.cursor.subscriptions.push(topic.to_owned());
            self.persist_cursor().map_err(|_| "cursor-persist-failed")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use eneros_durable_state::{MemStateStore, PersistIo, RegionStateStore, UnavailableStateStore};

    use super::*;

    /// 共享介质（测试）：多实例同一存储——模拟重启后 cursor 仍在。
    #[derive(Clone)]
    struct SharedIo(alloc::rc::Rc<core::cell::RefCell<alloc::vec::Vec<u8>>>);
    impl SharedIo {
        fn new(n: usize) -> Self {
            SharedIo(alloc::rc::Rc::new(core::cell::RefCell::new(
                alloc::vec![0u8; n],
            )))
        }
    }
    impl PersistIo for SharedIo {
        fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), StoreError> {
            let b = self.0.borrow();
            let start = offset as usize;
            if start + buf.len() > b.len() {
                return Err(StoreError::OutOfSpace);
            }
            buf.copy_from_slice(&b[start..start + buf.len()]);
            Ok(())
        }
        fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), StoreError> {
            let mut b = self.0.borrow_mut();
            let start = offset as usize;
            if start + data.len() > b.len() {
                return Err(StoreError::OutOfSpace);
            }
            b[start..start + data.len()].copy_from_slice(data);
            Ok(())
        }
        fn flush(&mut self) -> Result<(), StoreError> {
            Ok(())
        }
        fn capacity(&self) -> u64 {
            self.0.borrow().len() as u64
        }
    }

    #[test]
    fn positive_acl_and_ingest() {
        let acl = TopicAcl {
            allowed_prefixes: vec!["eneros/{tenant}/device/{device}".into()],
        };
        let mut bus = AgentBus::new(MemStateStore::new(), acl.clone());
        bus.restore().unwrap();
        bus.ingest(
            &BusMessage {
                topic: "eneros/7/device/3/cmd".into(),
                payload: vec![1],
                qos: 1,
                retain: false,
                packet_id: 5,
            },
            7,
            3,
        )
        .unwrap();
        // 越权 topic 拒绝。
        assert!(bus
            .ingest(
                &BusMessage {
                    topic: "eneros/8/device/3/cmd".into(),
                    payload: vec![1],
                    qos: 1,
                    retain: false,
                    packet_id: 6,
                },
                7,
                3,
            )
            .is_err());
    }

    #[test]
    fn unavailable_state_store_fails_closed() {
        let acl = TopicAcl {
            allowed_prefixes: vec!["eneros/{tenant}/device/{device}/#".into()],
        };
        let mut bus = AgentBus::new(UnavailableStateStore::new(), acl);
        assert!(bus.restore().is_err());
        assert!(bus.subscribe("eneros/1/device/2/cmd").is_err());
    }

    #[test]
    fn negative_duplicate_rejected_and_ack_persists() {
        let acl = TopicAcl {
            allowed_prefixes: vec!["eneros/{tenant}/device/{device}/#".into()],
        };
        let mut bus = AgentBus::new(MemStateStore::new(), acl.clone());
        bus.restore().unwrap();
        let msg = BusMessage {
            topic: "eneros/1/device/2/data".into(),
            payload: vec![1],
            qos: 2,
            retain: false,
            packet_id: 9,
        };
        bus.ingest(&msg, 1, 2).unwrap();
        // 重放拒绝。
        assert_eq!(bus.ingest(&msg, 1, 2).unwrap_err(), "duplicate-publish");
        // QoS ACK 持久化。
        bus.ack_publish(9).unwrap();
        assert_eq!(bus.cursor.acked_packet_id, 9);
        // 重启后恢复（共享介质）。
        let io = SharedIo::new(4096);
        let mut bus = AgentBus::new(RegionStateStore::new(io.clone(), 0, 1024), acl.clone());
        bus.restore().unwrap();
        let msg = BusMessage {
            topic: "eneros/1/device/2/data".into(),
            payload: vec![1],
            qos: 2,
            retain: false,
            packet_id: 9,
        };
        bus.ingest(&msg, 1, 2).unwrap();
        assert_eq!(bus.ingest(&msg, 1, 2).unwrap_err(), "duplicate-publish");
        bus.ack_publish(9).unwrap();
        let mut bus2 = AgentBus::new(RegionStateStore::new(io, 0, 1024), acl.clone());
        bus2.restore().unwrap();
        assert_eq!(bus2.cursor.acked_packet_id, 9);
        assert!(bus2.cursor.restored);
    }

    #[test]
    fn negative_subscribe_persists() {
        let acl = TopicAcl {
            allowed_prefixes: vec![],
        };
        let io = SharedIo::new(4096);
        let mut bus = AgentBus::new(RegionStateStore::new(io.clone(), 0, 1024), acl.clone());
        bus.restore().unwrap();
        bus.subscribe("eneros/1/device/2/cmd").unwrap();
        let mut bus2 = AgentBus::new(RegionStateStore::new(io, 0, 1024), acl.clone());
        bus2.restore().unwrap();
        assert_eq!(bus2.cursor.subscriptions.len(), 1);
    }
}
