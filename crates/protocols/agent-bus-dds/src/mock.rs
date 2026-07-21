//! MockDdsNode — 默认可用的 Mock 实现（D3 / D4 / D11）.
//!
//! 纯 Rust，无外部 C 库依赖。用于单元测试与无 Cyclone DDS 环境下的接口验证。
//!
//! # 消息存储设计
//!
//! 每个 reader 持有独立 buffer（`VecDeque<DdsSample>`），write 时遍历所有 reader
//! 匹配 topic 推入样本（广播语义）。KeepLast 时按 reader 自身 qos 截断 buffer。
//!
//! # 句柄管理
//!
//! reader/writer 存储在 `MockDdsNode` 的全局 `SlotMap` 中（非嵌套于 participant），
//! 避免跨 `SlotMap` 实例的 key 碰撞问题。`MockParticipant` 持有其下 reader/writer
//! 的 ID 列表用于关联管理。

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

use slotmap::SlotMap;

use crate::config::DdsConfig;
use crate::error::DdsError;
use crate::node::{DdsNode, DdsNodeConfig};
use crate::qos::{History, QosPolicy};
use crate::types::{DdsSample, ParticipantId, ReaderId, WriterId};

/// Mock reader（每个 reader 持有独立 buffer，广播语义）.
pub struct MockReader {
    /// 订阅主题名.
    pub topic: String,
    /// reader 服务质量策略.
    pub qos: QosPolicy,
    /// 接收样本缓冲区（FIFO，KeepLast 时按 `History::KeepLast(depth)` 截断）.
    pub buffer: VecDeque<DdsSample>,
}

/// Mock writer.
pub struct MockWriter {
    /// 发布主题名.
    pub topic: String,
    /// writer 服务质量策略.
    pub qos: QosPolicy,
    /// 实例计数器（每次 write 递增，作为 instance_handle）.
    pub instance_counter: u64,
}

/// Mock participant.
///
/// 持有其下 reader/writer 的 ID 列表。reader/writer 的实际数据存储在
/// `MockDdsNode` 的全局 `SlotMap` 中，避免跨 `SlotMap` 实例的 key 碰撞。
pub struct MockParticipant {
    /// 该 participant 下的 reader ID 列表.
    pub readers: Vec<ReaderId>,
    /// 该 participant 下的 writer ID 列表.
    pub writers: Vec<WriterId>,
}

/// Mock DDS 节点（D3：默认可用，纯 Rust）.
///
/// 支持：
/// - 创建 participant / reader / writer
/// - write 广播到所有匹配 topic 的 reader
/// - read（不清空）/ take（清空）
/// - KeepLast 深度限制
/// - set_now_ns 注入时间戳（D11，避免 `SystemTime::now()`）
pub struct MockDdsNode {
    /// 节点配置.
    config: DdsConfig,
    /// participant 表.
    participants: SlotMap<ParticipantId, MockParticipant>,
    /// 全局 reader 表（跨 participant 共享 key 空间，避免 key 碰撞）.
    readers: SlotMap<ReaderId, MockReader>,
    /// 全局 writer 表.
    writers: SlotMap<WriterId, MockWriter>,
    /// 节点是否已关闭.
    shutdown: bool,
    /// 当前时间戳（ns），由 `set_now_ns` 设置（D11）.
    now_ns: u64,
}

impl MockDdsNode {
    /// 创建 MockDdsNode.
    pub fn new(config: DdsConfig) -> Self {
        Self {
            config,
            participants: SlotMap::with_key(),
            readers: SlotMap::with_key(),
            writers: SlotMap::with_key(),
            shutdown: false,
            now_ns: 0,
        }
    }

    /// 使用默认配置创建 MockDdsNode.
    pub fn new_default() -> Self {
        Self::new(DdsConfig::default())
    }

    /// 设置当前时间戳（ns），用于填充 `DdsSample.source_timestamp`（D11）.
    ///
    /// 避免 `SystemTime::now()`（no_std 不兼容），由调用方注入时间戳。
    pub fn set_now_ns(&mut self, now_ns: u64) {
        self.now_ns = now_ns;
    }
}

impl DdsNodeConfig for MockDdsNode {
    fn config(&self) -> &DdsConfig {
        &self.config
    }
}

impl DdsNode for MockDdsNode {
    fn create_participant(&mut self) -> Result<ParticipantId, DdsError> {
        if self.shutdown {
            return Err(DdsError::Closed);
        }
        let participant = MockParticipant {
            readers: Vec::new(),
            writers: Vec::new(),
        };
        Ok(self.participants.insert(participant))
    }

    fn create_reader(
        &mut self,
        participant: ParticipantId,
        topic: &str,
        qos: QosPolicy,
    ) -> Result<ReaderId, DdsError> {
        if self.shutdown {
            return Err(DdsError::Closed);
        }
        let p = self
            .participants
            .get_mut(participant)
            .ok_or(DdsError::InvalidHandle)?;
        let reader = MockReader {
            topic: String::from(topic),
            qos,
            buffer: VecDeque::new(),
        };
        let reader_id = self.readers.insert(reader);
        p.readers.push(reader_id);
        Ok(reader_id)
    }

    fn create_writer(
        &mut self,
        participant: ParticipantId,
        topic: &str,
        qos: QosPolicy,
    ) -> Result<WriterId, DdsError> {
        if self.shutdown {
            return Err(DdsError::Closed);
        }
        let p = self
            .participants
            .get_mut(participant)
            .ok_or(DdsError::InvalidHandle)?;
        let writer = MockWriter {
            topic: String::from(topic),
            qos,
            instance_counter: 0,
        };
        let writer_id = self.writers.insert(writer);
        p.writers.push(writer_id);
        Ok(writer_id)
    }

    fn read(&mut self, reader: ReaderId, max_samples: usize) -> Result<Vec<DdsSample>, DdsError> {
        if self.shutdown {
            return Err(DdsError::Closed);
        }
        let r = self.readers.get(reader).ok_or(DdsError::InvalidHandle)?;
        let count = r.buffer.len().min(max_samples);
        Ok(r.buffer.iter().take(count).cloned().collect())
    }

    fn take(&mut self, reader: ReaderId, max_samples: usize) -> Result<Vec<DdsSample>, DdsError> {
        if self.shutdown {
            return Err(DdsError::Closed);
        }
        let r = self
            .readers
            .get_mut(reader)
            .ok_or(DdsError::InvalidHandle)?;
        let count = r.buffer.len().min(max_samples);
        Ok(r.buffer.drain(..count).collect())
    }

    fn write(&mut self, writer: WriterId, data: &[u8]) -> Result<(), DdsError> {
        if self.shutdown {
            return Err(DdsError::Closed);
        }

        // 先从 writer 获取 topic 与 instance_handle（块内结束 writer 借用）.
        let (topic, instance_handle) = {
            let w = self
                .writers
                .get_mut(writer)
                .ok_or(DdsError::InvalidHandle)?;
            let topic = w.topic.clone();
            let instance_handle = w.instance_counter;
            w.instance_counter += 1;
            (topic, instance_handle)
        };

        let sample = DdsSample {
            payload: Vec::from(data),
            instance_handle,
            source_timestamp: self.now_ns,
        };

        // 广播：遍历所有 reader，匹配 topic 推入样本（受 reader 自身 KeepLast 限制）.
        // v0.76.0：KeepLast 深度内嵌于枚举变体，通过模式匹配获取深度。
        for r in self.readers.values_mut() {
            if r.topic == topic {
                r.buffer.push_back(sample.clone());
                if let History::KeepLast(depth) = r.qos.history {
                    let depth = depth as usize;
                    while r.buffer.len() > depth {
                        r.buffer.pop_front();
                    }
                }
            }
        }

        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), DdsError> {
        self.shutdown = true;
        Ok(())
    }

    fn is_shutdown(&self) -> bool {
        self.shutdown
    }
}
