use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use eneros_core::ElementId;
use eneros_device::DataQuality;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::config::ScadaConfig;

/// Trait for reading data from a data source (device, simulator, etc.)
pub trait DataSource: Send + Sync {
    /// Read a value for the given element and parameter.
    /// Returns None if the read fails.
    fn read(&self, element_id: ElementId, parameter: &str) -> Option<f64>;
}

/// A single SCADA reading
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScadaReading {
    /// Element ID
    pub element_id: ElementId,
    /// Parameter name
    pub parameter: String,
    /// Measured value
    pub value: f64,
    /// Data quality
    pub quality: DataQuality,
    /// Timestamp of the reading
    pub timestamp: DateTime<Utc>,
    /// Configured scan rate
    pub scan_rate_ms: u64,
}

/// SCADA data collector
pub struct ScadaCollector {
    /// Configuration
    config: ScadaConfig,
    /// Data source
    data_source: Arc<dyn DataSource>,
    /// Latest readings per (element_id, parameter)
    latest_values: RwLock<HashMap<(ElementId, String), ScadaReading>>,
}

impl ScadaCollector {
    /// Create a new SCADA collector
    pub fn new(config: ScadaConfig, data_source: Arc<dyn DataSource>) -> Self {
        Self {
            config,
            data_source,
            latest_values: RwLock::new(HashMap::new()),
        }
    }

    /// Perform a single collection cycle: read all configured points
    pub fn collect_once(&self) -> Vec<ScadaReading> {
        let now = Utc::now();
        let mut readings = Vec::with_capacity(self.config.points.len());

        for point in &self.config.points {
            let scan_rate = if point.scan_rate_ms > 0 {
                point.scan_rate_ms
            } else {
                self.config.default_scan_rate_ms
            };

            let (value, quality) = match self.data_source.read(point.element_id, &point.parameter) {
                Some(v) => {
                    if self.config.enable_quality_check {
                        let quality = self.check_quality(v, point.min_value, point.max_value);
                        (v, quality)
                    } else {
                        (v, DataQuality::Good)
                    }
                }
                None => (f64::NAN, DataQuality::Bad),
            };

            let reading = ScadaReading {
                element_id: point.element_id,
                parameter: point.parameter.clone(),
                value,
                quality,
                timestamp: now,
                scan_rate_ms: scan_rate,
            };

            // Update latest values
            {
                let mut latest = self.latest_values.write();
                latest.insert((point.element_id, point.parameter.clone()), reading.clone());
            }

            readings.push(reading);
        }

        readings
    }

    /// Check data quality based on configured limits
    fn check_quality(
        &self,
        value: f64,
        min_value: Option<f64>,
        max_value: Option<f64>,
    ) -> DataQuality {
        if let Some(min) = min_value {
            if value < min {
                return DataQuality::Bad;
            }
        }
        if let Some(max) = max_value {
            if value > max {
                return DataQuality::Bad;
            }
        }
        DataQuality::Good
    }

    /// Get the latest reading for a specific element and parameter
    pub fn latest(&self, element_id: ElementId, parameter: &str) -> Option<ScadaReading> {
        let latest = self.latest_values.read();
        latest.get(&(element_id, parameter.to_string())).cloned()
    }

    /// Get all latest readings
    pub fn latest_all(&self) -> Vec<ScadaReading> {
        let latest = self.latest_values.read();
        latest.values().cloned().collect()
    }
}

/// Mock data source for testing
pub struct MockDataSource {
    data: RwLock<HashMap<(ElementId, String), f64>>,
}

impl MockDataSource {
    /// Create a new mock data source
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }

    /// Insert a value
    pub fn insert(&self, element_id: ElementId, parameter: &str, value: f64) {
        let mut data = self.data.write();
        data.insert((element_id, parameter.to_string()), value);
    }

    /// Remove a value (simulates read failure)
    pub fn remove(&self, element_id: ElementId, parameter: &str) {
        let mut data = self.data.write();
        data.remove(&(element_id, parameter.to_string()));
    }
}

impl Default for MockDataSource {
    fn default() -> Self {
        Self::new()
    }
}

impl DataSource for MockDataSource {
    fn read(&self, element_id: ElementId, parameter: &str) -> Option<f64> {
        let data = self.data.read();
        data.get(&(element_id, parameter.to_string())).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ScadaPoint;

    fn make_config(points: Vec<ScadaPoint>) -> ScadaConfig {
        ScadaConfig {
            points,
            default_scan_rate_ms: 1000,
            timeout_ms: 5000,
            enable_quality_check: true,
        }
    }

    #[test]
    fn test_collect_once_good_quality() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 1.02);

        let config = make_config(vec![ScadaPoint {
            element_id: 1,
            parameter: "voltage_pu".to_string(),
            scan_rate_ms: 500,
            deadband: 0.01,
            min_value: Some(0.8),
            max_value: Some(1.2),
        }]);

        let collector = ScadaCollector::new(config, mock);
        let readings = collector.collect_once();

        assert_eq!(readings.len(), 1);
        assert_eq!(readings[0].element_id, 1);
        assert_eq!(readings[0].parameter, "voltage_pu");
        assert!((readings[0].value - 1.02).abs() < f64::EPSILON);
        assert_eq!(readings[0].quality, DataQuality::Good);
        assert_eq!(readings[0].scan_rate_ms, 500);
    }

    #[test]
    fn test_collect_once_bad_out_of_range_low() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 0.5);

        let config = make_config(vec![ScadaPoint {
            element_id: 1,
            parameter: "voltage_pu".to_string(),
            scan_rate_ms: 500,
            deadband: 0.01,
            min_value: Some(0.8),
            max_value: Some(1.2),
        }]);

        let collector = ScadaCollector::new(config, mock);
        let readings = collector.collect_once();

        assert_eq!(readings.len(), 1);
        assert!((readings[0].value - 0.5).abs() < f64::EPSILON);
        assert_eq!(readings[0].quality, DataQuality::Bad);
    }

    #[test]
    fn test_collect_once_bad_out_of_range_high() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 1.5);

        let config = make_config(vec![ScadaPoint {
            element_id: 1,
            parameter: "voltage_pu".to_string(),
            scan_rate_ms: 500,
            deadband: 0.01,
            min_value: Some(0.8),
            max_value: Some(1.2),
        }]);

        let collector = ScadaCollector::new(config, mock);
        let readings = collector.collect_once();

        assert_eq!(readings.len(), 1);
        assert!((readings[0].value - 1.5).abs() < f64::EPSILON);
        assert_eq!(readings[0].quality, DataQuality::Bad);
    }

    #[test]
    fn test_collect_once_bad_read_failure() {
        let mock = Arc::new(MockDataSource::new());
        // Don't insert any data — read will fail

        let config = make_config(vec![ScadaPoint {
            element_id: 1,
            parameter: "voltage_pu".to_string(),
            scan_rate_ms: 500,
            deadband: 0.01,
            min_value: Some(0.8),
            max_value: Some(1.2),
        }]);

        let collector = ScadaCollector::new(config, mock);
        let readings = collector.collect_once();

        assert_eq!(readings.len(), 1);
        assert!(readings[0].value.is_nan());
        assert_eq!(readings[0].quality, DataQuality::Bad);
    }

    #[test]
    fn test_collect_once_no_quality_check() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 1.5);

        let config = ScadaConfig {
            points: vec![ScadaPoint {
                element_id: 1,
                parameter: "voltage_pu".to_string(),
                scan_rate_ms: 500,
                deadband: 0.01,
                min_value: Some(0.8),
                max_value: Some(1.2),
            }],
            default_scan_rate_ms: 1000,
            timeout_ms: 5000,
            enable_quality_check: false,
        };

        let collector = ScadaCollector::new(config, mock);
        let readings = collector.collect_once();

        assert_eq!(readings[0].quality, DataQuality::Good);
    }

    #[test]
    fn test_collect_once_no_limits() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "frequency_hz", 50.5);

        let config = make_config(vec![ScadaPoint {
            element_id: 1,
            parameter: "frequency_hz".to_string(),
            scan_rate_ms: 1000,
            deadband: 0.0,
            min_value: None,
            max_value: None,
        }]);

        let collector = ScadaCollector::new(config, mock);
        let readings = collector.collect_once();

        assert_eq!(readings[0].quality, DataQuality::Good);
    }

    #[test]
    fn test_latest() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 1.02);

        let config = make_config(vec![ScadaPoint {
            element_id: 1,
            parameter: "voltage_pu".to_string(),
            scan_rate_ms: 500,
            deadband: 0.01,
            min_value: None,
            max_value: None,
        }]);

        let collector = ScadaCollector::new(config, mock);

        // Before collection, no latest
        assert!(collector.latest(1, "voltage_pu").is_none());

        collector.collect_once();

        let reading = collector.latest(1, "voltage_pu").unwrap();
        assert!((reading.value - 1.02).abs() < f64::EPSILON);

        // Non-existent parameter
        assert!(collector.latest(1, "current_a").is_none());
    }

    #[test]
    fn test_latest_all() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 1.02);
        mock.insert(2, "active_power_mw", 50.0);

        let config = make_config(vec![
            ScadaPoint {
                element_id: 1,
                parameter: "voltage_pu".to_string(),
                scan_rate_ms: 500,
                deadband: 0.01,
                min_value: None,
                max_value: None,
            },
            ScadaPoint {
                element_id: 2,
                parameter: "active_power_mw".to_string(),
                scan_rate_ms: 1000,
                deadband: 0.1,
                min_value: None,
                max_value: None,
            },
        ]);

        let collector = ScadaCollector::new(config, mock);
        collector.collect_once();

        let all = collector.latest_all();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_default_scan_rate_fallback() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 1.02);

        let config = make_config(vec![ScadaPoint {
            element_id: 1,
            parameter: "voltage_pu".to_string(),
            scan_rate_ms: 0, // zero means use default
            deadband: 0.01,
            min_value: None,
            max_value: None,
        }]);

        let collector = ScadaCollector::new(config, mock);
        let readings = collector.collect_once();

        assert_eq!(readings[0].scan_rate_ms, 1000); // falls back to default
    }

    #[test]
    fn test_mock_data_source() {
        let mock = MockDataSource::new();
        mock.insert(1, "voltage_pu", 1.05);
        assert!((mock.read(1, "voltage_pu").unwrap() - 1.05).abs() < f64::EPSILON);
        assert!(mock.read(2, "voltage_pu").is_none());

        mock.remove(1, "voltage_pu");
        assert!(mock.read(1, "voltage_pu").is_none());
    }
}
