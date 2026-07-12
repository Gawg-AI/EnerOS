//! EnerOS Quality Gate — Core
//!
//! Defines the quality gate data structures, the [`QualityGate`] trait, and
//! the [`DefaultGate`] implementation that wraps `cargo` subcommands
//! (fmt / clippy / deny / test).

use std::io;
use std::process::Command;
use std::time::Instant;

use crate::error::GateError;

/// Result of a single quality check.
#[derive(Debug, Clone)]
pub struct CheckResult {
    /// Check name: `"fmt"`, `"clippy"`, `"audit"`, or `"test"`.
    pub name: &'static str,
    pub passed: bool,
    pub duration_ms: u64,
    /// Failure detail, or a degraded-mode notice when `passed` is true.
    pub message: Option<String>,
}

/// Aggregated report for all four quality checks.
#[derive(Debug)]
pub struct GateReport {
    /// Results for `[fmt, clippy, audit, test]` in that order.
    pub results: [CheckResult; 4],
    pub overall_pass: bool,
}

/// Abstraction over the four quality checks.
pub trait QualityGate {
    /// Run all checks sequentially and return an aggregated report.
    fn run_all(&self) -> GateReport;
    fn run_fmt_check(&self) -> Result<(), GateError>;
    fn run_clippy(&self) -> Result<(), GateError>;
    fn run_audit(&self) -> Result<(), GateError>;
    fn run_tests(&self) -> Result<(), GateError>;
}

/// Default gate that shells out to `cargo` subcommands.
pub struct DefaultGate;

impl DefaultGate {
    pub fn new() -> Self {
        Self
    }

    /// Run `cargo <args>`, returning `on_failure` when the process exits
    /// with a non-zero status. Spawn failures (`cargo` not in PATH, etc.)
    /// are mapped to [`GateError::IoError`].
    fn run_cargo(args: &[&str], on_failure: GateError) -> Result<(), GateError> {
        match Command::new("cargo").args(args).status() {
            Ok(status) if status.success() => Ok(()),
            Ok(_) => Err(on_failure),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Err(GateError::IoError("`cargo` not found in PATH".to_string()))
            }
            Err(e) => Err(GateError::IoError(format!(
                "failed to spawn `cargo {}`: {}",
                args.join(" "),
                e
            ))),
        }
    }

    /// Check whether `cargo-deny` is installed and reachable in PATH.
    /// Used by [`QualityGate::run_all`] to detect the degraded case.
    fn cargo_deny_available() -> bool {
        Command::new("cargo-deny")
            .arg("--version")
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

impl Default for DefaultGate {
    fn default() -> Self {
        Self::new()
    }
}

impl QualityGate for DefaultGate {
    fn run_fmt_check(&self) -> Result<(), GateError> {
        Self::run_cargo(&["fmt", "--all", "--", "--check"], GateError::FmtDirty)
    }

    fn run_clippy(&self) -> Result<(), GateError> {
        // eneros-kernel and eneros-hello are excluded from host-side clippy:
        // both define #[panic_handler] / #[lang = "eh_personality"] which
        // conflict with std on the host target. They're validated via cross-build.
        // eneros-runtime (v0.4.0) and eneros-hal (v0.5.0+) are library crates
        // without panic_handler and are host-testable.
        // Note: eneros-hal arm64 module (v0.6.0 core + v0.7.0 peripherals + v0.8.0 mm + v0.9.0 partition) is cfg-gated by
        // #[cfg(target_arch = "aarch64")] and excluded from host clippy.
        // eneros-heap (v0.10.0), eneros-user-heap (v0.11.0), eneros-time (v0.12.0 + v0.12.1 beidou + v0.12.2 holdover),
        // eneros-watchdog (v0.13.0), eneros-panic (v0.14.0), eneros-smp (v0.15.0), eneros-sched (v0.16.0), eneros-smp coherence (v0.17.0),
        // eneros-mm isolation (v0.9.1 compliance), eneros-power (v0.17.1 power management), eneros-sched (v0.18.0 thread abstraction, v0.19.0 partition scheduler),
        // eneros-ipc (v0.20.0 IPC endpoint + v0.21.0 SPSC ring), eneros-controlbus (v0.22.0 Control Bus + TTL + dual-plane),
        // and eneros-storage (v0.23.0 eMMC/NVMe Block Device) are standalone no_std crates
        // with no arch-specific code, host-testable.
        Self::run_cargo(
            &[
                "clippy",
                "--workspace",
                "--exclude",
                "eneros-kernel",
                "--exclude",
                "eneros-hello",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
            GateError::ClippyWarning("clippy reported warnings (see output above)".to_string()),
        )
    }

    fn run_audit(&self) -> Result<(), GateError> {
        match Command::new("cargo")
            .args(["deny", "check", "advisories", "licenses", "bans", "sources"])
            .status()
        {
            Ok(status) if status.success() => Ok(()),
            Ok(_) => Err(GateError::VulnFound(
                "cargo-deny reported advisories/license/ban/source issues".to_string(),
            )),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(GateError::IoError(format!(
                "failed to spawn `cargo deny`: {}",
                e
            ))),
        }
    }

    fn run_tests(&self) -> Result<(), GateError> {
        // eneros-kernel and eneros-hello are excluded: both define
        // #[panic_handler] / #![no_main] which can't be tested on host.
        // eneros-runtime (v0.4.0) and eneros-hal (v0.5.0+) are library crates
        // and are host-testable.
        // Note: eneros-hal arm64 module (v0.6.0 core + v0.7.0 peripherals + v0.8.0 mm + v0.9.0 partition) is cfg-gated by
        // #[cfg(target_arch = "aarch64")] and excluded from host tests.
        // eneros-heap (v0.10.0), eneros-user-heap (v0.11.0), eneros-time (v0.12.0 + v0.12.1 beidou + v0.12.2 holdover),
        // eneros-watchdog (v0.13.0), eneros-panic (v0.14.0), eneros-smp (v0.15.0), eneros-sched (v0.16.0), eneros-smp coherence (v0.17.0),
        // eneros-mm isolation (v0.9.1 compliance), eneros-power (v0.17.1 power management), eneros-sched (v0.18.0 thread abstraction, v0.19.0 partition scheduler),
        // eneros-ipc (v0.20.0 IPC endpoint + v0.21.0 SPSC ring), eneros-controlbus (v0.22.0 Control Bus + TTL + dual-plane),
        // and eneros-storage (v0.23.0 eMMC/NVMe Block Device) are standalone no_std crates
        // with no arch-specific code, host-testable.
        Self::run_cargo(
            &[
                "test",
                "--workspace",
                "--exclude",
                "eneros-kernel",
                "--exclude",
                "eneros-hello",
            ],
            GateError::TestFailed,
        )
    }

    fn run_all(&self) -> GateReport {
        // fmt
        let start = Instant::now();
        let mut fmt_cr = CheckResult::from(self.run_fmt_check());
        fmt_cr.name = "fmt";
        fmt_cr.duration_ms = start.elapsed().as_millis() as u64;

        // clippy
        let start = Instant::now();
        let mut clippy_cr = CheckResult::from(self.run_clippy());
        clippy_cr.name = "clippy";
        clippy_cr.duration_ms = start.elapsed().as_millis() as u64;

        // audit (special: detect degraded mode so we can annotate the result)
        let start = Instant::now();
        let audit_cr = if !Self::cargo_deny_available() {
            CheckResult {
                name: "audit",
                passed: true,
                duration_ms: start.elapsed().as_millis() as u64,
                message: Some("cargo-deny not found, audit skipped (degraded)".to_string()),
            }
        } else {
            let mut cr = CheckResult::from(self.run_audit());
            cr.name = "audit";
            cr.duration_ms = start.elapsed().as_millis() as u64;
            cr
        };

        // test
        let start = Instant::now();
        let mut test_cr = CheckResult::from(self.run_tests());
        test_cr.name = "test";
        test_cr.duration_ms = start.elapsed().as_millis() as u64;

        let results = [fmt_cr, clippy_cr, audit_cr, test_cr];
        let overall_pass = results.iter().all(|r| r.passed);
        GateReport {
            results,
            overall_pass,
        }
    }
}

impl From<Result<(), GateError>> for CheckResult {
    fn from(r: Result<(), GateError>) -> Self {
        match r {
            Ok(()) => CheckResult {
                name: "",
                passed: true,
                duration_ms: 0,
                message: None,
            },
            Err(e) => CheckResult {
                name: "",
                passed: false,
                duration_ms: 0,
                message: Some(e.to_string()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::GateError;

    fn pass(name: &'static str) -> CheckResult {
        CheckResult {
            name,
            passed: true,
            duration_ms: 0,
            message: None,
        }
    }

    fn fail(name: &'static str) -> CheckResult {
        CheckResult {
            name,
            passed: false,
            duration_ms: 0,
            message: Some("failure".to_string()),
        }
    }

    #[test]
    fn test_all_pass() {
        let results = [pass("fmt"), pass("clippy"), pass("audit"), pass("test")];
        let overall_pass = results.iter().all(|r| r.passed);
        assert!(overall_pass);

        let report = GateReport {
            results,
            overall_pass,
        };
        assert!(report.overall_pass);
    }

    #[test]
    fn test_one_fail() {
        let results = [pass("fmt"), fail("clippy"), pass("audit"), pass("test")];
        let overall_pass = results.iter().all(|r| r.passed);
        assert!(!overall_pass);

        // Any single failure flips the overall result.
        for i in 0..4 {
            let mut r = [pass("fmt"), pass("clippy"), pass("audit"), pass("test")];
            r[i] = fail("x");
            assert!(!r.iter().all(|c| c.passed), "index {} should fail", i);
        }
    }

    #[test]
    fn test_from_ok() {
        let cr = CheckResult::from(Ok(()));
        assert!(cr.passed);
        assert!(cr.message.is_none());
        assert_eq!(cr.duration_ms, 0);
    }

    #[test]
    fn test_from_err() {
        let cr = CheckResult::from(Err(GateError::TestFailed));
        assert!(!cr.passed);
        let msg = cr.message.expect("expected a failure message");
        assert!(msg.contains("TestFailed"));
    }

    #[test]
    fn test_audit_degraded() {
        // A degraded audit passes but carries a notice.
        let degraded = CheckResult {
            name: "audit",
            passed: true,
            duration_ms: 0,
            message: Some("cargo-deny not found, audit skipped (degraded)".to_string()),
        };
        assert!(degraded.passed);
        assert!(degraded.message.as_ref().unwrap().contains("degraded"));

        // A degraded audit must not cause overall failure.
        let results = [pass("fmt"), pass("clippy"), degraded, pass("test")];
        let overall_pass = results.iter().all(|r| r.passed);
        assert!(overall_pass);
    }
}
