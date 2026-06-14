use std::time::Duration;
use chrono::{DateTime, Utc};
use eneros_core::{AuthorityLevel, ElementId, Jurisdiction, Result, ZoneId};
use eneros_eventbus::{Event, event::{EventType, EventPayload}};
use crate::agent::{Agent, AgentAction, AgentType};
use crate::context::AgentContext;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// Market price data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketPrice {
    pub energy_price_yuan_per_mwh: f64,
    pub capacity_price_yuan_per_mw: f64,
    pub timestamp: DateTime<Utc>,
}

/// Bidding strategy
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BidStrategy {
    /// Bid at marginal cost
    MarginalCost,
    /// Bid at marginal cost + markup factor
    StrategicMarkup(f64),
    /// Bid adjusted for risk
    RiskAdjusted,
}

/// A trading bid for a generator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingBid {
    pub gen_id: ElementId,
    pub bid_price_yuan_per_mwh: f64,
    pub bid_quantity_mw: f64,
    pub strategy: BidStrategy,
    pub timestamp: DateTime<Utc>,
}

/// Risk assessment for trading decisions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskAssessment {
    pub equipment_failure_probability: f64,
    pub price_volatility: f64,
    pub risk_adjustment_factor: f64,
}

/// Generator cost curve for trading: cost = a*P^2 + b*P + c
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenCostCurve {
    pub gen_id: ElementId,
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub p_min: f64,
    pub p_max: f64,
}

/// Trading Agent — handles energy market bidding and risk management
pub struct TradingAgent {
    agent_id: String,
    jurisdiction: Jurisdiction,
    pub gen_cost_curves: Vec<GenCostCurve>,
    pub current_market_price: RwLock<Option<MarketPrice>>,
    pub markup_factor: f64,
    pub risk_tolerance: f64,
}

impl TradingAgent {
    pub fn new(agent_id: &str, zone_ids: Vec<ZoneId>) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            jurisdiction: Jurisdiction::for_zones(zone_ids),
            gen_cost_curves: Vec::new(),
            current_market_price: RwLock::new(None),
            markup_factor: 1.05,
            risk_tolerance: 0.1,
        }
    }

    pub fn with_gen_cost_curves(mut self, curves: Vec<GenCostCurve>) -> Self {
        self.gen_cost_curves = curves;
        self
    }

    pub fn with_markup_factor(mut self, factor: f64) -> Self {
        self.markup_factor = factor;
        self
    }

    pub fn with_risk_tolerance(mut self, tolerance: f64) -> Self {
        self.risk_tolerance = tolerance;
        self
    }

    pub fn add_gen_cost_curve(&mut self, curve: GenCostCurve) {
        self.gen_cost_curves.push(curve);
    }

    /// Calculate marginal cost at given output: dC/dP = 2aP + b
    pub fn marginal_cost(&self, gen_id: ElementId, p_mw: f64) -> f64 {
        if let Some(curve) = self.gen_cost_curves.iter().find(|c| c.gen_id == gen_id) {
            2.0 * curve.a * p_mw + curve.b
        } else {
            0.0
        }
    }

    /// Generate bids at marginal cost + markup
    pub fn marginal_cost_pricing(&self) -> Vec<TradingBid> {
        let now = Utc::now();
        self.gen_cost_curves.iter().map(|curve| {
            let p_bid = (curve.p_min + curve.p_max) / 2.0;
            let mc = 2.0 * curve.a * p_bid + curve.b;
            let bid_price = mc * self.markup_factor;

            TradingBid {
                gen_id: curve.gen_id,
                bid_price_yuan_per_mwh: bid_price,
                bid_quantity_mw: curve.p_max - curve.p_min,
                strategy: BidStrategy::StrategicMarkup(self.markup_factor),
                timestamp: now,
            }
        }).collect()
    }

    /// Adjust bid for risk factors
    pub fn risk_adjusted_bid(&self, base_bid: &TradingBid, risk: &RiskAssessment) -> TradingBid {
        let risk_markup = 1.0 + risk.risk_adjustment_factor * self.risk_tolerance;
        TradingBid {
            gen_id: base_bid.gen_id,
            bid_price_yuan_per_mwh: base_bid.bid_price_yuan_per_mwh * risk_markup,
            bid_quantity_mw: base_bid.bid_quantity_mw,
            strategy: BidStrategy::RiskAdjusted,
            timestamp: Utc::now(),
        }
    }

    /// Assess current risk factors
    pub fn assess_risk(&self) -> RiskAssessment {
        let equipment_failure_probability = 0.02; // baseline 2%
        let price_volatility = match self.current_market_price.read().as_ref() {
            Some(price) => {
                // Higher prices imply higher volatility
                if price.energy_price_yuan_per_mwh > 500.0 { 0.3 }
                else if price.energy_price_yuan_per_mwh > 300.0 { 0.15 }
                else { 0.05 }
            }
            None => 0.1,
        };
        let risk_adjustment_factor = equipment_failure_probability + price_volatility;

        RiskAssessment {
            equipment_failure_probability,
            price_volatility,
            risk_adjustment_factor,
        }
    }

    /// Main bid generation logic
    pub fn generate_bids(&self) -> Vec<TradingBid> {
        let base_bids = self.marginal_cost_pricing();
        let risk = self.assess_risk();

        base_bids.iter().map(|bid| {
            self.risk_adjusted_bid(bid, &risk)
        }).collect()
    }
}

#[async_trait::async_trait]
impl Agent for TradingAgent {
    fn id(&self) -> &str { &self.agent_id }
    fn name(&self) -> &str { "trading-agent" }
    fn agent_type(&self) -> AgentType { AgentType::Custom("Trading".to_string()) }
    fn authority_level(&self) -> AuthorityLevel { AuthorityLevel::Operator }
    fn jurisdiction(&self) -> Jurisdiction { self.jurisdiction.clone() }
    fn tick_interval(&self) -> Duration { Duration::from_secs(300) }

    async fn start(&mut self) -> Result<()> {
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        Ok(())
    }

    async fn handle_event(&mut self, event: &Event, _ctx: &AgentContext) -> Result<Vec<AgentAction>> {
        let mut actions = Vec::new();

        match event.event_type {
            EventType::DataReceived => {
                // Market price data received — update current_market_price, generate bids
                if let EventPayload::Message(msg) = &event.payload {
                    // Try to parse market price from message
                    if let Ok(price) = msg.parse::<f64>() {
                        if price > 0.0 {
                            *self.current_market_price.write() = Some(MarketPrice {
                                energy_price_yuan_per_mwh: price,
                                capacity_price_yuan_per_mw: price * 0.1,
                                timestamp: Utc::now(),
                            });
                        }
                    }
                }

                let bids = self.generate_bids();
                if !bids.is_empty() {
                    actions.push(AgentAction::LogMessage(format!(
                        "TradingAgent: 收到市场数据，生成 {} 个报价", bids.len()
                    )));
                }
            }
            EventType::ConstraintViolation => {
                // Adjust bids for constrained generators
                actions.push(AgentAction::LogMessage(
                    "TradingAgent: 检测到越限，调整受限机组报价".to_string()
                ));
            }
            _ => {}
        }

        Ok(actions)
    }

    async fn tick(&mut self, _ctx: &AgentContext) -> Result<Vec<AgentAction>> {
        let mut actions = Vec::new();

        // Periodic bid generation if market price available
        if self.current_market_price.read().is_some() {
            let bids = self.generate_bids();
            for bid in &bids {
                actions.push(AgentAction::LogMessage(format!(
                    "TradingAgent: 机组 {} 报价 {:.1} 元/MWh, 容量 {:.1} MW, 策略 {:?}",
                    bid.gen_id, bid.bid_price_yuan_per_mwh, bid.bid_quantity_mw, bid.strategy
                )));
            }

            // Publish TradingBidsReady event
            actions.push(AgentAction::PublishEvent(Event::new(
                EventType::DataReceived,
                "trading-agent",
                EventPayload::Message(format!(
                    "TradingBidsReady: {} bids generated", bids.len()
                )),
            )));
        }

        Ok(actions)
    }

    async fn handle_emergency(&mut self, _event: &Event, _ctx: &AgentContext) -> Result<Vec<AgentAction>> {
        // In emergency, submit bids at marginal cost (no markup)
        let mut actions = Vec::new();
        let now = Utc::now();

        for curve in &self.gen_cost_curves {
            let p_bid = (curve.p_min + curve.p_max) / 2.0;
            let mc = 2.0 * curve.a * p_bid + curve.b;

            actions.push(AgentAction::LogMessage(format!(
                "TradingAgent: 紧急模式 — 机组 {} 按边际成本 {:.1} 元/MWh 报价",
                curve.gen_id, mc
            )));

            let _bid = TradingBid {
                gen_id: curve.gen_id,
                bid_price_yuan_per_mwh: mc,
                bid_quantity_mw: curve.p_max,
                strategy: BidStrategy::MarginalCost,
                timestamp: now,
            };
        }

        Ok(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use parking_lot::RwLock as PlRwLock;
    use eneros_eventbus::EventBus;
    use eneros_gateway::SafetyGateway;
    use eneros_tool::ToolEngine;
    use eneros_network::PowerNetwork;
    use eneros_memory::InMemoryMemory;
    use eneros_reasoning::RuleBasedEngine;

    fn test_context() -> AgentContext {
        AgentContext::new(
            Arc::new(EventBus::new(64)),
            Arc::new(SafetyGateway::new(100)),
            Arc::new(PlRwLock::new(ToolEngine::new())),
            Arc::new(PlRwLock::new(PowerNetwork::from_ieee14())),
            Arc::new(InMemoryMemory::default()),
            Arc::new(RuleBasedEngine::new()),
        )
    }

    fn make_test_curves() -> Vec<GenCostCurve> {
        vec![
            GenCostCurve { gen_id: 1, a: 0.01, b: 200.0, c: 500.0, p_min: 50.0, p_max: 300.0 },
            GenCostCurve { gen_id: 2, a: 0.02, b: 250.0, c: 400.0, p_min: 30.0, p_max: 200.0 },
            GenCostCurve { gen_id: 3, a: 0.015, b: 220.0, c: 450.0, p_min: 40.0, p_max: 250.0 },
        ]
    }

    fn make_test_agent() -> TradingAgent {
        TradingAgent::new("trade-1", vec![1, 2])
            .with_gen_cost_curves(make_test_curves())
    }

    #[test]
    fn test_marginal_cost_calculation() {
        let agent = make_test_agent();

        // dC/dP = 2aP + b for gen_id=1: a=0.01, b=200
        // At P=100: MC = 2*0.01*100 + 200 = 202
        let mc = agent.marginal_cost(1, 100.0);
        assert!((mc - 202.0).abs() < 0.01);

        // At P=200: MC = 2*0.01*200 + 200 = 204
        let mc2 = agent.marginal_cost(2, 100.0);
        assert!((mc2 - 254.0).abs() < 0.01);
    }

    #[test]
    fn test_marginal_cost_unknown_gen() {
        let agent = make_test_agent();
        let mc = agent.marginal_cost(999, 100.0);
        assert!((mc - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_marginal_cost_pricing() {
        let agent = make_test_agent();
        let bids = agent.marginal_cost_pricing();

        assert_eq!(bids.len(), 3);
        for bid in &bids {
            assert!(bid.bid_price_yuan_per_mwh > 0.0);
            assert!(bid.bid_quantity_mw > 0.0);
            assert_eq!(bid.strategy, BidStrategy::StrategicMarkup(1.05));
        }
    }

    #[test]
    fn test_risk_adjusted_bid() {
        let agent = make_test_agent();
        let base_bid = TradingBid {
            gen_id: 1,
            bid_price_yuan_per_mwh: 200.0,
            bid_quantity_mw: 250.0,
            strategy: BidStrategy::MarginalCost,
            timestamp: Utc::now(),
        };
        let risk = RiskAssessment {
            equipment_failure_probability: 0.02,
            price_volatility: 0.1,
            risk_adjustment_factor: 0.12,
        };

        let adjusted = agent.risk_adjusted_bid(&base_bid, &risk);

        // risk_markup = 1.0 + 0.12 * 0.1 = 1.012
        let expected_price = 200.0 * (1.0 + 0.12 * 0.1);
        assert!((adjusted.bid_price_yuan_per_mwh - expected_price).abs() < 0.01);
        assert_eq!(adjusted.bid_quantity_mw, base_bid.bid_quantity_mw);
        assert_eq!(adjusted.strategy, BidStrategy::RiskAdjusted);
    }

    #[test]
    fn test_assess_risk_no_market_price() {
        let agent = make_test_agent();
        let risk = agent.assess_risk();

        assert!((risk.equipment_failure_probability - 0.02).abs() < 1e-10);
        assert!((risk.price_volatility - 0.1).abs() < 1e-10); // no market price
        assert!((risk.risk_adjustment_factor - 0.12).abs() < 1e-10);
    }

    #[test]
    fn test_assess_risk_with_high_market_price() {
        let agent = make_test_agent();
        *agent.current_market_price.write() = Some(MarketPrice {
            energy_price_yuan_per_mwh: 600.0,
            capacity_price_yuan_per_mw: 60.0,
            timestamp: Utc::now(),
        });

        let risk = agent.assess_risk();
        assert!((risk.price_volatility - 0.3).abs() < 1e-10);
    }

    #[test]
    fn test_assess_risk_with_medium_market_price() {
        let agent = make_test_agent();
        *agent.current_market_price.write() = Some(MarketPrice {
            energy_price_yuan_per_mwh: 400.0,
            capacity_price_yuan_per_mw: 40.0,
            timestamp: Utc::now(),
        });

        let risk = agent.assess_risk();
        assert!((risk.price_volatility - 0.15).abs() < 1e-10);
    }

    #[test]
    fn test_generate_bids() {
        let agent = make_test_agent();
        *agent.current_market_price.write() = Some(MarketPrice {
            energy_price_yuan_per_mwh: 350.0,
            capacity_price_yuan_per_mw: 35.0,
            timestamp: Utc::now(),
        });

        let bids = agent.generate_bids();
        assert_eq!(bids.len(), 3);
        for bid in &bids {
            assert!(bid.bid_price_yuan_per_mwh > 0.0);
            assert_eq!(bid.strategy, BidStrategy::RiskAdjusted);
        }
    }

    #[test]
    fn test_trading_agent_new() {
        let agent = TradingAgent::new("trade-1", vec![1, 2]);
        assert_eq!(agent.id(), "trade-1");
        assert_eq!(agent.name(), "trading-agent");
        assert_eq!(agent.agent_type(), AgentType::Custom("Trading".to_string()));
        assert_eq!(agent.authority_level(), AuthorityLevel::Operator);
        assert_eq!(agent.tick_interval(), Duration::from_secs(300));
    }

    #[test]
    fn test_trading_agent_defaults() {
        let agent = TradingAgent::new("trade-1", vec![1]);
        assert!((agent.markup_factor - 1.05).abs() < 1e-10);
        assert!((agent.risk_tolerance - 0.1).abs() < 1e-10);
        assert!(agent.current_market_price.read().is_none());
    }

    #[test]
    fn test_trading_agent_builder() {
        let agent = TradingAgent::new("trade-1", vec![1])
            .with_markup_factor(1.10)
            .with_risk_tolerance(0.2);
        assert!((agent.markup_factor - 1.10).abs() < 1e-10);
        assert!((agent.risk_tolerance - 0.2).abs() < 1e-10);
    }

    #[tokio::test]
    async fn test_trading_agent_handle_event_data_received() {
        let mut agent = make_test_agent();
        let ctx = test_context();

        let event = Event::new(
            EventType::DataReceived,
            "market",
            EventPayload::Message("350.0".to_string()),
        );

        let actions = agent.handle_event(&event, &ctx).await.unwrap();
        assert!(agent.current_market_price.read().is_some());
        assert!(actions.iter().any(|a| matches!(a, AgentAction::LogMessage(_))));
    }

    #[tokio::test]
    async fn test_trading_agent_handle_event_constraint_violation() {
        let mut agent = make_test_agent();
        let ctx = test_context();

        let event = Event::new(
            EventType::ConstraintViolation,
            "test",
            EventPayload::ConstraintViolation {
                constraint_id: "line-1".to_string(),
                element_id: 1,
                actual_value: 120.0,
                limit_value: 100.0,
                severity: "major".to_string(),
            },
        );

        let actions = agent.handle_event(&event, &ctx).await.unwrap();
        assert!(actions.iter().any(|a| matches!(a, AgentAction::LogMessage(_))));
    }

    #[tokio::test]
    async fn test_trading_agent_tick_with_market_price() {
        let mut agent = make_test_agent();
        let ctx = test_context();

        *agent.current_market_price.write() = Some(MarketPrice {
            energy_price_yuan_per_mwh: 350.0,
            capacity_price_yuan_per_mw: 35.0,
            timestamp: Utc::now(),
        });

        let actions = agent.tick(&ctx).await.unwrap();
        // Should have log messages for each bid + a publish event
        assert!(!actions.is_empty());
        assert!(actions.iter().any(|a| matches!(a, AgentAction::PublishEvent(_))));
    }

    #[tokio::test]
    async fn test_trading_agent_tick_no_market_price() {
        let mut agent = make_test_agent();
        let ctx = test_context();

        let actions = agent.tick(&ctx).await.unwrap();
        assert!(actions.is_empty());
    }

    #[tokio::test]
    async fn test_trading_agent_handle_emergency() {
        let mut agent = make_test_agent();
        let ctx = test_context();

        let event = Event::new(
            EventType::SystemAlarm,
            "emergency",
            EventPayload::Message("emergency".to_string()),
        );

        let actions = agent.handle_emergency(&event, &ctx).await.unwrap();
        assert!(!actions.is_empty());
        // Should log marginal cost bids for each generator
        assert!(actions.iter().any(|a| matches!(a, AgentAction::LogMessage(_))));
    }

    #[tokio::test]
    async fn test_trading_agent_start_stop() {
        let mut agent = make_test_agent();
        agent.start().await.unwrap();
        agent.stop().await.unwrap();
    }
}
