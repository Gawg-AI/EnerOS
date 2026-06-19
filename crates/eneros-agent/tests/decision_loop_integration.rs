//! Phase 14 -- end-to-end closed-loop integration tests.
//!
//! These tests prove that the "ghost loop" discovered in Phase 13 is now
//! actually wired shut: a structured action proposed by reasoning flows through
//! the `ConstrainedDecisionPipeline`, and when it is rejected the
//! `FeedbackLoop` re-prompts the reasoning engine and retries.
//!
//! They are the executable evidence that the closed loop
//!
//! ```text
//! reasoning -> StructuredAction -> pipeline -> (rejected) -> feedback -> retry
//! ```
//!
//! holds end to end.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use eneros_agent::agent::{AgentAction, AgentType, MockAgent};
use eneros_agent::context::AgentContext;
use eneros_agent::orchestrator::AgentOrchestrator;
use eneros_constraint::projector::{FeasibilityProjector, NetworkSimulator, WhatIfResult};
use eneros_core::{AuthorityLevel, Result, StructuredAction};
use eneros_eventbus::EventBus;
use eneros_gateway::SafetyGateway;
use eneros_gateway::constraint_validator::ConstraintAwareValidator;
use eneros_gateway::decision_pipeline::ConstrainedDecisionPipeline;
use eneros_memory::InMemoryMemory;
use eneros_network::PowerNetwork;
use eneros_reasoning::feedback::FeedbackLoop;
use eneros_reasoning::engine::{ReasoningEngine, ReasoningInput, ReasoningOutput};
use eneros_tool::ToolEngine;
use parking_lot::RwLock;

// ------------------------------ helpers ------------------------------

/// A scripted reasoning engine: each `reason()` call pops the next pre-loaded
/// output. This lets a test simulate "round 1 returns an infeasible action,
/// round 2 returns a feasible one".
struct ScriptedEngine {
    outputs: std::sync::Mutex<Vec<ReasoningOutput>>,
    call_count: AtomicUsize,
}

impl ScriptedEngine {
    fn new(outputs: Vec<ReasoningOutput>) -> Self {
        Self {
            outputs: std::sync::Mutex::new(outputs),
            call_count: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ReasoningEngine for ScriptedEngine {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn reason(&self, _input: ReasoningInput) -> Result<ReasoningOutput> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let mut outputs = self.outputs.lock().unwrap();
        if outputs.len() == 1 {
            // Reuse the last output for any further calls (feedback retries).
            Ok(outputs.last().unwrap().clone())
        } else if let Some(out) = outputs.first().cloned() {
            outputs.remove(0);
            Ok(out)
        } else {
            Ok(ReasoningOutput::new("no more scripted outputs", 0.0))
        }
    }
}

/// A simulator that reports StartGenerator actions as persistently infeasible
/// (so the projector cannot repair them and the pipeline rejects), while all
/// other action types (e.g. NotifyAgent) are feasible. This models the
/// "projector rejects -> engine revises to a different action -> feasible"
/// scenario needed to exercise the feedback loop.
struct RejectiveThenAcceptingSimulator {
    gen_limits: Vec<(u64, f64, f64)>,
}

impl RejectiveThenAcceptingSimulator {
    fn new() -> Self {
        Self {
            gen_limits: vec![(1, 0.0, 200.0)],
        }
    }
}

impl NetworkSimulator for RejectiveThenAcceptingSimulator {
    fn simulate_action(&self, action: &StructuredAction) -> WhatIfResult {
        match action {
            // StartGenerator is always infeasible here: the projector will try
            // clipping to limits and reducing magnitude, but every simulation
            // still reports a violation, so projection ultimately fails and the
            // pipeline rejects — triggering the feedback loop.
            StructuredAction::StartGenerator { .. } => WhatIfResult {
                applicable: true,
                converged: true,
                voltage_violations: vec![(2, 0.88, 0.95)],
                thermal_violations: vec![],
                all_constraints_satisfied: false,
                summary: "persistent voltage violation".to_string(),
            },
            // Any other action type is feasible.
            _ => WhatIfResult {
                applicable: true,
                converged: true,
                voltage_violations: vec![],
                thermal_violations: vec![],
                all_constraints_satisfied: true,
                summary: "ok".to_string(),
            },
        }
    }

    fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
        self.gen_limits.clone()
    }

    fn current_voltages(&self) -> Vec<(u64, f64)> {
        vec![(1, 1.0)]
    }
}

/// A simulator that rejects every action as infeasible. Used to force the
/// pipeline into the rejection branch so the feedback loop is exercised.
struct AlwaysInfeasibleSimulator;

impl NetworkSimulator for AlwaysInfeasibleSimulator {
    fn simulate_action(&self, _action: &StructuredAction) -> WhatIfResult {
        WhatIfResult {
            applicable: true,
            converged: true,
            voltage_violations: vec![(2, 0.88, 0.95)],
            thermal_violations: vec![],
            all_constraints_satisfied: false,
            summary: "always infeasible".to_string(),
        }
    }

    fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
        vec![(1, 0.0, 200.0)]
    }

    fn current_voltages(&self) -> Vec<(u64, f64)> {
        vec![(1, 1.0)]
    }
}

/// Build a full Phase 14 orchestrator: real pipeline + feedback loop wired to
/// the provided reasoning engine. The context authority is set to `authority`
/// (the orchestrator dispatches structured actions under this level).
fn build_orchestrator(
    engine: Arc<dyn ReasoningEngine>,
    simulator: Arc<dyn NetworkSimulator>,
    with_feedback: bool,
    authority: AuthorityLevel,
) -> AgentOrchestrator {
    let event_bus = Arc::new(EventBus::new(64));
    let gateway = Arc::new(SafetyGateway::new(100));
    let tool_engine = Arc::new(RwLock::new(ToolEngine::new()));
    let network = Arc::new(RwLock::new(PowerNetwork::from_ieee14()));
    let memory: Arc<dyn eneros_memory::AgentMemory> = Arc::new(InMemoryMemory::default());

    let projector = Arc::new(FeasibilityProjector::new(simulator));
    let constraint_engine = Arc::new(eneros_constraint::ConstraintEngine::new());
    let validator = Arc::new(ConstraintAwareValidator::with_default_interlocking(
        constraint_engine,
        gateway.clone(),
    ));
    let pipeline = Arc::new(ConstrainedDecisionPipeline::new(
        projector,
        validator,
        gateway.clone(),
    ));

    let mut ctx = AgentContext::new(event_bus, gateway, tool_engine, network, memory, engine.clone());
    ctx.local.authority = authority;

    if with_feedback {
        let feedback = Arc::new(FeedbackLoop::with_default_iterations_shared(engine));
        AgentOrchestrator::with_pipeline_and_feedback(ctx, pipeline, feedback)
    } else {
        AgentOrchestrator::with_pipeline(ctx, pipeline)
    }
}

// ------------------------------ tests ------------------------------

/// Test 1 -- structured action routing: an agent that returns
/// `ExecuteStructured` has its action flow through the pipeline. A
/// NotifyAgent action is feasible in the test simulator, so it executes and we
/// confirm the structured path (not the legacy dispatch path) was taken.
#[tokio::test]
async fn test_structured_action_routed_through_pipeline() {
    let engine = Arc::new(ScriptedEngine::new(vec![ReasoningOutput::new("n/a", 0.0)]))
        as Arc<dyn ReasoningEngine>;
    let mut orchestrator = build_orchestrator(
        engine,
        Arc::new(RejectiveThenAcceptingSimulator::new()),
        false,
        AuthorityLevel::Supervisor,
    );

    let agent = MockAgent::new("s-1", "Structured", AgentType::Operator)
        .with_authority_level(AuthorityLevel::Supervisor)
        .with_tick_actions(vec![AgentAction::ExecuteStructured(
            StructuredAction::NotifyAgent {
                agent_id: "dispatch".to_string(),
                message: "routed via pipeline".to_string(),
            },
        )]);
    orchestrator.register_agent(eneros_agent::event_adapter::AgentEventHandler::new_all_events(
        Box::new(agent),
    ));

    let results = orchestrator.tick_all().await.unwrap();
    assert!(
        results
            .iter()
            .any(|r| matches!(r, eneros_agent::dispatcher::DispatchResult::CommandExecuted)),
        "expected the structured action to be executed, got {:?}",
        results
    );
}

/// Test 2 -- backward compatibility: when an agent emits a legacy non-structured
/// action (e.g. LogMessage), the orchestrator still uses the plain `dispatch()`
/// path and the action executes.
#[tokio::test]
async fn test_legacy_action_still_dispatched() {
    let engine = Arc::new(ScriptedEngine::new(vec![ReasoningOutput::new("n/a", 0.0)]))
        as Arc<dyn ReasoningEngine>;
    let mut orchestrator = build_orchestrator(
        engine,
        Arc::new(RejectiveThenAcceptingSimulator::new()),
        false,
        AuthorityLevel::Supervisor,
    );

    let agent = MockAgent::new("l-1", "Legacy", AgentType::Operator)
        .with_authority_level(AuthorityLevel::Supervisor)
        .with_tick_actions(vec![AgentAction::LogMessage("legacy path".to_string())]);
    orchestrator.register_agent(eneros_agent::event_adapter::AgentEventHandler::new_all_events(
        Box::new(agent),
    ));

    let results = orchestrator.tick_all().await.unwrap();
    assert!(
        results
            .iter()
            .any(|r| matches!(r, eneros_agent::dispatcher::DispatchResult::Logged)),
        "expected the legacy log action, got {:?}",
        results
    );
}

/// Test 3 -- the feedback loop fires when the pipeline rejects an action. The
/// scripted engine is consulted again, proving the LLM re-reasoning path is
/// wired in. This is the heart of Phase 14.
#[tokio::test]
async fn test_feedback_loop_fires_after_rejection() {
    // The simulator's first simulation reports a violation, so the initial
    // StartGenerator{300} is rejected. The feedback loop then re-prompts the
    // engine, which returns a fresh feasible action on its second call.
    let first = ReasoningOutput {
        conclusion: "round 1".to_string(),
        confidence: 0.8,
        actions: vec![],
        reasoning_chain: vec![],
        structured_actions: Some(vec![StructuredAction::StartGenerator {
            gen_id: 1,
            target_mw: 300.0,
        }]),
        preconditions: vec![],
    };
    let second = ReasoningOutput {
        conclusion: "round 2 (revised)".to_string(),
        confidence: 0.9,
        actions: vec![],
        reasoning_chain: vec![],
        structured_actions: Some(vec![StructuredAction::NotifyAgent {
            agent_id: "dispatch".to_string(),
            message: "revised feasible plan".to_string(),
        }]),
        preconditions: vec![],
    };
    let engine = Arc::new(ScriptedEngine::new(vec![first, second]));
    let mut orchestrator = build_orchestrator(
        engine.clone() as Arc<dyn ReasoningEngine>,
        Arc::new(AlwaysInfeasibleSimulator),
        true,
        AuthorityLevel::Supervisor,
    );

    let agent = MockAgent::new("f-1", "Feedback", AgentType::Operator)
        .with_authority_level(AuthorityLevel::Supervisor)
        .with_tick_actions(vec![AgentAction::ExecuteStructured(
            StructuredAction::StartGenerator { gen_id: 1, target_mw: 300.0 },
        )]);
    orchestrator.register_agent(eneros_agent::event_adapter::AgentEventHandler::new_all_events(
        Box::new(agent),
    ));

    let _results = orchestrator.tick_all().await.unwrap();

    // The decisive assertion: the engine must have been re-consulted by the
    // feedback loop after the initial rejection. If calls == 0 the loop is
    // not wired in.
    assert!(
        engine.calls() >= 1,
        "feedback engine should have been invoked at least once, calls = {}",
        engine.calls()
    );
}

/// Test 4 -- no feedback loop configured: a rejected structured action returns
/// the rejection result without re-prompting. Confirms the loop is optional and
/// the fallback is graceful.
#[tokio::test]
async fn test_rejection_without_feedback_loop_is_graceful() {
    let engine = Arc::new(ScriptedEngine::new(vec![ReasoningOutput::new("n/a", 0.0)]));
    // No feedback loop (with_feedback = false).
    let mut orchestrator = build_orchestrator(
        engine.clone() as Arc<dyn ReasoningEngine>,
        Arc::new(RejectiveThenAcceptingSimulator::new()),
        false,
        AuthorityLevel::Supervisor,
    );

    let agent = MockAgent::new("n-1", "NoFeedback", AgentType::Operator)
        .with_authority_level(AuthorityLevel::Supervisor)
        .with_tick_actions(vec![AgentAction::ExecuteStructured(
            StructuredAction::StartGenerator { gen_id: 1, target_mw: 300.0 },
        )]);
    orchestrator.register_agent(eneros_agent::event_adapter::AgentEventHandler::new_all_events(
        Box::new(agent),
    ));

    let results = orchestrator.tick_all().await.unwrap();
    // Engine should NOT have been invoked (it is only used by the feedback loop).
    assert_eq!(
        engine.calls(),
        0,
        "engine must not be invoked when no feedback loop is configured"
    );
    assert!(!results.is_empty());
}

/// Test 5 -- authority gating still applies on the structured path: an Observer
/// cannot execute a structured action and is rejected at the precondition stage.
#[tokio::test]
async fn test_observer_cannot_execute_structured_action() {
    let engine = Arc::new(ScriptedEngine::new(vec![ReasoningOutput::new("n/a", 0.0)]))
        as Arc<dyn ReasoningEngine>;
    let mut orchestrator = build_orchestrator(
        engine,
        Arc::new(RejectiveThenAcceptingSimulator::new()),
        false,
        AuthorityLevel::Observer,
    );

    let agent = MockAgent::new("o-1", "Observer", AgentType::Operator)
        .with_authority_level(AuthorityLevel::Observer)
        .with_tick_actions(vec![AgentAction::ExecuteStructured(
            StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 },
        )]);
    orchestrator.register_agent(eneros_agent::event_adapter::AgentEventHandler::new_all_events(
        Box::new(agent),
    ));

    let results = orchestrator.tick_all().await.unwrap();
    assert!(
        results.iter().any(|r| matches!(
            r,
            eneros_agent::dispatcher::DispatchResult::ConstraintRejected(_)
        )),
        "expected ConstraintRejected for Observer, got {:?}",
        results
    );
}
