//! 现货市场核心类型定义（ARCH-M01/M02 / ADR-0029 收敛）。
//!
//! `TradeDir`/`TimeSlot`/`Bid`/`BidSegment`/`BidAck` 与价格单位 newtype
//! 的唯一来源为 `eneros-market-types`，此处 re-export；本模块只保留现货市场
//! 专属类型（[`SpotMarketType`]、出清结果）。价格字段一律为 [`YuanPerKwh`]
//! 类型，禁止裸 `f64` 价格传递（ARCH-M02）。

use alloc::vec::Vec;

pub use eneros_market_types::{
    Bid, BidAck, BidSegment, MarketType, TimeSlot, TradeDir, YuanPerKwh,
};

/// 现货市场类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotMarketType {
    /// 日前市场
    DayAhead,
    /// 日内市场
    IntraDay,
}

impl SpotMarketType {
    /// 转为共享市场类型（ADR-0029）。
    pub const fn to_market_type(self) -> MarketType {
        match self {
            SpotMarketType::DayAhead => MarketType::DayAhead,
            SpotMarketType::IntraDay => MarketType::IntraDay,
        }
    }
}

/// 出清段
#[derive(Debug, Clone, PartialEq)]
pub struct ClearedSegment {
    /// 时段
    pub period: TimeSlot,
    /// 出清电量 (kWh)
    pub cleared_volume: f64,
    /// 出清价格 (元/kWh，单位化)
    pub cleared_price: YuanPerKwh,
}

/// 出清结果
#[derive(Debug, Clone, PartialEq)]
pub struct ClearingResult {
    /// 报价 ID
    pub bid_id: u64,
    /// 出清段列表
    pub cleared: Vec<ClearedSegment>,
    /// 统一出清价格 (元/kWh，单位化)
    pub clearing_price: YuanPerKwh,
    /// 出清时间 (UTC 秒)
    pub clearing_at: u64,
}

/// 执行状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecStatus {
    /// 已提交
    Submitted,
    /// 已出清
    Cleared,
    /// 调度中
    Dispatching,
    /// 已结算
    Settled,
    /// 偏差
    Deviated,
    /// 失败
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_time_slot_valid() {
        let slot = TimeSlot::new(1000, 2000);
        assert!(slot.is_valid());
        assert_eq!(slot.duration_secs(), 1000);
    }

    #[test]
    fn test_time_slot_invalid() {
        let slot = TimeSlot::new(2000, 1000);
        assert!(!slot.is_valid());

        let slot2 = TimeSlot::new(1000, 1000);
        assert!(!slot2.is_valid());
    }

    #[test]
    fn test_bid_segment_creation() {
        let seg = BidSegment {
            period: TimeSlot::new(0, 3600),
            volume_kwh: 100.0,
            price: YuanPerKwh(0.45),
            direction: TradeDir::Sell,
        };
        assert_eq!(seg.volume_kwh, 100.0);
        assert_eq!(seg.direction, TradeDir::Sell);
    }

    #[test]
    fn test_bid_creation() {
        let bid = Bid {
            bid_id: 1,
            market: MarketType::DayAhead,
            submitter: 42,
            segments: alloc::vec![BidSegment {
                period: TimeSlot::new(0, 3600),
                volume_kwh: 200.0,
                price: YuanPerKwh(0.5),
                direction: TradeDir::Buy,
            }],
            submit_at: 1000,
        };
        assert_eq!(bid.segments.len(), 1);
        assert_eq!(bid.market, MarketType::DayAhead);
    }

    #[test]
    fn test_exec_status_eq() {
        assert_eq!(ExecStatus::Submitted, ExecStatus::Submitted);
        assert_ne!(ExecStatus::Cleared, ExecStatus::Settled);
    }

    #[test]
    fn test_clearing_result() {
        let result = ClearingResult {
            bid_id: 1,
            cleared: alloc::vec![ClearedSegment {
                period: TimeSlot::new(0, 3600),
                cleared_volume: 150.0,
                cleared_price: YuanPerKwh(0.48),
            }],
            clearing_price: YuanPerKwh(0.48),
            clearing_at: 2000,
        };
        assert_eq!(result.cleared.len(), 1);
        assert_eq!(result.clearing_price, YuanPerKwh(0.48));
    }
}
