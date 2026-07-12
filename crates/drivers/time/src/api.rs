//! High-level time API: get_time, sleep_until, register_timer, etc.

use eneros_hal::HalClock;
use spin::Mutex;

use crate::hrtimer::{TimerId, TimerWheel};
use crate::monotonic::MonotonicClock;
use crate::rtc::{secs_to_rtc, Pl031Rtc, RtcTime, TimeStamp};

/// Wrapper that allows a `&'static dyn HalClock` to be stored in a `Sync`
/// static. This is sound because every real `HalClock` implementation (e.g.
/// `Arm64Timer`) is a read-only hardware singleton and therefore `Send + Sync`.
struct ClockRef(Option<&'static dyn HalClock>);
unsafe impl Send for ClockRef {}
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Sync for ClockRef {}

static CLOCK: Mutex<ClockRef> = Mutex::new(ClockRef(None));
static MONO: Mutex<Option<MonotonicClock>> = Mutex::new(None);
static WHEEL: Mutex<TimerWheel> = Mutex::new(TimerWheel::new());
static RTC_OFFSET_NS: Mutex<u64> = Mutex::new(0);
static RTC_BASE: Mutex<u64> = Mutex::new(0);

/// Initialize the time subsystem with a HAL clock source and PL031 RTC base.
///
/// Records the RTC seconds count (converted to nanoseconds) as the wall-clock
/// offset. If `rtc_base` is `0`, the offset is left at zero (no RTC present).
pub fn time_init(clock: &'static dyn HalClock, rtc_base: u64) {
    CLOCK.lock().0 = Some(clock);
    *MONO.lock() = Some(MonotonicClock::init(clock));
    *WHEEL.lock() = TimerWheel::new();
    *RTC_BASE.lock() = rtc_base;
    let offset = if rtc_base == 0 {
        0
    } else {
        Pl031Rtc::new(rtc_base).read_secs() * 1_000_000_000
    };
    *RTC_OFFSET_NS.lock() = offset;
}

/// Returns monotonic nanoseconds since boot, or `0` before `time_init`.
pub fn get_monotonic_ns() -> u64 {
    let clock = CLOCK.lock().0;
    let clock = match clock {
        Some(c) => c,
        None => return 0,
    };
    let mono = MONO.lock();
    match &*mono {
        Some(m) => m.now_ns(clock),
        None => 0,
    }
}

/// Returns the wall-clock timestamp as `RTC_OFFSET_NS + monotonic_ns`.
pub fn get_time() -> TimeStamp {
    TimeStamp(*RTC_OFFSET_NS.lock() + get_monotonic_ns())
}

/// Busy-wait until the monotonic clock reaches `deadline_ns`.
///
/// On aarch64 this uses the `wfe` instruction; on other architectures it
/// spins with `spin_loop` hints.
pub fn sleep_until(deadline_ns: u64) {
    while get_monotonic_ns() < deadline_ns {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("wfe");
        }
        #[cfg(not(target_arch = "aarch64"))]
        core::hint::spin_loop();
    }
}

/// Register a one-shot timer firing at `deadline_ns`. Returns the timer id,
/// or `None` if the timer wheel is full.
pub fn register_timer(deadline_ns: u64, cb: fn()) -> Option<TimerId> {
    WHEEL.lock().add(deadline_ns, cb, false, 0)
}

/// Register a periodic timer firing every `period_ns` (first fire one period
/// from now). Returns the timer id, or `None` if the wheel is full.
pub fn register_periodic(period_ns: u64, cb: fn()) -> Option<TimerId> {
    let deadline = get_monotonic_ns().saturating_add(period_ns);
    WHEEL.lock().add(deadline, cb, true, period_ns)
}

/// Cancel the timer identified by `id`. No-op if the id is not registered.
pub fn cancel_timer(id: TimerId) {
    WHEEL.lock().cancel(id);
}

/// Read the current wall-clock time from the PL031 RTC. If no RTC base was
/// configured, returns the Unix epoch (1970-01-01 00:00:00 Thursday).
pub fn rtc_read() -> RtcTime {
    let base = *RTC_BASE.lock();
    if base == 0 {
        secs_to_rtc(0)
    } else {
        Pl031Rtc::new(base).read()
    }
}

/// Write `t` to the PL031 RTC (time calibration). No-op if no RTC base.
pub fn rtc_write(t: RtcTime) {
    let base = *RTC_BASE.lock();
    if base != 0 {
        Pl031Rtc::new(base).write(t);
    }
}

/// Returns the total number of timers that have expired since `time_init`.
pub fn timer_expired_count() -> u64 {
    WHEEL.lock().expired_count
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use eneros_hal::{HalClock, HalError};

    use super::*;
    use crate::rtc::TimeStamp;

    // The production statics below are shared `spin::Mutex`-protected state.
    // Tests therefore serialize on this guard to avoid cross-test races.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    struct TestClock;
    impl HalClock for TestClock {
        fn now_ns(&self) -> u64 {
            0
        }
        fn frequency_hz(&self) -> u64 {
            1_000_000_000
        }
        fn set_deadline(&self, _ns: u64) -> Result<(), HalError> {
            Ok(())
        }
    }
    static TEST_CLOCK: TestClock = TestClock;

    fn dummy_cb() {}

    #[test]
    fn test_get_monotonic_ns_before_init_returns_zero() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        super::CLOCK.lock().0 = None;
        *super::MONO.lock() = None;
        assert_eq!(get_monotonic_ns(), 0);
    }

    #[test]
    fn test_get_time_before_init_returns_zero() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        super::CLOCK.lock().0 = None;
        *super::MONO.lock() = None;
        *super::RTC_OFFSET_NS.lock() = 0;
        assert_eq!(get_time(), TimeStamp(0));
    }

    #[test]
    fn test_time_init_and_get_monotonic_ns() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        time_init(&TEST_CLOCK, 0);
        // TestClock.now_ns() always returns 0, so the monotonic delta is 0
        // regardless of the exact MonotonicClock arithmetic.
        assert_eq!(get_monotonic_ns(), 0);
    }

    #[test]
    fn test_register_and_cancel_timer() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        time_init(&TEST_CLOCK, 0);
        let id = register_timer(1_000_000_000, dummy_cb);
        assert!(id.is_some());
        if let Some(tid) = id {
            cancel_timer(tid);
        }
    }

    #[test]
    fn test_timer_expired_count_initial_zero() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        time_init(&TEST_CLOCK, 0);
        assert_eq!(timer_expired_count(), 0);
    }
}
