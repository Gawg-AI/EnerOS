//! SOE 事件顺序记录引擎.

use alloc::boxed::Box;
use alloc::collections::BinaryHeap;
use alloc::vec::Vec;
use core::cmp::Ordering;

use eneros_upa_model::{DataPoint, DeviceId};

use crate::config::{SoeConfig, SoeStats};
use crate::error::SoeError;
use crate::event::SoeEvent;
use crate::storage::SoeStorage;
use crate::trigger::EventTrigger;
use crate::upload::UploadChannel;

/// 事件按时间戳排序的堆包装.
///
/// 实现逆序 `Ord`：时间戳越大"越小"，使 `BinaryHeap`（最大堆）弹出最小时间戳事件（D6）。
struct EventByTimestamp(SoeEvent);

impl PartialEq for EventByTimestamp {
    fn eq(&self, other: &Self) -> bool {
        self.0.timestamp_ms == other.0.timestamp_ms
    }
}

impl Eq for EventByTimestamp {}

impl PartialOrd for EventByTimestamp {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EventByTimestamp {
    fn cmp(&self, other: &Self) -> Ordering {
        // 逆序：时间戳越大排得越"小"，最大堆弹出最小时间戳。
        other.0.timestamp_ms.cmp(&self.0.timestamp_ms)
    }
}

/// SOE 事件顺序记录引擎.
pub struct SoeEngine {
    /// 事件队列（按时间戳升序出队，D6）.
    event_queue: BinaryHeap<EventByTimestamp>,
    /// 事件 ID 自增器（D8：非 AtomicU64）.
    next_event_id: u64,
    /// 事件触发器列表.
    triggers: Vec<Box<dyn EventTrigger>>,
    /// 持久化存储.
    storage: Box<dyn SoeStorage>,
    /// 上传通道.
    upload_channel: Option<Box<dyn UploadChannel>>,
    /// 引擎配置.
    config: SoeConfig,
    /// 运行时统计.
    stats: SoeStats,
}

impl SoeEngine {
    /// 构造引擎.
    pub fn new(config: SoeConfig, storage: Box<dyn SoeStorage>) -> Self {
        Self {
            event_queue: BinaryHeap::new(),
            next_event_id: 0,
            triggers: Vec::new(),
            storage,
            upload_channel: None,
            config,
            stats: SoeStats::default(),
        }
    }

    /// 添加事件触发器.
    pub fn add_trigger(&mut self, trigger: Box<dyn EventTrigger>) {
        self.triggers.push(trigger);
    }

    /// 设置上传通道.
    pub fn set_upload_channel(&mut self, channel: Box<dyn UploadChannel>) {
        self.upload_channel = Some(channel);
    }

    /// 记录事件（分配 event_id、入队、统计；达到批量阈值自动持久化）.
    pub fn record_event(&mut self, mut event: SoeEvent) -> Result<u64, SoeError> {
        event.event_id = self.next_event_id;
        self.next_event_id += 1;
        let event_id = event.event_id;
        self.event_queue.push(EventByTimestamp(event));
        self.stats.total_events += 1;
        if self.event_queue.len() >= self.config.persist_batch_size {
            self.persist_events()?;
        }
        Ok(event_id)
    }

    /// 处理数据点变化：遍历触发器，记录每个触发的事件，返回事件 ID 列表.
    pub fn process_point_change(
        &mut self,
        old: &DataPoint,
        new: &DataPoint,
        now_ms: u64,
    ) -> Vec<u64> {
        // 先收集触发事件（避免与 record_event 的可变借用冲突）.
        let mut triggered = Vec::new();
        for trigger in &self.triggers {
            if let Some(event) = trigger.check(old, new, now_ms) {
                triggered.push(event);
            }
        }
        let mut ids = Vec::new();
        for event in triggered {
            if let Ok(id) = self.record_event(event) {
                ids.push(id);
            }
        }
        ids
    }

    /// 批量持久化：排空队列，按时间戳排序后写入存储.
    pub fn persist_events(&mut self) -> Result<(), SoeError> {
        let mut events: Vec<SoeEvent> = self.event_queue.drain().map(|w| w.0).collect();
        // 按时间戳稳定排序，确保不乱序.
        events.sort_by_key(|e| e.timestamp_ms);
        let count = events.len();
        self.storage.append(&events)?;
        self.stats.persisted_events += count as u64;
        Ok(())
    }

    /// 按时间范围查询事件.
    pub fn query_by_time(&self, start_ms: u64, end_ms: u64) -> Result<Vec<SoeEvent>, SoeError> {
        self.storage.query_by_time(start_ms, end_ms)
    }

    /// 按设备查询事件.
    pub fn query_by_device(
        &self,
        device_id: DeviceId,
        limit: usize,
    ) -> Result<Vec<SoeEvent>, SoeError> {
        self.storage.query_by_device(device_id, limit)
    }

    /// 获取最新 count 条事件.
    pub fn get_latest(&self, count: usize) -> Vec<SoeEvent> {
        self.storage.get_latest(count)
    }

    /// 上传未上传事件，返回上传条数.
    pub fn upload_events(&mut self) -> Result<usize, SoeError> {
        let channel = match self.upload_channel.as_mut() {
            Some(c) if c.is_connected() => c,
            _ => return Ok(0),
        };
        let limit = self.config.persist_batch_size;
        let events = self.storage.get_unuploaded(limit)?;
        if events.is_empty() {
            return Ok(0);
        }
        let count = events.len();
        channel.upload(&events)?;
        let ids: Vec<u64> = events.iter().map(|e| e.event_id).collect();
        self.storage.mark_uploaded(&ids)?;
        self.stats.uploaded_events += count as u64;
        Ok(count)
    }

    /// 清理过期事件（按 retention_days 计算 cutoff）.
    pub fn cleanup_expired(&mut self, now_ms: u64) -> Result<usize, SoeError> {
        let retention_ms = self.config.retention_days as u64 * 86_400_000;
        let cutoff = now_ms.saturating_sub(retention_ms);
        self.storage.delete_before(cutoff)
    }

    /// 返回运行时统计.
    pub fn stats(&self) -> &SoeStats {
        &self.stats
    }
}
