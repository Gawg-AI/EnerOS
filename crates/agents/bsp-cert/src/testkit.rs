//! BSP 测试套件

#[cfg(any(test, feature = "mock"))]
use alloc::string::String;
#[cfg(any(test, feature = "mock"))]
use alloc::vec::Vec;

use crate::types::{BspSpec, BspTestReport};
#[cfg(any(test, feature = "mock"))]
use crate::types::{Outcome, TestCaseResult};

/// BSP 测试套件 trait
pub trait BspTestKit {
    /// 根据认证规范执行全部测试用例，返回测试报告
    fn run(&mut self, spec: &BspSpec) -> BspTestReport;
}

/// 模拟测试套件
///
/// - `pass_all = true`: 所有用例通过
/// - `pass_all = false`: 第一个强制项失败，其余通过
#[derive(Debug, Clone)]
#[cfg(any(test, feature = "mock"))]
pub struct MockTestKit {
    /// 是否全部通过
    pub pass_all: bool,
}

#[cfg(any(test, feature = "mock"))]
impl MockTestKit {
    /// 创建模拟测试套件
    pub fn new(pass_all: bool) -> Self {
        Self { pass_all }
    }
}

#[cfg(any(test, feature = "mock"))]
impl BspTestKit for MockTestKit {
    fn run(&mut self, spec: &BspSpec) -> BspTestReport {
        let mut results: Vec<TestCaseResult> = Vec::new();
        let mut passed: u32 = 0;
        let mut failed: u32 = 0;
        let skipped: u32 = 0;
        let mut first_mandatory_failed = false;

        for item in &spec.items {
            let outcome = if !self.pass_all && item.mandatory && !first_mandatory_failed {
                first_mandatory_failed = true;
                failed += 1;
                Outcome::Fail(String::from("mandatory check failed"))
            } else {
                passed += 1;
                Outcome::Pass
            };
            results.push(TestCaseResult {
                case_id: item.id.clone(),
                outcome,
                duration_ms: 10,
            });
        }

        let total = results.len() as u32;
        BspTestReport {
            total,
            passed,
            failed,
            skipped,
            results,
        }
    }
}

/// 执行测试套件并返回报告
pub fn run_tests(kit: &mut dyn BspTestKit, spec: &BspSpec) -> BspTestReport {
    kit.run(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::default_spec;

    #[test]
    fn test_all_pass() {
        let spec = default_spec();
        let mut kit = MockTestKit::new(true);
        let report = run_tests(&mut kit, &spec);
        assert_eq!(report.total, 9);
        assert_eq!(report.passed, 9);
        assert_eq!(report.failed, 0);
        assert_eq!(report.skipped, 0);
    }

    #[test]
    fn test_one_fail() {
        let spec = default_spec();
        let mut kit = MockTestKit::new(false);
        let report = run_tests(&mut kit, &spec);
        assert_eq!(report.total, 9);
        assert_eq!(report.failed, 1);
        assert_eq!(report.passed, 8);
    }

    #[test]
    fn test_report_counts_correct() {
        let spec = default_spec();
        let mut kit = MockTestKit::new(true);
        let report = run_tests(&mut kit, &spec);
        assert_eq!(report.passed + report.failed + report.skipped, report.total);
        assert_eq!(report.results.len() as u32, report.total);
    }

    #[test]
    fn test_first_mandatory_fails() {
        let spec = default_spec();
        let mut kit = MockTestKit::new(false);
        let report = kit.run(&spec);
        // 第一个用例应为 Fail
        let first = &report.results[0];
        assert!(matches!(first.outcome, Outcome::Fail(_)));
        // 其余应为 Pass
        for r in &report.results[1..] {
            assert_eq!(r.outcome, Outcome::Pass);
        }
    }

    #[test]
    fn test_empty_spec() {
        let spec = BspSpec {
            version: String::from("0.0.1"),
            items: Vec::new(),
        };
        let mut kit = MockTestKit::new(true);
        let report = run_tests(&mut kit, &spec);
        assert_eq!(report.total, 0);
        assert_eq!(report.passed, 0);
    }
}
