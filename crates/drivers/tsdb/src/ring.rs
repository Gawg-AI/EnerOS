//! 22 §2.W3.3：背压环形缓冲（锁定 2——有界缓冲 + 显式丢弃策略）。
//!
//! 写入路径：生产者 → [`RingBuffer`]（容量配置化）→ 独立消费任务
//! （调用方 drain）→ 块设备。满时 `DropOldest`（默认，保最新）+ 丢弃计数
//! 入指标（`tsdb.dropped_points`）。禁止无界增长。

use alloc::collections::VecDeque;

/// 入队结果（显式丢弃策略可观测）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    /// 入队成功。
    Ok,
    /// 缓冲满：按 DropOldest 丢弃最旧并计数（保最新）。
    DroppedOldest,
}

/// 有界环形缓冲（FIFO；容量固定，满时 DropOldest）。
#[derive(Debug, Clone)]
pub struct RingBuffer<T> {
    /// 容量（配置化；构造后固定）。
    capacity: usize,
    /// FIFO 存储（VecDeque 天然支持前端弹出；容量由 push 逻辑保证）。
    buf: VecDeque<T>,
    /// 丢弃计数（指标 `tsdb.dropped_points` 源数据）。
    dropped: u64,
}

impl<T> RingBuffer<T> {
    /// 新建（容量 > 0；`capacity = 0` 回退 1——不允许不可用缓冲）。
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            capacity,
            buf: VecDeque::with_capacity(capacity),
            dropped: 0,
        }
    }

    /// 入队：满时 DropOldest（保最新）+ 计数；否则入队。
    pub fn push(&mut self, item: T) -> PushOutcome {
        if self.buf.len() >= self.capacity {
            self.buf.pop_front();
            self.dropped += 1;
            self.buf.push_back(item);
            PushOutcome::DroppedOldest
        } else {
            self.buf.push_back(item);
            PushOutcome::Ok
        }
    }

    /// 消费队首（独立写入任务调用；None = 空）。
    pub fn pop(&mut self) -> Option<T> {
        self.buf.pop_front()
    }

    /// 丢弃计数（指标）。
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// 当前占用。
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// 容量。
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 有界：超容量入队 → DropOldest + 计数，缓冲不增长。
    #[test]
    fn test_ring_bounded_drop_oldest() {
        let mut ring = RingBuffer::new(3);
        assert_eq!(ring.push(1), PushOutcome::Ok);
        assert_eq!(ring.push(2), PushOutcome::Ok);
        assert_eq!(ring.push(3), PushOutcome::Ok);
        assert_eq!(ring.push(4), PushOutcome::DroppedOldest);
        assert_eq!(ring.dropped(), 1);
        assert_eq!(ring.len(), 3, "缓冲必须保持有界");
        // 保最新：队首为 2（1 被丢弃）。
        assert_eq!(ring.pop(), Some(2));
        assert_eq!(ring.pop(), Some(3));
        assert_eq!(ring.pop(), Some(4));
        assert!(ring.is_empty());
    }

    /// 容量配置化（不同容量）。
    #[test]
    fn test_ring_configurable_capacity() {
        let mut ring = RingBuffer::new(64);
        for i in 0..100 {
            ring.push(i);
        }
        assert_eq!(ring.dropped(), 36);
        assert_eq!(ring.len(), 64);
        assert_eq!(ring.pop(), Some(36), "保最新语义：0..35 被丢弃");
    }

    /// kHz 摄入压测：容量 1024、10 万点——生产者不阻塞（push 恒返回），
    /// 丢弃可观测，缓冲有界。
    #[test]
    fn test_ring_khz_ingest_pressure() {
        let mut ring = RingBuffer::new(1024);
        for i in 0..100_000u64 {
            let _ = ring.push(i);
        }
        assert_eq!(ring.len(), 1024);
        assert_eq!(ring.dropped(), 100_000 - 1024);
        // 消费端可继续 drain（不丢未满数据之外的一切——丢弃已计数）。
        let mut drained = 0u64;
        while ring.pop().is_some() {
            drained += 1;
        }
        assert_eq!(drained, 1024);
    }

    /// 零容量回退（不可用缓冲拒绝）。
    #[test]
    fn test_ring_zero_capacity_fallback() {
        let ring: RingBuffer<u8> = RingBuffer::new(0);
        assert_eq!(ring.capacity(), 1);
    }
}
