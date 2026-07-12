//! Per-core run queue and lightweight spinlock.
//!
//! This module provides:
//! - **`Spinlock`** — a test-and-set (TAS) spinlock built on `AtomicBool`
//!   with `compare_exchange_weak` + double-layer `spin_loop` backoff.
//! - **`Tid`** — a lightweight thread identifier (newtype over `u32`).
//! - **`PerCoreRq`** — a fixed-capacity (64) per-core run queue.
//!
//! Per the D1 design decision, a custom `Spinlock` is used instead of
//! `spin::Mutex` so that `PerCoreRq::new` can be a `const fn`, enabling
//! const initialization of the `[PerCoreRq; 8]` array in `Scheduler`.
//!
//! Per the D2 design decision, this module depends only on `core::*`.

use core::hint::spin_loop;
use core::sync::atomic::AtomicBool;
use core::sync::atomic::Ordering;

/// Capacity of a per-core run queue.
pub const RQ_CAPACITY: usize = 64;

/// Lightweight test-and-set spinlock.
///
/// Uses `compare_exchange_weak` for acquisition and a nested load+spin
/// backoff loop to reduce bus contention under contention. `const fn`
/// constructible so it can live inside `const`-initialized arrays.
#[derive(Debug)]
pub struct Spinlock {
    locked: AtomicBool,
}

impl Spinlock {
    /// Create an unlocked spinlock. `const fn` for array initialization.
    pub const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
        }
    }

    /// Acquire the lock.
    ///
    /// Uses an outer `compare_exchange_weak(Acquire/Relaxed)` TAS loop and an
    /// inner `load(Relaxed)` + `spin_loop` backoff to avoid hammering the
    /// cache line while the lock is held by another CPU.
    pub fn lock(&self) {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) {
                spin_loop();
            }
        }
    }

    /// Release the lock.
    ///
    /// Uses `store(false, Release)` to publish any protected writes before
    /// the lock becomes observable as free.
    pub fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

impl Default for Spinlock {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread identifier (newtype over `u32`).
///
/// `Default` yields `Tid(0)`. `Copy` is required so that `Option<Tid>` is
/// `Copy`, which in turn allows `[None; RQ_CAPACITY]` const initialization.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Tid(pub u32);

/// Per-core run queue.
///
/// A fixed-capacity (64) array of runnable `Tid`s plus bookkeeping for the
/// currently-running thread, reservation flag, and a `Spinlock` for
/// concurrent access.
#[derive(Debug)]
pub struct PerCoreRq {
    /// Core id owning this run queue.
    pub core_id: u32,
    /// Fixed-capacity array of runnable threads (`None` = empty slot).
    pub runnable: [Option<Tid>; RQ_CAPACITY],
    /// Number of `Some` entries currently in `runnable`.
    pub count: usize,
    /// Thread currently dispatched on this core (`None` if idle).
    pub current: Option<Tid>,
    /// Whether this core is reserved (RTOS-exclusive).
    pub reserved: bool,
    /// Lock protecting `runnable`/`count` mutations.
    pub lock: Spinlock,
}

impl PerCoreRq {
    /// Construct an empty run queue for `core_id`. `const fn` for array init.
    pub const fn new(core_id: u32) -> Self {
        Self {
            core_id,
            runnable: [None; RQ_CAPACITY],
            count: 0,
            current: None,
            reserved: false,
            lock: Spinlock::new(),
        }
    }

    /// Enqueue `tid` into the first free slot.
    ///
    /// Silently drops the thread if the queue is full (D4: no panic in
    /// no_std scheduling paths; callers should check `load()` beforehand
    /// when full-capacity behavior matters).
    pub fn enqueue(&mut self, tid: Tid) {
        for slot in self.runnable.iter_mut() {
            if slot.is_none() {
                *slot = Some(tid);
                self.count += 1;
                return;
            }
        }
        // Queue full — drop the thread. Higher layers handle back-pressure.
    }

    /// Dequeue the first runnable thread (FIFO by slot order).
    ///
    /// Returns `None` if the queue is empty.
    pub fn dequeue(&mut self) -> Option<Tid> {
        for slot in self.runnable.iter_mut() {
            if let Some(tid) = slot.take() {
                self.count -= 1;
                return Some(tid);
            }
        }
        None
    }

    /// Current load (number of runnable threads).
    pub fn load(&self) -> usize {
        self.count
    }

    /// Remove `tid` from the queue if present.
    ///
    /// Returns `true` if the thread was found and removed, `false` otherwise.
    /// Compaction is implicit: removed slots become `None` and are reused by
    /// subsequent `enqueue` calls (no shifting).
    pub fn remove(&mut self, tid: Tid) -> bool {
        for slot in self.runnable.iter_mut() {
            if *slot == Some(tid) {
                *slot = None;
                self.count -= 1;
                return true;
            }
        }
        false
    }
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
    fn test_spinlock_new_is_unlocked() {
        let s = Spinlock::new();
        // Should be immediately acquirable.
        s.lock();
        s.unlock();
    }

    #[test]
    fn test_spinlock_lock_unlock_cycle() {
        let _g = lock();
        let s = Spinlock::new();
        s.lock();
        s.unlock();
        // Re-acquire after unlock.
        s.lock();
        s.unlock();
    }

    #[test]
    fn test_tid_default_and_equality() {
        assert_eq!(Tid::default(), Tid(0));
        assert_eq!(Tid(7), Tid(7));
        assert_ne!(Tid(7), Tid(8));
    }

    #[test]
    fn test_percore_rq_new_is_empty() {
        let mut rq = PerCoreRq::new(3);
        assert_eq!(rq.core_id, 3);
        assert_eq!(rq.count, 0);
        assert_eq!(rq.load(), 0);
        assert_eq!(rq.current, None);
        assert!(!rq.reserved);
        assert_eq!(rq.dequeue(), None);
    }

    #[test]
    fn test_percore_enqueue_dequeue_fifo() {
        let _g = lock();
        let mut rq = PerCoreRq::new(0);
        rq.enqueue(Tid(10));
        rq.enqueue(Tid(20));
        rq.enqueue(Tid(30));
        assert_eq!(rq.load(), 3);
        assert_eq!(rq.dequeue(), Some(Tid(10)));
        assert_eq!(rq.dequeue(), Some(Tid(20)));
        assert_eq!(rq.dequeue(), Some(Tid(30)));
        assert_eq!(rq.dequeue(), None);
        assert_eq!(rq.load(), 0);
    }

    #[test]
    fn test_percore_remove_present() {
        let _g = lock();
        let mut rq = PerCoreRq::new(1);
        rq.enqueue(Tid(1));
        rq.enqueue(Tid(2));
        rq.enqueue(Tid(3));
        assert!(rq.remove(Tid(2)));
        assert_eq!(rq.load(), 2);
        // Remaining threads still dequeuable in slot order.
        assert_eq!(rq.dequeue(), Some(Tid(1)));
        assert_eq!(rq.dequeue(), Some(Tid(3)));
        assert_eq!(rq.dequeue(), None);
    }

    #[test]
    fn test_percore_remove_absent() {
        let _g = lock();
        let mut rq = PerCoreRq::new(0);
        rq.enqueue(Tid(1));
        assert!(!rq.remove(Tid(99)));
        assert_eq!(rq.load(), 1);
    }

    #[test]
    fn test_percore_enqueue_after_remove_reuses_slot() {
        let _g = lock();
        let mut rq = PerCoreRq::new(0);
        rq.enqueue(Tid(1));
        rq.enqueue(Tid(2));
        rq.enqueue(Tid(3));
        assert!(rq.remove(Tid(2)));
        // Slot 1 is now free; enqueue fills it (slot-order reuse).
        rq.enqueue(Tid(9));
        assert_eq!(rq.load(), 3);
        // Dequeue order: slot 0 (Tid 1), slot 1 (Tid 9), slot 2 (Tid 3).
        assert_eq!(rq.dequeue(), Some(Tid(1)));
        assert_eq!(rq.dequeue(), Some(Tid(9)));
        assert_eq!(rq.dequeue(), Some(Tid(3)));
    }

    #[test]
    fn test_percore_enqueue_full_silently_drops() {
        let _g = lock();
        let mut rq = PerCoreRq::new(0);
        for i in 0..RQ_CAPACITY as u32 {
            rq.enqueue(Tid(i));
        }
        assert_eq!(rq.load(), RQ_CAPACITY);
        // Overflow enqueue is silently dropped.
        rq.enqueue(Tid(99));
        assert_eq!(rq.load(), RQ_CAPACITY);
    }
}
