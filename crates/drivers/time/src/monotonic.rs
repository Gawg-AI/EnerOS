//! Monotonic clock based on HAL `HalClock` trait.

use eneros_hal::HalClock;

pub struct MonotonicClock {
    boot_ns: u64,
}

impl MonotonicClock {
    /// Capture the boot timestamp from `clock` and return a new clock.
    pub fn init(clock: &dyn HalClock) -> Self {
        Self {
            boot_ns: clock.now_ns(),
        }
    }

    /// Returns nanoseconds elapsed since [`init`], saturating at 0 if the
    /// underlying clock reports a value below the boot timestamp.
    pub fn now_ns(&self, clock: &dyn HalClock) -> u64 {
        clock.now_ns().saturating_sub(self.boot_ns)
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicU64, Ordering};

    use eneros_hal::HalClock;

    use super::MonotonicClock;

    /// Local mutable clock mock — `MockHal.now_ns()` always returns 0, so we
    /// back the monotonic tests with an atomic-backed counter instead.
    struct TestClock {
        now: AtomicU64,
    }

    impl TestClock {
        const fn new() -> Self {
            Self {
                now: AtomicU64::new(0),
            }
        }
        fn set(&self, ns: u64) {
            self.now.store(ns, Ordering::SeqCst);
        }
    }

    impl HalClock for TestClock {
        fn now_ns(&self) -> u64 {
            self.now.load(Ordering::SeqCst)
        }
        fn frequency_hz(&self) -> u64 {
            1_000_000_000
        }
        fn set_deadline(&self, _ns: u64) -> Result<(), eneros_hal::HalError> {
            Ok(())
        }
    }

    #[test]
    fn test_init_records_boot_ns() {
        let clock = TestClock::new();
        clock.set(1_000);
        let mono = MonotonicClock::init(&clock);
        assert_eq!(mono.boot_ns, 1_000);
    }

    #[test]
    fn test_now_ns_returns_zero_at_init() {
        let clock = TestClock::new();
        clock.set(5_000);
        let mono = MonotonicClock::init(&clock);
        assert_eq!(mono.now_ns(&clock), 0);
    }

    #[test]
    fn test_now_ns_monotonic() {
        let clock = TestClock::new();
        clock.set(1_000);
        let mono = MonotonicClock::init(&clock);
        let t1 = mono.now_ns(&clock);
        clock.set(1_500);
        let t2 = mono.now_ns(&clock);
        assert!(t2 >= t1);
        assert_eq!(t1, 0);
        assert_eq!(t2, 500);
    }

    #[test]
    fn test_now_ns_with_offset() {
        let clock = TestClock::new();
        clock.set(10_000);
        let mono = MonotonicClock::init(&clock);
        clock.set(12_345);
        assert_eq!(mono.now_ns(&clock), 2_345);
    }

    #[test]
    fn test_now_ns_saturating() {
        let clock = TestClock::new();
        clock.set(1_000);
        let mono = MonotonicClock::init(&clock);
        // Underlying clock rolled back below boot_ns: must not panic, returns 0.
        clock.set(500);
        assert_eq!(mono.now_ns(&clock), 0);
    }
}
