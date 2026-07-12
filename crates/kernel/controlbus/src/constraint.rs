//! Constraint checking for control commands (v0.22.0).
//!
//! Provides [`constraint_check`] which validates a [`ControlCommand`]
//! against the current [`DeviceState`]. Commands that violate hard limits
//! (SOC, voltage, frequency) are rejected; commands with out-of-range
//! power setpoints are truncated to the nearest bound.
//!
//! # Evaluation order
//!
//! 1. SOC — reject if outside `[soc_limit.0, soc_limit.1]`
//! 2. Voltage — reject if outside `[voltage_limit.0, voltage_limit.1]`
//! 3. Frequency — reject if outside `[frequency_limit.0, frequency_limit.1]`
//! 4. Power setpoint — truncate to `[min_power, max_power]` if out of range
//!
//! Hard-limit violations (1–3) take precedence over setpoint truncation (4).

use crate::command::ControlCommand;

/// Snapshot of the device's current physical state.
///
/// Used by [`constraint_check`] to validate that the device is in a safe
/// operating envelope before executing a command.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeviceState {
    /// State of charge (0–100%).
    pub soc: f32,
    /// Terminal voltage (V).
    pub voltage: f32,
    /// Grid frequency (Hz).
    pub frequency: f32,
    /// Current power output (kW).
    pub current_power: f32,
}

/// The outcome of a constraint check.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConstraintResult {
    /// All constraints satisfied; the command may execute as-is.
    Ok,
    /// The setpoint was out of range and has been truncated to the given
    /// value. The caller should use the truncated setpoint instead.
    Truncated(f32),
    /// A hard limit (SOC, voltage, or frequency) was violated; the command
    /// must not execute.
    Rejected,
}

/// Check `cmd` against the device's current `state`.
///
/// Returns [`ConstraintResult::Rejected`] if any hard limit (SOC, voltage,
/// frequency) is violated. Returns [`ConstraintResult::Truncated`] if the
/// power setpoint is outside `[min_power, max_power]` (with the clamped
/// value). Returns [`ConstraintResult::Ok`] if all constraints are
/// satisfied.
pub fn constraint_check(cmd: &ControlCommand, state: &DeviceState) -> ConstraintResult {
    // 1. SOC check
    if state.soc < cmd.constraints.soc_limit.0 || state.soc > cmd.constraints.soc_limit.1 {
        return ConstraintResult::Rejected;
    }

    // 2. Voltage check
    if state.voltage < cmd.constraints.voltage_limit.0
        || state.voltage > cmd.constraints.voltage_limit.1
    {
        return ConstraintResult::Rejected;
    }

    // 3. Frequency check
    if state.frequency < cmd.constraints.frequency_limit.0
        || state.frequency > cmd.constraints.frequency_limit.1
    {
        return ConstraintResult::Rejected;
    }

    // 4. Power setpoint truncation
    let mut setpoint = cmd.setpoint;
    if setpoint > cmd.constraints.max_power {
        setpoint = cmd.constraints.max_power;
    }
    if setpoint < cmd.constraints.min_power {
        setpoint = cmd.constraints.min_power;
    }
    if setpoint != cmd.setpoint {
        return ConstraintResult::Truncated(setpoint);
    }

    ConstraintResult::Ok
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{ConstraintPack, ControlAction, DeviceId};

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        crate::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Build a command with standard constraints and the given setpoint.
    fn make_cmd(setpoint: f32) -> ControlCommand {
        ControlCommand {
            cmd_id: [1; 16],
            timestamp: 0,
            ttl_ms: 100,
            target_device: DeviceId(1),
            action: ControlAction::Charge,
            setpoint,
            constraints: ConstraintPack {
                max_power: 80.0,
                min_power: 20.0,
                soc_limit: (10.0, 80.0),
                voltage_limit: (200.0, 400.0),
                frequency_limit: (49.0, 51.0),
            },
            signature: [0; 64],
        }
    }

    /// Build a device state with all values within the standard constraints.
    fn make_state() -> DeviceState {
        DeviceState {
            soc: 50.0,
            voltage: 300.0,
            frequency: 50.0,
            current_power: 40.0,
        }
    }

    #[test]
    fn test_constraint_ok() {
        let _g = lock();
        let cmd = make_cmd(50.0);
        let state = make_state();
        assert_eq!(constraint_check(&cmd, &state), ConstraintResult::Ok);
    }

    #[test]
    fn test_constraint_power_high_truncated() {
        let _g = lock();
        let cmd = make_cmd(100.0); // max_power = 80
        let state = make_state();
        assert_eq!(
            constraint_check(&cmd, &state),
            ConstraintResult::Truncated(80.0)
        );
    }

    #[test]
    fn test_constraint_power_low_truncated() {
        let _g = lock();
        let cmd = make_cmd(10.0); // min_power = 20
        let state = make_state();
        assert_eq!(
            constraint_check(&cmd, &state),
            ConstraintResult::Truncated(20.0)
        );
    }

    #[test]
    fn test_constraint_soc_high_rejected() {
        let _g = lock();
        let cmd = make_cmd(50.0);
        let mut state = make_state();
        state.soc = 90.0; // soc_limit = (10, 80)
        assert_eq!(constraint_check(&cmd, &state), ConstraintResult::Rejected);
    }

    #[test]
    fn test_constraint_soc_low_rejected() {
        let _g = lock();
        let cmd = make_cmd(50.0);
        let mut state = make_state();
        state.soc = 5.0; // soc_limit = (10, 80)
        assert_eq!(constraint_check(&cmd, &state), ConstraintResult::Rejected);
    }

    #[test]
    fn test_constraint_voltage_rejected() {
        let _g = lock();
        let cmd = make_cmd(50.0);
        let mut state = make_state();
        state.voltage = 500.0; // voltage_limit = (200, 400)
        assert_eq!(constraint_check(&cmd, &state), ConstraintResult::Rejected);
    }

    #[test]
    fn test_constraint_frequency_rejected() {
        let _g = lock();
        let cmd = make_cmd(50.0);
        let mut state = make_state();
        state.frequency = 55.0; // frequency_limit = (49, 51)
        assert_eq!(constraint_check(&cmd, &state), ConstraintResult::Rejected);
    }

    #[test]
    fn test_constraint_boundary_values() {
        let _g = lock();
        // setpoint exactly at max_power → Ok (no truncation).
        let cmd = make_cmd(80.0);
        let state = make_state();
        assert_eq!(constraint_check(&cmd, &state), ConstraintResult::Ok);

        // setpoint exactly at min_power → Ok.
        let cmd = make_cmd(20.0);
        assert_eq!(constraint_check(&cmd, &state), ConstraintResult::Ok);
    }
}
