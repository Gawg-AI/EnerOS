//! 指标存储抽象与内存实现（v0.120.0）.
//!
//! 对应蓝图 §4 存储层：`MetricsStore` trait 抽象存储后端（偏差 D3：TSDB
//! 持久化经 trait 预留对接，本版本不落地适配器）；`MemoryMetricsStore`
//! 为按指标名分桶的内存环形缓冲实现，每桶容量可配，写满丢弃最旧。

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;

use crate::metrics::{Metric, MetricValue, MetricsError};

/// 指标存储抽象（偏差 D1：`query_range` 返回 `alloc::vec::Vec<Metric>`）.
pub trait MetricsStore {
    /// 追加一条指标数据点.
    fn append(&mut self, m: &Metric) -> Result<(), MetricsError>;
    /// 读取指定指标最新值（无数据时返回 `None`）.
    fn get_latest(&self, name: &str) -> Option<u64>;
    /// 按时间窗查询指定指标数据点，按时间戳升序返回.
    fn query_range(
        &self,
        name: &str,
        start_ms: u64,
        end_ms: u64,
    ) -> Result<Vec<Metric>, MetricsError>;
}

/// 内存指标存储：按指标名分桶的环形缓冲，每桶写满丢弃最旧.
pub struct MemoryMetricsStore {
    /// 按指标名分桶的数据点队列.
    buckets: BTreeMap<String, VecDeque<Metric>>,
    /// 每桶容量（条数）.
    per_metric_capacity: usize,
}

impl MemoryMetricsStore {
    /// 创建内存存储；`per_metric_capacity` 为每个指标的最大保留条数.
    pub fn new(per_metric_capacity: usize) -> Self {
        Self {
            buckets: BTreeMap::new(),
            per_metric_capacity,
        }
    }

    /// 每个指标的最大保留条数.
    pub fn per_metric_capacity(&self) -> usize {
        self.per_metric_capacity
    }
}

impl MetricsStore for MemoryMetricsStore {
    fn append(&mut self, m: &Metric) -> Result<(), MetricsError> {
        let bucket = self
            .buckets
            .entry(String::from(m.name.as_str()))
            .or_default();
        // 环形语义：写满时丢弃最旧一条
        if bucket.len() >= self.per_metric_capacity {
            bucket.pop_front();
        }
        bucket.push_back(m.clone());
        Ok(())
    }

    fn get_latest(&self, name: &str) -> Option<u64> {
        self.buckets.get(name).and_then(|b| {
            b.back().map(|m| match m.value {
                MetricValue::U64(v) => v,
                MetricValue::I64(v) => v as u64,
                MetricValue::F64(v) => v as u64,
            })
        })
    }

    fn query_range(
        &self,
        name: &str,
        start_ms: u64,
        end_ms: u64,
    ) -> Result<Vec<Metric>, MetricsError> {
        if start_ms > end_ms {
            return Err(MetricsError::QueryRangeInvalid);
        }
        let mut out: Vec<Metric> = Vec::new();
        if let Some(bucket) = self.buckets.get(name) {
            for m in bucket.iter() {
                if m.timestamp_ms >= start_ms && m.timestamp_ms <= end_ms {
                    out.push(m.clone());
                }
            }
        }
        // 按时间戳升序
        out.sort_by_key(|m| m.timestamp_ms);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use heapless::String as HString;

    use super::*;
    use crate::metrics::MetricType;

    /// 构造一条测试指标数据点.
    fn make_metric(name: &str, value: u64, ts: u64) -> Metric {
        Metric {
            name: HString::<64>::try_from(name).unwrap(),
            metric_type: MetricType::Counter,
            value: MetricValue::U64(value),
            labels: heapless::Vec::new(),
            timestamp_ms: ts,
        }
    }

    /// STORE1：按名分桶互不影响.
    #[test]
    fn store1_buckets_isolated() {
        let mut s = MemoryMetricsStore::new(4);
        for i in 0..3u64 {
            s.append(&make_metric("a", i, i)).unwrap();
        }
        for i in 0..2u64 {
            s.append(&make_metric("b", 100 + i, i)).unwrap();
        }
        assert_eq!(s.query_range("a", 0, u64::MAX).unwrap().len(), 3);
        assert_eq!(s.query_range("b", 0, u64::MAX).unwrap().len(), 2);
        assert_eq!(s.get_latest("a"), Some(2));
        assert_eq!(s.get_latest("b"), Some(101));
    }

    /// STORE2：时间窗过滤且按时间戳升序.
    #[test]
    fn store2_time_window_filter_ascending() {
        let mut s = MemoryMetricsStore::new(16);
        for i in 0..10u64 {
            s.append(&make_metric("a", i, i * 100)).unwrap();
        }
        let got = s.query_range("a", 300, 600).unwrap();
        assert_eq!(got.len(), 4);
        let ts: Vec<u64> = got.iter().map(|m| m.timestamp_ms).collect();
        assert_eq!(ts, alloc::vec![300, 400, 500, 600]);
        // 窗口外数据不出现
        assert!(got
            .iter()
            .all(|m| m.timestamp_ms >= 300 && m.timestamp_ms <= 600));
    }

    /// STORE3：桶写满丢弃最旧.
    #[test]
    fn store3_capacity_drops_oldest() {
        let mut s = MemoryMetricsStore::new(3);
        for i in 0..5u64 {
            s.append(&make_metric("a", i, i)).unwrap();
        }
        let got = s.query_range("a", 0, u64::MAX).unwrap();
        assert_eq!(got.len(), 3);
        // 仅保留最新的 3 条（2、3、4）
        let vals: Vec<u64> = got
            .iter()
            .map(|m| match m.value {
                MetricValue::U64(v) => v,
                _ => 0,
            })
            .collect();
        assert_eq!(vals, alloc::vec![2, 3, 4]);
        assert_eq!(s.get_latest("a"), Some(4));
    }
}
