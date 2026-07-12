//! Holdover timing state machine — OCXO + RTC backup when BeiDou is lost.
//!
//! When the primary BeiDou GNSS reference is lost, the system enters holdover:
//! the OCXO (oven-controlled crystal oscillator) maintains time with frequency
//! compensation, and the RTC provides a coarse fallback. This module tracks
//! the holdover state, computes drift estimates, and exposes a quality score.
//!
//! # State Machine
//!
//! ```text
//!   Beidou (primary) --loss--> Ocxo (holdover) --unhealthy--> Rtc (degraded)
//!        ^                                                         |
//!        +-------- BeiDou restored (smooth fall-back) -------------+
//! ```
//!
//! # Smooth Transition
//!
//! Clock source switches do not cause time jumps. A residual `switch_offset`
//! is recorded at switch time and gradually decayed (slewed) toward zero so
//! that the effective time converges smoothly to the new source.

use core::time::Duration;

use spin::Mutex;

pub mod ocxo;

// Re-export SwitchError so callers can use `holdover::SwitchError` without
// importing the redundancy module separately.
pub use crate::redundancy::SwitchError;

// ============================================================================
// Public types
// ============================================================================

/// Available clock sources, in descending priority order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockSource {
    /// BeiDou GNSS — primary reference (highest accuracy).
    Beidou,
    /// OCXO — holdover oscillator (sub-ms/24h drift).
    Ocxo,
    /// RTC — coarse fallback (seconds-level accuracy).
    Rtc,
}

/// Holdover quality grade based on projected 24-hour drift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldoverQuality {
    /// Projected 24h drift < 100 us.
    Excellent,
    /// Projected 24h drift < 1 ms.
    Good,
    /// Projected 24h drift < 10 ms.
    Degraded,
    /// Source unavailable or projected 24h drift >= 10 ms.
    Lost,
}

/// Current holdover status snapshot.
#[derive(Clone, Copy, Debug)]
pub struct HoldoverStatus {
    /// The active clock source.
    pub source: ClockSource,
    /// Estimated drift in nanoseconds per hour (magnitude).
    pub drift_ns_per_hour: u64,
    /// How long the system has been in holdover (zero on primary).
    pub holdover_elapsed: Duration,
    /// Quality grade derived from the projected 24h drift.
    pub quality: HoldoverQuality,
}

/// Clock source priority table.
#[derive(Clone, Copy, Debug)]
pub struct ClockPriority {
    /// Primary reference (BeiDou).
    pub primary: ClockSource,
    /// Secondary reference (OCXO).
    pub secondary: ClockSource,
    /// Tertiary reference (RTC).
    pub tertiary: ClockSource,
}

impl ClockPriority {
    /// Default priority: BeiDou > OCXO > RTC.
    pub const fn default() -> Self {
        Self {
            primary: ClockSource::Beidou,
            secondary: ClockSource::Ocxo,
            tertiary: ClockSource::Rtc,
        }
    }

    /// Return the priority-ordered list of sources.
    pub const fn as_array(&self) -> [ClockSource; 3] {
        [self.primary, self.secondary, self.tertiary]
    }
}

// ============================================================================
// Constants
// ============================================================================

/// Slew rate for gradual offset correction: 100 us per decay step.
const SLEW_RATE_NS: i64 = 100_000;

/// Default RTC frequency drift: 20 ppm (parts per million).
/// Typical PL031 RTC crystal: 10-50 ppm. We use a conservative 20 ppm.
const RTC_DRIFT_PPB: i64 = 20_000;

/// Default operating temperature (degrees Celsius).
const DEFAULT_TEMP_C: i32 = 25;

/// Nanoseconds per hour.
const HOUR_NS: u64 = 3_600_000_000_000;

// ============================================================================
// Internal state
// ============================================================================

/// Mutable holdover state. All fields are `pub(crate)` so the `redundancy`
/// module can implement the switching logic against the same state.
#[derive(Clone, Copy)]
pub(crate) struct HoldoverInner {
    /// Currently active clock source.
    pub(crate) current_source: ClockSource,
    /// Monotonic nanoseconds when holdover began (0 while on primary).
    pub(crate) holdover_start_ns: u64,
    /// Residual offset for smooth transitions; decays toward 0.
    pub(crate) switch_offset_ns: i64,
    /// One-shot authorization flag for manual switches.
    pub(crate) authorized: bool,
    /// BeiDou health.
    pub(crate) beidou_healthy: bool,
    pub(crate) beidou_score: u8,
    /// OCXO health.
    pub(crate) ocxo_healthy: bool,
    pub(crate) ocxo_score: u8,
    /// RTC health.
    pub(crate) rtc_healthy: bool,
    pub(crate) rtc_score: u8,
    /// Last time source health was evaluated.
    pub(crate) last_check_ns: u64,
    /// Current operating temperature (degrees Celsius).
    pub(crate) temperature_c: i32,
    /// OCXO compensation model parameters.
    pub(crate) ocxo_model: ocxo::OcxoModel,
}

impl HoldoverInner {
    /// Initial state: BeiDou primary, all sources healthy, no holdover.
    pub(crate) const fn new() -> Self {
        Self {
            current_source: ClockSource::Beidou,
            holdover_start_ns: 0,
            switch_offset_ns: 0,
            authorized: false,
            beidou_healthy: true,
            beidou_score: 100,
            ocxo_healthy: true,
            ocxo_score: 90,
            rtc_healthy: true,
            rtc_score: 80,
            last_check_ns: 0,
            temperature_c: DEFAULT_TEMP_C,
            ocxo_model: ocxo::OcxoModel::new(),
        }
    }
}

// ============================================================================
// Global state (static, Mutex-protected)
// ============================================================================

/// Holdover state machine. `pub(crate)` so `redundancy` can access it.
pub(crate) static HOLDOVER_STATE: Mutex<HoldoverInner> = Mutex::new(HoldoverInner::new());

/// Internal monotonic time counter. Updated via [`sync_time`].
pub(crate) static NOW_NS: Mutex<u64> = Mutex::new(0);

// ============================================================================
// Internal helpers
// ============================================================================

/// Read the internal monotonic nanosecond counter.
pub(crate) fn now_ns() -> u64 {
    *NOW_NS.lock()
}

/// Decay the switch offset toward zero by [`SLEW_RATE_NS`].
fn decay_switch_offset(state: &mut HoldoverInner) {
    if state.switch_offset_ns > 0 {
        state.switch_offset_ns = state.switch_offset_ns.saturating_sub(SLEW_RATE_NS);
        if state.switch_offset_ns < 0 {
            state.switch_offset_ns = 0;
        }
    } else if state.switch_offset_ns < 0 {
        state.switch_offset_ns = state.switch_offset_ns.saturating_add(SLEW_RATE_NS);
        if state.switch_offset_ns > 0 {
            state.switch_offset_ns = 0;
        }
    }
}

/// Compute the drift per hour (magnitude in ns) for the current source.
fn compute_drift_per_hour(state: &HoldoverInner) -> u64 {
    match state.current_source {
        ClockSource::Beidou => 0,
        ClockSource::Ocxo => {
            let hour = Duration::from_nanos(HOUR_NS);
            let extrapolated = ocxo::extrapolate_time(&state.ocxo_model, hour, state.temperature_c);
            #[allow(clippy::cast_possible_wrap)]
            let drift = extrapolated.as_nanos() as i128 - HOUR_NS as i128;
            drift.unsigned_abs() as u64
        }
        ClockSource::Rtc => {
            // RTC drift: 20 ppm typical.
            #[allow(clippy::cast_possible_truncation)]
            {
                HOUR_NS * RTC_DRIFT_PPB as u64 / 1_000_000_000
            }
        }
    }
}

/// Derive quality from projected 24-hour drift (in ns).
fn quality_from_drift(drift_ns_per_hour: u64) -> HoldoverQuality {
    let projected_24h = drift_ns_per_hour.saturating_mul(24);
    if projected_24h < 100_000 {
        HoldoverQuality::Excellent
    } else if projected_24h < 1_000_000 {
        HoldoverQuality::Good
    } else if projected_24h < 10_000_000 {
        HoldoverQuality::Degraded
    } else {
        HoldoverQuality::Lost
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Sync the internal monotonic time counter.
///
/// In production, called by the periodic system tick; in tests, called
/// directly to simulate time progression.
pub fn sync_time(ns: u64) {
    *NOW_NS.lock() = ns;
}

/// Return the effective time (raw monotonic + switch offset).
///
/// The switch offset is applied additively and decays over time, ensuring
/// no instantaneous clock jump when sources change.
pub fn current_time_ns() -> u64 {
    let now = now_ns();
    let offset = HOLDOVER_STATE.lock().switch_offset_ns;
    if offset >= 0 {
        now.saturating_add(offset as u64)
    } else {
        now.saturating_sub(offset.unsigned_abs())
    }
}

/// Query the current holdover quality.
pub fn holdover_quality() -> HoldoverStatus {
    let mut state = HOLDOVER_STATE.lock();
    let now = now_ns();

    let elapsed_ns = match state.current_source {
        ClockSource::Beidou => 0,
        _ => now.saturating_sub(state.holdover_start_ns),
    };

    let drift_ns_per_hour = compute_drift_per_hour(&state);
    let quality = quality_from_drift(drift_ns_per_hour);

    // Gradually correct any residual switch offset.
    decay_switch_offset(&mut state);

    HoldoverStatus {
        source: state.current_source,
        drift_ns_per_hour,
        holdover_elapsed: Duration::from_nanos(elapsed_ns),
        quality,
    }
}

/// Grant one-shot authorization for a manual clock source switch.
///
/// Authorization is consumed by the next [`switch_clock_source`] call. This
/// prevents unprivileged code from forcing a clock-source downgrade.
pub fn authorize_switch() {
    HOLDOVER_STATE.lock().authorized = true;
}

/// Force a switch to the specified clock source (requires authorization).
///
/// Delegates to [`redundancy::switch_clock_source`].
pub fn switch_clock_source(target: ClockSource) -> Result<(), SwitchError> {
    crate::redundancy::switch_clock_source(target)
}

/// Return the default clock priority table.
pub fn default_priority() -> ClockPriority {
    ClockPriority::default()
}

// ============================================================================
// Test-only accessors
// ============================================================================

/// Reset all holdover state to initial values. Used by tests.
#[cfg(test)]
pub(crate) fn reset_state() {
    *HOLDOVER_STATE.lock() = HoldoverInner::new();
    *NOW_NS.lock() = 0;
}

/// Set the mock monotonic time (tests only).
#[cfg(test)]
pub(crate) fn set_now_ns(ns: u64) {
    *NOW_NS.lock() = ns;
}

/// Set a source's health and score (tests only).
#[cfg(test)]
pub(crate) fn set_source_health(source: ClockSource, healthy: bool, score: u8) {
    let mut state = HOLDOVER_STATE.lock();
    match source {
        ClockSource::Beidou => {
            state.beidou_healthy = healthy;
            state.beidou_score = score;
        }
        ClockSource::Ocxo => {
            state.ocxo_healthy = healthy;
            state.ocxo_score = score;
        }
        ClockSource::Rtc => {
            state.rtc_healthy = healthy;
            state.rtc_score = score;
        }
    }
}

/// Set the authorization flag (tests only).
#[cfg(test)]
pub(crate) fn set_authorized(auth: bool) {
    HOLDOVER_STATE.lock().authorized = auth;
}

/// Set the OCXO model parameters (tests only).
#[cfg(test)]
pub(crate) fn set_ocxo_model(model: ocxo::OcxoModel) {
    HOLDOVER_STATE.lock().ocxo_model = model;
}

/// Set the current source directly (tests only).
#[cfg(test)]
pub(crate) fn set_current_source(source: ClockSource) {
    HOLDOVER_STATE.lock().current_source = source;
}

/// Set the switch offset directly (tests only).
#[cfg(test)]
pub(crate) fn set_switch_offset_ns(offset: i64) {
    HOLDOVER_STATE.lock().switch_offset_ns = offset;
}

/// Get the current source (tests only).
#[cfg(test)]
pub(crate) fn current_source() -> ClockSource {
    HOLDOVER_STATE.lock().current_source
}

/// Get the current switch offset (tests only).
#[cfg(test)]
pub(crate) fn switch_offset_ns() -> i64 {
    HOLDOVER_STATE.lock().switch_offset_ns
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;
    use crate::holdover::ocxo::OcxoModel;

    // Serialize tests that touch shared global state.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    #[test]
    fn test_holdover_quality_on_beidou() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_now_ns(5_000_000_000);

        let status = holdover_quality();
        assert_eq!(status.source, ClockSource::Beidou);
        assert_eq!(status.drift_ns_per_hour, 0);
        assert_eq!(status.holdover_elapsed, Duration::ZERO);
        assert_eq!(status.quality, HoldoverQuality::Excellent);
    }

    #[test]
    fn test_holdover_quality_on_ocxo() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        let now = HOUR_NS + 1_000_000_000;
        set_now_ns(now);
        set_current_source(ClockSource::Ocxo);
        // Set holdover start to 1 hour ago.
        HOLDOVER_STATE.lock().holdover_start_ns = now - HOUR_NS;
        set_ocxo_model(OcxoModel::with_params(1, 0)); // 1 ppb

        let status = holdover_quality();
        assert_eq!(status.source, ClockSource::Ocxo);
        // drift = 1 ppb * HOUR_NS / 1e9 = 3600 ns/h
        assert_eq!(status.drift_ns_per_hour, 3_600);
        // 24h drift = 86400 ns < 100_000 ns -> Excellent
        assert_eq!(status.quality, HoldoverQuality::Excellent);
        // holdover elapsed ~ 1 hour
        assert_eq!(status.holdover_elapsed, Duration::from_nanos(HOUR_NS));
    }

    #[test]
    fn test_holdover_quality_on_rtc() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_now_ns(10_000_000_000);
        set_current_source(ClockSource::Rtc);
        HOLDOVER_STATE.lock().holdover_start_ns = 0;

        let status = holdover_quality();
        assert_eq!(status.source, ClockSource::Rtc);
        // RTC drift = 20 ppm = 72 ms/h
        let expected = HOUR_NS * 20_000 / 1_000_000_000;
        assert_eq!(status.drift_ns_per_hour, expected);
        // 24h drift = 72ms * 24 = 1728ms > 10ms -> Lost
        assert_eq!(status.quality, HoldoverQuality::Lost);
    }

    #[test]
    fn test_switch_clock_source_not_authorized() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_authorized(false);

        let err = switch_clock_source(ClockSource::Ocxo).unwrap_err();
        assert_eq!(err, SwitchError::NotAuthorized);
        // Source should not have changed.
        assert_eq!(current_source(), ClockSource::Beidou);
    }

    #[test]
    fn test_switch_clock_source_authorized() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_now_ns(42);
        authorize_switch();

        switch_clock_source(ClockSource::Ocxo).expect("switch should succeed");
        assert_eq!(current_source(), ClockSource::Ocxo);
        // Holdover start should be set.
        assert_eq!(HOLDOVER_STATE.lock().holdover_start_ns, 42);
        // Authorization consumed.
        assert!(!HOLDOVER_STATE.lock().authorized);
    }

    #[test]
    fn test_switch_clock_source_already_active() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        authorize_switch();

        let err = switch_clock_source(ClockSource::Beidou).unwrap_err();
        assert_eq!(err, SwitchError::AlreadyActive);
    }

    #[test]
    fn test_switch_clock_source_unavailable() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_source_health(ClockSource::Ocxo, false, 20);
        authorize_switch();

        let err = switch_clock_source(ClockSource::Ocxo).unwrap_err();
        assert_eq!(err, SwitchError::SourceUnavailable);
    }

    #[test]
    fn test_state_machine_beidou_to_ocxo_to_rtc() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_now_ns(1_000);

        // BeiDou healthy -> stay on BeiDou.
        assert_eq!(current_source(), ClockSource::Beidou);

        // BeiDou becomes unhealthy, OCXO healthy.
        set_source_health(ClockSource::Beidou, false, 20);
        let switched = crate::redundancy::auto_switch_if_needed();
        assert_eq!(switched, Some(ClockSource::Ocxo));
        assert_eq!(current_source(), ClockSource::Ocxo);

        // OCXO becomes unhealthy, RTC healthy.
        set_source_health(ClockSource::Ocxo, false, 10);
        let switched = crate::redundancy::auto_switch_if_needed();
        assert_eq!(switched, Some(ClockSource::Rtc));
        assert_eq!(current_source(), ClockSource::Rtc);
    }

    #[test]
    fn test_state_machine_recovery_beidou_restored() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_now_ns(1_000);

        // Fall to RTC.
        set_source_health(ClockSource::Beidou, false, 0);
        set_source_health(ClockSource::Ocxo, false, 0);
        crate::redundancy::auto_switch_if_needed();
        assert_eq!(current_source(), ClockSource::Rtc);

        // BeiDou restored.
        set_source_health(ClockSource::Beidou, true, 100);
        let switched = crate::redundancy::auto_switch_if_needed();
        assert_eq!(switched, Some(ClockSource::Beidou));
        assert_eq!(current_source(), ClockSource::Beidou);
        // Holdover start cleared on return to primary.
        assert_eq!(HOLDOVER_STATE.lock().holdover_start_ns, 0);
    }

    #[test]
    fn test_switch_offset_decays() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_switch_offset_ns(500_000);

        let _status = holdover_quality();
        // After one decay step, offset should be reduced by SLEW_RATE_NS.
        let offset = switch_offset_ns();
        assert_eq!(offset, 500_000 - SLEW_RATE_NS);

        // Keep decaying until it reaches zero.
        for _ in 0..10 {
            let _ = holdover_quality();
        }
        assert_eq!(switch_offset_ns(), 0);
    }

    #[test]
    fn test_current_time_ns_no_offset() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_now_ns(123_456);

        assert_eq!(current_time_ns(), 123_456);
    }

    #[test]
    fn test_current_time_ns_with_positive_offset() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_now_ns(100_000);
        set_switch_offset_ns(50_000);

        assert_eq!(current_time_ns(), 150_000);
    }

    #[test]
    fn test_current_time_ns_with_negative_offset() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_now_ns(100_000);
        set_switch_offset_ns(-30_000);

        assert_eq!(current_time_ns(), 70_000);
    }

    #[test]
    fn test_clock_priority_default() {
        let p = default_priority();
        assert_eq!(p.primary, ClockSource::Beidou);
        assert_eq!(p.secondary, ClockSource::Ocxo);
        assert_eq!(p.tertiary, ClockSource::Rtc);
        assert_eq!(
            p.as_array(),
            [ClockSource::Beidou, ClockSource::Ocxo, ClockSource::Rtc]
        );
    }
}
