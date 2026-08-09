//! 交易编排器
//!
//! 管理交易全生命周期状态转换：Matched → Frozen → Confirmed → Delivering → Delivered → Settled。

use alloc::vec::Vec;

use eneros_market_types::YuanPerKwh;

use crate::types::{Settlement, TradeContext, TradePhase};
use crate::TradeError;

/// 交易编排器
///
/// 维护所有交易上下文，驱动状态机转换。
#[derive(Debug, Clone)]
pub struct TradeOrchestrator {
    /// 所有交易
    pub trades: Vec<TradeContext>,
    /// 下一个交易 ID
    pub next_id: u64,
}

impl Default for TradeOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl TradeOrchestrator {
    /// 创建空的编排器
    pub fn new() -> Self {
        Self {
            trades: Vec::new(),
            next_id: 1,
        }
    }

    /// 创建交易，返回 deal_id
    ///
    /// 初始阶段为 Matched。
    pub fn create_trade(
        &mut self,
        seller: u64,
        buyer: u64,
        volume: crate::MilliKwh,
        price: YuanPerKwh,
    ) -> u64 {
        let deal_id = self.next_id;
        self.next_id += 1;
        let ctx = TradeContext {
            deal_id,
            seller,
            buyer,
            volume_kwh: volume,
            price,
            phase: TradePhase::Matched,
            freeze_id: None,
            delivered_kwh: crate::MilliKwh::ZERO,
            settlement: None,
        };
        self.trades.push(ctx);
        deal_id
    }

    /// 冻结：Matched → Frozen
    pub fn freeze(&mut self, deal_id: u64, freeze_id: u64) -> Result<(), TradeError> {
        let ctx = self.find_mut(deal_id)?;
        if ctx.phase != TradePhase::Matched {
            return Err(TradeError::InvalidPhase);
        }
        ctx.freeze_id = Some(freeze_id);
        ctx.phase = TradePhase::Frozen;
        Ok(())
    }

    /// 确认：Frozen → Confirmed
    pub fn confirm(&mut self, deal_id: u64) -> Result<(), TradeError> {
        let ctx = self.find_mut(deal_id)?;
        if ctx.phase != TradePhase::Frozen {
            return Err(TradeError::InvalidPhase);
        }
        ctx.phase = TradePhase::Confirmed;
        Ok(())
    }

    /// 开始交付：Confirmed → Delivering
    pub fn start_delivery(&mut self, deal_id: u64) -> Result<(), TradeError> {
        let ctx = self.find_mut(deal_id)?;
        if ctx.phase != TradePhase::Confirmed {
            return Err(TradeError::InvalidPhase);
        }
        ctx.phase = TradePhase::Delivering;
        Ok(())
    }

    /// 记录交付：Delivering → Delivered
    pub fn record_delivery(
        &mut self,
        deal_id: u64,
        kwh: crate::MilliKwh,
    ) -> Result<(), TradeError> {
        let ctx = self.find_mut(deal_id)?;
        if ctx.phase != TradePhase::Delivering {
            return Err(TradeError::InvalidPhase);
        }
        ctx.delivered_kwh = kwh;
        ctx.phase = TradePhase::Delivered;
        Ok(())
    }

    /// 结算：Delivered → Settled
    pub fn settle(&mut self, deal_id: u64, settlement: Settlement) -> Result<(), TradeError> {
        let ctx = self.find_mut(deal_id)?;
        if ctx.phase != TradePhase::Delivered {
            return Err(TradeError::InvalidPhase);
        }
        ctx.settlement = Some(settlement);
        ctx.phase = TradePhase::Settled;
        Ok(())
    }

    /// 失败：任何非终态 → Failed
    pub fn fail(&mut self, deal_id: u64) -> Result<(), TradeError> {
        let ctx = self.find_mut(deal_id)?;
        if ctx.phase == TradePhase::Settled || ctx.phase == TradePhase::Failed {
            return Err(TradeError::InvalidPhase);
        }
        ctx.phase = TradePhase::Failed;
        Ok(())
    }

    /// 查找交易（不可变）
    pub fn find(&self, deal_id: u64) -> Result<&TradeContext, TradeError> {
        self.trades
            .iter()
            .find(|t| t.deal_id == deal_id)
            .ok_or(TradeError::DealNotFound)
    }

    /// 查找交易（可变）
    fn find_mut(&mut self, deal_id: u64) -> Result<&mut TradeContext, TradeError> {
        self.trades
            .iter_mut()
            .find(|t| t.deal_id == deal_id)
            .ok_or(TradeError::DealNotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_orchestrator() -> TradeOrchestrator {
        let mut orch = TradeOrchestrator::new();
        orch.create_trade(1, 2, crate::MilliKwh::from_kwh(100.0), YuanPerKwh(0.5));
        orch
    }

    #[test]
    fn happy_path() {
        let mut orch = make_orchestrator();
        let id = 1;

        assert_eq!(orch.find(id).unwrap().phase, TradePhase::Matched);

        orch.freeze(id, 99).unwrap();
        assert_eq!(orch.find(id).unwrap().phase, TradePhase::Frozen);
        assert_eq!(orch.find(id).unwrap().freeze_id, Some(99));

        orch.confirm(id).unwrap();
        assert_eq!(orch.find(id).unwrap().phase, TradePhase::Confirmed);

        orch.start_delivery(id).unwrap();
        assert_eq!(orch.find(id).unwrap().phase, TradePhase::Delivering);

        orch.record_delivery(id, crate::MilliKwh::from_kwh(100.0))
            .unwrap();
        assert_eq!(orch.find(id).unwrap().phase, TradePhase::Delivered);
        assert_eq!(
            orch.find(id).unwrap().delivered_kwh,
            crate::MilliKwh::from_kwh(100.0)
        );

        let st = Settlement {
            deal_id: id,
            seller_credit_kwh: crate::MilliKwh::from_kwh(100.0),
            buyer_debit_kwh: crate::MilliKwh::from_kwh(100.0),
            amount_yuan: crate::types::MilliYuan(50_000),
            at: 2000,
        };
        orch.settle(id, st).unwrap();
        assert_eq!(orch.find(id).unwrap().phase, TradePhase::Settled);
        assert!(orch.find(id).unwrap().settlement.is_some());
    }

    #[test]
    fn invalid_transitions() {
        let mut orch = make_orchestrator();
        // Cannot confirm before freeze
        assert_eq!(orch.confirm(1), Err(TradeError::InvalidPhase));
        // Cannot start_delivery before confirm
        assert_eq!(orch.start_delivery(1), Err(TradeError::InvalidPhase));
        // Cannot settle before delivered
        let st = Settlement {
            deal_id: 1,
            seller_credit_kwh: crate::MilliKwh::ZERO,
            buyer_debit_kwh: crate::MilliKwh::ZERO,
            amount_yuan: crate::types::MilliYuan::ZERO,
            at: 0,
        };
        assert_eq!(orch.settle(1, st), Err(TradeError::InvalidPhase));
    }

    #[test]
    fn deal_not_found() {
        let mut orch = TradeOrchestrator::new();
        assert_eq!(orch.freeze(999, 1), Err(TradeError::DealNotFound));
    }

    #[test]
    fn fail_from_any_non_terminal() {
        // From Matched
        let mut orch = make_orchestrator();
        orch.fail(1).unwrap();
        assert_eq!(orch.find(1).unwrap().phase, TradePhase::Failed);

        // From Frozen
        let mut orch = make_orchestrator();
        orch.freeze(1, 10).unwrap();
        orch.fail(1).unwrap();
        assert_eq!(orch.find(1).unwrap().phase, TradePhase::Failed);

        // From Confirmed
        let mut orch = make_orchestrator();
        orch.freeze(1, 10).unwrap();
        orch.confirm(1).unwrap();
        orch.fail(1).unwrap();
        assert_eq!(orch.find(1).unwrap().phase, TradePhase::Failed);

        // From Delivering
        let mut orch = make_orchestrator();
        orch.freeze(1, 10).unwrap();
        orch.confirm(1).unwrap();
        orch.start_delivery(1).unwrap();
        orch.fail(1).unwrap();
        assert_eq!(orch.find(1).unwrap().phase, TradePhase::Failed);

        // From Delivered
        let mut orch = make_orchestrator();
        orch.freeze(1, 10).unwrap();
        orch.confirm(1).unwrap();
        orch.start_delivery(1).unwrap();
        orch.record_delivery(1, crate::MilliKwh::from_kwh(50.0))
            .unwrap();
        orch.fail(1).unwrap();
        assert_eq!(orch.find(1).unwrap().phase, TradePhase::Failed);
    }

    #[test]
    fn fail_from_terminal_rejected() {
        // From Settled
        let mut orch = make_orchestrator();
        orch.freeze(1, 10).unwrap();
        orch.confirm(1).unwrap();
        orch.start_delivery(1).unwrap();
        orch.record_delivery(1, crate::MilliKwh::from_kwh(100.0))
            .unwrap();
        let st = Settlement {
            deal_id: 1,
            seller_credit_kwh: crate::MilliKwh::from_kwh(100.0),
            buyer_debit_kwh: crate::MilliKwh::from_kwh(100.0),
            amount_yuan: crate::types::MilliYuan(50_000),
            at: 1000,
        };
        orch.settle(1, st).unwrap();
        assert_eq!(orch.fail(1), Err(TradeError::InvalidPhase));

        // From Failed
        let mut orch = make_orchestrator();
        orch.fail(1).unwrap();
        assert_eq!(orch.fail(1), Err(TradeError::InvalidPhase));
    }
}
