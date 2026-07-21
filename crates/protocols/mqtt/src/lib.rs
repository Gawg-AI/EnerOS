//! EnerOS MQTT v3.1.1 客户端（v0.53.1）.
//!
//! 提供物联网标准轻量协议 MQTT v3.1.1 的客户端实现，支持 QoS 0/1/2 发布与订阅、
//! 遗嘱消息（Last Will）、断线指数退避重连、订阅自动恢复，为储能终端运行数据
//! （SOC/功率/告警）上报至云端运维平台/SCADA 主站提供标准物联网通道。
//!
//! # 核心类型
//! - [`qos::QoS`] — QoS 等级（AtMostOnce/AtLeastOnce/ExactlyOnce）
//! - [`error::MqttError`] — 错误枚举（10 变体）
//! - [`will::LastWill`] — 遗嘱消息
//! - [`topic::TopicFilter`] — Topic 过滤器（支持 + 和 # 通配符）
//! - [`packet::MqttPacket`] — 14 种控制报文枚举
//! - [`packet::encode`]/[`packet::decode`] — 报文编解码
//! - [`transport::MqttTransport`] — 传输层 trait + [`transport::MockTransport`] mock
//! - [`client::MqttClient`] — 客户端状态机（Disconnected/Connecting/Connected/Reconnecting）
//! - [`client::ReconnectState`] — 指数退避重连状态
//!
//! # 偏差声明（D11~D18）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D11** | crate 放入 `crates/protocols/mqtt/`（P1-G 物联网协议层） |
//! | **D12** | TCP 传输抽象为 `MqttTransport` trait + `MockTransport` 实现（不直接依赖 smoltcp；与 v0.46.0/v0.49.0 transport trait 模式一致） |
//! | **D13** | 仅支持 MQTT v3.1.1（不支持 MQTT 5；蓝图 §5 技术交底明确"MVP 不需要复杂特性"） |
//! | **D14** | 不实现 TLS（MVP；蓝图 §8 注明"凭证安全：与 v0.31.0 国密联动"留待后续集成） |
//! | **D15** | QoS 1/2 未确认消息仅在内存（不持久化；蓝图 §5 提及"需持久化"但 MVP 简化） |
//! | **D16** | 时间戳/超时使用 `u64` 毫秒参数注入（与 D1 一致） |
//! | **D17** | 不要求 `Send + Sync`（no_std 单线程；与 v0.51.0 D2 一致） |
//! | **D18** | 凭证使用 `String` 明文（不加密；加密留待与 v0.31.0 国密集成） |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，依赖 `eneros-upa-model`（纯数据模型，目前未直接使用，
//! 保留供未来 DataPoint → MQTT payload 桥接）。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod client;
pub mod error;
pub mod packet;
pub mod qos;
pub mod topic;
pub mod transport;
pub mod will;

pub use client::{ConnectionState, MqttClient, PendingAck, ReconnectState};
pub use error::MqttError;
pub use packet::{
    ConnackPacket, ConnectPacket, MqttPacket, PublishPacket, SubackPacket, SubscribePacket,
    UnsubscribePacket,
};
pub use qos::QoS;
pub use topic::TopicFilter;
pub use transport::{MockTransport, MqttTransport};
pub use will::LastWill;

#[cfg(test)]
mod tests {
    //! 集成测试 — 覆盖 MQTT 客户端全链路（T1~T15）.

    use alloc::boxed::Box;
    use alloc::vec;

    use super::*;
    use crate::packet::{decode, encode};

    /// 构造 CONNACK 字节（return_code=0，session_present=false）.
    fn make_connack() -> Vec<u8> {
        vec![0x20, 0x02, 0x00, 0x00]
    }

    /// 构造 SUBACK 字节（packet_id，return_code=0 表示 QoS 0 接受）.
    fn make_suback(packet_id: u16) -> Vec<u8> {
        vec![
            0x90,
            0x03,
            ((packet_id >> 8) & 0xFF) as u8,
            (packet_id & 0xFF) as u8,
            0x00,
        ]
    }

    /// 构造 PUBACK 字节.
    fn make_puback(packet_id: u16) -> Vec<u8> {
        vec![
            0x40,
            0x02,
            ((packet_id >> 8) & 0xFF) as u8,
            (packet_id & 0xFF) as u8,
        ]
    }

    // ===== T1：QoS 枚举值 =====
    #[test]
    fn test_t1_qos_enum_values() {
        assert_eq!(QoS::AtMostOnce.as_u8(), 0);
        assert_eq!(QoS::AtLeastOnce.as_u8(), 1);
        assert_eq!(QoS::ExactlyOnce.as_u8(), 2);
        // from_u8 往返
        assert_eq!(QoS::from_u8(0), Some(QoS::AtMostOnce));
        assert_eq!(QoS::from_u8(1), Some(QoS::AtLeastOnce));
        assert_eq!(QoS::from_u8(2), Some(QoS::ExactlyOnce));
        assert_eq!(QoS::from_u8(3), None);
    }

    // ===== T2：LastWill 构造 =====
    #[test]
    fn test_t2_last_will_construction() {
        let will = LastWill::new("status/client-1", b"offline", QoS::AtLeastOnce, true);
        assert_eq!(will.topic, "status/client-1");
        assert_eq!(will.payload, vec![b'o', b'f', b'f', b'l', b'i', b'n', b'e']);
        assert_eq!(will.qos, QoS::AtLeastOnce);
        assert!(will.retain);
    }

    // ===== T3：TopicFilter 精确匹配 =====
    #[test]
    fn test_t3_topic_filter_exact_match() {
        let f = TopicFilter::new("sensor/temperature");
        assert!(f.matches("sensor/temperature"));
        assert!(!f.matches("sensor/humidity"));
        assert!(!f.matches("sensor/temperature/extra"));
        assert!(!f.matches("sensor"));
    }

    // ===== T4：TopicFilter + 单层通配符 =====
    #[test]
    fn test_t4_topic_filter_single_wildcard() {
        let f = TopicFilter::new("sensor/+");
        assert!(f.matches("sensor/temperature"));
        assert!(f.matches("sensor/humidity"));
        assert!(!f.matches("sensor/room/temperature"));
        assert!(!f.matches("sensor"));
    }

    // ===== T5：TopicFilter # 多层通配符 =====
    #[test]
    fn test_t5_topic_filter_multi_wildcard() {
        let f = TopicFilter::new("sensor/#");
        assert!(f.matches("sensor/"));
        assert!(f.matches("sensor/temperature"));
        assert!(f.matches("sensor/room/temperature"));
        // # 不匹配无 / 的父级（保守实现，与多数 Broker 一致）
        assert!(!f.matches("sensor"));
        assert!(!f.matches("actuator/temperature"));
    }

    // ===== T6：CONNECT 报文编码 =====
    #[test]
    fn test_t6_connect_encode() {
        let p = ConnectPacket {
            client_id: String::from("client-1"),
            username: None,
            password: None,
            will: None,
            keep_alive_secs: 60,
            clean_session: true,
        };
        let bytes = encode(&MqttPacket::Connect(p));
        // 第一字节：CONNECT(1) << 4 = 0x10，flags=0
        assert_eq!(bytes[0], 0x10);
        // 第二字节：剩余长度（应 > 0）
        assert!(bytes[1] > 0);
        // 协议名 "MQTT" 在可变头开头（跳过剩余长度字节后）
        // bytes[2] 起为剩余长度编码（1 字节），其后为 "MQTT" 长度前缀
        // 协议名长度 = 4，所以 bytes[2..4] = [0x00, 0x04]，bytes[4..8] = "MQTT"
        assert_eq!(&bytes[2..4], &[0x00, 0x04]);
        assert_eq!(&bytes[4..8], b"MQTT");
        // 协议级别 0x04
        assert_eq!(bytes[8], 0x04);
    }

    // ===== T7：CONNACK 报文解码 =====
    #[test]
    fn test_t7_connack_decode() {
        // CONNACK: 0x20 0x02 0x00 0x00（session_present=false, return_code=0）
        let bytes = [0x20u8, 0x02, 0x00, 0x00];
        let pkt = decode(&bytes).unwrap();
        assert!(matches!(pkt, MqttPacket::Connack(_)));
        if let MqttPacket::Connack(c) = pkt {
            assert!(!c.session_present);
            assert_eq!(c.return_code, 0);
        }
    }

    // ===== T8：PUBLISH QoS 0 编解码往返 =====
    #[test]
    fn test_t8_publish_qos0_roundtrip() {
        let p = PublishPacket {
            topic: String::from("sensor/temp"),
            packet_id: None,
            qos: QoS::AtMostOnce,
            retain: false,
            dup: false,
            payload: vec![1, 2, 3],
        };
        let bytes = encode(&MqttPacket::Publish(p.clone()));
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, MqttPacket::Publish(p));
    }

    // ===== T9：PUBLISH QoS 1 编解码往返（含 packet_id）=====
    #[test]
    fn test_t9_publish_qos1_roundtrip() {
        let p = PublishPacket {
            topic: String::from("sensor/temp"),
            packet_id: Some(42),
            qos: QoS::AtLeastOnce,
            retain: true,
            dup: false,
            payload: vec![0xDE, 0xAD, 0xBE, 0xEF],
        };
        let bytes = encode(&MqttPacket::Publish(p.clone()));
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, MqttPacket::Publish(p));
    }

    // ===== T10：SUBSCRIBE 报文编码（验证类型字节 0x82）=====
    #[test]
    fn test_t10_subscribe_encode() {
        let p = SubscribePacket {
            packet_id: 1,
            topics: vec![(String::from("a/b"), QoS::AtLeastOnce)],
        };
        let bytes = encode(&MqttPacket::Subscribe(p));
        // 第一字节：SUBSCRIBE(8) << 4 = 0x80，flags=0b0010 → 0x82
        assert_eq!(bytes[0], 0x82);
    }

    // ===== T11：PINGREQ/PINGRESP 编解码往返 =====
    #[test]
    fn test_t11_pingreq_pingresp_roundtrip() {
        // PINGREQ
        let bytes = encode(&MqttPacket::Pingreq);
        assert_eq!(bytes, vec![0xC0, 0x00]);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, MqttPacket::Pingreq);
        // PINGRESP
        let bytes = encode(&MqttPacket::Pingresp);
        assert_eq!(bytes, vec![0xD0, 0x00]);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, MqttPacket::Pingresp);
    }

    // ===== T12：MqttClient connect + publish QoS 0 =====
    #[test]
    fn test_t12_client_connect_and_publish_qos0() {
        let mut mock = MockTransport::new();
        mock.set_connected(true);
        mock.enqueue_recv(make_connack());
        let mut client = MqttClient::new("localhost:1883", "client-1", 60_000);
        client.set_transport(Box::new(mock));
        // 连接
        assert!(client.connect(0).is_ok());
        assert_eq!(client.state, ConnectionState::Connected);
        // 发布 QoS 0（无需 ACK）
        let r = client.publish("sensor/temp", &[1, 2, 3], QoS::AtMostOnce, 0);
        assert!(r.is_ok());
        // QoS 0 无在途 ACK
        assert_eq!(client.pending_count(), 0);
    }

    // ===== T13：MqttClient subscribe =====
    #[test]
    fn test_t13_client_subscribe() {
        let mut mock = MockTransport::new();
        mock.set_connected(true);
        mock.enqueue_recv(make_connack());
        // SUBACK 对应 packet_id=1（connect 不分配 ID，subscribe 分配 ID=1）
        mock.enqueue_recv(make_suback(1));
        let mut client = MqttClient::new("localhost", "client-1", 60_000);
        client.set_transport(Box::new(mock));
        // 连接
        assert!(client.connect(0).is_ok());
        // 订阅
        let r = client.subscribe("sensor/+", QoS::AtLeastOnce);
        assert!(r.is_ok());
        // 验证订阅已加入列表
        assert_eq!(client.subscriptions.len(), 1);
        assert_eq!(client.subscriptions[0].0, "sensor/+");
        assert_eq!(client.subscriptions[0].1, QoS::AtLeastOnce);
    }

    // ===== T14：MqttClient publish QoS 1 等待 PUBACK =====
    #[test]
    fn test_t14_client_publish_qos1_wait_puback() {
        let mut mock = MockTransport::new();
        mock.set_connected(true);
        mock.enqueue_recv(make_connack());
        // PUBACK 对应 packet_id=1（publish 分配 ID=1）
        mock.enqueue_recv(make_puback(1));
        let mut client = MqttClient::new("localhost", "client-1", 60_000);
        client.set_transport(Box::new(mock));
        // 连接
        assert!(client.connect(0).is_ok());
        // 发布 QoS 1
        let r = client.publish("sensor/temp", &[0xDE, 0xAD], QoS::AtLeastOnce, 0);
        assert!(r.is_ok());
        // 验证在途 ACK 已清除
        assert_eq!(client.pending_count(), 0);
    }

    // ===== T15：MqttClient 指数退避重连 =====
    #[test]
    fn test_t15_client_reconnect_backoff() {
        let mut mock = MockTransport::new();
        mock.set_connected(true);
        mock.enqueue_recv(make_connack());
        let mut client = MqttClient::new("localhost", "client-1", 60_000);
        client.set_transport(Box::new(mock));
        // 初始连接成功
        assert!(client.connect(0).is_ok());
        assert_eq!(client.state, ConnectionState::Connected);
        // 初始 backoff_ms=1000
        assert_eq!(client.reconnect_state.backoff_ms, 1000);
        assert_eq!(client.reconnect_state.attempt_count, 0);
        // 模拟断线
        client.mark_disconnected();
        assert_eq!(client.state, ConnectionState::Reconnecting);
        // 第 1 次重连（t=0）：无 CONNACK → 失败，backoff 翻倍 1000→2000
        let r = client.try_reconnect(0);
        assert!(r.is_err());
        assert_eq!(client.state, ConnectionState::Reconnecting);
        assert_eq!(client.reconnect_state.attempt_count, 1);
        assert_eq!(client.reconnect_state.backoff_ms, 2000);
        assert_eq!(client.reconnect_state.next_retry_ms, 1000);
        // 第 2 次重连（t=500）：仍在退避期（500 < 1000）→ Err(NotConnected)
        let r = client.try_reconnect(500);
        assert!(matches!(r, Err(MqttError::NotConnected)));
        // backoff 不变
        assert_eq!(client.reconnect_state.backoff_ms, 2000);
        assert_eq!(client.reconnect_state.attempt_count, 1);
        // 第 3 次重连（t=1000）：退避期满，再次尝试，失败 → backoff 翻倍 2000→4000
        let r = client.try_reconnect(1000);
        assert!(r.is_err());
        assert_eq!(client.reconnect_state.attempt_count, 2);
        assert_eq!(client.reconnect_state.backoff_ms, 4000);
        assert_eq!(client.reconnect_state.next_retry_ms, 3000); // 1000 + 2000
    }
}
