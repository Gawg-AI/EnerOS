//! EnerOS Power Management — Edge Box power-loss detection and shutdown sequence.
//!
//! This crate provides:
//! - **Power-loss detection** via dual-redundant ADC + GPIO (aarch64) or host mock
//! - **UPS/supercapacitor ride-through** budget tracking
//! - **Emergency checkpoint** flush via injected callback (FS-independent)
//! - **Graceful shutdown sequence** state machine (Detect → RideThrough → Checkpoint
//!   → GracefulShutdown → HardOff)
//!
//! # Usage
//!
//! ```ignore
//! use eneros_power::{on_power_loss, advance_sequence, PowerEvent, ShutdownStage};
//!
//! // Register power-loss interrupt callback (called by detect module on IRQ)
//! eneros_power::register_power_irq(|| {
//!     let mut seq = on_power_loss();
//!     // Drive sequence forward...
//!     advance_sequence(&mut seq, PowerEvent::PowerLost).ok();
//! });
//! ```

#![cfg_attr(not(test), no_std)]

use core::time::Duration;

use spin::Mutex;

pub mod detect;
pub mod sequence;

// Re-export public API
pub use detect::register_power_irq;
pub use sequence::{
    advance_sequence, emergency_checkpoint, on_power_loss, register_checkpoint_callback,
};

// ============================================================================
// Type definitions
// ============================================================================

/// Shutdown sequence stages, ordered from detection to hard power-off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownStage {
    /// Power loss detected — entering ride-through.
    Detect,
    /// UPS/supercapacitor sustaining the system.
    RideThrough,
    /// Flushing emergency checkpoint to persistent storage.
    Checkpoint,
    /// Graceful shutdown — unmount, notify, cleanup.
    GracefulShutdown,
    /// Hardware power-off — sequence is terminal.
    HardOff,
}

/// Events that drive the shutdown sequence state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    /// Main power lost (advances the sequence forward).
    PowerLost,
    /// Main power restored (cancels shutdown — requires authorization).
    PowerRestored,
    /// Ride-through budget exhausted (forces HardOff).
    RideThroughTimeout,
}

/// Power-down sequence state. Driven by [`advance_sequence`].
#[derive(Debug)]
pub struct PowerDownSequence {
    /// Current stage in the shutdown sequence.
    pub stage: ShutdownStage,
    /// Ride-through time budget (from UPS/supercapacitor).
    pub ride_through_budget: Duration,
    /// Whether the emergency checkpoint has completed successfully.
    pub checkpoint_done: bool,
}

/// Snapshot of the current power subsystem state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerState {
    /// Whether main power is currently OK.
    pub main_power_ok: bool,
    /// UPS/supercapacitor state of charge (0–100 %).
    pub ups_soc: u8,
    /// Timestamp of the last successful checkpoint.
    pub last_checkpoint: Duration,
    /// Whether the system is currently in a shutdown sequence.
    pub in_shutdown: bool,
}

/// Errors from [`emergency_checkpoint`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointError {
    /// I/O error during checkpoint flush.
    IoError,
    /// Checkpoint timed out.
    Timeout,
    /// A checkpoint is already in progress.
    AlreadyInProgress,
}

/// Errors from [`advance_sequence`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceError {
    /// Caller is not authorized to cancel the shutdown sequence.
    NotAuthorized,
    /// The requested transition is invalid for the current stage.
    InvalidTransition,
}

// ============================================================================
// Global state
// ============================================================================

/// Global power state shared across detect and sequence modules.
pub(crate) static POWER_STATE: Mutex<PowerState> = Mutex::new(PowerState {
    main_power_ok: true,
    ups_soc: 100,
    last_checkpoint: Duration::from_millis(0),
    in_shutdown: false,
});

// ============================================================================
// Public API
// ============================================================================

/// Returns a snapshot of the current power subsystem state.
pub fn current_state() -> PowerState {
    *POWER_STATE.lock()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;

    // Production statics are shared `spin::Mutex`-protected state. Tests
    // serialize on this guard to avoid cross-test races.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    fn reset_state() {
        *POWER_STATE.lock() = PowerState {
            main_power_ok: true,
            ups_soc: 100,
            last_checkpoint: Duration::from_millis(0),
            in_shutdown: false,
        };
        *detect::POWER_IRQ_CALLBACK.lock() = None;
        *sequence::CHECKPOINT_CALLBACK.lock() = None;
        *sequence::CHECKPOINT_IN_PROGRESS.lock() = false;
    }

    #[test]
    fn test_on_power_loss_initial_state() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        let seq = on_power_loss();
        assert_eq!(seq.stage, ShutdownStage::Detect);
        assert!(!seq.checkpoint_done);
        assert!(seq.ride_through_budget > Duration::from_millis(0));
    }

    #[test]
    fn test_current_state_default() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        let state = current_state();
        assert!(state.main_power_ok);
        assert!(!state.in_shutdown);
        assert_eq!(state.ups_soc, 100);
    }

    #[test]
    fn test_current_state_after_power_loss() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        let _seq = on_power_loss();
        let state = current_state();
        assert!(!state.main_power_ok);
        assert!(state.in_shutdown);
    }

    #[test]
    fn test_current_state_reflects_restore() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        // Simulate power loss then restoration via detect module.
        detect::set_main_power_ok(false);
        assert!(!current_state().main_power_ok);
        assert!(current_state().in_shutdown);

        detect::set_main_power_ok(true);
        assert!(current_state().main_power_ok);
        assert!(!current_state().in_shutdown);
    }
}
