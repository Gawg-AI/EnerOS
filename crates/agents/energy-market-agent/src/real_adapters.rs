//! energy-market-agent 真实适配器（v0.226.0，B3-03）。
//!
//! 落地三个"后续接入"适配器（此前仅 Mock）：
//!
//! 1. [`RuleBidIntentSource`] — 真实意图源：基于市场馈送（电价/DR 信号）
//!    与储能 SOC 的规则决策（可审计、确定性），作为 LLM 意图链路的
//!    生产回退；
//! 2. [`SolverBidOptimizer`] — 真实优化器：接 solver-core `Solver`
//!    （HiGHS）求解报价优化（价格调整 + 电量）；
//! 3. [`MarketChannelBidPublisher`] — 真实发布器：经 market-types `Bid`
//!    构造报价单，发布到市场通道（DDS/MQTT 由上层注入发送闭包）。
//!
//! 构造器显式注入（B3 纪律）；Mock 保持 cfg(test)/feature mock。

use alloc::vec;
use alloc::vec::Vec;

use eneros_market_types::TradeDir;
use eneros_solver_core::solver::Solver;

use crate::bid_generator::{
    BidError, BidIntent, BidIntentSource, BidOptimization, BidOptimizer, BidPublisher, ResourceBid,
};
use crate::market_feed::MarketFeed;
use crate::market_feed::YuanPerKwh;

/// 规则意图源：电价与 SOC 驱动的报价意图。
///
/// 规则（可审计）：
/// - 无价格数据 → `IntentFailed`（真实数据源失败语义）；
/// - 价格高于卖出阈值且 SOC 充足 → 卖出；
/// - 价格低于买入阈值 → 买入；
/// - 否则不报价（`IntentFailed` 表示无意图）。
pub struct RuleBidIntentSource {
    /// 卖出触发电价（元/kWh）。
    pub sell_price_threshold: YuanPerKwh,
    /// 买入触发电价（元/kWh）。
    pub buy_price_threshold: YuanPerKwh,
    /// 卖出 SOC 门限（0..1）。
    pub soc_threshold: f32,
    /// 目标报价电量（MW）。
    pub target_quantity: f32,
}

impl Default for RuleBidIntentSource {
    fn default() -> Self {
        Self {
            sell_price_threshold: YuanPerKwh(0.8),
            buy_price_threshold: YuanPerKwh(0.2),
            soc_threshold: 0.3,
            target_quantity: 1.0,
        }
    }
}

impl BidIntentSource for RuleBidIntentSource {
    fn generate_intent(&mut self, feed: &MarketFeed, soc: f32) -> Result<BidIntent, BidError> {
        let price = feed
            .prices
            .first()
            .map(|p| p.price)
            .ok_or(BidError::IntentFailed)?;
        let side = if price >= self.sell_price_threshold && soc >= self.soc_threshold {
            TradeDir::Sell
        } else if price <= self.buy_price_threshold {
            TradeDir::Buy
        } else {
            return Err(BidError::IntentFailed);
        };
        Ok(BidIntent {
            side,
            target_quantity: self.target_quantity,
        })
    }
}

/// 真实优化器：接 solver-core `Solver`（HiGHS）求解报价优化。
///
/// LP 问题：最大化 `(price + adjust) * quantity`，约束电量不超过
/// 容量且非负。求解成功取 `SolveResult` 的目标值构造优化结果；
/// 求解失败返回 `OptimizeFailed`。
pub struct SolverBidOptimizer<S: Solver> {
    /// 求解器（HiGHS 生产实现；测试注入 mock）。
    solver: S,
}

impl<S: Solver> SolverBidOptimizer<S> {
    /// 创建优化器（构造器显式注入求解器）。
    pub fn new(solver: S) -> Self {
        Self { solver }
    }
}

impl<S: Solver> BidOptimizer for SolverBidOptimizer<S> {
    fn optimize(
        &mut self,
        intent: &BidIntent,
        feed: &MarketFeed,
        _soc: f32,
        capacity: f32,
    ) -> Result<BidOptimization, BidError> {
        if capacity <= 0.0 {
            return Err(BidError::InvalidInput);
        }
        let price = feed
            .prices
            .first()
            .map(|p| p.price)
            .ok_or(BidError::OptimizeFailed)?;
        // 真实求解器路径：调用注入的 Solver（HiGHS 生产实现）。
        // 求解 1 变量 LP：max price*q s.t. 0<=q<=capacity。
        let problem = eneros_solver_core::problem::LpProblem {
            variables: vec!["q".into()],
            lower_bounds: vec![0.0],
            upper_bounds: vec![capacity as f64],
            var_types: vec![eneros_solver_core::problem::VarType::Continuous],
            objective: vec![price.0],
            sense: eneros_solver_core::problem::ObjectiveSense::Maximize,
            constraints: eneros_solver_core::problem::ConstraintMatrix {
                num_rows: 0,
                num_nz: 0,
                row_start: vec![0],
                col_index: Vec::new(),
                values: Vec::new(),
            },
            rhs_lower: Vec::new(),
            rhs_upper: Vec::new(),
        };
        let solved = self.solver.solve(&problem, 0);
        // 求解器结果优先：成功且非负取解值；失败/无效回退闭式解
        // （电量 = min(目标, 容量)）。v0.244.0：旧代码第二行赋值覆盖了
        // 求解器结果（真 bug——优化结果从未生效）。
        let quantity = match solved {
            Ok(r) if !r.solution.is_empty() && r.solution[0] >= 0.0 => r.solution[0] as f32,
            _ => capacity.min(intent.target_quantity).max(0.0),
        };
        // 价格调整：按意图方向微调（Sell 加价 2%，Buy 减价 2%）。
        let adjust = match intent.side {
            TradeDir::Sell => (price.0 * 0.02) as f32,
            TradeDir::Buy => -(price.0 * 0.02) as f32,
        };
        Ok(BidOptimization {
            price_adjust: adjust,
            quantity,
        })
    }
}

/// 真实发布器：构造 market-types `Bid` 报价单并发送。
///
/// 发送闭包注入（DDS/MQTT 传输由上层提供）。
pub struct MarketChannelBidPublisher<F> {
    /// 发送闭包（序列化后的报价字节 → 市场通道）。
    send: F,
    /// 已发布报价记录（审计重放）。
    published: Vec<ResourceBid>,
}

impl<F> MarketChannelBidPublisher<F>
where
    F: FnMut(&[u8]) -> Result<(), BidError>,
{
    /// 创建发布器。
    pub fn new(send: F) -> Self {
        Self {
            send,
            published: Vec::new(),
        }
    }

    /// 已发布报价（审计重放）。
    pub fn published(&self) -> &[ResourceBid] {
        &self.published
    }
}

impl<F> BidPublisher for MarketChannelBidPublisher<F>
where
    F: FnMut(&[u8]) -> Result<(), BidError>,
{
    fn publish_bids(&mut self, bids: &[ResourceBid]) -> Result<(), BidError> {
        // 构造可审计的报价单字节（period|quantity|price|direction）。
        let mut payload = Vec::new();
        for rb in bids {
            payload.push(rb.period as u8);
            payload.extend_from_slice(&rb.bid.segments[0].volume_kwh.to_le_bytes());
            payload.extend_from_slice(&rb.bid.segments[0].price.0.to_le_bytes());
            payload.push(rb.bid.segments[0].direction as u8);
        }
        (self.send)(&payload)?;
        self.published.extend_from_slice(bids);
        Ok(())
    }
}

// ============================================================================
// Tests（真实规则/闭包 + 测试求解器）
// ============================================================================

#[cfg(test)]
mod tests {
    use eneros_market_types::{Bid, BidSegment, MarketType, TimeSlot, YuanPerKwh};
    use eneros_solver_core::problem::LpProblem;
    use eneros_solver_core::result::SolveResult;

    use super::*;
    use crate::market_feed::PricePoint;

    fn feed_with_price(price: f64) -> MarketFeed {
        MarketFeed {
            market_type: MarketType::Spot,
            timestamp: 1000,
            prices: vec![PricePoint {
                time: 0,
                price: YuanPerKwh(price),
                period: crate::market_feed::Period::Peak,
            }],
            dr_signals: Vec::new(),
        }
    }

    #[test]
    fn test_rule_source_sell_when_price_high_and_soc_ok() {
        let mut src = RuleBidIntentSource::default();
        let intent = src
            .generate_intent(&feed_with_price(1.2), 0.8)
            .expect("intent");
        assert_eq!(intent.side, TradeDir::Sell);
        assert_eq!(intent.target_quantity, 1.0);
    }

    #[test]
    fn test_rule_source_buy_when_price_low() {
        let mut src = RuleBidIntentSource::default();
        let intent = src
            .generate_intent(&feed_with_price(0.1), 0.1)
            .expect("intent");
        assert_eq!(intent.side, TradeDir::Buy);
    }

    #[test]
    fn test_rule_source_no_intent_mid_price() {
        let mut src = RuleBidIntentSource::default();
        assert!(src.generate_intent(&feed_with_price(0.5), 0.5).is_err());
    }

    #[test]
    fn test_rule_source_sell_blocked_low_soc() {
        let mut src = RuleBidIntentSource::default();
        // 高价但 SOC 不足 → 不卖出。
        assert!(src.generate_intent(&feed_with_price(1.2), 0.1).is_err());
    }

    #[test]
    fn test_rule_source_empty_feed_fails() {
        let mut src = RuleBidIntentSource::default();
        let empty = MarketFeed {
            market_type: MarketType::Spot,
            timestamp: 0,
            prices: Vec::new(),
            dr_signals: Vec::new(),
        };
        assert!(src.generate_intent(&empty, 0.8).is_err());
    }

    #[test]
    fn test_solver_optimizer_respects_capacity() {
        // 用 solver-core 的测试求解器（mock feature 或真实 HiGHS）。
        // 不依赖具体求解器：验证容量约束与意图方向。
        struct DummySolver;
        impl Solver for DummySolver {
            fn solve(
                &mut self,
                _p: &LpProblem,
                _t: u64,
            ) -> Result<SolveResult, eneros_solver_core::error::SolverError> {
                Ok(SolveResult::optimal(0.0, vec![2.0]))
            }
            fn name(&self) -> &'static str {
                "dummy"
            }
            fn version(&self) -> &'static str {
                "0"
            }
            fn set_param(
                &mut self,
                _k: &str,
                _v: &str,
            ) -> Result<(), eneros_solver_core::error::SolverError> {
                Ok(())
            }
            fn status(&self) -> eneros_solver_core::solver::SolverStatus {
                eneros_solver_core::solver::SolverStatus::Idle
            }
        }
        let mut opt = SolverBidOptimizer::new(DummySolver);
        let intent = BidIntent {
            side: TradeDir::Sell,
            target_quantity: 5.0,
        };
        let r = opt
            .optimize(&intent, &feed_with_price(1.0), 0.9, 2.0)
            .expect("opt");
        // 电量不超过容量。
        assert!(r.quantity <= 2.0);
        assert!(r.quantity > 0.0);
        // Sell → 正调整。
        assert!(r.price_adjust > 0.0);
    }

    #[test]
    fn test_solver_optimizer_invalid_capacity() {
        struct DummySolver;
        impl Solver for DummySolver {
            fn solve(
                &mut self,
                _p: &LpProblem,
                _t: u64,
            ) -> Result<SolveResult, eneros_solver_core::error::SolverError> {
                Ok(SolveResult::optimal(0.0, vec![0.0]))
            }
            fn name(&self) -> &'static str {
                "dummy"
            }
            fn version(&self) -> &'static str {
                "0"
            }
            fn set_param(
                &mut self,
                _k: &str,
                _v: &str,
            ) -> Result<(), eneros_solver_core::error::SolverError> {
                Ok(())
            }
            fn status(&self) -> eneros_solver_core::solver::SolverStatus {
                eneros_solver_core::solver::SolverStatus::Idle
            }
        }
        let mut opt = SolverBidOptimizer::new(DummySolver);
        let intent = BidIntent {
            side: TradeDir::Buy,
            target_quantity: 1.0,
        };
        assert!(opt
            .optimize(&intent, &feed_with_price(0.5), 0.5, 0.0)
            .is_err());
    }

    #[test]
    fn test_publisher_records_and_sends() {
        let sent_payloads: core::cell::RefCell<Vec<Vec<u8>>> = core::cell::RefCell::new(Vec::new());
        let mut pub_ = MarketChannelBidPublisher::new(|payload: &[u8]| {
            sent_payloads.borrow_mut().push(payload.to_vec());
            Ok(())
        });
        let rb = ResourceBid {
            resource_id: 1,
            period: crate::market_feed::Period::Peak,
            bid: Bid {
                bid_id: 1,
                market: eneros_market_types::MarketType::Spot,
                submitter: 1,
                segments: vec![BidSegment {
                    period: TimeSlot { start: 0, end: 0 },
                    volume_kwh: 1.5,
                    price: YuanPerKwh(0.9),
                    direction: TradeDir::Sell,
                }],
                submit_at: 1000,
            },
        };
        pub_.publish_bids(&[rb]).expect("publish");
        assert_eq!(sent_payloads.borrow().len(), 1);
        assert_eq!(pub_.published().len(), 1);
        assert_eq!(pub_.published()[0].resource_id, 1);
    }

    #[test]
    fn test_publisher_send_error_propagates() {
        let mut pub_ =
            MarketChannelBidPublisher::new(|_payload: &[u8]| Err(BidError::PublishFailed));
        let rb = ResourceBid {
            resource_id: 1,
            period: crate::market_feed::Period::Peak,
            bid: Bid {
                bid_id: 2,
                market: eneros_market_types::MarketType::Spot,
                submitter: 1,
                segments: vec![BidSegment {
                    period: TimeSlot { start: 0, end: 0 },
                    volume_kwh: 1.0,
                    price: YuanPerKwh(0.5),
                    direction: TradeDir::Buy,
                }],
                submit_at: 1000,
            },
        };
        assert!(pub_.publish_bids(&[rb]).is_err());
    }
}
