//! v0.101.0 断网处理与孤岛模式：泛型事件缓存。
//!
//! 蓝图 phase2.md §v0.101.0：断网/孤岛期间本地产生的业务事件先缓存，
//! 待恢复同步后上传；缓存容量有界，满时丢弃最旧事件并告警。
//!
//! ## 设计要点
//!
//! - **泛型化解耦**（D5）：[`EventCache<T>`] 不耦合 v0.96.0 `EventRecord`，
//!   eneros-federation 保持仅依赖 eneros-crypto（SBOM 不变），避免 agents
//!   子系统内横向耦合；上层以任意事件类型实例化。
//! - **环形语义**（蓝图 §4.4）：`VecDeque` 队首最旧、队尾最新；队列满时
//!   `pop_front` 丢弃最旧，再 `push_back` 新事件入队尾。
//! - **溢出可观测**（D7）：no_std 无 log crate，"丢弃最旧并告警"落地为
//!   `overflow_count` 计数器字段化；`clear` 清空事件但保留计数（历史
//!   可观测不归零，C21）。

use alloc::collections::VecDeque;

/// 事件缓存（蓝图 §4.1 EventCache，D5 泛型化）
#[derive(Debug, Clone)]
pub struct EventCache<T> {
    /// 事件队列（队首最旧）
    pub events: VecDeque<T>,
    /// 最大容量
    pub max_size: usize,
    /// 溢出丢弃计数（D7 可观测，clear 不归零）
    pub overflow_count: u64,
}

impl<T> EventCache<T> {
    /// 创建缓存（`max_size == 0` 时退化为每次 push 都丢弃最旧并计数——
    /// 属配置错误容忍，生产 `max_size >= 1`）
    pub fn new(max_size: usize) -> Self {
        Self {
            events: VecDeque::new(),
            max_size,
            overflow_count: 0,
        }
    }

    /// 入队：`len >= max_size` → `pop_front` 丢弃最旧 + `overflow_count += 1`，
    /// 再 `push_back`（蓝图 §4.4）
    pub fn push(&mut self, e: T) {
        if self.events.len() >= self.max_size {
            self.events.pop_front();
            self.overflow_count += 1;
        }
        self.events.push_back(e);
    }

    /// 当前缓存事件数
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// 缓存为空时为 true
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// 清空事件，保留 `overflow_count`（历史可观测不归零，C21）
    pub fn clear(&mut self) {
        self.events.clear();
    }
}

// ============================================================
// Unit Tests TC1~TC7
// ============================================================

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use super::*;

    /// TC7 用自定义事件类型（验证泛型对非整数类型实例化）
    #[derive(Debug, Clone, PartialEq)]
    struct TestEvent {
        ts: u64,
        kind: u8,
    }

    /// 队首→队尾顺序收集为 Vec（断言辅助）
    fn collect<T: Clone>(c: &EventCache<T>) -> Vec<T> {
        c.events.iter().cloned().collect()
    }

    // TC1: new(10) 初始：events 空 / max_size==10 / overflow_count==0 / is_empty()
    #[test]
    fn tc1_new_initial() {
        let c: EventCache<u64> = EventCache::new(10);
        assert!(c.events.is_empty());
        assert_eq!(c.max_size, 10);
        assert_eq!(c.overflow_count, 0);
        assert!(c.is_empty());
        assert_eq!(c.len(), 0);
    }

    // TC2: push 顺序保持：push 1,2,3 → 迭代序 == [1,2,3]（队首最旧）
    #[test]
    fn tc2_push_order() {
        let mut c: EventCache<u64> = EventCache::new(10);
        c.push(1);
        c.push(2);
        c.push(3);
        assert_eq!(collect(&c), vec![1, 2, 3]);
        assert_eq!(c.len(), 3);
        assert_eq!(c.overflow_count, 0);
    }

    // TC3: 溢出丢弃最旧：max_size=2，push 1,2,3 → events == [2,3] / overflow_count==1 / len==2
    #[test]
    fn tc3_overflow_drops_oldest() {
        let mut c: EventCache<u64> = EventCache::new(2);
        c.push(1);
        c.push(2);
        c.push(3);
        assert_eq!(collect(&c), vec![2, 3]);
        assert_eq!(c.overflow_count, 1);
        assert_eq!(c.len(), 2);
    }

    // TC4: 连续溢出计数：max_size=2，push 1..=5 → events == [4,5] / overflow_count==3
    #[test]
    fn tc4_consecutive_overflow_count() {
        let mut c: EventCache<u64> = EventCache::new(2);
        for i in 1..=5u64 {
            c.push(i);
        }
        assert_eq!(collect(&c), vec![4, 5]);
        assert_eq!(c.overflow_count, 3);
        assert_eq!(c.len(), 2);
    }

    // TC5: max_size=1 边界：push 1 → [1] overflow 0；push 2 → [2] overflow 1；len 恒 1
    #[test]
    fn tc5_max_size_one_boundary() {
        let mut c: EventCache<u64> = EventCache::new(1);
        c.push(1);
        assert_eq!(collect(&c), vec![1]);
        assert_eq!(c.overflow_count, 0);
        assert_eq!(c.len(), 1);
        c.push(2);
        assert_eq!(collect(&c), vec![2]);
        assert_eq!(c.overflow_count, 1);
        assert_eq!(c.len(), 1);
        c.push(3);
        assert_eq!(collect(&c), vec![3]);
        assert_eq!(c.overflow_count, 2);
        assert_eq!(c.len(), 1);
    }

    // TC6: clear 清空 events 保留 overflow_count；再 push 正常工作
    #[test]
    fn tc6_clear_preserves_overflow_count() {
        let mut c: EventCache<u64> = EventCache::new(2);
        for i in 1..=4u64 {
            c.push(i);
        }
        assert_eq!(c.overflow_count, 2);
        c.clear();
        assert_eq!(c.len(), 0);
        assert!(c.is_empty());
        assert_eq!(c.overflow_count, 2); // 历史计数不归零
                                         // clear 后再 push 正常入队
        c.push(9);
        assert_eq!(collect(&c), vec![9]);
        assert_eq!(c.overflow_count, 2);
    }

    // TC7: 泛型双型实例化：EventCache<u64> 与 EventCache<TestEvent> 均正常工作
    #[test]
    fn tc7_generic_two_types() {
        // u64 实例化
        let mut c1: EventCache<u64> = EventCache::new(2);
        c1.push(10);
        c1.push(20);
        c1.push(30);
        assert_eq!(collect(&c1), vec![20, 30]);
        assert_eq!(c1.overflow_count, 1);

        // 自定义 struct 实例化
        let mut c2: EventCache<TestEvent> = EventCache::new(2);
        c2.push(TestEvent { ts: 1, kind: 1 });
        c2.push(TestEvent { ts: 2, kind: 2 });
        c2.push(TestEvent { ts: 3, kind: 3 });
        assert_eq!(c2.len(), 2);
        assert_eq!(c2.overflow_count, 1);
        assert_eq!(c2.events[0], TestEvent { ts: 2, kind: 2 });
        assert_eq!(c2.events[1], TestEvent { ts: 3, kind: 3 });
        assert_eq!(
            collect(&c2),
            vec![TestEvent { ts: 2, kind: 2 }, TestEvent { ts: 3, kind: 3 },]
        );
    }
}
