//! 闭环驱动
//!
//! 自动推进交易至最终状态（Settled 或 Failed）。

use crate::orchestrator::TradeOrchestrator;
use crate::settlement::{AutoSettlement, SettlementTrigger};
use eneros_market_types::YuanPerKwh;
use crate::types::TradePhase;
use crate::TradeError;

/// 闭环驱动 trait
///
/// 每次调用将交易推进到下一阶段。
pub trait ClosedLoop {
    /// 推进交易到下一阶段，返回新阶段
    fn advance(&mut self, deal_id: u64) -> Result<TradePhase, TradeError>;
}

/// 闭环驱动实现
///
/// 封装 TradeOrchestrator，自动推进交易状态。
pub struct ClosedLoopImpl {
    /// 交易编排器
    pub orchestrator: TradeOrchestrator,
    /// 自动结算器
    pub settlement: AutoSettlement,
    /// 冻结 ID 计数器
    next_freeze_id: u64,
    /// 真实计量闭包（v0.230.0：非全额假设；deal_id → 实际交付量）。
    meter_fn: fn(u64) -> Result<crate::MilliKwh, TradeError>,
    /// 账户管理器（冻结真实调用；None = 跳过）。
    account: Option<alloc::boxed::Box<dyn eneros_account::AccountManager>>,
}

impl ClosedLoopImpl {
    /// 创建闭环驱动（默认计量 = 全额交付回退，账户 = None）。
    pub fn new(orchestrator: TradeOrchestrator, settlement: AutoSettlement) -> Self {
        Self {
            orchestrator,
            settlement,
            next_freeze_id: 1,
            meter_fn: |_deal_id| {
                // 默认回退：全额交付（调用方应注入真实计量）。
                Err(TradeError::MeterUnavailable)
            },
            account: None,
        }
    }

    /// 注入真实计量闭包（v0.230.0）。
    pub fn with_meter(mut self, meter_fn: fn(u64) -> Result<crate::MilliKwh, TradeError>) -> Self {
        self.meter_fn = meter_fn;
        self
    }

    /// 注入账户管理器（冻结经真实接口，幂等）。
    pub fn with_account(mut self, account: alloc::boxed::Box<dyn eneros_account::AccountManager>) -> Self {
        self.account = Some(account);
        self
    }

    /// 运行至完成：推进直到 Settled 或 Failed
    pub fn run_to_completion(&mut self, deal_id: u64) -> Result<TradePhase, TradeError> {
        loop {
            let phase = self.advance(deal_id)?;
            if phase == TradePhase::Settled || phase == TradePhase::Failed {
                return Ok(phase);
            }
        }
    }
}

impl ClosedLoop for ClosedLoopImpl {
    fn advance(&mut self, deal_id: u64) -> Result<TradePhase, TradeError> {
        let current = self.orchestrator.find(deal_id)?.phase;
        match current {
            TradePhase::Matched => {
                let fid = self.next_freeze_id;
                self.next_freeze_id += 1;
                // v0.230.0：真实冻结（幂等——账户已冻结同额则跳过）。
                if let Some(account) = &mut self.account {
                    let ctx = self.orchestrator.find(deal_id)?;
                    let seller = ctx.seller;
                    let amount =
                        eneros_market_types::MilliYuan::from_yuan_f64(ctx.volume_kwh.as_kwh() * ctx.price.0);
                    let _ = account.freeze(seller, amount); // 失败不阻断状态机（由对账暴露）
                }
                self.orchestrator.freeze(deal_id, fid)?;
                Ok(TradePhase::Frozen)
            }
            TradePhase::Frozen => {
                self.orchestrator.confirm(deal_id)?;
                Ok(TradePhase::Confirmed)
            }
            TradePhase::Confirmed => {
                self.orchestrator.start_delivery(deal_id)?;
                Ok(TradePhase::Delivering)
            }
            TradePhase::Delivering => {
                // v0.230.0：真实计量输入（非全额假设）——经注入的计量闭包
                // 读取实际交付量。
                let measured = (self.meter_fn)(deal_id)?;
                self.orchestrator.record_delivery(deal_id, measured)?;
                Ok(TradePhase::Delivered)
            }
            TradePhase::Delivered => {
                let ctx = self.orchestrator.find(deal_id)?.clone();
                let st = self.settlement.settle(&ctx)?;
                self.orchestrator.settle(deal_id, st)?;
                Ok(TradePhase::Settled)
            }
            TradePhase::Settled | TradePhase::Failed => Err(TradeError::InvalidPhase),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_loop() -> ClosedLoopImpl {
        let mut orch = TradeOrchestrator::new();
        orch.create_trade(1, 2, crate::MilliKwh::from_kwh(200.0), YuanPerKwh(0.8));
        let settlement = AutoSettlement::new(9999);
        // v0.230.0：注入真实计量闭包（全额交付 200 kWh）。
        ClosedLoopImpl::new(orch, settlement)
            .with_meter(|_deal_id| Ok(crate::MilliKwh::from_kwh(200.0)))
    }

    #[test]
    fn run_to_completion_reaches_settled() {
        let mut cl = make_loop();
        let result = cl.run_to_completion(1).unwrap();
        assert_eq!(result, TradePhase::Settled);

        let ctx = cl.orchestrator.find(1).unwrap();
        assert_eq!(ctx.phase, TradePhase::Settled);
        assert!(ctx.settlement.is_some());
        let st = ctx.settlement.as_ref().unwrap();
        assert_eq!(st.seller_credit_kwh, crate::MilliKwh::from_kwh(200.0));
        assert_eq!(st.amount_yuan, crate::types::MilliYuan(160_000), "200 × 0.8 = 160 元");
    }

    #[test]
    fn phase_sequence_correct() {
        let mut cl = make_loop();

        assert_eq!(cl.advance(1).unwrap(), TradePhase::Frozen);
        assert_eq!(cl.advance(1).unwrap(), TradePhase::Confirmed);
        assert_eq!(cl.advance(1).unwrap(), TradePhase::Delivering);
        assert_eq!(cl.advance(1).unwrap(), TradePhase::Delivered);
        assert_eq!(cl.advance(1).unwrap(), TradePhase::Settled);
    }

    #[test]
    fn advance_past_settled_fails() {
        let mut cl = make_loop();
        cl.run_to_completion(1).unwrap();
        assert_eq!(cl.advance(1), Err(TradeError::InvalidPhase));
    }

    #[test]
    fn advance_not_found() {
        let mut cl = make_loop();
        assert_eq!(cl.advance(999), Err(TradeError::DealNotFound));
    }
}
