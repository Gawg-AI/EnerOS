use chrono::{DateTime, Duration, Utc};
use super::engine::{DataPoint, DataQuality};

/// Specification for a sliding window aggregation
#[derive(Debug, Clone)]
pub struct WindowSpec {
    pub window_size_secs: u64,
    pub step_size_secs: u64,
}

/// Result of a windowed aggregation
#[derive(Debug, Clone)]
pub struct WindowedResult {
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub avg: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub count: usize,
    pub sum: f64,
}

/// Sliding window aggregator for time-series data
pub struct WindowedAggregator;

impl WindowedAggregator {
    /// Compute sliding window aggregates over the given data points.
    ///
    /// Only points with `DataQuality::Good` are included in the aggregation.
    /// If all points in a window are Bad/Uncertain, the aggregate values are `None`.
    /// Empty windows (no data points at all) are skipped to avoid bloating results
    /// when data is sparse relative to the step size.
    pub fn aggregate(data_points: &[DataPoint], spec: &WindowSpec) -> Vec<WindowedResult> {
        if data_points.is_empty() {
            return Vec::new();
        }

        // Find the overall time range
        let min_ts = data_points.iter().map(|p| p.timestamp).min().unwrap();
        let max_ts = data_points.iter().map(|p| p.timestamp).max().unwrap();

        let window_duration = Duration::seconds(spec.window_size_secs as i64);
        let step_duration = Duration::seconds(spec.step_size_secs as i64);

        let mut results = Vec::new();
        let mut window_start = min_ts;

        while window_start <= max_ts {
            let window_end = window_start + window_duration;

            // Collect good-quality points within this window
            let good_values: Vec<f64> = data_points
                .iter()
                .filter(|p| p.quality == DataQuality::Good)
                .filter(|p| p.timestamp >= window_start && p.timestamp < window_end)
                .map(|p| p.value)
                .collect();

            // Skip empty windows entirely (no data points at all in this window)
            let has_any_point = data_points
                .iter()
                .any(|p| p.timestamp >= window_start && p.timestamp < window_end);

            if !has_any_point {
                window_start += step_duration;
                continue;
            }

            let count = good_values.len();
            let (avg, min, max, sum) = if count == 0 {
                (None, None, None, 0.0)
            } else {
                let s: f64 = good_values.iter().sum();
                (
                    Some(s / count as f64),
                    Some(good_values.iter().cloned().fold(f64::INFINITY, f64::min)),
                    Some(good_values.iter().cloned().fold(f64::NEG_INFINITY, f64::max)),
                    s,
                )
            };

            results.push(WindowedResult {
                window_start,
                window_end,
                avg,
                min,
                max,
                count,
                sum,
            });

            window_start += step_duration;
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn make_point(ts_secs: i64, value: f64, quality: DataQuality) -> DataPoint {
        DataPoint {
            timestamp: Utc.timestamp_opt(ts_secs, 0).unwrap(),
            value,
            quality,
        }
    }

    #[test]
    fn test_aggregate_basic() {
        let points = vec![
            make_point(0, 10.0, DataQuality::Good),
            make_point(5, 20.0, DataQuality::Good),
            make_point(10, 30.0, DataQuality::Good),
            make_point(15, 40.0, DataQuality::Good),
        ];

        let spec = WindowSpec {
            window_size_secs: 10,
            step_size_secs: 10,
        };

        let results = WindowedAggregator::aggregate(&points, &spec);
        assert_eq!(results.len(), 2);

        // Window [0, 10): points at 0 and 5
        assert_eq!(results[0].count, 2);
        assert_eq!(results[0].avg.unwrap(), 15.0);
        assert_eq!(results[0].min.unwrap(), 10.0);
        assert_eq!(results[0].max.unwrap(), 20.0);
        assert_eq!(results[0].sum, 30.0);

        // Window [10, 20): points at 10 and 15
        assert_eq!(results[1].count, 2);
        assert_eq!(results[1].avg.unwrap(), 35.0);
    }

    #[test]
    fn test_aggregate_empty() {
        let points: Vec<DataPoint> = vec![];
        let spec = WindowSpec {
            window_size_secs: 10,
            step_size_secs: 10,
        };
        let results = WindowedAggregator::aggregate(&points, &spec);
        assert!(results.is_empty());
    }

    #[test]
    fn test_aggregate_all_bad_quality() {
        let points = vec![
            make_point(0, 10.0, DataQuality::Bad),
            make_point(5, 20.0, DataQuality::Uncertain),
        ];

        let spec = WindowSpec {
            window_size_secs: 10,
            step_size_secs: 10,
        };

        let results = WindowedAggregator::aggregate(&points, &spec);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].count, 0);
        assert!(results[0].avg.is_none());
        assert!(results[0].min.is_none());
        assert!(results[0].max.is_none());
        assert_eq!(results[0].sum, 0.0);
    }

    #[test]
    fn test_aggregate_mixed_quality() {
        let points = vec![
            make_point(0, 10.0, DataQuality::Good),
            make_point(3, 20.0, DataQuality::Bad),
            make_point(6, 30.0, DataQuality::Good),
        ];

        let spec = WindowSpec {
            window_size_secs: 10,
            step_size_secs: 10,
        };

        let results = WindowedAggregator::aggregate(&points, &spec);
        assert_eq!(results.len(), 1);
        // Only Good points: 10.0 and 30.0
        assert_eq!(results[0].count, 2);
        assert_eq!(results[0].avg.unwrap(), 20.0);
    }

    #[test]
    fn test_aggregate_sliding_window() {
        let points = vec![
            make_point(0, 10.0, DataQuality::Good),
            make_point(5, 20.0, DataQuality::Good),
            make_point(10, 30.0, DataQuality::Good),
        ];

        let spec = WindowSpec {
            window_size_secs: 10,
            step_size_secs: 5,
        };

        let results = WindowedAggregator::aggregate(&points, &spec);
        // Windows: [0,10), [5,15), [10,20)
        assert_eq!(results.len(), 3);

        // Window [0, 10): points at 0 and 5
        assert_eq!(results[0].count, 2);
        assert_eq!(results[0].avg.unwrap(), 15.0);

        // Window [5, 15): points at 5 and 10
        assert_eq!(results[1].count, 2);
        assert_eq!(results[1].avg.unwrap(), 25.0);

        // Window [10, 20): point at 10 only
        assert_eq!(results[2].count, 1);
        assert_eq!(results[2].avg.unwrap(), 30.0);
    }

    #[test]
    fn test_aggregate_single_point() {
        let points = vec![make_point(0, 42.0, DataQuality::Good)];
        let spec = WindowSpec {
            window_size_secs: 10,
            step_size_secs: 10,
        };
        let results = WindowedAggregator::aggregate(&points, &spec);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].count, 1);
        assert_eq!(results[0].avg.unwrap(), 42.0);
        assert_eq!(results[0].min.unwrap(), 42.0);
        assert_eq!(results[0].max.unwrap(), 42.0);
    }
}
