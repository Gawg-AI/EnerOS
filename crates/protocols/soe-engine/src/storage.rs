//! SOE 事件持久化存储抽象（D4：trait + 内存 mock）.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use eneros_upa_model::DeviceId;

use crate::error::SoeError;
use crate::event::SoeEvent;

/// SOE 事件持久化存储 trait（D4：解耦 v0.25.0 TSDB）.
pub trait SoeStorage {
    /// 追加一批事件.
    fn append(&mut self, events: &[SoeEvent]) -> Result<(), SoeError>;
    /// 按时间范围查询（包含两端）.
    fn query_by_time(&self, start_ms: u64, end_ms: u64) -> Result<Vec<SoeEvent>, SoeError>;
    /// 按设备查询，最多返回 limit 条.
    fn query_by_device(&self, device_id: DeviceId, limit: usize)
        -> Result<Vec<SoeEvent>, SoeError>;
    /// 获取最新 count 条事件.
    fn get_latest(&self, count: usize) -> Vec<SoeEvent>;
    /// 获取未上传事件（最多 limit 条）.
    fn get_unuploaded(&self, limit: usize) -> Result<Vec<SoeEvent>, SoeError>;
    /// 标记事件已上传.
    fn mark_uploaded(&mut self, event_ids: &[u64]) -> Result<(), SoeError>;
    /// 删除 cutoff_ms 之前的事件，返回删除条数.
    fn delete_before(&mut self, cutoff_ms: u64) -> Result<usize, SoeError>;
}

/// 内存 mock 存储（用于测试与开发）.
#[derive(Debug, Default)]
pub struct InMemorySoeStorage {
    /// 已持久化事件（按时间戳升序）.
    events: Vec<SoeEvent>,
    /// 已上传事件 ID 集合.
    uploaded_ids: BTreeSet<u64>,
}

impl InMemorySoeStorage {
    /// 构造空存储.
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回已存储事件数.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// 是否为空.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

impl SoeStorage for InMemorySoeStorage {
    fn append(&mut self, events: &[SoeEvent]) -> Result<(), SoeError> {
        self.events.extend_from_slice(events);
        Ok(())
    }

    fn query_by_time(&self, start_ms: u64, end_ms: u64) -> Result<Vec<SoeEvent>, SoeError> {
        if start_ms > end_ms {
            return Err(SoeError::InvalidArgument);
        }
        Ok(self
            .events
            .iter()
            .filter(|e| e.timestamp_ms >= start_ms && e.timestamp_ms <= end_ms)
            .cloned()
            .collect())
    }

    fn query_by_device(
        &self,
        device_id: DeviceId,
        limit: usize,
    ) -> Result<Vec<SoeEvent>, SoeError> {
        Ok(self
            .events
            .iter()
            .filter(|e| e.device_id == device_id)
            .take(limit)
            .cloned()
            .collect())
    }

    fn get_latest(&self, count: usize) -> Vec<SoeEvent> {
        let len = self.events.len();
        if len == 0 || count == 0 {
            return Vec::new();
        }
        let start = len.saturating_sub(count);
        self.events[start..].to_vec()
    }

    fn get_unuploaded(&self, limit: usize) -> Result<Vec<SoeEvent>, SoeError> {
        Ok(self
            .events
            .iter()
            .filter(|e| !self.uploaded_ids.contains(&e.event_id))
            .take(limit)
            .cloned()
            .collect())
    }

    fn mark_uploaded(&mut self, event_ids: &[u64]) -> Result<(), SoeError> {
        for id in event_ids {
            self.uploaded_ids.insert(*id);
        }
        Ok(())
    }

    fn delete_before(&mut self, cutoff_ms: u64) -> Result<usize, SoeError> {
        let before = self.events.len();
        self.events.retain(|e| e.timestamp_ms >= cutoff_ms);
        Ok(before - self.events.len())
    }
}
