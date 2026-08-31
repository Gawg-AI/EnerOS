//! # 蓝图 61 §12 v0.314.48：容量测试基底（§3.5/§3.6）
//!
//! 「先建立负载生成器和 oracle，禁止先调大常数」（§3.6.1）：
//!
//! - [`LoadGen`]：确定性事件/命令意图流（按 OperationalProfile 速率），
//!   输出 `(key, seq, payload)` 供分片/队列消费；
//! - [`QueuedSink`]：有界队列模拟器——depth/capacity/enqueued/dequeued/
//!   dropped/rejected/oldest_age/high_watermark/retry_count 全观测
//!   （蓝图 03 §4 队列八指标）；
//! - [`Oracle`]：与 reference 模型等价性比对——分片消费后结果必须与
//!   未分片参考一致（幂等键 + 排序），差距即为缺陷。
//!
//! C2/C3 目标硬件容量证据（24h/突发/磁盘水位）登记 BLOCKED——本模块
//! 是 host 侧可复现的失配检测面，真实容量门槛见登记文档。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// 负载事件（Q0-Q3 任一类）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadEvent {
    pub key: u64,
    pub seq: u64,
    pub class: u8,
    pub payload: [u8; 32],
}

/// 确定性负载生成器：按键序+类分布生成事件流（无随机种子依赖）。
pub struct LoadGen {
    phase: u64,
    classes: Vec<u8>,
    counts: BTreeMap<u64, u64>,
}

impl LoadGen {
    pub fn new(phase_seed: u64, classes: &[u8]) -> Self {
        Self {
            phase: phase_seed,
            classes: classes.to_vec(),
            counts: BTreeMap::new(),
        }
    }

    /// 下一事件（key 在 `key_count` 内按相位轮转；seq 按 key 单调）。
    pub fn next(&mut self, key_count: u64) -> LoadEvent {
        let k = self.phase % key_count.max(1);
        let seq = self.counts.entry(k).or_insert(0);
        *seq += 1;
        let class = self.classes[(self.phase as usize) % self.classes.len().max(1)];
        let mut payload = [0u8; 32];
        payload[..8].copy_from_slice(&self.phase.to_be_bytes());
        payload[8..16].copy_from_slice(&k.to_be_bytes());
        self.phase += 1;
        LoadEvent {
            key: k,
            seq: *seq,
            class,
            payload,
        }
    }
}

/// 有界队列（Q0-Q3 策略差异由参数化 drop 策略体现）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropPolicy {
    /// Q3：丢弃最旧。
    DropOldest,
    /// Q2/Q1：拒绝新入队（必须计数）。
    RejectNew,
    /// Q0：禁止任何丢弃（满即安全降级信号）。
    FailClosed,
}

/// 有界队列观测（蓝图 03 §4 八指标）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueueStats {
    pub enqueued: u64,
    pub dequeued: u64,
    pub dropped: u64,
    pub rejected: u64,
    pub depth: u64,
    pub capacity: u64,
    pub high_watermark: u64,
    pub oldest_age: u64,
    pub retry_count: u64,
}

/// 有界队列模拟器（无界 VecDeque 禁令的落地面）。
pub struct QueuedSink {
    policy: DropPolicy,
    items: VecDequeOwn<(u64, u64)>,
    stats: QueueStats,
    /// key → 最后序列（幂等去重；重放检测）。
    last_seq: BTreeMap<u64, u64>,
}

/// 简化双端队列（定长 Vec 环，避免依赖）。
struct VecDequeOwn<T> {
    buf: Vec<Option<T>>,
    head: usize,
    len: usize,
}

impl<T: Clone> VecDequeOwn<T> {
    fn with_capacity(cap: usize) -> Self {
        let mut buf = Vec::with_capacity(cap);
        buf.resize_with(cap, || None);
        Self {
            buf,
            head: 0,
            len: 0,
        }
    }
    fn len(&self) -> usize {
        self.len
    }
    fn is_full(&self) -> bool {
        self.len == self.buf.len()
    }
    fn push_back(&mut self, v: T) -> bool {
        if self.is_full() {
            return false;
        }
        let idx = (self.head + self.len) % self.buf.len();
        self.buf[idx] = Some(v);
        self.len += 1;
        true
    }
    fn pop_front(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let v = self.buf[self.head].take();
        self.head = (self.head + 1) % self.buf.len();
        self.len -= 1;
        v
    }
    fn drop_oldest(&mut self) -> Option<T> {
        self.pop_front()
    }
}

impl QueuedSink {
    pub fn new(capacity: usize, policy: DropPolicy) -> Self {
        Self {
            policy,
            items: VecDequeOwn::with_capacity(capacity.max(1)),
            stats: QueueStats {
                capacity: capacity.max(1) as u64,
                ..QueueStats::default()
            },
            last_seq: BTreeMap::new(),
        }
    }

    /// 入队（幂等：同 key 同 seq 的重放视为重复，不入队不计数）。
    /// 返回 true=入队成功；false=拒绝/丢弃。
    pub fn enqueue(&mut self, key: u64, seq: u64) -> bool {
        let last = self.last_seq.get(&key).copied().unwrap_or(0);
        if seq <= last {
            return false; // 重放（幂等）：不计数、不入队。
        }
        if self.items.is_full() {
            match self.policy {
                DropPolicy::DropOldest => {
                    self.items.drop_oldest();
                    self.stats.dropped += 1;
                    self.stats.depth = self.items.len() as u64;
                    self.stats.high_watermark = self.stats.high_watermark.max(self.stats.depth);
                }
                DropPolicy::RejectNew => {
                    self.stats.rejected += 1;
                    return false;
                }
                DropPolicy::FailClosed => {
                    self.stats.rejected += 1;
                    return false; // 安全降级信号由上层读取 stats.rejected。
                }
            }
        }
        self.items.push_back((key, seq));
        self.last_seq.insert(key, seq);
        self.stats.enqueued += 1;
        self.stats.depth = self.items.len() as u64;
        self.stats.high_watermark = self.stats.high_watermark.max(self.stats.depth);
        if self.stats.depth > 0 {
            self.stats.oldest_age += 1;
        }
        true
    }

    pub fn dequeue(&mut self) -> Option<(u64, u64)> {
        let v = self.items.pop_front();
        if v.is_some() {
            self.stats.dequeued += 1;
            self.stats.depth = self.items.len() as u64;
        }
        v
    }

    pub fn stats(&self) -> &QueueStats {
        &self.stats
    }

    pub fn is_empty(&self) -> bool {
        self.items.len() == 0
    }
}

/// 参考模型：无分片顺序消费 = Oracle（幂等去重 + 全量顺序）。
pub struct Oracle {
    seen: BTreeMap<u64, u64>,
    pub consumed: u64,
}

impl Oracle {
    pub fn new() -> Self {
        Self {
            seen: BTreeMap::new(),
            consumed: 0,
        }
    }

    /// 消费一事件；返回 true=新事件（非重放）。
    pub fn consume(&mut self, key: u64, seq: u64) -> bool {
        let last = self.seen.get(&key).copied().unwrap_or(0);
        if seq <= last {
            return false;
        }
        self.seen.insert(key, seq);
        self.consumed += 1;
        true
    }
}

impl Default for Oracle {
    fn default() -> Self {
        Self::new()
    }
}

/// 等价性比对：分片结果 vs 参考。失配（缺少/多余/乱序）即缺陷。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EquivalenceReport {
    pub reference_consumed: u64,
    pub sharded_consumed: u64,
    pub missing: u64,
    pub unexpected: u64,
    pub order_violations: u64,
}

/// 比对两个按 key 单调的消费记录。
pub fn equivalence_check(
    reference: &BTreeMap<u64, u64>,
    sharded: &BTreeMap<u64, u64>,
) -> EquivalenceReport {
    let mut report = EquivalenceReport {
        reference_consumed: reference.len() as u64,
        sharded_consumed: sharded.len() as u64,
        missing: 0,
        unexpected: 0,
        order_violations: 0,
    };
    for (k, seq) in reference {
        match sharded.get(k) {
            None => report.missing += 1,
            Some(s) if s < seq => report.missing += 1,
            Some(s) if s > seq => report.unexpected += 1,
            Some(_) => {}
        }
    }
    for (k, s) in sharded {
        if !reference.contains_key(k) {
            report.unexpected += 1;
        }
        // 顺序违规：分片内 seq 必须与参考相同（同 key 单消费者保证顺序）。
        if let Some(r) = reference.get(k) {
            if r < s {
                report.order_violations += 1;
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]
    use super::*;

    #[test]
    fn load_gen_deterministic() {
        let mut a = LoadGen::new(7, &[0, 1, 2]);
        let mut b = LoadGen::new(7, &[0, 1, 2]);
        for _ in 0..1000 {
            assert_eq!(a.next(50), b.next(50));
        }
    }

    #[test]
    fn queue_stats_all_observable() {
        let mut q = QueuedSink::new(3, DropPolicy::RejectNew);
        assert!(q.enqueue(1, 1));
        assert!(q.enqueue(1, 2));
        assert!(q.enqueue(2, 1));
        // 满 → 拒绝。
        assert!(!q.enqueue(3, 1));
        assert_eq!(q.stats().rejected, 1);
        // 重放 → 幂等不入队不计数。
        assert!(!q.enqueue(1, 1));
        assert_eq!(q.stats().enqueued, 3);
        let (k, s) = q.dequeue().expect("item");
        assert_eq!((k, s), (1, 1));
        assert_eq!(q.stats().dequeued, 1);
        assert_eq!(q.stats().depth, 2);
        assert_eq!(q.stats().high_watermark, 3);
        assert!(q.stats().oldest_age > 0);
    }

    #[test]
    fn drop_oldest_policy_counted() {
        let mut q = QueuedSink::new(2, DropPolicy::DropOldest);
        q.enqueue(1, 1);
        q.enqueue(2, 1);
        q.enqueue(3, 1); // 丢 1,1
        assert_eq!(q.stats().dropped, 1);
        let (k, _) = q.dequeue().expect("2");
        assert_eq!(k, 2);
    }

    #[test]
    fn equivalence_detects_missing_and_extra() {
        let mut ref_map = BTreeMap::new();
        ref_map.insert(1u64, 5u64);
        ref_map.insert(2u64, 3u64);
        let mut sharded = BTreeMap::new();
        sharded.insert(1u64, 5u64);
        sharded.insert(9u64, 1u64);
        let rep = equivalence_check(&ref_map, &sharded);
        assert_eq!(rep.missing, 1, "key 2 缺失");
        assert_eq!(rep.unexpected, 1, "key 9 多余");
        // 全等。
        let rep2 = equivalence_check(&ref_map, &ref_map);
        assert_eq!(rep2.missing + rep2.unexpected + rep2.order_violations, 0);
    }
}
