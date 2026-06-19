//! Metrics collection and Prometheus export (v0.6.0 — S3).
//!
//! Provides a lightweight in-process metrics registry that collects counters,
//! gauges, and histograms, and exports them in Prometheus text format at
//! `GET /metrics`.
//!
//! ## Metric naming convention
//!
//! All metrics use the `eneros_` prefix and follow Prometheus naming rules:
//! - Counters: `eneros_<subsystem>_<event>_total{labels}`
//! - Gauges: `eneros_<subsystem>_<state>`
//! - Histograms: `eneros_<subsystem>_<operation>_seconds`
//!
//! ## Example output
//!
//! ```text
//! # HELP eneros_commands_total Total commands dispatched
//! # TYPE eneros_commands_total counter
//! eneros_commands_total{result="success"} 1234
//! eneros_commands_total{result="failed"} 5
//! # HELP eneros_command_queue_depth Current command queue depth
//! # TYPE eneros_command_queue_depth gauge
//! eneros_command_queue_depth 3
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use parking_lot::RwLock;
use serde::Serialize;

/// A monotonically increasing counter.
#[derive(Debug)]
pub struct Counter {
    name: String,
    help: String,
    value: AtomicU64,
    labels: Vec<(String, String)>,
}

impl Counter {
    /// Create a new counter with the given name and help text.
    pub fn new(name: impl Into<String>, help: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            help: help.into(),
            value: AtomicU64::new(0),
            labels: Vec::new(),
        }
    }

    /// Create a new counter with labels.
    pub fn with_labels(
        name: impl Into<String>,
        help: impl Into<String>,
        labels: Vec<(String, String)>,
    ) -> Self {
        Self {
            name: name.into(),
            help: help.into(),
            value: AtomicU64::new(0),
            labels,
        }
    }

    /// Increment the counter by 1.
    pub fn inc(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the counter by a specific amount.
    pub fn inc_by(&self, delta: u64) {
        self.value.fetch_add(delta, Ordering::Relaxed);
    }

    /// Get the current value.
    pub fn value(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }

    /// Format as Prometheus text.
    fn to_prometheus(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("# HELP {} {}\n", self.name, self.help));
        out.push_str(&format!("# TYPE {} counter\n", self.name));
        if self.labels.is_empty() {
            out.push_str(&format!("{} {}\n", self.name, self.value()));
        } else {
            let label_str = self
                .labels
                .iter()
                .map(|(k, v)| format!("{}=\"{}\"", k, v))
                .collect::<Vec<_>>()
                .join(",");
            out.push_str(&format!("{}{{{}}} {}\n", self.name, label_str, self.value()));
        }
        out
    }
}

/// A gauge that can go up and down.
#[derive(Debug)]
pub struct Gauge {
    name: String,
    help: String,
    value: AtomicU64,
}

impl Gauge {
    /// Create a new gauge.
    pub fn new(name: impl Into<String>, help: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            help: help.into(),
            value: AtomicU64::new(0),
        }
    }

    /// Set the gauge value (stored as bits of u64).
    pub fn set(&self, value: f64) {
        self.value.store(value.to_bits(), Ordering::Relaxed);
    }

    /// Get the current value.
    pub fn value(&self) -> f64 {
        f64::from_bits(self.value.load(Ordering::Relaxed))
    }

    /// Format as Prometheus text.
    fn to_prometheus(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("# HELP {} {}\n", self.name, self.help));
        out.push_str(&format!("# TYPE {} gauge\n", self.name));
        out.push_str(&format!("{} {}\n", self.name, self.value()));
        out
    }
}

/// A histogram tracking value distribution across buckets.
#[derive(Debug)]
pub struct Histogram {
    name: String,
    help: String,
    bucket_bounds: Vec<f64>,
    bucket_counts: Vec<AtomicU64>,
    sum: parking_lot::Mutex<f64>,
    count: AtomicU64,
}

impl Histogram {
    /// Create a new histogram with the given bucket boundaries.
    pub fn new(
        name: impl Into<String>,
        help: impl Into<String>,
        bucket_bounds: Vec<f64>,
    ) -> Self {
        let n = bucket_bounds.len();
        Self {
            name: name.into(),
            help: help.into(),
            bucket_counts: (0..=n).map(|_| AtomicU64::new(0)).collect(),
            bucket_bounds,
            sum: parking_lot::Mutex::new(0.0),
            count: AtomicU64::new(0),
        }
    }

    /// Observe a value.
    pub fn observe(&self, value: f64) {
        for (i, &bound) in self.bucket_bounds.iter().enumerate() {
            if value <= bound {
                self.bucket_counts[i].fetch_add(1, Ordering::Relaxed);
            }
        }
        // +Inf bucket
        self.bucket_counts[self.bucket_bounds.len()].fetch_add(1, Ordering::Relaxed);
        *self.sum.lock() += value;
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// Observe a duration (convenience method).
    pub fn observe_duration(&self, start: Instant) {
        let elapsed = start.elapsed().as_secs_f64();
        self.observe(elapsed);
    }

    /// Format as Prometheus text.
    fn to_prometheus(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("# HELP {} {}\n", self.name, self.help));
        out.push_str(&format!("# TYPE {} histogram\n", self.name));

        // bucket_counts[i] is already cumulative because observe() increments
        // every bucket whose bound >= value (Prometheus convention).
        for (i, &bound) in self.bucket_bounds.iter().enumerate() {
            let count = self.bucket_counts[i].load(Ordering::Relaxed);
            out.push_str(&format!(
                "{}_bucket{{le=\"{}\"}} {}\n",
                self.name, bound, count
            ));
        }
        // +Inf bucket
        let total = self.count.load(Ordering::Relaxed);
        out.push_str(&format!("{}_bucket{{le=\"+Inf\"}} {}\n", self.name, total));
        out.push_str(&format!("{}_sum {}\n", self.name, *self.sum.lock()));
        out.push_str(&format!("{}_count {}\n", self.name, total));
        out
    }
}

/// The central metrics registry holding all EnerOS metrics.
#[derive(Debug)]
pub struct MetricsRegistry {
    // Command execution
    pub commands_success: Counter,
    pub commands_failed: Counter,
    pub command_duration: Histogram,
    pub command_queue_depth: Gauge,

    // Constraint violations
    pub constraint_violations_voltage: Counter,
    pub constraint_violations_thermal: Counter,
    pub constraint_violations_frequency: Counter,

    // Agent decisions
    pub agent_decisions: RwLock<HashMap<String, AgentDecisionCounters>>,

    // Device connections
    pub device_connections_connected: Gauge,
    pub device_connections_disconnected: Gauge,

    // Power flow
    pub powerflow_iterations: Histogram,

    // Pipeline stage timing
    pub pipeline_stage_duration: RwLock<HashMap<String, Histogram>>,

    // HTTP requests
    pub http_requests_total: Counter,
    pub http_request_duration: Histogram,
}

/// Per-agent decision counters.
#[derive(Debug, Serialize)]
pub struct AgentDecisionCounters {
    pub agent_id: String,
    pub success: u64,
    pub failed: u64,
    pub rejected: u64,
}

impl MetricsRegistry {
    /// Create a new metrics registry with default buckets.
    pub fn new() -> Self {
        Self {
            commands_success: Counter::with_labels(
                "eneros_commands_total",
                "Total commands dispatched",
                vec![("result".to_string(), "success".to_string())],
            ),
            commands_failed: Counter::with_labels(
                "eneros_commands_total",
                "Total commands dispatched",
                vec![("result".to_string(), "failed".to_string())],
            ),
            command_duration: Histogram::new(
                "eneros_command_duration_seconds",
                "Command execution duration in seconds",
                vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0],
            ),
            command_queue_depth: Gauge::new(
                "eneros_command_queue_depth",
                "Current command queue depth",
            ),
            constraint_violations_voltage: Counter::with_labels(
                "eneros_constraint_violations_total",
                "Total constraint violations detected",
                vec![("type".to_string(), "voltage".to_string())],
            ),
            constraint_violations_thermal: Counter::with_labels(
                "eneros_constraint_violations_total",
                "Total constraint violations detected",
                vec![("type".to_string(), "thermal".to_string())],
            ),
            constraint_violations_frequency: Counter::with_labels(
                "eneros_constraint_violations_total",
                "Total constraint violations detected",
                vec![("type".to_string(), "frequency".to_string())],
            ),
            agent_decisions: RwLock::new(HashMap::new()),
            device_connections_connected: Gauge::new(
                "eneros_device_connections",
                "Current device connection count",
            ),
            device_connections_disconnected: Gauge::new(
                "eneros_device_disconnections_total",
                "Total device disconnection events",
            ),
            powerflow_iterations: Histogram::new(
                "eneros_powerflow_iterations",
                "Power flow solver iteration count",
                vec![1.0, 2.0, 3.0, 5.0, 10.0, 20.0, 50.0],
            ),
            pipeline_stage_duration: RwLock::new(HashMap::new()),
            http_requests_total: Counter::new(
                "eneros_http_requests_total",
                "Total HTTP requests received",
            ),
            http_request_duration: Histogram::new(
                "eneros_http_request_duration_seconds",
                "HTTP request duration in seconds",
                vec![0.001, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0],
            ),
        }
    }

    /// Record a command execution result.
    pub fn record_command(&self, success: bool, duration: std::time::Duration) {
        if success {
            self.commands_success.inc();
        } else {
            self.commands_failed.inc();
        }
        self.command_duration.observe(duration.as_secs_f64());
    }

    /// Record a constraint violation.
    pub fn record_violation(&self, violation_type: &str) {
        match violation_type {
            "voltage" => self.constraint_violations_voltage.inc(),
            "thermal" => self.constraint_violations_thermal.inc(),
            "frequency" => self.constraint_violations_frequency.inc(),
            _ => {}
        }
    }

    /// Record an agent decision.
    pub fn record_agent_decision(&self, agent_id: &str, result: &str) {
        let mut decisions = self.agent_decisions.write();
        let counters = decisions
            .entry(agent_id.to_string())
            .or_insert_with(|| AgentDecisionCounters {
                agent_id: agent_id.to_string(),
                success: 0,
                failed: 0,
                rejected: 0,
            });
        match result {
            "success" => counters.success += 1,
            "failed" => counters.failed += 1,
            "rejected" => counters.rejected += 1,
            _ => {}
        }
    }

    /// Record a pipeline stage duration.
    pub fn record_pipeline_stage(&self, stage: &str, duration: std::time::Duration) {
        let mut stages = self.pipeline_stage_duration.write();
        let histogram = stages
            .entry(stage.to_string())
            .or_insert_with(|| {
                Histogram::new(
                    "eneros_pipeline_stage_duration_seconds".to_string(),
                    format!("Pipeline stage duration: {}", stage),
                    vec![0.0001, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5],
                )
            });
        histogram.observe(duration.as_secs_f64());
    }

    /// Record an HTTP request.
    pub fn record_http_request(&self, duration: std::time::Duration) {
        self.http_requests_total.inc();
        self.http_request_duration.observe(duration.as_secs_f64());
    }

    /// Export all metrics in Prometheus text format.
    pub fn to_prometheus(&self) -> String {
        let mut out = String::with_capacity(4096);

        // Commands
        out.push_str(&self.commands_success.to_prometheus());
        out.push_str(&self.commands_failed.to_prometheus());
        out.push_str(&self.command_duration.to_prometheus());
        out.push_str(&self.command_queue_depth.to_prometheus());

        // Constraint violations
        out.push_str(&self.constraint_violations_voltage.to_prometheus());
        out.push_str(&self.constraint_violations_thermal.to_prometheus());
        out.push_str(&self.constraint_violations_frequency.to_prometheus());

        // Agent decisions
        let decisions = self.agent_decisions.read();
        for (agent_id, counters) in decisions.iter() {
            out.push_str("# HELP eneros_agent_decisions_total Agent decisions by result\n");
            out.push_str("# TYPE eneros_agent_decisions_total counter\n");
            out.push_str(&format!(
                "eneros_agent_decisions_total{{agent=\"{}\",result=\"success\"}} {}\n",
                agent_id, counters.success
            ));
            out.push_str(&format!(
                "eneros_agent_decisions_total{{agent=\"{}\",result=\"failed\"}} {}\n",
                agent_id, counters.failed
            ));
            out.push_str(&format!(
                "eneros_agent_decisions_total{{agent=\"{}\",result=\"rejected\"}} {}\n",
                agent_id, counters.rejected
            ));
        }

        // Device connections
        out.push_str(&self.device_connections_connected.to_prometheus());
        out.push_str(&self.device_connections_disconnected.to_prometheus());

        // Power flow
        out.push_str(&self.powerflow_iterations.to_prometheus());

        // Pipeline stages
        let stages = self.pipeline_stage_duration.read();
        for (_, histogram) in stages.iter() {
            out.push_str(&histogram.to_prometheus());
        }

        // HTTP
        out.push_str(&self.http_requests_total.to_prometheus());
        out.push_str(&self.http_request_duration.to_prometheus());

        out
    }
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Axum handler for `GET /metrics` — returns Prometheus text format.
pub async fn metrics_handler(
    axum::extract::State(state): axum::extract::State<crate::app::AppState>,
) -> axum::response::Response {
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;

    let metrics = match &state.metrics_registry {
        Some(m) => m,
        None => {
            return (StatusCode::SERVICE_UNAVAILABLE, "metrics disabled").into_response();
        }
    };

    let body = metrics.to_prometheus();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_counter_basic() {
        let c = Counter::new("test_counter", "test");
        assert_eq!(c.value(), 0);
        c.inc();
        c.inc();
        assert_eq!(c.value(), 2);
        c.inc_by(10);
        assert_eq!(c.value(), 12);
    }

    #[test]
    fn test_counter_with_labels() {
        let c = Counter::with_labels(
            "test_labeled",
            "test",
            vec![("type".to_string(), "voltage".to_string())],
        );
        c.inc();
        let prom = c.to_prometheus();
        assert!(prom.contains("test_labeled{type=\"voltage\"} 1"));
    }

    #[test]
    fn test_gauge_basic() {
        let g = Gauge::new("test_gauge", "test");
        g.set(42.5);
        assert!((g.value() - 42.5).abs() < 1e-10);
        g.set(-10.0);
        assert!((g.value() - (-10.0)).abs() < 1e-10);
    }

    #[test]
    fn test_histogram_observe() {
        let h = Histogram::new("test_hist", "test", vec![0.1, 0.5, 1.0]);
        h.observe(0.05);
        h.observe(0.3);
        h.observe(0.7);
        h.observe(2.0);

        let prom = h.to_prometheus();
        assert!(prom.contains("test_hist_bucket{le=\"0.1\"} 1"));
        assert!(prom.contains("test_hist_bucket{le=\"0.5\"} 2"));
        assert!(prom.contains("test_hist_bucket{le=\"1\"} 3"));
        assert!(prom.contains("test_hist_bucket{le=\"+Inf\"} 4"));
        assert!(prom.contains("test_hist_count 4"));
    }

    #[test]
    fn test_metrics_registry_record_command() {
        let reg = MetricsRegistry::new();
        reg.record_command(true, std::time::Duration::from_millis(50));
        reg.record_command(false, std::time::Duration::from_millis(100));

        assert_eq!(reg.commands_success.value(), 1);
        assert_eq!(reg.commands_failed.value(), 1);
    }

    #[test]
    fn test_metrics_registry_record_violation() {
        let reg = MetricsRegistry::new();
        reg.record_violation("voltage");
        reg.record_violation("voltage");
        reg.record_violation("thermal");
        reg.record_violation("frequency");

        assert_eq!(reg.constraint_violations_voltage.value(), 2);
        assert_eq!(reg.constraint_violations_thermal.value(), 1);
        assert_eq!(reg.constraint_violations_frequency.value(), 1);
    }

    #[test]
    fn test_metrics_registry_record_agent_decision() {
        let reg = MetricsRegistry::new();
        reg.record_agent_decision("voltage-agent", "success");
        reg.record_agent_decision("voltage-agent", "success");
        reg.record_agent_decision("voltage-agent", "failed");
        reg.record_agent_decision("load-agent", "rejected");

        let decisions = reg.agent_decisions.read();
        let va = decisions.get("voltage-agent").unwrap();
        assert_eq!(va.success, 2);
        assert_eq!(va.failed, 1);
        let la = decisions.get("load-agent").unwrap();
        assert_eq!(la.rejected, 1);
    }

    #[test]
    fn test_metrics_registry_record_pipeline_stage() {
        let reg = MetricsRegistry::new();
        reg.record_pipeline_stage("precondition", std::time::Duration::from_micros(500));
        reg.record_pipeline_stage("execution", std::time::Duration::from_millis(5));
        reg.record_pipeline_stage("precondition", std::time::Duration::from_micros(300));

        let stages = reg.pipeline_stage_duration.read();
        assert!(stages.contains_key("precondition"));
        assert!(stages.contains_key("execution"));
    }

    #[test]
    fn test_metrics_registry_to_prometheus() {
        let reg = MetricsRegistry::new();
        reg.record_command(true, std::time::Duration::from_millis(50));
        reg.record_violation("voltage");
        reg.record_agent_decision("test-agent", "success");
        reg.device_connections_connected.set(5.0);

        let prom = reg.to_prometheus();

        // Verify key metrics are present
        assert!(prom.contains("eneros_commands_total{result=\"success\"} 1"));
        assert!(prom.contains("eneros_constraint_violations_total{type=\"voltage\"} 1"));
        assert!(prom.contains("eneros_agent_decisions_total{agent=\"test-agent\",result=\"success\"} 1"));
        assert!(prom.contains("eneros_device_connections 5"));
        assert!(prom.contains("eneros_command_duration_seconds"));
        assert!(prom.contains("eneros_http_requests_total"));
    }

    #[test]
    fn test_metrics_registry_record_http_request() {
        let reg = MetricsRegistry::new();
        reg.record_http_request(std::time::Duration::from_millis(15));
        reg.record_http_request(std::time::Duration::from_millis(25));

        assert_eq!(reg.http_requests_total.value(), 2);
    }

    #[test]
    fn test_histogram_observe_duration() {
        let h = Histogram::new("test_dur", "test", vec![0.001, 0.01, 0.1]);
        let start = Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(1));
        h.observe_duration(start);

        let prom = h.to_prometheus();
        assert!(prom.contains("test_dur_count 1"));
    }
}
