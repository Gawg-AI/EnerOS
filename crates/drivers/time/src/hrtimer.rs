//! High-resolution timer wheel with 64 slots.
//!
//! Array-backed timer wheel for EnerOS v0.12.0. Each slot holds at most one
//! timer. The wheel supports one-shot and periodic timers identified by a
//! globally unique `TimerId` (monotonically increasing from 1).

/// Timer identifier (globally unique, starts at 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimerId(pub u64);

/// A single timer entry.
///
/// All fields are `Copy`, so `HrTimer` derives `Clone + Copy`. This allows
/// `[None; 64]` initialization of the wheel without requiring the
/// `const { None }` block expression (Rust 1.79+).
#[derive(Clone, Copy)]
pub struct HrTimer {
    pub id: TimerId,
    pub deadline_ns: u64,
    pub callback: fn(),
    pub periodic: bool,
    pub period_ns: u64,
}

/// Timer wheel backed by a fixed-size array of 64 slots.
///
/// The wheel itself is not synchronized; callers (e.g. `api.rs`) are
/// responsible for locking when sharing across contexts.
pub struct TimerWheel {
    pub timers: [Option<HrTimer>; 64],
    pub count: usize,
    pub next_id: u64,
    pub expired_count: u64,
}

impl Default for TimerWheel {
    fn default() -> Self {
        Self::new()
    }
}

impl TimerWheel {
    /// Create an empty timer wheel. `const fn` so it can be placed in a
    /// `static` initializer.
    pub const fn new() -> Self {
        Self {
            timers: [None; 64],
            count: 0,
            next_id: 1,
            expired_count: 0,
        }
    }

    /// Add a timer. Returns the assigned `TimerId`, or `None` if the wheel
    /// is full (64 slots occupied).
    pub fn add(
        &mut self,
        deadline_ns: u64,
        cb: fn(),
        periodic: bool,
        period_ns: u64,
    ) -> Option<TimerId> {
        if self.count >= 64 {
            return None;
        }
        for slot in self.timers.iter_mut() {
            if slot.is_none() {
                let id = TimerId(self.next_id);
                self.next_id += 1;
                *slot = Some(HrTimer {
                    id,
                    deadline_ns,
                    callback: cb,
                    periodic,
                    period_ns,
                });
                self.count += 1;
                return Some(id);
            }
        }
        // Unreachable when count < 64: a free slot always exists.
        None
    }

    /// Cancel the timer with the given id. No-op if the id is not present.
    pub fn cancel(&mut self, id: TimerId) {
        for slot in self.timers.iter_mut() {
            if let Some(t) = slot {
                if t.id == id {
                    *slot = None;
                    self.count -= 1;
                    return;
                }
            }
        }
    }

    /// Process all expired timers at `now_ns`, invoking their callbacks.
    /// Returns the nearest future deadline, or `u64::MAX` when no timers
    /// remain. Intended to be called from the timer interrupt context.
    pub fn tick(&mut self, now_ns: u64) -> u64 {
        let mut next_deadline = u64::MAX;
        for slot in self.timers.iter_mut() {
            if let Some(timer) = slot {
                if now_ns >= timer.deadline_ns {
                    // Expired: invoke callback.
                    (timer.callback)();
                    self.expired_count += 1;
                    if timer.periodic {
                        // Periodic timer: re-arm relative to now.
                        timer.deadline_ns = now_ns.saturating_add(timer.period_ns);
                    } else {
                        // One-shot timer: remove.
                        *slot = None;
                        self.count -= 1;
                        continue;
                    }
                }
                next_deadline = next_deadline.min(timer.deadline_ns);
            }
        }
        next_deadline
    }

    /// Return the nearest deadline without firing callbacks.
    /// `None` when the wheel is empty.
    pub fn next_deadline(&self) -> Option<u64> {
        self.timers.iter().flatten().map(|t| t.deadline_ns).min()
    }

    /// Number of active timers.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether the wheel holds no timers.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    static CALL_COUNT: AtomicU32 = AtomicU32::new(0);

    fn test_callback() {
        CALL_COUNT.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn test_new_wheel_is_empty() {
        let w = TimerWheel::new();
        assert_eq!(w.len(), 0);
        assert!(w.is_empty());
        assert_eq!(w.next_deadline(), None);
    }

    #[test]
    fn test_add_timer() {
        let mut w = TimerWheel::new();
        let id = w.add(1000, test_callback, false, 0);
        assert!(id.is_some());
        assert_eq!(w.len(), 1);
        assert_eq!(w.next_deadline(), Some(1000));
    }

    #[test]
    fn test_tick_fires_expired() {
        CALL_COUNT.store(0, Ordering::SeqCst);
        let mut w = TimerWheel::new();
        let _id = w.add(1000, test_callback, false, 0);
        // now < deadline -> not fired
        let next = w.tick(500);
        assert_eq!(next, 1000);
        assert_eq!(CALL_COUNT.load(Ordering::SeqCst), 0);
        assert_eq!(w.len(), 1);
        // now >= deadline -> fired
        let next = w.tick(1000);
        assert_eq!(CALL_COUNT.load(Ordering::SeqCst), 1);
        assert_eq!(w.len(), 0); // one-shot removed
        assert_eq!(next, u64::MAX); // no timers left
    }

    #[test]
    fn test_cancel_timer() {
        let mut w = TimerWheel::new();
        let id = w.add(1000, test_callback, false, 0).unwrap();
        assert_eq!(w.len(), 1);
        w.cancel(id);
        assert_eq!(w.len(), 0);
        assert_eq!(w.next_deadline(), None);
    }

    #[test]
    fn test_cancel_nonexistent() {
        let mut w = TimerWheel::new();
        let _id = w.add(1000, test_callback, false, 0);
        w.cancel(TimerId(999)); // unknown id
        assert_eq!(w.len(), 1); // existing timer untouched
    }

    #[test]
    fn test_periodic_timer() {
        CALL_COUNT.store(0, Ordering::SeqCst);
        let mut w = TimerWheel::new();
        let _id = w.add(1000, test_callback, true, 500);
        // First fire
        w.tick(1000);
        assert_eq!(CALL_COUNT.load(Ordering::SeqCst), 1);
        assert_eq!(w.len(), 1); // periodic stays
        assert_eq!(w.next_deadline(), Some(1500));
        // Second fire
        w.tick(1500);
        assert_eq!(CALL_COUNT.load(Ordering::SeqCst), 2);
        assert_eq!(w.next_deadline(), Some(2000));
    }

    #[test]
    fn test_wheel_full() {
        let mut w = TimerWheel::new();
        for i in 0..64 {
            assert!(w.add(i * 100, test_callback, false, 0).is_some());
        }
        assert_eq!(w.len(), 64);
        // 65th must fail
        let result = w.add(99999, test_callback, false, 0);
        assert!(result.is_none());
    }

    #[test]
    fn test_multiple_timers_same_tick() {
        CALL_COUNT.store(0, Ordering::SeqCst);
        let mut w = TimerWheel::new();
        let _id1 = w.add(1000, test_callback, false, 0);
        let _id2 = w.add(1000, test_callback, false, 0);
        let _id3 = w.add(2000, test_callback, false, 0);
        // tick at 1000 -> two timers fire
        let next = w.tick(1000);
        assert_eq!(CALL_COUNT.load(Ordering::SeqCst), 2);
        assert_eq!(w.len(), 1); // only deadline=2000 remains
        assert_eq!(next, 2000);
    }

    #[test]
    fn test_empty_tick() {
        let mut w = TimerWheel::new();
        let next = w.tick(99999);
        assert_eq!(next, u64::MAX);
    }

    #[test]
    fn test_expired_count() {
        let mut w = TimerWheel::new();
        let _id1 = w.add(1000, test_callback, false, 0);
        let _id2 = w.add(2000, test_callback, false, 0);
        w.tick(1500); // fire 1
        assert_eq!(w.expired_count, 1);
        w.tick(2500); // fire 1
        assert_eq!(w.expired_count, 2);
    }
}
