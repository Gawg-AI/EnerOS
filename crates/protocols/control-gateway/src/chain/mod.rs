//! Control chain — the complete deterministic safety path from envelope bytes
//! to a device-bound command (CTL-005/006/007/008/009/010 integration).
//!
//! [`ControlChain`] wires every CTL component into a single default-deny path:
//!
//! 1. **Gateway** (CTL-003/004) — decode + HMAC + expiry + future-skew + replay
//!    + structural lease/policy.
//! 2. **Lease revocation** (CTL-006) — real tombstone check via
//!    [`LeaseService`]: a revoked lease denies immediately, an old fencing
//!    token is stale.
//! 3. **Idempotency** (CTL-007) — a retransmit with a known
//!    `idempotency_key` returns the prior outcome without re-execution
//!    (exactly-once).
//! 4. **Override authority** (CTL-009) — an active emergency session bypasses
//!    the PDP for emergency actors and supersedes non-emergency commands on
//!    the overridden point.
//! 5. **Policy PDP** (CTL-005) — semantic authorization, default-deny.
//! 6. **Select-before-execute** (CTL-007) — a discrete command may execute
//!    only if the point was selected within the SBO window.
//! 7. **Feedback reconciliation** (CTL-010) — telemetry samples are checked
//!    for freshness/quality/range; any untrusted sample drives the plant
//!    context to a fail-safe value that denies dangerous actions.
//! 8. **Independent safety validator** (CTL-008) — recomputes physical safety
//!    from the reconciled context via its own point table.
//!
//! Every step is a hard deny on failure; no write reaches the device unless
//! all pass. The chain is the single integration point that turns the
//! individual CTL components into the MASTER §9 control execution path.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_telemetry_model::quality::QualityFlag;

use crate::gateway::{AuthorizedCommand, ControlGateway, GatewayError, LeaseChecker, LeaseStatus};
use crate::idempotency::{IdempotencyCache, OpOutcome, SboExecuteResult, SboTracker};
use crate::lease::{LeaseScope, LeaseService};
use crate::override_ctrl::{OverrideAuthority, OverrideDecision};
use crate::policy::{Decision, DenyReason, Obligation, Pdp, Policy};
use crate::reconcile::{reconcile, FeedbackSample, Reconciled};
use crate::safety::{PlantContext, SafetyValidator, SafetyVerdict};

/// Why the chain denied a command. Every variant is a hard deny: no write is
/// issued and the idempotency key (if any) is recorded as `Rejected`.
#[derive(Debug, Clone, PartialEq)]
pub enum ChainDenyReason {
    /// Gateway-level failure (decode / HMAC / expiry / future / replay /
    /// structural lease or policy).
    Gateway(GatewayError),
    /// Lease was revoked (CTL-006 tombstone) or unknown.
    LeaseRevoked,
    /// `fencing_token` is older than the current token for this lease.
    StaleFencing,
    /// Non-emergency command for a point under an active override session
    /// (CTL-009): the operator retains control.
    SupersededByOverride,
    /// Policy PDP denied (CTL-005).
    Policy(DenyReason),
    /// Select-before-execute: no prior select for this point (CTL-007).
    NotSelected,
    /// Select-before-execute: the prior select has expired (CTL-007).
    SelectExpired,
    /// Independent safety validator rejected the command (CTL-008).
    Unsafe(String),
    /// A feedback sample was stale / bad-quality / out-of-range (CTL-010) and
    /// the resulting fail-safe context made the command unsafe.
    FeedbackUntrusted(String),
}

/// Outcome of a chain execution.
#[derive(Debug, Clone, PartialEq)]
pub enum ChainOutcome {
    /// All checks passed; the command may be applied to the device. The
    /// caller (device executor) must discharge every [`Obligation`] before
    /// writing.
    Executed {
        cmd: AuthorizedCommand,
        obligations: Vec<Obligation>,
    },
    /// A prior execution with the same `idempotency_key` was found; its
    /// outcome is returned without re-execution (CTL-007 exactly-once).
    IdempotentHit(OpOutcome),
    /// The policy allowed the command as a **dry run** (Obligation::DryRun).
    /// The device executor must NOT write; it should emit what would have
    /// happened. This is the PEP enforcement of the DryRun obligation.
    DryRun {
        cmd: AuthorizedCommand,
        obligations: Vec<Obligation>,
    },
    /// The chain denied the command. No write is issued.
    Denied(ChainDenyReason),
}

/// A bundle of raw telemetry feedback samples used to build the plant context.
/// Each sample is reconciled (CTL-010) before the safety validator (CTL-008)
/// trusts it; an untrusted sample drives its field to a fail-safe value.
#[derive(Debug, Clone, Copy)]
pub struct FeedbackBundle {
    /// Grid status: `1.0` = up, `0.0` = down.
    pub grid_up: FeedbackSample,
    /// Battery state of charge (0..100 %).
    pub soc_pct: FeedbackSample,
    /// DC bus voltage (V). Currently informational.
    pub dc_bus_v: FeedbackSample,
    /// Overcurrent flag: `1.0` = yes, `0.0` = no.
    pub overcurrent: FeedbackSample,
    /// Overtemperature flag: `1.0` = yes, `0.0` = no.
    pub overtemp: FeedbackSample,
    /// Maintenance lockout (direct flag, not reconciled — operator-set).
    pub maintenance_lockout: bool,
}

impl FeedbackBundle {
    /// Convenience: build a bundle of fresh, good-quality, in-range samples.
    pub fn fresh(grid_up: bool, soc_pct: f64, now: u64) -> Self {
        Self {
            grid_up: FeedbackSample {
                value: if grid_up { 1.0 } else { 0.0 },
                quality: QualityFlag::Good,
                timestamp_epoch: now,
            },
            soc_pct: FeedbackSample {
                value: soc_pct,
                quality: QualityFlag::Good,
                timestamp_epoch: now,
            },
            dc_bus_v: FeedbackSample {
                value: 600.0,
                quality: QualityFlag::Good,
                timestamp_epoch: now,
            },
            overcurrent: FeedbackSample {
                value: 0.0,
                quality: QualityFlag::Good,
                timestamp_epoch: now,
            },
            overtemp: FeedbackSample {
                value: 0.0,
                quality: QualityFlag::Good,
                timestamp_epoch: now,
            },
            maintenance_lockout: false,
        }
    }
}

/// The complete control chain. Owns every CTL component.
pub struct ControlChain {
    gateway: ControlGateway,
    lease_service: LeaseService,
    policy: Policy,
    safety: SafetyValidator,
    idempotency: IdempotencyCache,
    sbo: SboTracker,
    override_auth: OverrideAuthority,
    feedback_max_age_secs: u64,
}

impl ControlChain {
    /// Create a new chain with empty lease grants, empty policy rules, empty
    /// safety table and empty override sessions. The caller must:
    /// - issue leases via [`lease_service_mut`](Self::lease_service_mut),
    /// - register safety points via [`safety_mut`](Self::safety_mut),
    /// - add policy rules via [`policy_mut`](Self::policy_mut).
    ///
    /// Until configured, every command is denied (default-deny: unknown lease,
    /// no matching policy rule, unassigned safety point).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        key: &[u8],
        max_skew_secs: u64,
        replay_cache_cap: usize,
        policy: Policy,
        feedback_max_age_secs: u64,
        idempotency_ttl_secs: u64,
        idempotency_cap: usize,
        sbo_select_ttl_secs: u64,
    ) -> Self {
        Self {
            gateway: ControlGateway::new(key, max_skew_secs, replay_cache_cap),
            lease_service: LeaseService::new(),
            policy,
            safety: SafetyValidator::new(),
            idempotency: IdempotencyCache::new(idempotency_ttl_secs, idempotency_cap),
            sbo: SboTracker::new(sbo_select_ttl_secs),
            override_auth: OverrideAuthority::new(),
            feedback_max_age_secs,
        }
    }

    /// Mutable access to the lease service (issue / revoke).
    pub fn lease_service_mut(&mut self) -> &mut LeaseService {
        &mut self.lease_service
    }

    /// Mutable access to the safety validator (register points).
    pub fn safety_mut(&mut self) -> &mut SafetyValidator {
        &mut self.safety
    }

    /// Mutable access to the override authority (activate / deactivate).
    pub fn override_auth_mut(&mut self) -> &mut OverrideAuthority {
        &mut self.override_auth
    }

    /// Mutable access to the policy (replace rules).
    pub fn policy_mut(&mut self) -> &mut Policy {
        &mut self.policy
    }

    /// Pre-select a point for select-before-execute (CTL-007).
    pub fn sbo_select(&mut self, point_id: u32, now: u64) {
        self.sbo.select(point_id, now);
    }
}

/// 执行链大方法（ARCH-G03 拆分）。
mod execute;

#[cfg(test)]
mod tests;
