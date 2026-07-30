//! 市场参与者抽象

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::bid::validate_bid;
use crate::tracker::ExecTracker;
use crate::types::{Bid, BidAck, ClearingResult, ExecStatus};
use crate::MarketError;

/// 现货市场参与者 trait
///
/// 定义报价提交、出清获取、执行跟踪的标准接口。
pub trait SpotMarketParticipant {
    /// 提交报价
    fn submit_bid(&mut self, bid: &Bid) -> Result<BidAck, MarketError>;

    /// 获取出清结果
    fn fetch_clearing(&self, bid_id: u64) -> Result<ClearingResult, MarketError>;

    /// 跟踪执行状态
    fn track_execution(&self, bid_id: u64) -> Result<ExecStatus, MarketError>;
}

/// 模拟参与者（本地状态，用于测试）
#[derive(Debug, Default)]
pub struct MockParticipant {
    /// bid_id -> Bid
    bids: BTreeMap<u64, Bid>,
    /// bid_id -> BidAck
    acks: BTreeMap<u64, BidAck>,
    /// bid_id -> ClearingResult
    clearings: BTreeMap<u64, ClearingResult>,
    /// bid_id -> ExecTracker
    trackers: BTreeMap<u64, ExecTracker>,
    /// 模拟时钟
    clock: u64,
}

impl MockParticipant {
    /// 创建模拟参与者
    pub fn new() -> Self {
        Self {
            bids: BTreeMap::new(),
            acks: BTreeMap::new(),
            clearings: BTreeMap::new(),
            trackers: BTreeMap::new(),
            clock: 1000,
        }
    }

    /// 注入出清结果（模拟市场运营方）
    pub fn inject_clearing(&mut self, result: ClearingResult) {
        let bid_id = result.bid_id;
        self.clearings.insert(bid_id, result);
        if let Some(tracker) = self.trackers.get_mut(&bid_id) {
            let _ = tracker.transition(ExecStatus::Cleared);
        }
    }

    /// 推进时钟
    pub fn tick(&mut self, secs: u64) {
        self.clock += secs;
    }

    /// 获取跟踪器引用
    pub fn get_tracker(&self, bid_id: u64) -> Option<&ExecTracker> {
        self.trackers.get(&bid_id)
    }

    /// 获取跟踪器可变引用
    pub fn get_tracker_mut(&mut self, bid_id: u64) -> Option<&mut ExecTracker> {
        self.trackers.get_mut(&bid_id)
    }

    /// 获取所有已提交的 bid_id
    pub fn submitted_bids(&self) -> Vec<u64> {
        self.bids.keys().copied().collect()
    }
}

impl SpotMarketParticipant for MockParticipant {
    fn submit_bid(&mut self, bid: &Bid) -> Result<BidAck, MarketError> {
        validate_bid(bid)?;
        self.clock += 1;
        let ack = BidAck {
            bid_id: bid.bid_id,
            ack_at: self.clock,
            accepted: true,
        };
        let planned: f64 = bid.segments.iter().map(|s| s.volume_kwh).sum();
        self.bids.insert(bid.bid_id, bid.clone());
        self.acks.insert(bid.bid_id, ack);
        self.trackers
            .insert(bid.bid_id, ExecTracker::new(bid.bid_id, planned));
        Ok(ack)
    }

    fn fetch_clearing(&self, bid_id: u64) -> Result<ClearingResult, MarketError> {
        self.clearings
            .get(&bid_id)
            .cloned()
            .ok_or(MarketError::ClearingNotFound)
    }

    fn track_execution(&self, bid_id: u64) -> Result<ExecStatus, MarketError> {
        self.trackers
            .get(&bid_id)
            .map(|t| t.state)
            .ok_or(MarketError::ClearingNotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bid::BidBuilder;
    use crate::types::{ClearedSegment, SpotMarketType, TimeSlot, TradeDir};

    fn make_bid(id: u64) -> Bid {
        BidBuilder::new(id, SpotMarketType::DayAhead, 1)
            .submit_at(1000)
            .add_segment(TimeSlot::new(0, 3600), 100.0, 0.5, TradeDir::Sell)
            .build()
            .unwrap()
    }

    #[test]
    fn test_submit_and_ack() {
        let mut p = MockParticipant::new();
        let bid = make_bid(1);
        let ack = p.submit_bid(&bid).unwrap();
        assert!(ack.accepted);
        assert_eq!(ack.bid_id, 1);
    }

    #[test]
    fn test_submit_invalid_bid() {
        let mut p = MockParticipant::new();
        let bid = Bid {
            bid_id: 2,
            market: SpotMarketType::DayAhead,
            submitter: 1,
            segments: Vec::new(),
            submit_at: 1000,
        };
        assert_eq!(p.submit_bid(&bid), Err(MarketError::InvalidBid));
    }

    #[test]
    fn test_fetch_clearing_not_found() {
        let p = MockParticipant::new();
        assert_eq!(p.fetch_clearing(99), Err(MarketError::ClearingNotFound));
    }

    #[test]
    fn test_inject_clearing_and_fetch() {
        let mut p = MockParticipant::new();
        let bid = make_bid(1);
        p.submit_bid(&bid).unwrap();

        let clearing = ClearingResult {
            bid_id: 1,
            cleared: alloc::vec![ClearedSegment {
                period: TimeSlot::new(0, 3600),
                cleared_volume: 80.0,
                cleared_price: 0.48,
            }],
            clearing_price: 0.48,
            clearing_at: 2000,
        };
        p.inject_clearing(clearing);

        let fetched = p.fetch_clearing(1).unwrap();
        assert_eq!(fetched.clearing_price, 0.48);
    }

    #[test]
    fn test_track_execution_lifecycle() {
        let mut p = MockParticipant::new();
        let bid = make_bid(1);
        p.submit_bid(&bid).unwrap();

        assert_eq!(p.track_execution(1).unwrap(), ExecStatus::Submitted);

        let clearing = ClearingResult {
            bid_id: 1,
            cleared: alloc::vec![ClearedSegment {
                period: TimeSlot::new(0, 3600),
                cleared_volume: 100.0,
                cleared_price: 0.5,
            }],
            clearing_price: 0.5,
            clearing_at: 2000,
        };
        p.inject_clearing(clearing);
        assert_eq!(p.track_execution(1).unwrap(), ExecStatus::Cleared);
    }

    #[test]
    fn test_submitted_bids() {
        let mut p = MockParticipant::new();
        p.submit_bid(&make_bid(1)).unwrap();
        p.submit_bid(&make_bid(2)).unwrap();
        assert_eq!(p.submitted_bids(), alloc::vec![1, 2]);
    }
}
