//! Control gateway — the **only** path that can turn an envelope into a write
//! command that reaches a device.
//!
//! The gateway is default-deny: every check (decode, HMAC, expiry, future
//! timestamp, replay, structural lease/policy, lease revocation) must pass for
//! an [`AuthorizedCommand`] to be produced. A single failure returns a
//! [`GatewayError`] and **no** write is issued. Protocol adapters that receive
//! write requests over the wire must forward them to
//! [`ControlGateway::authorize`]; they are forbidden from writing to devices
//! directly.
//!
//! # Lease / fencing (CTL-004 structural + CTL-006 revocation)
//!
//! Every command must ride on a lease: `fencing_token` must be non-zero and
//! `policy_version` non-zero, else the gateway hard-denies with
//! [`GatewayError::NoLease`] / [`GatewayError::NoPolicy`]. Revocation is
//! delegated to an injectable [`LeaseChecker`]; the default
//! [`StructuralLeaseChecker`] only enforces the structural rule. CTL-006
//! supplies a real [`LeaseService`](crate::lease::LeaseService) that makes
//! revoke take effect immediately and rejects stale fencing tokens.
//!
//! # Replay protection
//!
//! Every successfully authorized `command_id` is recorded in a bounded replay
//! cache. A second envelope with the same `command_id` is rejected as
//! [`GatewayError::Replayed`], even if its HMAC and expiry are valid. The cache
//! is a ring (FIFO eviction at `cap`) so memory stays bounded.
//!
//! # Time model
//!
//! `now` (epoch seconds) is injected by the caller so the gateway is testable
//! without a real clock. `max_skew_secs` bounds how far a command's issue
//! timestamp may be in the future.

use alloc::boxed::Box;
use alloc::vec::Vec;

// 蓝图/24 D4：`AuthorizedCommand`/`ActorKind` 下移至共享模型层
// （telemetry-model::command），本处 re-export 保持既有导入路径
// （`eneros_control_gateway::AuthorizedCommand` / `crate::envelope::ActorKind`）。
pub use eneros_telemetry_model::command::AuthorizedCommand;

use crate::envelope::{CommandEnvelope, EnvelopeError};

/// Lease revocation status, computed by a [`LeaseChecker`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseStatus {
    /// Lease is active and `fencing_token` is the current one.
    Valid,
    /// Lease was revoked; the command must not execute.
    Revoked,
    /// `fencing_token` is older than the current token for this lease
    /// (non-monotonic / replay of an old lease).
    Stale,
    /// Lease id is not known to the checker.
    Unknown,
}

/// Lease revocation checker. CTL-006 provides the real implementation backed by
/// a [`LeaseService`](crate::lease::LeaseService); the default
/// [`StructuralLeaseChecker`] only enforces that a non-zero fencing token is
/// present (structural), which is already checked before this is consulted.
pub trait LeaseChecker {
    fn check(&self, lease_id: &[u8; 16], fencing_token: u64, now: u64) -> LeaseStatus;
}

/// Default checker: accepts any structurally-valid lease (fencing_token > 0).
/// Used until CTL-006's `LeaseService` is wired in.
pub struct StructuralLeaseChecker;

impl LeaseChecker for StructuralLeaseChecker {
    fn check(&self, _lease_id: &[u8; 16], fencing_token: u64, _now: u64) -> LeaseStatus {
        if fencing_token == 0 {
            LeaseStatus::Stale
        } else {
            LeaseStatus::Valid
        }
    }
}

/// Errors raised by the gateway. Every variant is a hard deny: no write is
/// issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayError {
    Decode(EnvelopeError),
    Unauthorized,
    Expired,
    Future,
    Replayed,
    /// `fencing_token == 0`: the command carries no lease.
    NoLease,
    /// `policy_version == 0`: the command was issued without a policy generation.
    NoPolicy,
    /// Lease was revoked by CTL-006's `LeaseService`.
    LeaseRevoked,
    /// `fencing_token` is stale (older than the current token for the lease).
    StaleFencing,
    /// Wire `device_id` (u32) does not fit the typed `DeviceId` (u16) range
    /// (蓝图/24 D4 类型化；签发方配置错误，default-deny).
    BadDeviceId,
}

impl core::fmt::Display for GatewayError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GatewayError::Decode(e) => write!(f, "gateway decode error: {}", e),
            GatewayError::Unauthorized => write!(f, "gateway unauthorized (HMAC mismatch)"),
            GatewayError::Expired => write!(f, "gateway expired"),
            GatewayError::Future => write!(f, "gateway future-dated command"),
            GatewayError::Replayed => write!(f, "gateway replayed command_id"),
            GatewayError::NoLease => write!(f, "gateway no lease (fencing_token=0)"),
            GatewayError::NoPolicy => write!(f, "gateway no policy version"),
            GatewayError::LeaseRevoked => write!(f, "gateway lease revoked"),
            GatewayError::StaleFencing => write!(f, "gateway stale fencing token"),
            GatewayError::BadDeviceId => write!(f, "gateway device_id out of u16 range"),
        }
    }
}

/// The control gateway. Owns the shared HMAC key, a bounded replay cache, and a
/// lease revocation checker.
pub struct ControlGateway {
    key: Vec<u8>,
    max_skew_secs: u64,
    replay_cache: Vec<[u8; 16]>,
    replay_cache_cap: usize,
    lease_checker: Box<dyn LeaseChecker>,
}

impl ControlGateway {
    /// Creates a new gateway with the structural lease checker (no revocation
    /// tracking). Use [`ControlGateway::new_with_lease_checker`] to wire
    /// CTL-006's `LeaseService`.
    pub fn new(key: &[u8], max_skew_secs: u64, replay_cache_cap: usize) -> Self {
        Self::new_with_lease_checker(
            key,
            max_skew_secs,
            replay_cache_cap,
            Box::new(StructuralLeaseChecker),
        )
    }

    /// Creates a new gateway with a custom lease checker.
    pub fn new_with_lease_checker(
        key: &[u8],
        max_skew_secs: u64,
        replay_cache_cap: usize,
        lease_checker: Box<dyn LeaseChecker>,
    ) -> Self {
        let mut gw = Self {
            key: Vec::new(),
            max_skew_secs,
            replay_cache: Vec::new(),
            replay_cache_cap,
            lease_checker,
        };
        gw.key.extend_from_slice(key);
        gw
    }

    fn is_replayed(&self, command_id: &[u8; 16]) -> bool {
        self.replay_cache.iter().any(|c| c == command_id)
    }

    fn record(&mut self, command_id: [u8; 16]) {
        if self.replay_cache_cap == 0 {
            return;
        }
        if self.replay_cache.len() >= self.replay_cache_cap {
            self.replay_cache.remove(0);
        }
        self.replay_cache.push(command_id);
    }

    /// Authorizes a write command from raw envelope bytes.
    ///
    /// This is the **only** way to obtain an [`AuthorizedCommand`]. Every
    /// failure path is a hard deny (no write issued).
    pub fn authorize(&mut self, buf: &[u8], now: u64) -> Result<AuthorizedCommand, GatewayError> {
        // 1. Decode (length-bounded).
        let env = CommandEnvelope::decode(buf).map_err(GatewayError::Decode)?;

        // 2. Verify HMAC (default deny on mismatch).
        if !env.verify_hmac(&self.key) {
            return Err(GatewayError::Unauthorized);
        }

        // 3. Expiry.
        if now > env.expiry {
            return Err(GatewayError::Expired);
        }

        // 4. Future skew.
        if env.timestamp > now.saturating_add(self.max_skew_secs) {
            return Err(GatewayError::Future);
        }

        // 5. Replay.
        if self.is_replayed(&env.command_id) {
            return Err(GatewayError::Replayed);
        }

        // 6. Structural lease / policy (CTL-004).
        if env.fencing_token == 0 {
            return Err(GatewayError::NoLease);
        }
        if env.policy_version == 0 {
            return Err(GatewayError::NoPolicy);
        }

        // 7. Lease revocation / fencing monotonicity (CTL-006 via checker).
        match self
            .lease_checker
            .check(&env.lease_id, env.fencing_token, now)
        {
            LeaseStatus::Valid => {}
            LeaseStatus::Revoked => return Err(GatewayError::LeaseRevoked),
            LeaseStatus::Stale => return Err(GatewayError::StaleFencing),
            LeaseStatus::Unknown => return Err(GatewayError::LeaseRevoked),
        }

        // All checks passed — record and authorize.
        self.record(env.command_id);
        // D4：wire 上 device_id 仍为 u32（4 字节，布局不动）；类型化为
        // upa-model DeviceId(u16)。越界（签名合法但 id 超出 u16 范围）属
        // 签发方配置错误，default-deny。
        let device_id = u16::try_from(env.device_id).map_err(|_| GatewayError::BadDeviceId)?;
        // WP-26.4：构造经 from_parts（字段私有；本处为生产唯一合法构造方）。
        Ok(AuthorizedCommand::from_parts(
            device_id,
            env.point_id,
            env.actor_kind,
            env.issuer,
            env.correlation_id,
            env.sequence,
            env.lease_id,
            env.fencing_token,
            env.policy_version,
            env.idempotency_key,
            env.value,
            env.command_digest(),
        ))
    }

    /// Number of entries currently held in the replay cache.
    pub fn replay_cache_len(&self) -> usize {
        self.replay_cache.len()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use eneros_telemetry_model::command::{ControlCommand, DoubleCommand, SingleCommand};

    use super::*;
    use crate::envelope::{ActorKind, EnvelopeBuilder};

    const KEY: &[u8] = b"gateway-test-key-xxxxxxxxxxxxxx";
    fn now() -> u64 {
        1_700_000_000
    }

    /// Build a fully-valid signed envelope buffer (lease + policy set).
    fn signed_buf(
        id: [u8; 16],
        device: u32,
        point: u32,
        cmd: ControlCommand,
        ts: u64,
        expiry: u64,
    ) -> [u8; 181] {
        EnvelopeBuilder::new(id, device, point, cmd, ts, expiry, [0xCD; 16])
            .lease([3u8; 16], 7)
            .policy_version(3)
            .config_version(11)
            .sequence(1)
            .actor_kind(ActorKind::Agent)
            .idempotency_key([4u8; 16])
            .build(KEY)
            .encode()
    }

    #[test]
    fn test_authorize_accepts_valid_envelope() {
        let mut gw = ControlGateway::new(KEY, 5, 64);
        let buf = signed_buf(
            [1u8; 16],
            42,
            7,
            ControlCommand::Single(SingleCommand::On),
            now(),
            now() + 30,
        );
        let cmd = gw.authorize(&buf, now()).unwrap();
        assert_eq!(cmd.device_id(), 42);
        assert_eq!(cmd.point_id(), 7);
        assert_eq!(cmd.issuer(), [0xCD; 16]);
        assert_eq!(cmd.fencing_token(), 7);
        assert_eq!(cmd.policy_version(), 3);
        assert_eq!(cmd.actor_kind(), ActorKind::Agent);
    }

    #[test]
    fn test_authorize_denies_bad_length() {
        let mut gw = ControlGateway::new(KEY, 5, 64);
        let err = gw.authorize(&[0u8; 10], now()).unwrap_err();
        assert!(matches!(
            err,
            GatewayError::Decode(EnvelopeError::BadLength { .. })
        ));
    }

    #[test]
    fn test_authorize_denies_tampered_hmac() {
        let mut gw = ControlGateway::new(KEY, 5, 64);
        let mut buf = signed_buf(
            [2u8; 16],
            1,
            1,
            ControlCommand::Single(SingleCommand::On),
            now(),
            now() + 30,
        );
        buf[41] ^= 0x01; // device_id byte (covered by HMAC)
        assert_eq!(
            gw.authorize(&buf, now()).unwrap_err(),
            GatewayError::Unauthorized
        );
    }

    #[test]
    fn test_authorize_denies_wrong_key() {
        let mut gw = ControlGateway::new(b"different-key-xxxxxxxxxxxxxxxx", 5, 64);
        let buf = signed_buf(
            [3u8; 16],
            1,
            1,
            ControlCommand::Single(SingleCommand::On),
            now(),
            now() + 30,
        );
        assert_eq!(
            gw.authorize(&buf, now()).unwrap_err(),
            GatewayError::Unauthorized
        );
    }

    #[test]
    fn test_authorize_denies_expired() {
        let mut gw = ControlGateway::new(KEY, 5, 64);
        let buf = signed_buf(
            [4u8; 16],
            1,
            1,
            ControlCommand::Single(SingleCommand::On),
            now() - 100,
            now() - 10,
        );
        assert_eq!(
            gw.authorize(&buf, now()).unwrap_err(),
            GatewayError::Expired
        );
    }

    #[test]
    fn test_authorize_denies_future_dated() {
        let mut gw = ControlGateway::new(KEY, 5, 64);
        let buf = signed_buf(
            [5u8; 16],
            1,
            1,
            ControlCommand::Single(SingleCommand::On),
            now() + 1000,
            now() + 2000,
        );
        assert_eq!(gw.authorize(&buf, now()).unwrap_err(), GatewayError::Future);
    }

    #[test]
    fn test_authorize_accepts_within_skew() {
        let mut gw = ControlGateway::new(KEY, 10, 64);
        let buf = signed_buf(
            [6u8; 16],
            1,
            1,
            ControlCommand::Single(SingleCommand::On),
            now() + 8,
            now() + 60,
        );
        assert!(gw.authorize(&buf, now()).is_ok());
    }

    #[test]
    fn test_authorize_denies_replay() {
        let mut gw = ControlGateway::new(KEY, 5, 64);
        let buf = signed_buf(
            [7u8; 16],
            1,
            1,
            ControlCommand::Single(SingleCommand::On),
            now(),
            now() + 30,
        );
        assert!(gw.authorize(&buf, now()).is_ok());
        assert_eq!(
            gw.authorize(&buf, now()).unwrap_err(),
            GatewayError::Replayed
        );
    }

    #[test]
    fn test_replay_cache_is_bounded() {
        let mut gw = ControlGateway::new(KEY, 5, 4);
        for i in 0u8..4 {
            let mut id = [0u8; 16];
            id[0] = i;
            let buf = signed_buf(
                id,
                1,
                1,
                ControlCommand::Single(SingleCommand::On),
                now(),
                now() + 30,
            );
            assert!(gw.authorize(&buf, now()).is_ok());
        }
        assert_eq!(gw.replay_cache_len(), 4);
        let mut id5 = [0u8; 16];
        id5[0] = 4;
        let buf5 = signed_buf(
            id5,
            1,
            1,
            ControlCommand::Single(SingleCommand::On),
            now(),
            now() + 30,
        );
        assert!(gw.authorize(&buf5, now()).is_ok());
        assert_eq!(gw.replay_cache_len(), 4);
        let mut id0 = [0u8; 16];
        id0[0] = 0;
        let buf0 = signed_buf(
            id0,
            1,
            1,
            ControlCommand::Single(SingleCommand::On),
            now(),
            now() + 30,
        );
        assert!(gw.authorize(&buf0, now()).is_ok());
    }

    // ---- CTL-004 structural lease/policy enforcement ----

    #[test]
    fn test_authorize_denies_no_lease() {
        let mut gw = ControlGateway::new(KEY, 5, 64);
        // Envelope with fencing_token == 0 (no .lease(...) call).
        let buf = EnvelopeBuilder::new(
            [8u8; 16],
            1,
            1,
            ControlCommand::Single(SingleCommand::On),
            now(),
            now() + 30,
            [0xCD; 16],
        )
        .policy_version(3)
        .build(KEY)
        .encode();
        assert_eq!(
            gw.authorize(&buf, now()).unwrap_err(),
            GatewayError::NoLease
        );
    }

    #[test]
    fn test_authorize_denies_no_policy() {
        let mut gw = ControlGateway::new(KEY, 5, 64);
        let buf = EnvelopeBuilder::new(
            [9u8; 16],
            1,
            1,
            ControlCommand::Single(SingleCommand::On),
            now(),
            now() + 30,
            [0xCD; 16],
        )
        .lease([3u8; 16], 7)
        .build(KEY)
        .encode();
        assert_eq!(
            gw.authorize(&buf, now()).unwrap_err(),
            GatewayError::NoPolicy
        );
    }

    // ---- CTL-006 lease revocation via injected checker ----

    /// Checker that revokes a specific lease id.
    struct RevokeSpecific([u8; 16]);
    impl LeaseChecker for RevokeSpecific {
        fn check(&self, lease_id: &[u8; 16], fencing_token: u64, _now: u64) -> LeaseStatus {
            if lease_id == &self.0 {
                LeaseStatus::Revoked
            } else if fencing_token == 0 {
                LeaseStatus::Stale
            } else {
                LeaseStatus::Valid
            }
        }
    }

    #[test]
    fn test_authorize_denies_revoked_lease() {
        let revoked = [3u8; 16];
        let mut gw =
            ControlGateway::new_with_lease_checker(KEY, 5, 64, Box::new(RevokeSpecific(revoked)));
        let buf = signed_buf(
            [10u8; 16],
            1,
            1,
            ControlCommand::Double(DoubleCommand::On),
            now(),
            now() + 30,
        );
        assert_eq!(
            gw.authorize(&buf, now()).unwrap_err(),
            GatewayError::LeaseRevoked
        );
    }

    /// Checker that enforces fencing monotonicity: token must equal current.
    struct MonotonicFencing {
        current: u64,
    }
    impl LeaseChecker for MonotonicFencing {
        fn check(&self, _lease_id: &[u8; 16], fencing_token: u64, _now: u64) -> LeaseStatus {
            if fencing_token < self.current {
                LeaseStatus::Stale
            } else {
                LeaseStatus::Valid
            }
        }
    }

    #[test]
    fn test_authorize_denies_stale_fencing() {
        let mut gw = ControlGateway::new_with_lease_checker(
            KEY,
            5,
            64,
            Box::new(MonotonicFencing { current: 100 }),
        );
        // fencing_token = 7 < 100 → stale.
        let buf = signed_buf(
            [11u8; 16],
            1,
            1,
            ControlCommand::Single(SingleCommand::On),
            now(),
            now() + 30,
        );
        assert_eq!(
            gw.authorize(&buf, now()).unwrap_err(),
            GatewayError::StaleFencing
        );
    }

    #[test]
    fn test_default_deny_no_authorized_command_on_failure() {
        let mut gw = ControlGateway::new(KEY, 5, 64);
        let err = gw.authorize(&[0u8; 5], now()).unwrap_err();
        assert_eq!(
            err,
            GatewayError::Decode(EnvelopeError::BadLength { got: 5 })
        );
    }
}
