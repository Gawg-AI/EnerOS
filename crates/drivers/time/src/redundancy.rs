//! Three-source clock redundancy and failover.
//!
//! Provides health evaluation and automatic/manual switching between the
//! three clock sources: BeiDou (primary), OCXO (holdover), and RTC (fallback).
//!
//! # Failover Strategy
//!
//! 1. **Health scoring**: each source gets a 0-100 score. Below 50 is
//!    considered unhealthy.
//! 2. **Automatic failover**: when the active source becomes unhealthy, the
//!    system switches to the next healthy source in priority order.
//! 3. **Smooth transition**: switches record a residual offset that is
//!    gradually decayed, so the effective time never jumps.
//! 4. **Authorization**: manual switches require prior authorization to
//!    prevent malicious downgrade attacks. Automatic safety failover bypasses
//!    this check.

use crate::holdover::{now_ns, ClockPriority, ClockSource, HOLDOVER_STATE};

// ============================================================================
// Public types
// ============================================================================

/// Health snapshot of a single clock source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceHealth {
    /// Which source this health entry describes.
    pub source: ClockSource,
    /// Whether the source is considered usable (score >= 50).
    pub healthy: bool,
    /// Health score in [0, 100]. Higher is better.
    pub score: u8,
    /// Monotonic nanoseconds when this source was last evaluated.
    pub last_check: u64,
}

/// Errors that can occur during a manual clock source switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchError {
    /// The caller has not been authorized via [`holdover::authorize_switch`].
    NotAuthorized,
    /// The target source is currently unavailable (unhealthy).
    SourceUnavailable,
    /// The target source is already the active source.
    AlreadyActive,
}

// ============================================================================
// Constants
// ============================================================================

/// Health score threshold: below this a source is considered unhealthy.
const HEALTH_THRESHOLD: u8 = 50;

// ============================================================================
// Internal helpers
// ============================================================================

/// Return the health (healthy, score) of a source from the holdover state.
///
/// A source is considered healthy only if its health flag is set AND its
/// score meets the [`HEALTH_THRESHOLD`].
fn source_health_tuple(state: &crate::holdover::HoldoverInner, source: ClockSource) -> (bool, u8) {
    let (stored_healthy, score) = match source {
        ClockSource::Beidou => (state.beidou_healthy, state.beidou_score),
        ClockSource::Ocxo => (state.ocxo_healthy, state.ocxo_score),
        ClockSource::Rtc => (state.rtc_healthy, state.rtc_score),
    };
    (stored_healthy && score >= HEALTH_THRESHOLD, score)
}

/// Apply a source switch: update current_source and holdover_start_ns.
///
/// Does NOT check authorization — that is the caller's responsibility.
/// Does NOT change the switch offset, so the effective time does not jump.
fn apply_switch(
    state: &mut crate::holdover::HoldoverInner,
    from: ClockSource,
    target: ClockSource,
    now: u64,
) {
    // Record holdover start when leaving the primary (BeiDou) source.
    if from == ClockSource::Beidou && target != ClockSource::Beidou {
        state.holdover_start_ns = now;
    } else if target == ClockSource::Beidou {
        // Returning to primary — clear holdover.
        state.holdover_start_ns = 0;
    }
    state.current_source = target;
}

// ============================================================================
// Public API
// ============================================================================

/// Evaluate the health of all three clock sources.
///
/// Returns an array of [`SourceHealth`] in priority order: BeiDou, OCXO, RTC.
/// Also updates the `last_check_ns` field in the internal state.
pub fn evaluate_sources() -> [SourceHealth; 3] {
    let mut state = HOLDOVER_STATE.lock();
    let now = now_ns();
    state.last_check_ns = now;

    let priority = ClockPriority::default().as_array();
    let mut result = [SourceHealth {
        source: ClockSource::Beidou,
        healthy: false,
        score: 0,
        last_check: now,
    }; 3];

    for (i, &src) in priority.iter().enumerate() {
        let (healthy, score) = source_health_tuple(&state, src);
        result[i] = SourceHealth {
            source: src,
            healthy,
            score,
            last_check: now,
        };
    }

    result
}

/// Force a switch to the specified clock source (requires authorization).
///
/// # Authorization
///
/// The caller must have called [`holdover::authorize_switch`] beforehand.
/// Authorization is consumed (one-shot) regardless of whether the switch
/// succeeds due to `AlreadyActive` or `SourceUnavailable`.
///
/// # Errors
///
/// - [`SwitchError::NotAuthorized`] — no prior authorization.
/// - [`SwitchError::AlreadyActive`] — `target` is already the active source.
/// - [`SwitchError::SourceUnavailable`] — `target` is currently unhealthy.
pub fn switch_clock_source(target: ClockSource) -> Result<(), SwitchError> {
    let mut state = HOLDOVER_STATE.lock();

    // Check and consume authorization.
    if !state.authorized {
        return Err(SwitchError::NotAuthorized);
    }
    state.authorized = false;

    let current = state.current_source;
    if current == target {
        return Err(SwitchError::AlreadyActive);
    }

    let (healthy, _) = source_health_tuple(&state, target);
    if !healthy {
        return Err(SwitchError::SourceUnavailable);
    }

    let now = now_ns();
    apply_switch(&mut state, current, target, now);
    Ok(())
}

/// Automatically switch to the highest-priority healthy source.
///
/// This single rule implements both failover and recovery:
/// - **Failover**: when the current source becomes unhealthy, the system
///   drops to the next healthy source in priority order.
/// - **Recovery**: when a higher-priority source than the current one
///   becomes healthy again (e.g. BeiDou restored while on OCXO/RTC), the
///   system switches back to it.
///
/// This bypasses the authorization requirement because it is a safety
/// mechanism — the system must not remain on a failed source waiting for
/// authorization, and must recover to the primary reference as soon as it
/// is available again.
///
/// Returns `Some(new_source)` if a switch was performed, `None` if the
/// current source is already the highest-priority healthy source or no
/// healthy source exists.
pub fn auto_switch_if_needed() -> Option<ClockSource> {
    let mut state = HOLDOVER_STATE.lock();
    let current = state.current_source;

    // Scan sources in priority order and pick the first (highest-priority)
    // healthy one. This naturally handles both failover (current unhealthy
    // → lower-priority healthy source) and recovery (higher-priority source
    // restored → switch back up).
    let priority = ClockPriority::default().as_array();
    for &target in &priority {
        let (healthy, _) = source_health_tuple(&state, target);
        if healthy {
            if target == current {
                return None; // Current is already the best healthy source.
            }
            let now = now_ns();
            apply_switch(&mut state, current, target, now);
            return Some(target);
        }
    }

    // No healthy source found at all — stay on the current source.
    None
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;
    use crate::holdover::{
        authorize_switch, current_source, current_time_ns, holdover_quality, reset_state,
        set_current_source, set_now_ns, set_source_health, ClockSource::Beidou, ClockSource::Ocxo,
        ClockSource::Rtc, HoldoverQuality,
    };

    // Serialize tests that touch shared global state.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    // ---- evaluate_sources ----

    #[test]
    fn test_evaluate_sources_returns_three() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_source_health(Beidou, true, 100);
        set_source_health(Ocxo, true, 85);
        set_source_health(Rtc, true, 70);

        let healths = evaluate_sources();
        assert_eq!(healths.len(), 3);
        assert_eq!(healths[0].source, Beidou);
        assert!(healths[0].healthy);
        assert_eq!(healths[0].score, 100);
        assert_eq!(healths[1].source, Ocxo);
        assert_eq!(healths[1].score, 85);
        assert_eq!(healths[2].source, Rtc);
        assert_eq!(healths[2].score, 70);
    }

    #[test]
    fn test_evaluate_sources_unhealthy() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_source_health(Beidou, false, 30);
        set_source_health(Ocxo, true, 60);

        let healths = evaluate_sources();
        assert!(!healths[0].healthy);
        assert_eq!(healths[0].score, 30);
        assert!(healths[1].healthy);
    }

    // ---- switch_clock_source ----

    #[test]
    fn test_switch_not_authorized() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        let err = switch_clock_source(Ocxo).unwrap_err();
        assert_eq!(err, SwitchError::NotAuthorized);
        assert_eq!(current_source(), Beidou);
    }

    #[test]
    fn test_switch_authorized_success() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_now_ns(999);
        authorize_switch();

        switch_clock_source(Ocxo).expect("switch should succeed");
        assert_eq!(current_source(), Ocxo);
    }

    #[test]
    fn test_switch_already_active() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        authorize_switch();

        let err = switch_clock_source(Beidou).unwrap_err();
        assert_eq!(err, SwitchError::AlreadyActive);
    }

    #[test]
    fn test_switch_source_unavailable() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_source_health(Ocxo, false, 20);
        authorize_switch();

        let err = switch_clock_source(Ocxo).unwrap_err();
        assert_eq!(err, SwitchError::SourceUnavailable);
        assert_eq!(current_source(), Beidou);
    }

    #[test]
    fn test_switch_authorization_consumed_on_failure() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        authorize_switch();

        // AlreadyActive consumes authorization.
        let _ = switch_clock_source(Beidou);
        assert!(!crate::holdover::HOLDOVER_STATE.lock().authorized);

        // Second attempt without re-authorizing should fail.
        let err = switch_clock_source(Ocxo).unwrap_err();
        assert_eq!(err, SwitchError::NotAuthorized);
    }

    // ---- auto_switch_if_needed ----

    #[test]
    fn test_auto_switch_no_switch_needed() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        // All sources healthy.
        assert_eq!(auto_switch_if_needed(), None);
        assert_eq!(current_source(), Beidou);
    }

    #[test]
    fn test_auto_switch_beidou_to_ocxo() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_now_ns(500);
        set_source_health(Beidou, false, 20);

        let switched = auto_switch_if_needed();
        assert_eq!(switched, Some(Ocxo));
        assert_eq!(current_source(), Ocxo);
        // Holdover start recorded.
        assert_eq!(
            crate::holdover::HOLDOVER_STATE.lock().holdover_start_ns,
            500
        );
    }

    #[test]
    fn test_auto_switch_ocxo_to_rtc() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_now_ns(1_000);
        // BeiDou must be unavailable for the Ocxo->Rtc transition to occur;
        // otherwise the recovery rule would switch back to BeiDou.
        set_source_health(Beidou, false, 0);
        set_current_source(Ocxo);
        crate::holdover::HOLDOVER_STATE.lock().holdover_start_ns = 500;
        set_source_health(Ocxo, false, 10);

        let switched = auto_switch_if_needed();
        assert_eq!(switched, Some(Rtc));
        assert_eq!(current_source(), Rtc);
    }

    #[test]
    fn test_auto_switch_no_healthy_source() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_source_health(Beidou, false, 0);
        set_source_health(Ocxo, false, 0);
        set_source_health(Rtc, false, 0);

        let switched = auto_switch_if_needed();
        assert_eq!(switched, None);
        // Stays on current (BeiDou) — no better option.
        assert_eq!(current_source(), Beidou);
    }

    #[test]
    fn test_auto_switch_recovers_to_beidou() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_now_ns(2_000);

        // Fall to RTC.
        set_source_health(Beidou, false, 0);
        set_source_health(Ocxo, false, 0);
        auto_switch_if_needed();
        assert_eq!(current_source(), Rtc);

        // BeiDou restored — should switch back.
        set_source_health(Beidou, true, 100);
        let switched = auto_switch_if_needed();
        assert_eq!(switched, Some(Beidou));
        assert_eq!(current_source(), Beidou);
    }

    // ---- Smooth transition (no clock jump) ----

    #[test]
    fn test_smooth_transition_no_jump() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_now_ns(10_000_000);
        set_source_health(Beidou, false, 20);
        set_source_health(Ocxo, true, 90);

        // Record effective time before switch.
        let before = current_time_ns();

        // Auto-switch BeiDou -> Ocxo.
        auto_switch_if_needed();

        // Record effective time after switch — must be identical (no jump).
        let after = current_time_ns();
        assert_eq!(
            before, after,
            "effective time must not jump on source switch"
        );
    }

    #[test]
    fn test_smooth_transition_full_chain_no_jump() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_now_ns(100_000);

        let t0 = current_time_ns();

        // BeiDou -> Ocxo.
        set_source_health(Beidou, false, 20);
        auto_switch_if_needed();
        let t1 = current_time_ns();
        assert_eq!(t0, t1, "no jump on BeiDou->Ocxo");

        // Ocxo -> Rtc.
        set_source_health(Ocxo, false, 10);
        auto_switch_if_needed();
        let t2 = current_time_ns();
        assert_eq!(t1, t2, "no jump on Ocxo->Rtc");

        // Rtc -> BeiDou (recovery).
        set_source_health(Beidou, true, 100);
        auto_switch_if_needed();
        let t3 = current_time_ns();
        assert_eq!(t2, t3, "no jump on Rtc->BeiDou");
    }

    // ---- RTC-only monotonic ----

    #[test]
    fn test_rtc_only_monotonic() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        // Force RTC-only mode.
        set_source_health(Beidou, false, 0);
        set_source_health(Ocxo, false, 0);
        set_source_health(Rtc, true, 70);
        auto_switch_if_needed();
        assert_eq!(current_source(), Rtc);

        // Advance time and verify monotonic increase.
        set_now_ns(1_000_000);
        let t1 = current_time_ns();

        set_now_ns(2_000_000);
        let t2 = current_time_ns();
        assert!(t2 > t1, "RTC-only mode must be monotonic: {t2} > {t1}");

        set_now_ns(3_000_000);
        let t3 = current_time_ns();
        assert!(t3 > t2, "RTC-only mode must be monotonic: {t3} > {t2}");
    }

    // ---- Integration: holdover quality reflects source ----

    #[test]
    fn test_quality_changes_with_source() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        // On BeiDou: Excellent.
        let s = holdover_quality();
        assert_eq!(s.quality, HoldoverQuality::Excellent);

        // Switch to RTC: Lost (20 ppm drift).
        set_source_health(Beidou, false, 0);
        set_source_health(Ocxo, false, 0);
        auto_switch_if_needed();

        let s = holdover_quality();
        assert_eq!(s.source, Rtc);
        assert_eq!(s.quality, HoldoverQuality::Lost);
    }
}
