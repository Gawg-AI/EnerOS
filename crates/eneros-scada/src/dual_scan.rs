use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use tracing::info;

use crate::collector::DataSource;
use crate::config::{ScadaConfig, ScadaPoint};
use crate::pipeline::DataPipeline;
use eneros_timeseries::TimeSeriesEngine;

/// Classification of a scan point into fast or normal group
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScanGroup {
    /// Fast scan group — protection signals, frequency, voltage, breaker position
    Fast,
    /// Normal scan group — power, temperature, equipment status
    Normal,
}

/// Dual scan group configuration: separates SCADA points into fast and normal groups.
pub struct DualScanGroup {
    /// Points in the fast scan group
    pub fast_points: Vec<ScadaPoint>,
    /// Points in the normal scan group
    pub normal_points: Vec<ScadaPoint>,
    /// Fast group scan interval (default: 100ms)
    pub fast_interval: Duration,
    /// Normal group scan interval (default: 1000ms)
    pub normal_interval: Duration,
}

impl DualScanGroup {
    /// Create a new dual scan group with default intervals (100ms fast, 1000ms normal).
    pub fn new(fast_points: Vec<ScadaPoint>, normal_points: Vec<ScadaPoint>) -> Self {
        Self {
            fast_points,
            normal_points,
            fast_interval: Duration::from_millis(100),
            normal_interval: Duration::from_millis(1000),
        }
    }

    /// Create with custom intervals.
    pub fn with_intervals(
        fast_points: Vec<ScadaPoint>,
        normal_points: Vec<ScadaPoint>,
        fast_interval: Duration,
        normal_interval: Duration,
    ) -> Self {
        Self {
            fast_points,
            normal_points,
            fast_interval,
            normal_interval,
        }
    }

    /// Auto-classify points based on parameter name and scan rate.
    /// Points with scan_rate_ms <= 200 or matching fast-scan patterns go to fast group.
    /// Everything else goes to normal group.
    pub fn auto_classify(points: Vec<ScadaPoint>) -> Self {
        let mut fast = Vec::new();
        let mut normal = Vec::new();

        for point in points {
            match classify_point(&point) {
                ScanGroup::Fast => fast.push(point),
                ScanGroup::Normal => normal.push(point),
            }
        }

        Self::new(fast, normal)
    }

    /// Total number of points across both groups.
    pub fn total_points(&self) -> usize {
        self.fast_points.len() + self.normal_points.len()
    }

    /// Whether the fast group has any points.
    pub fn has_fast_group(&self) -> bool {
        !self.fast_points.is_empty()
    }

    /// Whether the normal group has any points.
    pub fn has_normal_group(&self) -> bool {
        !self.normal_points.is_empty()
    }
}

/// Builder for constructing DualScanGroup with custom configuration.
pub struct DualScanGroupBuilder {
    fast_points: Vec<ScadaPoint>,
    normal_points: Vec<ScadaPoint>,
    fast_interval: Duration,
    normal_interval: Duration,
}

impl DualScanGroupBuilder {
    pub fn new() -> Self {
        Self {
            fast_points: Vec::new(),
            normal_points: Vec::new(),
            fast_interval: Duration::from_millis(100),
            normal_interval: Duration::from_millis(1000),
        }
    }

    /// Add points to the fast scan group.
    pub fn fast(mut self, points: Vec<ScadaPoint>) -> Self {
        self.fast_points = points;
        self
    }

    /// Add points to the normal scan group.
    pub fn normal(mut self, points: Vec<ScadaPoint>) -> Self {
        self.normal_points = points;
        self
    }

    /// Auto-classify a set of points into fast/normal groups.
    pub fn auto_classify(mut self, points: Vec<ScadaPoint>) -> Self {
        let classified = DualScanGroup::auto_classify(points);
        self.fast_points = classified.fast_points;
        self.normal_points = classified.normal_points;
        self
    }

    /// Set the fast scan interval.
    pub fn fast_interval(mut self, interval: Duration) -> Self {
        self.fast_interval = interval;
        self
    }

    /// Set the normal scan interval.
    pub fn normal_interval(mut self, interval: Duration) -> Self {
        self.normal_interval = interval;
        self
    }

    /// Build the DualScanGroup.
    pub fn build(self) -> DualScanGroup {
        DualScanGroup::with_intervals(
            self.fast_points,
            self.normal_points,
            self.fast_interval,
            self.normal_interval,
        )
    }
}

impl Default for DualScanGroupBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Handles for the dual scan background tasks.
pub struct DualScanHandles {
    /// JoinHandle for the fast scan task
    pub fast_handle: Option<JoinHandle<()>>,
    /// JoinHandle for the normal scan task
    pub normal_handle: Option<JoinHandle<()>>,
}

impl DualScanHandles {
    /// Abort both scan tasks.
    pub fn abort(&self) {
        if let Some(h) = &self.fast_handle {
            h.abort();
        }
        if let Some(h) = &self.normal_handle {
            h.abort();
        }
    }
}

/// Start dual scan pipelines with the given data source and time-series engine.
/// Returns handles for both the fast and normal scan tasks.
pub fn start_dual_scan(
    group: &DualScanGroup,
    data_source: Arc<dyn DataSource>,
    ts_engine: Arc<TimeSeriesEngine>,
) -> DualScanHandles {
    let fast_handle = if !group.fast_points.is_empty() {
        let config = ScadaConfig {
            points: group.fast_points.clone(),
            default_scan_rate_ms: group.fast_interval.as_millis() as u64,
            timeout_ms: 5000,
            enable_quality_check: true,
        };
        let collector = Arc::new(crate::collector::ScadaCollector::new(config, data_source.clone()));
        let pipeline = DataPipeline::new(collector, ts_engine.clone());
        let interval = group.fast_interval.as_millis() as u64;
        info!(
            "Starting fast scan group ({}ms interval, {} points)",
            interval,
            group.fast_points.len()
        );
        Some(pipeline.start(interval))
    } else {
        None
    };

    let normal_handle = if !group.normal_points.is_empty() {
        let config = ScadaConfig {
            points: group.normal_points.clone(),
            default_scan_rate_ms: group.normal_interval.as_millis() as u64,
            timeout_ms: 5000,
            enable_quality_check: true,
        };
        let collector = Arc::new(crate::collector::ScadaCollector::new(config, data_source.clone()));
        let pipeline = DataPipeline::new(collector, ts_engine);
        let interval = group.normal_interval.as_millis() as u64;
        info!(
            "Starting normal scan group ({}ms interval, {} points)",
            interval,
            group.normal_points.len()
        );
        Some(pipeline.start(interval))
    } else {
        None
    };

    DualScanHandles {
        fast_handle,
        normal_handle,
    }
}

/// Classify a single SCADA point into fast or normal group.
fn classify_point(point: &ScadaPoint) -> ScanGroup {
    // Fast scan if scan_rate_ms is explicitly set to <= 200ms
    if point.scan_rate_ms > 0 && point.scan_rate_ms <= 200 {
        return ScanGroup::Fast;
    }

    // Fast scan patterns: frequency, voltage, breaker/switch position
    let param = point.parameter.to_lowercase();
    let fast_patterns = [
        "freq",
        "frequency",
        "volt",
        "voltage",
        "breaker",
        "switch",
        "position",
        "current",
        "relay",
    ];

    for pattern in &fast_patterns {
        if param.contains(pattern) {
            return ScanGroup::Fast;
        }
    }

    // Default to normal
    ScanGroup::Normal
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ScadaPoint;

    fn make_point(element_id: u64, parameter: &str, scan_rate_ms: u64) -> ScadaPoint {
        ScadaPoint {
            element_id,
            parameter: parameter.to_string(),
            scan_rate_ms,
            deadband: 0.01,
            min_value: None,
            max_value: None,
        }
    }

    #[test]
    fn test_auto_classify_fast_patterns() {
        let points = vec![
            make_point(1, "frequency_hz", 1000),
            make_point(2, "voltage_pu", 1000),
            make_point(3, "breaker_status", 1000),
        ];

        let group = DualScanGroup::auto_classify(points);

        assert_eq!(group.fast_points.len(), 3);
        assert!(group.normal_points.is_empty());
    }

    #[test]
    fn test_auto_classify_normal_patterns() {
        let points = vec![
            make_point(1, "active_power_mw", 1000),
            make_point(2, "temperature_c", 1000),
            make_point(3, "reactive_power_mvar", 1000),
        ];

        let group = DualScanGroup::auto_classify(points);

        assert!(group.fast_points.is_empty());
        assert_eq!(group.normal_points.len(), 3);
    }

    #[test]
    fn test_auto_classify_scan_rate() {
        let points = vec![
            make_point(1, "some_param", 100),  // <= 200 → fast
            make_point(2, "other_param", 200),  // <= 200 → fast
            make_point(3, "another_param", 500), // > 200, no fast pattern → normal
        ];

        let group = DualScanGroup::auto_classify(points);

        assert_eq!(group.fast_points.len(), 2);
        assert_eq!(group.normal_points.len(), 1);
    }

    #[test]
    fn test_builder_pattern() {
        let fast_points = vec![make_point(1, "frequency_hz", 100)];
        let normal_points = vec![make_point(2, "active_power_mw", 1000)];

        let group = DualScanGroupBuilder::new()
            .fast(fast_points.clone())
            .normal(normal_points.clone())
            .fast_interval(Duration::from_millis(50))
            .normal_interval(Duration::from_millis(500))
            .build();

        assert_eq!(group.fast_points.len(), 1);
        assert_eq!(group.normal_points.len(), 1);
        assert_eq!(group.fast_interval, Duration::from_millis(50));
        assert_eq!(group.normal_interval, Duration::from_millis(500));
    }

    #[test]
    fn test_total_points() {
        let group = DualScanGroup::new(
            vec![make_point(1, "frequency_hz", 100), make_point(2, "voltage_pu", 100)],
            vec![make_point(3, "active_power_mw", 1000)],
        );

        assert_eq!(group.total_points(), 3);
    }

    #[test]
    fn test_empty_groups() {
        let group = DualScanGroup::new(vec![], vec![]);

        assert!(!group.has_fast_group());
        assert!(!group.has_normal_group());
        assert_eq!(group.total_points(), 0);
    }
}
