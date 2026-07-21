//! DdsNode trait — DDS 节点统一接口（D2 / D7 / D8）.
//!
//! D2：trait 不要求 `Send + Sync`（no_std 单线程；`*mut c_void` 非 `Send`）。
//! D7：合并 DdsReader/DdsWriter trait 为 DdsNode 统一接口。
//! D8：create_reader/create_writer 接受 `&str` topic（no_std 兼容）。

use alloc::vec::Vec;

use crate::config::DdsConfig;
use crate::error::DdsError;
use crate::qos::QosPolicy;
use crate::types::{DdsSample, ParticipantId, ReaderId, WriterId};

/// DDS 节点统一接口.
///
/// 定义 DDS 节点的生命周期与资源管理接口：创建 participant / reader / writer，
/// 读写样本，关闭节点。实现方包括 [`crate::mock::MockDdsNode`]（默认可用）与
/// [`crate::cyclone_dds::CycloneDdsNode`]（feature = "cyclone-dds"，D3）。
///
/// # 广播语义
///
/// Mock 实现中，同一 topic 的多个 reader 各自持独立 buffer，
/// writer 写入时向所有匹配 topic 的 reader 推入样本（广播）。
pub trait DdsNode {
    /// 创建 DDS participant.
    ///
    /// 返回 [`ParticipantId`] 用于后续创建 reader/writer。
    /// 节点已关闭时返回 [`DdsError::Closed`]。
    fn create_participant(&mut self) -> Result<ParticipantId, DdsError>;

    /// 在指定 participant 上创建 reader.
    ///
    /// `topic` 为订阅主题名（D8：`&str` 签名）。
    /// `qos` 为 reader 的服务质量策略。
    /// participant 无效时返回 [`DdsError::InvalidHandle`]。
    fn create_reader(
        &mut self,
        participant: ParticipantId,
        topic: &str,
        qos: QosPolicy,
    ) -> Result<ReaderId, DdsError>;

    /// 在指定 participant 上创建 writer.
    ///
    /// `topic` 为发布主题名（D8：`&str` 签名）。
    /// `qos` 为 writer 的服务质量策略。
    /// participant 无效时返回 [`DdsError::InvalidHandle`]。
    fn create_writer(
        &mut self,
        participant: ParticipantId,
        topic: &str,
        qos: QosPolicy,
    ) -> Result<WriterId, DdsError>;

    /// 读取样本（不清空 reader buffer）.
    ///
    /// 返回最多 `max_samples` 条样本。read 不消费样本，可重复读取。
    fn read(&mut self, reader: ReaderId, max_samples: usize) -> Result<Vec<DdsSample>, DdsError>;

    /// 取走样本（清空 reader buffer 中被取走的样本）.
    ///
    /// 返回最多 `max_samples` 条样本。take 消费样本，取走后不可再读。
    fn take(&mut self, reader: ReaderId, max_samples: usize) -> Result<Vec<DdsSample>, DdsError>;

    /// 写入样本.
    ///
    /// 向 writer 关联的 topic 广播样本。`data` 为原始字节。
    fn write(&mut self, writer: WriterId, data: &[u8]) -> Result<(), DdsError>;

    /// 关闭节点.
    ///
    /// 关闭后所有操作返回 [`DdsError::Closed`]。
    fn shutdown(&mut self) -> Result<(), DdsError>;

    /// 查询节点是否已关闭.
    fn is_shutdown(&self) -> bool;
}

/// 节点配置访问扩展.
///
/// 为 `DdsNode` 提供配置查询能力，便于上层获取 domain_id 等信息。
pub trait DdsNodeConfig {
    /// 返回节点配置引用.
    fn config(&self) -> &DdsConfig;
}
