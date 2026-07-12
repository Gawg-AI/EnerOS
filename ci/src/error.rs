//! EnerOS Quality Gate — Error Types
//!
//! Defines the error enum raised by the four quality gate checks
//! (fmt / clippy / deny / test).

use std::fmt;

/// Errors raised by quality gate checks.
#[derive(Debug)]
pub enum GateError {
    /// `cargo fmt --check` detected unformatted code.
    FmtDirty,
    /// `cargo clippy -D warnings` emitted warnings.
    ClippyWarning(String),
    /// `cargo deny` found advisories, license, ban, or source issues.
    VulnFound(String),
    /// `cargo test` reported one or more failing tests.
    TestFailed,
    /// A command could not be executed (e.g. `cargo` not in PATH).
    IoError(String),
}

impl fmt::Display for GateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GateError::FmtDirty => {
                write!(f, "FmtDirty: code is not formatted (run `cargo fmt --all`)")
            }
            GateError::ClippyWarning(msg) => write!(f, "ClippyWarning: {}", msg),
            GateError::VulnFound(msg) => write!(f, "VulnFound: {}", msg),
            GateError::TestFailed => write!(f, "TestFailed: one or more unit tests failed"),
            GateError::IoError(msg) => write!(f, "IoError: {}", msg),
        }
    }
}

impl std::error::Error for GateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}
