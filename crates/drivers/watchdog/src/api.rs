//! Global watchdog API.
//!
//! Thin wrapper around [`Watchdog`] that stores a single global instance
//! protected by a [`spin::Mutex`]. All functions are safe to call before
//! [`wdt_init`]: they return early or yield a safe default.

use spin::Mutex;

use crate::layered::{LayerId, Watchdog, WatchdogStatus};
use crate::wdt::HwWatchdog;

static WATCHDOG: Mutex<Watchdog> = Mutex::new(Watchdog::new(HwWatchdog::new(0), 0));
static INITIALIZED: Mutex<bool> = Mutex::new(false);

/// Initialize the global watchdog with hardware base `wdt_base` and
/// `timeout_ms` reset threshold.
///
/// `wdt_base == 0` selects software-only mode (no MMIO writes).
/// `hard_timeout_ms` is set to `timeout_ms` per spec D7.
pub fn wdt_init(timeout_ms: u32, wdt_base: u64) {
    let hw = HwWatchdog::new(wdt_base);
    hw.init(timeout_ms);
    *WATCHDOG.lock() = Watchdog::new(hw, timeout_ms);
    *INITIALIZED.lock() = true;
}

/// Kick the hardware watchdog directly. No-op if not initialized.
pub fn wdt_kick() {
    if !*INITIALIZED.lock() {
        return;
    }
    WATCHDOG.lock().hw.kick();
}

/// Register a new feed layer. Returns `None` if not initialized or all 8
/// slots are occupied.
pub fn wdt_register_layer(name: &'static str, period_ms: u32) -> Option<LayerId> {
    if !*INITIALIZED.lock() {
        return None;
    }
    WATCHDOG.lock().register_layer(name, period_ms)
}

/// Record a feed event for `id` at the current monotonic time.
/// No-op if not initialized.
pub fn wdt_feed_layer(id: LayerId) {
    if !*INITIALIZED.lock() {
        return;
    }
    let now_ns = eneros_time::get_monotonic_ns();
    WATCHDOG.lock().feed_layer(id, now_ns);
}

/// Inspect all enabled layers and drive the hardware watchdog accordingly.
/// Returns [`WatchdogStatus::AllFed`] if not initialized (safe default).
pub fn wdt_check() -> WatchdogStatus {
    if !*INITIALIZED.lock() {
        return WatchdogStatus::AllFed;
    }
    let now_ns = eneros_time::get_monotonic_ns();
    WATCHDOG.lock().check(now_ns)
}

/// Stop the hardware watchdog and mark the subsystem as uninitialized.
/// No-op if not initialized.
pub fn wdt_stop() {
    if !*INITIALIZED.lock() {
        return;
    }
    WATCHDOG.lock().hw.stop();
    *INITIALIZED.lock() = false;
}

/// Returns the number of registered feed layers, or `0` if not initialized.
pub fn wdt_layer_count() -> usize {
    if !*INITIALIZED.lock() {
        return 0;
    }
    WATCHDOG.lock().layers.iter().flatten().count()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;

    // Production statics are shared `spin::Mutex`-protected state. Tests
    // serialize on this guard to avoid cross-test races.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    fn reset_state() {
        *WATCHDOG.lock() = Watchdog::new(HwWatchdog::new(0), 0);
        *INITIALIZED.lock() = false;
    }

    #[test]
    fn test_uninitialized_no_op() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        // All operations must be safe no-ops before wdt_init.
        wdt_kick();
        wdt_feed_layer(LayerId(1));
        wdt_stop();
        assert_eq!(wdt_register_layer("x", 100), None);
        assert_eq!(wdt_check(), WatchdogStatus::AllFed);
        assert_eq!(wdt_layer_count(), 0);
    }

    #[test]
    fn test_init_register_feed_check() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        wdt_init(1000, 0); // base=0 → software mode, no real MMIO
        assert_eq!(wdt_layer_count(), 0);

        let id1 = wdt_register_layer("kernel", 100);
        assert!(id1.is_some());
        assert_eq!(wdt_layer_count(), 1);

        let id2 = wdt_register_layer("runtime", 200);
        assert!(id2.is_some());
        assert_eq!(wdt_layer_count(), 2);

        // Feed both layers and verify AllFed. `eneros_time::get_monotonic_ns()`
        // returns 0 in tests (time service not initialized), so last_feed_ns
        // becomes 0 and elapsed=0 < period → AllFed.
        wdt_feed_layer(id1.unwrap());
        wdt_feed_layer(id2.unwrap());
        assert_eq!(wdt_check(), WatchdogStatus::AllFed);
    }

    #[test]
    fn test_layer_count_tracking() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        wdt_init(5000, 0);
        assert_eq!(wdt_layer_count(), 0);

        let ids: Vec<_> = (0..5)
            .map(|i| wdt_register_layer("layer", 100 * (i + 1)))
            .collect();
        assert_eq!(wdt_layer_count(), 5);
        assert!(ids.iter().all(|id| id.is_some()));
    }

    #[test]
    fn test_wdt_stop_resets_state() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        wdt_init(1000, 0);
        let _ = wdt_register_layer("kernel", 100);
        assert_eq!(wdt_layer_count(), 1);

        wdt_stop();
        // After stop, subsystem is uninitialized → defaults again.
        assert_eq!(wdt_layer_count(), 0);
        assert_eq!(wdt_check(), WatchdogStatus::AllFed);
        assert_eq!(wdt_register_layer("post", 100), None);
        wdt_kick(); // must not panic
    }

    #[test]
    fn test_wdt_api_integration() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        // (a) Uninitialized no-op
        wdt_kick();
        assert_eq!(wdt_register_layer("x", 100), None);
        assert_eq!(wdt_check(), WatchdogStatus::AllFed);
        assert_eq!(wdt_layer_count(), 0);

        // (b) Init + register + feed + check AllFed
        wdt_init(1000, 0);
        let id = wdt_register_layer("kernel", 100).unwrap();
        assert_eq!(wdt_layer_count(), 1);
        wdt_feed_layer(id);
        assert_eq!(wdt_check(), WatchdogStatus::AllFed);

        // (c) Additional layer registration
        let id2 = wdt_register_layer("runtime", 200).unwrap();
        assert_eq!(wdt_layer_count(), 2);
        wdt_feed_layer(id2);
        assert_eq!(wdt_check(), WatchdogStatus::AllFed);

        // (d) Stop resets initialized state
        wdt_stop();
        assert_eq!(wdt_layer_count(), 0);
        assert_eq!(wdt_check(), WatchdogStatus::AllFed);

        // (e) Re-init works after stop
        wdt_init(2000, 0);
        let id3 = wdt_register_layer("post_stop", 50).unwrap();
        wdt_feed_layer(id3);
        assert_eq!(wdt_check(), WatchdogStatus::AllFed);
        assert_eq!(wdt_layer_count(), 1);
    }
}
