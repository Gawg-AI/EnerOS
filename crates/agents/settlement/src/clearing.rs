//! 清算引擎：根据交付电量与合同电量计算结算金额及偏差考核。

use alloc::vec::Vec;

use crate::types::{ClearStatus, ClearingJob, ClearingResult};
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

        let amount_yuan = job.delivered_kwh * job.contract_price;
        let deviation = (job.delivered_kwh - job.contract_kwh).abs() / job.contract_kwh;

        let (status, penalty) = if deviation > 0.05 {
            let penalty = amount_yuan * self.penalty_rate;
            if job.delivered_kwh < job.contract_kwh {
                (ClearStatus::ShortDelivered, penalty)
            } else {
                (ClearStatus::OverDelivered, penalty)
            }
        } else {
            (ClearStatus::Cleared, 0.0)
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
            contract_price: price,
            at: 1000,
        }
    }

    #[test]
    fn test_normal_clearing() {
        let mut engine = ClearingEngine::new();
        let job = make_job(100.0, 100.0, 0.5);
        let result = engine.clear(&job).unwrap();

        assert_eq!(result.status, ClearStatus::Cleared);
        assert_eq!(result.amount_yuan, 50.0);
        assert_eq!(result.deviation_penalty, 0.0);
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
        assert_eq!(result.amount_yuan, 40.0);
        // penalty = 40 * 0.05 = 2.0
        assert!((result.deviation_penalty - 2.0).abs() < 1e-9);
    }

    #[test]
    fn test_over_delivery_with_penalty() {
        let mut engine = ClearingEngine::new();
        // 交付 120，合同 100 => 偏差 20% > 5%
        let job = make_job(120.0, 100.0, 0.5);
        let result = engine.clear(&job).unwrap();

        assert_eq!(result.status, ClearStatus::OverDelivered);
        assert_eq!(result.amount_yuan, 60.0);
        // penalty = 60 * 0.05 = 3.0
        assert!((result.deviation_penalty - 3.0).abs() < 1e-9);
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
        assert_eq!(result.deviation_penalty, 0.0);
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
