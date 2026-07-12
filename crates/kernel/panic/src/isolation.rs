//! Partition isolation — tracks per-partition liveness and panic handlers.
//!
//! When a partition panics, `PartitionIsolateStrategy` marks it `Dead` and
//! halts its core. If marking fails (invalid id or already dead), the strategy
//! escalates to a full kernel reset (handled in `lib.rs`).

use spin::Mutex;

use crate::PanicContext;

/// Liveness state of a partition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionState {
    /// Partition is running normally.
    Alive,
    /// Partition has panicked and been fenced off.
    Dead,
}

/// Errors returned by isolation operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationError {
    /// Partition id is out of range (>= `MAX_PARTITIONS`).
    InvalidId,
    /// Partition is already `Dead`.
    AlreadyDead,
}

/// Maximum number of tracked partitions.
pub const MAX_PARTITIONS: usize = 8;

/// Per-partition liveness table. All partitions start `Alive`.
static PARTITION_TABLE: [Mutex<PartitionState>; MAX_PARTITIONS] = [
    Mutex::new(PartitionState::Alive),
    Mutex::new(PartitionState::Alive),
    Mutex::new(PartitionState::Alive),
    Mutex::new(PartitionState::Alive),
    Mutex::new(PartitionState::Alive),
    Mutex::new(PartitionState::Alive),
    Mutex::new(PartitionState::Alive),
    Mutex::new(PartitionState::Alive),
];

/// Per-partition panic handlers (`None` until registered).
#[allow(clippy::type_complexity)]
static HANDLERS: [Mutex<Option<fn(&PanicContext) -> !>>; MAX_PARTITIONS] = [
    Mutex::new(None),
    Mutex::new(None),
    Mutex::new(None),
    Mutex::new(None),
    Mutex::new(None),
    Mutex::new(None),
    Mutex::new(None),
    Mutex::new(None),
];

/// Mark `id` as `Dead`. Returns `Err` if `id` is invalid or already dead.
pub fn mark_partition_dead(id: u32) -> Result<(), IsolationError> {
    let i = match usize::try_from(id) {
        Ok(v) if v < MAX_PARTITIONS => v,
        _ => return Err(IsolationError::InvalidId),
    };
    let mut slot = PARTITION_TABLE[i].lock();
    if *slot == PartitionState::Dead {
        return Err(IsolationError::AlreadyDead);
    }
    *slot = PartitionState::Dead;
    Ok(())
}

/// Query the liveness state of `id`. Returns `None` if out of range.
pub fn partition_state(id: u32) -> Option<PartitionState> {
    let i = usize::try_from(id).ok()?;
    if i >= MAX_PARTITIONS {
        return None;
    }
    Some(*PARTITION_TABLE[i].lock())
}

/// Reset `id` back to `Alive` (test/debug utility).
pub fn reset_partition(id: u32) -> Result<(), IsolationError> {
    let i = match usize::try_from(id) {
        Ok(v) if v < MAX_PARTITIONS => v,
        _ => return Err(IsolationError::InvalidId),
    };
    *PARTITION_TABLE[i].lock() = PartitionState::Alive;
    Ok(())
}

/// Register a partition-specific panic handler. Slots are 0..8; out-of-range
/// registrations are silently ignored (per blueprint).
pub fn register_partition_panic_handler(partition: u32, handler: fn(&PanicContext) -> !) {
    if let Ok(i) = usize::try_from(partition) {
        if i < MAX_PARTITIONS {
            *HANDLERS[i].lock() = Some(handler);
        }
    }
}

/// Look up the handler registered for `id`. Returns `None` if unset or invalid.
pub fn get_partition_handler(id: u32) -> Option<fn(&PanicContext) -> !> {
    let i = usize::try_from(id).ok()?;
    if i >= MAX_PARTITIONS {
        return None;
    }
    *HANDLERS[i].lock()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn reset_all() {
        for i in 0..MAX_PARTITIONS {
            *PARTITION_TABLE[i].lock() = PartitionState::Alive;
            *HANDLERS[i].lock() = None;
        }
    }

    #[test]
    fn test_partition_state_initial_alive() {
        let _g = lock();
        reset_all();
        assert_eq!(partition_state(0), Some(PartitionState::Alive));
        assert_eq!(partition_state(7), Some(PartitionState::Alive));
    }

    #[test]
    fn test_mark_partition_dead_success() {
        let _g = lock();
        reset_all();
        assert_eq!(mark_partition_dead(0), Ok(()));
        assert_eq!(partition_state(0), Some(PartitionState::Dead));
        assert_eq!(partition_state(1), Some(PartitionState::Alive));
    }

    #[test]
    fn test_mark_partition_dead_invalid_id() {
        let _g = lock();
        reset_all();
        assert_eq!(mark_partition_dead(8), Err(IsolationError::InvalidId));
        assert_eq!(
            mark_partition_dead(u32::MAX),
            Err(IsolationError::InvalidId)
        );
    }

    #[test]
    fn test_mark_partition_dead_already_dead() {
        let _g = lock();
        reset_all();
        assert_eq!(mark_partition_dead(2), Ok(()));
        assert_eq!(mark_partition_dead(2), Err(IsolationError::AlreadyDead));
    }

    #[test]
    fn test_partition_state_query() {
        let _g = lock();
        reset_all();
        assert_eq!(partition_state(0), Some(PartitionState::Alive));
        assert_eq!(partition_state(8), None);
        assert_eq!(partition_state(99), None);
        assert_eq!(mark_partition_dead(5), Ok(()));
        assert_eq!(partition_state(5), Some(PartitionState::Dead));
    }

    #[test]
    fn test_reset_partition() {
        let _g = lock();
        reset_all();
        assert_eq!(mark_partition_dead(1), Ok(()));
        assert_eq!(partition_state(1), Some(PartitionState::Dead));
        assert_eq!(reset_partition(1), Ok(()));
        assert_eq!(partition_state(1), Some(PartitionState::Alive));
        assert_eq!(reset_partition(8), Err(IsolationError::InvalidId));
    }

    fn dummy_handler(_ctx: &PanicContext) -> ! {
        loop {
            core::hint::spin_loop();
        }
    }

    #[test]
    fn test_register_partition_panic_handler() {
        let _g = lock();
        reset_all();
        register_partition_panic_handler(0, dummy_handler);
        assert!(get_partition_handler(0).is_some());
        // Out-of-range registration is ignored (no panic).
        register_partition_panic_handler(8, dummy_handler);
    }

    #[test]
    fn test_get_partition_handler() {
        let _g = lock();
        reset_all();
        // Unset handler.
        assert!(get_partition_handler(3).is_none());
        register_partition_panic_handler(3, dummy_handler);
        assert!(get_partition_handler(3).is_some());
        // Out-of-range query.
        assert!(get_partition_handler(8).is_none());
    }
}
