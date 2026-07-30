//! 出清结果处理

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::types::{ClearingResult, ExecStatus};
use crate::MarketError;

/// 出清处理器
///
/// 管理报价的出清结果，并根据出清结果推进执行状态。
#[derive(Debug, Default)]
pub struct ClearingProcessor {
    /// bid_id -> ClearingResult
    results: BTreeMap<u64, ClearingResult>,
    /// bid_id -> ExecStatus
    states: BTreeMap<u64, ExecStatus>,
}

impl ClearingProcessor {
    /// 创建空的出清处理器
    pub fn new() -> Self {
        Self {
            results: BTreeMap::new(),
            states: BTreeMap::new(),
        }
    }

    /// 注册一个已提交的报价
    pub fn register_bid(&mut self, bid_id: u64) {
        self.states.insert(bid_id, ExecStatus::Submitted);
    }

    /// 应用出清结果
    ///
    /// 将报价状态从 Submitted 推进到 Cleared。
    pub fn apply_clearing(&mut self, result: &ClearingResult) -> Result<ExecStatus, MarketError> {
        let state = self.states.get(&result.bid_id).copied();
        match state {
            Some(ExecStatus::Submitted) => {
                self.results.insert(result.bid_id, result.clone());
                self.states.insert(result.bid_id, ExecStatus::Cleared);
                Ok(ExecStatus::Cleared)
            }
            Some(_) => Err(MarketError::InvalidStateTransition),
            None => Err(MarketError::ClearingNotFound),
        }
    }

    /// 获取报价当前状态
    pub fn get_status(&self, bid_id: u64) -> Option<ExecStatus> {
        self.states.get(&bid_id).copied()
    }

    /// 获取出清结果
    pub fn get_clearing(&self, bid_id: u64) -> Option<&ClearingResult> {
        self.results.get(&bid_id)
    }

    /// 获取所有已出清的 bid_id 列表
    pub fn cleared_bids(&self) -> Vec<u64> {
        self.states
            .iter()
            .filter(|(_, s)| **s == ExecStatus::Cleared)
            .map(|(id, _)| *id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ClearedSegment, TimeSlot};

    fn make_clearing(bid_id: u64) -> ClearingResult {
        ClearingResult {
            bid_id,
            cleared: alloc::vec![ClearedSegment {
                period: TimeSlot::new(0, 3600),
                cleared_volume: 100.0,
                cleared_price: 0.45,
            }],
            clearing_price: 0.45,
            clearing_at: 5000,
        }
    }

    #[test]
    fn test_apply_clearing_success() {
        let mut proc = ClearingProcessor::new();
        proc.register_bid(1);
        let result = make_clearing(1);
        let status = proc.apply_clearing(&result).unwrap();
        assert_eq!(status, ExecStatus::Cleared);
        assert_eq!(proc.get_status(1), Some(ExecStatus::Cleared));
    }

    #[test]
    fn test_apply_clearing_unknown_bid() {
        let mut proc = ClearingProcessor::new();
        let result = make_clearing(99);
        assert_eq!(
            proc.apply_clearing(&result),
            Err(MarketError::ClearingNotFound)
        );
    }

    #[test]
    fn test_apply_clearing_wrong_state() {
        let mut proc = ClearingProcessor::new();
        proc.register_bid(1);
        let result = make_clearing(1);
        proc.apply_clearing(&result).unwrap();
        // 再次 apply 应该失败（已经是 Cleared 状态）
        assert_eq!(
            proc.apply_clearing(&result),
            Err(MarketError::InvalidStateTransition)
        );
    }

    #[test]
    fn test_get_clearing_result() {
        let mut proc = ClearingProcessor::new();
        proc.register_bid(1);
        let result = make_clearing(1);
        proc.apply_clearing(&result).unwrap();
        let stored = proc.get_clearing(1).unwrap();
        assert_eq!(stored.clearing_price, 0.45);
    }

    #[test]
    fn test_cleared_bids() {
        let mut proc = ClearingProcessor::new();
        proc.register_bid(1);
        proc.register_bid(2);
        proc.apply_clearing(&make_clearing(1)).unwrap();
        let cleared = proc.cleared_bids();
        assert_eq!(cleared, alloc::vec![1]);
    }
}
