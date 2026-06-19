use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::info;

use crate::collector::DataSource;
use crate::config::{ScadaConfig, ScadaPoint};
use crate::pipeline::DataPipeline;
use eneros_eventbus::EventBus;
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
    /// Uses default intervals (100ms fast, 1000ms normal).
    pub fn auto_classify(points: Vec<ScadaPoint>) -> Self {
        Self::auto_classify_with_intervals(points, Duration::from_millis(100), Duration::from_millis(1000))
    }

    /// Auto-classify points with custom fast/normal intervals.
    /// This is the preferred constructor when config-driven intervals are available.
    pub fn auto_classify_with_intervals(
        points: Vec<ScadaPoint>,
        fast_interval: Duration,
        normal_interval: Duration,
    ) -> Self {
        let mut fast = Vec::new();
        let mut normal = Vec::new();

        for point in points {
            match classify_point(&point) {
                ScanGroup::Fast => fast.push(point),
                ScanGroup::Normal => normal.push(point),
            }
        }

        Self::with_intervals(fast, normal, fast_interval, normal_interval)
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
///
/// Each entry holds the `JoinHandle` and the corresponding shutdown signal
/// sender (`watch::Sender<bool>`). Sending `true` (or dropping the sender)
/// causes the background task to exit its loop gracefully after finishing
/// the current cycle.
pub struct DualScanHandles {
    /// JoinHandle + shutdown sender for the fast scan task
    pub fast: Option<(JoinHandle<()>, watch::Sender<bool>)>,
    /// JoinHandle + shutdown sender for the normal scan task
    pub normal: Option<(JoinHandle<()>, watch::Sender<bool>)>,
}

impl DualScanHandles {
    /// Abort both scan tasks immediately (non-graceful).
    ///
    /// This is the fallback for `Drop`. Prefer `shutdown()` for graceful
    /// termination.
    pub fn abort(&self) {
        if let Some((h, _)) = &self.fast {
            h.abort();
        }
        if let Some((h, _)) = &self.normal {
            h.abort();
        }
    }

    /// Gracefully shut down both scan tasks.
    ///
    /// Sends the shutdown signal, then awaits both tasks to finish their
    /// current cycle. This ensures no partial writes to the time-series
    /// engine.
    pub async fn shutdown(mut self) {
        // Take the options out so Drop doesn't fire on them (we handle
        // shutdown explicitly here). Drop will still run on `self` but
        // the fields will be None at that point — the Drop impl checks
        // for None via the Option.
        if let Some((handle, tx)) = self.fast.take() {
            let _ = tx.send(true);
            let _ = handle.await;
        }
        if let Some((handle, tx)) = self.normal.take() {
            let _ = tx.send(true);
            let _ = handle.await;
        }
    }
}

impl Drop for DualScanHandles {
    fn drop(&mut self) {
        // Best-effort graceful shutdown: signal both tasks to stop.
        // If the tasks don't exit in time, they will be leaked (tokio will
        // clean them up when the runtime shuts down). This is safer than
        // `abort()` which can interrupt a write mid-flight.
        if let Some((_, tx)) = &self.fast {
            let _ = tx.send(true);
        }
        if let Some((_, tx)) = &self.normal {
            let _ = tx.send(true);
        }
    }
}

/// Options for `start_dual_scan`.
pub struct DualScanOptions {
    /// Timeout in milliseconds for each collection cycle.
    pub timeout_ms: u64,
    /// Whether to enable quality checks on collected readings.
    pub enable_quality_check: bool,
    /// Optional event bus for publishing `DataReceived` events.
    pub event_bus: Option<Arc<EventBus>>,
}

impl Default for DualScanOptions {
    fn default() -> Self {
        Self {
            timeout_ms: 5000,
            enable_quality_check: true,
            event_bus: None,
        }
    }
}

/// Start dual scan pipelines with the given data source and time-series engine.
///
/// Returns handles for both the fast and normal scan tasks. The caller should
/// use `DualScanHandles::shutdown()` for graceful termination.
pub fn start_dual_scan(
    group: &DualScanGroup,
    data_source: Arc<dyn DataSource>,
    ts_engine: Arc<TimeSeriesEngine>,
    options: DualScanOptions,
) -> DualScanHandles {
    let fast = if !group.fast_points.is_empty() {
        let config = ScadaConfig {
            points: group.fast_points.clone(),
            default_scan_rate_ms: group.fast_interval.as_millis() as u64,
            timeout_ms: options.timeout_ms,
            enable_quality_check: options.enable_quality_check,
        };
        let collector = Arc::new(crate::collector::ScadaCollector::new(config, data_source.clone()));
        let mut pipeline = DataPipeline::new(collector, ts_engine.clone());
        if let Some(ref bus) = options.event_bus {
            pipeline = pipeline.with_event_bus(bus.clone());
        }
        let interval = group.fast_interval.as_millis() as u64;
        info!(
            "Starting fast scan group ({}ms interval, {} points)",
            interval,
            group.fast_points.len()
        );
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = pipeline.start_with_shutdown(interval, shutdown_rx);
        Some((handle, shutdown_tx))
    } else {
        None
    };

    let normal = if !group.normal_points.is_empty() {
        let config = ScadaConfig {
            points: group.normal_points.clone(),
            default_scan_rate_ms: group.normal_interval.as_millis() as u64,
            timeout_ms: options.timeout_ms,
            enable_quality_check: options.enable_quality_check,
        };
        let collector = Arc::new(crate::collector::ScadaCollector::new(config, data_source.clone()));
        let mut pipeline = DataPipeline::new(collector, ts_engine);
        if let Some(ref bus) = options.event_bus {
            pipeline = pipeline.with_event_bus(bus.clone());
        }
        let interval = group.normal_interval.as_millis() as u64;
        info!(
            "Starting normal scan group ({}ms interval, {} points)",
            interval,
            group.normal_points.len()
        );
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = pipeline.start_with_shutdown(interval, shutdown_rx);
        Some((handle, shutdown_tx))
    } else {
        None
    };

    DualScanHandles { fast, normal }
}

/// Classify a single SCADA point into fast or normal group.
fn classify_point(point: &ScadaPoint) -> ScanGroup {
    // Fast scan if scan_rate_ms is explicitly set to <= 200ms
    if point.scan_rate_ms > 0 && point.scan_rate_ms <= 200 {
        return ScanGroup::Fast;
    }

    // Fast scan patterns: frequency, voltage, breaker/switch position, relay
    let param = point.parameter.to_lowercase();
    let fast_patterns = [
        "freq",
        "frequency",
        "volt",
        "voltage",
        "breaker",
        "switch",
        "position",
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
            // current is no longer auto-classified as fast (it's a measurement, not protection)
            make_point(4, "current_a", 1000),
        ];

        let group = DualScanGroup::auto_classify(points);

        assert!(group.fast_points.is_empty());
        assert_eq!(group.normal_points.len(), 4);
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
    fn test_auto_classify_with_custom_intervals() {
        let points = vec![
            make_point(1, "frequency_hz", 100),
            make_point(2, "active_power_mw", 1000),
        ];

        let group = DualScanGroup::auto_classify_with_intervals(
            points,
            Duration::from_millis(50),
            Duration::from_millis(500),
        );

        assert_eq!(group.fast_points.len(), 1);
        assert_eq!(group.normal_points.len(), 1);
        assert_eq!(group.fast_interval, Duration::from_millis(50));
        assert_eq!(group.normal_interval, Duration::from_millis(500));
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

    #[tokio::test]
    async fn test_dual_scan_shutdown_graceful() {
        use crate::collector::MockDataSource;

        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "frequency_hz", 50.0);
        mock.insert(2, "active_power_mw", 100.0);

        let group = DualScanGroup::with_intervals(
            vec![ScadaPoint {
                element_id: 1,
                parameter: "frequency_hz".to_string(),
                scan_rate_ms: 100,
                deadband: 0.01,
                min_value: None,
                max_value: None,
            }],
            vec![ScadaPoint {
                element_id: 2,
                parameter: "active_power_mw".to_string(),
                scan_rate_ms: 1000,
                deadband: 0.1,
                min_value: None,
                max_value: None,
            }],
            Duration::from_millis(50),
            Duration::from_millis(100),
        );

        let ts_engine = Arc::new(TimeSeriesEngine::new(1000));
        let handles = start_dual_scan(
            &group,
            mock,
            ts_engine.clone(),
            DualScanOptions::default(),
        );

        // Let it run for a few cycles
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Graceful shutdown should complete without hanging
        handles.shutdown().await;

        // Verify data was written
        let dp = ts_engine.latest(1, "frequency_hz").unwrap();
        assert!((dp.value - 50.0).abs() < f64::EPSILON);
    }
}
