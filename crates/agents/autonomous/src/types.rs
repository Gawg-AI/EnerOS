//! 自主交易核心类型定义
//!
//! 包含交易阶段、结算信息和交易上下文。

pub use eneros_market_types::{YuanPerKwh, MilliYuan};

/// 交易阶段
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradePhase {
    /// 已撮合
    Matched,
    /// 已冻结
    Frozen,
    /// 已确认
    Confirmed,
    /// 交付中
    Delivering,
    /// 已交付
    Delivered,
    /// 已结算
    Settled,
    /// 失败
    Failed,
}

impl core::fmt::Display for TradePhase {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = match self {
            Self::Matched => "Matched",
            Self::Frozen => "Frozen",
            Self::Confirmed => "Confirmed",
            Self::Delivering => "Delivering",
            Self::Delivered => "Delivered",
            Self::Settled => "Settled",
            Self::Failed => "Failed",
        };
        write!(f, "{s}")
    }
}

/// 结算信息
#[derive(Debug, Clone, PartialEq)]
pub struct Settlement {
    /// 交易 ID
    pub deal_id: u64,
    /// 卖方入账电量 (kWh)
    pub seller_credit_kwh: crate::MilliKwh,
    /// 买方扣减电量 (kWh)
    pub buyer_debit_kwh: crate::MilliKwh,
    /// 结算金额（毫元定点，ARC-003）
    pub amount_yuan: MilliYuan,
    /// 结算时间戳
    pub at: u64,
}

/// 交易上下文
#[derive(Debug, Clone, PartialEq)]
pub struct TradeContext {
    /// 交易 ID
    pub deal_id: u64,
    /// 卖方 Agent ID
    pub seller: u64,
    /// 买方 Agent ID
    pub buyer: u64,
    /// 交易电量（毫 kWh 定点，v0.230.0）
    pub volume_kwh: crate::MilliKwh,
    /// 单价（元/kWh）——蓝图/20 R3-2：金额类型定点化（禁裸 f64）。
    pub price: YuanPerKwh,
    /// 当前阶段
    pub phase: TradePhase,
    /// 冻结 ID
    pub freeze_id: Option<u64>,
    /// 已交付电量（毫 kWh 定点，v0.230.0）
    pub delivered_kwh: crate::MilliKwh,
    /// 结算信息
    pub settlement: Option<Settlement>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trade_phase_display() {
        assert_eq!(TradePhase::Matched.to_string(), "Matched");
        assert_eq!(TradePhase::Frozen.to_string(), "Frozen");
        assert_eq!(TradePhase::Confirmed.to_string(), "Confirmed");
        assert_eq!(TradePhase::Delivering.to_string(), "Delivering");
        assert_eq!(TradePhase::Delivered.to_string(), "Delivered");
        assert_eq!(TradePhase::Settled.to_string(), "Settled");
        assert_eq!(TradePhase::Failed.to_string(), "Failed");
    }

    #[test]
    fn settlement_struct() {
        let s = Settlement {
            deal_id: 1,
            seller_credit_kwh: crate::MilliKwh(100_000),
            buyer_debit_kwh: crate::MilliKwh(100_000),
            amount_yuan: MilliYuan(50_000),
            at: 1000,
        };
        assert_eq!(s.deal_id, 1);
        assert_eq!(s.seller_credit_kwh, crate::MilliKwh(100_000));
        assert_eq!(s.amount_yuan, MilliYuan(50_000));
    }

    #[test]
    fn trade_context_initial() {
        let ctx = TradeContext {
            deal_id: 1,
            seller: 10,
            buyer: 20,
            volume_kwh: crate::MilliKwh::from_kwh(500.0),
            price: YuanPerKwh(0.5),
            phase: TradePhase::Matched,
            freeze_id: None,
            delivered_kwh: crate::MilliKwh::ZERO,
            settlement: None,
        };
        assert_eq!(ctx.phase, TradePhase::Matched);
        assert!(ctx.freeze_id.is_none());
        assert!(ctx.settlement.is_none());
        assert_eq!(ctx.delivered_kwh, crate::MilliKwh::ZERO);
    }
}
