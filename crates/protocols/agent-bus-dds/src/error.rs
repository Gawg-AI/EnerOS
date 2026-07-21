//! DDS 错误类型（D7）.

use core::fmt;

/// DDS 中间件错误.
///
/// 覆盖 FFI 调用失败、句柄无效、节点关闭、QoS 不一致、序列化失败、
/// topic 缺失、participant 缺失、超时等 8 类失败场景。
#[derive(Debug)]
pub enum DdsError {
    /// FFI 调用返回错误码（Cyclone DDS C 库返回负数）.
    Ffi(i32),
    /// 句柄无效（ParticipantId / ReaderId / WriterId 不存在或已释放）.
    InvalidHandle,
    /// 节点已关闭（shutdown 后所有操作返回此错误）.
    Closed,
    /// QoS 不一致（reader 与 writer 的 QoS 不兼容）.
    InconsistentQos(alloc::string::String),
    /// 序列化/反序列化失败（payload 无法转换）.
    Serialization(alloc::string::String),
    /// Topic 不存在（create_reader/create_writer 时 topic 未注册）.
    TopicNotFound(alloc::string::String),
    /// Participant 不存在（create_reader/create_writer 时 ParticipantId 无效）.
    ParticipantNotFound,
    /// 操作超时（read/take 等待样本超时）.
    Timeout,
}

impl fmt::Display for DdsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DdsError::Ffi(code) => write!(f, "dds ffi error (code={})", code),
            DdsError::InvalidHandle => f.write_str("invalid dds handle"),
            DdsError::Closed => f.write_str("dds node closed"),
            DdsError::InconsistentQos(msg) => write!(f, "inconsistent qos: {}", msg),
            DdsError::Serialization(msg) => write!(f, "serialization error: {}", msg),
            DdsError::TopicNotFound(topic) => write!(f, "topic not found: {}", topic),
            DdsError::ParticipantNotFound => f.write_str("participant not found"),
            DdsError::Timeout => f.write_str("dds operation timeout"),
        }
    }
}

impl core::error::Error for DdsError {}
