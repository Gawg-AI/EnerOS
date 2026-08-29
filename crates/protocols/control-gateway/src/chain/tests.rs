#![allow(clippy::disallowed_macros)]

use eneros_telemetry_model::command::{ControlCommand, SingleCommand};
use eneros_telemetry_model::point_spec::{ControlDirection, PointSpec};
use eneros_telemetry_model::quality::QualityFlag;
use eneros_telemetry_model::unit::PhysicalUnit;

use super::*;
use crate::envelope::{ActorKind, EnvelopeBuilder};
use crate::policy::{CommandClass, PolicyRule};
use crate::safety::{SafetyPoint, SafetyRole};

const KEY: &[u8] = b"chain-test-key-xxxxxxxxxxxxxxxxxxxx";
const LEASE_ID: [u8; 16] = [0xA1; 16];
const ISSUER: [u8; 16] = [0xCD; 16];

fn now() -> u64 {
    1_700_000_000
}

fn quality_good() -> QualityFlag {
    QualityFlag::Good
}

/// Build a chain with one PCC breaker point, one operator-allow rule, and
/// an active lease covering device 1 / point 10.
fn chain_with_pcc() -> ControlChain {
    let mut policy = Policy::new(3).with_rule(PolicyRule {
        actor_kind: Some(ActorKind::Operator),
        devices: vec![1],
        points: vec![10],
        kinds: vec![CommandClass::Switch],
        time_window: (0, 0),
        obligations: vec![],
    });
    let _ = policy.replace_rules(vec![PolicyRule {
        actor_kind: Some(ActorKind::Operator),
        devices: vec![1],
        points: vec![10],
        kinds: vec![CommandClass::Switch],
        time_window: (0, 0),
        obligations: vec![],
    }]);
    // replace_rules bumps generation to 4; pass the policy with rules to the chain.
    let mut chain = ControlChain::new(KEY, 5, 64, policy, 60, 300, 64, 30);
    // issue lease
    let scope = LeaseScope {
        devices: vec![1],
        points: vec![10],
        max_class: 1,
    };
    let (token, _exp) = chain
        .lease_service_mut()
        .issue(LEASE_ID, scope, 600, now())
        .unwrap();
    assert_eq!(token, 1);
    // register PCC breaker
    chain.safety_mut().register(
        1,
        10,
        SafetyPoint {
            role: SafetyRole::PccBreaker,
            spec: PointSpec {
                point_id: 10,
                device_id: 1,
                unit: PhysicalUnit::Dimensionless,
                safe_min: 0.0,
                safe_max: 1.0,
                max_ramp_per_sec: None,
                direction: ControlDirection::Discrete,
                remote_controllable: true,
            },
        },
    );
    chain
}

/// Token returned by chain_with_pcc's lease.
fn lease_token() -> u64 {
    1
}

/// Policy generation of chain_with_pcc.
fn policy_gen() -> u64 {
    4
}

/// Build a signed envelope for a PCC close (SingleCommand::On) by an
/// Operator, riding the active lease.
fn pcc_close_buf(id: [u8; 16], idem: [u8; 16]) -> [u8; 172] {
    EnvelopeBuilder::new(
        id,
        1,
        10,
        ControlCommand::Single(SingleCommand::On),
        now(),
        now() + 60,
        ISSUER,
    )
    .lease(LEASE_ID, lease_token())
    .policy_version(policy_gen())
    .config_version(11)
    .sequence(1)
    .actor_kind(ActorKind::Operator)
    .idempotency_key(idem)
    .build(KEY)
    .encode()
}

#[test]
fn test_full_chain_executes_valid_command() {
    let mut chain = chain_with_pcc();
    chain.sbo_select(10, now());
    let buf = pcc_close_buf([1u8; 16], [2u8; 16]);
    let fb = FeedbackBundle::fresh(true, 50.0, now());
    match chain.execute(&buf, now(), &fb) {
        ChainOutcome::Executed { cmd, obligations } => {
            assert_eq!(cmd.device_id(), 1);
            assert_eq!(cmd.point_id(), 10);
            assert!(obligations.is_empty());
        }
        other => panic!("expected Executed, got {:?}", other),
    }
}

#[test]
fn test_chain_denies_revoked_lease() {
    let mut chain = chain_with_pcc();
    chain.sbo_select(10, now());
    assert!(chain.lease_service_mut().revoke(&LEASE_ID));
    let buf = pcc_close_buf([2u8; 16], [3u8; 16]);
    let fb = FeedbackBundle::fresh(true, 50.0, now());
    match chain.execute(&buf, now(), &fb) {
        ChainOutcome::Denied(ChainDenyReason::LeaseRevoked) => {}
        other => panic!("expected LeaseRevoked, got {:?}", other),
    }
}

#[test]
fn test_chain_idempotent_retransmit_returns_prior_outcome() {
    let mut chain = chain_with_pcc();
    chain.sbo_select(10, now());
    let idem = [5u8; 16];
    let buf = pcc_close_buf([3u8; 16], idem);
    let fb = FeedbackBundle::fresh(true, 50.0, now());
    assert!(matches!(
        chain.execute(&buf, now(), &fb),
        ChainOutcome::Executed { .. }
    ));
    // Retransmit same envelope (different command_id so gateway replay
    // doesn't fire first; same idempotency_key).
    let buf2 = pcc_close_buf([4u8; 16], idem);
    match chain.execute(&buf2, now(), &fb) {
        ChainOutcome::IdempotentHit(OpOutcome::Authorized) => {}
        other => panic!("expected IdempotentHit(Authorized), got {:?}", other),
    }
}

#[test]
fn test_chain_denies_superseded_by_override() {
    let mut chain = chain_with_pcc();
    chain.sbo_select(10, now());
    // Activate emergency override on point 10.
    chain
        .override_auth_mut()
        .activate(10, 300, now(), ActorKind::Emergency);
    let buf = pcc_close_buf([5u8; 16], [6u8; 16]);
    let fb = FeedbackBundle::fresh(true, 50.0, now());
    match chain.execute(&buf, now(), &fb) {
        ChainOutcome::Denied(ChainDenyReason::SupersededByOverride) => {}
        other => panic!("expected SupersededByOverride, got {:?}", other),
    }
}

#[test]
fn test_chain_emergency_bypasses_pdp_but_not_safety() {
    let mut chain = chain_with_pcc();
    chain.sbo_select(10, now());
    // Emergency actor closing PCC while grid is DOWN → safety still denies.
    let buf = EnvelopeBuilder::new(
        [6u8; 16],
        1,
        10,
        ControlCommand::Single(SingleCommand::On),
        now(),
        now() + 60,
        ISSUER,
    )
    .lease(LEASE_ID, lease_token())
    .policy_version(policy_gen())
    .actor_kind(ActorKind::Emergency)
    .idempotency_key([7u8; 16])
    .build(KEY)
    .encode();
    let fb = FeedbackBundle::fresh(false, 50.0, now()); // grid down
    match chain.execute(&buf, now(), &fb) {
        ChainOutcome::Denied(ChainDenyReason::Unsafe(_)) => {}
        other => panic!("expected Unsafe, got {:?}", other),
    }
}

#[test]
fn test_chain_denies_no_sbo_select() {
    let mut chain = chain_with_pcc();
    // NO sbo_select — discrete command must be rejected.
    let buf = pcc_close_buf([7u8; 16], [8u8; 16]);
    let fb = FeedbackBundle::fresh(true, 50.0, now());
    match chain.execute(&buf, now(), &fb) {
        ChainOutcome::Denied(ChainDenyReason::NotSelected) => {}
        other => panic!("expected NotSelected, got {:?}", other),
    }
}

#[test]
fn test_chain_denies_sbo_select_expired() {
    let mut chain = chain_with_pcc();
    chain.sbo_select(10, now());
    // Select TTL is 30s; advance past it.
    let buf = pcc_close_buf([8u8; 16], [9u8; 16]);
    let fb = FeedbackBundle::fresh(true, 50.0, now() + 31);
    // Note: envelope timestamp is `now()`; executing at now()+31 is within
    // 60s expiry but past the 30s SBO window.
    match chain.execute(&buf, now() + 31, &fb) {
        ChainOutcome::Denied(ChainDenyReason::SelectExpired) => {}
        other => panic!("expected SelectExpired, got {:?}", other),
    }
}

#[test]
fn test_chain_denies_unsafe_pcc_close_on_grid_down() {
    let mut chain = chain_with_pcc();
    chain.sbo_select(10, now());
    let buf = pcc_close_buf([9u8; 16], [10u8; 16]);
    let fb = FeedbackBundle::fresh(false, 50.0, now()); // grid down
    match chain.execute(&buf, now(), &fb) {
        ChainOutcome::Denied(ChainDenyReason::Unsafe(_)) => {}
        other => panic!("expected Unsafe, got {:?}", other),
    }
}

#[test]
fn test_chain_denies_on_stale_feedback() {
    let mut chain = chain_with_pcc();
    chain.sbo_select(10, now());
    let buf = pcc_close_buf([11u8; 16], [12u8; 16]);
    // Feedback timestamp is 120s old; max_age is 60s → stale.
    let fb = FeedbackBundle {
        grid_up: FeedbackSample {
            value: 1.0,
            quality: quality_good(),
            timestamp_epoch: now() - 120,
        },
        soc_pct: FeedbackSample {
            value: 50.0,
            quality: quality_good(),
            timestamp_epoch: now(),
        },
        dc_bus_v: FeedbackSample {
            value: 600.0,
            quality: quality_good(),
            timestamp_epoch: now(),
        },
        overcurrent: FeedbackSample {
            value: 0.0,
            quality: quality_good(),
            timestamp_epoch: now(),
        },
        overtemp: FeedbackSample {
            value: 0.0,
            quality: quality_good(),
            timestamp_epoch: now(),
        },
        maintenance_lockout: false,
    };
    match chain.execute(&buf, now(), &fb) {
        ChainOutcome::Denied(ChainDenyReason::FeedbackUntrusted(_)) => {}
        other => panic!("expected FeedbackUntrusted, got {:?}", other),
    }
}

#[test]
fn test_chain_denies_on_bad_quality_feedback() {
    let mut chain = chain_with_pcc();
    chain.sbo_select(10, now());
    let buf = pcc_close_buf([12u8; 16], [13u8; 16]);
    use eneros_telemetry_model::quality::QualityFlag;
    let fb = FeedbackBundle {
        grid_up: FeedbackSample {
            value: 1.0,
            quality: QualityFlag::Invalid,
            timestamp_epoch: now(),
        },
        soc_pct: FeedbackSample {
            value: 50.0,
            quality: quality_good(),
            timestamp_epoch: now(),
        },
        dc_bus_v: FeedbackSample {
            value: 600.0,
            quality: quality_good(),
            timestamp_epoch: now(),
        },
        overcurrent: FeedbackSample {
            value: 0.0,
            quality: quality_good(),
            timestamp_epoch: now(),
        },
        overtemp: FeedbackSample {
            value: 0.0,
            quality: quality_good(),
            timestamp_epoch: now(),
        },
        maintenance_lockout: false,
    };
    match chain.execute(&buf, now(), &fb) {
        ChainOutcome::Denied(ChainDenyReason::FeedbackUntrusted(_)) => {}
        other => panic!("expected FeedbackUntrusted, got {:?}", other),
    }
}

#[test]
fn test_chain_dry_run_obligation_intercepts_execution() {
    let policy = Policy::new(5).with_rule(PolicyRule {
        actor_kind: Some(ActorKind::Operator),
        devices: vec![1],
        points: vec![10],
        kinds: vec![CommandClass::Switch],
        time_window: (0, 0),
        obligations: vec![Obligation::DryRun],
    });
    let mut chain = ControlChain::new(KEY, 5, 64, policy, 60, 300, 64, 30);
    let scope = LeaseScope {
        devices: vec![1],
        points: vec![10],
        max_class: 1,
    };
    chain.lease_service_mut().issue(LEASE_ID, scope, 600, now());
    chain.safety_mut().register(
        1,
        10,
        SafetyPoint {
            role: SafetyRole::PccBreaker,
            spec: PointSpec {
                point_id: 10,
                device_id: 1,
                unit: PhysicalUnit::Dimensionless,
                safe_min: 0.0,
                safe_max: 1.0,
                max_ramp_per_sec: None,
                direction: ControlDirection::Discrete,
                remote_controllable: true,
            },
        },
    );
    chain.sbo_select(10, now());
    let buf = EnvelopeBuilder::new(
        [13u8; 16],
        1,
        10,
        ControlCommand::Single(SingleCommand::On),
        now(),
        now() + 60,
        ISSUER,
    )
    .lease(LEASE_ID, 1)
    .policy_version(5)
    .actor_kind(ActorKind::Operator)
    .idempotency_key([14u8; 16])
    .build(KEY)
    .encode();
    let fb = FeedbackBundle::fresh(true, 50.0, now());
    match chain.execute(&buf, now(), &fb) {
        ChainOutcome::DryRun { cmd, obligations } => {
            assert_eq!(cmd.point_id(), 10);
            assert!(obligations.iter().any(|o| matches!(o, Obligation::DryRun)));
        }
        other => panic!("expected DryRun, got {:?}", other),
    }
}

#[test]
fn test_chain_denies_policy_no_matching_rule() {
    // Empty policy → every command denied by PDP.
    let mut chain = ControlChain::new(KEY, 5, 64, Policy::new(7), 60, 300, 64, 30);
    let scope = LeaseScope {
        devices: vec![1],
        points: vec![10],
        max_class: 1,
    };
    chain.lease_service_mut().issue(LEASE_ID, scope, 600, now());
    chain.safety_mut().register(
        1,
        10,
        SafetyPoint {
            role: SafetyRole::PccBreaker,
            spec: PointSpec {
                point_id: 10,
                device_id: 1,
                unit: PhysicalUnit::Dimensionless,
                safe_min: 0.0,
                safe_max: 1.0,
                max_ramp_per_sec: None,
                direction: ControlDirection::Discrete,
                remote_controllable: true,
            },
        },
    );
    chain.sbo_select(10, now());
    let buf = EnvelopeBuilder::new(
        [14u8; 16],
        1,
        10,
        ControlCommand::Single(SingleCommand::On),
        now(),
        now() + 60,
        ISSUER,
    )
    .lease(LEASE_ID, 1)
    .policy_version(7)
    .actor_kind(ActorKind::Operator)
    .idempotency_key([15u8; 16])
    .build(KEY)
    .encode();
    let fb = FeedbackBundle::fresh(true, 50.0, now());
    match chain.execute(&buf, now(), &fb) {
        ChainOutcome::Denied(ChainDenyReason::Policy(DenyReason::NoMatchingRule)) => {}
        other => panic!("expected Policy(NoMatchingRule), got {:?}", other),
    }
}

#[test]
fn test_chain_denies_gateway_hmac_failure() {
    let mut chain = chain_with_pcc();
    chain.sbo_select(10, now());
    let mut buf = pcc_close_buf([15u8; 16], [16u8; 16]);
    buf[41] ^= 0x01; // tamper device_id byte (HMAC-covered)
    let fb = FeedbackBundle::fresh(true, 50.0, now());
    match chain.execute(&buf, now(), &fb) {
        ChainOutcome::Denied(ChainDenyReason::Gateway(GatewayError::Unauthorized)) => {}
        other => panic!("expected Gateway(Unauthorized), got {:?}", other),
    }
}
