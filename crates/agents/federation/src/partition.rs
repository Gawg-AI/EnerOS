//! v0.101.0 断网处理与孤岛模式：孤岛自治状态。
//!
//! 蓝图 phase2.md §v0.101.0：网络分区确认后进入孤岛自治——冻结外部交易
//! （由 `PartitionDetector::trading_frozen` 判定），本地产生的业务事件缓存
//! 待恢复同步；退出孤岛停止缓存但**保留缓存内容**（数据不丢，蓝图 §7.2），
//! 由恢复同步流程（recovery.rs）统一上传。
//!
//! ## 设计要点
//!
//! - **泛型化持有**（D5）：[`IslandMode<T>`] 持有 [`EventCache<T>`]，与
//!   v0.96.0 `EventRecord` 解耦，eneros-federation 保持仅依赖
//!   eneros-crypto（SBOM 不变）。
//! - **注入时钟**（D6）：进入孤岛时刻 `since` 由 `activate(now_ms)` 参数
//!   注入，禁系统时钟，确定性可复现。
//! - **幂等激活**（C50）：已 `active` 时再次 `activate` 不重置 `since`、
//!   不递增 `activated_count`——重入安全，状态首次确立时刻可追溯。
//! - **可观测**（D7/D10）：`activated_count` 记录进入孤岛次数；
//!   `cache_event` 返回 `bool`（未激活静默丢弃可测化，D10）。
//! - **模块独立**（C56）：`IslandMode` 不持有 `PartitionDetector`——状态
//!   联动由上层组合（detector 判定 Partitioned → 上层调 `activate`；
//!   Recovering 同步完成 → 上层调 `deactivate`），模块独立可测。

use crate::cache::EventCache;

/// 孤岛模式（蓝图 §4.1/§4.5 IslandMode，D5 泛型化持有 `EventCache<T>`）
#[derive(Debug, Clone)]
pub struct IslandMode<T> {
    /// 孤岛激活标志
    pub active: bool,
    /// 进入孤岛时刻（ms，注入时钟 D6）
    pub since: u64,
    /// 本地事件缓存（退出孤岛后保留待同步）
    pub cache: EventCache<T>,
    /// 进入孤岛次数（D7 可观测）
    pub activated_count: u64,
}

impl<T> IslandMode<T> {
    /// 创建：`active=false` / `since=0` / `activated_count=0` / cache 空
    pub fn new(cache_max_size: usize) -> Self {
        Self {
            active: false,
            since: 0,
            cache: EventCache::new(cache_max_size),
            activated_count: 0,
        }
    }

    /// 激活孤岛：幂等——已 `active` 时 `since` 不重置、`activated_count`
    /// 不递增（C50）
    pub fn activate(&mut self, now_ms: u64) {
        if !self.active {
            self.active = true;
            self.since = now_ms;
            self.activated_count += 1;
        }
    }

    /// 退出孤岛：`active=false`；缓存保留（蓝图 §4.5 数据不丢）
    pub fn deactivate(&mut self) {
        self.active = false;
    }

    /// 缓存事件（D10 返回 `bool` 可观测）：`!active` → `false` 不入缓存；
    /// `active` → `cache.push` + `true`
    pub fn cache_event(&mut self, e: T) -> bool {
        if !self.active {
            return false;
        }
        self.cache.push(e);
        true
    }
}

// ============================================================
// Unit Tests TI20~TI27
// ============================================================

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use super::*;

    /// 队首→队尾顺序收集为 Vec（断言辅助，cache.rs 同例）
    fn collect<T: Clone>(c: &EventCache<T>) -> Vec<T> {
        c.events.iter().cloned().collect()
    }

    // TI20: new(3) 初始：active==false / since==0 / activated_count==0 / cache.is_empty()
    #[test]
    fn ti20_new_initial() {
        let m: IslandMode<u64> = IslandMode::new(3);
        assert!(!m.active);
        assert_eq!(m.since, 0);
        assert_eq!(m.activated_count, 0);
        assert!(m.cache.is_empty());
        assert_eq!(m.cache.len(), 0);
        assert_eq!(m.cache.max_size, 3);
        assert_eq!(m.cache.overflow_count, 0);
    }

    // TI21: activate(1000) → active==true / since==1000 / activated_count==1
    #[test]
    fn ti21_activate_sets_since_and_count() {
        let mut m: IslandMode<u64> = IslandMode::new(3);
        m.activate(1000);
        assert!(m.active);
        assert_eq!(m.since, 1000);
        assert_eq!(m.activated_count, 1);
    }

    // TI22: activate 幂等：activate(1000) 后 activate(2000) → since 仍 1000 / activated_count 仍 1
    #[test]
    fn ti22_activate_idempotent() {
        let mut m: IslandMode<u64> = IslandMode::new(3);
        m.activate(1000);
        m.activate(2000);
        assert!(m.active);
        assert_eq!(m.since, 1000);
        assert_eq!(m.activated_count, 1);
    }

    // TI23: cache_event 激活入队：activate 后 cache_event(7)==true → cache.len()==1，events[0]==7
    #[test]
    fn ti23_cache_event_accepted_when_active() {
        let mut m: IslandMode<u64> = IslandMode::new(3);
        m.activate(1000);
        assert!(m.cache_event(7));
        assert_eq!(m.cache.len(), 1);
        assert_eq!(m.cache.events[0], 7);
    }

    // TI24: cache_event 未激活拒绝：未 activate 时 cache_event(7)==false → cache.is_empty()
    #[test]
    fn ti24_cache_event_rejected_when_inactive() {
        let mut m: IslandMode<u64> = IslandMode::new(3);
        assert!(!m.cache_event(7));
        assert!(m.cache.is_empty());
        assert_eq!(m.cache.len(), 0);
    }

    // TI25: 溢出经 IslandMode 计数：new(2) activate 后 cache_event 1,2,3 →
    //       cache.events==[2,3] / cache.overflow_count==1（丢弃最旧透传）
    #[test]
    fn ti25_overflow_counted_via_island_mode() {
        let mut m: IslandMode<u64> = IslandMode::new(2);
        m.activate(1000);
        assert!(m.cache_event(1));
        assert!(m.cache_event(2));
        assert!(m.cache_event(3));
        assert_eq!(collect(&m.cache), vec![2, 3]);
        assert_eq!(m.cache.overflow_count, 1);
        assert_eq!(m.cache.len(), 2);
    }

    // TI26: deactivate 缓存保留：activate+cache 2 事件后 deactivate →
    //       active==false / cache.len()==2（数据不丢）
    #[test]
    fn ti26_deactivate_preserves_cache() {
        let mut m: IslandMode<u64> = IslandMode::new(3);
        m.activate(1000);
        assert!(m.cache_event(10));
        assert!(m.cache_event(20));
        m.deactivate();
        assert!(!m.active);
        assert_eq!(m.cache.len(), 2);
        assert_eq!(collect(&m.cache), vec![10, 20]);
    }

    // TI27: deactivate 后拒缓存：deactivate 后 cache_event(9)==false → cache.len() 不变
    #[test]
    fn ti27_cache_event_rejected_after_deactivate() {
        let mut m: IslandMode<u64> = IslandMode::new(3);
        m.activate(1000);
        assert!(m.cache_event(10));
        m.deactivate();
        assert!(!m.cache_event(9));
        assert_eq!(m.cache.len(), 1);
        assert_eq!(collect(&m.cache), vec![10]);
    }
}
