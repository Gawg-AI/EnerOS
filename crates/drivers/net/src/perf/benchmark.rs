//! Network performance benchmarks (v0.30.0).
//!
//! Provides throughput and latency benchmark framework for the firewall and
//! connection-tracking subsystems. Because `no_std` environments lack
//! `std::time::Instant`, latency is currently a placeholder constant; real
//! measurements require wiring up `HalClock::now_ns()` on hardware.

use alloc::string::String;
use alloc::vec::Vec;

use crate::security::firewall::{Firewall, FirewallAction, FirewallPolicy, FirewallRule};
use crate::security::rate_limit::ConnectionTracker;
use crate::tcpip::addr::{ipv4_addr, ipv4_cidr};

/// Placeholder latency per operation in microseconds.
///
/// `no_std` builds cannot use `std::time::Instant`. Real benchmarks should
/// read `HalClock::now_ns()` around the loop and divide by the iteration count
/// to obtain an accurate per-operation latency.
const LATENCY_US: u32 = 1;

/// Assumed packet size in bytes for throughput calculations.
const PACKET_SIZE_BYTES: u32 = 64;

/// Benchmark result for a single test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BenchmarkResult {
    /// Estimated throughput in kbit/s.
    pub throughput_kbps: u32,
    /// Per-operation latency in microseconds (placeholder).
    pub latency_us: u32,
    /// Packets processed per second.
    pub packets_per_sec: u32,
}

impl BenchmarkResult {
    /// Create a new benchmark result from explicit metric values.
    pub fn new(throughput_kbps: u32, latency_us: u32, packets_per_sec: u32) -> Self {
        Self {
            throughput_kbps,
            latency_us,
            packets_per_sec,
        }
    }

    /// Derive a result from a placeholder latency value.
    ///
    /// `packets_per_sec = 1_000_000 / latency_us` and
    /// `throughput_kbps = packets_per_sec * PACKET_SIZE_BYTES / 1024`.
    fn from_latency(latency_us: u32) -> Self {
        let packets_per_sec = 1_000_000u32.checked_div(latency_us).unwrap_or(0);
        let throughput_kbps = packets_per_sec.saturating_mul(PACKET_SIZE_BYTES) / 1024;
        Self {
            throughput_kbps,
            latency_us,
            packets_per_sec,
        }
    }
}

/// Benchmark suite for network performance testing.
///
/// Accumulates named [`BenchmarkResult`] entries produced by the various
/// `run_*` methods. Results can be inspected with [`BenchmarkSuite::results`]
/// and reset with [`BenchmarkSuite::clear`].
#[derive(Default)]
pub struct BenchmarkSuite {
    results: Vec<(String, BenchmarkResult)>,
}

impl BenchmarkSuite {
    /// Create an empty benchmark suite.
    pub fn new() -> Self {
        Self::default()
    }

    /// Run a firewall rule-matching benchmark.
    ///
    /// Creates a `Firewall` with `AllowAll` default policy, an Allow rule for
    /// `192.168.1.0/24`, and a Drop rule for `10.0.0.0/8`, then calls
    /// [`Firewall::check_connection`] for `192.168.1.100` `iterations` times.
    /// The first rule matches on every iteration, so the connection tracker is
    /// never consulted — this isolates the rule-scan cost.
    pub fn run_firewall_benchmark(&mut self, iterations: u32) -> BenchmarkResult {
        let tracker = ConnectionTracker::new(100, 1000);
        let mut fw = Firewall::new(FirewallPolicy::AllowAll, tracker);
        fw.add_rule(FirewallRule {
            action: FirewallAction::Allow,
            src_ip: Some(ipv4_cidr(ipv4_addr(192, 168, 1, 0), 24)),
            dst_port: None,
            protocol: None,
        });
        fw.add_rule(FirewallRule {
            action: FirewallAction::Drop,
            src_ip: Some(ipv4_cidr(ipv4_addr(10, 0, 0, 0), 8)),
            dst_port: None,
            protocol: None,
        });

        let target = ipv4_addr(192, 168, 1, 100);
        for _ in 0..iterations {
            // black_box prevents the compiler from eliding the call.
            core::hint::black_box(fw.check_connection(target, 0));
        }

        let result = BenchmarkResult::from_latency(LATENCY_US);
        self.results.push((String::from("firewall"), result));
        result
    }

    /// Run a connection-tracker benchmark.
    ///
    /// Creates a `ConnectionTracker` with `max_per_ip=100` and
    /// `max_total=10000`, then calls [`ConnectionTracker::try_connect`] for
    /// `iterations` distinct source IPs (cycling through `10.0.x.y`). Once
    /// `max_total` is reached further attempts are rejected, but the loop still
    /// runs to measure per-call cost.
    pub fn run_connection_benchmark(&mut self, iterations: u32) -> BenchmarkResult {
        let mut tracker = ConnectionTracker::new(100, 10000);
        for i in 0..iterations {
            let ip = ipv4_addr(10, 0, (i / 256) as u8, (i % 256) as u8);
            core::hint::black_box(tracker.try_connect(ip, i as u64));
        }

        let result = BenchmarkResult::from_latency(LATENCY_US);
        self.results.push((String::from("connection"), result));
        result
    }

    /// Return all accumulated benchmark results as a slice of `(name, result)`
    /// pairs, in insertion order.
    pub fn results(&self) -> &[(String, BenchmarkResult)] {
        &self.results
    }

    /// Clear all accumulated results.
    pub fn clear(&mut self) {
        self.results.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_benchmark_result_new() {
        let r = BenchmarkResult::new(100, 5, 200);
        assert_eq!(r.throughput_kbps, 100);
        assert_eq!(r.latency_us, 5);
        assert_eq!(r.packets_per_sec, 200);
    }

    #[test]
    fn test_benchmark_suite_new_empty() {
        let suite = BenchmarkSuite::new();
        assert!(suite.results().is_empty());
    }

    #[test]
    fn test_firewall_benchmark_returns_valid_result() {
        let mut suite = BenchmarkSuite::new();
        let r = suite.run_firewall_benchmark(100);
        // Placeholder latency drives the derived metrics.
        assert_eq!(r.latency_us, 1);
        assert_eq!(r.packets_per_sec, 1_000_000);
        assert_eq!(r.throughput_kbps, 62_500);
    }

    #[test]
    fn test_firewall_benchmark_pushes_result() {
        let mut suite = BenchmarkSuite::new();
        suite.run_firewall_benchmark(50);
        assert_eq!(suite.results().len(), 1);
        assert_eq!(suite.results()[0].0, "firewall");
    }

    #[test]
    fn test_connection_benchmark_returns_valid_result() {
        let mut suite = BenchmarkSuite::new();
        let r = suite.run_connection_benchmark(100);
        assert_eq!(r.latency_us, 1);
        assert_eq!(r.packets_per_sec, 1_000_000);
        assert_eq!(r.throughput_kbps, 62_500);
    }

    #[test]
    fn test_connection_benchmark_pushes_result() {
        let mut suite = BenchmarkSuite::new();
        suite.run_connection_benchmark(50);
        assert_eq!(suite.results().len(), 1);
        assert_eq!(suite.results()[0].0, "connection");
    }

    #[test]
    fn test_results_query() {
        let mut suite = BenchmarkSuite::new();
        suite.run_firewall_benchmark(10);
        suite.run_connection_benchmark(10);
        let results = suite.results();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "firewall");
        assert_eq!(results[1].0, "connection");
    }

    #[test]
    fn test_clear_empties_results() {
        let mut suite = BenchmarkSuite::new();
        suite.run_firewall_benchmark(10);
        suite.run_connection_benchmark(10);
        assert_eq!(suite.results().len(), 2);
        suite.clear();
        assert!(suite.results().is_empty());
    }

    #[test]
    fn test_multiple_runs_accumulate() {
        let mut suite = BenchmarkSuite::new();
        suite.run_firewall_benchmark(10);
        suite.run_connection_benchmark(10);
        suite.run_firewall_benchmark(20);
        suite.run_connection_benchmark(20);
        assert_eq!(suite.results().len(), 4);
        assert_eq!(suite.results()[0].0, "firewall");
        assert_eq!(suite.results()[1].0, "connection");
        assert_eq!(suite.results()[2].0, "firewall");
        assert_eq!(suite.results()[3].0, "connection");
    }
}
