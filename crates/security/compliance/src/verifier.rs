//! Phase 3 出口验证器.
//!
//! 综合信创认证与等保 2.0 三级检查结果，生成 Phase 3 出口验证报告。

use crate::dengbao::DengbaoChecker;
use crate::types::{Phase3ExitReport, VerificationReport};

#[cfg(test)]
use crate::types::CheckStatus;
use crate::xinchuang::XinchuangChecker;

/// 合规验证 trait.
///
/// 定义 Phase 3 出口标准验证的核心接口。
pub trait ComplianceVerifier {
    /// 验证信创认证.
    fn verify_xinchuang(&self) -> VerificationReport;

    /// 验证等保 2.0 三级.
    fn verify_dengbao_level3(&self) -> VerificationReport;

    /// Phase 3 出口综合检查.
    fn phase3_exit_check(&self) -> Phase3ExitReport;
}

/// Phase 3 出口验证结构体.
#[derive(Clone, Copy, Debug, Default)]
pub struct Phase3ExitVerification {
    xinchuang: XinchuangChecker,
    dengbao: DengbaoChecker,
}

impl Phase3ExitVerification {
    /// 创建验证器实例.
    pub fn new() -> Self {
        Self {
            xinchuang: XinchuangChecker::new(),
            dengbao: DengbaoChecker::new(),
        }
    }
}

impl ComplianceVerifier for Phase3ExitVerification {
    fn verify_xinchuang(&self) -> VerificationReport {
        self.xinchuang.check_all()
    }

    fn verify_dengbao_level3(&self) -> VerificationReport {
        self.dengbao.check_level3()
    }

    fn phase3_exit_check(&self) -> Phase3ExitReport {
        let xc_report = self.verify_xinchuang();
        let db_report = self.verify_dengbao_level3();

        // 内核验证：自研内核项通过
        let kernel_ok = xc_report
            .items
            .iter()
            .any(|i| i.name == "自研内核" && i.status == crate::types::CheckStatus::Passed);

        // CPU 验证：国产 CPU 支持项通过
        let cpu_ok = xc_report
            .items
            .iter()
            .any(|i| i.name == "国产 CPU 支持" && i.status == crate::types::CheckStatus::Passed);

        // 信创整体验证
        let xinchuang_ok = xc_report.all_passed();

        // 等保整体验证
        let dengbao_ok = db_report.all_passed();

        Phase3ExitReport {
            kernel_ok,
            cpu_ok,
            xinchuang_ok,
            dengbao_ok,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_xinchuang() {
        let verifier = Phase3ExitVerification::new();
        let report = verifier.verify_xinchuang();
        // v0.212.0（P0-13）：无动态证据时标 Pending，不得硬编码 Passed。
        assert!(!report.all_passed(), "无证据不得 all_passed");
        assert_eq!(report.total, 7);
        assert!(report
            .items
            .iter()
            .any(|i| i.status == CheckStatus::Pending));
    }

    #[test]
    fn test_verify_dengbao() {
        let verifier = Phase3ExitVerification::new();
        let report = verifier.verify_dengbao_level3();
        assert!(report.all_passed());
        assert_eq!(report.total, 15);
    }

    #[test]
    fn test_phase3_exit_check_all_passed() {
        let verifier = Phase3ExitVerification::new();
        let report = verifier.phase3_exit_check();
        // v0.212.0（P0-13）：无证据时信创项不全过（防静态 Passed）。
        assert!(!report.kernel_ok, "无证据内核不得 ok");
        assert!(!report.cpu_ok, "无证据 CPU 不得 ok");
        assert!(!report.all_passed(), "无证据不得 all_passed");
    }

    #[test]
    fn test_phase3_exit_report_structure() {
        let verifier = Phase3ExitVerification::new();
        let report = verifier.phase3_exit_check();
        // v0.212.0（P0-13）：无证据时信创项 false（dengbao 保持其状态）。
        assert_eq!(
            report,
            Phase3ExitReport {
                kernel_ok: false,
                cpu_ok: false,
                xinchuang_ok: false,
                dengbao_ok: true,
            }
        );
    }

    #[test]
    fn test_trait_object_usage() {
        let verifier: &dyn ComplianceVerifier = &Phase3ExitVerification::new();
        let xc = verifier.verify_xinchuang();
        let db = verifier.verify_dengbao_level3();
        // v0.212.0（P0-13）：无证据信创不全过。
        assert!(!xc.all_passed(), "无证据信创不得全过");
        assert_eq!(db.total, 15);
    }
}
