//! 合规检查核心类型定义.
//!
//! 提供 `CheckStatus`、`CheckItem`、`VerificationReport`、`Phase3ExitReport`
//! 等结构，用于信创认证与等保 2.0 三级验证。

use alloc::string::String;
use alloc::vec::Vec;

/// 检查项状态.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckStatus {
    /// 通过.
    Passed,
    /// 未通过.
    Failed,
    /// 待验证.
    Pending,
    /// 不适用.
    NotApplicable,
}

/// 单个检查项.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckItem {
    /// 检查项编号.
    pub id: u32,
    /// 检查项名称.
    pub name: String,
    /// 所属类别.
    pub category: String,
    /// 当前状态.
    pub status: CheckStatus,
    /// 佐证说明.
    pub evidence: String,
    /// 关联版本.
    pub version: String,
}

impl CheckItem {
    /// 创建新检查项.
    pub fn new(
        id: u32,
        name: &str,
        category: &str,
        status: CheckStatus,
        evidence: &str,
        version: &str,
    ) -> Self {
        Self {
            id,
            name: String::from(name),
            category: String::from(category),
            status,
            evidence: String::from(evidence),
            version: String::from(version),
        }
    }
}

/// 验证报告.
#[derive(Clone, Debug)]
pub struct VerificationReport {
    /// 报告标题.
    pub title: String,
    /// 检查项列表.
    pub items: Vec<CheckItem>,
    /// 通过数量.
    pub passed_count: u32,
    /// 未通过数量.
    pub failed_count: u32,
    /// 总数量.
    pub total: u32,
}

impl VerificationReport {
    /// 创建验证报告并自动统计.
    pub fn new(title: &str, items: Vec<CheckItem>) -> Self {
        let total = items.len() as u32;
        let passed_count = items
            .iter()
            .filter(|i| i.status == CheckStatus::Passed)
            .count() as u32;
        let failed_count = items
            .iter()
            .filter(|i| i.status == CheckStatus::Failed)
            .count() as u32;
        Self {
            title: String::from(title),
            items,
            passed_count,
            failed_count,
            total,
        }
    }

    /// 是否全部通过.
    pub fn all_passed(&self) -> bool {
        self.failed_count == 0 && self.total > 0 && self.passed_count == self.total
    }
}

/// Phase 3 出口验证报告.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Phase3ExitReport {
    /// 自研内核验证通过.
    pub kernel_ok: bool,
    /// 国产 CPU 支持验证通过.
    pub cpu_ok: bool,
    /// 信创认证验证通过.
    pub xinchuang_ok: bool,
    /// 等保 2.0 三级验证通过.
    pub dengbao_ok: bool,
}

impl Phase3ExitReport {
    /// 是否全部通过.
    pub fn all_passed(&self) -> bool {
        self.kernel_ok && self.cpu_ok && self.xinchuang_ok && self.dengbao_ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_item_new() {
        let item = CheckItem::new(1, "测试项", "安全", CheckStatus::Passed, "证据", "v0.170.0");
        assert_eq!(item.id, 1);
        assert_eq!(item.name, "测试项");
        assert_eq!(item.status, CheckStatus::Passed);
    }

    #[test]
    fn test_verification_report_all_passed() {
        let items = alloc::vec![
            CheckItem::new(1, "A", "cat", CheckStatus::Passed, "ev", "v1"),
            CheckItem::new(2, "B", "cat", CheckStatus::Passed, "ev", "v1"),
        ];
        let report = VerificationReport::new("测试报告", items);
        assert_eq!(report.total, 2);
        assert_eq!(report.passed_count, 2);
        assert_eq!(report.failed_count, 0);
        assert!(report.all_passed());
    }

    #[test]
    fn test_verification_report_with_failure() {
        let items = alloc::vec![
            CheckItem::new(1, "A", "cat", CheckStatus::Passed, "ev", "v1"),
            CheckItem::new(2, "B", "cat", CheckStatus::Failed, "ev", "v1"),
        ];
        let report = VerificationReport::new("测试报告", items);
        assert_eq!(report.failed_count, 1);
        assert!(!report.all_passed());
    }

    #[test]
    fn test_verification_report_empty() {
        let report = VerificationReport::new("空报告", Vec::new());
        assert_eq!(report.total, 0);
        assert!(!report.all_passed());
    }

    #[test]
    fn test_phase3_exit_report_all_passed() {
        let report = Phase3ExitReport {
            kernel_ok: true,
            cpu_ok: true,
            xinchuang_ok: true,
            dengbao_ok: true,
        };
        assert!(report.all_passed());
    }

    #[test]
    fn test_phase3_exit_report_partial() {
        let report = Phase3ExitReport {
            kernel_ok: true,
            cpu_ok: true,
            xinchuang_ok: false,
            dengbao_ok: true,
        };
        assert!(!report.all_passed());
    }
}
