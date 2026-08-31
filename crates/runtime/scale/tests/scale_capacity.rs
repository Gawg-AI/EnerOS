//! WP-7.2 容量基底：分片消费 vs 参考模型等价 + 慢消费者/满队列可观测
//! （蓝图 61 §12 §3.5/§3.6「先建立负载生成器和 oracle，禁止先调大常数」）。

#![allow(clippy::disallowed_macros)]
use std::collections::BTreeMap;

use eneros_scale::capacity::{equivalence_check, DropPolicy, LoadGen, Oracle, QueuedSink};
use eneros_scale::shard::{ShardKey, ShardRing};

#[test]
fn sharded_consumption_matches_reference_oracle() {
    // 参考模型：单消费者顺序消费全部键。
    let mut oracle = Oracle::new();
    let mut gen = LoadGen::new(1, &[0, 1, 2, 3]);
    // 分片模型：8 分片、每分片一个有界队列。
    let ring = ShardRing::new(4096, 8, 1).expect("ring");
    let mut sharded = BTreeMap::new(); // seq_key → seq
    let mut queues: Vec<QueuedSink> = (0..8)
        .map(|_| QueuedSink::new(2048, DropPolicy::DropOldest))
        .collect();
    let mut ref_map = BTreeMap::new();
    for _ in 0..40_000 {
        let e = gen.next(1_024);
        // 参考全量消费（幂等去重）。
        if oracle.consume(e.key, e.seq) {
            ref_map.insert(e.key, e.seq);
        }
        // 分片：路由入队 + 排空消费。
        let k = ShardKey {
            tenant: 1,
            site: (e.key % 100) as u32,
            source: (e.key / 100) as u32,
        };
        let r = ring.route(&k);
        queues[r.shard as usize].enqueue(e.key, e.seq);
        if let Some((key, seq)) = queues[r.shard as usize].dequeue() {
            sharded.insert(key, seq);
        }
    }
    // 排空剩余。
    for q in queues.iter_mut() {
        while let Some((k, s)) = q.dequeue() {
            sharded.insert(k, s);
        }
    }
    // 等价性：sharded 是 ref_map 的子集（同键同 seq；通道无丢失）。
    let rep = equivalence_check(&ref_map, &sharded);
    // 由于每键只有 1 个消费者且 seq 单调，缺失 = 0（DropOldest 只丢最旧
    // 但同键最新仍在）。
    assert_eq!(rep.missing, 0, "分片消费不得丢键: {rep:?}");
    assert_eq!(rep.unexpected, 0, "不得出现参考外键: {rep:?}");
    assert_eq!(rep.order_violations, 0, "不得乱序: {rep:?}");
}

#[test]
fn slow_consumer_observable_not_silent() {
    // 慢消费者：消费速率 < 生产速率 → 队列满 → 丢弃被计数（DropOldest）
    // 或拒绝被计数（RejectNew），绝不静默。
    let mut gen = LoadGen::new(2, &[3, 3, 3, 3]); // Q3 全部
    let mut q = QueuedSink::new(64, DropPolicy::DropOldest);
    for _ in 0..10_000 {
        let e = gen.next(50);
        q.enqueue(e.key, e.seq);
        // 慢消费者：每 5 事件消费 1。
        if e.seq % 5 == 0 {
            q.dequeue();
        }
    }
    let s = *q.stats();
    assert!(s.high_watermark <= s.capacity, "高水位不得超容量");
    assert!(
        s.dropped > 0 || s.rejected > 0,
        "慢消费者必须产生可观测丢弃/拒绝"
    );
    // 守恒：深度 = 入队 − 出队 − 丢弃；生产 = 入队 + 拒绝（无静默）。
    assert_eq!(
        s.enqueued - s.dequeued - s.dropped,
        s.depth,
        "计数闭合（深度=入队-出队-丢弃）"
    );
    assert_eq!(s.enqueued + s.rejected, 10_000, "生产=入队+拒绝（无静默）");
}

#[test]
fn load_gen_scale_100k_events_bounded() {
    // C1 参考量级（10k 点、稳定流）的 host 侧压测面：100k 事件全流程
    // 无 panic、全部计数闭合。
    let mut gen = LoadGen::new(9, &[0, 1, 2, 3]);
    let mut q = QueuedSink::new(4096, DropPolicy::DropOldest);
    let mut total = 0u64;
    for _ in 0..100_000 {
        let e = gen.next(10_000);
        if q.enqueue(e.key, e.seq) {
            total += 1;
        }
        if e.seq % 50 == 0 {
            while q.dequeue().is_some() {}
        }
    }
    assert_eq!(total, 100_000, "100k 事件全部入队（无静默吞）");
    assert_eq!(q.stats().enqueued + q.stats().rejected, 100_000);
}
