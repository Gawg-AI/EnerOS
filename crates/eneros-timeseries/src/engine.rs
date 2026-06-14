use std::collections::HashMap;
use parking_lot::RwLock;
use chrono::{DateTime, Utc};
use eneros_core::{ElementId, Result};

use crate::aggregation::{WindowedAggregator, WindowSpec, WindowedResult};

/// Time-series data point
#[derive(Debug, Clone)]
pub struct DataPoint {
    pub timestamp: DateTime<Utc>,
    pub value: f64,
    pub quality: DataQuality,
}

/// Data quality indicator
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataQuality {
    Good,
    Uncertain,
    Bad,
}

/// Time-series data for an element
#[derive(Debug, Clone)]
pub struct TimeSeries {
    pub element_id: ElementId,
    pub parameter: String,
    pub data_points: Vec<DataPoint>,
}

/// Time-series engine for storing and querying historical data
pub struct TimeSeriesEngine {
    storage: RwLock<HashMap<(ElementId, String), Vec<DataPoint>>>,
    max_retention: usize,
}

impl TimeSeriesEngine {
    /// Create a new time-series engine
    pub fn new(max_retention: usize) -> Self {
        Self {
            storage: RwLock::new(HashMap::new()),
            max_retention,
        }
    }

    /// Record a data point
    pub fn record(
        &self,
        element_id: ElementId,
        parameter: &str,
        value: f64,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let mut storage = self.storage.write();
        let key = (element_id, parameter.to_string());

        let data_points = storage.entry(key).or_default();
        data_points.push(DataPoint {
            timestamp,
            value,
            quality: DataQuality::Good,
        });

        // Trim old data if超过限制
        if data_points.len() > self.max_retention {
            let excess = data_points.len() - self.max_retention;
            data_points.drain(0..excess);
        }

        Ok(())
    }

    /// Query historical data
    pub fn query(
        &self,
        element_id: ElementId,
        parameter: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Vec<DataPoint> {
        let storage = self.storage.read();
        let key = (element_id, parameter.to_string());

        storage
            .get(&key)
            .map(|points| {
                points
                    .iter()
                    .filter(|p| p.timestamp >= start && p.timestamp <= end)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get latest value
    pub fn latest(
        &self,
        element_id: ElementId,
        parameter: &str,
    ) -> Option<DataPoint> {
        let storage = self.storage.read();
        let key = (element_id, parameter.to_string());

        storage.get(&key).and_then(|points| points.last().cloned())
    }

    /// Get storage statistics
    pub fn statistics(&self) -> TimeSeriesStatistics {
        let storage = self.storage.read();
        let total_points: usize = storage.values().map(|v| v.len()).sum();
        let series_count = storage.len();

        TimeSeriesStatistics {
            series_count,
            total_points,
            max_retention: self.max_retention,
        }
    }

    /// Query and aggregate data in one call using sliding window aggregation
    pub fn query_aggregated(
        &self,
        element_id: ElementId,
        parameter: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        window_secs: u64,
    ) -> Vec<WindowedResult> {
        let points = self.query(element_id, parameter, start, end);
        let spec = WindowSpec {
            window_size_secs: window_secs,
            step_size_secs: window_secs,
        };
        WindowedAggregator::aggregate(&points, &spec)
    }
}

impl Default for TimeSeriesEngine {
    fn default() -> Self {
        Self::new(100_000)
    }
}

/// Time-series engine statistics
#[derive(Debug, Clone)]
pub struct TimeSeriesStatistics {
    pub series_count: usize,
    pub total_points: usize,
    pub max_retention: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_query_aggregated() {
        let engine = TimeSeriesEngine::new(100_000);
        let element_id: ElementId = 1;
        let param = "temperature";

        let base = Utc.timestamp_opt(0, 0).unwrap();
        for i in 0..20 {
            let ts = base + chrono::Duration::seconds(i * 5);
            engine.record(element_id, param, i as f64 * 10.0, ts).unwrap();
        }

        let start = base;
        let end = base + chrono::Duration::seconds(100);

        let results = engine.query_aggregated(element_id, param, start, end, 50);
        assert!(!results.is_empty());

        // First window [0, 50): points at 0, 5, 10, 15, 20, 25, 30, 35, 40, 45
        assert_eq!(results[0].count, 10);
    }

    #[test]
    fn test_query_aggregated_empty() {
        let engine = TimeSeriesEngine::new(100_000);
        let element_id: ElementId = 99;
        let start = Utc.timestamp_opt(0, 0).unwrap();
        let end = Utc.timestamp_opt(100, 0).unwrap();

        let results = engine.query_aggregated(element_id, "nonexistent", start, end, 10);
        assert!(results.is_empty());
    }
}
