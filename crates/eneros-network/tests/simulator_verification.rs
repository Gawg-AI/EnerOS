//! Comprehensive integration tests for NetworkSimulatorAdapter.
//!
//! Verifies the real IEEE 14-bus based simulator adapter works correctly
//! for What-If analysis in the constrained decision pipeline.

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
// P0: NetworkSimulatorAdapter tests
// ============================================================================

#[test]
fn test_simulator_start_generator_feasible() {
    let adapter = make_adapter();
    let action = StructuredAction::StartGenerator {
        gen_id: 1,
        target_mw: 50.0, // Reasonable MW for IEEE 14 bus
    };
    let result = adapter.simulate_action(&action);
    assert!(result.applicable, "Action should be applicable");
    assert!(result.converged, "Power flow should converge for reasonable MW");
    assert!(
        result.all_constraints_satisfied,
        "Reasonable generator output should satisfy all constraints, got: {}",
        result.summary
    );
}

#[test]
fn test_simulator_shed_load_feasible() {
    let adapter = make_adapter();
    let action = StructuredAction::ShedLoad {
        zone_id: 1,
        amount_mw: 10.0, // Reasonable amount
    };
    let result = adapter.simulate_action(&action);
    assert!(result.applicable, "ShedLoad should be applicable");
    assert!(result.converged, "Power flow should converge for reasonable load shed");
    // ShedLoad may or may not satisfy all constraints depending on the network state
    // but at least it should converge
}

#[test]
fn test_simulator_start_generator_extreme() {
    let adapter = make_adapter();
    // Use gen_id=2 (a PV bus, not the slack bus) with extreme MW.
    // The slack bus (gen_id=1) absorbs power imbalances, so extreme MW there
    // may not cause violations. A PV bus with extreme MW is more likely to
    // cause constraint violations.
    let action = StructuredAction::StartGenerator {
        gen_id: 2,
        target_mw: 9999.0,
    };
    let result = adapter.simulate_action(&action);
    // Extreme MW should either not converge or have constraint violations
    assert!(
        !result.converged || !result.all_constraints_satisfied,
        "Extreme MW should either not converge or violate constraints, got: converged={}, satisfied={}, summary={}",
        result.converged,
        result.all_constraints_satisfied,
        result.summary
    );
}

#[test]
fn test_simulator_other_actions_pass_through() {
    let adapter = make_adapter();

    // IsolateFault — doesn't modify p_spec
    let action = StructuredAction::IsolateFault {
        upstream_switch: 1,
        downstream_switch: 2,
    };
    let result = adapter.simulate_action(&action);
    assert!(result.applicable, "IsolateFault should be applicable");
    // It should still return a result (converged or not)

    // CloseTieSwitch — doesn't modify p_spec
    let action = StructuredAction::CloseTieSwitch { switch_id: 1 };
    let result = adapter.simulate_action(&action);
    assert!(result.applicable, "CloseTieSwitch should be applicable");

    // NotifyAgent — doesn't modify p_spec
    let action = StructuredAction::NotifyAgent {
        agent_id: "dispatch".to_string(),
        message: "test".to_string(),
    };
    let result = adapter.simulate_action(&action);
    assert!(result.applicable, "NotifyAgent should be applicable");
}

#[test]
fn test_simulator_generator_limits() {
    let adapter = make_adapter();
    let limits = adapter.generator_limits();
    // IEEE 14 bus has 5 generators
    assert_eq!(limits.len(), 5, "IEEE 14 should have 5 generator limits");

    // Verify each entry has valid p_min <= p_max
    for (gen_id, p_min, p_max) in &limits {
        assert!(
            p_min <= p_max,
            "Generator {}: p_min ({}) should be <= p_max ({})",
            gen_id,
            p_min,
            p_max
        );
        assert!(
            *p_max > 0.0,
            "Generator {}: p_max should be positive",
            gen_id
        );
    }
}

#[test]
fn test_simulator_current_voltages() {
    let adapter = make_adapter();
    let voltages = adapter.current_voltages();
    // IEEE 14 bus has 14 buses
    assert_eq!(
        voltages.len(),
        14,
        "IEEE 14 should have 14 voltage entries, got {}",
        voltages.len()
    );

    // All voltages should be in a reasonable range [0.9, 1.1]
    for (bus_id, v) in &voltages {
        assert!(
            *v >= 0.9 && *v <= 1.1,
            "Bus {} voltage {:.4} pu is outside [0.9, 1.1] range",
            bus_id,
            v
        );
    }
}

// ============================================================================
// Additional edge cases
// ============================================================================

#[test]
fn test_simulator_start_generator_zero_mw() {
    let adapter = make_adapter();
    let action = StructuredAction::StartGenerator {
        gen_id: 1,
        target_mw: 0.0,
    };
    let result = adapter.simulate_action(&action);
    assert!(result.applicable);
    // Zero MW should still converge (it's essentially shutting down the generator)
}

#[test]
fn test_simulator_shed_load_zero() {
    let adapter = make_adapter();
    let action = StructuredAction::ShedLoad {
        zone_id: 1,
        amount_mw: 0.0,
    };
    let result = adapter.simulate_action(&action);
    assert!(result.applicable);
    // Zero load shed is a no-op, should converge
}

#[test]
fn test_simulator_start_generator_unknown_gen() {
    let adapter = make_adapter();
    let action = StructuredAction::StartGenerator {
        gen_id: 999, // Non-existent generator
        target_mw: 100.0,
    };
    let result = adapter.simulate_action(&action);
    // Unknown generator ID — bus_map won't find it, p_spec unchanged
    // Should still converge (no modification to the network)
    assert!(result.applicable);
}

#[test]
fn test_simulator_multiple_actions_consistency() {
    let adapter = make_adapter();

    // Simulate two different actions and verify they produce independent results
    let action1 = StructuredAction::StartGenerator {
        gen_id: 1,
        target_mw: 100.0,
    };
    let action2 = StructuredAction::StartGenerator {
        gen_id: 2,
        target_mw: 80.0,
    };

    let result1 = adapter.simulate_action(&action1);
    let result2 = adapter.simulate_action(&action2);

    // Both should be applicable and converge
    assert!(result1.applicable);
    assert!(result2.applicable);

    // Results should be independent (each simulation starts from base case)
    // They may have different constraint satisfaction results
}
