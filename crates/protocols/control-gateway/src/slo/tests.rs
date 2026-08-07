#![allow(clippy::disallowed_macros)]

use super::*;

#[test]
fn test_frozen_slos_exist_for_all_ids() {
    // Every SloId variant must have a frozen spec.
    for id in [
        SloId::ControlLoop,
        SloId::IrqLatency,
        SloId::IpcRoundTrip,
        SloId::SafetyValidation,
        SloId::GatewayAuthorize,
        SloId::ContextSwitch,
        SloId::ScheduleLatency,
    ] {
        assert!(frozen_slo(id).is_some(), "missing frozen SLO for {:?}", id);
    }
}

#[test]
fn test_control_loop_deadline_is_10ms_period() {
    let spec = frozen_slo(SloId::ControlLoop).unwrap();
    assert_eq!(
        spec.period_ns, 10_000_000,
        "control loop must be 10ms (100Hz)"
    );
    assert!(
        spec.deadline_ns < spec.period_ns,
        "deadline must be < period"
    );
}

#[test]
fn test_miss_class_classification() {
    let spec = frozen_slo(SloId::ControlLoop).unwrap();
    assert_eq!(MissClass::from_elapsed(spec, 1_000_000), MissClass::Nominal);
    assert_eq!(
        MissClass::from_elapsed(spec, spec.p99_threshold_ns + 1),
        MissClass::Warning
    );
    assert_eq!(
        MissClass::from_elapsed(spec, spec.p999_threshold_ns + 1),
        MissClass::Critical
    );
    assert_eq!(
        MissClass::from_elapsed(spec, spec.deadline_ns + 1),
        MissClass::Missed
    );
}

#[test]
fn test_nominal_samples_no_degradation() {
    let mut mon = DeadlineMonitor::default();
    for i in 0..200u64 {
        let (class, action) = mon.record(SloId::ControlLoop, 1_000_000 + i * 100, i * 10_000_000);
        assert_eq!(class, MissClass::Nominal);
        assert_eq!(action, DegradationAction::None);
    }
    assert_eq!(mon.current_degradation(), DegradationAction::None);
    let snap = mon.snapshot();
    assert!(snap.all_pass());
}

#[test]
fn test_control_loop_miss_triggers_safe_state() {
    let mut mon = DeadlineMonitor::default();
    // Fill with nominal samples.
    for i in 0..200u64 {
        mon.record(SloId::ControlLoop, 1_000_000, i * 10_000_000);
    }
    // One miss.
    let (_, action) = mon.record(SloId::ControlLoop, 9_000_000, 2_000_000_000);
    assert_eq!(action, DegradationAction::EnterSafeState);
    assert_eq!(mon.current_degradation(), DegradationAction::EnterSafeState);
    let snap = mon.snapshot();
    assert!(!snap.all_pass());
    assert_eq!(mon.stats(SloId::ControlLoop).unwrap().misses, 1);
}

#[test]
fn test_safety_validation_miss_triggers_safe_state() {
    let mut mon = DeadlineMonitor::default();
    mon.record(SloId::SafetyValidation, 1_500_000, 0); // > deadline (1ms)
    assert_eq!(mon.current_degradation(), DegradationAction::EnterSafeState);
}

#[test]
fn test_irq_miss_triggers_pause_agents() {
    let mut mon = DeadlineMonitor::default();
    mon.record(SloId::IrqLatency, 60_000, 0); // > 50us deadline
    assert_eq!(
        mon.current_degradation(),
        DegradationAction::PauseNonCriticalAgents
    );
}

#[test]
fn test_gateway_miss_triggers_shed_optional() {
    let mut mon = DeadlineMonitor::default();
    mon.record(SloId::GatewayAuthorize, 3_000_000, 0); // > 2ms deadline
    assert_eq!(
        mon.current_degradation(),
        DegradationAction::ShedOptionalServices
    );
}

#[test]
fn test_p99_breach_triggers_throttle_ai() {
    let mut mon = DeadlineMonitor::default();
    // 200 samples: 190 nominal, 11 warnings (> P99 threshold).
    for i in 0..189u64 {
        mon.record(SloId::ControlLoop, 1_000_000, i * 10_000_000);
    }
    let p99 = frozen_slo(SloId::ControlLoop).unwrap().p99_threshold_ns;
    for i in 0..11u64 {
        mon.record(SloId::ControlLoop, p99 + 100_000, (189 + i) * 10_000_000);
    }
    // 11/200 = 5.5% > 1% warning rate → ThrottleAi
    assert_eq!(mon.current_degradation(), DegradationAction::ThrottleAi);
}

#[test]
fn test_degradation_max_picks_more_severe() {
    assert_eq!(
        DegradationAction::None.max(DegradationAction::ThrottleAi),
        DegradationAction::ThrottleAi
    );
    assert_eq!(
        DegradationAction::EnterSafeState.max(DegradationAction::None),
        DegradationAction::EnterSafeState
    );
    assert_eq!(
        DegradationAction::PauseNonCriticalAgents.max(DegradationAction::ShedOptionalServices),
        DegradationAction::ShedOptionalServices
    );
}

#[test]
fn test_percentile_nearest_rank() {
    let mut mon = DeadlineMonitor::default();
    // Insert 100 samples: 1..=100 (in ns).
    for i in 1..=100u64 {
        mon.record(SloId::ControlLoop, i * 1_000, i * 10_000_000);
    }
    // P99 of 1..100 → nearest-rank: ceil(99/100 * 100) = 99th value = 99.
    let p99 = mon.p99_ns(SloId::ControlLoop);
    assert!(
        (99_000..=100_000).contains(&p99),
        "P99 should be ~99000, got {}",
        p99
    );
}

#[test]
fn test_workload_profile_digest() {
    let w = WorkloadProfile::frozen_baseline();
    let d = w.digest();
    assert!(d.contains("cl8"));
    assert!(d.contains("ag16"));
    assert!(d.contains("ai4"));
}

#[test]
fn test_snapshot_reports_pass_fail() {
    let mut mon = DeadlineMonitor::default();
    // All nominal.
    for i in 0..100u64 {
        mon.record(SloId::ControlLoop, 1_000_000, i * 10_000_000);
    }
    let snap = mon.snapshot();
    assert!(snap.all_pass());
    assert_eq!(snap.pass_count(), snap.total());

    // Add a miss.
    mon.record(SloId::ControlLoop, 9_000_000, 1_000_000_000);
    let snap2 = mon.snapshot();
    assert!(!snap2.all_pass());
}

#[test]
fn test_jitter_tracking() {
    let mut mon = DeadlineMonitor::default();
    mon.record(SloId::ControlLoop, 1_000_000, 0);
    mon.record(SloId::ControlLoop, 3_000_000, 10_000_000);
    let st = mon.stats(SloId::ControlLoop).unwrap();
    assert_eq!(st.jitter_ns(), 2_000_000);
}

#[test]
fn test_safe_state_is_most_severe() {
    assert!(DegradationAction::EnterSafeState.severity() > DegradationAction::None.severity());
    assert!(
        DegradationAction::EnterSafeState.severity() > DegradationAction::ThrottleAi.severity()
    );
}
