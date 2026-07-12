//! Core reservation for RTOS pinning.
//!
//! This module provides `CoreReservation`, which marks cores as exclusive to
//! RTOS workloads. A reserved core rejects non-RTOS threads via
//! [`CoreReservation::can_enqueue`] returning `false`, enabling the mixed
//! criticality architecture where Core 0 runs an RTOS and Cores 1+ run Agent
//! workloads.
//!
//! It also defines the shared [`SchedError`] enum used across the scheduler.
//!
//! Per the D2 design decision, this module depends only on `core::*`.

/// Maximum number of cores whose reservation state can be tracked.
pub const MAX_CORES: usize = 8;

/// Errors returned by scheduler operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedError {
    /// The core index is out of range (>= configured `core_count` or `MAX_CORES`).
    InvalidCore,
    /// The core has been reserved (RTOS-exclusive) and rejects the request.
    CoreReserved,
    /// No runnable task was found, or the thread id is out of range.
    NoRunnableTask,
    /// No timer registrar has been injected; cannot start partition scheduling.
    NoTimerRegistrar,
    /// The major frame's slot table is full (MAX_SLOTS reached).
    SlotFull,
}

/// Per-core reservation table.
///
/// A reserved core may only host RTOS threads (see [`can_enqueue`]). Used to
/// pin an RTOS onto a specific core (e.g. Core 0) so that Agent workloads on
/// Cores 1+ cannot preempt it.
///
/// [`can_enqueue`]: CoreReservation::can_enqueue
#[derive(Debug)]
pub struct CoreReservation {
    /// `reserved[i] == true` means core `i` is RTOS-exclusive.
    pub reserved: [bool; MAX_CORES],
}

impl CoreReservation {
    /// Construct a reservation table with all cores free.
    pub const fn new() -> Self {
        Self {
            reserved: [false; MAX_CORES],
        }
    }

    /// Reserve `core` for RTOS use.
    ///
    /// Returns [`Err(SchedError::InvalidCore)`](SchedError::InvalidCore) if
    /// `core >= MAX_CORES`, or [`Err(SchedError::CoreReserved)`](SchedError::CoreReserved)
    /// if the core is already reserved.
    pub fn reserve(&mut self, core: u32) -> Result<(), SchedError> {
        if core as usize >= MAX_CORES {
            return Err(SchedError::InvalidCore);
        }
        if self.reserved[core as usize] {
            return Err(SchedError::CoreReserved);
        }
        self.reserved[core as usize] = true;
        Ok(())
    }

    /// Release a previously reserved core. No-op if `core` is out of range
    /// or already free.
    pub fn release(&mut self, core: u32) {
        if (core as usize) < MAX_CORES {
            self.reserved[core as usize] = false;
        }
    }

    /// Whether `core` is reserved. Returns `false` for out-of-range cores.
    pub fn is_reserved(&self, core: u32) -> bool {
        if (core as usize) < MAX_CORES {
            self.reserved[core as usize]
        } else {
            false
        }
    }

    /// Whether a thread may be enqueued onto `core`.
    ///
    /// Reserved cores only accept RTOS threads (`is_rtos == true`). Free
    /// cores accept any thread. Returns `false` for out-of-range cores
    /// (defensive: callers should validate `core` separately).
    pub fn can_enqueue(&self, core: u32, is_rtos: bool) -> bool {
        if (core as usize) >= MAX_CORES {
            return false;
        }
        if self.is_reserved(core) {
            is_rtos
        } else {
            true
        }
    }
}

impl Default for CoreReservation {
    fn default() -> Self {
        Self::new()
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
    fn test_new_has_no_reservations() {
        let r = CoreReservation::new();
        for i in 0..MAX_CORES {
            assert!(!r.is_reserved(i as u32));
        }
    }

    #[test]
    fn test_reserve_success() {
        let _g = lock();
        let mut r = CoreReservation::new();
        assert_eq!(r.reserve(0), Ok(()));
        assert!(r.is_reserved(0));
        assert!(!r.is_reserved(1));
    }

    #[test]
    fn test_reserve_already_reserved() {
        let _g = lock();
        let mut r = CoreReservation::new();
        assert_eq!(r.reserve(2), Ok(()));
        assert_eq!(r.reserve(2), Err(SchedError::CoreReserved));
    }

    #[test]
    fn test_reserve_invalid_core() {
        let _g = lock();
        let mut r = CoreReservation::new();
        assert_eq!(r.reserve(8), Err(SchedError::InvalidCore));
        assert_eq!(r.reserve(MAX_CORES as u32), Err(SchedError::InvalidCore));
        assert_eq!(r.reserve(u32::MAX), Err(SchedError::InvalidCore));
    }

    #[test]
    fn test_release_clears_reservation() {
        let _g = lock();
        let mut r = CoreReservation::new();
        assert_eq!(r.reserve(1), Ok(()));
        assert!(r.is_reserved(1));
        r.release(1);
        assert!(!r.is_reserved(1));
        // Releasing a free core is a no-op.
        r.release(1);
        assert!(!r.is_reserved(1));
    }

    #[test]
    fn test_release_out_of_range_is_noop() {
        let _g = lock();
        let mut r = CoreReservation::new();
        r.release(8);
        r.release(u32::MAX);
        for i in 0..MAX_CORES {
            assert!(!r.is_reserved(i as u32));
        }
    }

    #[test]
    fn test_can_enqueue_free_core_accepts_all() {
        let _g = lock();
        let r = CoreReservation::new();
        assert!(r.can_enqueue(0, false));
        assert!(r.can_enqueue(0, true));
    }

    #[test]
    fn test_can_enqueue_reserved_rejects_non_rtos() {
        let _g = lock();
        let mut r = CoreReservation::new();
        assert_eq!(r.reserve(0), Ok(()));
        // Reserved core rejects non-RTOS threads.
        assert!(!r.can_enqueue(0, false));
        // Reserved core still accepts RTOS threads.
        assert!(r.can_enqueue(0, true));
        // Other cores unaffected.
        assert!(r.can_enqueue(1, false));
    }

    #[test]
    fn test_can_enqueue_out_of_range_false() {
        let _g = lock();
        let r = CoreReservation::new();
        assert!(!r.can_enqueue(8, false));
        assert!(!r.can_enqueue(8, true));
        assert!(!r.can_enqueue(u32::MAX, true));
    }

    #[test]
    fn test_is_reserved_out_of_range_false() {
        let _g = lock();
        let r = CoreReservation::new();
        assert!(!r.is_reserved(8));
        assert!(!r.is_reserved(u32::MAX));
    }
}
