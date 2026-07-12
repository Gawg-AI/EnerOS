//! Dual-plane integration simulation (v0.22.0, Phase 0 exit verification).
//!
//! Provides [`integration_step`] which simulates the interaction between
//! the Agent plane and the RTOS plane across the agent-alive → crash →
//! recovery lifecycle. This module is the Phase 0 capstone — it exercises
//! the command bus, TTL checking, and fallback logic together to verify
//! that dual-plane coordination works end-to-end.
//!
//! # Lifecycle simulation
//!
//! 1. **Agent alive**: [`integration_step`] simulates the agent sending a
//!    fresh command (updates `last_cmd_time`, stores the command in
//!    `LAST_CMD`) and sets the mode to [`Normal`].
//! 2. **Agent crashed**: [`simulate_agent_crash`] marks the agent as dead.
//!    Subsequent [`integration_step`] calls invoke
//!    [`execute_or_fallback(None, now)`](crate::fallback::execute_or_fallback),
//!    which checks the last command's TTL:
//!    - Within TTL → [`WaitForCommand`] (hold the last command)
//!    - Expired → [`SafeDefault`] (fall back to safe behavior)
//! 3. **Agent recovery**: [`simulate_agent_recovery`] marks the agent as
//!    alive. The next [`integration_step`] resumes normal operation.

use crate::command::{set_last_cmd, ControlCommand};
use crate::fallback::{execute_or_fallback, FallbackMode};

/// Integration simulation state.
///
/// Tracks the agent's liveness, the last command timestamp, the current
/// fallback mode, and the TTL window for commands.
#[derive(Debug, Clone)]
pub struct IntegrationState {
    /// Whether the Agent plane is currently alive.
    pub agent_alive: bool,
    /// Nanosecond timestamp of the last command issued by the agent.
    pub last_cmd_time: u64,
    /// Current operating mode of the RTOS plane.
    pub current_mode: FallbackMode,
    /// TTL window (ms) for commands issued during simulation.
    pub ttl_ms: u32,
}

/// Create a new integration state with the agent alive and the given TTL.
///
/// `last_cmd_time` starts at 0, `current_mode` starts as [`Normal`].
pub fn new_integration_state(ttl_ms: u32) -> IntegrationState {
    IntegrationState {
        agent_alive: true,
        last_cmd_time: 0,
        current_mode: FallbackMode::Normal,
        ttl_ms,
    }
}

/// Simulate the agent crashing (becoming unresponsive).
pub fn simulate_agent_crash(state: &mut IntegrationState) {
    state.agent_alive = false;
}

/// Simulate the agent recovering (becoming responsive again).
pub fn simulate_agent_recovery(state: &mut IntegrationState) {
    state.agent_alive = true;
}

/// Execute one integration step at time `now_ns`.
///
/// - If the agent is alive: updates `last_cmd_time`, stores a fresh command
///   in `LAST_CMD`, and sets the mode to [`Normal`].
/// - If the agent has crashed: calls
///   [`execute_or_fallback(None, now)`](execute_or_fallback) to determine
///   the mode based on the last command's TTL.
///
/// Returns the resulting [`FallbackMode`].
pub fn integration_step(state: &mut IntegrationState, now_ns: u64) -> FallbackMode {
    if state.agent_alive {
        // Agent is alive — simulate sending a fresh command.
        state.last_cmd_time = now_ns;
        let cmd = ControlCommand {
            timestamp: now_ns,
            ttl_ms: state.ttl_ms,
            ..Default::default()
        };
        set_last_cmd(cmd);
        state.current_mode = FallbackMode::Normal;
    } else {
        // Agent has crashed — fall back to the last command or safe default.
        state.current_mode = execute_or_fallback(None, now_ns);
    }
    state.current_mode
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

    #[test]
    fn test_agent_normal_mode() {
        let _g = lock();
        crate::command::reset_last_cmd();

        let mut state = new_integration_state(100);
        let mode = integration_step(&mut state, 1_000);
        assert_eq!(mode, FallbackMode::Normal);
    }

    #[test]
    fn test_agent_crash_then_ttl_expire() {
        let _g = lock();
        crate::command::reset_last_cmd();

        let mut state = new_integration_state(100);

        // Step 1: agent alive, issues a command at t=1000.
        let mode = integration_step(&mut state, 1_000);
        assert_eq!(mode, FallbackMode::Normal);

        // Step 2: agent crashes.
        simulate_agent_crash(&mut state);

        // Step 3: within TTL (50 ms after timestamp) → WaitForCommand.
        let mode = integration_step(&mut state, 1_000 + 50_000_000);
        assert_eq!(mode, FallbackMode::WaitForCommand);

        // Step 4: after TTL (150 ms after timestamp) → SafeDefault.
        let mode = integration_step(&mut state, 1_000 + 150_000_000);
        assert_eq!(mode, FallbackMode::SafeDefault);
    }

    #[test]
    fn test_agent_recovery() {
        let _g = lock();
        crate::command::reset_last_cmd();

        let mut state = new_integration_state(100);

        // Crash the agent.
        simulate_agent_crash(&mut state);
        let mode = integration_step(&mut state, 1_000);
        // No last command → SafeDefault.
        assert_eq!(mode, FallbackMode::SafeDefault);

        // Recover the agent.
        simulate_agent_recovery(&mut state);
        let mode = integration_step(&mut state, 2_000);
        assert_eq!(mode, FallbackMode::Normal);
    }

    #[test]
    fn test_integration_step_updates_last_cmd_time() {
        let _g = lock();
        crate::command::reset_last_cmd();

        let mut state = new_integration_state(100);
        assert_eq!(state.last_cmd_time, 0);

        // Agent alive — last_cmd_time should be updated.
        let mode = integration_step(&mut state, 42_000_000);
        assert_eq!(mode, FallbackMode::Normal);
        assert_eq!(state.last_cmd_time, 42_000_000);
    }
}
