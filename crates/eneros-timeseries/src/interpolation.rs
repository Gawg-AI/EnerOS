use chrono::{DateTime, Utc};
use super::engine::{DataPoint, DataQuality};

/// Perform linear interpolation at the specified target timestamps.
///
/// For each target time, find the two surrounding data points and interpolate.
/// If the target time is before the first point or after the last point,
/// the nearest value is used (no extrapolation).
/// Interpolated points have quality = `DataQuality::Uncertain`.
pub fn interpolate_linear(
    points: &[DataPoint],
    target_times: &[DateTime<Utc>],
) -> Vec<DataPoint> {
    if points.is_empty() || target_times.is_empty() {
        return Vec::new();
    }

    // Sort points by timestamp for binary search
    let mut sorted: Vec<&DataPoint> = points.iter().collect();
    sorted.sort_by_key(|p| p.timestamp);

    let first = sorted.first().unwrap();
    let last = sorted.last().unwrap();

    target_times
        .iter()
        .map(|&target| {
            // Before first point: use nearest (first)
            if target <= first.timestamp {
                return DataPoint {
                    timestamp: target,
                    value: first.value,
                    quality: DataQuality::Uncertain,
                };
            }

            // After last point: use nearest (last)
            if target >= last.timestamp {
                return DataPoint {
                    timestamp: target,
                    value: last.value,
                    quality: DataQuality::Uncertain,
                };
            }

            // Find the two surrounding points using binary search
            let idx = sorted
                .binary_search_by_key(&target, |p| p.timestamp)
                .unwrap_or_else(|i| i);

            // idx is the first point with timestamp >= target
            let right = &sorted[idx];
            let left = &sorted[idx - 1];

            let left_ts = left.timestamp.timestamp_millis() as f64;
            let right_ts = right.timestamp.timestamp_millis() as f64;
            let target_ts = target.timestamp_millis() as f64;

            let ratio = if (right_ts - left_ts).abs() < f64::EPSILON {
                0.0
            } else {
                (target_ts - left_ts) / (right_ts - left_ts)
            };

            let interpolated_value = left.value + ratio * (right.value - left.value);

            DataPoint {
                timestamp: target,
                value: interpolated_value,
                quality: DataQuality::Uncertain,
            }
        })
        .collect()
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
    fn test_interpolate_linear_basic() {
        let points = vec![
            make_point(0, 0.0, DataQuality::Good),
            make_point(10, 100.0, DataQuality::Good),
        ];

        let targets: Vec<DateTime<Utc>> = vec![
            Utc.timestamp_opt(5, 0).unwrap(),
        ];

        let result = interpolate_linear(&points, &targets);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].value, 50.0);
        assert_eq!(result[0].quality, DataQuality::Uncertain);
    }

    #[test]
    fn test_interpolate_before_first() {
        let points = vec![
            make_point(10, 100.0, DataQuality::Good),
            make_point(20, 200.0, DataQuality::Good),
        ];

        let targets: Vec<DateTime<Utc>> = vec![
            Utc.timestamp_opt(5, 0).unwrap(),
        ];

        let result = interpolate_linear(&points, &targets);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].value, 100.0); // nearest (first)
        assert_eq!(result[0].quality, DataQuality::Uncertain);
    }

    #[test]
    fn test_interpolate_after_last() {
        let points = vec![
            make_point(10, 100.0, DataQuality::Good),
            make_point(20, 200.0, DataQuality::Good),
        ];

        let targets: Vec<DateTime<Utc>> = vec![
            Utc.timestamp_opt(30, 0).unwrap(),
        ];

        let result = interpolate_linear(&points, &targets);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].value, 200.0); // nearest (last)
        assert_eq!(result[0].quality, DataQuality::Uncertain);
    }

    #[test]
    fn test_interpolate_empty_points() {
        let points: Vec<DataPoint> = vec![];
        let targets: Vec<DateTime<Utc>> = vec![Utc.timestamp_opt(5, 0).unwrap()];
        let result = interpolate_linear(&points, &targets);
        assert!(result.is_empty());
    }

    #[test]
    fn test_interpolate_empty_targets() {
        let points = vec![make_point(0, 10.0, DataQuality::Good)];
        let targets: Vec<DateTime<Utc>> = vec![];
        let result = interpolate_linear(&points, &targets);
        assert!(result.is_empty());
    }

    #[test]
    fn test_interpolate_gap_filling() {
        let points = vec![
            make_point(0, 0.0, DataQuality::Good),
            make_point(100, 1000.0, DataQuality::Good),
        ];

        // Fill gaps at t=20, 40, 60, 80
        let targets: Vec<DateTime<Utc>> = (1..=4)
            .map(|i| Utc.timestamp_opt(i * 20, 0).unwrap())
            .collect();

        let result = interpolate_linear(&points, &targets);
        assert_eq!(result.len(), 4);

        // Each 20-second step should add 200.0
        assert_eq!(result[0].value, 200.0);
        assert_eq!(result[1].value, 400.0);
        assert_eq!(result[2].value, 600.0);
        assert_eq!(result[3].value, 800.0);

        for r in &result {
            assert_eq!(r.quality, DataQuality::Uncertain);
        }
    }

    #[test]
    fn test_interpolate_single_point() {
        let points = vec![make_point(10, 42.0, DataQuality::Good)];
        let targets: Vec<DateTime<Utc>> = vec![
            Utc.timestamp_opt(5, 0).unwrap(),
            Utc.timestamp_opt(15, 0).unwrap(),
        ];

        let result = interpolate_linear(&points, &targets);
        assert_eq!(result.len(), 2);
        // Both before and after should use the single point's value
        assert_eq!(result[0].value, 42.0);
        assert_eq!(result[1].value, 42.0);
    }

    #[test]
    fn test_interpolate_exact_match() {
        let points = vec![
            make_point(0, 10.0, DataQuality::Good),
            make_point(10, 20.0, DataQuality::Good),
        ];

        let targets: Vec<DateTime<Utc>> = vec![Utc.timestamp_opt(0, 0).unwrap()];

        let result = interpolate_linear(&points, &targets);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].value, 10.0);
    }
}
