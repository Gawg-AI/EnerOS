//! EnerOS Multi-core Scheduler — Phase 0 P0-F (v0.16.0 + v0.18.0 + v0.19.0, ★ bottleneck for v0.16.0/v0.18.0).
//!
//! This crate provides:
//! - **Per-core run queues** ([`percore::PerCoreRq`]) protected by a custom
//!   TAS spinlock ([`percore::Spinlock`])
//! - **Core affinity** ([`affinity::CoreMask`]) to restrict which cores a
//!   thread may run on
//! - **Core reservation** ([`isolation::CoreReservation`]) to pin an RTOS
//!   onto a dedicated core (e.g. Core 0)
//! - **Load balancing** ([`balance::Balancer`]) that migrates threads from
//!   the busiest to the idlest core
//! - **Global scheduler** ([`Scheduler`]) tying the above together
//! - **Thread abstraction** (v0.18.0): [`tcb::Tcb`], [`tcb::ThreadState`],
//!   [`switch::context_switch`], [`priority::select_next_by_priority`], and
//!   global thread management API ([`thread_create`], [`thread_block`], etc.)
//! - **Partition scheduling** (v0.19.0): [`partition_sched::schedule_add`],
//!   [`partition_sched::schedule_run`], [`partition_sched::on_tick`],
//!   [`jitter::JitterStats`], [`wcet::wcet_estimate`], and time-source
//!   injection ([`partition_sched::set_time_source`])
//!
//! # Design Decisions
//!
//! - **D1**: Custom `Spinlock` (not `spin::Mutex`) so `PerCoreRq::new` is
//!   `const fn`, enabling `[PerCoreRq; 8]` const initialization.
//! - **D2**: Zero external dependencies — production code uses `core::*`
//!   and `alloc::*` only (no `spin` or `heapless`). `alloc` is a Rust
//!   built-in, not an external dep.
//! - **D3**: Does not depend on `eneros-smp`; `core_count` is passed into
//!   [`sched_init`]. IPI integration is the caller's responsibility.
//!   aarch64 inline asm is cfg-gated; host builds get stubs.
//! - **D4**: Bottleneck version (★) — code is "skeleton usable", no
//!   `todo!()`/`unimplemented!()` stubs; key algorithms (CAS+backoff lock,
//!   load scan + migration) are complete.
//! - **D5**: `Tcb` contains raw pointers and does NOT auto-impl `Send`/`Sync`.
//!   Global access is serialized through the `THREAD_TABLE` spinlock.
//! - **D6**: Partition scheduler uses function-pointer injection for the time
//!   source (not a crate dependency), preserving zero-external-deps.
//! - **D7**: `schedule_run` returns `Result<(), SchedError>` — `NoTimerRegistrar`
//!   if no timer registrar has been injected.
//!
//! # Usage
//!
//! ```ignore
//! use eneros_sched::{sched_init, enqueue, pick_next, reserve_core, Tid};
//!
//! let mut sched = sched_init(4);
//! reserve_core(&mut sched, 0); // Core 0 = RTOS-exclusive
//! enqueue(&mut sched, Tid(1), 1); // Agent thread onto Core 1
//! let next = pick_next(&mut sched, 1); // Some(Tid(1))
//! ```

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod affinity;
pub mod balance;
pub mod isolation;
pub mod jitter;
pub mod partition_sched;
pub mod percore;
pub mod priority;
pub mod switch;
pub mod tcb;
pub mod timeline;
pub mod wcet;

pub use affinity::CoreMask;
pub use balance::Balancer;
pub use isolation::{CoreReservation, SchedError};
pub use jitter::{jitter_measure, jitter_reset, record_jitter, JitterStats};
pub use partition_sched::{
    current_partition, on_tick, schedule_add, schedule_run, schedule_stop, set_time_source,
    set_timer_registrar, switch_count,
};
pub use percore::{PerCoreRq, Spinlock, Tid, RQ_CAPACITY};
pub use priority::{select_next_by_priority, PriorityQueue, PRIO_QUEUE_CAPACITY};
pub use switch::{context_switch, thread_switch};
pub use tcb::{
    current_tid, init_stack_frame, set_current_tid, thread_block, thread_create, thread_destroy,
    thread_exit, thread_partition, thread_resume, thread_state, thread_yield, Tcb, ThreadState,
};
pub use timeline::{MajorFrame, PartitionId, PartitionSlot, MAX_SLOTS};
pub use wcet::{check_partition_overrun, wcet_estimate, wcet_set};

/// Maximum number of cores supported by the scheduler.
pub const MAX_CORES: usize = 8;

/// Maximum number of threads tracked for affinity.
pub const MAX_THREADS: usize = 256;

/// Global multi-core scheduler.
///
/// Holds the per-core run queues (up to 8), the configured core count, the
/// core reservation table, the load balancer, and a per-thread affinity
/// table (256 entries, indexed by `Tid.0`).
#[derive(Debug)]
pub struct Scheduler {
    /// Per-core run queues (indices `0..core_count` are active).
    pub rqs: [PerCoreRq; MAX_CORES],
    /// Number of active cores (`<= MAX_CORES`).
    pub core_count: u32,
    /// Core reservation table (RTOS-exclusive cores).
    pub reservation: CoreReservation,
    /// Load balancer instance.
    pub balancer: Balancer,
    /// Per-thread affinity mask (`affinity[tid]` restricts the cores `tid`
    /// may run on; `CoreMask::default()` means "any core").
    pub affinity: [CoreMask; MAX_THREADS],
}

/// Initialize a scheduler with `core_count` active cores.
///
/// All run queues start empty, no cores are reserved, the balancer uses
/// defaults (threshold = 2, interval = 10 ms), and all affinity masks are
/// cleared (any core). Cores beyond `MAX_CORES` are silently capped.
pub fn sched_init(core_count: u32) -> Scheduler {
    let core_count = if core_count as usize > MAX_CORES {
        MAX_CORES as u32
    } else {
        core_count
    };
    let mut rqs = [
        PerCoreRq::new(0),
        PerCoreRq::new(1),
        PerCoreRq::new(2),
        PerCoreRq::new(3),
        PerCoreRq::new(4),
        PerCoreRq::new(5),
        PerCoreRq::new(6),
        PerCoreRq::new(7),
    ];
    for (i, rq) in rqs.iter_mut().enumerate().take(core_count as usize) {
        rq.core_id = i as u32;
    }
    Scheduler {
        rqs,
        core_count,
        reservation: CoreReservation::new(),
        balancer: Balancer::default(),
        affinity: [CoreMask::default(); MAX_THREADS],
    }
}

/// Set the affinity mask for `tid`.
///
/// Returns [`Err(SchedError::NoRunnableTask)`](SchedError::NoRunnableTask) if
/// `tid.0 >= MAX_THREADS` (256).
pub fn set_affinity(sched: &mut Scheduler, tid: Tid, cores: CoreMask) -> Result<(), SchedError> {
    if tid.0 as usize >= MAX_THREADS {
        return Err(SchedError::NoRunnableTask);
    }
    sched.affinity[tid.0 as usize] = cores;
    Ok(())
}

/// Pin `tid` to a single core (shorthand for `set_affinity(tid, single(core))`).
///
/// Returns [`Err(SchedError::InvalidCore)`](SchedError::InvalidCore) if
/// `core >= core_count`.
pub fn pin_to_core(sched: &mut Scheduler, tid: Tid, core: u32) -> Result<(), SchedError> {
    if core >= sched.core_count {
        return Err(SchedError::InvalidCore);
    }
    set_affinity(sched, tid, CoreMask::single(core))
}

/// Reserve `core` for RTOS-exclusive use.
///
/// Delegates to [`CoreReservation::reserve`]; see its docs for error cases.
pub fn reserve_core(sched: &mut Scheduler, core: u32) -> Result<(), SchedError> {
    sched.reservation.reserve(core)
}

/// Release a previously reserved core.
pub fn release_core(sched: &mut Scheduler, core: u32) {
    sched.reservation.release(core)
}

/// Enqueue `tid` onto `core`'s run queue.
///
/// Non-RTOS threads (`is_rtos = false`, the default for this simplified
/// API) are rejected on reserved cores — the enqueue is silently dropped.
/// Out-of-range `core` is also silently dropped (defensive: callers should
/// validate `core` separately).
pub fn enqueue(sched: &mut Scheduler, tid: Tid, core: u32) {
    // D4 simplification: is_rtos is always false here. A real implementation
    // would read the thread's RTOS flag from its TCB.
    if !sched.reservation.can_enqueue(core, false) {
        return;
    }
    if core as usize >= MAX_CORES {
        return;
    }
    let rq = &mut sched.rqs[core as usize];
    rq.lock.lock();
    rq.enqueue(tid);
    rq.lock.unlock();
}

/// Remove `tid` from whichever core's run queue it currently occupies.
///
/// Searches all active cores (`0..core_count`) and removes the first
/// occurrence. No-op if `tid` is not runnable on any core.
pub fn dequeue(sched: &mut Scheduler, tid: Tid) {
    for rq in sched.rqs.iter_mut().take(sched.core_count as usize) {
        rq.lock.lock();
        let removed = rq.remove(tid);
        rq.lock.unlock();
        if removed {
            return;
        }
    }
}

/// Pick the next runnable thread on `core`.
///
/// Dequeues one thread from `core`'s run queue (under its lock), records it
/// as `current` on that core, and returns it. Returns `None` if the queue is
/// empty or `core` is out of range.
pub fn pick_next(sched: &mut Scheduler, core: u32) -> Option<Tid> {
    if core as usize >= MAX_CORES {
        return None;
    }
    let rq = &mut sched.rqs[core as usize];
    rq.lock.lock();
    let tid = rq.dequeue();
    rq.lock.unlock();
    if let Some(t) = tid {
        rq.current = Some(t);
    }
    tid
}

/// Trigger a single load-balance pass across all active cores.
pub fn balance_load(sched: &mut Scheduler) {
    let core_count = sched.core_count;
    sched.balancer.balance(&mut sched.rqs, core_count);
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

    #[test]
    fn test_sched_init_defaults() {
        let _g = lock();
        let s = sched_init(4);
        assert_eq!(s.core_count, 4);
        assert_eq!(s.rqs[0].core_id, 0);
        assert_eq!(s.rqs[3].core_id, 3);
        for rq in &s.rqs {
            assert_eq!(rq.load(), 0);
            assert!(!rq.reserved);
        }
        assert_eq!(s.balancer.threshold, 2);
        assert_eq!(s.balancer.interval_ms, 10);
        for mask in &s.affinity {
            assert!(mask.is_empty());
        }
    }

    #[test]
    fn test_sched_init_caps_excess_cores() {
        let _g = lock();
        let s = sched_init(99);
        assert_eq!(s.core_count, MAX_CORES as u32);
    }

    #[test]
    fn test_reserve_and_release_core() {
        let _g = lock();
        let mut s = sched_init(4);
        assert_eq!(reserve_core(&mut s, 0), Ok(()));
        assert!(s.reservation.is_reserved(0));
        release_core(&mut s, 0);
        assert!(!s.reservation.is_reserved(0));
    }

    #[test]
    fn test_reserve_core_out_of_range() {
        let _g = lock();
        let mut s = sched_init(4);
        assert_eq!(reserve_core(&mut s, 8), Err(SchedError::InvalidCore));
    }

    #[test]
    fn test_enqueue_pick_next_basic() {
        let _g = lock();
        let mut s = sched_init(4);
        enqueue(&mut s, Tid(1), 1);
        enqueue(&mut s, Tid(2), 1);
        assert_eq!(pick_next(&mut s, 1), Some(Tid(1)));
        assert_eq!(pick_next(&mut s, 1), Some(Tid(2)));
        assert_eq!(pick_next(&mut s, 1), None);
    }

    #[test]
    fn test_enqueue_rejected_on_reserved_core() {
        let _g = lock();
        let mut s = sched_init(4);
        assert_eq!(reserve_core(&mut s, 0), Ok(()));
        // Non-RTOS thread enqueue onto reserved core is dropped.
        enqueue(&mut s, Tid(1), 0);
        assert_eq!(s.rqs[0].load(), 0);
        assert_eq!(pick_next(&mut s, 0), None);
    }

    #[test]
    fn test_enqueue_accepted_on_non_reserved_core() {
        let _g = lock();
        let mut s = sched_init(4);
        enqueue(&mut s, Tid(1), 1);
        assert_eq!(s.rqs[1].load(), 1);
        assert_eq!(pick_next(&mut s, 1), Some(Tid(1)));
    }

    #[test]
    fn test_set_affinity_valid() {
        let _g = lock();
        let mut s = sched_init(4);
        let mask = CoreMask::all(4);
        assert_eq!(set_affinity(&mut s, Tid(5), mask), Ok(()));
        assert_eq!(s.affinity[5], mask);
        assert!(s.affinity[5].contains(2));
    }

    #[test]
    fn test_set_affinity_out_of_range() {
        let _g = lock();
        let mut s = sched_init(4);
        assert_eq!(
            set_affinity(&mut s, Tid(256), CoreMask::single(0)),
            Err(SchedError::NoRunnableTask)
        );
    }

    #[test]
    fn test_pin_to_core_valid() {
        let _g = lock();
        let mut s = sched_init(4);
        assert_eq!(pin_to_core(&mut s, Tid(7), 3), Ok(()));
        assert_eq!(s.affinity[7], CoreMask::single(3));
        assert!(s.affinity[7].contains(3));
        assert!(!s.affinity[7].contains(2));
    }

    #[test]
    fn test_pin_to_core_invalid_core() {
        let _g = lock();
        let mut s = sched_init(4);
        assert_eq!(pin_to_core(&mut s, Tid(7), 4), Err(SchedError::InvalidCore));
        assert_eq!(
            pin_to_core(&mut s, Tid(7), 99),
            Err(SchedError::InvalidCore)
        );
    }

    #[test]
    fn test_dequeue_removes_from_specific_core() {
        let _g = lock();
        let mut s = sched_init(4);
        enqueue(&mut s, Tid(10), 1);
        enqueue(&mut s, Tid(20), 2);
        dequeue(&mut s, Tid(10));
        assert_eq!(s.rqs[1].load(), 0);
        assert_eq!(s.rqs[2].load(), 1);
    }

    #[test]
    fn test_dequeue_absent_is_noop() {
        let _g = lock();
        let mut s = sched_init(4);
        enqueue(&mut s, Tid(10), 1);
        dequeue(&mut s, Tid(99));
        assert_eq!(s.rqs[1].load(), 1);
    }

    #[test]
    fn test_balance_load_migrates_thread() {
        let _g = lock();
        let mut s = sched_init(2);
        // Core 0: 4 threads, Core 1: 0. diff = 4 > threshold 2.
        enqueue(&mut s, Tid(1), 0);
        enqueue(&mut s, Tid(2), 0);
        enqueue(&mut s, Tid(3), 0);
        enqueue(&mut s, Tid(4), 0);
        assert_eq!(s.rqs[0].load(), 4);
        assert_eq!(s.rqs[1].load(), 0);

        balance_load(&mut s);

        assert_eq!(s.rqs[0].load(), 3);
        assert_eq!(s.rqs[1].load(), 1);
    }

    #[test]
    fn test_rtos_scenario_core0_reserved_agents_on_core1() {
        // End-to-end scenario: RTOS on Core 0, agents on Core 1+.
        let _g = lock();
        let mut s = sched_init(2);
        assert_eq!(reserve_core(&mut s, 0), Ok(()));
        // Agent threads rejected on reserved Core 0.
        enqueue(&mut s, Tid(100), 0);
        enqueue(&mut s, Tid(101), 0);
        assert_eq!(s.rqs[0].load(), 0);
        // Agent threads accepted on free Core 1.
        enqueue(&mut s, Tid(100), 1);
        enqueue(&mut s, Tid(101), 1);
        assert_eq!(s.rqs[1].load(), 2);
        // Core 0 remains idle for RTOS work.
        assert_eq!(pick_next(&mut s, 0), None);
        assert_eq!(pick_next(&mut s, 1), Some(Tid(100)));
    }
}
