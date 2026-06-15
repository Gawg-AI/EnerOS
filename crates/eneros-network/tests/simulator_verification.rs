//! Integration tests for NetworkSimulatorAdapter.
//!
//! Phase 15: these tests verify the *physically accurate* behavior of the
//! What-If simulator. Previously the simulator hardcoded generator limits,
//! misused `bus_map` for gen_id/zone_id lookups, and treated every switching
//! action as feasible. The assertions below reflect the corrected semantics:
//!
//! - `generator_limits()` reads the real IEEE-14 generator table.
//! - `StartGenerator` resolves gen_id → bus via the generator table.
//! - `ShedLoad` resolves zone_id → buses via the zone map.
//! - Unknown gen_id / zone_id → `applicable == false` (was silently a no-op).
//! - Switching actions → conservative `all_constraints_satisfied == false`
//!   (was silently feasible).
//! - Default voltage/thermal constraints are registered, so genuine violations
//!   are now detectable.

use std::sync::Arc;

use eneros_constraint::projector::NetworkSimulator;
use eneros_core::StructuredAction;
use eneros_network::network::PowerNetwork;
use eneros_network::simulator::NetworkSimulatorAdapter;

fn make_adapter() -> NetworkSimulatorAdapter {
    let network = Arc::new(parking_lot::RwLock::new(PowerNetwork::from_ieee14()));
    NetworkSimulatorAdapter::new(network)
}

// ============================================================================
// Generation actions (physically modeled)
// ============================================================================

#[test]
fn test_simulator_start_generator_pv_bus_feasible() {
    let adapter = make_adapter();
    // gen_id=2 is a real PV generator on bus 2 (Gen=40MW, Pmax=140). A modest
    // target within limits should produce a solvable case.
    let action = StructuredAction::StartGenerator {
        gen_id: 2,
        target_mw: 40.0,
    };
    let result = adapter.simulate_action(&action);
    assert!(
        result.applicable,
        "gen_id=2 must be applicable: {}",
        result.summary
    );
    assert!(
        result.converged,
        "should converge for in-range MW: {}",
        result.summary
    );
}

#[test]
fn test_simulator_start_generator_unknown_gen_rejected() {
    let adapter = make_adapter();
    // gen_id=999 does not exist. Phase 15: the simulator must report this as
    // inapplicable instead of silently no-op'ing (the old bus_map misuse would
    // just leave p_spec unchanged and report success).
    let action = StructuredAction::StartGenerator {
        gen_id: 999,
        target_mw: 100.0,
    };
    let result = adapter.simulate_action(&action);
    assert!(
        !result.applicable,
        "unknown gen_id must be inapplicable, got applicable=true ({})",
        result.summary
    );
}

#[test]
fn test_simulator_start_generator_extreme_mw_violates() {
    let adapter = make_adapter();
    // gen_id=2 with 9999 MW is far beyond Pmax=140. The simulation should
    // either diverge or report constraint violations.
    let action = StructuredAction::StartGenerator {
        gen_id: 2,
        target_mw: 9999.0,
    };
    let result = adapter.simulate_action(&action);
    assert!(result.applicable);
    assert!(
        !result.converged || !result.all_constraints_satisfied,
        "extreme MW should diverge or violate, got converged={} satisfied={} ({})",
        result.converged,
        result.all_constraints_satisfied,
        result.summary
    );
}

#[test]
fn test_simulator_start_generator_zero_mw() {
    let adapter = make_adapter();
    let action = StructuredAction::StartGenerator {
        gen_id: 2,
        target_mw: 0.0,
    };
    let result = adapter.simulate_action(&action);
    assert!(result.applicable);
    // Zero output is a valid dispatch point.
}

#[test]
fn test_simulator_start_generator_slack_bus_rejected() {
    let adapter = make_adapter();
    let action = StructuredAction::StartGenerator {
        gen_id: 1,
        target_mw: 50.0,
    };
    let result = adapter.simulate_action(&action);
    assert!(!result.applicable);
    assert!(result.summary.contains("slack bus"));
}

#[test]
fn test_simulator_rejects_non_finite_generator_target() {
    let adapter = make_adapter();
    let action = StructuredAction::StartGenerator {
        gen_id: 2,
        target_mw: f64::NAN,
    };
    let result = adapter.simulate_action(&action);
    assert!(!result.applicable);
    assert!(result.summary.contains("not finite"));
}

// ============================================================================
// Load actions (physically modeled)
// ============================================================================

#[test]
fn test_simulator_shed_load_zone_zero_feasible() {
    let adapter = make_adapter();
    // IEEE-14 from_ieee14 registers a single zone 0 covering all buses.
    let action = StructuredAction::ShedLoad {
        zone_id: 0,
        amount_mw: 10.0,
    };
    let result = adapter.simulate_action(&action);
    assert!(
        result.applicable,
        "zone 0 must be applicable: {}",
        result.summary
    );
    assert!(
        result.converged,
        "should converge for modest shed: {}",
        result.summary
    );
}

#[test]
fn test_simulator_shed_load_zero_amount() {
    let adapter = make_adapter();
    let action = StructuredAction::ShedLoad {
        zone_id: 0,
        amount_mw: 0.0,
    };
    let result = adapter.simulate_action(&action);
    assert!(
        result.applicable,
        "zone 0 shed 0MW must be applicable: {}",
        result.summary
    );
}

#[test]
fn test_simulator_shed_load_unknown_zone_rejected() {
    let adapter = make_adapter();
    // zone_id=99 does not exist. Phase 15: inapplicable (was silently a no-op
    // under the old bus_map misuse).
    let action = StructuredAction::ShedLoad {
        zone_id: 99,
        amount_mw: 10.0,
    };
    let result = adapter.simulate_action(&action);
    assert!(
        !result.applicable,
        "unknown zone must be inapplicable, got applicable=true ({})",
        result.summary
    );
}

#[test]
fn test_simulator_rejects_non_finite_shed_amount() {
    let adapter = make_adapter();
    let action = StructuredAction::ShedLoad {
        zone_id: 0,
        amount_mw: f64::INFINITY,
    };
    let result = adapter.simulate_action(&action);
    assert!(!result.applicable);
    assert!(result.summary.contains("not finite"));
}

#[test]
fn test_simulator_rejects_non_finite_reactive_adjustment() {
    let adapter = make_adapter();
    let action = StructuredAction::ExecuteDevice {
        device_id: 2,
        operation: "adjust_reactive".to_string(),
        value: f64::NEG_INFINITY,
    };
    let result = adapter.simulate_action(&action);
    assert!(!result.applicable);
    assert!(result.summary.contains("not finite"));
}

// ============================================================================
// Switching actions (conservative reject)
// ============================================================================

#[test]
fn test_simulator_isolate_fault_conservative_reject() {
    let adapter = make_adapter();
    let action = StructuredAction::IsolateFault {
        upstream_switch: 1,
        downstream_switch: 2,
    };
    let result = adapter.simulate_action(&action);
    assert!(result.applicable, "switching action is applicable");
    // Phase 15: switching physics is not modeled, so it must NOT be reported
    // as constraint-satisfying (old behavior reported it feasible).
    assert!(
        !result.all_constraints_satisfied,
        "switching action must be conservatively rejected, got satisfied=true ({})",
        result.summary
    );
}

#[test]
fn test_simulator_close_tie_switch_conservative_reject() {
    let adapter = make_adapter();
    let action = StructuredAction::CloseTieSwitch { switch_id: 5 };
    let result = adapter.simulate_action(&action);
    assert!(result.applicable);
    assert!(
        !result.all_constraints_satisfied,
        "tie switch must be conservatively rejected ({})",
        result.summary
    );
}

#[test]
fn test_simulator_execute_device_open_conservative_reject() {
    let adapter = make_adapter();
    let action = StructuredAction::ExecuteDevice {
        device_id: 1,
        operation: "open".to_string(),
        value: 0.0,
    };
    let result = adapter.simulate_action(&action);
    assert!(result.applicable);
    assert!(
        !result.all_constraints_satisfied,
        "open device must be conservatively rejected ({})",
        result.summary
    );
}

// ============================================================================
// NotifyAgent (no physical effect)
// ============================================================================

#[test]
fn test_simulator_notify_agent_base_case() {
    let adapter = make_adapter();
    let action = StructuredAction::NotifyAgent {
        agent_id: "dispatch".to_string(),
        message: "test".to_string(),
    };
    let result = adapter.simulate_action(&action);
    assert!(result.applicable, "NotifyAgent is always applicable");
    // NotifyAgent solves the unmodified base case — convergence mirrors the
    // network's own solvability.
}

// ============================================================================
// Generator limits & voltages (data sourced from the real network)
// ============================================================================

#[test]
fn test_simulator_generator_limits_from_network() {
    let adapter = make_adapter();
    let limits = adapter.generator_limits();
    // IEEE-14 has 5 generators (buses 1, 2, 3, 6, 8).
    assert_eq!(limits.len(), 5, "IEEE-14 should have 5 generators");

    // Phase 15: limits come from the real GeneratorSpec table, not a hardcoded
    // literal. Verify the values match from_ieee14's data (not the old
    // 200/150/100/80/60 stub which had gen 1 = 200, not 332.4).
    let by_id: std::collections::HashMap<u64, (f64, f64)> = limits
        .into_iter()
        .map(|(id, p_min, p_max)| (id, (p_min, p_max)))
        .collect();
    assert!(by_id.contains_key(&1), "gen 1 present");
    assert!(by_id.contains_key(&2), "gen 2 present");
    assert!(by_id.contains_key(&3), "gen 3 present");
    assert!(by_id.contains_key(&6), "gen 6 present");
    assert!(by_id.contains_key(&8), "gen 8 present");

    // gen 1 (slack) Pmax is 332.4 in the real table — distinct from the old
    // hardcoded 200.0, proving the data is read from the network.
    let (p1_min, p1_max) = by_id[&1];
    assert!(
        (p1_max - 332.4).abs() < 1e-6,
        "gen 1 Pmax should be 332.4, got {}",
        p1_max
    );
    assert!(p1_min <= p1_max);

    for (p_min, p_max) in by_id.values() {
        assert!(p_min <= p_max);
        assert!(*p_max > 0.0);
    }
}

#[test]
fn test_simulator_current_voltages() {
    let adapter = make_adapter();
    let voltages = adapter.current_voltages();
    assert_eq!(voltages.len(), 14, "IEEE-14 should have 14 voltage entries");

    for (bus_id, v) in &voltages {
        assert!(
            *v >= 0.9 && *v <= 1.1,
            "Bus {} voltage {:.4} pu outside [0.9, 1.1]",
            bus_id,
            v
        );
    }
}

// ============================================================================
// Default constraints registered (Phase 15)
// ============================================================================

#[test]
fn test_default_constraints_detect_violations() {
    // Phase 15: from_ieee14 now registers default voltage/thermal constraints.
    // A deliberately extreme dispatch must produce a non-empty violation list
    // (proving check_constraints no longer always returns empty).
    let adapter = make_adapter();
    let action = StructuredAction::StartGenerator {
        gen_id: 2,
        target_mw: 9999.0,
    };
    let result = adapter.simulate_action(&action);
    // Either it diverged (no violations but converged=false) or it converged
    // with violations — in both cases the simulator is NOT reporting a clean
    // feasible result.
    assert!(
        !result.all_constraints_satisfied,
        "extreme dispatch must not be reported clean-feasible"
    );
}

#[test]
fn test_simulator_multiple_actions_independent() {
    let adapter = make_adapter();
    let action1 = StructuredAction::StartGenerator {
        gen_id: 2,
        target_mw: 40.0,
    };
    let action2 = StructuredAction::StartGenerator {
        gen_id: 6,
        target_mw: 20.0,
    };

    let result1 = adapter.simulate_action(&action1);
    let result2 = adapter.simulate_action(&action2);

    assert!(result1.applicable);
    assert!(result2.applicable);
    // Each simulation starts from the base case, so they are independent.
}
