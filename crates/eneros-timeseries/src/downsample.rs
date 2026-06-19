//! 存储级降采样基础（Task 5）
//!
//! 在内存中维护多粒度缓存（1s/1min/1h），后台 rollup 任务定期聚合原始
//! 数据点。查询大量历史数据时，根据时间范围自动选择合适的粒度，避免
//! 返回过多原始 1s 粒度数据。
//!
//! 与 `aggregation.rs` 的区别：
//! - `aggregation.rs` 是**查询时**滑动窗口聚合（每次查询重新计算）
//! - `downsample.rs` 是**存储级**预聚合（后台定期计算并缓存，查询时直接读取缓存）

use std::collections::HashMap;

use chrono::{DateTime, TimeZone, Utc};

use crate::engine::DataPoint;

/// 降采样粒度
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DownsampleLevel {
    /// 1s 原始粒度
    Second,
    /// 1min 聚合
    Minute,
    /// 1h 聚合
    Hour,
}

impl DownsampleLevel {
    /// 返回该粒度的窗口大小（毫秒）
    pub fn interval_ms(&self) -> i64 {
        match self {
            DownsampleLevel::Second => 1_000,
            DownsampleLevel::Minute => 60_000,
            DownsampleLevel::Hour => 3_600_000,
        }
    }

    /// 根据查询时间范围自动选择粒度
    ///
    /// - `<= 1h` → Second（返回原始 1s 数据）
    /// - `<= 7d` → Minute（返回 1min 聚合）
    /// - `> 7d`  → Hour（返回 1h 聚合）
    pub fn for_range(start_ms: i64, end_ms: i64) -> DownsampleLevel {
        let range_ms = end_ms - start_ms;
        if range_ms <= 3_600_000 {
            // <= 1h
            DownsampleLevel::Second
        } else if range_ms <= 604_800_000 {
            // <= 7d
            DownsampleLevel::Minute
        } else {
            DownsampleLevel::Hour
        }
    }
}

/// 聚合数据点
#[derive(Debug, Clone)]
pub struct AggregatedPoint {
    /// 窗口起始时间戳
    pub timestamp: DateTime<Utc>,
    /// 窗口内平均值
    pub avg: f64,
    /// 窗口内最小值
    pub min: f64,
    /// 窗口内最大值
    pub max: f64,
    /// 窗口内数据点数
    pub count: u64,
    /// 窗口内数据点值之和
    pub sum: f64,
}

/// 多粒度降采样缓存
///
/// 以 `(element_id, parameter, level)` 为键存储聚合后的数据点列表。
/// 后台 rollup 任务定期调用 [`DownsampledCache::rollup`] 将原始数据点
/// 聚合到指定粒度并替换该键的缓存。
pub struct DownsampledCache {
    // (element_id, parameter, level) -> Vec<AggregatedPoint>
    caches: HashMap<(u64, String, DownsampleLevel), Vec<AggregatedPoint>>,
}

impl DownsampledCache {
    /// 创建空的降采样缓存
    pub fn new() -> Self {
        Self {
            caches: HashMap::new(),
        }
    }

    /// 将原始数据点聚合到指定粒度，替换该键的缓存。
    ///
    /// 聚合逻辑：
    /// 1. 将原始 DataPoint 按时间窗口分组（窗口对齐到整秒/整分/整时）
    /// 2. 每个窗口计算 avg/min/max/count/sum
    /// 3. 结果按时间戳排序后存入缓存
    pub fn rollup(
        &mut self,
        element_id: u64,
        parameter: &str,
        level: DownsampleLevel,
        points: &[DataPoint],
    ) {
        if points.is_empty() {
            return;
        }

        let interval_ms = level.interval_ms();
        let key = (element_id, parameter.to_string(), level);

        // 按窗口分组
        let mut windows: HashMap<i64, Vec<f64>> = HashMap::new();
        for p in points {
            let ts_ms = p.timestamp.timestamp_millis();
            // 窗口对齐：将时间戳向下取整到窗口边界
            let window_start = (ts_ms / interval_ms) * interval_ms;
            windows.entry(window_start).or_default().push(p.value);
        }

        // 计算每个窗口的聚合值
        let mut aggregated: Vec<AggregatedPoint> = windows
            .into_iter()
            .map(|(window_start_ms, values)| {
                let count = values.len() as u64;
                let sum: f64 = values.iter().sum();
                let avg = sum / count as f64;
                let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
                let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                let timestamp = Utc.timestamp_millis_opt(window_start_ms).unwrap();
                AggregatedPoint {
                    timestamp,
                    avg,
                    min,
                    max,
                    count,
                    sum,
                }
            })
            .collect();

        // 按时间戳排序
        aggregated.sort_by_key(|p| p.timestamp);

        self.caches.insert(key, aggregated);
    }

    /// 查询指定粒度的聚合数据
    ///
    /// 返回时间戳在 `[start, end]`（毫秒）范围内的聚合数据点。
    /// 如果该键/粒度无缓存数据，返回空 Vec。
    pub fn query(
        &self,
        element_id: u64,
        parameter: &str,
        level: DownsampleLevel,
        start: i64,
        end: i64,
    ) -> Vec<AggregatedPoint> {
        let key = (element_id, parameter.to_string(), level);
        self.caches
            .get(&key)
            .map(|points| {
                points
                    .iter()
                    .filter(|p| {
                        let ts = p.timestamp.timestamp_millis();
                        ts >= start && ts <= end
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 检查指定键/粒度是否有缓存数据
    pub fn has_data(&self, element_id: u64, parameter: &str, level: DownsampleLevel) -> bool {
        let key = (element_id, parameter.to_string(), level);
        self.caches.get(&key).is_some_and(|v| !v.is_empty())
    }
}

impl Default for DownsampledCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::DataQuality;
    use chrono::TimeZone;

    fn make_point(ts_secs: i64, value: f64) -> DataPoint {
        DataPoint {
            timestamp: Utc.timestamp_opt(ts_secs, 0).unwrap(),
            value,
            quality: DataQuality::Good,
        }
    }

    #[test]
    fn test_for_range_second() {
        // <= 1h → Second
        assert_eq!(
            DownsampleLevel::for_range(0, 3_600_000),
            DownsampleLevel::Second
        );
        // 30 分钟
        assert_eq!(
            DownsampleLevel::for_range(0, 1_800_000),
            DownsampleLevel::Second
        );
    }

    #[test]
    fn test_for_range_minute() {
        // 1h < range <= 7d → Minute
        assert_eq!(
            DownsampleLevel::for_range(0, 3_600_001),
            DownsampleLevel::Minute
        );
        // 正好 7d
        assert_eq!(
            DownsampleLevel::for_range(0, 604_800_000),
            DownsampleLevel::Minute
        );
    }

    #[test]
    fn test_for_range_hour() {
        // > 7d → Hour
        assert_eq!(
            DownsampleLevel::for_range(0, 604_800_001),
            DownsampleLevel::Hour
        );
        // 30 天
        assert_eq!(
            DownsampleLevel::for_range(0, 2_592_000_000),
            DownsampleLevel::Hour
        );
    }

    #[test]
    fn test_interval_ms() {
        assert_eq!(DownsampleLevel::Second.interval_ms(), 1_000);
        assert_eq!(DownsampleLevel::Minute.interval_ms(), 60_000);
        assert_eq!(DownsampleLevel::Hour.interval_ms(), 3_600_000);
    }

    #[test]
    fn test_rollup_minute_basic() {
        // 2 分钟的数据，每秒一个点
        // 第 1 分钟（0-59s）：值 10.0..69.0
        // 第 2 分钟（60-119s）：值 70.0..129.0
        let points: Vec<DataPoint> = (0..120)
            .map(|i| make_point(i, i as f64 + 10.0))
            .collect();

        let mut cache = DownsampledCache::new();
        cache.rollup(1, "temperature", DownsampleLevel::Minute, &points);

        let result = cache.query(1, "temperature", DownsampleLevel::Minute, 0, 120_000);
        assert_eq!(result.len(), 2, "should have 2 minute windows");

        // 第 1 分钟窗口 [0, 60s)
        let w1 = &result[0];
        assert_eq!(w1.count, 60);
        assert_eq!(w1.min, 10.0);
        assert_eq!(w1.max, 69.0);
        assert!((w1.avg - 39.5).abs() < 1e-9);
        assert!((w1.sum - 2370.0).abs() < 1e-9);

        // 第 2 分钟窗口 [60s, 120s)
        let w2 = &result[1];
        assert_eq!(w2.count, 60);
        assert_eq!(w2.min, 70.0);
        assert_eq!(w2.max, 129.0);
        assert!((w2.avg - 99.5).abs() < 1e-9);
    }

    #[test]
    fn test_rollup_hour_basic() {
        // 2 小时的数据，每分钟一个点
        let points: Vec<DataPoint> = (0..120)
            .map(|i| make_point(i * 60, i as f64))
            .collect();

        let mut cache = DownsampledCache::new();
        cache.rollup(1, "load", DownsampleLevel::Hour, &points);

        let result = cache.query(1, "load", DownsampleLevel::Hour, 0, 7_200_000);
        assert_eq!(result.len(), 2, "should have 2 hour windows");

        // 第 1 小时窗口 [0, 3600s)
        let w1 = &result[0];
        assert_eq!(w1.count, 60);
        assert_eq!(w1.min, 0.0);
        assert_eq!(w1.max, 59.0);
        assert!((w1.avg - 29.5).abs() < 1e-9);

        // 第 2 小时窗口 [3600s, 7200s)
        let w2 = &result[1];
        assert_eq!(w2.count, 60);
        assert_eq!(w2.min, 60.0);
        assert_eq!(w2.max, 119.0);
    }

    #[test]
    fn test_rollup_window_alignment() {
        // 数据点不在窗口边界上，验证窗口对齐
        // 1 分钟粒度：窗口应为 [0, 60s), [60s, 120s), ...
        let points = vec![
            make_point(5, 10.0),   // 落入 [0, 60s)
            make_point(55, 20.0),  // 落入 [0, 60s)
            make_point(65, 30.0),  // 落入 [60s, 120s)
            make_point(119, 40.0), // 落入 [60s, 120s)
        ];

        let mut cache = DownsampledCache::new();
        cache.rollup(1, "voltage", DownsampleLevel::Minute, &points);

        let result = cache.query(1, "voltage", DownsampleLevel::Minute, 0, 200_000);
        assert_eq!(result.len(), 2);

        // 窗口 [0, 60s) 的起始时间戳应为 0
        assert_eq!(result[0].timestamp, Utc.timestamp_opt(0, 0).unwrap());
        assert_eq!(result[0].count, 2);
        assert!((result[0].avg - 15.0).abs() < 1e-9);

        // 窗口 [60s, 120s) 的起始时间戳应为 60s
        assert_eq!(result[1].timestamp, Utc.timestamp_opt(60, 0).unwrap());
        assert_eq!(result[1].count, 2);
        assert!((result[1].avg - 35.0).abs() < 1e-9);
    }

    #[test]
    fn test_rollup_empty() {
        let mut cache = DownsampledCache::new();
        cache.rollup(1, "temp", DownsampleLevel::Minute, &[]);

        assert!(!cache.has_data(1, "temp", DownsampleLevel::Minute));
    }

    #[test]
    fn test_rollup_single_point() {
        let points = vec![make_point(30, 42.0)];

        let mut cache = DownsampledCache::new();
        cache.rollup(1, "temp", DownsampleLevel::Minute, &points);

        let result = cache.query(1, "temp", DownsampleLevel::Minute, 0, 60_000);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].count, 1);
        assert_eq!(result[0].avg, 42.0);
        assert_eq!(result[0].min, 42.0);
        assert_eq!(result[0].max, 42.0);
        assert_eq!(result[0].sum, 42.0);
    }

    #[test]
    fn test_query_filters_by_time_range() {
        // 3 分钟的数据
        let points: Vec<DataPoint> = (0..180)
            .map(|i| make_point(i, i as f64))
            .collect();

        let mut cache = DownsampledCache::new();
        cache.rollup(1, "temp", DownsampleLevel::Minute, &points);

        // 只查询第 2 分钟 [60s, 120s)
        // 窗口时间戳为 60000ms，查询范围 [60000, 119999] 只包含该窗口
        let result = cache.query(1, "temp", DownsampleLevel::Minute, 60_000, 119_999);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].count, 60);

        // 查询全部 3 个窗口 [0, 180000]
        let all = cache.query(1, "temp", DownsampleLevel::Minute, 0, 180_000);
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_has_data() {
        let mut cache = DownsampledCache::new();
        assert!(!cache.has_data(1, "temp", DownsampleLevel::Minute));

        cache.rollup(1, "temp", DownsampleLevel::Minute, &[make_point(0, 1.0)]);
        assert!(cache.has_data(1, "temp", DownsampleLevel::Minute));

        // 不同键/粒度无数据
        assert!(!cache.has_data(2, "temp", DownsampleLevel::Minute));
        assert!(!cache.has_data(1, "temp", DownsampleLevel::Hour));
    }
}
