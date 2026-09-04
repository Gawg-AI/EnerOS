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
    /// Go/No-Go 文档与 workspace 版本不一致（蓝图/20 R1-1）。
    VersionGateInconsistent(String),
    /// crates/agents + crates/ai 存在无 cfg 保护的 pub struct Mock（蓝图/20 R2-1）。
    MockGateViolation(String),
    /// `config compile` 调用块缺显式签名钥来源（蓝图 66 WP-66.1.1：
    /// 须带 `--sign-key-hex` 或 `--accept-test-signing-key`）。
    SignKeyDisciplineViolation(String),
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
            GateError::VersionGateInconsistent(msg) => {
                write!(f, "VersionGateInconsistent: {}", msg)
            }
            GateError::MockGateViolation(msg) => write!(f, "MockGateViolation: {}", msg),
            GateError::SignKeyDisciplineViolation(msg) => {
                write!(f, "SignKeyDisciplineViolation: {}", msg)
            }
            GateError::IoError(msg) => write!(f, "IoError: {}", msg),
        }
    }
}

impl std::error::Error for GateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}
