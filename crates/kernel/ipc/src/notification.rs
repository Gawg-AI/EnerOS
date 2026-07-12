//! Bit-mask notifications with thread wake-up (v0.20.0).
//!
//! Provides:
//! - [`Notification`] — per-thread notification slot with an atomic
//!   64-bit bit-mask and a waiting thread Tid
//! - [`notify`] — signal a bit on a target thread's notification slot
//! - [`wait_notification`] — block until a notification arrives, returning
//!   the accumulated bit-mask
//!
//! Per D2, global state uses `Spinlock + UnsafeCell<T>` (NOT `static mut`).

use core::cell::UnsafeCell;
use core::sync::atomic::AtomicU64;
use core::sync::atomic::Ordering;

use eneros_sched::{current_tid, thread_block, thread_resume, Spinlock, Tid};

/// Maximum number of notification slots (indexed by `Tid.0`, clamped).
pub const MAX_NOTIFY_SLOTS: usize = 256;

/// Per-thread notification slot.
///
/// `bits` is an atomic 64-bit mask where each bit represents a distinct
/// notification source. `waiter` records the thread that is currently
/// blocked in [`wait_notification`] (if any).
pub struct Notification {
    pub bits: AtomicU64,
    pub waiter: Option<Tid>,
}

/// Wrapper around the global notification table.
///
/// Combines a [`Spinlock`] with an `UnsafeCell`-protected array of
/// [`Notification`] slots. The `UnsafeCell` provides interior mutability;
/// the spinlock serializes access to the `waiter` field (the `bits`
/// field uses atomics for lock-free read/write).
///
/// # Safety
///
/// `Sync` is sound because all access to `waiter` is gated by `lock`.
/// The `bits` field is accessed via atomics and is safe to share across
/// threads.
struct NotificationTable {
    lock: Spinlock,
    entries: UnsafeCell<[Notification; MAX_NOTIFY_SLOTS]>,
}

// SAFETY: Access to `waiter` is serialized by `lock`. The `bits` field
// uses atomic operations which are inherently thread-safe.
unsafe impl Sync for NotificationTable {}

static NOTIFICATIONS: NotificationTable = NotificationTable {
    lock: Spinlock::new(),
    entries: UnsafeCell::new(
        [const {
            Notification {
                bits: AtomicU64::new(0),
                waiter: None,
            }
        }; MAX_NOTIFY_SLOTS],
    ),
};

/// Signal a notification bit on `target`'s notification slot.
///
/// Sets bit `bit` (0..63) in the target thread's notification mask using
/// `fetch_or` with `Release` ordering, then resumes the target thread if
/// it is blocked (no-op if the thread is not blocked).
///
/// If `target.0` exceeds `MAX_NOTIFY_SLOTS`, the index is clamped to the
/// last slot.
pub fn notify(target: Tid, bit: u32) {
    let idx = if target.0 as usize >= MAX_NOTIFY_SLOTS {
        MAX_NOTIFY_SLOTS - 1
    } else {
        target.0 as usize
    };

    NOTIFICATIONS.lock.lock();
    // SAFETY: We hold the lock.
    let entries = unsafe { &*NOTIFICATIONS.entries.get() };
    entries[idx].bits.fetch_or(1u64 << bit, Ordering::Release);
    NOTIFICATIONS.lock.unlock();

    // Resume the target thread (no-op if not blocked).
    let _ = thread_resume(target);
}

/// Wait for a notification.
///
/// Atomically swaps the current thread's notification mask to 0 and
/// returns the previous value. If no bits were set (mask was 0), the
/// current thread is blocked (on host, blocking is a no-op — the
/// function returns 0 immediately).
///
/// Returns the accumulated bit-mask that was cleared.
pub fn wait_notification() -> u64 {
    let tid = current_tid();
    let idx = if tid.0 as usize >= MAX_NOTIFY_SLOTS {
        MAX_NOTIFY_SLOTS - 1
    } else {
        tid.0 as usize
    };

    NOTIFICATIONS.lock.lock();
    // SAFETY: We hold the lock.
    let entries = unsafe { &mut *NOTIFICATIONS.entries.get() };
    let bits = entries[idx].bits.swap(0, Ordering::Acquire);
    if bits == 0 {
        entries[idx].waiter = Some(tid);
    } else {
        entries[idx].waiter = None;
    }
    NOTIFICATIONS.lock.unlock();

    if bits == 0 {
        let _ = thread_block(tid);
        0
    } else {
        bits
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

    /// Clear all notification slots.
    fn reset_notifications() {
        NOTIFICATIONS.lock.lock();
        // SAFETY: We hold the lock.
        let entries = unsafe { &mut *NOTIFICATIONS.entries.get() };
        for n in entries.iter_mut() {
            n.bits.store(0, Ordering::Relaxed);
            n.waiter = None;
        }
        NOTIFICATIONS.lock.unlock();
    }

    /// Read the raw bits of a thread's notification slot (test helper).
    fn read_bits(tid: Tid) -> u64 {
        let idx = if tid.0 as usize >= MAX_NOTIFY_SLOTS {
            MAX_NOTIFY_SLOTS - 1
        } else {
            tid.0 as usize
        };
        NOTIFICATIONS.lock.lock();
        // SAFETY: We hold the lock.
        let entries = unsafe { &*NOTIFICATIONS.entries.get() };
        let bits = entries[idx].bits.load(Ordering::Relaxed);
        NOTIFICATIONS.lock.unlock();
        bits
    }

    #[test]
    fn test_notify_sets_bit() {
        let _g = lock();
        reset_notifications();

        let target = Tid(3);
        notify(target, 3);

        let bits = read_bits(target);
        assert_ne!(bits & (1u64 << 3), 0, "bit 3 should be set");
        assert_eq!(bits, 1u64 << 3, "only bit 3 should be set");

        reset_notifications();
    }

    #[test]
    fn test_wait_notification_reads_and_clears() {
        let _g = lock();
        reset_notifications();

        let tid = Tid(10);
        // Set some bits via notify.
        notify(tid, 2);
        notify(tid, 5);

        // Set current_tid so wait_notification looks up the right slot.
        eneros_sched::set_current_tid(tid);

        let result = wait_notification();
        assert_ne!(result & (1u64 << 2), 0, "bit 2 should be in result");
        assert_ne!(result & (1u64 << 5), 0, "bit 5 should be in result");
        assert_eq!(result, (1u64 << 2) | (1u64 << 5));

        // Bits should be cleared after wait.
        let bits = read_bits(tid);
        assert_eq!(bits, 0, "bits should be cleared after wait_notification");

        reset_notifications();
        eneros_sched::set_current_tid(Tid(0));
    }

    #[test]
    fn test_multiple_bits() {
        let _g = lock();
        reset_notifications();

        let tid = Tid(20);
        notify(tid, 1);
        notify(tid, 3);
        notify(tid, 5);

        // Verify all bits are set.
        let bits = read_bits(tid);
        assert_eq!(
            bits,
            (1u64 << 1) | (1u64 << 3) | (1u64 << 5),
            "bits 1, 3, 5 should all be set"
        );

        eneros_sched::set_current_tid(tid);

        // First wait returns all bits.
        let result = wait_notification();
        assert_eq!(
            result,
            (1u64 << 1) | (1u64 << 3) | (1u64 << 5),
            "wait should return all set bits"
        );

        // Second wait returns 0 (all bits were cleared).
        let result2 = wait_notification();
        assert_eq!(result2, 0, "second wait should return 0 after clear");

        reset_notifications();
        eneros_sched::set_current_tid(Tid(0));
    }
}
