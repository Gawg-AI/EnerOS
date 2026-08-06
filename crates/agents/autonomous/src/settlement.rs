//! 结算触发器
//!
//! 定义结算 trait 及自动结算实现。

use crate::types::{MilliYuan, Settlement, TradeContext};
use crate::TradeError;

/// 结算触发器 trait
///
/// 根据交易上下文计算并生成结算信息。
pub trait SettlementTrigger {
    /// 执行结算，返回 Settlement
    fn settle(&mut self, ctx: &TradeContext) -> Result<Settlement, TradeError>;
}

/// 自动结算
///
/// 根据已交付电量与单价自动计算结算金额。
#[derive(Debug, Clone, Default)]
pub struct AutoSettlement {
    /// 结算时间戳
    pub timestamp: u64,
}

impl AutoSettlement {
    /// 创建自动结算器
    pub fn new(timestamp: u64) -> Self {
        Self { timestamp }
    }
}

impl SettlementTrigger for AutoSettlement {
    fn settle(&mut self, ctx: &TradeContext) -> Result<Settlement, TradeError> {
        if ctx.delivered_kwh <= 0.0 {
            return Err(TradeError::SettlementFailed);
        }
        // ARC-003：金额定点（delivered × price 精确积 round-half-up 定毫）
        let amount = MilliYuan::from_yuan_f64(ctx.delivered_kwh * ctx.price);
        Ok(Settlement {
            deal_id: ctx.deal_id,
            seller_credit_kwh: ctx.delivered_kwh,
            buyer_debit_kwh: ctx.delivered_kwh,
            amount_yuan: amount,
            at: self.timestamp,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TradePhase;

    fn make_ctx(delivered: f64, price: f64) -> TradeContext {
        TradeContext {
            deal_id: 1,
            seller: 10,
            buyer: 20,
            volume_kwh: 200.0,
            price,
            phase: TradePhase::Delivered,
            freeze_id: Some(1),
            delivered_kwh: delivered,
            settlement: None,
        }
    }

    #[test]
    fn auto_settlement_calculates() {
        let mut s = AutoSettlement::new(5000);
        let ctx = make_ctx(150.0, 0.6);
        let result = s.settle(&ctx).unwrap();
        assert_eq!(result.deal_id, 1);
        assert_eq!(result.seller_credit_kwh, 150.0);
        assert_eq!(result.buyer_debit_kwh, 150.0);
        assert_eq!(result.amount_yuan, MilliYuan(90_000), "150 × 0.6 = 90 元 = 90000 毫元");
        assert_eq!(result.at, 5000);
    }

    #[test]
    fn auto_settlement_zero_delivery_fails() {
        let mut s = AutoSettlement::new(1000);
        let ctx = make_ctx(0.0, 0.5);
        assert_eq!(s.settle(&ctx), Err(TradeError::SettlementFailed));
    }

    #[test]
    fn auto_settlement_negative_delivery_fails() {
        let mut s = AutoSettlement::new(1000);
        let ctx = make_ctx(-10.0, 0.5);
        assert_eq!(s.settle(&ctx), Err(TradeError::SettlementFailed));
    }
}
