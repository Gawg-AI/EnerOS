//! 指数退避重试队列与有界死信队列（D6/D10）。
//!
//! 退避公式：`min(base << retry_count, 300_000) + xorshift32 确定性抖动`，
//! 抖动区间 `[0, base]`（D6：同 retry_count 同结果，测试可断言，零依赖零状态）。
//! 超 `max_retries` 批次移入死信队列（有界 8 批，溢出丢最旧死信）并计数（D10）。

use alloc::collections::VecDeque;

use crate::delta_sync::SyncBatch;

/// 死信队列容量上限（D10 有界死信，溢出丢最旧）。
const DEAD_LETTER_CAPACITY: usize = 8;

/// 退避封顶（ms，蓝图 §4.5：5 分钟）。
const BACKOFF_CAP_MS: u64 = 300_000;

/// xorshift32 伪随机（D6 确定性抖动源；seed 非零）。
fn xorshift32(mut x: u32) -> u32 {
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    x
}

/// 指数退避重试队列（D10：生产路径零 unwrap）。
pub struct RetryQueue {
    /// 待重试批次（FIFO，仅检查队首）。
    pending: VecDeque<SyncBatch>,
    /// 单批最大重试次数。
    max_retries: u32,
    /// 退避基数（ms）。
    backoff_base_ms: u64,
    /// 死信队列（有界 8 批）。
    dead_letters: VecDeque<SyncBatch>,
    /// 累计死信批次数（含被丢弃的旧死信）。
    dead_letter_count: u64,
}

impl RetryQueue {
    /// 构造重试队列：`max_retries` 次后转死信，`backoff_base_ms` 为退避基数。
    pub fn new(max_retries: u32, backoff_base_ms: u64) -> Self {
        Self {
            pending: VecDeque::new(),
            max_retries,
            backoff_base_ms,
            dead_letters: VecDeque::new(),
            dead_letter_count: 0,
        }
    }

    /// 批次入队尾。
    pub fn enqueue(&mut self, batch: SyncBatch) {
        self.pending.push_back(batch);
    }

    /// 检查队首批次（D7 时间注入）：
    /// - `retry_count >= max_retries` → 移死信（有界 8，丢最旧）+ 计数，返回 `None`；
    /// - `now - created_at >= calculate_backoff(retry_count)` → 弹出且
    ///   `retry_count += 1`，返回 `Some`；
    /// - 否则返回 `None`。
    pub fn retry_pending(&mut self, now: u64) -> Option<SyncBatch> {
        let front = self.pending.front()?;
        if front.retry_count >= self.max_retries {
            if let Some(batch) = self.pending.pop_front() {
                if self.dead_letters.len() >= DEAD_LETTER_CAPACITY {
                    self.dead_letters.pop_front();
                }
                self.dead_letters.push_back(batch);
                self.dead_letter_count += 1;
            }
            return None;
        }
        let backoff = self.calculate_backoff(front.retry_count);
        if now.saturating_sub(front.created_at) >= backoff {
            if let Some(mut batch) = self.pending.pop_front() {
                batch.retry_count += 1;
                return Some(batch);
            }
        }
        None
    }

    /// 指数退避（D6）：`min(base << retry_count, 300_000)` + 确定性抖动
    /// `xorshift32(retry_count × 2654435761 | 1) mod (base + 1)`，
    /// 结果区间 `[exp, exp + base]`。
    pub fn calculate_backoff(&self, retry_count: u32) -> u64 {
        // saturating_*：retry_count 极大时值溢出饱和到 u64::MAX，再封顶 300s
        let exp = self
            .backoff_base_ms
            .saturating_mul(2u64.saturating_pow(retry_count))
            .min(BACKOFF_CAP_MS);
        let jitter = u64::from(xorshift32(retry_count.wrapping_mul(2_654_435_761) | 1))
            % (self.backoff_base_ms + 1);
        exp + jitter
    }

    /// 待重试批次数。
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// 累计死信批次数（D12 可观测）。
    pub fn dead_letter_count(&self) -> u64 {
        self.dead_letter_count
    }

    /// 死信队列引用（容量 ≤ 8）。
    pub fn dead_letters(&self) -> &VecDeque<SyncBatch> {
        &self.dead_letters
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_store::Event;
    use crate::EventType;

    /// 构造测试批次（指定 batch_id / retry_count / created_at）。
    fn make_batch(batch_id: u64, retry_count: u32, created_at: u64) -> SyncBatch {
        SyncBatch {
            batch_id,
            events: alloc::vec![Event {
                offset: batch_id,
                timestamp: created_at,
                event_type: EventType::Telemetry,
                payload: alloc::vec![1, 2, 3],
                checksum: 0,
                synced: false,
            }],
            from_offset: batch_id,
            to_offset: batch_id,
            retry_count,
            created_at,
        }
    }

    /// RQ17 enqueue + 退避未到 → None。
    #[test]
    fn rq17_enqueue_backoff_not_due() {
        let mut q = RetryQueue::new(3, 1000);
        q.enqueue(make_batch(0, 0, 1000));
        assert_eq!(q.pending_len(), 1);
        // backoff(0) >= 1000 > 500 → 未到期
        assert!(q.retry_pending(1500).is_none());
        assert_eq!(q.pending_len(), 1);
    }

    /// RQ18 退避到 → 出队 retry_count+1。
    #[test]
    fn rq18_backoff_due_dequeue() {
        let mut q = RetryQueue::new(3, 1000);
        q.enqueue(make_batch(7, 0, 1000));
        let batch = q.retry_pending(1_000_000).unwrap();
        assert_eq!(batch.batch_id, 7);
        assert_eq!(batch.retry_count, 1);
        assert_eq!(q.pending_len(), 0);
    }

    /// RQ19 超 max_retries → 死信 + 计数。
    #[test]
    fn rq19_over_max_retries_dead_letter() {
        let mut q = RetryQueue::new(2, 1000);
        q.enqueue(make_batch(0, 2, 1000));
        assert!(q.retry_pending(1_000_000).is_none());
        assert_eq!(q.pending_len(), 0);
        assert_eq!(q.dead_letter_count(), 1);
        assert_eq!(q.dead_letters().len(), 1);
        assert_eq!(q.dead_letters()[0].batch_id, 0);
    }

    /// RQ20 死信有界 8 丢最旧。
    #[test]
    fn rq20_dead_letter_bounded_drop_oldest() {
        let mut q = RetryQueue::new(0, 1000);
        for i in 0..10u64 {
            q.enqueue(make_batch(i, 0, 0));
            assert!(q.retry_pending(0).is_none());
        }
        assert_eq!(q.dead_letter_count(), 10);
        assert_eq!(q.dead_letters().len(), 8);
        // 最旧 2 批（batch_id 0/1）已丢弃，队首为 batch_id=2
        assert_eq!(q.dead_letters()[0].batch_id, 2);
        assert_eq!(q.dead_letters()[7].batch_id, 9);
    }

    /// RQ21 指数退避 1/2/4s…封顶 300s（蓝图 §4.5）。
    #[test]
    fn rq21_backoff_exponential_capped() {
        let q = RetryQueue::new(32, 1000);
        assert!(q.calculate_backoff(0) >= 1000);
        assert!(q.calculate_backoff(1) >= 2000);
        assert!(q.calculate_backoff(2) >= 4000);
        assert!(q.calculate_backoff(8) >= 256_000);
        // 封顶 300s（retry_count=20：1000 << 20 远超上限）
        let b20 = q.calculate_backoff(20);
        assert!((300_000..=300_000 + 1000).contains(&b20));
        // 极大 retry_count 不移位溢出
        let b63 = q.calculate_backoff(63);
        assert!((300_000..=300_000 + 1000).contains(&b63));
    }

    /// RQ22 抖动确定性（同 retry_count 同结果）+ 区间 [exp, exp+base]。
    #[test]
    fn rq22_jitter_deterministic_bounded() {
        let q = RetryQueue::new(32, 1000);
        for rc in 0..6u32 {
            assert_eq!(q.calculate_backoff(rc), q.calculate_backoff(rc));
            let exp = (1000u64 << rc).min(300_000);
            let b = q.calculate_backoff(rc);
            assert!(
                (exp..=exp + 1000).contains(&b),
                "rc={} backoff={} 越界",
                rc,
                b
            );
        }
    }

    /// RQ23 max_retries=0 → 立即死信。
    #[test]
    fn rq23_zero_max_retries_immediate_dead_letter() {
        let mut q = RetryQueue::new(0, 1000);
        q.enqueue(make_batch(3, 0, 0));
        assert!(q.retry_pending(0).is_none());
        assert_eq!(q.pending_len(), 0);
        assert_eq!(q.dead_letter_count(), 1);
        assert_eq!(q.dead_letters()[0].batch_id, 3);
    }
}
