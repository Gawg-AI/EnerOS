//! v0.247.0（C-247）：遥测队列并发模型验证（loom，host 侧）。
//!
//! 生产 `TelemetryQueue` 为 `&mut self` 单线程 API；并发访问由上层经
//! Mutex 串行化。本模型验证经 `loom::sync::Mutex` 串行化后的**队列
//! 不变量**：
//! - 事件 ID 全局唯一（单调递增，无重复）；
//! - 队列长度不超过容量（有界语义）；
//! - 并发 push 的丢事件计数与溢出策略一致。

extern crate std;

use loom::sync::Mutex;
use std::vec::Vec;

/// 与生产 `TelemetryQueue` 有界语义同构的 loom 复刻。
struct LoomTelemetryQueue {
    events: Vec<u64>,
    capacity: usize,
    next_id: u64,
    dropped: u64,
}

impl LoomTelemetryQueue {
    fn new(capacity: usize) -> Self {
        Self {
            events: Vec::new(),
            capacity,
            next_id: 1,
            dropped: 0,
        }
    }

    /// push 一个事件；队列满时丢弃最旧（DropOldest 策略，与生产一致）。
    /// 返回事件 ID 或 None（不可达——容量 ≥ 1 时总是腾出空间）。
    fn push(&mut self) -> Option<u64> {
        if self.events.len() >= self.capacity {
            self.events.remove(0);
            self.dropped += 1;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.events.push(id);
        Some(id)
    }

    /// 排空全部事件。
    fn drain_all(&mut self) -> Vec<u64> {
        core::mem::take(&mut self.events)
    }
}

/// 并发 push：事件 ID 必须全局唯一、队列长度 ≤ 容量。
#[test]
fn vrf_009_concurrent_push_unique_ids_bounded() {
    loom::model(|| {
        let queue = loom::sync::Arc::new(Mutex::new(LoomTelemetryQueue::new(4)));

        let mut handles = Vec::new();
        for _ in 0..2 {
            let q = queue.clone();
            handles.push(loom::thread::spawn(move || {
                let mut g = q.lock().unwrap();
                let mut ids = Vec::new();
                for _ in 0..3 {
                    ids.push(g.push().unwrap());
                }
                ids
            }));
        }
        let mut all: Vec<u64> = Vec::new();
        for h in handles {
            all.extend(h.join().unwrap());
        }
        let q = queue.lock().unwrap();
        assert!(
            q.events.len() <= 4,
            "队列有界：长度 {0} > 容量 4",
            q.events.len()
        );
        // ID 唯一性：去重后数量不变。
        let mut sorted = all.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), all.len(), "事件 ID 必须全局唯一");
        // 守恒：丢弃数 + 现存数 = 总入队数（每个 push 要么现存要么丢弃）。
        let total_pushed = 6u64;
        assert_eq!(
            q.dropped + q.events.len() as u64,
            total_pushed,
            "DropOldest 下丢弃+现存必须等于总入队数"
        );
        assert_eq!(q.dropped, total_pushed - 4, "容量 4、入队 6 → 丢弃 2");
    });
}

/// 并发 push + drain：drain 后队列为空，ID 连续无空洞。
#[test]
fn vrf_009_push_then_drain_no_gap() {
    loom::model(|| {
        let queue = loom::sync::Arc::new(Mutex::new(LoomTelemetryQueue::new(8)));
        let q1 = queue.clone();
        let producer = loom::thread::spawn(move || {
            let mut g = q1.lock().unwrap();
            for _ in 0..4 {
                g.push();
            }
        });
        let q2 = queue.clone();
        let consumer = loom::thread::spawn(move || {
            let mut g = q2.lock().unwrap();
            g.drain_all()
        });
        producer.join().unwrap();
        let drained = consumer.join().unwrap();
        let q = queue.lock().unwrap();
        // 不变式（loom 探索所有交错，producer/consumer 顺序不定）：
        // drain 取走的与队列剩余的合并 = 全部已分配 ID，且 ID 互不重复。
        let mut all = drained.clone();
        all.extend(q.events.iter().copied());
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 4, "4 个事件 ID 全部存在且唯一");
        for (i, id) in all.iter().enumerate() {
            assert_eq!(*id, (i + 1) as u64, "ID 从 1 起连续无空洞");
        }
    });
}
