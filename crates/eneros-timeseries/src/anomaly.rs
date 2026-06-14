use chrono::{DateTime, Utc};
use super::engine::{DataPoint, DataQuality};

/// Configuration for anomaly detection
#[derive(Debug, Clone)]
pub struct AnomalyConfig {
    /// Number of standard deviations beyond which a point is considered anomalous
    pub sigma_threshold: f64,
    /// Maximum allowed rate of change (e.g. 0.3 = 30%)
    pub change_rate_threshold: f64,
}

impl Default for AnomalyConfig {
    fn default() -> Self {
        Self {
            sigma_threshold: 3.0,
            change_rate_threshold: 0.3,
        }
    }
}

/// Type of detected anomaly
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnomalyType {
    SigmaViolation,
    SuddenChange,
}

/// A detected anomaly in time-series data
#[derive(Debug, Clone)]
pub struct Anomaly {
    pub timestamp: DateTime<Utc>,
    pub value: f64,
    pub anomaly_type: AnomalyType,
    pub description: String,
}

/// Anomaly detector for time-series data
pub struct AnomalyDetector;

impl AnomalyDetector {
    /// Detect anomalies using the 3-sigma rule.
    ///
    /// Compute mean and standard deviation of good-quality points,
    /// then flag points whose value lies beyond `sigma_threshold * std` from the mean.
    pub fn detect_sigma(points: &[DataPoint], config: &AnomalyConfig) -> Vec<Anomaly> {
        let good_values: Vec<f64> = points
            .iter()
            .filter(|p| p.quality == DataQuality::Good)
            .map(|p| p.value)
            .collect();

        if good_values.len() < 2 {
            return Vec::new();
        }

        let n = good_values.len() as f64;
        let mean: f64 = good_values.iter().sum::<f64>() / n;
        let variance: f64 =
            good_values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
        let std = variance.sqrt();

        if std < f64::EPSILON {
            return Vec::new();
        }

        let lower = mean - config.sigma_threshold * std;
        let upper = mean + config.sigma_threshold * std;

        points
            .iter()
            .filter(|p| p.quality == DataQuality::Good)
            .filter(|p| p.value < lower || p.value > upper)
            .map(|p| Anomaly {
                timestamp: p.timestamp,
                value: p.value,
                anomaly_type: AnomalyType::SigmaViolation,
                description: format!(
                    "Value {:.4} is outside {:.1}-sigma range [{:.4}, {:.4}] (mean={:.4}, std={:.4})",
                    p.value, config.sigma_threshold, lower, upper, mean, std
                ),
            })
            .collect()
    }

    /// Detect sudden changes where the rate of change exceeds the threshold.
    ///
    /// The change rate is calculated as `|value[i] - value[i-1]| / |value[i-1]|`.
    pub fn detect_sudden_change(points: &[DataPoint], config: &AnomalyConfig) -> Vec<Anomaly> {
        let good_points: Vec<&DataPoint> = points
            .iter()
            .filter(|p| p.quality == DataQuality::Good)
            .collect();

        if good_points.len() < 2 {
            return Vec::new();
        }

        let mut anomalies = Vec::new();

        for i in 1..good_points.len() {
            let prev = good_points[i - 1];
            let curr = good_points[i];

            let change_rate = if prev.value.abs() < f64::EPSILON {
                if curr.value.abs() < f64::EPSILON {
                    0.0
                } else {
                    f64::INFINITY
                }
            } else {
                ((curr.value - prev.value) / prev.value).abs()
            };

            if change_rate > config.change_rate_threshold {
                anomalies.push(Anomaly {
                    timestamp: curr.timestamp,
                    value: curr.value,
                    anomaly_type: AnomalyType::SuddenChange,
                    description: format!(
                        "Change rate {:.1}% exceeds threshold {:.1}% (from {:.4} to {:.4})",
                        change_rate * 100.0,
                        config.change_rate_threshold * 100.0,
                        prev.value,
                        curr.value,
                    ),
                });
            }
        }

        anomalies
    }

    /// Run both sigma and sudden-change detectors.
    pub fn detect(points: &[DataPoint], config: &AnomalyConfig) -> Vec<Anomaly> {
        let mut results = Self::detect_sigma(points, config);
        results.extend(Self::detect_sudden_change(points, config));
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
    fn test_detect_sigma_violation() {
        let points: Vec<DataPoint> = (0..10)
            .map(|i| make_point(i, 10.0, DataQuality::Good))
            .chain(std::iter::once(make_point(10, 1000.0, DataQuality::Good)))
            .collect();

        let config = AnomalyConfig {
            sigma_threshold: 3.0,
            change_rate_threshold: 1.0, // high threshold so sudden change doesn't trigger
        };

        let anomalies = AnomalyDetector::detect_sigma(&points, &config);
        assert!(!anomalies.is_empty());
        assert_eq!(anomalies[0].anomaly_type, AnomalyType::SigmaViolation);
    }

    #[test]
    fn test_detect_sigma_no_anomaly() {
        let points: Vec<DataPoint> = (0..10)
            .map(|i| make_point(i, 10.0 + i as f64, DataQuality::Good))
            .collect();

        let config = AnomalyConfig::default();
        let anomalies = AnomalyDetector::detect_sigma(&points, &config);
        assert!(anomalies.is_empty());
    }

    #[test]
    fn test_detect_sudden_change() {
        let points = vec![
            make_point(0, 100.0, DataQuality::Good),
            make_point(1, 200.0, DataQuality::Good), // 100% change
        ];

        let config = AnomalyConfig {
            sigma_threshold: 10.0, // high threshold so sigma doesn't trigger
            change_rate_threshold: 0.3,
        };

        let anomalies = AnomalyDetector::detect_sudden_change(&points, &config);
        assert_eq!(anomalies.len(), 1);
        assert_eq!(anomalies[0].anomaly_type, AnomalyType::SuddenChange);
    }

    #[test]
    fn test_detect_sudden_change_no_anomaly() {
        let points = vec![
            make_point(0, 100.0, DataQuality::Good),
            make_point(1, 110.0, DataQuality::Good), // 10% change
        ];

        let config = AnomalyConfig {
            sigma_threshold: 10.0,
            change_rate_threshold: 0.3,
        };

        let anomalies = AnomalyDetector::detect_sudden_change(&points, &config);
        assert!(anomalies.is_empty());
    }

    #[test]
    fn test_detect_combined() {
        let mut points: Vec<DataPoint> = (0..10)
            .map(|i| make_point(i, 100.0, DataQuality::Good))
            .collect();
        // Add a point that is both a sigma violation and a sudden change
        points.push(make_point(10, 500.0, DataQuality::Good));

        let config = AnomalyConfig {
            sigma_threshold: 2.0,
            change_rate_threshold: 0.3,
        };

        let anomalies = AnomalyDetector::detect(&points, &config);
        assert!(anomalies.len() >= 2); // at least one sigma + one sudden change
    }

    #[test]
    fn test_detect_empty() {
        let points: Vec<DataPoint> = vec![];
        let config = AnomalyConfig::default();
        let anomalies = AnomalyDetector::detect(&points, &config);
        assert!(anomalies.is_empty());
    }

    #[test]
    fn test_detect_single_point() {
        let points = vec![make_point(0, 42.0, DataQuality::Good)];
        let config = AnomalyConfig::default();
        let anomalies = AnomalyDetector::detect(&points, &config);
        assert!(anomalies.is_empty());
    }

    #[test]
    fn test_detect_all_bad_quality() {
        let points = vec![
            make_point(0, 10.0, DataQuality::Bad),
            make_point(1, 1000.0, DataQuality::Bad),
        ];
        let config = AnomalyConfig::default();
        let anomalies = AnomalyDetector::detect(&points, &config);
        assert!(anomalies.is_empty());
    }

    #[test]
    fn test_anomaly_config_default() {
        let config = AnomalyConfig::default();
        assert_eq!(config.sigma_threshold, 3.0);
        assert_eq!(config.change_rate_threshold, 0.3);
    }
}
