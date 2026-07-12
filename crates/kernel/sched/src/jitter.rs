//! Jitter measurement for partition scheduling — Phase 0 P0-F (v0.19.0).
//!
//! Provides [`JitterStats`] aggregation and [`record_jitter`]/[`jitter_measure`]/[`jitter_reset`]
//! API for measuring partition scheduling jitter (actual vs expected tick time).
//!
//! Per D2, uses `Spinlock` + `UnsafeCell` (not `static mut`) for safe interior mutability.

use core::cell::UnsafeCell;

use crate::percore::Spinlock;

/// Aggregated jitter statistics (in microseconds).
///
/// `min_jitter_us`/`max_jitter_us` are initialized to `i64::MAX`/`i64::MIN`
/// respectively so that the first recorded sample becomes both the min and
/// the max. `samples == 0` indicates no data has been recorded yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JitterStats {
    /// Minimum observed jitter (µs).
    pub min_jitter_us: i64,
    /// Maximum observed jitter (µs).
    pub max_jitter_us: i64,
    /// Sum of all observed jitter samples (µs) — divide by `samples` for mean.
    pub sum_jitter_us: i64,
    /// Number of samples recorded.
    pub samples: u64,
}

impl JitterStats {
    /// Create the initial (empty) state.
    ///
    /// `const fn` so the static `JITTER` can be const-initialized.
    pub const fn new() -> Self {
        Self {
            min_jitter_us: i64::MAX,
            max_jitter_us: i64::MIN,
            sum_jitter_us: 0,
            samples: 0,
        }
    }
}

impl Default for JitterStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Interior-mutable wrapper around `JitterStats` protected by a `Spinlock`.
///
/// Mirrors the `ThreadTable` pattern from `tcb.rs`: the lock is a raw
/// `Spinlock` (no inner data) and the protected data lives in an
/// `UnsafeCell`. Callers must `lock()`/`unlock()` manually and may then
/// obtain a `&mut` reference to the inner data via `UnsafeCell::get`.
struct JitterState {
    lock: Spinlock,
    data: UnsafeCell<JitterStats>,
}

// SAFETY: The only access to `data` is gated by `lock`/`unlock` on `lock`,
// providing mutual exclusion. There is no shared mutable access without
// holding the lock.
unsafe impl Sync for JitterState {}

static JITTER: JitterState = JitterState {
    lock: Spinlock::new(),
    data: UnsafeCell::new(JitterStats::new()),
};

/// Record a single jitter sample (in microseconds).
///
/// Updates `min`/`max`/`sum`/`samples` atomically with respect to other
/// callers. Negative jitter (tick arrived early) is supported.
pub fn record_jitter(j_us: i64) {
    JITTER.lock.lock();
    // SAFETY: We hold the lock.
    let stats = unsafe { &mut *JITTER.data.get() };
    if j_us < stats.min_jitter_us {
        stats.min_jitter_us = j_us;
    }
    if j_us > stats.max_jitter_us {
        stats.max_jitter_us = j_us;
    }
    stats.sum_jitter_us += j_us;
    stats.samples += 1;
    JITTER.lock.unlock();
}

/// Take a consistent snapshot of the current jitter statistics.
///
/// Returns a `Copy` of the aggregated stats. Safe to call concurrently with
/// `record_jitter`/`jitter_reset`.
pub fn jitter_measure() -> JitterStats {
    JITTER.lock.lock();
    // SAFETY: We hold the lock.
    let stats = unsafe { &*JITTER.data.get() };
    let snapshot = *stats;
    JITTER.lock.unlock();
    snapshot
}

/// Reset all jitter statistics to the initial (empty) state.
pub fn jitter_reset() {
    JITTER.lock.lock();
    // SAFETY: We hold the lock.
    let stats = unsafe { &mut *JITTER.data.get() };
    *stats = JitterStats::new();
    JITTER.lock.unlock();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn reset_jitter() {
        JITTER.lock.lock();
        unsafe { &mut *JITTER.data.get() }.min_jitter_us = i64::MAX;
        unsafe { &mut *JITTER.data.get() }.max_jitter_us = i64::MIN;
        unsafe { &mut *JITTER.data.get() }.sum_jitter_us = 0;
        unsafe { &mut *JITTER.data.get() }.samples = 0;
        JITTER.lock.unlock();
    }

    #[test]
    fn test_record_single_jitter() {
        let _g = lock();
        reset_jitter();
        record_jitter(100);
        let stats = jitter_measure();
        assert_eq!(stats.min_jitter_us, 100);
        assert_eq!(stats.max_jitter_us, 100);
        assert_eq!(stats.sum_jitter_us, 100);
        assert_eq!(stats.samples, 1);
    }

    #[test]
    fn test_record_multiple_jitter() {
        let _g = lock();
        reset_jitter();
        record_jitter(100);
        record_jitter(200);
        record_jitter(50);
        let stats = jitter_measure();
        assert_eq!(stats.min_jitter_us, 50);
        assert_eq!(stats.max_jitter_us, 200);
        assert_eq!(stats.sum_jitter_us, 350);
        assert_eq!(stats.samples, 3);
    }

    #[test]
    fn test_jitter_reset() {
        let _g = lock();
        reset_jitter();
        record_jitter(100);
        record_jitter(200);
        // Sanity check we have data before reset.
        assert_eq!(jitter_measure().samples, 2);
        jitter_reset();
        let stats = jitter_measure();
        assert_eq!(stats.min_jitter_us, i64::MAX);
        assert_eq!(stats.max_jitter_us, i64::MIN);
        assert_eq!(stats.sum_jitter_us, 0);
        assert_eq!(stats.samples, 0);
    }

    #[test]
    fn test_empty_jitter_measure() {
        let _g = lock();
        reset_jitter();
        let stats = jitter_measure();
        assert_eq!(stats.min_jitter_us, i64::MAX);
        assert_eq!(stats.max_jitter_us, i64::MIN);
        assert_eq!(stats.sum_jitter_us, 0);
        assert_eq!(stats.samples, 0);
    }

    #[test]
    fn test_negative_jitter() {
        let _g = lock();
        reset_jitter();
        record_jitter(-50);
        record_jitter(100);
        let stats = jitter_measure();
        assert_eq!(stats.min_jitter_us, -50);
        assert_eq!(stats.max_jitter_us, 100);
        assert_eq!(stats.sum_jitter_us, 50);
        assert_eq!(stats.samples, 2);
    }

    #[test]
    fn test_jitter_measure_returns_copy() {
        let _g = lock();
        reset_jitter();
        record_jitter(42);
        let snapshot = jitter_measure();
        // `snapshot` is a `Copy` of the stats — mutating the live state must
        // not affect the previously-returned snapshot.
        record_jitter(1000);
        assert_eq!(snapshot.samples, 1);
        assert_eq!(snapshot.sum_jitter_us, 42);
        // The live state now reflects both samples.
        let updated = jitter_measure();
        assert_eq!(updated.samples, 2);
        assert_eq!(updated.sum_jitter_us, 1042);
    }
}
