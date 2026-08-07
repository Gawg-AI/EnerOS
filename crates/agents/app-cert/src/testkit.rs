//! 测试套件

#[cfg(any(test, feature = "mock"))]
use alloc::string::String;
#[cfg(any(test, feature = "mock"))]
use alloc::vec::Vec;

#[cfg(any(test, feature = "mock"))]
use crate::types::{AppCertCategory, Outcome, TestCaseResult};
use crate::types::{AppCertReport, AppCertSpec, AppPackage};

/// 应用测试套件 trait
pub trait AppTestKit {
    /// 对应用包执行认证测试，返回测试报告
    fn run(&self, app: &AppPackage, spec: &AppCertSpec) -> AppCertReport;
}

/// Mock 测试套件实现
///
/// 规则：若 `app.signature_valid` 为 true，则所有项通过；
/// 否则 Security 类别项失败，其余通过。
#[cfg(any(test, feature = "mock"))]
pub struct MockAppTestKit;

#[cfg(any(test, feature = "mock"))]
impl AppTestKit for MockAppTestKit {
    fn run(&self, app: &AppPackage, spec: &AppCertSpec) -> AppCertReport {
        let mut results: Vec<TestCaseResult> = Vec::new();
        let mut passed: u32 = 0;
        let mut failed: u32 = 0;

        for item in &spec.items {
            let outcome = if !app.signature_valid && item.category == AppCertCategory::Security {
                failed += 1;
                Outcome::Fail(String::from("signature invalid"))
            } else {
                passed += 1;
                Outcome::Pass
            };

            results.push(TestCaseResult {
                case_id: item.id.clone(),
                outcome,
            });
        }

        let total = passed + failed;
        AppCertReport {
            total,
            passed,
            failed,
            results,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::default_app_spec;

    fn make_app(signature_valid: bool) -> AppPackage {
        AppPackage {
            app_id: 1,
            name: String::from("test-app"),
            version: String::from("0.1.0"),
            signature_valid,
            binary_hash: [0xAB; 32],
            signer_pubkey: None,
            signature: None,
        }
    }

    #[test]
    fn test_valid_app_passes_all() {
        let kit = MockAppTestKit;
        let app = make_app(true);
        let spec = default_app_spec();
        let report = kit.run(&app, &spec);

        assert_eq!(report.total, 5);
        assert_eq!(report.passed, 5);
        assert_eq!(report.failed, 0);
        for r in &report.results {
            assert_eq!(r.outcome, Outcome::Pass);
        }
    }

    #[test]
    fn test_invalid_signature_fails_security() {
        let kit = MockAppTestKit;
        let app = make_app(false);
        let spec = default_app_spec();
        let report = kit.run(&app, &spec);

        assert_eq!(report.total, 5);
        assert_eq!(report.passed, 4);
        assert_eq!(report.failed, 1);

        let security_result = report
            .results
            .iter()
            .find(|r| r.case_id == "security-001")
            .expect("security item should exist");
        assert_eq!(
            security_result.outcome,
            Outcome::Fail(String::from("signature invalid"))
        );
    }

    #[test]
    fn test_non_security_items_pass_with_invalid_sig() {
        let kit = MockAppTestKit;
        let app = make_app(false);
        let spec = default_app_spec();
        let report = kit.run(&app, &spec);

        for r in &report.results {
            if r.case_id != "security-001" {
                assert_eq!(r.outcome, Outcome::Pass);
            }
        }
    }
}
