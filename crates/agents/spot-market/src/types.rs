//! 现货市场核心类型定义

use alloc::vec::Vec;

/// 现货市场类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotMarketType {
    /// 日前市场
    DayAhead,
    /// 日内市场
    IntraDay,
}

/// 交易方向
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeDir {
    /// 买入
    Buy,
    /// 卖出
    Sell,
}

/// 时间段 (UTC 秒)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSlot {
    /// 起始时间 (UTC 秒)
    pub start: u64,
    /// 结束时间 (UTC 秒)
    pub end: u64,
}

impl TimeSlot {
    /// 创建时间段
    pub const fn new(start: u64, end: u64) -> Self {
        Self { start, end }
    }

    /// 检查时间段是否有效 (end > start)
    pub const fn is_valid(&self) -> bool {
        self.end > self.start
    }

    /// 获取持续秒数
    pub const fn duration_secs(&self) -> u64 {
        self.end - self.start
    }
}

/// 报价段
#[derive(Debug, Clone, PartialEq)]
pub struct BidSegment {
    /// 时段
    pub period: TimeSlot,
    /// 电量 (kWh)
    pub volume_kwh: f64,
    /// 价格 (元/kWh)
    pub price_yuan_per_kwh: f64,
    /// 交易方向
    pub direction: TradeDir,
}

/// 报价单
#[derive(Debug, Clone, PartialEq)]
pub struct Bid {
    /// 报价 ID
    pub bid_id: u64,
    /// 市场类型
    pub market: SpotMarketType,
    /// 提交者 ID
    pub submitter: u64,
    /// 报价段列表
    pub segments: Vec<BidSegment>,
    /// 提交时间 (UTC 秒)
    pub submit_at: u64,
}

/// 报价回执
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BidAck {
    /// 报价 ID
    pub bid_id: u64,
    /// 回执时间 (UTC 秒)
    pub ack_at: u64,
    /// 是否被接受
    pub accepted: bool,
}

/// 出清段
#[derive(Debug, Clone, PartialEq)]
pub struct ClearedSegment {
    /// 时段
    pub period: TimeSlot,
    /// 出清电量 (kWh)
    pub cleared_volume: f64,
    /// 出清价格 (元/kWh)
    pub cleared_price: f64,
}

/// 出清结果
#[derive(Debug, Clone, PartialEq)]
pub struct ClearingResult {
    /// 报价 ID
    pub bid_id: u64,
    /// 出清段列表
    pub cleared: Vec<ClearedSegment>,
    /// 统一出清价格 (元/kWh)
    pub clearing_price: f64,
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
            price_yuan_per_kwh: 0.45,
            direction: TradeDir::Sell,
        };
        assert_eq!(seg.volume_kwh, 100.0);
        assert_eq!(seg.direction, TradeDir::Sell);
    }

    #[test]
    fn test_bid_creation() {
        let bid = Bid {
            bid_id: 1,
            market: SpotMarketType::DayAhead,
            submitter: 42,
            segments: alloc::vec![BidSegment {
                period: TimeSlot::new(0, 3600),
                volume_kwh: 200.0,
                price_yuan_per_kwh: 0.5,
                direction: TradeDir::Buy,
            }],
            submit_at: 1000,
        };
        assert_eq!(bid.segments.len(), 1);
        assert_eq!(bid.market, SpotMarketType::DayAhead);
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
                cleared_price: 0.48,
            }],
            clearing_price: 0.48,
            clearing_at: 2000,
        };
        assert_eq!(result.cleared.len(), 1);
        assert_eq!(result.clearing_price, 0.48);
    }
}
