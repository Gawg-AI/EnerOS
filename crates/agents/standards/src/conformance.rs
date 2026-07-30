//! 一致性检查 — 验证实现对标准条款的符合程度。

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

use crate::types::{ConformanceClause, ConformanceReport};

/// 一致性检查器 trait
pub trait ConformanceChecker {
    /// 检查条款符合情况
    ///
    /// - `clauses`: 待检查的一致性条款列表
    /// - `passed`: 已通过测试的 ID 列表
    fn check(&self, clauses: &[ConformanceClause], passed: &[String]) -> ConformanceReport;
}

/// Power Native OS 一致性检查实现
pub struct PnoConformance;

impl ConformanceChecker for PnoConformance {
    fn check(&self, clauses: &[ConformanceClause], passed: &[String]) -> ConformanceReport {
        let total = clauses.len() as u32;
        let mut conformed: u32 = 0;
        let mut non_conformed: Vec<String> = Vec::new();

        for clause in clauses {
            let is_passed = passed.contains(&clause.test);
            if is_passed {
                conformed += 1;
            } else if clause.must {
                non_conformed.push(clause.id.clone());
            }
        }

        ConformanceReport {
            total,
            conformed,
            non_conformed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_clauses() -> Vec<ConformanceClause> {
        alloc::vec![
            ConformanceClause {
                id: String::from("c1"),
                must: true,
                test: String::from("test-boot"),
            },
            ConformanceClause {
                id: String::from("c2"),
                must: true,
                test: String::from("test-sched"),
            },
            ConformanceClause {
                id: String::from("c3"),
                must: false,
                test: String::from("test-optional"),
            },
        ]
    }

    #[test]
    fn test_all_conform() {
        let checker = PnoConformance;
        let clauses = make_clauses();
        let passed = alloc::vec![
            String::from("test-boot"),
            String::from("test-sched"),
            String::from("test-optional"),
        ];
        let report = checker.check(&clauses, &passed);
        assert_eq!(report.total, 3);
        assert_eq!(report.conformed, 3);
        assert!(report.non_conformed.is_empty());
    }

    #[test]
    fn test_some_non_conform() {
        let checker = PnoConformance;
        let clauses = make_clauses();
        let passed = alloc::vec![String::from("test-boot")];
        let report = checker.check(&clauses, &passed);
        assert_eq!(report.total, 3);
        assert_eq!(report.conformed, 1);
        // c2 is must and failed -> non_conformed
        assert_eq!(report.non_conformed, alloc::vec![String::from("c2")]);
    }

    #[test]
    fn test_must_vs_optional() {
        let checker = PnoConformance;
        let clauses = make_clauses();
        // Only optional test fails
        let passed = alloc::vec![String::from("test-boot"), String::from("test-sched")];
        let report = checker.check(&clauses, &passed);
        assert_eq!(report.total, 3);
        assert_eq!(report.conformed, 2);
        // c3 is optional, so not in non_conformed
        assert!(report.non_conformed.is_empty());
    }

    #[test]
    fn test_empty_clauses() {
        let checker = PnoConformance;
        let report = checker.check(&[], &[]);
        assert_eq!(report.total, 0);
        assert_eq!(report.conformed, 0);
        assert!(report.non_conformed.is_empty());
    }
}
