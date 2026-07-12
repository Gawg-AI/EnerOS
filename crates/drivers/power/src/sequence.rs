//! Shutdown sequence state machine and emergency checkpoint.
//!
//! The sequence progresses through five stages:
//!
//! ```text
//! Detect → RideThrough → Checkpoint → GracefulShutdown → HardOff
//! ```
//!
//! [`advance_sequence`] drives transitions based on [`PowerEvent`]s.
//! [`emergency_checkpoint`] flushes persistent state via an injected callback
//! (the power crate does not depend on a file system).

use core::time::Duration;

use spin::Mutex;

use crate::{CheckpointError, PowerDownSequence, PowerEvent, SequenceError, ShutdownStage};

// ============================================================================
// Constants (mirrors configs/power/sequence.toml)
// ============================================================================

/// Ride-through budget from UPS/supercapacitor (500 ms).
const RIDE_THROUGH_BUDGET_MS: u64 = 500;

// ============================================================================
// Checkpoint callback injection
// ============================================================================

/// Type of the injected checkpoint flush callback.
pub(crate) type CheckpointFn = fn() -> Result<(), CheckpointError>;

/// Injected callback for flushing checkpoint data to persistent storage.
/// The power crate does not depend on FS — the caller registers this at init.
pub(crate) static CHECKPOINT_CALLBACK: Mutex<Option<CheckpointFn>> = Mutex::new(None);

/// Re-entrancy guard for [`emergency_checkpoint`].
pub(crate) static CHECKPOINT_IN_PROGRESS: Mutex<bool> = Mutex::new(false);

/// Register the checkpoint flush callback.
///
/// The callback is invoked by [`emergency_checkpoint`] to flush persistent
/// state. It should return `Ok(())` on success, or an error on failure.
pub fn register_checkpoint_callback(cb: fn() -> Result<(), CheckpointError>) {
    *CHECKPOINT_CALLBACK.lock() = Some(cb);
}

// ============================================================================
// Public API
// ============================================================================

/// Create a new power-down sequence at the `Detect` stage.
///
/// Also updates the global power state: `main_power_ok = false`,
/// `in_shutdown = true`.
pub fn on_power_loss() -> PowerDownSequence {
    {
        let mut state = crate::POWER_STATE.lock();
        state.main_power_ok = false;
        state.in_shutdown = true;
    }
    PowerDownSequence {
        stage: ShutdownStage::Detect,
        ride_through_budget: Duration::from_millis(RIDE_THROUGH_BUDGET_MS),
        checkpoint_done: false,
    }
}

/// Advance the shutdown sequence based on `ev`.
///
/// # Transitions
///
/// | Stage             | Event                | Result                         |
/// |-------------------|----------------------|--------------------------------|
/// | Detect            | PowerLost            | → RideThrough                  |
/// | RideThrough       | PowerLost            | → Checkpoint                   |
/// | Checkpoint        | PowerLost            | → GracefulShutdown (if done)   |
/// | GracefulShutdown  | PowerLost            | → HardOff                      |
/// | RideThrough/Checkpoint/GracefulShutdown | RideThroughTimeout | → HardOff |
/// | Any               | PowerRestored        | `Err(NotAuthorized)`           |
/// | HardOff           | any                  | `Err(InvalidTransition)`       |
pub fn advance_sequence(seq: &mut PowerDownSequence, ev: PowerEvent) -> Result<(), SequenceError> {
    match (seq.stage, ev) {
        // Normal tasks cannot cancel the shutdown sequence via advance_sequence.
        (_, PowerEvent::PowerRestored) => Err(SequenceError::NotAuthorized),

        // HardOff is terminal — no transitions out.
        (ShutdownStage::HardOff, _) => Err(SequenceError::InvalidTransition),

        // PowerLost advances the sequence forward.
        (ShutdownStage::Detect, PowerEvent::PowerLost) => {
            seq.stage = ShutdownStage::RideThrough;
            Ok(())
        }
        (ShutdownStage::RideThrough, PowerEvent::PowerLost) => {
            seq.stage = ShutdownStage::Checkpoint;
            Ok(())
        }
        (ShutdownStage::Checkpoint, PowerEvent::PowerLost) => {
            if seq.checkpoint_done {
                seq.stage = ShutdownStage::GracefulShutdown;
                Ok(())
            } else {
                Err(SequenceError::InvalidTransition)
            }
        }
        (ShutdownStage::GracefulShutdown, PowerEvent::PowerLost) => {
            seq.stage = ShutdownStage::HardOff;
            Ok(())
        }

        // Ride-through timeout forces HardOff from any active ride stage.
        (
            ShutdownStage::RideThrough
            | ShutdownStage::Checkpoint
            | ShutdownStage::GracefulShutdown,
            PowerEvent::RideThroughTimeout,
        ) => {
            seq.stage = ShutdownStage::HardOff;
            Ok(())
        }

        // Ride-through timeout in Detect is invalid (haven't started yet).
        (ShutdownStage::Detect, PowerEvent::RideThroughTimeout) => {
            Err(SequenceError::InvalidTransition)
        }
    }
}

/// Flush emergency checkpoint to persistent storage via the injected callback.
///
/// If no callback is registered, returns `Err(IoError)`.
/// If a checkpoint is already in progress, returns `Err(AlreadyInProgress)`.
pub fn emergency_checkpoint() -> Result<(), CheckpointError> {
    // Re-entrancy guard.
    {
        let mut in_progress = CHECKPOINT_IN_PROGRESS.lock();
        if *in_progress {
            return Err(CheckpointError::AlreadyInProgress);
        }
        *in_progress = true;
    }

    // Invoke callback outside of the in-progress lock to allow the callback
    // to safely call other power APIs. (A recursive call to
    // emergency_checkpoint will hit the guard above and return
    // AlreadyInProgress.)
    let result = match *CHECKPOINT_CALLBACK.lock() {
        Some(cb) => cb(),
        None => Err(CheckpointError::IoError),
    };

    *CHECKPOINT_IN_PROGRESS.lock() = false;
    result
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;
    use crate::detect;

    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    fn reset_state() {
        *crate::POWER_STATE.lock() = crate::PowerState {
            main_power_ok: true,
            ups_soc: 100,
            last_checkpoint: Duration::from_millis(0),
            in_shutdown: false,
        };
        *CHECKPOINT_CALLBACK.lock() = None;
        *CHECKPOINT_IN_PROGRESS.lock() = false;
        *detect::POWER_IRQ_CALLBACK.lock() = None;
    }

    // ------------------------------------------------------------------------
    // State machine transition tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_full_shutdown_sequence() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        let mut seq = on_power_loss();
        assert_eq!(seq.stage, ShutdownStage::Detect);

        // Detect → RideThrough
        advance_sequence(&mut seq, PowerEvent::PowerLost).unwrap();
        assert_eq!(seq.stage, ShutdownStage::RideThrough);

        // RideThrough → Checkpoint
        advance_sequence(&mut seq, PowerEvent::PowerLost).unwrap();
        assert_eq!(seq.stage, ShutdownStage::Checkpoint);

        // Complete checkpoint before proceeding.
        seq.checkpoint_done = true;

        // Checkpoint → GracefulShutdown
        advance_sequence(&mut seq, PowerEvent::PowerLost).unwrap();
        assert_eq!(seq.stage, ShutdownStage::GracefulShutdown);

        // GracefulShutdown → HardOff
        advance_sequence(&mut seq, PowerEvent::PowerLost).unwrap();
        assert_eq!(seq.stage, ShutdownStage::HardOff);
    }

    #[test]
    fn test_ride_through_timeout_to_hard_off() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        let mut seq = on_power_loss();
        advance_sequence(&mut seq, PowerEvent::PowerLost).unwrap(); // → RideThrough
        advance_sequence(&mut seq, PowerEvent::PowerLost).unwrap(); // → Checkpoint

        // Ride-through times out before checkpoint completes.
        assert!(!seq.checkpoint_done);
        advance_sequence(&mut seq, PowerEvent::RideThroughTimeout).unwrap();
        assert_eq!(seq.stage, ShutdownStage::HardOff);
        assert!(!seq.checkpoint_done);
    }

    #[test]
    fn test_ride_through_timeout_from_ride_through() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        let mut seq = on_power_loss();
        advance_sequence(&mut seq, PowerEvent::PowerLost).unwrap(); // → RideThrough

        // Timeout during ride-through (before checkpoint even starts).
        advance_sequence(&mut seq, PowerEvent::RideThroughTimeout).unwrap();
        assert_eq!(seq.stage, ShutdownStage::HardOff);
    }

    #[test]
    fn test_ride_through_timeout_from_graceful_shutdown() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        let mut seq = on_power_loss();
        advance_sequence(&mut seq, PowerEvent::PowerLost).unwrap(); // → RideThrough
        advance_sequence(&mut seq, PowerEvent::PowerLost).unwrap(); // → Checkpoint
        seq.checkpoint_done = true;
        advance_sequence(&mut seq, PowerEvent::PowerLost).unwrap(); // → GracefulShutdown

        // Timeout during graceful shutdown.
        advance_sequence(&mut seq, PowerEvent::RideThroughTimeout).unwrap();
        assert_eq!(seq.stage, ShutdownStage::HardOff);
    }

    // ------------------------------------------------------------------------
    // Power restoration / cancellation tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_power_restore_cancels_shutdown() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        let _seq = on_power_loss();
        assert!(crate::current_state().in_shutdown);

        // Power restored via detect module (authorized path).
        detect::notify_power_restored();
        assert!(!crate::current_state().in_shutdown);
        assert!(crate::current_state().main_power_ok);
    }

    #[test]
    fn test_normal_task_cancel_rejected() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        let mut seq = on_power_loss();
        advance_sequence(&mut seq, PowerEvent::PowerLost).unwrap(); // → RideThrough

        // Normal task tries to cancel via advance_sequence — rejected.
        let result = advance_sequence(&mut seq, PowerEvent::PowerRestored);
        assert_eq!(result, Err(SequenceError::NotAuthorized));
        // Stage unchanged.
        assert_eq!(seq.stage, ShutdownStage::RideThrough);
    }

    #[test]
    fn test_power_restored_rejected_from_all_stages() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        let mut seq = on_power_loss();
        // Detect
        assert_eq!(
            advance_sequence(&mut seq, PowerEvent::PowerRestored),
            Err(SequenceError::NotAuthorized)
        );

        advance_sequence(&mut seq, PowerEvent::PowerLost).unwrap(); // → RideThrough
        assert_eq!(
            advance_sequence(&mut seq, PowerEvent::PowerRestored),
            Err(SequenceError::NotAuthorized)
        );

        advance_sequence(&mut seq, PowerEvent::PowerLost).unwrap(); // → Checkpoint
        assert_eq!(
            advance_sequence(&mut seq, PowerEvent::PowerRestored),
            Err(SequenceError::NotAuthorized)
        );
    }

    // ------------------------------------------------------------------------
    // Invalid transition tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_checkpoint_not_done_cannot_advance() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        let mut seq = on_power_loss();
        advance_sequence(&mut seq, PowerEvent::PowerLost).unwrap(); // → RideThrough
        advance_sequence(&mut seq, PowerEvent::PowerLost).unwrap(); // → Checkpoint

        // checkpoint_done is false — cannot proceed to GracefulShutdown.
        let result = advance_sequence(&mut seq, PowerEvent::PowerLost);
        assert_eq!(result, Err(SequenceError::InvalidTransition));
        assert_eq!(seq.stage, ShutdownStage::Checkpoint);
    }

    #[test]
    fn test_hard_off_is_terminal() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        let mut seq = on_power_loss();
        advance_sequence(&mut seq, PowerEvent::PowerLost).unwrap(); // → RideThrough
        advance_sequence(&mut seq, PowerEvent::PowerLost).unwrap(); // → Checkpoint
        seq.checkpoint_done = true;
        advance_sequence(&mut seq, PowerEvent::PowerLost).unwrap(); // → GracefulShutdown
        advance_sequence(&mut seq, PowerEvent::PowerLost).unwrap(); // → HardOff

        // No event can transition out of HardOff.
        assert_eq!(
            advance_sequence(&mut seq, PowerEvent::PowerLost),
            Err(SequenceError::InvalidTransition)
        );
        assert_eq!(
            advance_sequence(&mut seq, PowerEvent::RideThroughTimeout),
            Err(SequenceError::InvalidTransition)
        );
    }

    #[test]
    fn test_ride_through_timeout_in_detect_invalid() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        let mut seq = on_power_loss();
        let result = advance_sequence(&mut seq, PowerEvent::RideThroughTimeout);
        assert_eq!(result, Err(SequenceError::InvalidTransition));
        assert_eq!(seq.stage, ShutdownStage::Detect);
    }

    // ------------------------------------------------------------------------
    // Emergency checkpoint tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_emergency_checkpoint_success() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        fn success_cb() -> Result<(), CheckpointError> {
            Ok(())
        }
        register_checkpoint_callback(success_cb);

        let result = emergency_checkpoint();
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn test_emergency_checkpoint_failure_io() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        fn fail_cb() -> Result<(), CheckpointError> {
            Err(CheckpointError::IoError)
        }
        register_checkpoint_callback(fail_cb);

        let result = emergency_checkpoint();
        assert_eq!(result, Err(CheckpointError::IoError));
    }

    #[test]
    fn test_emergency_checkpoint_failure_timeout() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        fn timeout_cb() -> Result<(), CheckpointError> {
            Err(CheckpointError::Timeout)
        }
        register_checkpoint_callback(timeout_cb);

        let result = emergency_checkpoint();
        assert_eq!(result, Err(CheckpointError::Timeout));
    }

    #[test]
    fn test_emergency_checkpoint_no_callback() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        // No callback registered — returns IoError.
        let result = emergency_checkpoint();
        assert_eq!(result, Err(CheckpointError::IoError));
    }

    #[test]
    fn test_emergency_checkpoint_already_in_progress() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        // Manually set in-progress flag to simulate concurrent checkpoint.
        *CHECKPOINT_IN_PROGRESS.lock() = true;
        let result = emergency_checkpoint();
        assert_eq!(result, Err(CheckpointError::AlreadyInProgress));

        // Clear for other tests.
        *CHECKPOINT_IN_PROGRESS.lock() = false;
    }

    #[test]
    fn test_emergency_checkpoint_recursive_rejected() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        static RECURSIVE_RESULT: StdMutex<Option<Result<(), CheckpointError>>> =
            StdMutex::new(None);

        fn recursive_cb() -> Result<(), CheckpointError> {
            // Attempt recursive call — should be rejected.
            let inner = emergency_checkpoint();
            *RECURSIVE_RESULT.lock().unwrap_or_else(|e| e.into_inner()) = Some(inner);
            Ok(())
        }
        register_checkpoint_callback(recursive_cb);

        let outer = emergency_checkpoint();
        assert_eq!(outer, Ok(()));
        // The recursive call inside the callback was blocked.
        assert_eq!(
            *RECURSIVE_RESULT.lock().unwrap_or_else(|e| e.into_inner()),
            Some(Err(CheckpointError::AlreadyInProgress))
        );
    }

    // ------------------------------------------------------------------------
    // Ride-through budget tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_ride_through_budget_set() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        let seq = on_power_loss();
        assert_eq!(
            seq.ride_through_budget,
            Duration::from_millis(RIDE_THROUGH_BUDGET_MS)
        );
    }
}
