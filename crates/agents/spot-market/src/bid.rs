//! 报价构建与校验

use alloc::vec::Vec;

use eneros_market_types::YuanPerKwh;

use crate::types::{Bid, BidSegment, SpotMarketType, TimeSlot, TradeDir};
use crate::MarketError;

/// 校验报价单合法性
///
/// 规则：
/// - segments 不能为空
/// - 每段 volume_kwh > 0
/// - 每段 price >= 0（YuanPerKwh）
/// - 每段 period 有效 (end > start)
pub fn validate_bid(bid: &Bid) -> Result<(), MarketError> {
    if bid.segments.is_empty() {
        return Err(MarketError::InvalidBid);
    }
    for seg in &bid.segments {
        if seg.volume_kwh <= 0.0 {
            return Err(MarketError::InvalidBid);
        }
        if seg.price.0 < 0.0 {
            return Err(MarketError::InvalidBid);
        }
        if !seg.period.is_valid() {
            return Err(MarketError::InvalidBid);
        }
    }
    Ok(())
}

/// 报价构建器
#[derive(Debug)]
pub struct BidBuilder {
    bid_id: u64,
    market: SpotMarketType,
    submitter: u64,
    segments: Vec<BidSegment>,
    submit_at: u64,
}

impl BidBuilder {
    /// 创建构建器
    pub fn new(bid_id: u64, market: SpotMarketType, submitter: u64) -> Self {
        Self {
            bid_id,
            market,
            submitter,
            segments: Vec::new(),
            submit_at: 0,
        }
    }

    /// 设置提交时间
    pub fn submit_at(mut self, ts: u64) -> Self {
        self.submit_at = ts;
        self
    }

    /// 添加报价段
    pub fn add_segment(
        mut self,
        period: TimeSlot,
        volume_kwh: f64,
        price: YuanPerKwh,
        direction: TradeDir,
    ) -> Self {
        self.segments.push(BidSegment {
            period,
            volume_kwh,
            price,
            direction,
        });
        self
    }

    /// 构建报价单（自动校验）
    pub fn build(self) -> Result<Bid, MarketError> {
        let bid = Bid {
            bid_id: self.bid_id,
            market: self.market.to_market_type(),
            submitter: self.submitter,
            segments: self.segments,
            submit_at: self.submit_at,
        };
        validate_bid(&bid)?;
        Ok(bid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_market_types::MarketType;

    fn make_valid_bid() -> Bid {
        BidBuilder::new(1, SpotMarketType::DayAhead, 100)
            .submit_at(1000)
            .add_segment(
                TimeSlot::new(0, 3600),
                100.0,
                YuanPerKwh(0.45),
                TradeDir::Sell,
            )
            .build()
            .unwrap()
    }

    #[test]
    fn test_valid_bid() {
        let bid = make_valid_bid();
        assert!(validate_bid(&bid).is_ok());
        assert_eq!(bid.bid_id, 1);
        assert_eq!(bid.segments.len(), 1);
    }

    #[test]
    fn test_empty_segments() {
        let bid = Bid {
            bid_id: 2,
            market: MarketType::IntraDay,
            submitter: 100,
            segments: Vec::new(),
            submit_at: 1000,
        };
        assert_eq!(validate_bid(&bid), Err(MarketError::InvalidBid));
    }

    #[test]
    fn test_negative_volume() {
        let bid = Bid {
            bid_id: 3,
            market: MarketType::DayAhead,
            submitter: 100,
            segments: alloc::vec![BidSegment {
                period: TimeSlot::new(0, 3600),
                volume_kwh: -10.0,
                price: YuanPerKwh(0.5),
                direction: TradeDir::Buy,
            }],
            submit_at: 1000,
        };
        assert_eq!(validate_bid(&bid), Err(MarketError::InvalidBid));
    }

    #[test]
    fn test_zero_volume() {
        let bid = Bid {
            bid_id: 4,
            market: MarketType::DayAhead,
            submitter: 100,
            segments: alloc::vec![BidSegment {
                period: TimeSlot::new(0, 3600),
                volume_kwh: 0.0,
                price: YuanPerKwh(0.5),
                direction: TradeDir::Buy,
            }],
            submit_at: 1000,
        };
        assert_eq!(validate_bid(&bid), Err(MarketError::InvalidBid));
    }

    #[test]
    fn test_negative_price() {
        let bid = Bid {
            bid_id: 5,
            market: MarketType::DayAhead,
            submitter: 100,
            segments: alloc::vec![BidSegment {
                period: TimeSlot::new(0, 3600),
                volume_kwh: 100.0,
                price: YuanPerKwh(-0.1),
                direction: TradeDir::Sell,
            }],
            submit_at: 1000,
        };
        assert_eq!(validate_bid(&bid), Err(MarketError::InvalidBid));
    }

    #[test]
    fn test_invalid_period() {
        let bid = Bid {
            bid_id: 6,
            market: MarketType::DayAhead,
            submitter: 100,
            segments: alloc::vec![BidSegment {
                period: TimeSlot::new(3600, 0),
                volume_kwh: 100.0,
                price: YuanPerKwh(0.5),
                direction: TradeDir::Sell,
            }],
            submit_at: 1000,
        };
        assert_eq!(validate_bid(&bid), Err(MarketError::InvalidBid));
    }

    #[test]
    fn test_builder_rejects_invalid() {
        let result = BidBuilder::new(7, SpotMarketType::DayAhead, 100)
            .submit_at(1000)
            .build();
        assert_eq!(result, Err(MarketError::InvalidBid));
    }

    #[test]
    fn test_builder_multiple_segments() {
        let bid = BidBuilder::new(8, SpotMarketType::IntraDay, 200)
            .submit_at(2000)
            .add_segment(TimeSlot::new(0, 3600), 50.0, YuanPerKwh(0.4), TradeDir::Buy)
            .add_segment(
                TimeSlot::new(3600, 7200),
                80.0,
                YuanPerKwh(0.5),
                TradeDir::Sell,
            )
            .build()
            .unwrap();
        assert_eq!(bid.segments.len(), 2);
        assert_eq!(bid.market, MarketType::IntraDay);
    }
}
