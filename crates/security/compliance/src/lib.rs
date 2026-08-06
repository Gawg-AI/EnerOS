//! EnerOS 信创认证 + 等保 2.0 三级 (workspace v0.192.0).
//!
//! Phase 3 出口标准验证模块，提供信创认证检查、等保 2.0 三级检查
//! 以及综合出口验证能力。
//!
//! # 架构
//! ```text
//! ┌──────────────────────────────────────────────────────┐
//! │  eneros-compliance (workspace v0.192.0)                         │
//! │  ┌──────────┐  ┌──────────┐  ┌───────────────────┐  │
//! │  │ types    │  │xinchuang │  │ dengbao           │  │
//! │  │ (Check   │  │(7 items) │  │ (15 items)        │  │
//! │  │  Status, │  │          │  │                   │  │
//! │  │  Report) │  │          │  │                   │  │
//! │  └────┬─────┘  └────┬─────┘  └────────┬──────────┘  │
//! │       │              │                  │             │
//! │       │        ┌─────▼──────────────────▼──────┐    │
//! │       └───────►│ verifier (Phase3ExitVerification)│   │
//! │                │ ComplianceVerifier trait        │   │
//! │                └────────────────────────────────┘   │
//! └──────────────────────────────────────────────────────┘
//! ```
//!
//! # 使用示例
//! ```
//! use eneros_compliance::{Phase3ExitVerification, ComplianceVerifier};
//!
//! let verifier = Phase3ExitVerification::new();
//! let report = verifier.phase3_exit_check();
//! // v0.212.0（P0-13）：无动态证据时不全过（防静态 Passed）。
//! assert!(!report.all_passed());
//! ```
//!
//! # no_std 合规
//! 所有代码 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 使用 `alloc::vec::Vec`、`alloc::string::String`，不使用 `std::*`。
//!
//! # 偏差声明
//! 1. **检查项静态化**：当前所有检查项为静态证据枚举，生产环境应对接运行时动态检测。
//! 2. **第三方审计**：等保 2.0 三级需公安部授权测评机构现场评估，本模块为内部预检。
//! 3. **信创目录**：信创认证需进入工信部信创产品目录，本模块验证技术准备度。
//! 4. **持续合规**：生产环境需定期复评（等保每年一次），本模块为一次性快照验证。
//! 5. **物理安全**：安全物理环境项依赖部署现场条件，代码层面仅做声明式验证。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod dengbao;
pub mod types;
pub mod verifier;
pub mod xinchuang;

// Re-exports
pub use dengbao::DengbaoChecker;
pub use types::{CheckItem, CheckStatus, Phase3ExitReport, VerificationReport};
pub use verifier::{ComplianceVerifier, Phase3ExitVerification};
pub use xinchuang::XinchuangChecker;

/// 合规检查错误类型.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComplianceError {
    /// 检查项未通过.
    CheckFailed,
    /// 报告不完整.
    ReportIncomplete,
    /// 认证待审批.
    CertificationPending,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compliance_error_variants() {
        let e1 = ComplianceError::CheckFailed;
        let e2 = ComplianceError::ReportIncomplete;
        let e3 = ComplianceError::CertificationPending;
        assert_ne!(e1, e2);
        assert_ne!(e2, e3);
        assert_ne!(e1, e3);
    }

    #[test]
    fn test_compliance_error_is_copy() {
        let err = ComplianceError::CheckFailed;
        let err2 = err;
        assert_eq!(err, err2);
    }

    #[test]
    fn test_full_verification_flow() {
        let verifier = Phase3ExitVerification::new();

        // 信创检查（v0.212.0 P0-13：无证据不全过）
        let xc = verifier.verify_xinchuang();
        assert!(!xc.all_passed(), "无证据信创不得全过");
        assert_eq!(xc.total, 7);

        // 等保检查
        let db = verifier.verify_dengbao_level3();
        assert!(db.all_passed());
        assert_eq!(db.total, 15);

        // 综合出口验证（v0.212.0 P0-13：无证据不全过）
        let exit = verifier.phase3_exit_check();
        assert!(!exit.all_passed(), "无证据出口不得全过");
    }

    #[test]
    fn test_reexports_accessible() {
        // 验证 re-export 的类型可正常访问
        let _status = CheckStatus::Passed;
        let _err = ComplianceError::CertificationPending;
        let checker = XinchuangChecker::new();
        let report = checker.check_all();
        // v0.212.0（P0-13）：标题改为证据驱动 + 无证据不全过。
        assert_eq!(
            report.title,
            alloc::string::String::from("信创认证检查（证据驱动）")
        );
        assert!(!report.all_passed(), "无证据不得全过");
    }
}
