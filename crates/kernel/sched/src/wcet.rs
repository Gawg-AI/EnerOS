//! WCET (Worst-Case Execution Time) estimation — Phase 0 P0-F (v0.19.0).
//!
//! Provides a static-table WCET estimation for threads, plus partition overrun
//! detection. WCET values are configured via [`wcet_set`] and queried via
//! [`wcet_estimate`]. [`check_partition_overrun`] scans threads in a partition
//! and returns the first whose WCET exceeds the slot duration.
//!
//! Per D6, this is a simple static table (not formal analysis). Per D2, uses
//! `Spinlock` + `UnsafeCell` (not `static mut`).

use core::cell::UnsafeCell;

use crate::percore::Spinlock;
use crate::Tid;
use crate::MAX_THREADS;

struct WcetTable {
    lock: Spinlock,
    entries: UnsafeCell<[u64; MAX_THREADS]>,
}

unsafe impl Sync for WcetTable {}

static WCET_TABLE: WcetTable = WcetTable {
    lock: Spinlock::new(),
    entries: UnsafeCell::new([0; MAX_THREADS]),
};

/// Set the WCET estimate (in nanoseconds) for `tid`.
///
/// `Tid(0)` and `tid.0 > MAX_THREADS` are silently ignored (invalid tid).
/// Tid indices are 1-based: `Tid(1)` maps to `entries[0]`.
pub fn wcet_set(tid: Tid, ns: u64) {
    if tid.0 == 0 || tid.0 as usize > MAX_THREADS {
        return;
    }
    let idx = (tid.0 - 1) as usize;
    WCET_TABLE.lock.lock();
    // SAFETY: We hold the lock.
    let entries = unsafe { &mut *WCET_TABLE.entries.get() };
    entries[idx] = ns;
    WCET_TABLE.lock.unlock();
}

/// Query the WCET estimate (in nanoseconds) for `tid`.
///
/// Returns `0` for invalid tids (`Tid(0)` or `tid.0 > MAX_THREADS`) and for
/// threads whose WCET has not been configured.
pub fn wcet_estimate(tid: Tid) -> u64 {
    if tid.0 == 0 || tid.0 as usize > MAX_THREADS {
        return 0;
    }
    let idx = (tid.0 - 1) as usize;
    WCET_TABLE.lock.lock();
    // SAFETY: We hold the lock.
    let entries = unsafe { &*WCET_TABLE.entries.get() };
    let val = entries[idx];
    WCET_TABLE.lock.unlock();
    val
}

/// Scan a partition for WCET overrun.
///
/// Iterates over all possible thread ids (`1..=MAX_THREADS`), and for each
/// thread whose partition equals `partition`, compares its WCET estimate
/// against `slot_duration_ns`. Returns the first `Tid` whose WCET exceeds the
/// slot duration, or `None` if no overrun is found.
///
/// # Lock ordering
///
/// This function does NOT hold the `WCET_TABLE` lock while calling
/// [`crate::tcb::thread_partition`] (which locks `THREAD_TABLE`). WCET is read
/// first (lock acquired/released), then the partition lookup is performed.
/// This avoids an AB-BA deadlock with any path that locks the tables in the
/// reverse order.
pub fn check_partition_overrun(partition: u32, slot_duration_ns: u64) -> Option<Tid> {
    for i in 1..=MAX_THREADS as u32 {
        let tid = Tid(i);
        let part = match crate::tcb::thread_partition(tid) {
            Some(p) => p,
            None => continue,
        };
        if part == partition {
            let wcet = wcet_estimate(tid);
            if wcet > slot_duration_ns {
                return Some(tid);
            }
        }
    }
    None
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

    fn reset_wcet_table() {
        WCET_TABLE.lock.lock();
        unsafe { &mut *WCET_TABLE.entries.get() }.fill(0);
        WCET_TABLE.lock.unlock();
    }

    #[test]
    fn test_default_wcet_is_zero() {
        let _g = lock();
        reset_wcet_table();
        assert_eq!(wcet_estimate(Tid(5)), 0);
    }

    #[test]
    fn test_wcet_set_and_estimate() {
        let _g = lock();
        reset_wcet_table();
        wcet_set(Tid(5), 500_000);
        assert_eq!(wcet_estimate(Tid(5)), 500_000);
    }

    #[test]
    fn test_wcet_set_invalid_tid() {
        let _g = lock();
        reset_wcet_table();
        // Tid(0) is invalid — should be a no-op (no panic).
        wcet_set(Tid(0), 100);
        // Tid(999) exceeds MAX_THREADS (256) — should be a no-op.
        wcet_set(Tid(999), 100);
        // Ensure nothing was written to a valid slot.
        assert_eq!(wcet_estimate(Tid(1)), 0);
        assert_eq!(wcet_estimate(Tid(256)), 0);
    }

    #[test]
    fn test_wcet_estimate_invalid_tid() {
        let _g = lock();
        reset_wcet_table();
        assert_eq!(wcet_estimate(Tid(0)), 0);
        assert_eq!(wcet_estimate(Tid(999)), 0);
    }

    #[test]
    fn test_check_partition_overrun_no_threads() {
        let _g = lock();
        reset_wcet_table();
        // No threads have been created, so thread_partition returns None for
        // every tid — no overrun should be reported.
        assert_eq!(check_partition_overrun(0, 1_000_000), None);
    }
}
