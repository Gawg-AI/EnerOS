use std::time::Duration;
use eneros_core::{AuthorityLevel, Jurisdiction, Result, ZoneId};
use eneros_gateway::command::{Command, CommandType, CommandPriority};
use eneros_eventbus::Event;
use eneros_reasoning::ReasoningInput;
use crate::agent::{Agent, AgentType, AgentAction};
use crate::context::AgentContext;
use serde::{Deserialize, Serialize};

/// Generator cost curve - quadratic cost model: cost = a*P^2 + b*P + c
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratorCostCurve {
    pub gen_id: String,
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub p_min_mw: f64,
    pub p_max_mw: f64,
}

impl GeneratorCostCurve {
    pub fn cost_at(&self, p_mw: f64) -> f64 {
        self.a * p_mw * p_mw + self.b * p_mw + self.c
    }

    pub fn incremental_cost(&self, p_mw: f64) -> f64 {
        2.0 * self.a * p_mw + self.b
    }
}

/// Economic dispatch result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EconomicDispatchResult {
    pub gen_outputs: Vec<(String, f64)>,
    pub total_cost: f64,
    pub total_generation_mw: f64,
    pub total_load_mw: f64,
}

/// Perform economic dispatch using lambda-iteration method
pub fn economic_dispatch(
    generators: &[GeneratorCostCurve],
    total_load_mw: f64,
) -> EconomicDispatchResult {
    if generators.is_empty() {
        return EconomicDispatchResult {
            gen_outputs: Vec::new(),
            total_cost: 0.0,
            total_generation_mw: 0.0,
            total_load_mw,
        };
    }

    let tolerance = 0.001;
    let max_iterations = 100;
    let mut lambda = 10.0;

    for _ in 0..max_iterations {
        let mut total_gen = 0.0;
        let mut gen_outputs = Vec::new();

        for gen in generators {
            let p = if gen.a.abs() > 1e-10 {
                (lambda - gen.b) / (2.0 * gen.a)
            } else {
                if lambda >= gen.b { gen.p_max_mw } else { gen.p_min_mw }
            };
            let p_clamped = p.clamp(gen.p_min_mw, gen.p_max_mw);
            total_gen += p_clamped;
            gen_outputs.push((gen.gen_id.clone(), p_clamped));
        }

        let mismatch = total_gen - total_load_mw;
        if mismatch.abs() < tolerance {
            let total_cost: f64 = gen_outputs.iter().zip(generators.iter())
                .map(|((_, p), gen)| gen.cost_at(*p))
                .sum();

            return EconomicDispatchResult {
                gen_outputs,
                total_cost,
                total_generation_mw: total_gen,
                total_load_mw,
            };
        }

        let step = 0.1;
        if mismatch > 0.0 {
            lambda -= step;
        } else {
            lambda += step;
        }
    }

    let total_capacity: f64 = generators.iter().map(|g| g.p_max_mw - g.p_min_mw).sum();
    let gen_outputs: Vec<(String, f64)> = generators.iter().map(|g| {
        let share = (g.p_max_mw - g.p_min_mw) / total_capacity;
        let p = g.p_min_mw + share * (total_load_mw - generators.iter().map(|g| g.p_min_mw).sum::<f64>()).max(0.0);
        (g.gen_id.clone(), p.clamp(g.p_min_mw, g.p_max_mw))
    }).collect();

    let total_cost: f64 = gen_outputs.iter().zip(generators.iter())
        .map(|((_, p), gen)| gen.cost_at(*p))
        .sum();
    let total_gen: f64 = gen_outputs.iter().map(|(_, p)| *p).sum();

    EconomicDispatchResult {
        gen_outputs,
        total_cost,
        total_generation_mw: total_gen,
        total_load_mw,
    }
}

/// Calculate Area Control Error (ACE)
pub fn calculate_ace(frequency_hz: f64, nominal_hz: f64, k_gov: f64) -> f64 {
    let delta_f = frequency_hz - nominal_hz;
    -k_gov * delta_f
}

/// Dispatch Agent - handles economic dispatch, AGC, and scheduling
pub struct DispatchAgent {
    id: String,
    name: String,
    jurisdiction: Jurisdiction,
    generators: Vec<GeneratorCostCurve>,
    k_gov: f64,
    nominal_hz: f64,
    last_dispatch: Option<EconomicDispatchResult>,
}

impl DispatchAgent {
    pub fn new(id: &str, name: &str, zone_ids: Vec<ZoneId>) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            jurisdiction: Jurisdiction::for_zones(zone_ids),
            generators: Vec::new(),
            k_gov: 100.0,
            nominal_hz: 50.0,
            last_dispatch: None,
        }
    }

    pub fn with_generators(mut self, generators: Vec<GeneratorCostCurve>) -> Self {
        self.generators = generators;
        self
    }

    pub fn with_k_gov(mut self, k_gov: f64) -> Self {
        self.k_gov = k_gov;
        self
    }

    pub fn add_generator(&mut self, gen: GeneratorCostCurve) {
        self.generators.push(gen);
    }

    fn get_total_load(&self, ctx: &AgentContext) -> f64 {
        let network = ctx.network.read();
        if let Ok(result) = network.solve() {
            let total_load: f64 = result.bus_results.iter()
                .filter(|b| b.p_injection < 0.0)
                .map(|b| -b.p_injection)
                .sum();
            if total_load > 0.0 {
                return total_load;
            }
        }
        100.0
    }

    fn get_frequency(&self, _ctx: &AgentContext) -> f64 {
        self.nominal_hz
    }

    fn gen_id_to_element_id(gen_id: &str) -> u64 {
        gen_id.parse::<u64>().unwrap_or(0)
    }

    /// Review dispatch result using the reasoning engine
    async fn review_dispatch_with_reasoning(
        &self,
        dispatch: &EconomicDispatchResult,
        context: &str,
        ctx: &AgentContext,
    ) -> Vec<AgentAction> {
        let goal = format!("dispatch review: {}", context);
        let mut input = ReasoningInput::new(&goal);

        input = input.with_observation(context);

        for (gen_id, p_mw) in &dispatch.gen_outputs {
            input = input.with_observation(&format!("Generator {} output: {:.1} MW", gen_id, p_mw));
        }
        input = input.with_observation(&format!(
            "Total generation: {:.1} MW, Total load: {:.1} MW, Total cost: ${:.2}",
            dispatch.total_generation_mw, dispatch.total_load_mw, dispatch.total_cost
        ));

        match ctx.reasoning.reason(input).await {
            Ok(output) => {
                vec![AgentAction::LogMessage(format!(
                    "dispatch review: {} (confidence: {:.2})",
                    output.conclusion, output.confidence
                ))]
            }
            Err(_) => Vec::new(),
        }
    }
}

#[async_trait::async_trait]
impl Agent for DispatchAgent {
    fn id(&self) -> &str { &self.id }
    fn name(&self) -> &str { &self.name }
    fn agent_type(&self) -> AgentType { AgentType::Dispatcher }
    fn authority_level(&self) -> AuthorityLevel { AuthorityLevel::Supervisor }
    fn jurisdiction(&self) -> Jurisdiction { self.jurisdiction.clone() }
    fn tick_interval(&self) -> Duration { Duration::from_secs(5) }

    async fn start(&mut self) -> Result<()> {
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        Ok(())
    }

    async fn handle_event(&mut self, event: &Event, ctx: &AgentContext) -> Result<Vec<AgentAction>> {
        let mut actions = Vec::new();

        match event.event_type {
            eneros_eventbus::event::EventType::ConstraintViolation => {
                let load = self.get_total_load(ctx);
                let dispatch = economic_dispatch(&self.generators, load);
                self.last_dispatch = Some(dispatch.clone());

                for (gen_id, target_mw) in &dispatch.gen_outputs {
                    actions.push(AgentAction::ExecuteCommand(
                        Command::new(
                            CommandType::GeneratorSetpoint,
                            Self::gen_id_to_element_id(gen_id),
                            CommandPriority::Normal,
                            &self.id,
                        )
                        .with_parameter("P_MW", *target_mw)
                    ));
                }

                let context = match &event.payload {
                    eneros_eventbus::event::EventPayload::ConstraintViolation {
                        constraint_id, element_id, actual_value, limit_value, severity, ..
                    } => format!(
                        "ConstraintViolation: {} on element {} (actual={:.4}, limit={:.4}, severity={})",
                        constraint_id, element_id, actual_value, limit_value, severity
                    ),
                    eneros_eventbus::event::EventPayload::Message(msg) => msg.clone(),
                    _ => "constraint violation".to_string(),
                };
                let review_actions = self.review_dispatch_with_reasoning(&dispatch, &context, ctx).await;
                actions.extend(review_actions);
            }
            eneros_eventbus::event::EventType::DataReceived => {
                let freq = self.get_frequency(ctx);
                let ace = calculate_ace(freq, self.nominal_hz, self.k_gov);

                if ace.abs() > 5.0 {
                    let load = self.get_total_load(ctx);
                    let dispatch = economic_dispatch(&self.generators, load + ace);

                    for (gen_id, target_mw) in &dispatch.gen_outputs {
                        actions.push(AgentAction::ExecuteCommand(
                            Command::new(
                                CommandType::GeneratorSetpoint,
                                Self::gen_id_to_element_id(gen_id),
                                CommandPriority::High,
                                &self.id,
                            )
                            .with_parameter("P_MW", *target_mw)
                        ));
                    }
                }
            }
            _ => {}
        }

        Ok(actions)
    }

    async fn tick(&mut self, ctx: &AgentContext) -> Result<Vec<AgentAction>> {
        let mut actions = Vec::new();

        let load = self.get_total_load(ctx);
        let dispatch = economic_dispatch(&self.generators, load);
        self.last_dispatch = Some(dispatch.clone());

        actions.push(AgentAction::LogMessage(format!(
            "Dispatch: load={:.1}MW, gen={:.1}MW, cost=${:.2}",
            dispatch.total_load_mw, dispatch.total_generation_mw, dispatch.total_cost
        )));

        Ok(actions)
    }

    async fn handle_emergency(&mut self, _event: &Event, _ctx: &AgentContext) -> Result<Vec<AgentAction>> {
        let mut actions = Vec::new();

        for gen in &self.generators {
            actions.push(AgentAction::EmergencyOverride {
                action: Box::new(AgentAction::ExecuteCommand(
                    Command::new(
                        CommandType::GeneratorSetpoint,
                        Self::gen_id_to_element_id(&gen.gen_id),
                        CommandPriority::Critical,
                        &self.id,
                    )
                    .with_parameter("P_MW", gen.p_max_mw)
                )),
                justification: "Emergency: maximize generation".to_string(),
            });
        }

        Ok(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_generators() -> Vec<GeneratorCostCurve> {
        vec![
            GeneratorCostCurve { gen_id: "1".to_string(), a: 0.001, b: 10.0, c: 100.0, p_min_mw: 20.0, p_max_mw: 200.0 },
            GeneratorCostCurve { gen_id: "2".to_string(), a: 0.002, b: 12.0, c: 80.0, p_min_mw: 10.0, p_max_mw: 150.0 },
            GeneratorCostCurve { gen_id: "3".to_string(), a: 0.0015, b: 11.0, c: 90.0, p_min_mw: 15.0, p_max_mw: 180.0 },
        ]
    }

    #[test]
    fn test_generator_cost_at() {
        let gen = &make_test_generators()[0];
        let cost = gen.cost_at(100.0);
        assert!(cost > 0.0);
    }

    #[test]
    fn test_generator_incremental_cost() {
        let gen = &make_test_generators()[0];
        let ic = gen.incremental_cost(100.0);
        assert!(ic > 0.0);
    }

    #[test]
    fn test_economic_dispatch_basic() {
        let gens = make_test_generators();
        let result = economic_dispatch(&gens, 300.0);
        assert!(result.total_generation_mw > 0.0);
        assert!(result.total_cost > 0.0);
        assert_eq!(result.gen_outputs.len(), 3);
    }

    #[test]
    fn test_economic_dispatch_respects_limits() {
        let gens = make_test_generators();
        let result = economic_dispatch(&gens, 500.0);
        for (gen_id, p) in &result.gen_outputs {
            let gen = gens.iter().find(|g| &g.gen_id == gen_id).unwrap();
            assert!(*p >= gen.p_min_mw);
            assert!(*p <= gen.p_max_mw);
        }
    }

    #[test]
    fn test_economic_dispatch_empty() {
        let result = economic_dispatch(&[], 100.0);
        assert!(result.gen_outputs.is_empty());
    }

    #[test]
    fn test_calculate_ace_normal() {
        let ace = calculate_ace(50.0, 50.0, 100.0);
        assert!(ace.abs() < 0.001);
    }

    #[test]
    fn test_calculate_ace_under_frequency() {
        let ace = calculate_ace(49.8, 50.0, 100.0);
        assert!(ace > 0.0);
    }

    #[test]
    fn test_calculate_ace_over_frequency() {
        let ace = calculate_ace(50.2, 50.0, 100.0);
        assert!(ace < 0.0);
    }

    #[test]
    fn test_dispatch_agent_new() {
        let agent = DispatchAgent::new("d1", "Dispatch-1", vec![1, 2]);
        assert_eq!(agent.id(), "d1");
        assert_eq!(agent.agent_type(), AgentType::Dispatcher);
        assert_eq!(agent.authority_level(), AuthorityLevel::Supervisor);
    }

    #[test]
    fn test_dispatch_agent_with_generators() {
        let agent = DispatchAgent::new("d1", "Dispatch-1", vec![1])
            .with_generators(make_test_generators());
        assert_eq!(agent.generators.len(), 3);
    }

    #[test]
    fn test_dispatch_agent_tick_interval() {
        let agent = DispatchAgent::new("d1", "Dispatch-1", vec![1]);
        assert_eq!(agent.tick_interval(), Duration::from_secs(5));
    }
}
