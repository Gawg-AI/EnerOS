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
// SAFETY: 所有真实 `HalClock` 实现（如 `Arm64Timer`）为只读硬件单例，
// 无内部可变状态，跨线程共享安全（Send）。
unsafe impl Send for ClockRef {}
#[allow(clippy::non_send_fields_in_send_ty)]
// SAFETY: 同上——只读硬件单例共享引用可安全跨线程访问（Sync）。
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

/// 31 D5（review）：仅注入/更新 RTC 基址与墙钟偏移，**不**重置 CLOCK/MONO/WHEEL。
///
/// `time_init` 会重建 `TimerWheel`，boot 后再调会丢弃已注册定时器——故提供此
/// 轻量入口供 driver_probe rtc_factory 装配后调用。`rtc_base` 为已映射的 MMIO
/// **虚拟**地址（由 `map_mmio` 取得）；0 = 无 RTC，偏移留零。
pub fn set_rtc_base(rtc_base: u64) {
    *RTC_BASE.lock() = rtc_base;
    let offset = if rtc_base == 0 {
        0
    } else {
        Pl031Rtc::new(rtc_base).read_secs() * 1_000_000_000
    };
    *RTC_OFFSET_NS.lock() = offset;
}

/// SPEC-B WP-B.1：直接注入 Unix epoch 秒作为墙钟偏移——EL0 服务侧公共初始化面。
///
/// root 进程持有 PL031 MMIO 映射，EL0 服务进程没有；故 root 把经合理性闸门
/// 的 epoch 秒经启动寄存器（X6）下发给服务，服务在此以偏移形式生效。与
/// [`set_rtc_base`] 同思路：**不**重置 CLOCK/MONO/WHEEL、可重复调用
/// （restart_service 重灌寄存器路径每次新读新注入），`get_time()` =
/// 该偏移 + 单调增量。
///
/// 注意注入的是"可信时间值"而非硬件基址：服务侧无 MMIO 访问权，RTC 硬件
/// 真值仍由 root 侧 [`rtc_now_secs`] 唯一读取。
pub fn set_wall_epoch_secs(epoch_secs: u64) {
    *RTC_OFFSET_NS.lock() = epoch_secs.saturating_mul(1_000_000_000);
}

/// SPEC-B WP-B.1：实时读已配置 PL031 的当前 Unix epoch 秒（RTCDR 新读，
/// 非缓存偏移）。无 RTC 基址 → `None`——调用方按缺失降级，绝不猜测。
///
/// restart_service 重灌寄存器前调用本函数取新鲜墙钟，保证受控重启后
/// 服务侧墙钟继续前进不回退到 spawn 时刻的旧值。
pub fn rtc_now_secs() -> Option<u64> {
    let base = *RTC_BASE.lock();
    if base == 0 {
        None
    } else {
        Some(Pl031Rtc::new(base).read_secs())
    }
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

/// WP-53.5 INV-DRV：把 HalClock 单调钟读数喂进 holdover 计数器。
///
/// 装配层（root-task）在时钟子系统就绪后调用一次，此后由周期 tick
/// 持续调用——保证 holdover 读数可溯源到硬件路径，
/// 禁止冻结零值以正常类型流入上层决策。
pub fn holdover_sync_tick() {
    crate::holdover::sync_time(get_monotonic_ns());
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
        // SAFETY: `wfe` 不访问内存、无输出约束；副作用仅为进入低功耗
        // 等待事件，由后续事件唤醒，不改变可观察状态。
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

    /// WP-53.5 INV-DRV：`holdover_sync_tick` 必须把 HalClock 单调钟读数
    /// 喂进 holdover——生产读数可溯源到硬件路径，而非冻结零值冒充实测。
    #[test]
    fn test_wp535_holdover_sync_tick_feeds_from_hal_clock() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        struct FixedClock;
        /// 差分单调钟要求硬件时钟前进才产生非零读数——每次调用步进。
        static NOW: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
        const STEP: u64 = 10_000_000;
        impl HalClock for FixedClock {
            fn now_ns(&self) -> u64 {
                NOW.fetch_add(STEP, core::sync::atomic::Ordering::Relaxed) + STEP
            }
            fn frequency_hz(&self) -> u64 {
                1_000_000_000
            }
            fn set_deadline(&self, _ns: u64) -> Result<(), HalError> {
                Ok(())
            }
        }
        static FIXED_CLOCK: FixedClock = FixedClock;

        // 清 holdover 计数并置哨兵，隔离其他测试的残留。
        super::CLOCK.lock().0 = None;
        crate::holdover::NOW_NS.lock().clone_from(&999_999);

        time_init(&FIXED_CLOCK, 0);

        // 喂时：哨兵值必须被 HalClock 单调钟读数覆写
        //（单调钟为差分设计：init 消耗首次步进，读数 = n×STEP）。
        holdover_sync_tick();
        let fed = crate::holdover::now_ns();
        assert!(
            fed != 999_999 && fed % STEP == 0,
            "喂时后 holdover 读数必须来自 HalClock 单调钟（而非残留哨兵 {fed}）"
        );

        // 还原，避免污染其他测试。
        super::CLOCK.lock().0 = None;
        *super::MONO.lock() = None;
        crate::holdover::NOW_NS.lock().clone_from(&0);
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

    /// SPEC-B WP-B.1：`set_wall_epoch_secs` 注入墙钟偏移；`get_time` =
    /// 偏移 + 单调增量（TestClock 单调恒 0 → 读数恰为偏移）。
    /// 极大值走 saturating 不得 panic（垃圾时间在 root 闸门拦截，
    /// 本层只需数值健壮性兜底）。
    #[test]
    fn wpb1_set_wall_epoch_feeds_get_time_and_saturates() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        super::CLOCK.lock().0 = None;
        *super::MONO.lock() = None;
        time_init(&TEST_CLOCK, 0);

        const EPOCH: u64 = 1_800_000_000; // 2027-01-15 附近，闸门窗内
        set_wall_epoch_secs(EPOCH);
        assert_eq!(get_time(), TimeStamp(EPOCH * 1_000_000_000));

        // 饱和兜底：u64::MAX 秒 × 1e9 必须 clamp 到 u64::MAX 而非溢出回卷。
        set_wall_epoch_secs(u64::MAX);
        assert_eq!(get_time(), TimeStamp(u64::MAX));

        // 还原，避免污染其他测试。
        set_wall_epoch_secs(0);
        super::CLOCK.lock().0 = None;
        *super::MONO.lock() = None;
    }

    /// SPEC-B WP-B.1：无 RTC 基址时 `rtc_now_secs` 返回 None（显式缺失语义，
    /// 绝不猜测）。host 测试不触碰真实 MMIO——非零基址的读取路径由 target
    /// （QEMU 实弹 transcript）覆盖。
    #[test]
    fn wpb1_rtc_now_secs_none_without_base() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        *super::RTC_BASE.lock() = 0;
        assert_eq!(rtc_now_secs(), None);
    }
}
