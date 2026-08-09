//! 22 §2.W3.2：流式/窗口聚合——时间桶 rollup（消灭全扫描物化 OOM）。
//!
//! 写入时按时间桶（1s/1min/1h）维护预聚合（count/sum/min/max/last）；
//! 聚合查询命中 rollup 读预聚合（O(桶数)），不命中走**流式扫描**
//! （逐块 fold，有界内存——不物化全量 `Vec<f64>`）。

use alloc::vec::Vec;
use core::ops::RangeInclusive;

/// 时间桶预聚合（count/sum/min/max/last）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RollupBucket {
    /// 桶内点数。
    pub count: u64,
    /// 值总和。
    pub sum: f64,
    /// 最小值。
    pub min: f64,
    /// 最大值。
    pub max: f64,
    /// 最后一点值。
    pub last: f64,
}

impl RollupBucket {
    /// 新建空桶。
    pub const fn empty() -> Self {
        Self {
            count: 0,
            sum: 0.0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            last: 0.0,
        }
    }

    /// 累加一点。
    pub fn add(&mut self, v: f64) {
        self.count += 1;
        self.sum += v;
        if v < self.min {
            self.min = v;
        }
        if v > self.max {
            self.max = v;
        }
        self.last = v;
    }

    /// 合并另一桶（rollup 层级合并用）。
    pub fn merge(&mut self, other: &RollupBucket) {
        self.count += other.count;
        self.sum += other.sum;
        if other.min < self.min {
            self.min = other.min;
        }
        if other.max > self.max {
            self.max = other.max;
        }
        if other.count > 0 {
            self.last = other.last;
        }
    }

    /// 均值（空桶返回 0）。
    pub fn mean(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.sum / self.count as f64
        }
    }

    /// 是否为空桶。
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// 时间桶 rollup 存储（桶粒度可配置：1s/1min/1h）。
///
/// 有界内存：桶数 = 时间跨度 / 桶粒度（kHz 摄入下 1min 桶 1 天 = 1440 桶）。
/// 序列化/持久化为 W3.5 项（本模块提供内存语义 + 可序列化结构）。
#[derive(Debug, Clone)]
pub struct RollupStore {
    /// 桶粒度（毫秒）。
    pub bucket_ms: u64,
    /// 桶 → 预聚合（按桶起点时间索引）。
    buckets: alloc::collections::BTreeMap<u64, RollupBucket>,
}

impl RollupStore {
    /// 新建（`bucket_ms` = 桶粒度，如 1000/60000/3600000）。
    pub fn new(bucket_ms: u64) -> Self {
        Self {
            bucket_ms: bucket_ms.max(1),
            buckets: alloc::collections::BTreeMap::new(),
        }
    }

    /// 桶起点（时间对齐）。
    pub fn bucket_start(&self, ts: u64) -> u64 {
        ts / self.bucket_ms * self.bucket_ms
    }

    /// 写入一点（按时间入桶）。
    pub fn add_point(&mut self, ts: u64, v: f64) {
        let start = self.bucket_start(ts);
        self.buckets
            .entry(start)
            .or_insert_with(RollupBucket::empty)
            .add(v);
    }

    /// 查询范围聚合（O(桶数)）——命中即返回，不扫描原始点。
    pub fn aggregate(&self, range: RangeInclusive<u64>) -> Option<RollupBucket> {
        let mut out = RollupBucket::empty();
        let mut any = false;
        for (start, bucket) in self.buckets.range(range) {
            let _ = start;
            out.merge(bucket);
            any = true;
        }
        if any {
            Some(out)
        } else {
            None
        }
    }

    /// 时间桶 GROUP BY 查询（返回桶起点 → 预聚合）。
    pub fn group_by(&self, range: RangeInclusive<u64>) -> Vec<(u64, RollupBucket)> {
        self.buckets.range(range).map(|(k, v)| (*k, *v)).collect()
    }

    /// 桶数（指标）。
    pub fn bucket_count(&self) -> usize {
        self.buckets.len()
    }
}

/// 流式聚合（不命中 rollup 的兜底）：逐值 fold，有界内存（不物化）。
///
/// `iter` 为迭代器（调用方从索引逐块读点）；返回范围聚合。
pub fn aggregate_streaming(iter: impl Iterator<Item = f64>) -> Option<RollupBucket> {
    let mut out = RollupBucket::empty();
    let mut any = false;
    for v in iter {
        out.add(v);
        any = true;
    }
    if any {
        Some(out)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 写入时按桶预聚合（1s 桶）。
    #[test]
    fn test_rollup_buckets_by_time() {
        let mut r = RollupStore::new(1_000);
        r.add_point(1_000, 10.0);
        r.add_point(1_200, 20.0);
        r.add_point(1_800, 30.0);
        r.add_point(2_500, 5.0); // 下一桶
        assert_eq!(r.bucket_count(), 2);
        let b = r.aggregate(0..=3_000).expect("agg");
        assert_eq!(b.count, 4);
        assert_eq!(b.sum, 65.0);
        assert_eq!(b.min, 5.0);
        assert_eq!(b.max, 30.0);
        assert_eq!(b.last, 5.0);
        assert!((b.mean() - 16.25).abs() < 1e-9);
    }

    /// GROUP BY 查询：桶粒度分组。
    #[test]
    fn test_rollup_group_by() {
        let mut r = RollupStore::new(60_000); // 1min 桶
        r.add_point(0, 1.0);
        r.add_point(30_000, 2.0);
        r.add_point(60_000, 3.0);
        let groups = r.group_by(0..=120_000);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, 0);
        assert_eq!(groups[0].1.count, 2);
        assert_eq!(groups[1].0, 60_000);
        assert_eq!(groups[1].1.count, 1);
    }

    /// 流式聚合：不物化全量（有界内存），百万点可聚合。
    #[test]
    fn test_aggregate_streaming_million_points_bounded() {
        let agg = aggregate_streaming((0..1_000_000).map(|i| i as f64));
        let b = agg.expect("agg");
        assert_eq!(b.count, 1_000_000);
        assert_eq!(b.min, 0.0);
        assert_eq!(b.max, 999_999.0);
    }

    /// 桶合并（层级 rollup：1s → 1min 合并）。
    #[test]
    fn test_bucket_merge_hierarchy() {
        let mut outer = RollupBucket::empty();
        let mut inner = RollupBucket::empty();
        inner.add(10.0);
        inner.add(30.0);
        outer.merge(&inner);
        assert_eq!(outer.count, 2);
        assert_eq!(outer.sum, 40.0);
        assert_eq!(outer.min, 10.0);
        assert_eq!(outer.max, 30.0);
        assert_eq!(outer.last, 30.0);
    }

    /// 空范围聚合 → None。
    #[test]
    fn test_aggregate_empty_range() {
        let r = RollupStore::new(1_000);
        assert!(r.aggregate(0..=100).is_none());
    }
}
