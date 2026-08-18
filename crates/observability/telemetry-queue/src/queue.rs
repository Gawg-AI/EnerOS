//! 有界遥测队列 — 离线缓存/恢复补传/溢出降级 (OBS-013).
//!
//! 设计：
//! - 设备离线时，遥测事件入队（有界缓冲区）。
//! - 缓冲区满时按溢出策略降级（丢弃旧事件/丢弃低优先级/拒绝新事件）。
//! - 连接恢复后，按 FIFO 顺序补传。
//! - 补传成功的事件从队列移除。
//! - 队列持久化到非易失存储（由调用方负责序列化/反序列化）。

use alloc::string::String;
use alloc::vec::Vec;

/// 遥测事件类别（用于隐私过滤）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum TelemetryCategory {
    /// 系统指标（CPU/内存/磁盘使用率）。
    SystemMetrics,
    /// 性能指标（延迟/吞吐/SLO）。
    PerformanceMetrics,
    /// 告警/事件。
    AlertEvents,
    /// 安全审计。
    SecurityAudit,
    /// Agent 行为日志。
    AgentBehavior,
    /// 崩溃转储引用（不含转储本身）。
    CrashDumpRef,
    /// 配置变更。
    ConfigChanges,
    /// 网络统计。
    NetworkStats,
}

impl TelemetryCategory {
    pub fn name(self) -> &'static str {
        match self {
            Self::SystemMetrics => "system-metrics",
            Self::PerformanceMetrics => "performance-metrics",
            Self::AlertEvents => "alert-events",
            Self::SecurityAudit => "security-audit",
            Self::AgentBehavior => "agent-behavior",
            Self::CrashDumpRef => "crash-dump-ref",
            Self::ConfigChanges => "config-changes",
            Self::NetworkStats => "network-stats",
        }
    }
}

/// 溢出策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverflowPolicy {
    /// 丢弃最旧的事件（FIFO 驱逐）。
    #[default]
    DropOldest,
    /// 拒绝新事件（保持队列不变）。
    RejectNew,
    /// 丢弃低优先级类别（SecurityAudit/CrashDumpRef 优先保留）。
    DropLowPriority,
}

/// 遥测事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryEvent {
    /// 事件 ID（单调递增）。
    pub id: u64,
    /// 事件类别。
    pub category: TelemetryCategory,
    /// 事件时间戳（Unix 秒）。
    pub timestamp: u64,
    /// 事件负载（JSON 或其他格式）。
    pub payload: String,
}

impl TelemetryEvent {
    /// 事件优先级（越高越重要）。
    pub fn priority(&self) -> u8 {
        match self.category {
            TelemetryCategory::SecurityAudit => 100,
            TelemetryCategory::CrashDumpRef => 90,
            TelemetryCategory::AlertEvents => 80,
            TelemetryCategory::ConfigChanges => 70,
            TelemetryCategory::PerformanceMetrics => 50,
            TelemetryCategory::SystemMetrics => 40,
            TelemetryCategory::NetworkStats => 30,
            TelemetryCategory::AgentBehavior => 20,
        }
    }
}

/// 队列统计。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QueueStats {
    /// 当前队列长度。
    pub current_len: usize,
    /// 总入队数。
    pub total_enqueued: u64,
    /// 总出队（补传）数。
    pub total_dequeued: u64,
    /// 因溢出丢弃的事件数。
    pub total_dropped: u64,
    /// 队列最大容量。
    pub capacity: usize,
}

/// 有界遥测队列。
pub struct TelemetryQueue {
    /// 内部存储。
    events: Vec<TelemetryEvent>,
    /// 最大容量。
    capacity: usize,
    /// 溢出策略。
    overflow_policy: OverflowPolicy,
    /// 下一个事件 ID。
    next_id: u64,
    /// 统计。
    stats: QueueStats,
}

impl TelemetryQueue {
    /// 导出当前队列为持久化快照（v0.238.0，B5-01）。
    ///
    /// `last_acked_id` = 已确认发送的最大事件 ID（幂等断点：重启后
    /// 跳过 ≤ 此 ID 的事件，不重复补传）。
    pub fn snapshot(&self, last_acked_id: u64) -> crate::persist::QueueSnapshot {
        crate::persist::QueueSnapshot {
            last_acked_id,
            events: self.events.clone(),
            capacity: self.capacity,
            policy: self.overflow_policy,
        }
    }

    /// 从持久化快照恢复（v0.238.0）：重建队列并推进 next_id。
    ///
    /// 返回 `(队列, last_acked_id)`——调用方据此从断点继续补传。
    pub fn restore(snap: crate::persist::QueueSnapshot) -> (Self, u64) {
        // 33 OB-03：恢复原始容量/策略（旧快照缺省推导——len×2 与
        // DropOldest 硬编码丢失原配置）。
        let capacity = if snap.capacity >= 16 {
            snap.capacity
        } else {
            snap.events.len().max(16) * 2
        };
        let next_id = snap
            .events
            .iter()
            .map(|e| e.id)
            .max()
            .map(|m| m + 1)
            .unwrap_or(snap.last_acked_id + 1);
        let mut q = Self::new(capacity, snap.policy);
        q.events = snap.events;
        q.next_id = next_id.max(1);
        (q, snap.last_acked_id)
    }

    /// 创建队列。
    pub fn new(capacity: usize, policy: OverflowPolicy) -> Self {
        Self {
            events: Vec::with_capacity(capacity),
            capacity,
            overflow_policy: policy,
            next_id: 1,
            stats: QueueStats {
                capacity,
                ..Default::default()
            },
        }
    }

    /// 入队事件。
    ///
    /// 如果队列满，按溢出策略处理。
    /// 返回入队的事件 ID，或 None（如果被拒绝）。
    pub fn enqueue(
        &mut self,
        category: TelemetryCategory,
        timestamp: u64,
        payload: &str,
    ) -> Option<u64> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        let event = TelemetryEvent {
            id,
            category,
            timestamp,
            payload: String::from(payload),
        };

        if self.events.len() >= self.capacity {
            match self.overflow_policy {
                OverflowPolicy::DropOldest => {
                    self.events.remove(0);
                    self.stats.total_dropped += 1;
                }
                OverflowPolicy::RejectNew => {
                    self.stats.total_dropped += 1;
                    return None;
                }
                OverflowPolicy::DropLowPriority => {
                    // 找到最低优先级的事件
                    if let Some((idx, _)) = self
                        .events
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, e)| e.priority())
                    {
                        if self.events[idx].priority() < event.priority() {
                            self.events.remove(idx);
                            self.stats.total_dropped += 1;
                        } else {
                            self.stats.total_dropped += 1;
                            return None;
                        }
                    } else {
                        self.stats.total_dropped += 1;
                        return None;
                    }
                }
            }
        }

        self.events.push(event);
        self.stats.total_enqueued += 1;
        self.stats.current_len = self.events.len();
        Some(id)
    }

    /// 批量出队事件（用于补传）。
    ///
    /// 返回最多 `max_count` 个事件（按 FIFO 顺序）。
    /// 返回的事件从队列中移除。
    pub fn dequeue_batch(&mut self, max_count: usize) -> Vec<TelemetryEvent> {
        let count = max_count.min(self.events.len());
        let drained: Vec<TelemetryEvent> = self.events.drain(..count).collect();
        self.stats.total_dequeued += drained.len() as u64;
        self.stats.current_len = self.events.len();
        drained
    }

    /// 查看队列头部事件（不出队）。
    pub fn peek(&self) -> Option<&TelemetryEvent> {
        self.events.first()
    }

    /// 当前队列长度。
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// 是否已满。
    pub fn is_full(&self) -> bool {
        self.events.len() >= self.capacity
    }

    /// 返回统计。
    pub fn stats(&self) -> QueueStats {
        self.stats
    }

    /// 清空队列（用于工厂重置）。
    pub fn clear(&mut self) {
        self.events.clear();
        self.stats.current_len = 0;
    }

    /// 按类别过滤事件（用于隐私过滤后补传）。
    ///
    /// 只出队允许类别的事件，其余保留在队列中。
    pub fn dequeue_filtered(
        &mut self,
        max_count: usize,
        allowed: &[TelemetryCategory],
    ) -> Vec<TelemetryEvent> {
        let mut result = Vec::new();
        let mut i = 0;
        while i < self.events.len() && result.len() < max_count {
            if allowed.contains(&self.events[i].category) {
                result.push(self.events.remove(i));
            } else {
                i += 1;
            }
        }
        self.stats.total_dequeued += result.len() as u64;
        self.stats.current_len = self.events.len();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_dequeue_basic() {
        let mut q = TelemetryQueue::new(100, OverflowPolicy::DropOldest);
        let id = q.enqueue(TelemetryCategory::SystemMetrics, 1000, r#"{"cpu":42}"#);
        assert!(id.is_some());
        assert_eq!(q.len(), 1);

        let batch = q.dequeue_batch(10);
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].category, TelemetryCategory::SystemMetrics);
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn fifo_order() {
        let mut q = TelemetryQueue::new(100, OverflowPolicy::DropOldest);
        q.enqueue(TelemetryCategory::SystemMetrics, 1000, "first");
        q.enqueue(TelemetryCategory::AlertEvents, 1001, "second");
        q.enqueue(TelemetryCategory::SecurityAudit, 1002, "third");

        let batch = q.dequeue_batch(10);
        assert_eq!(batch[0].payload, "first");
        assert_eq!(batch[1].payload, "second");
        assert_eq!(batch[2].payload, "third");
    }

    #[test]
    fn overflow_drop_oldest() {
        let mut q = TelemetryQueue::new(2, OverflowPolicy::DropOldest);
        q.enqueue(TelemetryCategory::SystemMetrics, 1000, "first");
        q.enqueue(TelemetryCategory::SystemMetrics, 1001, "second");
        // 队列满，应丢弃最旧
        q.enqueue(TelemetryCategory::SystemMetrics, 1002, "third");

        assert_eq!(q.len(), 2);
        let batch = q.dequeue_batch(10);
        assert_eq!(batch[0].payload, "second");
        assert_eq!(batch[1].payload, "third");
        assert_eq!(q.stats().total_dropped, 1);
    }

    #[test]
    fn overflow_reject_new() {
        let mut q = TelemetryQueue::new(2, OverflowPolicy::RejectNew);
        q.enqueue(TelemetryCategory::SystemMetrics, 1000, "first");
        q.enqueue(TelemetryCategory::SystemMetrics, 1001, "second");
        // 队列满，应拒绝新事件
        let result = q.enqueue(TelemetryCategory::SystemMetrics, 1002, "third");

        assert!(result.is_none());
        assert_eq!(q.len(), 2);
        let batch = q.dequeue_batch(10);
        assert_eq!(batch[0].payload, "first");
        assert_eq!(batch[1].payload, "second");
        assert_eq!(q.stats().total_dropped, 1);
    }

    #[test]
    fn overflow_drop_low_priority() {
        let mut q = TelemetryQueue::new(2, OverflowPolicy::DropLowPriority);
        q.enqueue(TelemetryCategory::SystemMetrics, 1000, "low-prio"); // priority 40
        q.enqueue(TelemetryCategory::SecurityAudit, 1001, "high-prio"); // priority 100
                                                                        // 队列满，新事件是 AlertEvents (priority 80) > SystemMetrics (40)
        q.enqueue(TelemetryCategory::AlertEvents, 1002, "medium-prio");

        assert_eq!(q.len(), 2);
        let batch = q.dequeue_batch(10);
        // 应丢弃 SystemMetrics (lowest priority)
        assert_eq!(batch[0].payload, "high-prio");
        assert_eq!(batch[1].payload, "medium-prio");
    }

    #[test]
    fn overflow_drop_low_priority_rejects_when_all_higher() {
        let mut q = TelemetryQueue::new(2, OverflowPolicy::DropLowPriority);
        q.enqueue(TelemetryCategory::SecurityAudit, 1000, "highest"); // 100
        q.enqueue(TelemetryCategory::AlertEvents, 1001, "high"); // 80
                                                                 // 新事件是 SystemMetrics (40) < both existing
        let result = q.enqueue(TelemetryCategory::SystemMetrics, 1002, "low");

        assert!(result.is_none());
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn dequeue_batch_respects_max_count() {
        let mut q = TelemetryQueue::new(100, OverflowPolicy::DropOldest);
        for i in 0..10 {
            q.enqueue(TelemetryCategory::SystemMetrics, 1000 + i, "event");
        }
        let batch = q.dequeue_batch(3);
        assert_eq!(batch.len(), 3);
        assert_eq!(q.len(), 7);
    }

    #[test]
    fn dequeue_filtered_only_allowed_categories() {
        let mut q = TelemetryQueue::new(100, OverflowPolicy::DropOldest);
        q.enqueue(TelemetryCategory::SystemMetrics, 1000, "sys");
        q.enqueue(TelemetryCategory::SecurityAudit, 1001, "sec");
        q.enqueue(TelemetryCategory::AlertEvents, 1002, "alert");
        q.enqueue(TelemetryCategory::SystemMetrics, 1003, "sys2");

        let allowed = [
            TelemetryCategory::SystemMetrics,
            TelemetryCategory::AlertEvents,
        ];
        let batch = q.dequeue_filtered(10, &allowed);
        assert_eq!(batch.len(), 3); // 2 system + 1 alert
        assert!(batch.iter().all(|e| allowed.contains(&e.category)));
        // SecurityAudit 应保留在队列中
        assert_eq!(q.len(), 1);
        assert_eq!(q.peek().unwrap().category, TelemetryCategory::SecurityAudit);
    }

    #[test]
    fn stats_track_correctly() {
        let mut q = TelemetryQueue::new(3, OverflowPolicy::DropOldest);
        q.enqueue(TelemetryCategory::SystemMetrics, 1000, "1");
        q.enqueue(TelemetryCategory::SystemMetrics, 1001, "2");
        q.enqueue(TelemetryCategory::SystemMetrics, 1002, "3");
        q.enqueue(TelemetryCategory::SystemMetrics, 1003, "4"); // overflow

        let stats = q.stats();
        assert_eq!(stats.total_enqueued, 4);
        assert_eq!(stats.total_dropped, 1);
        assert_eq!(stats.current_len, 3);
        assert_eq!(stats.capacity, 3);

        q.dequeue_batch(2);
        let stats = q.stats();
        assert_eq!(stats.total_dequeued, 2);
        assert_eq!(stats.current_len, 1);
    }

    #[test]
    fn clear_empties_queue() {
        let mut q = TelemetryQueue::new(100, OverflowPolicy::DropOldest);
        q.enqueue(TelemetryCategory::SystemMetrics, 1000, "data");
        q.clear();
        assert!(q.is_empty());
    }

    #[test]
    fn event_ids_increment() {
        let mut q = TelemetryQueue::new(100, OverflowPolicy::DropOldest);
        let id1 = q
            .enqueue(TelemetryCategory::SystemMetrics, 1000, "1")
            .unwrap();
        let id2 = q
            .enqueue(TelemetryCategory::SystemMetrics, 1001, "2")
            .unwrap();
        assert!(id2 > id1);
    }

    #[test]
    fn priority_ordering_correct() {
        let audit = TelemetryEvent {
            id: 1,
            category: TelemetryCategory::SecurityAudit,
            timestamp: 0,
            payload: String::new(),
        };
        let metrics = TelemetryEvent {
            id: 2,
            category: TelemetryCategory::SystemMetrics,
            timestamp: 0,
            payload: String::new(),
        };
        assert!(audit.priority() > metrics.priority());
    }
}
