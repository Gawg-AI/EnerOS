//! Control command types and the command bus (v0.22.0).
//!
//! Provides [`ControlCommand`], the primary data structure dispatched from
//! the Agent plane to the RTOS plane, along with the global command ring
//! ([`CMD_RING`]) and last-command cache ([`LAST_CMD`]).
//!
//! # Encoding
//!
//! Commands are serialized via raw byte copy (`copy_nonoverlapping`) into
//! a 256-byte ring slot. The encoding is symmetric: [`encode_command`] and
//! [`decode_command`] are exact inverses. The `ControlCommand` struct is
//! `Copy` and contains no pointers, making byte-level serialization sound.
//!
//! # Design Decisions
//!
//! - **D2**: Global state uses `Spinlock + UnsafeCell<T>` (NOT `static mut`).
//! - The command ring is externally initialized via [`control_bus_init`]
//!   and stored globally; the caller owns the backing buffer.

use core::cell::UnsafeCell;

use eneros_ipc::SpscRing;
use eneros_sched::Spinlock;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Device identifier (newtype over `u32`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceId(pub u32);

/// The control action to be executed by the target device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlAction {
    /// Charge the storage device.
    Charge,
    /// Discharge the storage device.
    Discharge,
    /// No active power command — hold current state.
    Idle,
    /// Emergency stop — immediately curtail output.
    Emergency,
}

/// A pack of operational constraints for a control command.
///
/// All limits are inclusive: the device state must fall within
/// `[low, high]` for each dimension.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ConstraintPack {
    /// Maximum allowed power output (kW).
    pub max_power: f32,
    /// Minimum allowed power output (kW).
    pub min_power: f32,
    /// State-of-charge limits `(low, high)` in percent (0–100).
    pub soc_limit: (f32, f32),
    /// Voltage limits `(low, high)` in volts.
    pub voltage_limit: (f32, f32),
    /// Frequency limits `(low, high)` in Hz.
    pub frequency_limit: (f32, f32),
}

/// A control command dispatched from the Agent plane to the RTOS plane.
///
/// The command carries:
/// - A unique 128-bit `cmd_id` for deduplication
/// - A nanosecond `timestamp` and `ttl_ms` for freshness checking
/// - The `target_device` and `action` to execute
/// - A `setpoint` (power target) with an associated `constraints` pack
/// - A 512-bit `signature` for integrity (populated by the crypto layer)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlCommand {
    /// Unique command identifier (128-bit UUID-like).
    pub cmd_id: [u8; 16],
    /// Nanosecond timestamp at which the command was issued.
    pub timestamp: u64,
    /// Time-to-live in milliseconds; the command is valid for this long
    /// after `timestamp`.
    pub ttl_ms: u32,
    /// Target device for this command.
    pub target_device: DeviceId,
    /// The action to execute.
    pub action: ControlAction,
    /// Power setpoint (kW).
    pub setpoint: f32,
    /// Operational constraints to enforce.
    pub constraints: ConstraintPack,
    /// Cryptographic signature (512-bit, e.g. Ed25519 or SM2).
    pub signature: [u8; 64],
}

impl Default for ControlCommand {
    fn default() -> Self {
        Self {
            cmd_id: [0; 16],
            timestamp: 0,
            ttl_ms: 0,
            target_device: DeviceId(0),
            action: ControlAction::Idle,
            setpoint: 0.0,
            constraints: ConstraintPack::default(),
            signature: [0; 64],
        }
    }
}

/// Control bus error variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CbError {
    /// The command bus has not been initialized with a ring buffer.
    NotInitialized,
    /// The ring buffer is full — no more commands can be enqueued.
    RingFull,
    /// The ring buffer is empty — no command to consume.
    RingEmpty,
    /// The command failed structural validation.
    InvalidCommand,
    /// The command signature verification failed.
    SignatureFailed,
}

// ---------------------------------------------------------------------------
// Global state (D2: Spinlock + UnsafeCell)
// ---------------------------------------------------------------------------

/// Global command ring state.
///
/// Wraps an `Option<SpscRing>` in `UnsafeCell` with a `Spinlock` for
/// serialized access. The ring is `None` until [`control_bus_init`] is
/// called.
struct CmdRingState {
    lock: Spinlock,
    ring: UnsafeCell<Option<SpscRing>>,
}

// SAFETY: Access to `ring` is serialized by `lock`. All public API
// functions acquire the spinlock before reading/writing `ring`.
unsafe impl Sync for CmdRingState {}

static CMD_RING: CmdRingState = CmdRingState {
    lock: Spinlock::new(),
    ring: UnsafeCell::new(None),
};

/// Global last-command cache.
///
/// Stores the most recently consumed command so that the fallback module
/// can replay it if the Agent plane crashes. Protected by a `Spinlock`.
struct LastCmdState {
    lock: Spinlock,
    cmd: UnsafeCell<Option<ControlCommand>>,
}

// SAFETY: Access to `cmd` is serialized by `lock`. All accessors acquire
// the spinlock before reading/writing `cmd`.
unsafe impl Sync for LastCmdState {}

static LAST_CMD: LastCmdState = LastCmdState {
    lock: Spinlock::new(),
    cmd: UnsafeCell::new(None),
};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the command bus with a pre-configured [`SpscRing`].
///
/// The ring's `slot_size` must be at least `size_of::<ControlCommand>()`
/// (typically 256 bytes). The caller owns the backing buffer and must
/// ensure it outlives the ring.
pub fn control_bus_init(ring: SpscRing) {
    CMD_RING.lock.lock();
    // SAFETY: We hold the lock.
    unsafe {
        *CMD_RING.ring.get() = Some(ring);
    }
    CMD_RING.lock.unlock();
}

/// Send a control command through the command bus.
///
/// Encodes `cmd` into a byte buffer and pushes it into the ring. Returns
/// `Err(NotInitialized)` if the bus has not been initialized, or
/// `Err(RingFull)` if the ring is at capacity.
pub fn command_send(cmd: &ControlCommand) -> Result<(), CbError> {
    CMD_RING.lock.lock();
    let result = {
        // SAFETY: We hold the lock.
        let ring_opt = unsafe { &*CMD_RING.ring.get() };
        match ring_opt {
            None => Err(CbError::NotInitialized),
            Some(ring) => {
                let mut buf = [0u8; 256];
                let len = encode_command(cmd, &mut buf);
                match ring.push(&buf[..len]) {
                    Ok(()) => Ok(()),
                    Err(_) => Err(CbError::RingFull),
                }
            }
        }
    };
    CMD_RING.lock.unlock();
    result
}

/// Consume the next command from the command bus.
///
/// Pops one command from the ring, decodes it, and updates the
/// last-command cache. Returns `None` if the bus is not initialized or
/// the ring is empty.
pub fn command_consume() -> Option<ControlCommand> {
    CMD_RING.lock.lock();
    let result = {
        // SAFETY: We hold the lock.
        let ring_opt = unsafe { &*CMD_RING.ring.get() };
        match ring_opt {
            None => None,
            Some(ring) => {
                let mut buf = [0u8; 256];
                match ring.pop(&mut buf) {
                    Ok(len) => {
                        let cmd = decode_command(&buf[..len]);
                        Some(cmd)
                    }
                    Err(_) => None,
                }
            }
        }
    };
    CMD_RING.lock.unlock();

    if let Some(cmd) = result {
        set_last_cmd(cmd);
    }

    result
}

// ---------------------------------------------------------------------------
// pub(crate) helpers (used by fallback, integration, and tests)
// ---------------------------------------------------------------------------

/// Encode a `ControlCommand` into `buf` via raw byte copy.
///
/// Returns the number of bytes written (`size_of::<ControlCommand>()`).
/// The caller must ensure `buf.len() >= size_of::<ControlCommand>()`.
pub(crate) fn encode_command(cmd: &ControlCommand, buf: &mut [u8]) -> usize {
    let len = core::mem::size_of::<ControlCommand>();
    if buf.len() >= len {
        // SAFETY: `cmd` is a valid reference to a `ControlCommand`. `buf`
        // has at least `len` bytes. The two regions do not overlap
        // (`cmd` is borrowed externally, `buf` is local).
        unsafe {
            core::ptr::copy_nonoverlapping(
                cmd as *const ControlCommand as *const u8,
                buf.as_mut_ptr(),
                len,
            );
        }
    }
    len
}

/// Decode a `ControlCommand` from `buf` via raw byte copy.
///
/// Creates a default `ControlCommand` and overwrites its bytes from `buf`.
/// The caller must ensure `buf.len() >= size_of::<ControlCommand>()`.
pub(crate) fn decode_command(buf: &[u8]) -> ControlCommand {
    let mut cmd = ControlCommand::default();
    let len = core::mem::size_of::<ControlCommand>();
    if buf.len() >= len {
        // SAFETY: `cmd` is a local variable, properly aligned and writable.
        // `buf` has at least `len` bytes. The two regions do not overlap.
        unsafe {
            core::ptr::copy_nonoverlapping(
                buf.as_ptr(),
                &mut cmd as *mut ControlCommand as *mut u8,
                len,
            );
        }
    }
    cmd
}

/// Read the last consumed command (used by the fallback module).
pub(crate) fn get_last_cmd() -> Option<ControlCommand> {
    LAST_CMD.lock.lock();
    // SAFETY: We hold the lock.
    let result = unsafe { *LAST_CMD.cmd.get() };
    LAST_CMD.lock.unlock();
    result
}

/// Write the last consumed command (used by `command_consume` and tests).
pub(crate) fn set_last_cmd(cmd: ControlCommand) {
    LAST_CMD.lock.lock();
    // SAFETY: We hold the lock.
    unsafe {
        *LAST_CMD.cmd.get() = Some(cmd);
    }
    LAST_CMD.lock.unlock();
}

/// Clear the last-command cache (test-only helper).
#[cfg(test)]
pub(crate) fn reset_last_cmd() {
    LAST_CMD.lock.lock();
    // SAFETY: We hold the lock.
    unsafe {
        *LAST_CMD.cmd.get() = None;
    }
    LAST_CMD.lock.unlock();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        crate::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Reset both CMD_RING and LAST_CMD to their initial (empty) state.
    fn reset_state() {
        CMD_RING.lock.lock();
        // SAFETY: We hold the lock.
        unsafe {
            *CMD_RING.ring.get() = None;
        }
        CMD_RING.lock.unlock();
        reset_last_cmd();
    }

    /// Create a test ring backed by a static buffer.
    ///
    /// Slot size 256, 16 slots (4096 bytes total).
    static mut RING_BUF: [u8; 4096] = [0; 4096];

    #[allow(static_mut_refs)]
    fn make_test_ring() -> SpscRing {
        // SAFETY: Tests are serialized by `TEST_LOCK`, so only one test
        // accesses `RING_BUF` at a time. Each call resets head/tail to 0.
        unsafe { SpscRing::new(&mut RING_BUF, 256, 16) }
    }

    /// Create a fully-populated test command for roundtrip verification.
    fn make_test_cmd() -> ControlCommand {
        ControlCommand {
            cmd_id: [0xAA; 16],
            timestamp: 1_000,
            ttl_ms: 100,
            target_device: DeviceId(42),
            action: ControlAction::Charge,
            setpoint: 50.5,
            constraints: ConstraintPack {
                max_power: 100.0,
                min_power: 0.0,
                soc_limit: (10.0, 90.0),
                voltage_limit: (200.0, 400.0),
                frequency_limit: (49.0, 51.0),
            },
            signature: [0xBB; 64],
        }
    }

    /// Compare two commands field-by-field (f32 via `to_bits` for exactness).
    fn assert_cmds_equal(a: &ControlCommand, b: &ControlCommand) {
        assert_eq!(a.cmd_id, b.cmd_id, "cmd_id mismatch");
        assert_eq!(a.timestamp, b.timestamp, "timestamp mismatch");
        assert_eq!(a.ttl_ms, b.ttl_ms, "ttl_ms mismatch");
        assert_eq!(a.target_device, b.target_device, "target_device mismatch");
        assert_eq!(a.action, b.action, "action mismatch");
        assert_eq!(
            a.setpoint.to_bits(),
            b.setpoint.to_bits(),
            "setpoint mismatch"
        );
        assert_eq!(
            a.constraints.max_power.to_bits(),
            b.constraints.max_power.to_bits(),
            "max_power mismatch"
        );
        assert_eq!(
            a.constraints.min_power.to_bits(),
            b.constraints.min_power.to_bits(),
            "min_power mismatch"
        );
        assert_eq!(
            a.constraints.soc_limit.0.to_bits(),
            b.constraints.soc_limit.0.to_bits(),
            "soc_limit.0 mismatch"
        );
        assert_eq!(
            a.constraints.soc_limit.1.to_bits(),
            b.constraints.soc_limit.1.to_bits(),
            "soc_limit.1 mismatch"
        );
        assert_eq!(
            a.constraints.voltage_limit.0.to_bits(),
            b.constraints.voltage_limit.0.to_bits(),
            "voltage_limit.0 mismatch"
        );
        assert_eq!(
            a.constraints.voltage_limit.1.to_bits(),
            b.constraints.voltage_limit.1.to_bits(),
            "voltage_limit.1 mismatch"
        );
        assert_eq!(
            a.constraints.frequency_limit.0.to_bits(),
            b.constraints.frequency_limit.0.to_bits(),
            "frequency_limit.0 mismatch"
        );
        assert_eq!(
            a.constraints.frequency_limit.1.to_bits(),
            b.constraints.frequency_limit.1.to_bits(),
            "frequency_limit.1 mismatch"
        );
        assert_eq!(a.signature, b.signature, "signature mismatch");
    }

    #[test]
    fn test_command_send_not_initialized() {
        let _g = lock();
        reset_state();

        let cmd = make_test_cmd();
        let result = command_send(&cmd);
        assert_eq!(result, Err(CbError::NotInitialized));
    }

    #[test]
    fn test_command_consume_not_initialized() {
        let _g = lock();
        reset_state();

        let result = command_consume();
        assert_eq!(result, None);
    }

    #[test]
    fn test_command_send_consume_roundtrip() {
        let _g = lock();
        reset_state();

        let ring = make_test_ring();
        control_bus_init(ring);

        let cmd = make_test_cmd();
        assert_eq!(command_send(&cmd), Ok(()));

        let consumed = command_consume();
        assert!(consumed.is_some(), "expected a command to be consumed");
        let consumed = consumed.unwrap();
        assert_cmds_equal(&cmd, &consumed);
    }

    #[test]
    fn test_encode_decode_symmetric() {
        let _g = lock();
        reset_state();

        let cmd = make_test_cmd();
        let mut buf = [0u8; 256];
        let len = encode_command(&cmd, &mut buf);
        assert_eq!(len, core::mem::size_of::<ControlCommand>());

        let decoded = decode_command(&buf[..len]);
        assert_cmds_equal(&cmd, &decoded);
    }

    #[test]
    fn test_command_consume_updates_last_cmd() {
        let _g = lock();
        reset_state();

        let ring = make_test_ring();
        control_bus_init(ring);

        let cmd = make_test_cmd();
        assert_eq!(command_send(&cmd), Ok(()));

        // Before consume, last_cmd is None.
        assert!(get_last_cmd().is_none());

        let consumed = command_consume();
        assert!(consumed.is_some());

        // After consume, last_cmd matches the consumed command.
        let last = get_last_cmd();
        assert!(last.is_some(), "last_cmd should be set after consume");
        let last = last.unwrap();
        assert_cmds_equal(&cmd, &last);
    }

    #[test]
    fn test_command_consume_empty_returns_none() {
        let _g = lock();
        reset_state();

        let ring = make_test_ring();
        control_bus_init(ring);

        // Consume without sending anything — ring is empty.
        let result = command_consume();
        assert_eq!(result, None);
    }
}
