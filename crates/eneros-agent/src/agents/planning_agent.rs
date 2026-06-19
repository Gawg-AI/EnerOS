use std::time::Duration;
use eneros_core::{AuthorityLevel, ElementId, Jurisdiction, PowerSystemState, Result, ZoneId};
use eneros_eventbus::{Event, event::{EventType, EventPayload}};
use crate::agent::{Agent, AgentAction, AgentType};
use crate::context::AgentContext;
use serde::{Deserialize, Serialize};

/// Risk level for expansion plans
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

/// Candidate transmission line for expansion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateLine {
    pub from_bus: ElementId,
    pub to_bus: ElementId,
    pub capacity_mw: f64,
    pub cost_million_cny: f64,
    pub length_km: f64,
}

/// Candidate transformer for expansion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateTransformer {
    pub bus_id: ElementId,
    pub capacity_mva: f64,
    pub cost_million_cny: f64,
    pub voltage_ratio: String,
}

/// Expansion plan with candidate investments
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpansionPlan {
    pub plan_id: String,
    pub candidate_lines: Vec<CandidateLine>,
    pub candidate_transformers: Vec<CandidateTransformer>,
    pub total_investment_cost: f64,
    pub annual_benefit: f64,
    pub payback_years: f64,
    pub risk_level: RiskLevel,
}

/// Capacity assessment result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapacityAssessment {
    pub current_max_load_mw: f64,
    pub forecast_peak_load_mw: f64,
    pub overloaded_branches: Vec<(ElementId, f64)>,
    pub capacity_margin_percent: f64,
    pub needs_expansion: bool,
}

/// Planning Agent — handles capacity assessment and expansion planning
pub struct PlanningAgent {
    agent_id: String,
    jurisdiction: Jurisdiction,
    pub load_growth_rate: f64,
    pub planning_horizon_years: u32,
    pub discount_rate: f64,
    pub candidate_plans: Vec<ExpansionPlan>,
}

impl PlanningAgent {
    pub fn new(agent_id: &str, zone_ids: Vec<ZoneId>) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            jurisdiction: Jurisdiction::for_zones(zone_ids),
            load_growth_rate: 0.05,
            planning_horizon_years: 5,
            discount_rate: 0.08,
            candidate_plans: Vec::new(),
        }
    }

    pub fn with_load_growth_rate(mut self, rate: f64) -> Self {
        self.load_growth_rate = rate;
        self
    }

    pub fn with_planning_horizon(mut self, years: u32) -> Self {
        self.planning_horizon_years = years;
        self
    }

    pub fn with_discount_rate(mut self, rate: f64) -> Self {
        self.discount_rate = rate;
        self
    }

    /// Evaluate current network capacity against loading
    pub fn evaluate_capacity(&self, state: &PowerSystemState) -> CapacityAssessment {
        let current_max_load_mw: f64 = state.loads.iter()
            .filter(|l| l.status)
            .map(|l| l.active_power_mw)
            .sum();

        let forecast_peak_load_mw = current_max_load_mw
            * (1.0 + self.load_growth_rate).powi(self.planning_horizon_years as i32);

        let overloaded_branches: Vec<(ElementId, f64)> = state.branch_flows.iter()
            .filter(|b| b.loading_percent > 100.0)
            .map(|b| (b.branch_id, b.loading_percent))
            .collect();

        // Capacity margin: how much headroom before hitting limits
        let total_capacity: f64 = state.branch_flows.iter()
            .map(|b| b.active_power_mw / (b.loading_percent / 100.0).max(0.01))
            .sum();
        let capacity_margin_percent = if total_capacity > 0.0 {
            (1.0 - current_max_load_mw / total_capacity) * 100.0
        } else {
            0.0
        };

        let needs_expansion = !overloaded_branches.is_empty()
            || capacity_margin_percent < 15.0
            || forecast_peak_load_mw > current_max_load_mw * 1.2;

        CapacityAssessment {
            current_max_load_mw,
            forecast_peak_load_mw,
            overloaded_branches,
            capacity_margin_percent,
            needs_expansion,
        }
    }

    /// Generate candidate expansion plans based on capacity assessment
    pub fn propose_expansion(&self, assessment: &CapacityAssessment) -> Vec<ExpansionPlan> {
        if !assessment.needs_expansion {
            return Vec::new();
        }

        let mut plans = Vec::new();
        let deficit_mw = (assessment.forecast_peak_load_mw - assessment.current_max_load_mw).max(0.0);

        // Plan 1: Minimal — add lines only for overloaded branches
        let mut minimal_lines = Vec::new();
        for &(branch_id, loading) in &assessment.overloaded_branches {
            let excess_percent = loading - 100.0;
            let needed_capacity = excess_percent / 100.0 * 100.0; // approximate
            minimal_lines.push(CandidateLine {
                from_bus: branch_id,
                to_bus: branch_id + 100,
                capacity_mw: needed_capacity.max(50.0),
                cost_million_cny: needed_capacity.max(50.0) * 0.8,
                length_km: 20.0,
            });
        }
        let minimal_cost: f64 = minimal_lines.iter().map(|l| l.cost_million_cny).sum();
        let minimal_benefit = deficit_mw * 0.5 * 8760.0 * 0.0005; // simplified annual benefit
        if !minimal_lines.is_empty() {
            plans.push(ExpansionPlan {
                plan_id: "minimal-expansion".to_string(),
                candidate_lines: minimal_lines,
                candidate_transformers: Vec::new(),
                total_investment_cost: minimal_cost,
                annual_benefit: minimal_benefit,
                payback_years: if minimal_benefit > 0.0 { minimal_cost / minimal_benefit } else { f64::INFINITY },
                risk_level: RiskLevel::Low,
            });
        }

        // Plan 2: Moderate — add lines and transformers
        let moderate_lines = vec![CandidateLine {
            from_bus: 1,
            to_bus: 2,
            capacity_mw: deficit_mw.max(200.0),
            cost_million_cny: deficit_mw.max(200.0) * 0.6,
            length_km: 50.0,
        }];
        let moderate_transformers = vec![CandidateTransformer {
            bus_id: 1,
            capacity_mva: deficit_mw.max(200.0) * 1.1,
            cost_million_cny: deficit_mw.max(200.0) * 0.3,
            voltage_ratio: "220/110".to_string(),
        }];
        let moderate_cost: f64 = moderate_lines.iter().map(|l| l.cost_million_cny).sum::<f64>()
            + moderate_transformers.iter().map(|t| t.cost_million_cny).sum::<f64>();
        let moderate_benefit = deficit_mw * 0.7 * 8760.0 * 0.0005;
        plans.push(ExpansionPlan {
            plan_id: "moderate-expansion".to_string(),
            candidate_lines: moderate_lines,
            candidate_transformers: moderate_transformers,
            total_investment_cost: moderate_cost,
            annual_benefit: moderate_benefit,
            payback_years: if moderate_benefit > 0.0 { moderate_cost / moderate_benefit } else { f64::INFINITY },
            risk_level: RiskLevel::Medium,
        });

        // Plan 3: Aggressive — large-scale expansion
        let aggressive_lines = vec![
            CandidateLine {
                from_bus: 1, to_bus: 3,
                capacity_mw: deficit_mw.max(500.0),
                cost_million_cny: deficit_mw.max(500.0) * 0.5,
                length_km: 80.0,
            },
            CandidateLine {
                from_bus: 2, to_bus: 4,
                capacity_mw: deficit_mw.max(300.0),
                cost_million_cny: deficit_mw.max(300.0) * 0.6,
                length_km: 60.0,
            },
        ];
        let aggressive_transformers = vec![
            CandidateTransformer {
                bus_id: 1,
                capacity_mva: deficit_mw.max(500.0) * 1.1,
                cost_million_cny: deficit_mw.max(500.0) * 0.3,
                voltage_ratio: "500/220".to_string(),
            },
            CandidateTransformer {
                bus_id: 2,
                capacity_mva: deficit_mw.max(300.0) * 1.1,
                cost_million_cny: deficit_mw.max(300.0) * 0.25,
                voltage_ratio: "220/110".to_string(),
            },
        ];
        let aggressive_cost: f64 = aggressive_lines.iter().map(|l| l.cost_million_cny).sum::<f64>()
            + aggressive_transformers.iter().map(|t| t.cost_million_cny).sum::<f64>();
        let aggressive_benefit = deficit_mw * 0.9 * 8760.0 * 0.0005;
        plans.push(ExpansionPlan {
            plan_id: "aggressive-expansion".to_string(),
            candidate_lines: aggressive_lines,
            candidate_transformers: aggressive_transformers,
            total_investment_cost: aggressive_cost,
            annual_benefit: aggressive_benefit,
            payback_years: if aggressive_benefit > 0.0 { aggressive_cost / aggressive_benefit } else { f64::INFINITY },
            risk_level: RiskLevel::High,
        });

        plans
    }

    /// Evaluate economics of a plan using NPV calculation
    /// NPV = -C0 + sum(B_t / (1+r)^t) for t=1..n
    pub fn evaluate_economics(&self, plan: &ExpansionPlan) -> f64 {
        let mut npv = -plan.total_investment_cost;
        for year in 1..=self.planning_horizon_years {
            let discount_factor = 1.0 / (1.0 + self.discount_rate).powi(year as i32);
            npv += plan.annual_benefit * discount_factor;
        }
        npv
    }
}

#[async_trait::async_trait]
impl Agent for PlanningAgent {
    fn id(&self) -> &str { &self.agent_id }
    fn name(&self) -> &str { "planning-agent" }
    fn agent_type(&self) -> AgentType { AgentType::Custom("Planning".to_string()) }
    fn authority_level(&self) -> AuthorityLevel { AuthorityLevel::Supervisor }
    fn jurisdiction(&self) -> Jurisdiction { self.jurisdiction.clone() }
    fn tick_interval(&self) -> Duration { Duration::from_secs(3600) }

    async fn handle_event(&mut self, event: &Event, _ctx: &AgentContext) -> Result<Vec<AgentAction>> {
        let mut actions = Vec::new();

        match event.event_type {
            EventType::ConstraintViolation => {
                // Overload detected — trigger capacity assessment
                let detail = match &event.payload {
                    EventPayload::ConstraintViolation { constraint_id, element_id, severity, .. } =>
                        format!("{} on element {} (severity={})", constraint_id, element_id, severity),
                    _ => "unknown constraint".to_string(),
                };
                actions.push(AgentAction::LogMessage(
                    format!("PlanningAgent: 收到越限事件，触发容量评估 — {}", detail)
                ));
            }
            EventType::DataReceived => {
                // Load forecast data received — update growth assumptions
                if let EventPayload::Message(msg) = &event.payload {
                    // Try to parse growth rate from message
                    if let Ok(rate) = msg.parse::<f64>() {
                        if rate > 0.0 && rate < 1.0 {
                            self.load_growth_rate = rate;
                            actions.push(AgentAction::LogMessage(format!(
                                "PlanningAgent: 更新负荷增长率至 {:.2}%", rate * 100.0
                            )));
                        }
                    }
                }
            }
            _ => {}
        }

        Ok(actions)
    }

    async fn tick(&mut self, ctx: &AgentContext) -> Result<Vec<AgentAction>> {
        let mut actions = Vec::new();

        // Get current network state
        let network = ctx.remote.network.read();
        if let Ok(power_flow) = network.solve() {
            let state = PowerSystemState {
                timestamp: chrono::Utc::now(),
                bus_voltages: power_flow.bus_results.iter().map(|b| eneros_core::BusVoltage {
                    bus_id: b.bus_id,
                    voltage_magnitude: b.voltage_magnitude,
                    voltage_angle: b.voltage_angle,
                    voltage_kv: b.voltage_magnitude * 230.0,
                }).collect(),
                branch_flows: power_flow.branch_results.iter().map(|b| eneros_core::BranchFlow {
                    branch_id: b.branch_id,
                    from_bus: b.from_bus,
                    to_bus: b.to_bus,
                    active_power_mw: b.p_from,
                    reactive_power_mvar: b.q_from,
                    current_ka: 0.0,
                    loading_percent: b.loading_percent,
                }).collect(),
                generation: Vec::new(),
                loads: Vec::new(),
                frequency: 50.0,
                total_losses: power_flow.total_losses,
            };

            // Step 1: Evaluate capacity
            let assessment = self.evaluate_capacity(&state);

            actions.push(AgentAction::LogMessage(format!(
                "PlanningAgent: 容量评估完成, 当前负荷={:.1}MW, 预测峰值={:.1}MW, 容量裕度={:.1}%, 需要扩容={}",
                assessment.current_max_load_mw,
                assessment.forecast_peak_load_mw,
                assessment.capacity_margin_percent,
                assessment.needs_expansion
            )));

            // Step 2: Propose expansion if needed
            if assessment.needs_expansion {
                let plans = self.propose_expansion(&assessment);
                self.candidate_plans = plans.clone();

                for plan in &plans {
                    let npv = self.evaluate_economics(plan);
                    actions.push(AgentAction::LogMessage(format!(
                        "PlanningAgent: 扩容方案 {} 投资={:.1}百万, 年收益={:.1}百万, 回收期={:.1}年, NPV={:.1}百万, 风险={:?}",
                        plan.plan_id, plan.total_investment_cost, plan.annual_benefit,
                        plan.payback_years, npv, plan.risk_level
                    )));
                }

                // Publish planning recommendation event
                actions.push(AgentAction::PublishEvent(Event::new(
                    EventType::DataReceived,
                    "planning-agent",
                    EventPayload::Message(format!(
                        "PlanningRecommendation: {} candidate plans generated",
                        plans.len()
                    )),
                )));
            }
        }

        Ok(actions)
    }

    async fn handle_emergency(&mut self, _event: &Event, _ctx: &AgentContext) -> Result<Vec<AgentAction>> {
        // In emergency, focus on immediate capacity relief
        let actions = vec![
            AgentAction::LogMessage(
                "PlanningAgent: 紧急模式 — 建议立即投入备用容量缓解过载".to_string()
            ),
            AgentAction::DelegateTask {
                target_agent_id: "dispatch".to_string(),
                task_description: "紧急：请最大化可用发电出力以缓解容量不足".to_string(),
            },
        ];

        Ok(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::sync::Arc;
    use parking_lot::RwLock;
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
            Arc::new(RwLock::new(ToolEngine::new())),
            Arc::new(RwLock::new(PowerNetwork::from_ieee14())),
            Arc::new(InMemoryMemory::default()),
            Arc::new(RuleBasedEngine::new()),
        )
    }

    fn make_test_state(loading_percents: Vec<f64>, load_mw: Vec<f64>) -> PowerSystemState {
        let branch_flows: Vec<eneros_core::BranchFlow> = loading_percents.iter().enumerate()
            .map(|(i, lp)| eneros_core::BranchFlow {
                branch_id: (i + 1) as ElementId,
                from_bus: (i + 1) as ElementId,
                to_bus: (i + 2) as ElementId,
                active_power_mw: 50.0,
                reactive_power_mvar: 10.0,
                current_ka: 0.2,
                loading_percent: *lp,
            })
            .collect();

        let loads: Vec<eneros_core::LoadConsumption> = load_mw.iter().enumerate()
            .map(|(i, mw)| eneros_core::LoadConsumption {
                load_id: (i + 1) as ElementId,
                bus_id: (i + 1) as ElementId,
                active_power_mw: *mw,
                reactive_power_mvar: mw * 0.3,
                status: true,
            })
            .collect();

        PowerSystemState {
            timestamp: Utc::now(),
            bus_voltages: Vec::new(),
            branch_flows,
            generation: Vec::new(),
            loads,
            frequency: 50.0,
            total_losses: 5.0,
        }
    }

    fn make_test_agent() -> PlanningAgent {
        PlanningAgent::new("plan-1", vec![1, 2])
    }

    #[test]
    fn test_capacity_assessment_normal_state() {
        let agent = make_test_agent();
        let state = make_test_state(vec![60.0, 70.0, 80.0], vec![100.0, 80.0]);
        let assessment = agent.evaluate_capacity(&state);

        // With loading 60-80%, no branches are overloaded
        assert!(assessment.overloaded_branches.is_empty());
        assert!(assessment.current_max_load_mw > 0.0);
        assert!(assessment.forecast_peak_load_mw > assessment.current_max_load_mw);
    }

    #[test]
    fn test_capacity_assessment_overloaded_state() {
        let agent = make_test_agent();
        let state = make_test_state(vec![60.0, 120.0, 150.0], vec![100.0, 80.0]);
        let assessment = agent.evaluate_capacity(&state);

        assert!(assessment.needs_expansion);
        assert_eq!(assessment.overloaded_branches.len(), 2);
        assert_eq!(assessment.overloaded_branches[0].0, 2); // branch_id 2
        assert_eq!(assessment.overloaded_branches[1].0, 3); // branch_id 3
    }

    #[test]
    fn test_propose_expansion_no_need() {
        let agent = make_test_agent();
        let assessment = CapacityAssessment {
            current_max_load_mw: 100.0,
            forecast_peak_load_mw: 110.0,
            overloaded_branches: Vec::new(),
            capacity_margin_percent: 30.0,
            needs_expansion: false,
        };
        let plans = agent.propose_expansion(&assessment);
        assert!(plans.is_empty());
    }

    #[test]
    fn test_propose_expansion_generates_valid_plans() {
        let agent = make_test_agent();
        let assessment = CapacityAssessment {
            current_max_load_mw: 100.0,
            forecast_peak_load_mw: 150.0,
            overloaded_branches: vec![(1, 120.0)],
            capacity_margin_percent: 5.0,
            needs_expansion: true,
        };
        let plans = agent.propose_expansion(&assessment);

        assert!(!plans.is_empty());
        // Should have minimal, moderate, and aggressive plans
        assert!(plans.iter().any(|p| p.plan_id == "minimal-expansion"));
        assert!(plans.iter().any(|p| p.plan_id == "moderate-expansion"));
        assert!(plans.iter().any(|p| p.plan_id == "aggressive-expansion"));

        for plan in &plans {
            assert!(plan.total_investment_cost > 0.0);
            assert!(plan.annual_benefit > 0.0);
            assert!(plan.payback_years > 0.0);
        }
    }

    #[test]
    fn test_evaluate_economics_npv() {
        let agent = make_test_agent(); // discount_rate = 0.08, horizon = 5

        let plan = ExpansionPlan {
            plan_id: "test-plan".to_string(),
            candidate_lines: Vec::new(),
            candidate_transformers: Vec::new(),
            total_investment_cost: 100.0,
            annual_benefit: 30.0,
            payback_years: 3.33,
            risk_level: RiskLevel::Medium,
        };

        let npv = agent.evaluate_economics(&plan);

        // NPV = -100 + 30/(1.08) + 30/(1.08^2) + 30/(1.08^3) + 30/(1.08^4) + 30/(1.08^5)
        let expected = -100.0
            + 30.0 / 1.08_f64.powi(1)
            + 30.0 / 1.08_f64.powi(2)
            + 30.0 / 1.08_f64.powi(3)
            + 30.0 / 1.08_f64.powi(4)
            + 30.0 / 1.08_f64.powi(5);

        assert!((npv - expected).abs() < 0.01);
        assert!(npv > 0.0); // This plan should have positive NPV
    }

    #[test]
    fn test_evaluate_economics_negative_npv() {
        let agent = make_test_agent();

        let plan = ExpansionPlan {
            plan_id: "bad-plan".to_string(),
            candidate_lines: Vec::new(),
            candidate_transformers: Vec::new(),
            total_investment_cost: 1000.0,
            annual_benefit: 10.0,
            payback_years: 100.0,
            risk_level: RiskLevel::High,
        };

        let npv = agent.evaluate_economics(&plan);
        assert!(npv < 0.0);
    }

    #[test]
    fn test_planning_agent_new() {
        let agent = PlanningAgent::new("plan-1", vec![1, 2]);
        assert_eq!(agent.id(), "plan-1");
        assert_eq!(agent.name(), "planning-agent");
        assert_eq!(agent.agent_type(), AgentType::Custom("Planning".to_string()));
        assert_eq!(agent.authority_level(), AuthorityLevel::Supervisor);
        assert_eq!(agent.tick_interval(), Duration::from_secs(3600));
    }

    #[test]
    fn test_planning_agent_defaults() {
        let agent = make_test_agent();
        assert!((agent.load_growth_rate - 0.05).abs() < 1e-10);
        assert_eq!(agent.planning_horizon_years, 5);
        assert!((agent.discount_rate - 0.08).abs() < 1e-10);
    }

    #[test]
    fn test_planning_agent_builder() {
        let agent = make_test_agent()
            .with_load_growth_rate(0.08)
            .with_planning_horizon(10)
            .with_discount_rate(0.10);
        assert!((agent.load_growth_rate - 0.08).abs() < 1e-10);
        assert_eq!(agent.planning_horizon_years, 10);
        assert!((agent.discount_rate - 0.10).abs() < 1e-10);
    }

    #[tokio::test]
    async fn test_planning_agent_handle_event_constraint_violation() {
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
    async fn test_planning_agent_handle_event_data_received() {
        let mut agent = make_test_agent();
        let ctx = test_context();

        let event = Event::new(
            EventType::DataReceived,
            "forecast",
            EventPayload::Message("0.07".to_string()),
        );

        let _actions = agent.handle_event(&event, &ctx).await.unwrap();
        assert!((agent.load_growth_rate - 0.07).abs() < 1e-10);
    }

    #[tokio::test]
    async fn test_planning_agent_handle_emergency() {
        let mut agent = make_test_agent();
        let ctx = test_context();

        let event = Event::new(
            EventType::SystemAlarm,
            "emergency",
            EventPayload::Message("emergency".to_string()),
        );

        let actions = agent.handle_emergency(&event, &ctx).await.unwrap();
        assert!(!actions.is_empty());
        assert!(actions.iter().any(|a| matches!(a, AgentAction::DelegateTask { .. })));
    }
}
