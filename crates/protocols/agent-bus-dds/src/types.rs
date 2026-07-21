//! DDS 数据样本与句柄类型（D4：slotmap 管理句柄）.

use alloc::vec::Vec;

use slotmap::new_key_type;

/// DDS 实例句柄（标识同一 topic 下的数据实例）.
pub type InstanceHandle = u64;

// D4：使用 slotmap::SlotMap 管理句柄，避免 use-after-free 与 ABA 问题。
new_key_type! {
    /// Participant 句柄（由 create_participant 返回）.
    pub struct ParticipantId;
}

new_key_type! {
    /// Reader 句柄（由 create_reader 返回）.
    pub struct ReaderId;
}

new_key_type! {
    /// Writer 句柄（由 create_writer 返回）.
    pub struct WriterId;
}

/// DDS 数据样本.
///
/// `payload` 为原始字节，由上层协议定义序列化格式；
/// `instance_handle` 标识同一 topic 下的数据实例；
/// `source_timestamp` 为发布方时间戳（ns），由 Mock 实现填充 `now_ns`。
#[derive(Debug, Clone, PartialEq)]
pub struct DdsSample {
    /// 样本负载（原始字节）.
    pub payload: Vec<u8>,
    /// 实例句柄（同一 writer 的实例递增计数）.
    pub instance_handle: InstanceHandle,
    /// 发布方时间戳（ns）.
    pub source_timestamp: u64,
}
