//! MQTT 遗嘱消息（Last Will and Testament）.

use alloc::string::String;
use alloc::vec::Vec;

use crate::qos::QoS;

/// MQTT 遗嘱消息.
///
/// 客户端异常断开时由 Broker 自动发布的消息（MQTT v3.1.1 §3.1.2.5）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastWill {
    /// 遗嘱 Topic.
    pub topic: String,
    /// 遗嘱 Payload.
    pub payload: Vec<u8>,
    /// 遗嘱 QoS.
    pub qos: QoS,
    /// 是否为保留消息.
    pub retain: bool,
}

impl LastWill {
    /// 构造遗嘱消息.
    pub fn new(topic: &str, payload: &[u8], qos: QoS, retain: bool) -> Self {
        Self {
            topic: String::from(topic),
            payload: Vec::from(payload),
            qos,
            retain,
        }
    }
}
