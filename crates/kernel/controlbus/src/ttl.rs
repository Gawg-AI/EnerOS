//! TTL (time-to-live) checking for control commands (v0.22.0).
//!
//! Provides [`ttl_check`] which determines whether a [`ControlCommand`] is
//! still valid at a given point in time, based on its `timestamp` and
//! `ttl_ms` fields.
//!
//! # Semantics
//!
//! A command is **Valid** if the elapsed time since `timestamp` is strictly
//! less than `ttl_ms` milliseconds. It is **Expired** once the elapsed time
//! reaches or exceeds `ttl_ms`.
//!
//! Overflow is handled with `saturating_sub`: if `now_ns` is before
//! `timestamp` (clock skew or test scenario), the elapsed time is treated
//! as 0, yielding **Valid**.

use crate::command::ControlCommand;

/// The result of a TTL check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtlStatus {
    /// The command is within its TTL and may be executed.
    Valid,
    /// The command has exceeded its TTL and must not be executed.
    Expired,
}

/// Check whether `cmd` is still valid at `now_ns`.
///
/// Computes `elapsed_ns = now_ns.saturating_sub(cmd.timestamp)` and converts
/// to milliseconds. Returns [`TtlStatus::Expired`] if
/// `elapsed_ms >= cmd.ttl_ms`, otherwise [`TtlStatus::Valid`].
pub fn ttl_check(cmd: &ControlCommand, now_ns: u64) -> TtlStatus {
    let elapsed_ns = now_ns.saturating_sub(cmd.timestamp);
    let elapsed_ms = elapsed_ns / 1_000_000;
    if elapsed_ms >= cmd.ttl_ms as u64 {
        TtlStatus::Expired
    } else {
        TtlStatus::Valid
    }
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
    fn test_ttl_valid() {
        let _g = lock();
        let cmd = ControlCommand {
            timestamp: 1000,
            ttl_ms: 10,
            ..Default::default()
        };
        // 5 ms after timestamp — within TTL.
        let now_ns = 1000 + 5_000_000;
        assert_eq!(ttl_check(&cmd, now_ns), TtlStatus::Valid);
    }

    #[test]
    fn test_ttl_expired() {
        let _g = lock();
        let cmd = ControlCommand {
            timestamp: 1000,
            ttl_ms: 10,
            ..Default::default()
        };
        // 15 ms after timestamp — exceeded TTL.
        let now_ns = 1000 + 15_000_000;
        assert_eq!(ttl_check(&cmd, now_ns), TtlStatus::Expired);
    }

    #[test]
    fn test_ttl_exactly_expired() {
        let _g = lock();
        let cmd = ControlCommand {
            timestamp: 1000,
            ttl_ms: 10,
            ..Default::default()
        };
        // Exactly 10 ms after timestamp — boundary: elapsed == ttl → Expired.
        let now_ns = 1000 + 10_000_000;
        assert_eq!(ttl_check(&cmd, now_ns), TtlStatus::Expired);
    }

    #[test]
    fn test_ttl_zero_immediate_expiry() {
        let _g = lock();
        let cmd = ControlCommand {
            timestamp: 1000,
            ttl_ms: 0,
            ..Default::default()
        };
        // ttl_ms = 0 → immediately expired at any time.
        let now_ns = 1000;
        assert_eq!(ttl_check(&cmd, now_ns), TtlStatus::Expired);
    }

    #[test]
    fn test_ttl_timestamp_zero() {
        let _g = lock();
        let cmd = ControlCommand {
            timestamp: 0,
            ttl_ms: 100,
            ..Default::default()
        };
        // 99.999999 ms after timestamp 0 — just under 100 ms → Valid.
        let now_ns = 99_999_999;
        assert_eq!(ttl_check(&cmd, now_ns), TtlStatus::Valid);
    }

    #[test]
    fn test_ttl_now_before_timestamp() {
        let _g = lock();
        let cmd = ControlCommand {
            timestamp: 1000,
            ttl_ms: 10,
            ..Default::default()
        };
        // now_ns < timestamp — saturating_sub yields 0 elapsed → Valid.
        let now_ns = 500;
        assert_eq!(ttl_check(&cmd, now_ns), TtlStatus::Valid);
    }
}
