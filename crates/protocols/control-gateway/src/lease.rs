//! Lease / fencing token service (CTL-006).
//!
//! Every write command must ride on a lease ([`crate::envelope::CommandEnvelope`]
//! carries `lease_id` + `fencing_token`). A lease bounds **who** may write
//! **what** for **how long**. Revoking a lease must take effect **immediately**:
//! a running Agent that held the lease loses access at once, and an old
//! `fencing_token` can never authorize a write again.
//!
//! # Fencing model
//!
//! - Each lease gets a monotonically-increasing `fencing_token` drawn from a
//!   global epoch counter.
//! - `revoke(lease_id)` tombstones the lease: any subsequent `check` for that
//!   `lease_id` returns [`LeaseStatus::Revoked`], regardless of the token. The
//!   lease can **never** become valid again — a new grant always mints a fresh
//!   `lease_id` with a higher token.
//! - A command presenting an old token for a still-active lease is [`Stale`]
//!   (token < current), which the gateway maps to
//!   [`GatewayError::StaleFencing`](crate::gateway::GatewayError::StaleFencing).
//!
//! [`Stale`]: crate::gateway::LeaseStatus::Stale

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::gateway::{LeaseChecker, LeaseStatus};

/// What a lease authorizes. The gateway does not enforce scope itself; the PDP
/// (CTL-005) consults this when deciding whether a command is within the lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseScope {
    /// Authorized device ids (empty = none).
    pub devices: Vec<u32>,
    /// Authorized point ids (empty = none).
    pub points: Vec<u32>,
    /// Maximum command magnitude / class hint (free-form, enforced by PDP).
    pub max_class: u8,
}

/// An active or historical lease grant.
#[derive(Debug, Clone)]
struct LeaseGrant {
    token: u64,
    expiry: u64,
    scope: LeaseScope,
}

/// The lease service. Owns the monotonic fencing epoch, active grants and the
/// revocation tombstone set. Implements [`LeaseChecker`] so it can be wired
/// into [`ControlGateway::new_with_lease_checker`](crate::gateway::ControlGateway::new_with_lease_checker).
pub struct LeaseService {
    /// Monotonic fencing epoch; every grant mints `epoch+1` and advances it.
    epoch: u64,
    grants: BTreeMap<[u8; 16], LeaseGrant>,
    /// Tombstones: lease ids that have been revoked and can never be reused.
    revoked: Vec<[u8; 16]>,
}

impl Default for LeaseService {
    fn default() -> Self {
        Self::new()
    }
}

impl LeaseService {
    pub fn new() -> Self {
        Self {
            epoch: 0,
            grants: BTreeMap::new(),
            revoked: Vec::new(),
        }
    }

    /// 以平台重启计数为种子设定 fencing epoch（蓝图/24 D7：跨重启单调，
    /// 重启不能洗白旧命令；种子必须来自单调重启计数而非墙钟）。
    ///
    /// 仅允许在**签发任何租约之前**的启动期调用；事后调用会破坏已签发
    /// token 的单调性（调用方违反时行为未定义，本服务不校验）。
    pub fn seed_epoch(&mut self, seed: u64) {
        self.epoch = seed;
    }

    /// 读取指定租约的当前 fencing token（`None` = 租约未知/已吊销/已过期）。
    pub fn token_of(&self, lease_id: &[u8; 16]) -> Option<u64> {
        self.grants.get(lease_id).map(|g| g.token)
    }

    /// Issue a new lease. Returns the `(lease_id, fencing_token, expiry)` the
    /// caller must embed in every command envelope that rides on this lease.
    /// `lease_id` must be fresh (not previously revoked); if it collides with a
    /// tombstone, issuance is refused (`None`).
    pub fn issue(
        &mut self,
        lease_id: [u8; 16],
        scope: LeaseScope,
        ttl_secs: u64,
        now: u64,
    ) -> Option<(u64, u64)> {
        if self.revoked.contains(&lease_id) {
            // A revoked lease_id can never be re-granted.
            return None;
        }
        if self.grants.contains_key(&lease_id) {
            // Already active — refuse to silently reissue.
            return None;
        }
        self.epoch = self.epoch.checked_add(1)?;
        let token = self.epoch;
        let expiry = now.saturating_add(ttl_secs);
        self.grants.insert(
            lease_id,
            LeaseGrant {
                token,
                expiry,
                scope,
            },
        );
        Some((token, expiry))
    }

    /// Revoke a lease **immediately**. After this, `check` for `lease_id`
    /// always returns [`LeaseStatus::Revoked`]; the lease can never be valid
    /// again. Returns `true` if the lease was active (now revoked), `false` if
    /// it was already revoked or never existed.
    pub fn revoke(&mut self, lease_id: &[u8; 16]) -> bool {
        let was_active = self.grants.remove(lease_id).is_some();
        if !self.revoked.iter().any(|r| r == lease_id) {
            self.revoked.push(*lease_id);
        }
        was_active
    }

    /// Look up the scope of an active lease (for the PDP). `None` if the lease
    /// is revoked, expired or unknown.
    pub fn scope(&self, lease_id: &[u8; 16], now: u64) -> Option<&LeaseScope> {
        if self.revoked.iter().any(|r| r == lease_id) {
            return None;
        }
        self.grants
            .get(lease_id)
            .and_then(|g| if now > g.expiry { None } else { Some(&g.scope) })
    }

    /// Number of currently-active (non-revoked, non-expired-checked-on-issue) grants.
    pub fn active_count(&self) -> usize {
        self.grants.len()
    }
}

impl LeaseChecker for LeaseService {
    fn check(&self, lease_id: &[u8; 16], fencing_token: u64, now: u64) -> LeaseStatus {
        // 1. Tombstone: revoked leases are dead forever.
        if self.revoked.iter().any(|r| r == lease_id) {
            return LeaseStatus::Revoked;
        }
        match self.grants.get(lease_id) {
            None => LeaseStatus::Unknown,
            Some(g) => {
                // 2. Expiry: an expired lease is no longer valid.
                if now > g.expiry {
                    return LeaseStatus::Revoked;
                }
                // 3. Fencing monotonicity: token must equal the current grant.
                if fencing_token < g.token {
                    LeaseStatus::Stale
                } else if fencing_token == g.token {
                    LeaseStatus::Valid
                } else {
                    // A token higher than current is impossible for a genuine
                    // grant (tokens come from our monotonic epoch); treat as
                    // stale/unknown → deny.
                    LeaseStatus::Stale
                }
            }
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;
    use crate::gateway::{LeaseChecker, LeaseStatus};

    fn scope() -> LeaseScope {
        LeaseScope {
            devices: vec![1, 2],
            points: vec![10, 20],
            max_class: 1,
        }
    }

    #[test]
    fn test_issue_and_check_valid() {
        let mut svc = LeaseService::new();
        let id = [1u8; 16];
        let (token, _exp) = svc.issue(id, scope(), 60, 1_000).unwrap();
        assert_eq!(svc.check(&id, token, 1_000), LeaseStatus::Valid);
    }

    #[test]
    fn test_revoke_invalidates_active_lease() {
        // SR-005 trace target: revoke must immediately invalidate; old token
        // can never authorize again.
        let mut svc = LeaseService::new();
        let id = [2u8; 16];
        let (token, _exp) = svc.issue(id, scope(), 60, 1_000).unwrap();
        assert_eq!(svc.check(&id, token, 1_000), LeaseStatus::Valid);

        // Revoke → immediately invalid.
        assert!(svc.revoke(&id));
        assert_eq!(svc.check(&id, token, 1_000), LeaseStatus::Revoked);

        // The SAME old token (replayed) is still revoked — not stale, not valid.
        assert_eq!(svc.check(&id, token, 1_001), LeaseStatus::Revoked);
    }

    #[test]
    fn test_revoked_lease_can_never_be_reissued() {
        let mut svc = LeaseService::new();
        let id = [3u8; 16];
        let _ = svc.issue(id, scope(), 60, 1_000).unwrap();
        svc.revoke(&id);
        // Re-issuing the same lease_id is refused.
        assert!(svc.issue(id, scope(), 60, 1_000).is_none());
        // And check still says revoked.
        assert_eq!(svc.check(&id, 999, 1_000), LeaseStatus::Revoked);
    }

    #[test]
    fn test_new_lease_gets_higher_token_than_revoked() {
        let mut svc = LeaseService::new();
        let id_a = [4u8; 16];
        let id_b = [5u8; 16];
        let (tok_a, _) = svc.issue(id_a, scope(), 60, 1_000).unwrap();
        svc.revoke(&id_a);
        let (tok_b, _) = svc.issue(id_b, scope(), 60, 1_000).unwrap();
        assert!(tok_b > tok_a, "new token must fence off the old");
        // Old lease_id stays revoked.
        assert_eq!(svc.check(&id_a, tok_a, 1_000), LeaseStatus::Revoked);
        // New lease is valid with its own token.
        assert_eq!(svc.check(&id_b, tok_b, 1_000), LeaseStatus::Valid);
        // A command trying to use the new lease with the OLD token is stale.
        assert_eq!(svc.check(&id_b, tok_a, 1_000), LeaseStatus::Stale);
    }

    #[test]
    fn test_expired_lease_is_revoked() {
        let mut svc = LeaseService::new();
        let id = [6u8; 16];
        let (token, _) = svc.issue(id, scope(), 60, 1_000).unwrap();
        assert_eq!(svc.check(&id, token, 1_000 + 61), LeaseStatus::Revoked);
    }

    #[test]
    fn test_unknown_lease_is_unknown() {
        let svc = LeaseService::new();
        assert_eq!(svc.check(&[9u8; 16], 1, 1_000), LeaseStatus::Unknown);
    }

    #[test]
    fn test_stale_token_for_active_lease_is_stale() {
        let mut svc = LeaseService::new();
        let id = [7u8; 16];
        let (token, _) = svc.issue(id, scope(), 60, 1_000).unwrap();
        // A token lower than current → stale (e.g. replay of a pre-rotation token).
        assert_eq!(svc.check(&id, token - 1, 1_000), LeaseStatus::Stale);
    }

    #[test]
    fn test_scope_lookup_reflects_revoke_and_expiry() {
        let mut svc = LeaseService::new();
        let id = [8u8; 16];
        let _ = svc.issue(id, scope(), 60, 1_000).unwrap();
        assert!(svc.scope(&id, 1_000).is_some());
        svc.revoke(&id);
        assert!(svc.scope(&id, 1_000).is_none());
    }

    #[test]
    fn test_revoke_idempotent() {
        let mut svc = LeaseService::new();
        let id = [10u8; 16];
        let _ = svc.issue(id, scope(), 60, 1_000).unwrap();
        assert!(svc.revoke(&id)); // was active
        assert!(!svc.revoke(&id)); // already revoked → false
    }
}
