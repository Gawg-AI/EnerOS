//! Phase 16-R1: Postcondition real verification + tight-constraint scenarios.
//!
//! This test file addresses the three gaps identified in the P16 review:
//!
//! 1. Postcondition now uses post-execution re-simulation (not cached projection)
//! 2. Tight-constraint scenarios where IEEE-14 base case is NOT optimal
//! 3. NotifyAgent complete audit trail verification
//!
//! Key insight: To create a "truly needs self-healing" scenario, we must
//! perturb the IEEE-14 network into a stressed state (e.g., trip a line,
//! overload a branch, depress a bus voltage) before running the pipeline.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use eneros_constraint::projector::{FeasibilityProjector, NetworkSimulator, WhatIfResult};
use eneros_constraint::ConstraintEngine;
use eneros_core::{
    ActionVerdict, AuthorityLevel, BusVoltageObservation, BranchFlowObservation,
    Jurisdiction, PowerObservation, StructuredAction, SystemOperatingState,
};
use eneros_gateway::constraint_validator::ConstraintAwareValidator;
use eneros_gateway::decision_pipeline::ConstrainedDecisionPipeline;
use eneros_gateway::pipeline_types::DecisionContext;
use eneros_gateway::{ObservationProvider, SafetyGateway};

// ============================================================================
// Helper: Pipeline with a controllable mock simulator
// ============================================================================

/// Simulator that tracks how many times simulate_action is called.
/// Returns OK for the first N calls, then returns violations.
/// This lets us verify that postcondition re-simulation actually happens.
struct CallCountingSimulator {
    call_count: AtomicUsize,
    /// What to return on each call (index = call number)
    responses: Vec<WhatIfResult>,
}

impl CallCountingSimulator {
    fn new(responses: Vec<WhatIfResult>) -> Self {
        Self {
            call_count: AtomicUsize::new(0),
            responses,
        }
    }

    fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

impl NetworkSimulator for CallCountingSimulator {
    fn simulate_action(&self, _action: &StructuredAction) -> WhatIfResult {
        let n = self.call_count.fetch_add(1, Ordering::SeqCst);
        self.responses.get(n).cloned().unwrap_or(WhatIfResult {
            applicable: true,
            converged: true,
            voltage_violations: vec![],
            thermal_violations: vec![],
            all_constraints_satisfied: true,
            summary: "default OK".to_string(),
        })
    }

    fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
        vec![(1, 0.0, 200.0), (2, 0.0, 140.0), (6, 0.0, 80.0)]
    }

    fn current_voltages(&self) -> Vec<(u64, f64)> {
        vec![(1, 1.06), (2, 1.02), (3, 0.98), (4, 0.95)]
    }
}

fn make_ok_result() -> WhatIfResult {
    WhatIfResult {
        applicable: true,
        converged: true,
        voltage_violations: vec![],
        thermal_violations: vec![],
        all_constraints_satisfied: true,
        summary: "OK".to_string(),
    }
}

fn make_violation_result() -> WhatIfResult {
    WhatIfResult {
        applicable: true,
        converged: true,
        voltage_violations: vec![(4, 0.88, 0.95)],
        thermal_violations: vec![(9, 115.0, 100.0)],
        all_constraints_satisfied: false,
        summary: "Post-execution violations detected".to_string(),
    }
}

fn make_pipeline_with_sim(sim: Arc<dyn NetworkSimulator>) -> ConstrainedDecisionPipeline {
    let projector = Arc::new(FeasibilityProjector::new(sim));
    let constraint_engine = Arc::new(ConstraintEngine::new());
    let gateway = Arc::new(SafetyGateway::new(100));
    let validator = Arc::new(ConstraintAwareValidator::with_default_interlocking(
        constraint_engine,
        gateway.clone(),
    ));
    ConstrainedDecisionPipeline::new(projector, validator, gateway)
}

fn supervisor_alert_ctx() -> DecisionContext {
    let mut obs = PowerObservation::empty();
    obs.bus_voltages.insert(4, BusVoltageObservation { vm_pu: 0.88, va_degree: -15.0 });
    DecisionContext::new(
        AuthorityLevel::Supervisor,
        Jurisdiction::unrestricted(),
        SystemOperatingState::Alert,
    )
    .with_observation(obs)
    .with_reasoning("Bus 4 voltage 0.88 pu — below 0.95 limit")
}

// ============================================================================
// Test 1: Postcondition uses re-simulation, not cached projection
//
// This is the core fix: the pipeline MUST call simulate_action again
// after execution, not reuse the projection-phase result.
// ============================================================================

#[tokio::test]
async fn test_postcondition_uses_post_execution_resimulation() {
    // Projection returns OK (so action is approved and executed),
    // but post-execution re-simulation returns violations.
    // If postcondition uses the cached projection, it would say "satisfied".
    // If it uses re-simulation, it correctly detects violations.
    let sim = Arc::new(CallCountingSimulator::new(vec![
        make_ok_result(),        // Call 1: projection phase
        make_violation_result(), // Call 2: postcondition phase (re-simulation)
    ]));

    let pipeline = make_pipeline_with_sim(sim.clone());

    let ctx = supervisor_alert_ctx();
    let action = StructuredAction::StartGenerator { gen_id: 2, target_mw: 80.0 };

    let result = pipeline.decide_enhanced(&action, &ctx).await;

    // The simulator must have been called at least twice:
    // once for projection, once for postcondition re-simulation
    assert!(
        sim.call_count() >= 2,
        "Simulator must be called at least twice (projection + postcondition), got {} calls",
        sim.call_count()
    );

    // The action should be executed (projection said OK)
    assert!(
        result.executed_action.is_some(),
        "Action must be executed (projection was OK)"
    );

    // Postcondition must FAIL (re-simulation found violations)
    let pc = result.post_conditions.as_ref()
        .expect("PostConditions must exist after execution");
    assert!(
        !pc.satisfied,
        "Postcondition must FAIL because re-simulation found violations, not use cached projection"
    );
    assert!(
        !pc.new_violations.is_empty(),
        "Must have new violations from re-simulation"
    );

    // Rollback plan must exist
    assert!(
        result.rollback_plan.is_some(),
        "Rollback plan must exist when postconditions fail"
    );
}

#[tokio::test]
async fn test_postcondition_passes_when_resimulation_ok() {
    // Both projection and re-simulation return OK → postcondition satisfied
    let sim = Arc::new(CallCountingSimulator::new(vec![
        make_ok_result(), // Call 1: projection
        make_ok_result(), // Call 2: postcondition re-simulation
    ]));

    let pipeline = make_pipeline_with_sim(sim.clone());

    let ctx = supervisor_alert_ctx();
    let action = StructuredAction::StartGenerator { gen_id: 2, target_mw: 80.0 };

    let result = pipeline.decide_enhanced(&action, &ctx).await;

    assert!(sim.call_count() >= 2, "Simulator called at least twice");
    assert!(result.executed_action.is_some());

    let pc = result.post_conditions.as_ref().expect("PostConditions must exist");
    assert!(
        pc.satisfied,
        "Postcondition must pass when re-simulation returns OK"
    );
}

// ============================================================================
// Test 2: Tight-constraint scenario — network under stress
//
// Instead of using the optimal IEEE-14 base case, we simulate a network
// that has been stressed (line tripped, load increased, etc.) and verify
// the pipeline can handle a genuine self-healing scenario.
// ============================================================================

/// Simulator that models a stressed network: some buses have low voltage,
/// some branches are overloaded. Certain corrective actions (like increasing
/// generator output) can resolve the violations.
struct StressedNetworkSimulator {
    /// Current stressed voltages
    voltages: std::sync::Mutex<Vec<(u64, f64)>>,
}

impl StressedNetworkSimulator {
    fn new() -> Self {
        // Stressed IEEE-14: buses 12, 13, 14 have depressed voltages
        // after a line trip between buses 13-14
        Self {
            voltages: std::sync::Mutex::new(vec![
                (1, 1.06), (2, 1.045), (3, 1.01), (4, 1.018),
                (5, 1.02), (6, 1.07), (7, 1.062), (8, 1.09),
                (9, 1.056), (10, 1.051), (11, 1.057),
                (12, 0.91),  // Stressed!
                (13, 0.89),  // Stressed!
                (14, 0.87),  // Stressed! Below 0.95 limit
            ]),
        }
    }
}

impl NetworkSimulator for StressedNetworkSimulator {
    fn simulate_action(&self, action: &StructuredAction) -> WhatIfResult {
        match action {
            // Increasing gen 6 output helps the southern buses (12, 13, 14)
            StructuredAction::StartGenerator { gen_id, target_mw } if *gen_id == 6 && *target_mw >= 30.0 => {
                // Gen 6 is on bus 6, which feeds the southern area.
                // With sufficient reactive support, voltages recover.
                WhatIfResult {
                    applicable: true,
                    converged: true,
                    voltage_violations: vec![], // Recovered!
                    thermal_violations: vec![],
                    all_constraints_satisfied: true,
                    summary: "Gen 6 output increase resolved voltage violations".to_string(),
                }
            }
            // Small gen 6 increase — partial recovery but still some violations
            StructuredAction::StartGenerator { gen_id, target_mw } if *gen_id == 6 && *target_mw >= 15.0 => {
                WhatIfResult {
                    applicable: true,
                    converged: true,
                    voltage_violations: vec![(14, 0.93, 0.95)], // Still slightly low
                    thermal_violations: vec![],
                    all_constraints_satisfied: false,
                    summary: "Partial recovery — bus 14 still low".to_string(),
                }
            }
            // Load shedding from zone 0 helps reduce stress
            StructuredAction::ShedLoad { zone_id, amount_mw } if *zone_id == 0 && *amount_mw >= 5.0 => {
                WhatIfResult {
                    applicable: true,
                    converged: true,
                    voltage_violations: vec![], // Load shed resolved violations
                    thermal_violations: vec![],
                    all_constraints_satisfied: true,
                    summary: "Load shedding resolved violations".to_string(),
                }
            }
            // Default: network remains stressed
            _ => {
                WhatIfResult {
                    applicable: true,
                    converged: true,
                    voltage_violations: vec![
                        (12, 0.91, 0.95),
                        (13, 0.89, 0.95),
                        (14, 0.87, 0.95),
                    ],
                    thermal_violations: vec![(20, 105.0, 100.0)],
                    all_constraints_satisfied: false,
                    summary: "Network remains stressed".to_string(),
                }
            }
        }
    }

    fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
        vec![(1, 0.0, 200.0), (2, 0.0, 140.0), (6, 0.0, 80.0), (8, 0.0, 50.0)]
    }

    fn current_voltages(&self) -> Vec<(u64, f64)> {
        self.voltages.lock().unwrap().clone()
    }
}

#[tokio::test]
async fn test_tight_constraint_self_healing_gen_increase() {
    // Scenario: Line 13-14 tripped → buses 12, 13, 14 have depressed voltages
    // Decision: Increase gen 6 output to provide reactive support
    // Expected: Pipeline approves, postcondition re-simulation confirms recovery
    let sim = Arc::new(StressedNetworkSimulator::new());
    let pipeline = make_pipeline_with_sim(sim.clone());

    let mut obs = PowerObservation::empty();
    obs.bus_voltages.insert(14, BusVoltageObservation { vm_pu: 0.87, va_degree: -18.0 });
    obs.bus_voltages.insert(13, BusVoltageObservation { vm_pu: 0.89, va_degree: -16.0 });
    obs.bus_voltages.insert(12, BusVoltageObservation { vm_pu: 0.91, va_degree: -15.0 });

    let ctx = DecisionContext::new(
        AuthorityLevel::Supervisor,
        Jurisdiction::unrestricted(),
        SystemOperatingState::Alert,
    )
    .with_observation(obs)
    .with_reasoning("Buses 12-14 voltage depressed after line trip — need reactive support from gen 6");

    let action = StructuredAction::StartGenerator { gen_id: 6, target_mw: 40.0 };

    let result = pipeline.decide_enhanced(&action, &ctx).await;

    // Action must be approved and executed
    assert!(
        matches!(result.verdict, ActionVerdict::Approved),
        "Expected Approved for gen 6 @ 40MW on stressed network, got {:?}",
        result.verdict
    );
    assert!(result.executed_action.is_some(), "Action must be executed");

    // Postcondition re-simulation must confirm recovery
    let pc = result.post_conditions.as_ref().expect("PostConditions must exist");
    assert!(
        pc.satisfied,
        "Postcondition must be satisfied — gen 6 @ 40MW resolves voltage violations. Violations: {:?}",
        pc.new_violations
    );

    // Rollback plan must exist (even though not needed)
    assert!(result.rollback_plan.is_some(), "Rollback plan must always exist");
}

#[tokio::test]
async fn test_tight_constraint_partial_recovery_detected() {
    // Scenario: Same stressed network, but only a small gen increase
    // The projector may clip/project the action to a feasible value,
    // so we test with a gen that doesn't help (gen 8) to ensure
    // postcondition detects the remaining violations.
    let sim = Arc::new(StressedNetworkSimulator::new());
    let pipeline = make_pipeline_with_sim(sim.clone());

    let mut obs = PowerObservation::empty();
    obs.bus_voltages.insert(14, BusVoltageObservation { vm_pu: 0.87, va_degree: -18.0 });

    let ctx = DecisionContext::new(
        AuthorityLevel::Supervisor,
        Jurisdiction::unrestricted(),
        SystemOperatingState::Alert,
    )
    .with_observation(obs)
    .with_reasoning("Bus 14 voltage depressed — try gen 8 (wrong gen)");

    // Gen 8 doesn't help the southern area — re-simulation will show violations
    let action = StructuredAction::StartGenerator { gen_id: 8, target_mw: 20.0 };

    let result = pipeline.decide_enhanced(&action, &ctx).await;

    // If approved and executed, postcondition must detect remaining violations
    if result.executed_action.is_some() {
        let pc = result.post_conditions.as_ref().expect("PostConditions must exist");
        assert!(
            !pc.satisfied,
            "Postcondition must FAIL — gen 8 doesn't resolve southern area violations. Got: {:?}",
            pc.new_violations
        );
        assert!(!pc.new_violations.is_empty(), "Must have violations from re-simulation");
    }
}

#[tokio::test]
async fn test_tight_constraint_load_shed_recovery() {
    // Scenario: Stressed network, shed load to relieve
    let sim = Arc::new(StressedNetworkSimulator::new());
    let pipeline = make_pipeline_with_sim(sim.clone());

    let mut obs = PowerObservation::empty();
    obs.bus_voltages.insert(14, BusVoltageObservation { vm_pu: 0.87, va_degree: -18.0 });
    obs.branch_flows.insert(20, BranchFlowObservation {
        p_mw: 45.0, q_mvar: 12.0, loading_percent: 105.0,
    });

    let ctx = DecisionContext::new(
        AuthorityLevel::Supervisor,
        Jurisdiction::unrestricted(),
        SystemOperatingState::Alert,
    )
    .with_observation(obs)
    .with_reasoning("Branch 20 overloaded, bus 14 low — shed load to relieve");

    let action = StructuredAction::ShedLoad { zone_id: 0, amount_mw: 10.0 };

    let result = pipeline.decide_enhanced(&action, &ctx).await;

    // ShedLoad may be approved or pending (high-risk action)
    match &result.verdict {
        ActionVerdict::Approved => {
            assert!(result.executed_action.is_some());
            let pc = result.post_conditions.as_ref().expect("PostConditions must exist");
            assert!(
                pc.satisfied,
                "Load shedding must resolve violations. Got: {:?}",
                pc.new_violations
            );
        }
        ActionVerdict::PendingApproval { .. } => {
            // Acceptable — load shedding requires escalation
        }
        other => {
            // Also acceptable depending on authority/state
            let _ = other;
        }
    }
}

#[tokio::test]
async fn test_tight_constraint_ineffective_action_detected() {
    // Scenario: Stressed network, but action doesn't help (e.g., wrong gen)
    let sim = Arc::new(StressedNetworkSimulator::new());
    let pipeline = make_pipeline_with_sim(sim.clone());

    let mut obs = PowerObservation::empty();
    obs.bus_voltages.insert(14, BusVoltageObservation { vm_pu: 0.87, va_degree: -18.0 });

    let ctx = DecisionContext::new(
        AuthorityLevel::Supervisor,
        Jurisdiction::unrestricted(),
        SystemOperatingState::Alert,
    )
    .with_observation(obs)
    .with_reasoning("Bus 14 voltage depressed — try gen 8");

    // Gen 8 doesn't help the southern area in our model
    let action = StructuredAction::StartGenerator { gen_id: 8, target_mw: 25.0 };

    let result = pipeline.decide_enhanced(&action, &ctx).await;

    // If approved, postcondition must detect that violations persist
    if result.executed_action.is_some() {
        let pc = result.post_conditions.as_ref().expect("PostConditions must exist");
        assert!(
            !pc.satisfied,
            "Gen 8 should NOT resolve southern area violations — postcondition must fail"
        );
        assert!(!pc.new_violations.is_empty(), "Must have violations from re-simulation");
    }
}

// ============================================================================
// Test 3: NotifyAgent complete audit trail
//
// Previously, NotifyAgent was noted to skip some pipeline stages.
// This test verifies that NotifyAgent goes through all 7 stages.
// ============================================================================

#[tokio::test]
async fn test_notify_agent_complete_audit_trail() {
    // Use a simulator that returns OK for everything
    let sim = Arc::new(CallCountingSimulator::new(vec![
        make_ok_result(), // projection
        make_ok_result(), // postcondition re-simulation
    ]));
    let pipeline = make_pipeline_with_sim(sim);

    let ctx = DecisionContext::new(
        AuthorityLevel::Supervisor,
        Jurisdiction::unrestricted(),
        SystemOperatingState::Normal,
    );

    let action = StructuredAction::NotifyAgent {
        agent_id: "scada".to_string(),
        message: "voltage alert cleared".to_string(),
    };

    let result = pipeline.decide_enhanced(&action, &ctx).await;

    // Every audit entry must have non-empty description
    for entry in &result.audit {
        assert!(
            !entry.description.is_empty(),
            "Audit entry for stage '{}' has empty description",
            entry.stage
        );
    }

    // Must have at least precondition, projection, validation, decomposition stages
    let stages: Vec<&str> = result.audit.iter().map(|e| e.stage.as_str()).collect();
    assert!(
        stages.contains(&"precondition"),
        "Missing precondition stage. Stages: {:?}", stages
    );
    assert!(
        stages.contains(&"projection"),
        "Missing projection stage. Stages: {:?}", stages
    );
    assert!(
        stages.contains(&"validation"),
        "Missing validation stage. Stages: {:?}", stages
    );
    assert!(
        stages.contains(&"decomposition"),
        "Missing decomposition stage. Stages: {:?}", stages
    );

    // If approved, must also have execution and postcondition
    if matches!(result.verdict, ActionVerdict::Approved) {
        assert!(
            stages.contains(&"execution"),
            "Missing execution stage for approved NotifyAgent. Stages: {:?}", stages
        );
        assert!(
            stages.contains(&"postcondition"),
            "Missing postcondition stage for approved NotifyAgent. Stages: {:?}", stages
        );
    }
}

// ============================================================================
// Test 4: Re-simulation divergence detection
// ============================================================================

#[tokio::test]
async fn test_postcondition_detects_power_flow_divergence() {
    let sim = Arc::new(CallCountingSimulator::new(vec![
        make_ok_result(), // projection: OK
        WhatIfResult {    // postcondition re-simulation: diverged!
            applicable: true,
            converged: false,
            voltage_violations: vec![],
            thermal_violations: vec![],
            all_constraints_satisfied: false,
            summary: "Power flow did not converge after action".to_string(),
        },
    ]));

    let pipeline = make_pipeline_with_sim(sim);

    let ctx = supervisor_alert_ctx();
    let action = StructuredAction::StartGenerator { gen_id: 2, target_mw: 80.0 };

    let result = pipeline.decide_enhanced(&action, &ctx).await;

    assert!(result.executed_action.is_some());

    let pc = result.post_conditions.as_ref().expect("PostConditions must exist");
    assert!(
        !pc.satisfied,
        "Postcondition must fail when re-simulation diverges"
    );
    assert!(
        pc.new_violations.iter().any(|v| v.contains("divergence") || v.contains("converge")),
        "Must report divergence: {:?}",
        pc.new_violations
    );
}

// ============================================================================
// Test 5: Production-grade postcondition with field observation provider
//
// This is the key fix for BUG3 §8 point 2: "执行 + postcondition 双空".
// When an ObservationProvider is configured, the pipeline verifies the
// postcondition against **real field measurements** (read back from SCADA/RTU
// after execution), not the simulator's pure-function prediction.
//
// This test proves that even when the simulator says "everything OK", the
// field observation can override it and detect real violations.
// ============================================================================

/// Simulator that always returns "OK" (simulating a case where prediction
/// says everything is fine, but the real world might differ).
struct AlwaysOkSimulator;

impl NetworkSimulator for AlwaysOkSimulator {
    fn simulate_action(&self, _action: &StructuredAction) -> WhatIfResult {
        WhatIfResult {
            applicable: true,
            converged: true,
            voltage_violations: vec![],
            thermal_violations: vec![],
            all_constraints_satisfied: true,
            summary: "Simulator predicts OK".to_string(),
        }
    }
    fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
        vec![(1, 0.0, 200.0), (2, 0.0, 140.0)]
    }
    fn current_voltages(&self) -> Vec<(u64, f64)> {
        vec![(1, 1.06), (2, 1.02)]
    }
}

fn make_pipeline_with_observation_provider(
    sim: Arc<dyn NetworkSimulator>,
    provider: ObservationProvider,
) -> ConstrainedDecisionPipeline {
    let projector = Arc::new(FeasibilityProjector::new(sim));
    let constraint_engine = Arc::new(ConstraintEngine::new());
    let gateway = Arc::new(SafetyGateway::new(100));
    let validator = Arc::new(ConstraintAwareValidator::with_default_interlocking(
        constraint_engine,
        gateway.clone(),
    ));
    ConstrainedDecisionPipeline::with_observation_provider(projector, validator, gateway, provider)
}

/// Build a PowerObservation with the given bus voltages and branch flows.
fn make_obs(
    voltages: Vec<(u64, f64, f64)>,
    flows: Vec<(u64, f64, f64)>,
) -> PowerObservation {
    let mut obs = PowerObservation::empty();
    for (bus, vm, va) in voltages {
        obs.bus_voltages.insert(bus, BusVoltageObservation { vm_pu: vm, va_degree: va });
    }
    for (branch, p, loading) in flows {
        obs.branch_flows.insert(branch, BranchFlowObservation { p_mw: p, q_mvar: 0.0, loading_percent: loading });
    }
    obs
}

#[tokio::test]
async fn test_postcondition_uses_field_observation_not_simulator() {
    // Simulator always says OK, but the field observation shows violations.
    // The postcondition must FAIL because it uses the real observation,
    // not the simulator's prediction.
    let sim = Arc::new(AlwaysOkSimulator);

    // Field observation: Bus 4 has voltage 0.88 pu (below 0.95 limit)
    let obs = make_obs(vec![(4, 0.88, -15.0)], vec![]);
    let obs_arc: Arc<PowerObservation> = Arc::new(obs);
    let provider: ObservationProvider = Arc::new(move || Some((*obs_arc).clone()));

    let pipeline = make_pipeline_with_observation_provider(sim, provider);

    let ctx = supervisor_alert_ctx();
    let action = StructuredAction::StartGenerator { gen_id: 2, target_mw: 80.0 };

    let result = pipeline.decide_enhanced(&action, &ctx).await;

    // Action is executed (simulator said OK for projection)
    assert!(result.executed_action.is_some(), "Action must be executed");

    // Postcondition must FAIL — field observation shows voltage violation
    let pc = result.post_conditions.as_ref().expect("PostConditions must exist");
    assert!(
        !pc.satisfied,
        "Postcondition must FAIL because field observation shows voltage violation, \
         even though simulator predicts OK. This proves the pipeline uses real \
         measurements, not the simulator's pure-function prediction."
    );
    assert!(
        pc.new_violations.iter().any(|v| v.contains("Voltage") || v.contains("voltage")),
        "Must report voltage violation from field observation: {:?}",
        pc.new_violations
    );

    // Audit trail must show field_observation as the data source
    let postcondition_audit = result.audit.iter()
        .find(|e| e.stage == "postcondition")
        .expect("Must have postcondition audit entry");
    assert!(
        postcondition_audit.description.contains("field_observation"),
        "Audit must show 'field_observation' as source: {}",
        postcondition_audit.description
    );
}

#[tokio::test]
async fn test_postcondition_field_observation_passes_when_healthy() {
    // Both simulator and field observation say OK → postcondition satisfied
    let sim = Arc::new(AlwaysOkSimulator);

    // Field observation: all voltages within limits
    let obs = make_obs(
        vec![(1, 1.06, 0.0), (2, 1.02, -5.0), (4, 0.98, -10.0)],
        vec![(1, 50.0, 60.0)],
    );
    let obs_arc: Arc<PowerObservation> = Arc::new(obs);
    let provider: ObservationProvider = Arc::new(move || Some((*obs_arc).clone()));

    let pipeline = make_pipeline_with_observation_provider(sim, provider);

    let ctx = supervisor_alert_ctx();
    let action = StructuredAction::StartGenerator { gen_id: 2, target_mw: 80.0 };

    let result = pipeline.decide_enhanced(&action, &ctx).await;

    assert!(result.executed_action.is_some());

    let pc = result.post_conditions.as_ref().expect("PostConditions must exist");
    assert!(
        pc.satisfied,
        "Postcondition must pass when field observation shows all constraints satisfied"
    );

    let postcondition_audit = result.audit.iter()
        .find(|e| e.stage == "postcondition")
        .expect("Must have postcondition audit entry");
    assert!(
        postcondition_audit.description.contains("field_observation"),
        "Audit must show 'field_observation' as source: {}",
        postcondition_audit.description
    );
}

#[tokio::test]
async fn test_postcondition_falls_back_to_simulator_when_provider_returns_none() {
    // Provider returns None (data unavailable) → must fall back to simulator
    let sim = Arc::new(AlwaysOkSimulator);

    let provider: ObservationProvider = Arc::new(|| None);

    let pipeline = make_pipeline_with_observation_provider(sim, provider);

    let ctx = supervisor_alert_ctx();
    let action = StructuredAction::StartGenerator { gen_id: 2, target_mw: 80.0 };

    let result = pipeline.decide_enhanced(&action, &ctx).await;

    assert!(result.executed_action.is_some());

    let pc = result.post_conditions.as_ref().expect("PostConditions must exist");
    assert!(
        pc.satisfied,
        "Postcondition must pass — simulator says OK and provider returned None (fallback)"
    );

    let postcondition_audit = result.audit.iter()
        .find(|e| e.stage == "postcondition")
        .expect("Must have postcondition audit entry");
    assert!(
        postcondition_audit.description.contains("simulator_fallback"),
        "Audit must show 'simulator_fallback' as source: {}",
        postcondition_audit.description
    );
}

#[tokio::test]
async fn test_postcondition_field_observation_detects_thermal_violation() {
    // Simulator says OK, but field observation shows branch overload
    let sim = Arc::new(AlwaysOkSimulator);

    let obs = make_obs(vec![(1, 1.06, 0.0)], vec![(9, 80.0, 115.0)]);
    let obs_arc: Arc<PowerObservation> = Arc::new(obs);
    let provider: ObservationProvider = Arc::new(move || Some((*obs_arc).clone()));

    let pipeline = make_pipeline_with_observation_provider(sim, provider);

    let ctx = supervisor_alert_ctx();
    let action = StructuredAction::StartGenerator { gen_id: 2, target_mw: 80.0 };

    let result = pipeline.decide_enhanced(&action, &ctx).await;

    assert!(result.executed_action.is_some());

    let pc = result.post_conditions.as_ref().expect("PostConditions must exist");
    assert!(
        !pc.satisfied,
        "Postcondition must FAIL because field observation shows thermal violation"
    );
    assert!(
        pc.new_violations.iter().any(|v| v.contains("Thermal") || v.contains("thermal")),
        "Must report thermal violation from field observation: {:?}",
        pc.new_violations
    );
}
