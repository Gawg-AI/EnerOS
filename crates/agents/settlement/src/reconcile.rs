//! 对账模块：将清算结果与预期金额进行逐笔核对。

use alloc::vec::Vec;

use crate::types::{ClearingResult, ReconcileReport};

/// 金额匹配容差（元）。
const AMOUNT_TOLERANCE: f64 = 0.01;

/// 对账器。
#[derive(Debug, Default)]
pub struct Reconciler;

impl Reconciler {
    /// 创建对账器。
    pub fn new() -> Self {
        Self
    }

    /// 执行对账。
    ///
    /// 对每条预期记录 `(deal_id, amount)`，在清算结果中查找对应 deal_id，
    /// 若找到且金额差 <= 0.01 则视为匹配，否则视为不匹配。
    pub fn reconcile(
        &self,
        results: &[ClearingResult],
        expected: &[(u64, f64)],
    ) -> ReconcileReport {
        let mut report = ReconcileReport {
            checked: expected.len() as u64,
            matched: 0,
            mismatched: 0,
            mismatched_deals: Vec::new(),
        };

        for &(deal_id, expected_amount) in expected {
            let found = results.iter().find(|r| r.deal_id == deal_id);
            match found {
                Some(r) if (r.amount_yuan - expected_amount).abs() <= AMOUNT_TOLERANCE => {
                    report.matched += 1;
                }
                _ => {
                    report.mismatched += 1;
                    report.mismatched_deals.push(deal_id);
                }
            }
        }

        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ClearStatus;

    fn make_result(deal_id: u64, amount: f64) -> ClearingResult {
        ClearingResult {
            deal_id,
            seller_credit: 100.0,
            buyer_debit: 100.0,
            amount_yuan: amount,
            deviation_penalty: 0.0,
            status: ClearStatus::Cleared,
        }
    }

    #[test]
    fn test_all_match() {
        let reconciler = Reconciler::new();
        let results = vec![make_result(1, 50.0), make_result(2, 80.0)];
        let expected = vec![(1, 50.0), (2, 80.0)];

        let report = reconciler.reconcile(&results, &expected);
        assert_eq!(report.checked, 2);
        assert_eq!(report.matched, 2);
        assert_eq!(report.mismatched, 0);
        assert!(report.mismatched_deals.is_empty());
    }

    #[test]
    fn test_some_mismatch() {
        let reconciler = Reconciler::new();
        let results = vec![make_result(1, 50.0), make_result(2, 80.0)];
        // deal 2 预期 90，实际 80 => 不匹配
        let expected = vec![(1, 50.0), (2, 90.0)];

        let report = reconciler.reconcile(&results, &expected);
        assert_eq!(report.checked, 2);
        assert_eq!(report.matched, 1);
        assert_eq!(report.mismatched, 1);
        assert_eq!(report.mismatched_deals, vec![2]);
    }

    #[test]
    fn test_missing_deal_is_mismatch() {
        let reconciler = Reconciler::new();
        let results = vec![make_result(1, 50.0)];
        // deal 99 不存在于结果中
        let expected = vec![(1, 50.0), (99, 100.0)];

        let report = reconciler.reconcile(&results, &expected);
        assert_eq!(report.checked, 2);
        assert_eq!(report.matched, 1);
        assert_eq!(report.mismatched, 1);
        assert_eq!(report.mismatched_deals, vec![99]);
    }

    #[test]
    fn test_empty_expected() {
        let reconciler = Reconciler::new();
        let results = vec![make_result(1, 50.0)];
        let expected: Vec<(u64, f64)> = vec![];

        let report = reconciler.reconcile(&results, &expected);
        assert_eq!(report.checked, 0);
        assert_eq!(report.matched, 0);
        assert_eq!(report.mismatched, 0);
    }

    #[test]
    fn test_empty_results() {
        let reconciler = Reconciler::new();
        let results: Vec<ClearingResult> = vec![];
        let expected = vec![(1, 50.0), (2, 80.0)];

        let report = reconciler.reconcile(&results, &expected);
        assert_eq!(report.checked, 2);
        assert_eq!(report.matched, 0);
        assert_eq!(report.mismatched, 2);
        assert_eq!(report.mismatched_deals, vec![1, 2]);
    }

    #[test]
    fn test_tolerance_boundary() {
        let reconciler = Reconciler::new();
        let results = vec![make_result(1, 50.005)];
        // 差值 0.005 < 0.01 => 匹配
        let expected = vec![(1, 50.0)];

        let report = reconciler.reconcile(&results, &expected);
        assert_eq!(report.matched, 1);
        assert_eq!(report.mismatched, 0);
    }
}
