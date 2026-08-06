//! Idempotency + select-before-execute (CTL-007).
//!
//! Two complementary guards sit between the gateway and the device:
//!
//! - [`IdempotencyCache`] — if a command carries an `idempotency_key` that was
//!   already applied within its TTL, the retransmit is **not** re-executed; the
//!   prior outcome is returned instead. This makes retries safe under exactly-
//!   once semantics at the device.
//! - [`SboTracker`] — select-before-operate: a discrete command may execute
//!   only if the same point was *selected* within a short window and the
//!   execute matches the select. This blocks blind/forced writes.
//!
//! Both are independent of the gateway's HMAC/lease checks and default to the
//! safe side (deny / re-execute nothing).

use alloc::collections::BTreeMap;

/// Outcome of an idempotent operation, cached for retransmits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpOutcome {
    /// The operation was applied (or already had been).
    Applied,
    /// The operation was rejected downstream (safety/policy).
    Rejected,
}

/// A bounded idempotency cache keyed by `idempotency_key`. Entries expire after
/// `ttl_secs`. A zero key (`[0;16]`) means "no idempotency requested" and is
/// never cached.
pub struct IdempotencyCache {
    ttl_secs: u64,
    /// key → (outcome, expiry_epoch)
    entries: BTreeMap<[u8; 16], (OpOutcome, u64)>,
    cap: usize,
}

impl IdempotencyCache {
    pub fn new(ttl_secs: u64, cap: usize) -> Self {
        Self {
            ttl_secs,
            entries: BTreeMap::new(),
            cap,
        }
    }

    /// If `key` is a known, non-expired hit, return the cached outcome without
    /// re-executing. Otherwise return `None` (caller should execute, then call
    /// [`record`](Self::record)).
    pub fn check(&self, key: &[u8; 16], now: u64) -> Option<OpOutcome> {
        if key == &[0u8; 16] {
            return None;
        }
        let (outcome, expiry) = self.entries.get(key)?;
        if now > *expiry {
            None
        } else {
            Some(*outcome)
        }
    }

    /// Record the outcome of an operation so retransmits are deduped.
    pub fn record(&mut self, key: [u8; 16], outcome: OpOutcome, now: u64) {
        if key == [0u8; 16] {
            return;
        }
        if self.entries.len() >= self.cap && !self.entries.contains_key(&key) {
            // Evict the entry with the smallest key (deterministic, bounded).
            if let Some(&k) = self.entries.keys().next() {
                self.entries.remove(&k);
            }
        }
        self.entries
            .insert(key, (outcome, now.saturating_add(self.ttl_secs)));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Select-before-operate state for a single point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SboState {
    Idle,
    /// Selected at `select_epoch`, expires at `expiry_epoch`.
    Selected {
        select_epoch: u64,
        expiry_epoch: u64,
    },
    /// Executed (terminal until reset).
    Done,
}

/// SBO tracker. Enforces that a discrete command on a point may execute only if
/// that point was selected within `select_ttl_secs` and the execute arrives
/// before expiry.
pub struct SboTracker {
    select_ttl_secs: u64,
    states: BTreeMap<u32, SboState>,
}

/// Result of an execute attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SboExecuteResult {
    /// Was selected & valid → executed.
    Executed,
    /// No prior select for this point.
    NotSelected,
    /// Select existed but expired.
    SelectExpired,
}

impl SboTracker {
    pub fn new(select_ttl_secs: u64) -> Self {
        Self {
            select_ttl_secs,
            states: BTreeMap::new(),
        }
    }

    /// Select a point. Resets any prior select.
    pub fn select(&mut self, point_id: u32, now: u64) {
        self.states.insert(
            point_id,
            SboState::Selected {
                select_epoch: now,
                expiry_epoch: now.saturating_add(self.select_ttl_secs),
            },
        );
    }

    /// Attempt to execute on a point. Returns `Executed` only if a non-expired
    /// select exists; transitions to `Done`.
    pub fn execute(&mut self, point_id: u32, now: u64) -> SboExecuteResult {
        match self.states.get(&point_id) {
            None | Some(SboState::Idle) => SboExecuteResult::NotSelected,
            Some(SboState::Done) => SboExecuteResult::NotSelected,
            Some(SboState::Selected { expiry_epoch, .. }) => {
                if now > *expiry_epoch {
                    SboExecuteResult::SelectExpired
                } else {
                    self.states.insert(point_id, SboState::Done);
                    SboExecuteResult::Executed
                }
            }
        }
    }

    /// Cancel a select (e.g. on disengagement).
    pub fn cancel(&mut self, point_id: u32) {
        self.states.insert(point_id, SboState::Idle);
    }

    pub fn state(&self, point_id: u32) -> SboState {
        self.states
            .get(&point_id)
            .copied()
            .unwrap_or(SboState::Idle)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    #[test]
    fn test_idempotency_dedups_retransmit() {
        let mut cache = IdempotencyCache::new(60, 16);
        let key = [1u8; 16];
        assert!(cache.check(&key, 1_000).is_none()); // first time
        cache.record(key, OpOutcome::Applied, 1_000);
        // Retransmit within TTL → cached outcome, no re-execution.
        assert_eq!(cache.check(&key, 1_010), Some(OpOutcome::Applied));
    }

    #[test]
    fn test_idempotency_expires() {
        let mut cache = IdempotencyCache::new(60, 16);
        let key = [2u8; 16];
        cache.record(key, OpOutcome::Applied, 1_000);
        assert!(cache.check(&key, 1_061).is_none());
    }

    #[test]
    fn test_idempotency_zero_key_never_cached() {
        let mut cache = IdempotencyCache::new(60, 16);
        cache.record([0u8; 16], OpOutcome::Applied, 1_000);
        assert!(cache.check(&[0u8; 16], 1_000).is_none());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_idempotency_bounded() {
        let mut cache = IdempotencyCache::new(60, 2);
        cache.record([1u8; 16], OpOutcome::Applied, 1_000);
        cache.record([2u8; 16], OpOutcome::Applied, 1_000);
        cache.record([3u8; 16], OpOutcome::Applied, 1_000); // evicts smallest key
        assert_eq!(cache.len(), 2);
        assert!(cache.check(&[1u8; 16], 1_000).is_none());
    }

    #[test]
    fn test_sbo_select_then_execute() {
        let mut sbo = SboTracker::new(10);
        assert_eq!(sbo.execute(10, 1_000), SboExecuteResult::NotSelected);
        sbo.select(10, 1_000);
        assert_eq!(sbo.execute(10, 1_005), SboExecuteResult::Executed);
        assert_eq!(sbo.state(10), SboState::Done);
        // Re-execute after Done is NotSelected (must re-select).
        assert_eq!(sbo.execute(10, 1_006), SboExecuteResult::NotSelected);
    }

    #[test]
    fn test_sbo_execute_without_select_rejected() {
        let mut sbo = SboTracker::new(10);
        assert_eq!(sbo.execute(20, 1_000), SboExecuteResult::NotSelected);
    }

    #[test]
    fn test_sbo_select_expiry() {
        let mut sbo = SboTracker::new(10);
        sbo.select(30, 1_000);
        assert_eq!(sbo.execute(30, 1_011), SboExecuteResult::SelectExpired);
    }

    #[test]
    fn test_sbo_cancel() {
        let mut sbo = SboTracker::new(10);
        sbo.select(40, 1_000);
        sbo.cancel(40);
        assert_eq!(sbo.state(40), SboState::Idle);
        assert_eq!(sbo.execute(40, 1_005), SboExecuteResult::NotSelected);
    }
}
