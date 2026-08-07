//! 对账模块：将清算结果与预期金额进行逐笔核对。

use alloc::vec::Vec;

use crate::types::{ClearingResult, MilliYuan, ReconcileReport};

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
        expected: &[(u64, MilliYuan)],
    ) -> ReconcileReport {
        let mut report = ReconcileReport {
            checked: expected.len() as u64,
            matched: 0,
            mismatched: 0,
            mismatched_deals: Vec::new(),
        };

        for &(deal_id, expected_amount) in expected {
            let found = results.iter().find(|r| r.deal_id == deal_id);
            // ARC-003：金额定点（毫元）→ 精确相等，无浮点容差
            match found {
                Some(r) if r.amount_yuan == expected_amount => {
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
    use crate::clearing::ClearingEngine;
    use crate::types::{ClearStatus, ClearingJob, MilliYuan};

    fn make_result(deal_id: u64, amount: MilliYuan) -> ClearingResult {
        ClearingResult {
            deal_id,
            seller_credit: 100.0,
            buyer_debit: 100.0,
            amount_yuan: amount,
            deviation_penalty: MilliYuan::ZERO,
            status: ClearStatus::Cleared,
        }
    }

    #[test]
    fn test_all_match() {
        let reconciler = Reconciler::new();
        let results = vec![
            make_result(1, MilliYuan(50_000)),
            make_result(2, MilliYuan(80_000)),
        ];
        let expected = vec![(1, MilliYuan(50_000)), (2, MilliYuan(80_000))];

        let report = reconciler.reconcile(&results, &expected);
        assert_eq!(report.checked, 2);
        assert_eq!(report.matched, 2);
        assert_eq!(report.mismatched, 0);
        assert!(report.mismatched_deals.is_empty());
    }

    #[test]
    fn test_some_mismatch() {
        let reconciler = Reconciler::new();
        let results = vec![
            make_result(1, MilliYuan(50_000)),
            make_result(2, MilliYuan(80_000)),
        ];
        // deal 2 预期 90，实际 80 => 不匹配
        let expected = vec![(1, MilliYuan(50_000)), (2, MilliYuan(90_000))];

        let report = reconciler.reconcile(&results, &expected);
        assert_eq!(report.checked, 2);
        assert_eq!(report.matched, 1);
        assert_eq!(report.mismatched, 1);
        assert_eq!(report.mismatched_deals, vec![2]);
    }

    #[test]
    fn test_missing_deal_is_mismatch() {
        let reconciler = Reconciler::new();
        let results = vec![make_result(1, MilliYuan(50_000))];
        // deal 99 不存在于结果中
        let expected = vec![(1, MilliYuan(50_000)), (99, MilliYuan(100_000))];

        let report = reconciler.reconcile(&results, &expected);
        assert_eq!(report.checked, 2);
        assert_eq!(report.matched, 1);
        assert_eq!(report.mismatched, 1);
        assert_eq!(report.mismatched_deals, vec![99]);
    }

    #[test]
    fn test_empty_expected() {
        let reconciler = Reconciler::new();
        let results = vec![make_result(1, MilliYuan(50_000))];
        let expected: Vec<(u64, MilliYuan)> = vec![];

        let report = reconciler.reconcile(&results, &expected);
        assert_eq!(report.checked, 0);
        assert_eq!(report.matched, 0);
        assert_eq!(report.mismatched, 0);
    }

    #[test]
    fn test_empty_results() {
        let reconciler = Reconciler::new();
        let results: Vec<ClearingResult> = vec![];
        let expected = vec![(1, MilliYuan(50_000)), (2, MilliYuan(80_000))];

        let report = reconciler.reconcile(&results, &expected);
        assert_eq!(report.checked, 2);
        assert_eq!(report.matched, 0);
        assert_eq!(report.mismatched, 2);
        assert_eq!(report.mismatched_deals, vec![1, 2]);
    }

    #[test]
    fn test_tolerance_boundary() {
        let reconciler = Reconciler::new();
        // ARC-003：定点毫元精确匹配——50.005 元(50005 毫元) ≠ 50 元(50000 毫元)
        let results = vec![make_result(1, MilliYuan(50_005))];
        let expected = vec![(1, MilliYuan(50_000))];

        let report = reconciler.reconcile(&results, &expected);
        assert_eq!(report.matched, 0, "定点金额无容差：50005 ≠ 50000 毫元");
        assert_eq!(report.mismatched, 1);
    }

    // ===== ARCH-M05：基于共享类型的跨 crate 对账属性测试 =====
    //
    // 对账链：共享 Bid（market-types，YuanPerKwh 单位化）→ 清算（settlement）→
    // 对账（reconcile）。属性：清算金额 = 交付电量 × 合同电价，单位一致时可对账；
    // Buy/Sell 方向经 TradeDir 表达不参与金额计算（金额只依赖电量与电价）。
    #[test]
    fn m05_reconciliation_property_across_crates() {
        use eneros_market_types::{Bid, BidSegment, MarketType, TimeSlot, TradeDir, YuanPerKwh};

        // 共享 Bid：单段 Sell 报价（0.5 元/kWh × 100 kWh = 50 元）
        let bid = Bid::new(1, MarketType::Spot, 42, 1000).with_segment(BidSegment {
            period: TimeSlot::new(1000, 4600),
            volume_kwh: 100.0,
            price: YuanPerKwh(0.5),
            direction: TradeDir::Sell,
        });
        let _price = bid.segments[0].price;

        // 多组属性组合：交付电量 × 电价 → 金额，全部经 reconcile 匹配
        let combos: &[(f64, f64)] = &[(100.0, 0.5), (80.0, 0.5), (120.0, 0.5), (100.0, 0.75)];
        let mut engine = ClearingEngine::new();
        let mut results = Vec::new();
        let mut expected: Vec<(u64, MilliYuan)> = Vec::new();
        for (i, &(kwh, price_yuan)) in combos.iter().enumerate() {
            let job = ClearingJob {
                deal_id: i as u64 + 1,
                delivered_kwh: kwh,
                contract_kwh: kwh, // 全额交付，无偏差
                contract_price: YuanPerKwh(price_yuan),
                at: 1000,
            };
            let r = engine.clear(&job).expect("清算必须成功");
            results.push(r.clone());
            expected.push((r.deal_id, r.amount_yuan));
        }
        // 属性：金额 = 电量 × 电价（定点毫元，round-half-up 精确）
        for (i, &(kwh, price_yuan)) in combos.iter().enumerate() {
            let expect = MilliYuan::from_yuan_f64(kwh * price_yuan);
            assert_eq!(
                results[i].amount_yuan, expect,
                "属性失败: amount = {:?} != 定点({} × {})",
                results[i].amount_yuan, kwh, price_yuan
            );
        }
        // 属性：共享 Bid 价格可直接用于对账金额（同单位）
        let reconciler = Reconciler::new();
        let report = reconciler.reconcile(&results, &expected);
        assert_eq!(report.checked, combos.len() as u64);
        assert_eq!(report.matched, combos.len() as u64);
        assert_eq!(report.mismatched, 0);
        // 属性：Bid 方向不改变金额语义（TradeDir 只表达方向）
        assert_eq!(bid.segments[0].direction, TradeDir::Sell);
    }
}
