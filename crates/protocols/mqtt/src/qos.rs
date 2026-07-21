//! MQTT QoS（服务质量）等级.

/// MQTT QoS 等级（MQTT v3.1.1 §3.2.3）.
///
/// - `AtMostOnce`（0）：至多一次，火忘，不等待 ACK
/// - `AtLeastOnce`（1）：至少一次，PUBLISH → PUBACK
/// - `ExactlyOnce`（2）：恰好一次，PUBLISH → PUBREC → PUBREL → PUBCOMP
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum QoS {
    /// 至多一次（火忘）.
    AtMostOnce = 0,
    /// 至少一次（PUBLISH → PUBACK）.
    AtLeastOnce = 1,
    /// 恰好一次（PUBLISH → PUBREC → PUBREL → PUBCOMP）.
    ExactlyOnce = 2,
}

impl QoS {
    /// 从 u8 转换为 QoS（非法值返回 None）.
    pub fn from_u8(value: u8) -> Option<QoS> {
        match value {
            0 => Some(QoS::AtMostOnce),
            1 => Some(QoS::AtLeastOnce),
            2 => Some(QoS::ExactlyOnce),
            _ => None,
        }
    }

    /// 返回 QoS 对应的 u8 值.
    pub fn as_u8(&self) -> u8 {
        *self as u8
    }
}
