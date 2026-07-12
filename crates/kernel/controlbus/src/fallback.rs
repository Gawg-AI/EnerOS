//! Dual-plane fallback strategy (v0.22.0).
//!
//! Provides [`execute_or_fallback`] which decides the RTOS plane's operating
//! mode based on the availability and freshness of control commands from
//! the Agent plane.
//!
//! # Fallback modes
//!
//! | Mode | Trigger | Behavior |
//! |------|---------|----------|
//! | [`Normal`] | A fresh command is available | Execute the command |
//! | [`WaitForCommand`] | No new command, but last command is still valid | Hold the last command |
//! | [`SafeDefault`] | No fresh command and last command is expired/absent | Fall back to safe defaults |
//! | [`Emergency`] | (Reserved for explicit emergency triggers) | Curtail output |
//!
//! # Design
//!
//! When the Agent plane is alive, it passes `Some(cmd)` to
//! [`execute_or_fallback`]. If the command's TTL has not expired, the mode
//! is [`Normal`]; otherwise it is [`SafeDefault`].
//!
//! When the Agent plane has crashed, it passes `None`. The fallback logic
//! then checks the last consumed command (stored in `LAST_CMD`). If it is
//! still within TTL, the mode is [`WaitForCommand`] (the RTOS holds the
//! last command). If the last command has also expired or there is none,
//! the mode is [`SafeDefault`].
//!
//! [`Normal`]: FallbackMode::Normal
//! [`WaitForCommand`]: FallbackMode::WaitForCommand
//! [`SafeDefault`]: FallbackMode::SafeDefault
//! [`Emergency`]: FallbackMode::Emergency

use crate::command::{get_last_cmd, ControlCommand};
use crate::ttl::{ttl_check, TtlStatus};

/// The operating mode of the RTOS plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackMode {
    /// Agent plane is alive and issuing fresh commands.
    Normal,
    /// Agent plane has stopped issuing new commands, but the last command
    /// is still within TTL — hold it.
    WaitForCommand,
    /// No valid command available — fall back to safe default behavior.
    SafeDefault,
    /// Emergency mode — explicit curtailment required.
    Emergency,
}

/// Decide the fallback mode based on command availability and freshness.
///
/// - If `cmd` is `Some(c)` and `c` is within TTL → [`Normal`](FallbackMode::Normal)
/// - If `cmd` is `Some(c)` and `c` has expired → [`SafeDefault`](FallbackMode::SafeDefault)
/// - If `cmd` is `None` and the last consumed command is within TTL → [`WaitForCommand`](FallbackMode::WaitForCommand)
/// - If `cmd` is `None` and the last command is expired or absent → [`SafeDefault`](FallbackMode::SafeDefault)
pub fn execute_or_fallback(cmd: Option<&ControlCommand>, now_ns: u64) -> FallbackMode {
    match cmd {
        Some(c) => {
            if ttl_check(c, now_ns) == TtlStatus::Expired {
                FallbackMode::SafeDefault
            } else {
                FallbackMode::Normal
            }
        }
        None => {
            // No new command — check the last consumed command.
            if let Some(last) = get_last_cmd() {
                if ttl_check(&last, now_ns) == TtlStatus::Valid {
                    FallbackMode::WaitForCommand
                } else {
                    FallbackMode::SafeDefault
                }
            } else {
                FallbackMode::SafeDefault
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{set_last_cmd, ControlCommand};

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        crate::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn test_normal_with_valid_command() {
        let _g = lock();
        crate::command::reset_last_cmd();

        let cmd = ControlCommand {
            timestamp: 1_000,
            ttl_ms: 100,
            ..Default::default()
        };
        // now is 50 ms after timestamp — within TTL.
        let now_ns = 1_000 + 50_000_000;
        assert_eq!(
            execute_or_fallback(Some(&cmd), now_ns),
            FallbackMode::Normal
        );
    }

    #[test]
    fn test_safe_default_with_expired_command() {
        let _g = lock();
        crate::command::reset_last_cmd();

        let cmd = ControlCommand {
            timestamp: 1_000,
            ttl_ms: 10,
            ..Default::default()
        };
        // now is 15 ms after timestamp — expired.
        let now_ns = 1_000 + 15_000_000;
        assert_eq!(
            execute_or_fallback(Some(&cmd), now_ns),
            FallbackMode::SafeDefault
        );
    }

    #[test]
    fn test_wait_for_command_no_new_but_last_valid() {
        let _g = lock();
        crate::command::reset_last_cmd();

        // Set a last command that is still within TTL.
        let last = ControlCommand {
            timestamp: 1_000,
            ttl_ms: 100,
            ..Default::default()
        };
        set_last_cmd(last);

        // now is 50 ms after timestamp — within TTL.
        let now_ns = 1_000 + 50_000_000;
        assert_eq!(
            execute_or_fallback(None, now_ns),
            FallbackMode::WaitForCommand
        );
    }

    #[test]
    fn test_safe_default_no_new_and_last_expired() {
        let _g = lock();
        crate::command::reset_last_cmd();

        // Set a last command that has expired.
        let last = ControlCommand {
            timestamp: 1_000,
            ttl_ms: 10,
            ..Default::default()
        };
        set_last_cmd(last);

        // now is 15 ms after timestamp — expired.
        let now_ns = 1_000 + 15_000_000;
        assert_eq!(execute_or_fallback(None, now_ns), FallbackMode::SafeDefault);
    }

    #[test]
    fn test_safe_default_no_new_no_last() {
        let _g = lock();
        crate::command::reset_last_cmd();

        // No last command set — should fall back to SafeDefault.
        let now_ns = 1_000;
        assert_eq!(execute_or_fallback(None, now_ns), FallbackMode::SafeDefault);
    }
}
