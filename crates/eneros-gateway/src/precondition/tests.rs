use super::*;
use crate::interlocking::DeviceStates;
use eneros_core::{AuthorityLevel, Jurisdiction, PowerObservation, SystemOperatingState};

fn make_ctx() -> DecisionContext {
    DecisionContext::new(
        AuthorityLevel::Supervisor,
        Jurisdiction::unrestricted(),
        SystemOperatingState::Normal,
    )
}

#[test]
fn test_precondition_generator_passes() {
    let checker = PreConditionChecker::new();
    let ctx = make_ctx();
    let action = StructuredAction::StartGenerator {
        gen_id: 1,
        target_mw: 100.0,
    };
    let result = checker.check(&action, &ctx);
    assert!(
        result.satisfied,
        "Pre-conditions should pass: {:?}",
        result.failure_summary
    );
}

#[test]
fn test_precondition_observer_rejected() {
    let checker = PreConditionChecker::new();
    let ctx = DecisionContext::new(
        AuthorityLevel::Observer,
        Jurisdiction::unrestricted(),
        SystemOperatingState::Normal,
    );
    let action = StructuredAction::StartGenerator {
        gen_id: 1,
        target_mw: 100.0,
    };
    let result = checker.check(&action, &ctx);
    assert!(!result.satisfied);
    assert!(result
        .failure_summary
        .iter()
        .any(|r| r.contains("Insufficient authority")));
}

#[test]
fn test_precondition_negative_generator_target() {
    let checker = PreConditionChecker::new();
    let ctx = make_ctx();
    let action = StructuredAction::StartGenerator {
        gen_id: 1,
        target_mw: -50.0,
    };
    let result = checker.check(&action, &ctx);
    assert!(!result.satisfied);
}

#[test]
fn test_precondition_rejects_non_finite_generator_targets() {
    let checker = PreConditionChecker::new();
    let ctx = make_ctx();

    for target_mw in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let action = StructuredAction::StartGenerator {
            gen_id: 1,
            target_mw,
        };
        let result = checker.check(&action, &ctx);
        assert!(
            !result.satisfied,
            "target_mw={target_mw} should fail preconditions"
        );
    }
}

#[test]
fn test_precondition_shed_load_fraction_exceeded() {
    let checker = PreConditionChecker::new();
    let mut obs = PowerObservation::empty();
    obs.total_load_mw = 100.0;
    let ctx = make_ctx().with_observation(obs);
    let action = StructuredAction::ShedLoad {
        zone_id: 1,
        amount_mw: 60.0,
    };
    let result = checker.check(&action, &ctx);
    assert!(!result.satisfied);
    assert!(result
        .failure_summary
        .iter()
        .any(|r| r.contains("exceeds maximum")));
}

#[test]
fn test_precondition_shed_load_within_fraction() {
    let checker = PreConditionChecker::new();
    let mut obs = PowerObservation::empty();
    obs.total_load_mw = 100.0;
    let ctx = make_ctx().with_observation(obs);
    let action = StructuredAction::ShedLoad {
        zone_id: 1,
        amount_mw: 30.0,
    };
    let result = checker.check(&action, &ctx);
    assert!(
        result.satisfied,
        "Should pass: {:?}",
        result.failure_summary
    );
}

#[test]
fn test_precondition_rejects_non_finite_shed_amounts() {
    let checker = PreConditionChecker::new();
    let ctx = make_ctx();

    for amount_mw in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let action = StructuredAction::ShedLoad {
            zone_id: 1,
            amount_mw,
        };
        let result = checker.check(&action, &ctx);
        assert!(
            !result.satisfied,
            "amount_mw={amount_mw} should fail preconditions"
        );
    }
}

#[test]
fn test_precondition_isolate_same_switch_rejected() {
    let checker = PreConditionChecker::new();
    let ctx = make_ctx().with_device_states(DeviceStates::default());
    let action = StructuredAction::IsolateFault {
        upstream_switch: 5,
        downstream_switch: 5,
    };
    let result = checker.check(&action, &ctx);
    assert!(!result.satisfied);
}

#[test]
fn test_precondition_blackout_restricts_actions() {
    let checker = PreConditionChecker::new();
    let ctx = DecisionContext::new(
        AuthorityLevel::Emergency,
        Jurisdiction::unrestricted(),
        SystemOperatingState::Blackout,
    );
    let action = StructuredAction::ShedLoad {
        zone_id: 1,
        amount_mw: 10.0,
    };
    let result = checker.check(&action, &ctx);
    assert!(!result.satisfied);
}

#[test]
fn test_precondition_blackout_allows_generator_start() {
    let checker = PreConditionChecker::new();
    let ctx = DecisionContext::new(
        AuthorityLevel::Emergency,
        Jurisdiction::unrestricted(),
        SystemOperatingState::Blackout,
    );
    let action = StructuredAction::StartGenerator {
        gen_id: 1,
        target_mw: 100.0,
    };
    let result = checker.check(&action, &ctx);
    assert!(
        result.satisfied,
        "Black start should be allowed: {:?}",
        result.failure_summary
    );
}

#[test]
fn test_precondition_notify_always_passes() {
    let checker = PreConditionChecker::new();
    let ctx = make_ctx();
    let action = StructuredAction::NotifyAgent {
        agent_id: "dispatch".to_string(),
        message: "test".to_string(),
    };
    let result = checker.check(&action, &ctx);
    assert!(result.satisfied);
}

#[test]
fn test_precondition_jurisdiction_zone_rejected() {
    let checker = PreConditionChecker::new();
    let ctx = DecisionContext::new(
        AuthorityLevel::Supervisor,
        Jurisdiction::for_zones(vec![1, 2]),
        SystemOperatingState::Normal,
    );
    let action = StructuredAction::ShedLoad {
        zone_id: 5,
        amount_mw: 10.0,
    };
    let result = checker.check(&action, &ctx);
    assert!(!result.satisfied);
}

#[test]
fn test_precondition_ground_switch_blocks_close() {
    let checker = PreConditionChecker::new();
    let mut states = DeviceStates::default();
    states.ground_switch_states.insert(10, true);
    let ctx = make_ctx().with_device_states(states);
    let action = StructuredAction::ExecuteDevice {
        device_id: 1,
        operation: "close".to_string(),
        value: 1.0,
    };
    let result = checker.check(&action, &ctx);
    assert!(!result.satisfied);
}

#[test]
fn test_precondition_rejects_non_finite_device_value() {
    let checker = PreConditionChecker::new();
    let ctx = make_ctx();
    let action = StructuredAction::ExecuteDevice {
        device_id: 1,
        operation: "adjust_reactive".to_string(),
        value: f64::NAN,
    };
    let result = checker.check(&action, &ctx);
    assert!(!result.satisfied);
}
