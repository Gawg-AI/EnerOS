//! 成交管理器

use alloc::vec::Vec;

use crate::types::{Deal, DealState, ExecReport};
use crate::MatchError;

/// 成交管理器
#[derive(Debug, Clone, Default)]
pub struct DealManager {
    /// 成交记录列表
    pub deals: Vec<Deal>,
}

impl DealManager {
    /// 创建新的成交管理器
    pub fn new() -> Self {
        Self { deals: Vec::new() }
    }

    /// 添加成交记录
    pub fn add_deal(&mut self, deal: Deal) {
        self.deals.push(deal);
    }

    /// 根据 ID 查询成交记录
    pub fn get_deal(&self, id: u64) -> Option<&Deal> {
        self.deals.iter().find(|d| d.deal_id == id)
    }

    /// 状态转换
    ///
    /// 合法转换:
    /// - Matched → Confirmed | Cancelled
    /// - Confirmed → Delivering | Cancelled
    /// - Delivering → Settled
    pub fn transition(&mut self, id: u64, new_state: DealState) -> Result<(), MatchError> {
        let deal = self
            .deals
            .iter_mut()
            .find(|d| d.deal_id == id)
            .ok_or(MatchError::DealNotFound)?;

        let valid = matches!(
            (deal.state, new_state),
            (DealState::Matched, DealState::Confirmed)
                | (DealState::Matched, DealState::Cancelled)
                | (DealState::Confirmed, DealState::Delivering)
                | (DealState::Confirmed, DealState::Cancelled)
                | (DealState::Delivering, DealState::Settled)
        );

        if !valid {
            return Err(MatchError::InvalidTransition);
        }

        deal.state = new_state;
        Ok(())
    }

    /// 记录执行报告
    pub fn record_execution(&mut self, id: u64, report: ExecReport) -> Result<(), MatchError> {
        let deal = self
            .deals
            .iter()
            .find(|d| d.deal_id == id)
            .ok_or(MatchError::DealNotFound)?;

        // 验证交割量不超过成交量
        if report.delivered_kwh > deal.volume_kwh {
            return Err(MatchError::VolumeMismatch);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_deal(id: u64) -> Deal {
        Deal {
            deal_id: id,
            seller: 1,
            buyer: 2,
            seller_seq: 0,
            buyer_seq: 0,
            volume_kwh: 100.0,
            price: 0.65,
            window_start: 1000,
            window_end: 2000,
            state: DealState::Matched,
        }
    }

    #[test]
    fn test_add_and_get_deal() {
        let mut mgr = DealManager::new();
        mgr.add_deal(make_deal(1));

        let deal = mgr.get_deal(1).expect("deal should exist");
        assert_eq!(deal.deal_id, 1);
        assert_eq!(deal.seller, 1);
        assert_eq!(deal.buyer, 2);
    }

    #[test]
    fn test_get_deal_not_found() {
        let mgr = DealManager::new();
        assert!(mgr.get_deal(999).is_none());
    }

    #[test]
    fn test_valid_transition_matched_to_confirmed() {
        let mut mgr = DealManager::new();
        mgr.add_deal(make_deal(1));

        assert!(mgr.transition(1, DealState::Confirmed).is_ok());
        assert_eq!(mgr.get_deal(1).unwrap().state, DealState::Confirmed);
    }

    #[test]
    fn test_valid_transition_matched_to_cancelled() {
        let mut mgr = DealManager::new();
        mgr.add_deal(make_deal(1));

        assert!(mgr.transition(1, DealState::Cancelled).is_ok());
        assert_eq!(mgr.get_deal(1).unwrap().state, DealState::Cancelled);
    }

    #[test]
    fn test_valid_transition_confirmed_to_delivering() {
        let mut mgr = DealManager::new();
        mgr.add_deal(make_deal(1));
        mgr.transition(1, DealState::Confirmed).unwrap();

        assert!(mgr.transition(1, DealState::Delivering).is_ok());
        assert_eq!(mgr.get_deal(1).unwrap().state, DealState::Delivering);
    }

    #[test]
    fn test_valid_transition_delivering_to_settled() {
        let mut mgr = DealManager::new();
        mgr.add_deal(make_deal(1));
        mgr.transition(1, DealState::Confirmed).unwrap();
        mgr.transition(1, DealState::Delivering).unwrap();

        assert!(mgr.transition(1, DealState::Settled).is_ok());
        assert_eq!(mgr.get_deal(1).unwrap().state, DealState::Settled);
    }

    #[test]
    fn test_invalid_transition_matched_to_settled() {
        let mut mgr = DealManager::new();
        mgr.add_deal(make_deal(1));

        let result = mgr.transition(1, DealState::Settled);
        assert_eq!(result, Err(MatchError::InvalidTransition));
    }

    #[test]
    fn test_invalid_transition_settled_to_any() {
        let mut mgr = DealManager::new();
        mgr.add_deal(make_deal(1));
        mgr.transition(1, DealState::Confirmed).unwrap();
        mgr.transition(1, DealState::Delivering).unwrap();
        mgr.transition(1, DealState::Settled).unwrap();

        assert_eq!(
            mgr.transition(1, DealState::Cancelled),
            Err(MatchError::InvalidTransition)
        );
    }

    #[test]
    fn test_transition_deal_not_found() {
        let mut mgr = DealManager::new();
        let result = mgr.transition(999, DealState::Confirmed);
        assert_eq!(result, Err(MatchError::DealNotFound));
    }

    #[test]
    fn test_record_execution_success() {
        let mut mgr = DealManager::new();
        mgr.add_deal(make_deal(1));

        let report = ExecReport {
            deal_id: 1,
            delivered_kwh: 95.0,
            deviation: -5.0,
        };
        assert!(mgr.record_execution(1, report).is_ok());
    }

    #[test]
    fn test_record_execution_volume_mismatch() {
        let mut mgr = DealManager::new();
        mgr.add_deal(make_deal(1));

        let report = ExecReport {
            deal_id: 1,
            delivered_kwh: 150.0, // 超过成交量 100.0
            deviation: 50.0,
        };
        assert_eq!(
            mgr.record_execution(1, report),
            Err(MatchError::VolumeMismatch)
        );
    }

    #[test]
    fn test_record_execution_deal_not_found() {
        let mut mgr = DealManager::new();
        let report = ExecReport {
            deal_id: 999,
            delivered_kwh: 50.0,
            deviation: 0.0,
        };
        assert_eq!(
            mgr.record_execution(999, report),
            Err(MatchError::DealNotFound)
        );
    }
}
