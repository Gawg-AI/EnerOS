//! Factory test runner: test suites, items, reports, and the
//! [`FactoryTestRunner`] trait (D11: host-side std).
//!
//! 定义 [`TestCategory`] / [`TestItem`] / [`TestSuite`] / [`TestFailure`] /
//! [`TestReport`] 与 [`FactoryTestRunner`] trait，以及默认实现
//! [`DefaultTestRunner`]。当前 `run_item` 为骨架实现（恒通过），真实
//! 实现将通过协议适配器层调用被测设备 API。

use std::fmt;

/// 工厂测试项的粗粒度分类。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestCategory {
    /// 功能正确性（点表、配置 ...）。
    Functional,
    /// 通信链路（Modbus/IEC104/CAN 往返）。
    Communication,
    /// 性能 / 延迟 / 吞吐。
    Performance,
    /// 安全联锁 / 保护行为。
    Safety,
}

impl fmt::Display for TestCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TestCategory::Functional => write!(f, "Functional"),
            TestCategory::Communication => write!(f, "Communication"),
            TestCategory::Performance => write!(f, "Performance"),
            TestCategory::Safety => write!(f, "Safety"),
        }
    }
}

/// 单个工厂测试项。
#[derive(Debug, Clone)]
pub struct TestItem {
    pub name: String,
    pub category: TestCategory,
    pub passed: bool,
    pub failure_reason: Option<String>,
    pub duration_ms: u64,
}

/// 一组相关的测试项集合。
#[derive(Debug, Clone)]
pub struct TestSuite {
    pub name: String,
    pub items: Vec<TestItem>,
}

/// 测试报告中记录的单个失败项。
#[derive(Debug, Clone)]
pub struct TestFailure {
    pub test_name: String,
    pub reason: String,
    pub timestamp: u64,
}

/// 一次测试套件运行的聚合报告。
#[derive(Debug, Clone)]
pub struct TestReport {
    pub suite_name: String,
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
    pub duration_ms: u64,
    pub failures: Vec<TestFailure>,
}

impl TestReport {
    /// 返回单行可读摘要。
    pub fn summary(&self) -> String {
        format!(
            "TestReport[{}]: total={} passed={} failed={} ({}ms, {} failure(s))",
            self.suite_name,
            self.total,
            self.passed,
            self.failed,
            self.duration_ms,
            self.failures.len()
        )
    }
}

/// 工厂测试执行引擎抽象。
pub trait FactoryTestRunner {
    /// 运行 `suite` 中的全部测试项并返回聚合报告。
    fn run_suite(&mut self, suite: &TestSuite) -> TestReport;
    /// 运行单个测试项；返回结果字段已更新的副本。
    fn run_item(&mut self, item: &TestItem) -> TestItem;
}

/// 默认 runner，记录所有已执行套件的报告。
///
/// `run_item` 为骨架实现，恒返回通过；真实实现将通过协议适配器层调用
/// 被测设备 API 并记录实际结果。
pub struct DefaultTestRunner {
    /// 已执行套件报告历史（按运行顺序）。
    pub reports: Vec<TestReport>,
}

impl DefaultTestRunner {
    pub fn new() -> Self {
        Self {
            reports: Vec::new(),
        }
    }
}

impl Default for DefaultTestRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl FactoryTestRunner for DefaultTestRunner {
    fn run_suite(&mut self, suite: &TestSuite) -> TestReport {
        let suite_start = now_ms();
        let mut passed = 0u32;
        let mut failed = 0u32;
        let mut failures = Vec::new();
        let mut total_duration = 0u64;

        for item in &suite.items {
            let result = self.run_item(item);
            total_duration += result.duration_ms;
            if result.passed {
                passed += 1;
            } else {
                failed += 1;
                failures.push(TestFailure {
                    test_name: result.name.clone(),
                    reason: result
                        .failure_reason
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                    timestamp: now_ms(),
                });
            }
        }

        let report = TestReport {
            suite_name: suite.name.clone(),
            total: suite.items.len() as u32,
            passed,
            failed,
            duration_ms: now_ms().saturating_sub(suite_start) + total_duration,
            failures,
        };
        self.reports.push(report.clone());
        report
    }

    fn run_item(&mut self, item: &TestItem) -> TestItem {
        // Stub: 恒通过。真实实现将通过协议适配器层调用设备 API。
        TestItem {
            name: item.name.clone(),
            category: item.category.clone(),
            passed: true,
            failure_reason: None,
            duration_ms: 0,
        }
    }
}

/// 尽力获取的墙上时钟毫秒（主机侧 std）。
fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_suite() -> TestSuite {
        TestSuite {
            name: "smoke".to_string(),
            items: vec![
                TestItem {
                    name: "ping".to_string(),
                    category: TestCategory::Communication,
                    passed: false,
                    failure_reason: None,
                    duration_ms: 0,
                },
                TestItem {
                    name: "read_points".to_string(),
                    category: TestCategory::Functional,
                    passed: false,
                    failure_reason: None,
                    duration_ms: 0,
                },
            ],
        }
    }

    #[test]
    fn runner_passes_all_items_in_stub() {
        let mut r = DefaultTestRunner::new();
        let report = r.run_suite(&sample_suite());
        assert_eq!(report.total, 2);
        assert_eq!(report.passed, 2);
        assert_eq!(report.failed, 0);
        assert!(report.failures.is_empty());
        assert_eq!(r.reports.len(), 1);
    }

    #[test]
    fn run_item_marks_passed() {
        let mut r = DefaultTestRunner::new();
        let item = TestItem {
            name: "x".to_string(),
            category: TestCategory::Performance,
            passed: false,
            failure_reason: Some("pre".to_string()),
            duration_ms: 5,
        };
        let out = r.run_item(&item);
        assert!(out.passed);
        assert!(out.failure_reason.is_none());
        assert_eq!(out.name, "x");
    }

    #[test]
    fn summary_contains_counts() {
        let report = TestReport {
            suite_name: "s".to_string(),
            total: 4,
            passed: 3,
            failed: 1,
            duration_ms: 123,
            failures: vec![TestFailure {
                test_name: "x".to_string(),
                reason: "boom".to_string(),
                timestamp: 1,
            }],
        };
        let s = report.summary();
        assert!(s.contains("total=4"));
        assert!(s.contains("passed=3"));
        assert!(s.contains("failed=1"));
        assert!(s.contains("1 failure(s)"));
    }

    #[test]
    fn category_display() {
        assert_eq!(TestCategory::Functional.to_string(), "Functional");
        assert_eq!(TestCategory::Safety.to_string(), "Safety");
    }
}
