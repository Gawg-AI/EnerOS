//! Partition scheduler — Phase 0 P0-F (v0.19.0).
//!
//! Implements ARINC 653-style time-triggered partition scheduling on top of
//! the v0.18.0 thread abstraction. A major frame (cycle of partition time
//! slots) is driven by a periodic timer; each tick advances to the next
//! partition slot.
//!
//! # Time Source Injection (D1)
//!
//! To preserve the crate's zero-external-dependency design, the time source
//! and timer registrar are injected via function pointers:
//! - [`set_time_source`] — injects `fn() -> u64` (monotonic nanoseconds)
//! - [`set_timer_registrar`] — injects `fn(u64, fn()) -> bool` (periodic timer)
//!
//! The caller (which has access to `eneros-time`) sets these at init time.
//! Host tests can use mock functions or call [`on_tick`] directly.
//!
//! # Design Decisions
//!
//! - D2: Uses `Spinlock` + `UnsafeCell` (not `static mut`)
//! - D7: `schedule_run` returns `Result<(), SchedError>` (NoTimerRegistrar if unset)
//! - D8: Non-bottleneck version — algorithms complete, jitter < 1ms deferred to QEMU
//! - D11: `switch_partition` records the switch; actual thread block/resume is caller's job

use core::cell::UnsafeCell;

use crate::isolation::SchedError;
use crate::jitter::record_jitter;
use crate::percore::Spinlock;
use crate::timeline::{MajorFrame, PartitionId};

// ---------------------------------------------------------------------------
// Time source injection (D1)
// ---------------------------------------------------------------------------

struct TimeSource {
    lock: Spinlock,
    get_ns: UnsafeCell<Option<fn() -> u64>>,
}

// SAFETY: The only access to `get_ns` is gated by `lock`/`unlock` on `lock`,
// providing mutual exclusion. No shared mutable access occurs without holding
// the lock.
unsafe impl Sync for TimeSource {}

static TIME_SOURCE: TimeSource = TimeSource {
    lock: Spinlock::new(),
    get_ns: UnsafeCell::new(None),
};

struct TimerRegistrar {
    lock: Spinlock,
    #[allow(clippy::type_complexity)]
    register: UnsafeCell<Option<fn(u64, fn()) -> bool>>,
}

// SAFETY: The only access to `register` is gated by `lock`/`unlock` on `lock`,
// providing mutual exclusion. No shared mutable access occurs without holding
// the lock.
unsafe impl Sync for TimerRegistrar {}

static TIMER_REGISTRAR: TimerRegistrar = TimerRegistrar {
    lock: Spinlock::new(),
    register: UnsafeCell::new(None),
};

/// Inject a monotonic time source (returns nanoseconds since boot).
pub fn set_time_source(f: fn() -> u64) {
    TIME_SOURCE.lock.lock();
    // SAFETY: We hold the lock, so exclusive access is guaranteed.
    unsafe {
        *TIME_SOURCE.get_ns.get() = Some(f);
    }
    TIME_SOURCE.lock.unlock();
}

/// Inject a periodic timer registrar.
///
/// The registrar takes a period in nanoseconds and a callback, returns
/// `true` if registration succeeded.
pub fn set_timer_registrar(f: fn(u64, fn()) -> bool) {
    TIMER_REGISTRAR.lock.lock();
    // SAFETY: We hold the lock, so exclusive access is guaranteed.
    unsafe {
        *TIMER_REGISTRAR.register.get() = Some(f);
    }
    TIMER_REGISTRAR.lock.unlock();
}

/// Returns current monotonic nanoseconds, or 0 if no time source injected.
fn now_ns() -> u64 {
    TIME_SOURCE.lock.lock();
    // SAFETY: We hold the lock, so the read is exclusive.
    let f = unsafe { *TIME_SOURCE.get_ns.get() };
    TIME_SOURCE.lock.unlock();
    match f {
        Some(f) => f(),
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Global scheduling state
// ---------------------------------------------------------------------------

struct FrameState {
    lock: Spinlock,
    frame: UnsafeCell<MajorFrame>,
    running: UnsafeCell<bool>,
}

// SAFETY: The only access to `frame` and `running` is gated by `lock`/`unlock`
// on `lock`, providing mutual exclusion. No shared mutable access occurs
// without holding the lock.
unsafe impl Sync for FrameState {}

static FRAME: FrameState = FrameState {
    lock: Spinlock::new(),
    frame: UnsafeCell::new(MajorFrame::new()),
    running: UnsafeCell::new(false),
};

struct CurrentPartitionState {
    lock: Spinlock,
    partition: UnsafeCell<Option<PartitionId>>,
    switch_count: UnsafeCell<u64>,
}

// SAFETY: The only access to `partition` and `switch_count` is gated by
// `lock`/`unlock` on `lock`, providing mutual exclusion. No shared mutable
// access occurs without holding the lock.
unsafe impl Sync for CurrentPartitionState {}

static CURRENT_PARTITION: CurrentPartitionState = CurrentPartitionState {
    lock: Spinlock::new(),
    partition: UnsafeCell::new(None),
    switch_count: UnsafeCell::new(0),
};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Add a partition time slot to the major frame.
///
/// Returns `Err(SchedError::SlotFull)` if 16 slots already exist.
pub fn schedule_add(partition: PartitionId, duration_ms: u32) -> Result<(), SchedError> {
    FRAME.lock.lock();
    // SAFETY: We hold the lock, so exclusive mutable access is guaranteed.
    let frame = unsafe { &mut *FRAME.frame.get() };
    let result = frame.add_slot(partition, duration_ms);
    FRAME.lock.unlock();
    result
}

/// Start partition scheduling.
///
/// Initializes the frame start time and registers the first periodic timer.
/// Returns `Err(SchedError::NoTimerRegistrar)` if no timer registrar has been
/// injected (the frame is still initialized, allowing manual `on_tick` calls
/// for testing).
pub fn schedule_run() -> Result<(), SchedError> {
    // Read timer registrar (don't hold lock while calling it)
    TIMER_REGISTRAR.lock.lock();
    // SAFETY: We hold the lock, so the read is exclusive.
    let registrar = unsafe { *TIMER_REGISTRAR.register.get() };
    TIMER_REGISTRAR.lock.unlock();

    FRAME.lock.lock();
    // SAFETY: We hold the lock, so exclusive mutable access is guaranteed.
    let frame = unsafe { &mut *FRAME.frame.get() };
    frame.frame_start_ns = now_ns();
    frame.current_slot = 0;
    // SAFETY: We hold the FRAME lock, which protects the `running` flag.
    unsafe {
        *FRAME.running.get() = true;
    }
    // Set initial partition
    let first_partition = frame.current_partition();
    let first_duration_ns = frame.current_duration_ns();
    FRAME.lock.unlock();

    // Update current partition
    if let Some(p) = first_partition {
        switch_partition(p);
    }

    match registrar {
        Some(reg) => {
            reg(first_duration_ns, on_tick);
            Ok(())
        }
        None => Err(SchedError::NoTimerRegistrar),
    }
}

/// Stop partition scheduling.
///
/// Sets the running flag to false. The caller is responsible for cancelling
/// any registered periodic timer.
pub fn schedule_stop() {
    FRAME.lock.lock();
    // SAFETY: We hold the lock, so exclusive access is guaranteed.
    unsafe {
        *FRAME.running.get() = false;
    }
    FRAME.lock.unlock();
}

/// Returns the currently active partition, or `None` if scheduling hasn't
/// started or the frame is empty.
pub fn current_partition() -> Option<PartitionId> {
    CURRENT_PARTITION.lock.lock();
    // SAFETY: We hold the lock, so the read is exclusive.
    let p = unsafe { *CURRENT_PARTITION.partition.get() };
    CURRENT_PARTITION.lock.unlock();
    p
}

/// Returns the total number of partition switches since `schedule_run`.
pub fn switch_count() -> u64 {
    CURRENT_PARTITION.lock.lock();
    // SAFETY: We hold the lock, so the read is exclusive.
    let c = unsafe { *CURRENT_PARTITION.switch_count.get() };
    CURRENT_PARTITION.lock.unlock();
    c
}

/// Timer tick callback — advances to the next partition slot.
///
/// Computes jitter (actual - expected time), records it, advances the slot,
/// and switches to the new partition. Called by the periodic timer.
pub fn on_tick() {
    let now = now_ns();

    FRAME.lock.lock();
    // SAFETY: We hold the lock, so exclusive mutable access is guaranteed.
    let frame = unsafe { &mut *FRAME.frame.get() };
    // SAFETY: We hold the FRAME lock, which protects the `running` flag.
    let running = unsafe { *FRAME.running.get() };
    if !running {
        FRAME.lock.unlock();
        return;
    }

    // Calculate jitter: expected = frame_start + current_slot_duration.
    // For D8 (non-bottleneck), this approximation is acceptable; a more
    // accurate approach would track the absolute expected tick time across
    // multiple slot cycles.
    let slot_duration_ns = frame.current_duration_ns();
    let expected = frame.frame_start_ns + slot_duration_ns;
    let jitter_us = if now >= expected {
        ((now - expected) / 1000) as i64
    } else {
        -(((expected - now) / 1000) as i64)
    };

    // Advance slot
    let new_slot = frame.advance_slot();
    // If we wrapped to 0, update frame_start
    if new_slot == 0 {
        frame.frame_start_ns = now;
    }

    let new_partition = frame.current_partition();
    FRAME.lock.unlock();

    // Record jitter
    record_jitter(jitter_us);

    // Switch partition
    if let Some(p) = new_partition {
        switch_partition(p);
    }
}

/// Switch to a new partition (internal).
///
/// Per D11, this only records the switch (sets current partition, increments
/// count). Actual thread block/resume is the caller's responsibility.
fn switch_partition(partition: PartitionId) {
    CURRENT_PARTITION.lock.lock();
    // SAFETY: We hold the lock, so exclusive mutable access is guaranteed.
    unsafe {
        *CURRENT_PARTITION.partition.get() = Some(partition);
        *CURRENT_PARTITION.switch_count.get() += 1;
    }
    CURRENT_PARTITION.lock.unlock();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jitter::jitter_reset;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn reset_state() {
        FRAME.lock.lock();
        // SAFETY: We hold the lock, so exclusive mutable access is guaranteed.
        unsafe {
            *FRAME.frame.get() = MajorFrame::new();
            *FRAME.running.get() = false;
        }
        FRAME.lock.unlock();
        CURRENT_PARTITION.lock.lock();
        // SAFETY: We hold the lock, so exclusive mutable access is guaranteed.
        unsafe {
            *CURRENT_PARTITION.partition.get() = None;
            *CURRENT_PARTITION.switch_count.get() = 0;
        }
        CURRENT_PARTITION.lock.unlock();
        // Reset time source
        TIME_SOURCE.lock.lock();
        // SAFETY: We hold the lock, so exclusive access is guaranteed.
        unsafe {
            *TIME_SOURCE.get_ns.get() = None;
        }
        TIME_SOURCE.lock.unlock();
        // Reset timer registrar
        TIMER_REGISTRAR.lock.lock();
        // SAFETY: We hold the lock, so exclusive access is guaranteed.
        unsafe {
            *TIMER_REGISTRAR.register.get() = None;
        }
        TIMER_REGISTRAR.lock.unlock();
        jitter_reset();
    }

    static mut MOCK_TIME: u64 = 0;

    fn mock_time_impl() -> u64 {
        // SAFETY: Tests are single-threaded (guarded by TEST_LOCK).
        unsafe { MOCK_TIME }
    }

    fn mock_time(ns: u64) -> fn() -> u64 {
        // SAFETY: Tests are single-threaded (guarded by TEST_LOCK).
        unsafe {
            MOCK_TIME = ns;
        }
        mock_time_impl
    }

    fn mock_registrar(_ns: u64, _cb: fn()) -> bool {
        true
    }

    #[test]
    fn test_schedule_add_success() {
        let _g = lock();
        reset_state();
        assert!(schedule_add(PartitionId(0), 5).is_ok());
        assert!(schedule_add(PartitionId(1), 10).is_ok());
    }

    #[test]
    fn test_schedule_add_overflow() {
        let _g = lock();
        reset_state();
        for i in 0..16 {
            assert!(schedule_add(PartitionId(i as u32), 1).is_ok());
        }
        assert_eq!(schedule_add(PartitionId(16), 1), Err(SchedError::SlotFull));
    }

    #[test]
    fn test_schedule_run_no_timer_registrar() {
        let _g = lock();
        reset_state();
        schedule_add(PartitionId(0), 5).unwrap();
        assert_eq!(schedule_run(), Err(SchedError::NoTimerRegistrar));
    }

    #[test]
    fn test_schedule_run_with_mock_timer() {
        let _g = lock();
        reset_state();
        set_timer_registrar(mock_registrar);
        schedule_add(PartitionId(0), 5).unwrap();
        assert!(schedule_run().is_ok());
    }

    #[test]
    fn test_on_tick_advances_slot() {
        let _g = lock();
        reset_state();
        set_time_source(mock_time(0));
        set_timer_registrar(mock_registrar);
        schedule_add(PartitionId(0), 5).unwrap();
        schedule_add(PartitionId(1), 10).unwrap();
        schedule_add(PartitionId(2), 15).unwrap();
        schedule_run().unwrap();
        let initial_count = switch_count();
        let initial_partition = current_partition();
        on_tick();
        assert!(switch_count() > initial_count);
        assert_ne!(current_partition(), initial_partition);
    }

    #[test]
    fn test_on_tick_not_running() {
        let _g = lock();
        reset_state();
        schedule_add(PartitionId(0), 5).unwrap();
        on_tick();
        assert_eq!(switch_count(), 0);
    }

    #[test]
    fn test_current_partition_initial_none() {
        let _g = lock();
        reset_state();
        assert_eq!(current_partition(), None);
    }

    #[test]
    fn test_switch_count() {
        let _g = lock();
        reset_state();
        set_time_source(mock_time(0));
        set_timer_registrar(mock_registrar);
        schedule_add(PartitionId(0), 5).unwrap();
        schedule_add(PartitionId(1), 10).unwrap();
        schedule_run().unwrap();
        on_tick();
        on_tick();
        assert_eq!(switch_count(), 3);
    }
}
