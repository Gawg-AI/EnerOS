//! 结算核心数据类型。

use alloc::string::String;
use alloc::vec::Vec;

/// 清算状态。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClearStatus {
    /// 正常清算。
    Cleared,
    /// 存在争议。
    Disputed,
    /// 欠交付（实际交付 < 合同电量）。
    ShortDelivered,
    /// 超交付（实际交付 > 合同电量）。
    OverDelivered,
}

/// 清算任务输入。
#[derive(Clone, Debug)]
pub struct ClearingJob {
    /// 交易 ID。
    pub deal_id: u64,
    /// 实际交付电量 (kWh)。
    pub delivered_kwh: f64,
    /// 合同电量 (kWh)。
    pub contract_kwh: f64,
    /// 合同电价 (元/kWh)。
    pub contract_price: f64,
    /// 时间戳。
    pub at: u64,
}

/// 清算结果。
#[derive(Clone, Debug)]
pub struct ClearingResult {
    /// 交易 ID。
    pub deal_id: u64,
    /// 卖方入账电量。
    pub seller_credit: f64,
    /// 买方扣减电量。
    pub buyer_debit: f64,
    /// 结算金额（元）。
    pub amount_yuan: f64,
    /// 偏差考核费（元）。
    pub deviation_penalty: f64,
    /// 清算状态。
    pub status: ClearStatus,
}

/// 对账报告。
#[derive(Clone, Debug, Default)]
pub struct ReconcileReport {
    /// 核对总数。
    pub checked: u64,
    /// 匹配数。
    pub matched: u64,
    /// 不匹配数。
    pub mismatched: u64,
    /// 不匹配的交易 ID 列表。
    pub mismatched_deals: Vec<u64>,
}

/// 争议状态。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DisputeState {
    /// 已开启。
    Open,
    /// 仲裁中。
    Arbitrating,
    /// 已解决。
    Resolved,
}

/// 争议案例。
#[derive(Clone, Debug)]
pub struct DisputeCase {
    /// 争议 ID。
    pub case_id: u64,
    /// 关联交易 ID。
    pub deal_id: u64,
    /// 争议原因。
    pub reason: String,
    /// 当前状态。
    pub state: DisputeState,
    /// 开启时间戳。
    pub opened_at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clear_status_equality() {
        assert_eq!(ClearStatus::Cleared, ClearStatus::Cleared);
        assert_ne!(ClearStatus::Cleared, ClearStatus::Disputed);
        assert_ne!(ClearStatus::ShortDelivered, ClearStatus::OverDelivered);
    }

    #[test]
    fn test_clearing_job_creation() {
        let job = ClearingJob {
            deal_id: 1,
            delivered_kwh: 100.0,
            contract_kwh: 100.0,
            contract_price: 0.5,
            at: 1000,
        };
        assert_eq!(job.deal_id, 1);
        assert_eq!(job.delivered_kwh, 100.0);
        assert_eq!(job.contract_kwh, 100.0);
        assert_eq!(job.contract_price, 0.5);
        assert_eq!(job.at, 1000);
    }

    #[test]
    fn test_clearing_result_fields() {
        let result = ClearingResult {
            deal_id: 42,
            seller_credit: 100.0,
            buyer_debit: 100.0,
            amount_yuan: 50.0,
            deviation_penalty: 0.0,
            status: ClearStatus::Cleared,
        };
        assert_eq!(result.deal_id, 42);
        assert_eq!(result.status, ClearStatus::Cleared);
        assert_eq!(result.deviation_penalty, 0.0);
    }

    #[test]
    fn test_reconcile_report_default() {
        let report = ReconcileReport::default();
        assert_eq!(report.checked, 0);
        assert_eq!(report.matched, 0);
        assert_eq!(report.mismatched, 0);
        assert!(report.mismatched_deals.is_empty());
    }

    #[test]
    fn test_dispute_state_transitions() {
        assert_eq!(DisputeState::Open, DisputeState::Open);
        assert_ne!(DisputeState::Open, DisputeState::Arbitrating);
        assert_ne!(DisputeState::Arbitrating, DisputeState::Resolved);
    }

    #[test]
    fn test_dispute_case_creation() {
        let case = DisputeCase {
            case_id: 1,
            deal_id: 100,
            reason: String::from("金额不符"),
            state: DisputeState::Open,
            opened_at: 2000,
        };
        assert_eq!(case.case_id, 1);
        assert_eq!(case.deal_id, 100);
        assert_eq!(case.reason, "金额不符");
        assert_eq!(case.state, DisputeState::Open);
        assert_eq!(case.opened_at, 2000);
    }
}
