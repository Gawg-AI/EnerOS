//! 工具链锁定与可复现性 (REL-001~003).
//!
//! REL-001: pinned toolchain manifest — digest 可验证
//! REL-002: hermetic/offline build env — clean host 可重建
//! REL-003: SOURCE_DATE_EPOCH 清理 — 无当前时间/路径漂移

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

/// 工具链清单。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainManifest {
    /// Rust 工具链版本（如 "nightly-2026-04-04"）。
    pub rust_version: String,
    /// rust-toolchain.toml 的 SHA-256。
    pub toolchain_toml_sha256: String,
    /// Cargo.lock 的 SHA-256。
    pub cargo_lock_sha256: String,
    /// 构建目标三元组。
    pub target: String,
    /// 是否启用 offline 构建。
    pub offline_build: bool,
    /// SOURCE_DATE_EPOCH 值（Unix 秒）。
    pub source_date_epoch: u64,
}

impl ToolchainManifest {
    /// 校验清单完整性。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.rust_version.is_empty() {
            return Err("rust_version is empty");
        }
        if self.toolchain_toml_sha256.len() != 64 {
            return Err("toolchain_toml_sha256 must be 64 hex chars");
        }
        if self.cargo_lock_sha256.len() != 64 {
            return Err("cargo_lock_sha256 must be 64 hex chars");
        }
        if self.target.is_empty() {
            return Err("target is empty");
        }
        if self.source_date_epoch == 0 {
            return Err("source_date_epoch must be non-zero (no implicit current time)");
        }
        Ok(())
    }
}

/// 可复现性检查项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReproducibilityCheck {
    /// 工具链版本固定。
    ToolchainPinned,
    /// Cargo.lock 入仓且匹配。
    CargoLockCommitted,
    /// 离线构建可用。
    OfflineBuildOk,
    /// SOURCE_DATE_EPOCH 已设置（无当前时间漂移）。
    SourceDateEpochSet,
    /// 无路径依赖漂移（无绝对路径嵌入产物）。
    NoAbsolutePathLeak,
    /// 构建产物 digest 可复现（双 builder 一致）。
    DigestReproducible,
}

impl ReproducibilityCheck {
    pub fn label(self) -> &'static str {
        match self {
            ReproducibilityCheck::ToolchainPinned => "TOOLCHAIN_PINNED",
            ReproducibilityCheck::CargoLockCommitted => "CARGO_LOCK_COMMITTED",
            ReproducibilityCheck::OfflineBuildOk => "OFFLINE_BUILD_OK",
            ReproducibilityCheck::SourceDateEpochSet => "SOURCE_DATE_EPOCH_SET",
            ReproducibilityCheck::NoAbsolutePathLeak => "NO_ABSOLUTE_PATH_LEAK",
            ReproducibilityCheck::DigestReproducible => "DIGEST_REPRODUCIBLE",
        }
    }

    pub fn all() -> &'static [ReproducibilityCheck] {
        &[
            ReproducibilityCheck::ToolchainPinned,
            ReproducibilityCheck::CargoLockCommitted,
            ReproducibilityCheck::OfflineBuildOk,
            ReproducibilityCheck::SourceDateEpochSet,
            ReproducibilityCheck::NoAbsolutePathLeak,
            ReproducibilityCheck::DigestReproducible,
        ]
    }
}

/// 可复现性检查结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReproResult {
    pub check: ReproducibilityCheck,
    pub passed: bool,
    pub evidence: String,
}

impl ReproResult {
    pub fn pass(check: ReproducibilityCheck, evidence: &str) -> Self {
        Self { check, passed: true, evidence: String::from(evidence) }
    }
    pub fn fail(check: ReproducibilityCheck, evidence: &str) -> Self {
        Self { check, passed: false, evidence: String::from(evidence) }
    }
}

/// 执行可复现性审计。
///
/// `results` 为调用方提供的各项检查结果。
pub fn audit_reproducibility(results: &[ReproResult]) -> bool {
    if results.len() < ReproducibilityCheck::all().len() {
        return false;
    }
    results.iter().all(|r| r.passed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_manifest() -> ToolchainManifest {
        ToolchainManifest {
            rust_version: String::from("nightly-2026-04-04"),
            toolchain_toml_sha256: String::from("a1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890"),
            cargo_lock_sha256: String::from("b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890a1"),
            target: String::from("aarch64-unknown-none"),
            offline_build: true,
            source_date_epoch: 1717200000,
        }
    }

    #[test]
    fn manifest_validates() {
        assert!(valid_manifest().validate().is_ok());
    }

    #[test]
    fn empty_rust_version_rejected() {
        let mut m = valid_manifest();
        m.rust_version = String::new();
        assert!(m.validate().is_err());
    }

    #[test]
    fn short_sha256_rejected() {
        let mut m = valid_manifest();
        m.toolchain_toml_sha256 = String::from("short");
        assert!(m.validate().is_err());
    }

    #[test]
    fn zero_source_date_epoch_rejected() {
        let mut m = valid_manifest();
        m.source_date_epoch = 0;
        assert!(m.validate().is_err());
    }

    #[test]
    fn reproducibility_all_pass() {
        let results: Vec<ReproResult> = ReproducibilityCheck::all()
            .iter()
            .map(|c| ReproResult::pass(*c, "verified"))
            .collect();
        assert!(audit_reproducibility(&results));
    }

    #[test]
    fn reproducibility_one_fail_blocks() {
        let mut results: Vec<ReproResult> = ReproducibilityCheck::all()
            .iter()
            .map(|c| ReproResult::pass(*c, "verified"))
            .collect();
        results[3] = ReproResult::fail(ReproducibilityCheck::SourceDateEpochSet, "not set");
        assert!(!audit_reproducibility(&results));
    }

    #[test]
    fn incomplete_checks_fail() {
        let results = vec![ReproResult::pass(ReproducibilityCheck::ToolchainPinned, "ok")];
        assert!(!audit_reproducibility(&results));
    }
}
