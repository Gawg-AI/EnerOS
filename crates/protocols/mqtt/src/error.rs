//! MQTT 客户端错误类型.

/// MQTT 客户端错误.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MqttError {
    /// 未连接（操作前需先 connect）.
    NotConnected,
    /// Broker 不可达（CONNACK 返回非 0 或传输失败）.
    BrokerUnreachable,
    /// 发布超时（QoS 1/2 等待 ACK 超时）.
    PublishTimeout,
    /// Topic 非法（含通配符或长度越界）.
    InvalidTopic,
    /// 订阅失败（SUBACK 拒绝或状态错误）.
    SubscribeFailed,
    /// 取消订阅失败.
    UnsubscribeFailed,
    /// 报文解码错误（字节流不合法或长度不足）.
    PacketDecodeError,
    /// 传输层错误（connect/send/recv/close 失败）.
    TransportError,
    /// 报文 ID 冲突（QoS 1/2 packet ID 已存在于 pending_acks）.
    PacketIdInUse,
    /// 等待 ACK 时收到非预期报文.
    UnexpectedPacket,
}
