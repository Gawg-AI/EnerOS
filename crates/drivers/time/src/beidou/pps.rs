//! 1PPS interrupt handling and PI clock disciplining.
//!
//! The 1PPS (one pulse-per-second) signal from the BeiDou receiver provides a
//! precise second boundary. When a PPS pulse arrives, the hardware captures a
//! local monotonic timestamp; this is paired with the NMEA time sentence to
//! compute the clock offset (钟差). A PI controller then fine-tunes the
//! monotonic clock rate to converge the offset toward zero.
//!
//! # PI Controller
//!
//! The controller uses proportional and integral terms:
//!
//! ```text
//! error     = BDT_ns − local_ns      (positive: local clock is behind)
//! P_term    = Kp × error
//! I_term   += Ki × error             (with anti-windup clamping)
//! output    = clamp(P_term + I_term, ±MAX_CORRECTION)
//! ```
//!
//! The output is a signed correction in nanoseconds. A positive value speeds
//! the clock up; a negative value slows it down. The magnitude is clamped to
//! [`MAX_CORRECTION_NS`] to guarantee the disciplined clock never jumps
//! backward — it is always monotonically increasing.

use core::time::Duration;

use crate::beidou::{BeidouState, SyncError, TimeStamp};
use crate::beidou::{BEIDOU_STATE, LAST_PPS_NS, PI_STATE};

// ============================================================================
// PI controller parameters
// ============================================================================

/// Proportional gain numerator (Kp = KP_NUM / KP_DEN = 0.5).
const KP_NUM: i64 = 1;
const KP_DEN: i64 = 2;

/// Integral gain numerator (Ki = KI_NUM / KI_DEN = 0.1).
const KI_NUM: i64 = 1;
const KI_DEN: i64 = 10;

/// Anti-windup: maximum absolute value of the integral term (500 µs).
const INTEGRAL_LIMIT_NS: i64 = 500_000;

/// Maximum correction per discipline call (50 µs).  This bound guarantees the
/// disciplined clock advances monotonically — the correction can slow the
/// clock but never reverse it.
const MAX_CORRECTION_NS: i64 = 50_000;

/// Number of consecutive PPS captures retained for jitter estimation.
const PPS_HISTORY_LEN: usize = 4;

/// Ring buffer of recent PPS timestamps for jitter calculation.
static PPS_HISTORY: spin::Mutex<PpsHistory> = spin::Mutex::new(PpsHistory::new());

// ============================================================================
// PPS history (for jitter estimation)
// ============================================================================

struct PpsHistory {
    /// Circular buffer of PPS local timestamps.
    samples: [u64; PPS_HISTORY_LEN],
    /// Number of valid samples (0..=PPS_HISTORY_LEN).
    count: usize,
    /// Insertion index (wraps around).
    head: usize,
}

impl PpsHistory {
    const fn new() -> Self {
        Self {
            samples: [0; PPS_HISTORY_LEN],
            count: 0,
            head: 0,
        }
    }

    /// Push a new PPS timestamp, evicting the oldest if full.
    fn push(&mut self, ts: u64) {
        self.samples[self.head] = ts;
        self.head = (self.head + 1) % PPS_HISTORY_LEN;
        if self.count < PPS_HISTORY_LEN {
            self.count += 1;
        }
    }

    /// Compute the jitter (max deviation from the mean interval) in ns.
    /// Returns 0 when fewer than 2 samples are available.
    fn jitter_ns(&self) -> u32 {
        if self.count < 2 {
            return 0;
        }
        // Collect valid intervals between consecutive PPS pulses.
        let mut intervals = [0u64; PPS_HISTORY_LEN - 1];
        let mut n = 0usize;
        for i in 0..(self.count - 1) {
            let idx_a = (self.head + PPS_HISTORY_LEN - self.count + i) % PPS_HISTORY_LEN;
            let idx_b = (idx_a + 1) % PPS_HISTORY_LEN;
            let interval = self.samples[idx_b].saturating_sub(self.samples[idx_a]);
            intervals[n] = interval;
            n += 1;
        }
        if n == 0 {
            return 0;
        }
        // Mean interval (should be ~1_000_000_000 ns).
        let mean: u64 = intervals[..n].iter().sum::<u64>() / n as u64;
        // Max deviation from mean.
        let max_dev: u64 = intervals[..n]
            .iter()
            .map(|&v| v.abs_diff(mean))
            .max()
            .unwrap_or(0);
        // Truncate to u32 (jitter is sub-second).
        if max_dev > u32::MAX as u64 {
            u32::MAX
        } else {
            max_dev as u32
        }
    }
}

// ============================================================================
// Public API
// ============================================================================

/// 1PPS interrupt callback.
///
/// Captures the hardware timestamp at the PPS edge and stores it for pairing
/// with the next NMEA sentence. Also updates the PPS jitter estimate in the
/// global [`BeidouState`].
///
/// `ts.nanos_since_epoch` should be the local monotonic nanoseconds at the
/// rising edge of the 1PPS pulse.
pub fn on_pps_pulse(ts: TimeStamp) {
    let local_ns = ts.nanos_since_epoch;

    // Store the capture for beidou_sync() pairing.
    *LAST_PPS_NS.lock() = Some(local_ns);

    // Push into the history ring for jitter estimation.
    let mut hist = PPS_HISTORY.lock();
    hist.push(local_ns);
    let jitter = hist.jitter_ns();
    drop(hist);

    // Update the global state.
    let mut state = BEIDOU_STATE.lock();
    state.pps_jitter_ns = jitter;
    state.disciplined = true;
}

/// Discipline the local monotonic clock toward BDT using a PI controller.
///
/// Reads the last BDT fix and PPS capture, computes the clock offset, applies
/// the PI algorithm, and returns the correction as a [`Duration`].
///
/// The correction is the absolute magnitude; the sign (speed-up vs. slow-down)
/// is recorded in the internal PI state. The magnitude is clamped to
/// [`MAX_CORRECTION_NS`] to guarantee monotonicity.
///
/// # Errors
///
/// - [`SyncError::NoSignal`] — no BDT fix available (`pps.last_fix` is `None`).
/// - [`SyncError::PpsTimeout`] — no PPS capture available.
pub fn discipline_clock(pps: &BeidouState) -> Result<Duration, SyncError> {
    let fix = pps.last_fix.ok_or(SyncError::NoSignal)?;

    let local_ns = match *LAST_PPS_NS.lock() {
        Some(ns) => ns,
        None => return Err(SyncError::PpsTimeout),
    };

    // Clock error: positive means local clock is behind BDT (speed up).
    // Both values fit comfortably in i64 (BDT ns for hundreds of years < i64::MAX).
    #[allow(clippy::cast_possible_truncation)]
    let error_ns = fix.nanos_since_epoch as i64 - local_ns as i64;

    let mut pi = PI_STATE.lock();

    // Integral term with anti-windup clamping.
    let integral_delta = KI_NUM.saturating_mul(error_ns) / KI_DEN;
    pi.integral_ns = pi.integral_ns.saturating_add(integral_delta);
    pi.integral_ns = pi.integral_ns.clamp(-INTEGRAL_LIMIT_NS, INTEGRAL_LIMIT_NS);

    // PI output: P term + I term.
    let p_term = KP_NUM.saturating_mul(error_ns) / KP_DEN;
    let raw_output = p_term.saturating_add(pi.integral_ns);

    // Clamp to ±MAX_CORRECTION_NS. The bounded correction guarantees the
    // disciplined clock is always monotonically increasing: even a maximum
    // negative correction only slows the clock, it never reverses it.
    let correction = raw_output.clamp(-MAX_CORRECTION_NS, MAX_CORRECTION_NS);

    pi.last_output_ns = correction;
    pi.call_count = pi.call_count.wrapping_add(1);

    // Return the magnitude as Duration.
    let abs_ns = correction.unsigned_abs();
    Ok(Duration::from_nanos(abs_ns))
}

// ============================================================================
// Internal accessors (for tests)
// ============================================================================

/// Reset the PPS history ring buffer to its initial state. Used by tests.
#[cfg(test)]
pub(crate) fn reset_pps_history() {
    *PPS_HISTORY.lock() = PpsHistory::new();
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;
    use crate::beidou::{reset_state, FixQuality, PiState};

    // Serialize tests that touch shared global state.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    /// Build a BeidouState with the given BDT fix (in BDT nanos).
    fn make_state(bdt_nanos: u64) -> BeidouState {
        BeidouState {
            last_fix: Some(TimeStamp {
                nanos_since_epoch: bdt_nanos,
                leap_seconds: 4,
                fix_quality: FixQuality::Fix3D { satellites: 8 },
            }),
            pps_jitter_ns: 10,
            satellites_visible: 8,
            disciplined: true,
        }
    }

    fn set_local_ns(ns: u64) {
        *LAST_PPS_NS.lock() = Some(ns);
    }

    // ---- on_pps_pulse ----

    #[test]
    fn test_on_pps_pulse_stores_capture() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        on_pps_pulse(TimeStamp {
            nanos_since_epoch: 1_000_000_000,
            leap_seconds: 4,
            fix_quality: FixQuality::NoFix,
        });

        assert_eq!(*LAST_PPS_NS.lock(), Some(1_000_000_000));
        assert!(BEIDOU_STATE.lock().disciplined);
    }

    #[test]
    fn test_on_pps_pulse_updates_jitter() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        // Three PPS pulses 1s apart → jitter should be 0.
        for i in 0..3u64 {
            on_pps_pulse(TimeStamp {
                nanos_since_epoch: i * 1_000_000_000,
                leap_seconds: 0,
                fix_quality: FixQuality::NoFix,
            });
        }
        let jitter = BEIDOU_STATE.lock().pps_jitter_ns;
        assert_eq!(jitter, 0, "jitter should be zero for uniform intervals");
    }

    #[test]
    fn test_on_pps_pulse_detects_jitter() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        // Pulses with varying intervals: 1.000s, 1.002s, 0.998s
        let pulses = [0u64, 1_000_000_000, 1_002_000_000, 2_000_000_000];
        for &ns in &pulses {
            on_pps_pulse(TimeStamp {
                nanos_since_epoch: ns,
                leap_seconds: 0,
                fix_quality: FixQuality::NoFix,
            });
        }
        let jitter = BEIDOU_STATE.lock().pps_jitter_ns;
        // Mean interval = (1.000 + 0.998 + 0.998) / 3 = 0.998667s
        // Deviations: |1.000 - 0.998667| = 1_333_333, etc.
        assert!(
            jitter > 0,
            "jitter should be non-zero for irregular intervals"
        );
    }

    // ---- discipline_clock ----

    #[test]
    fn test_discipline_clock_no_fix() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        let state = BeidouState::new();
        let err = discipline_clock(&state).unwrap_err();
        assert_eq!(err, SyncError::NoSignal);
    }

    #[test]
    fn test_discipline_clock_no_pps() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        let state = make_state(1_000_000_000);
        // No PPS capture set.
        *LAST_PPS_NS.lock() = None;
        let err = discipline_clock(&state).unwrap_err();
        assert_eq!(err, SyncError::PpsTimeout);
    }

    #[test]
    fn test_discipline_clock_positive_error() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        *PI_STATE.lock() = PiState::new();

        // BDT is 1ms ahead of local → positive error → speed up.
        set_local_ns(1_000_000_000);
        let state = make_state(1_001_000_000);

        let dur = discipline_clock(&state).expect("should succeed");
        let correction = PI_STATE.lock().last_output_ns;
        assert!(correction > 0, "positive error → positive correction");
        assert!(correction <= MAX_CORRECTION_NS, "correction bounded");
        assert_eq!(dur.as_nanos(), correction.unsigned_abs() as u128);
    }

    #[test]
    fn test_discipline_clock_negative_error() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        *PI_STATE.lock() = PiState::new();

        // BDT is 1ms behind local → negative error → slow down.
        set_local_ns(1_001_000_000);
        let state = make_state(1_000_000_000);

        let dur = discipline_clock(&state).expect("should succeed");
        let correction = PI_STATE.lock().last_output_ns;
        assert!(correction < 0, "negative error → negative correction");
        assert!(correction.abs() <= MAX_CORRECTION_NS, "correction bounded");
        assert_eq!(dur.as_nanos(), correction.unsigned_abs() as u128);
    }

    #[test]
    fn test_discipline_clock_correction_bounded() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        *PI_STATE.lock() = PiState::new();

        // Enormous error (1 second) — correction must be clamped.
        set_local_ns(1_000_000_000);
        let state = make_state(2_000_000_000);

        let _dur = discipline_clock(&state).expect("should succeed");
        let correction = PI_STATE.lock().last_output_ns;
        assert!(
            correction.abs() <= MAX_CORRECTION_NS,
            "correction must be clamped to ±{MAX_CORRECTION_NS}, got {correction}"
        );
    }

    #[test]
    fn test_discipline_clock_integral_accumulates() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        *PI_STATE.lock() = PiState::new();

        set_local_ns(1_000_000_000);
        let state = make_state(1_001_000_000); // 1ms error

        // First call.
        let _d1 = discipline_clock(&state).unwrap();
        let integral_1 = PI_STATE.lock().integral_ns;

        // Second call with same error.
        let _d2 = discipline_clock(&state).unwrap();
        let integral_2 = PI_STATE.lock().integral_ns;

        assert!(
            integral_2.abs() > integral_1.abs(),
            "integral should accumulate: {integral_1} → {integral_2}"
        );
    }

    #[test]
    fn test_discipline_clock_integral_anti_windup() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        *PI_STATE.lock() = PiState::new();

        // Sustained large error → integral should saturate at INTEGRAL_LIMIT_NS.
        set_local_ns(0);
        let state = make_state(10_000_000_000); // 10s error

        for _ in 0..1000 {
            let _ = discipline_clock(&state).unwrap();
        }
        let integral = PI_STATE.lock().integral_ns;
        assert!(
            integral.abs() <= INTEGRAL_LIMIT_NS,
            "integral must be clamped to ±{INTEGRAL_LIMIT_NS}, got {integral}"
        );
    }

    #[test]
    fn test_discipline_clock_monotonic() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        *PI_STATE.lock() = PiState::new();

        // Start with local clock 500µs behind BDT.
        let mut local_ns: i64 = 1_000_000_000;
        let mut bdt_ns: i64 = 1_000_500_000;
        set_local_ns(local_ns as u64);
        let mut state = make_state(bdt_ns as u64);

        let mut prev_clock = local_ns;
        for _ in 0..20 {
            let _dur = discipline_clock(&state).unwrap();
            let correction = PI_STATE.lock().last_output_ns;

            // Each iteration represents one PPS pulse (1 second of real time).
            // Both the local clock and BDT advance by ~1 second; the PI
            // correction nudges the local clock toward BDT. Because the real
            // time per call (1s) exceeds the maximum negative correction
            // (50µs), the disciplined clock always advances monotonically.
            const REAL_TIME_PER_CALL_NS: i64 = 1_000_000_000;
            local_ns = local_ns
                .saturating_add(REAL_TIME_PER_CALL_NS)
                .saturating_add(correction);
            bdt_ns = bdt_ns.saturating_add(REAL_TIME_PER_CALL_NS);
            set_local_ns(local_ns as u64);
            state = make_state(bdt_ns as u64);

            assert!(
                local_ns >= prev_clock,
                "clock must be monotonically increasing: {local_ns} < {prev_clock}"
            );
            prev_clock = local_ns;
        }
    }

    // ---- PpsHistory ----

    #[test]
    fn test_pps_history_jitter_zero() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut hist = PpsHistory::new();
        // Uniform 1s intervals.
        for i in 0..4u64 {
            hist.push(i * 1_000_000_000);
        }
        assert_eq!(hist.jitter_ns(), 0);
    }

    #[test]
    fn test_pps_history_jitter_nonzero() {
        let mut hist = PpsHistory::new();
        // Intervals: 1.000s, 1.010s → mean = 1.005s, max dev = 5ms.
        hist.push(0);
        hist.push(1_000_000_000);
        hist.push(2_010_000_000);
        let jitter = hist.jitter_ns();
        assert_eq!(jitter, 5_000_000); // 5ms
    }

    #[test]
    fn test_pps_history_few_samples() {
        let mut hist = PpsHistory::new();
        assert_eq!(hist.jitter_ns(), 0);
        hist.push(100);
        assert_eq!(hist.jitter_ns(), 0); // only 1 sample
    }

    #[test]
    fn test_pps_history_ring_wrap() {
        let mut hist = PpsHistory::new();
        // Push more than capacity to test wrap-around.
        for i in 0..6u64 {
            hist.push(i * 1_000_000_000);
        }
        // After wrap, the last 4 samples should be in the buffer.
        assert_eq!(hist.count, PPS_HISTORY_LEN);
        // Jitter should still be 0 for uniform intervals.
        assert_eq!(hist.jitter_ns(), 0);
    }
}
