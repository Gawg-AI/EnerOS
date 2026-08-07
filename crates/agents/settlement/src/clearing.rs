//! 清算引擎：根据交付电量与合同电量计算结算金额及偏差考核。

use alloc::vec::Vec;

#[cfg(test)]
use crate::types::YuanPerKwh;
use crate::types::{ClearStatus, ClearingJob, ClearingResult, MilliYuan};
use crate::SettlementError;

/// 清算引擎。
#[derive(Debug)]
pub struct ClearingEngine {
    /// 历史清算结果。
    pub results: Vec<ClearingResult>,
    /// 偏差考核费率（默认 5%）。
    pub penalty_rate: f64,
}

impl Default for ClearingEngine {
    fn default() -> Self {
        Self {
            results: Vec::new(),
            penalty_rate: 0.05,
        }
    }
}

impl ClearingEngine {
    /// 创建清算引擎，使用默认考核费率 5%。
    pub fn new() -> Self {
        Self::default()
    }

    /// 执行单笔清算。
    ///
    /// - 偏差率 = |delivered - contract| / contract
    /// - 偏差率 > 5%：标记 ShortDelivered / OverDelivered，收取考核费
    /// - 否则：正常 Cleared，无考核费
    pub fn clear(&mut self, job: &ClearingJob) -> Result<ClearingResult, SettlementError> {
        if job.contract_kwh == 0.0 {
            return Err(SettlementError::InvalidJob);
        }

        // ARC-003：金额定点化——电量 × 电价（f64 精确积）round-half-up 定毫；
        // 考核费 = 金额 × 费率（毫元定点乘）。金额路径无 f64 残留。
        let amount_yuan = MilliYuan::from_yuan_f64(job.delivered_kwh * job.contract_price.0);
        let deviation = (job.delivered_kwh - job.contract_kwh).abs() / job.contract_kwh;

        let (status, penalty) = if deviation > 0.05 {
            let penalty = amount_yuan.mul_f64(self.penalty_rate);
            if job.delivered_kwh < job.contract_kwh {
                (ClearStatus::ShortDelivered, penalty)
            } else {
                (ClearStatus::OverDelivered, penalty)
            }
        } else {
            (ClearStatus::Cleared, MilliYuan::ZERO)
        };

        let result = ClearingResult {
            deal_id: job.deal_id,
            seller_credit: job.delivered_kwh,
            buyer_debit: job.delivered_kwh,
            amount_yuan,
            deviation_penalty: penalty,
            status,
        };

        self.results.push(result.clone());
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_job(delivered: f64, contract: f64, price: f64) -> ClearingJob {
        ClearingJob {
            deal_id: 1,
            delivered_kwh: delivered,
            contract_kwh: contract,
            contract_price: YuanPerKwh(price),
            at: 1000,
        }
    }

    #[test]
    fn test_normal_clearing() {
        let mut engine = ClearingEngine::new();
        let job = make_job(100.0, 100.0, 0.5);
        let result = engine.clear(&job).unwrap();

        assert_eq!(result.status, ClearStatus::Cleared);
        assert_eq!(
            result.amount_yuan,
            MilliYuan(50_000),
            "100 kWh × 0.5 元 = 50 元 = 50000 毫元"
        );
        assert_eq!(result.deviation_penalty, MilliYuan::ZERO);
        assert_eq!(result.seller_credit, 100.0);
        assert_eq!(result.buyer_debit, 100.0);
    }

    #[test]
    fn test_short_delivery_with_penalty() {
        let mut engine = ClearingEngine::new();
        // 交付 80，合同 100 => 偏差 20% > 5%
        let job = make_job(80.0, 100.0, 0.5);
        let result = engine.clear(&job).unwrap();

        assert_eq!(result.status, ClearStatus::ShortDelivered);
        assert_eq!(result.amount_yuan, MilliYuan(40_000));
        assert_eq!(
            result.deviation_penalty,
            MilliYuan(2_000),
            "penalty = 40 元 × 5% = 2 元"
        );
    }

    #[test]
    fn test_over_delivery_with_penalty() {
        let mut engine = ClearingEngine::new();
        // 交付 120，合同 100 => 偏差 20% > 5%
        let job = make_job(120.0, 100.0, 0.5);
        let result = engine.clear(&job).unwrap();

        assert_eq!(result.status, ClearStatus::OverDelivered);
        assert_eq!(result.amount_yuan, MilliYuan(60_000));
        assert_eq!(
            result.deviation_penalty,
            MilliYuan(3_000),
            "penalty = 60 元 × 5% = 3 元"
        );
    }

    #[test]
    fn test_zero_contract_error() {
        let mut engine = ClearingEngine::new();
        let job = make_job(100.0, 0.0, 0.5);
        let result = engine.clear(&job);

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), SettlementError::InvalidJob);
    }

    #[test]
    fn test_within_tolerance_no_penalty() {
        let mut engine = ClearingEngine::new();
        // 交付 103，合同 100 => 偏差 3% <= 5%
        let job = make_job(103.0, 100.0, 0.5);
        let result = engine.clear(&job).unwrap();

        assert_eq!(result.status, ClearStatus::Cleared);
        assert_eq!(result.deviation_penalty, MilliYuan::ZERO);
    }

    #[test]
    fn test_results_accumulated() {
        let mut engine = ClearingEngine::new();
        let job1 = make_job(100.0, 100.0, 0.5);
        let job2 = make_job(90.0, 100.0, 0.6);
        engine.clear(&job1).unwrap();
        engine.clear(&job2).unwrap();

        assert_eq!(engine.results.len(), 2);
    }
}
