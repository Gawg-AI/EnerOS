//! Policy Decision Point / Policy Enforcement Point (CTL-005).
//!
//! The gateway already enforces *authenticity* (HMAC), *freshness* (expiry),
//! *ordering* (replay/sequence) and *lease validity* (CTL-006). The PDP applies
//! **semantic authorization**: given an authenticated, lease-bound command, is
//! this *actor* allowed to operate this *point* at this *time* under the
//! current *policy generation*? The PEP then enforces the decision and any
//! obligations (audit log, secondary confirm, rate limit).
//!
//! # Default-deny
//!
//! If no rule explicitly matches, the decision is [`Decision::Deny`]. There is
//! no implicit allow. A stale `policy_version` (older than the current
//! generation) is also a deny — the issuer must re-read policy and retry.
//!
//! # Independence
//!
//! The PDP is a separate code path from the command generator (Agent/AI) and
//! from the gateway's crypto checks. It consumes the already-verified
//! [`AuthorizedCommand`] fields plus the lease [`LeaseScope`], and recomputes
//! the decision from the static [`Policy`].

use alloc::vec::Vec;

use crate::envelope::ActorKind;
use crate::gateway::AuthorizedCommand;
use crate::lease::LeaseScope;
use crate::policy::CommandClass::*;

/// Coarse command class used by policy rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandClass {
    /// Read / interrogate (no state change).
    Read,
    /// Discrete two-state switch (open/close, on/off).
    Switch,
    /// Continuous setpoint (analog value).
    Setpoint,
    /// Emergency / manual override.
    Emergency,
}

impl CommandClass {
    /// Classify a verified authorized command by its target point + command.
    /// Discrete Single/Double commands are `Switch`; the caller may upgrade to
    /// `Emergency` when `actor_kind == Emergency`.
    pub fn from_command(cmd: &AuthorizedCommand) -> Self {
        match cmd.actor_kind {
            ActorKind::Emergency => Emergency,
            _ => Switch, // ControlCommand is discrete in v1; setpoint class added with analog cmds
        }
    }
}

/// A single policy rule. All non-empty match fields must match for the rule to
/// apply; an empty `devices`/`points`/`kinds` means "any".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRule {
    pub actor_kind: Option<ActorKind>,
    pub devices: Vec<u32>,
    pub points: Vec<u32>,
    pub kinds: Vec<CommandClass>,
    /// Allowed only within [time_window.0, time_window.1] (epoch secs). (0,0) = any time.
    pub time_window: (u64, u64),
    pub obligations: Vec<Obligation>,
}

/// Obligations the PEP must discharge when a command is allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Obligation {
    /// Write a tamper-evident audit record before execution.
    AuditBefore,
    /// Require a second human confirm (out-of-band) before execution.
    SecondaryConfirm,
    /// Rate-limit: at most N commands per minute for this actor+point.
    RateLimitPerMinute(u32),
    /// Dry-run only: emit what would happen, do not execute.
    DryRun,
}

/// A policy decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow { obligations: Vec<Obligation> },
    Deny(DenyReason),
}

/// Why a command was denied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyReason {
    NoMatchingRule,
    StalePolicyVersion { current: u64, presented: u64 },
    OutsideTimeWindow,
    OutsideLeaseScope,
}

/// The policy: a generation number and an ordered rule set.
#[derive(Debug, Clone)]
pub struct Policy {
    pub generation: u64,
    rules: Vec<PolicyRule>,
}

impl Policy {
    pub fn new(generation: u64) -> Self {
        Self {
            generation,
            rules: Vec::new(),
        }
    }

    pub fn with_rule(mut self, rule: PolicyRule) -> Self {
        self.rules.push(rule);
        self
    }

    /// Replace the rule set and bump the generation.
    pub fn replace_rules(&mut self, rules: Vec<PolicyRule>) -> u64 {
        self.rules = rules;
        self.generation = self.generation.wrapping_add(1).max(1);
        self.generation
    }
}

/// The Policy Decision Point.
pub struct Pdp<'a> {
    policy: &'a Policy,
}

impl<'a> Pdp<'a> {
    pub fn new(policy: &'a Policy) -> Self {
        Self { policy }
    }

    /// Decide whether `cmd` is allowed under the current policy, given the
    /// lease scope it rode on. Default-deny on no match.
    pub fn decide(&self, cmd: &AuthorizedCommand, lease: &LeaseScope, now: u64) -> Decision {
        // 1. Policy-version staleness: the issuer must have seen the current
        //    generation. A stale version is a hard deny.
        if cmd.policy_version != self.policy.generation {
            return Decision::Deny(DenyReason::StalePolicyVersion {
                current: self.policy.generation,
                presented: cmd.policy_version,
            });
        }

        // 2. Lease scope: the target device/point must be within the lease.
        let in_scope = (lease.devices.is_empty() || lease.devices.contains(&cmd.device_id))
            && (lease.points.is_empty() || lease.points.contains(&cmd.point_id));
        if !in_scope {
            return Decision::Deny(DenyReason::OutsideLeaseScope);
        }

        let class = CommandClass::from_command(cmd);

        // 3. Find the first matching rule. No match → deny.
        for rule in &self.policy.rules {
            if let Some(ak) = rule.actor_kind {
                if ak != cmd.actor_kind {
                    continue;
                }
            }
            if !rule.devices.is_empty() && !rule.devices.contains(&cmd.device_id) {
                continue;
            }
            if !rule.points.is_empty() && !rule.points.contains(&cmd.point_id) {
                continue;
            }
            if !rule.kinds.is_empty() && !rule.kinds.contains(&class) {
                continue;
            }
            if rule.time_window != (0, 0) && (now < rule.time_window.0 || now > rule.time_window.1)
            {
                return Decision::Deny(DenyReason::OutsideTimeWindow);
            }
            return Decision::Allow {
                obligations: rule.obligations.clone(),
            };
        }
        Decision::Deny(DenyReason::NoMatchingRule)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use eneros_telemetry_model::command::{ControlCommand, SingleCommand};

    use super::*;
    use crate::envelope::ActorKind;
    use crate::gateway::AuthorizedCommand;
    use crate::lease::LeaseScope;

    fn auth_cmd(
        actor: ActorKind,
        device: u32,
        point: u32,
        policy_version: u64,
    ) -> AuthorizedCommand {
        AuthorizedCommand {
            device_id: device,
            point_id: point,
            command: ControlCommand::Single(SingleCommand::On),
            actor_kind: actor,
            issuer: [0; 16],
            correlation_id: [0; 16],
            sequence: 1,
            lease_id: [0; 16],
            fencing_token: 1,
            policy_version,
            idempotency_key: [0; 16],
        }
    }

    fn full_scope() -> LeaseScope {
        LeaseScope {
            devices: vec![1, 2],
            points: vec![10, 20],
            max_class: 1,
        }
    }

    #[test]
    fn test_default_deny_when_no_rule_matches() {
        let policy = Policy::new(3);
        let pdp = Pdp::new(&policy);
        let cmd = auth_cmd(ActorKind::Operator, 1, 10, 3);
        assert_eq!(
            pdp.decide(&cmd, &full_scope(), 1_000),
            Decision::Deny(DenyReason::NoMatchingRule)
        );
    }

    #[test]
    fn test_allow_with_matching_rule_and_obligations() {
        let policy = Policy::new(3).with_rule(PolicyRule {
            actor_kind: Some(ActorKind::Operator),
            devices: vec![1],
            points: vec![10],
            kinds: vec![Switch],
            time_window: (0, 0),
            obligations: vec![Obligation::AuditBefore, Obligation::SecondaryConfirm],
        });
        let pdp = Pdp::new(&policy);
        let cmd = auth_cmd(ActorKind::Operator, 1, 10, 3);
        let d = pdp.decide(&cmd, &full_scope(), 1_000);
        match d {
            Decision::Allow { obligations } => {
                assert_eq!(
                    obligations,
                    vec![Obligation::AuditBefore, Obligation::SecondaryConfirm]
                );
            }
            _ => panic!("expected allow"),
        }
    }

    #[test]
    fn test_deny_stale_policy_version() {
        let policy = Policy::new(5);
        let pdp = Pdp::new(&policy);
        let cmd = auth_cmd(ActorKind::Operator, 1, 10, 4); // stale
        assert_eq!(
            pdp.decide(&cmd, &full_scope(), 1_000),
            Decision::Deny(DenyReason::StalePolicyVersion {
                current: 5,
                presented: 4
            })
        );
    }

    #[test]
    fn test_deny_outside_lease_scope() {
        let policy = Policy::new(3).with_rule(PolicyRule {
            actor_kind: Some(ActorKind::Operator),
            devices: vec![],
            points: vec![],
            kinds: vec![],
            time_window: (0, 0),
            obligations: vec![],
        });
        let pdp = Pdp::new(&policy);
        // device 99 is NOT in the lease scope.
        let cmd = auth_cmd(ActorKind::Operator, 99, 10, 3);
        assert_eq!(
            pdp.decide(&cmd, &full_scope(), 1_000),
            Decision::Deny(DenyReason::OutsideLeaseScope)
        );
    }

    #[test]
    fn test_deny_outside_time_window() {
        let policy = Policy::new(3).with_rule(PolicyRule {
            actor_kind: Some(ActorKind::Operator),
            devices: vec![],
            points: vec![],
            kinds: vec![],
            time_window: (2_000, 3_000),
            obligations: vec![],
        });
        let pdp = Pdp::new(&policy);
        let cmd = auth_cmd(ActorKind::Operator, 1, 10, 3);
        assert_eq!(
            pdp.decide(&cmd, &full_scope(), 1_000),
            Decision::Deny(DenyReason::OutsideTimeWindow)
        );
        let allow_cmd = auth_cmd(ActorKind::Operator, 1, 10, 3);
        assert!(matches!(
            pdp.decide(&allow_cmd, &full_scope(), 2_500),
            Decision::Allow { .. }
        ));
    }

    #[test]
    fn test_replace_rules_bumps_generation() {
        let mut policy = Policy::new(3);
        let g = policy.replace_rules(vec![]);
        assert_eq!(g, 4);
        assert_eq!(policy.generation, 4);
    }

    #[test]
    fn test_emergency_actor_classifies_as_emergency() {
        let cmd = auth_cmd(ActorKind::Emergency, 1, 10, 1);
        assert_eq!(CommandClass::from_command(&cmd), Emergency);
    }
}
