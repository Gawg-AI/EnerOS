use std::sync::Arc;

use eneros_agent::agent::{AgentAction, AgentType, MockAgent};
use eneros_agent::context::AgentContext;
use eneros_agent::dispatcher::DispatchResult;
use eneros_agent::event_adapter::AgentEventHandler;
use eneros_agent::orchestrator::AgentOrchestrator;
use eneros_constraint::projector::{FeasibilityProjector, NetworkSimulator, WhatIfResult};
use eneros_core::{AuthorityLevel, StructuredAction};
use eneros_eventbus::EventBus;
use eneros_gateway::constraint_validator::ConstraintAwareValidator;
use eneros_gateway::decision_pipeline::ConstrainedDecisionPipeline;
use eneros_gateway::SafetyGateway;
use eneros_memory::InMemoryMemory;
use eneros_network::PowerNetwork;
use eneros_reasoning::RuleBasedEngine;
use eneros_tool::ToolEngine;
use parking_lot::RwLock;

struct FeasibleSimulator;

impl NetworkSimulator for FeasibleSimulator {
    fn simulate_action(&self, _action: &StructuredAction) -> WhatIfResult {
        WhatIfResult {
            applicable: true,
            converged: true,
            voltage_violations: vec![],
            thermal_violations: vec![],
            all_constraints_satisfied: true,
            summary: "ok".to_string(),
        }
    }

    fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
        vec![(2, 0.0, 140.0)]
    }

    fn current_voltages(&self) -> Vec<(u64, f64)> {
        vec![(1, 1.0), (2, 1.0)]
    }
}

fn build_orchestrator(context_authority: AuthorityLevel) -> AgentOrchestrator {
    let event_bus = Arc::new(EventBus::new(64));
    let gateway = Arc::new(SafetyGateway::new(100));
    let tool_engine = Arc::new(RwLock::new(ToolEngine::new()));
    let network = Arc::new(RwLock::new(PowerNetwork::from_ieee14()));
    let memory = Arc::new(InMemoryMemory::default());
    let reasoning = Arc::new(RuleBasedEngine::new());

    let projector = Arc::new(FeasibilityProjector::new(Arc::new(FeasibleSimulator)));
    let validator = Arc::new(ConstraintAwareValidator::with_default_interlocking(
        Arc::new(eneros_constraint::ConstraintEngine::new()),
        gateway.clone(),
    ));
    let pipeline = Arc::new(ConstrainedDecisionPipeline::new(
        projector,
        validator,
        gateway.clone(),
    ));

    let mut ctx = AgentContext::new(event_bus, gateway, tool_engine, network, memory, reasoning);
    ctx.local.authority = context_authority;
    AgentOrchestrator::with_pipeline(ctx, pipeline)
}

fn start_generator_action() -> AgentAction {
    AgentAction::ExecuteStructured(StructuredAction::StartGenerator {
        gen_id: 2,
        target_mw: 40.0,
    })
}

#[tokio::test]
async fn supervisor_agent_executes_when_context_is_observer() {
    let mut orchestrator = build_orchestrator(AuthorityLevel::Observer);
    let agent = MockAgent::new("supervisor", "Supervisor", AgentType::Dispatcher)
        .with_authority_level(AuthorityLevel::Supervisor)
        .with_tick_actions(vec![start_generator_action()]);

    orchestrator.register_agent(AgentEventHandler::new_all_events(Box::new(agent)));

    let results = orchestrator.tick_all().await.unwrap();

    assert!(
        results
            .iter()
            .any(|result| matches!(result, DispatchResult::CommandExecuted)),
        "expected supervisor agent authority to execute through pipeline, got {:?}",
        results
    );
}

#[tokio::test]
async fn observer_agent_is_rejected_when_context_is_supervisor() {
    let mut orchestrator = build_orchestrator(AuthorityLevel::Supervisor);
    let agent = MockAgent::new("observer", "Observer", AgentType::Operator)
        .with_authority_level(AuthorityLevel::Observer)
        .with_tick_actions(vec![start_generator_action()]);

    orchestrator.register_agent(AgentEventHandler::new_all_events(Box::new(agent)));

    let results = orchestrator.tick_all().await.unwrap();

    assert!(
        results
            .iter()
            .any(|result| matches!(result, DispatchResult::ConstraintRejected(_))),
        "expected observer agent authority to be rejected by pipeline, got {:?}",
        results
    );
}
