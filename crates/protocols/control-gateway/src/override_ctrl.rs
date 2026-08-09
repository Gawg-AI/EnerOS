//! Emergency / manual override (CTL-009).
//!
//! An emergency override is a **local**, highest-priority, time-bounded session
//! that lets a human operator bypass the normal policy PDP (CTL-005) — for
//! example to trip a breaker from the panel during a fault. It does **not**
//! bypass the independent safety validator (CTL-008): an unsafe action stays
//! unsafe even in emergency. Every override is audit-logged.
//!
//! # Local priority
//!
//! An active override wins over any Agent/AI command for the same point: while
//! the override session is live, non-emergency commands for the overridden
//! point are rejected so the operator retains control.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::envelope::ActorKind;
use crate::gateway::AuthorizedCommand;

/// An active emergency override session on a point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverrideSession {
    pub point_id: u32,
    pub issued_epoch: u64,
    pub expiry_epoch: u64,
}

/// Authority that tracks active emergency override sessions.
pub struct OverrideAuthority {
    sessions: BTreeMap<u32, OverrideSession>,
}

impl Default for OverrideAuthority {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether a command is allowed under the override authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverrideDecision {
    /// Emergency command: PDP is bypassed, safety validator still runs.
    EmergencyBypass,
    /// Non-emergency command for a point NOT under override → normal PDP path.
    Normal,
    /// Non-emergency command for a point under active override → rejected
    /// (operator retains control).
    SupersededByOverride,
}

impl OverrideAuthority {
    pub fn new() -> Self {
        Self {
            sessions: BTreeMap::new(),
        }
    }

    /// Activate an emergency override on `point_id` for `ttl_secs`. Only the
    /// emergency actor may do this. Returns the session.
    pub fn activate(
        &mut self,
        point_id: u32,
        ttl_secs: u64,
        now: u64,
        actor: ActorKind,
    ) -> Option<OverrideSession> {
        if actor != ActorKind::Emergency {
            return None;
        }
        let s = OverrideSession {
            point_id,
            issued_epoch: now,
            expiry_epoch: now.saturating_add(ttl_secs),
        };
        self.sessions.insert(point_id, s);
        Some(s)
    }

    /// Drop an override (operator disengages). Idempotent.
    pub fn deactivate(&mut self, point_id: u32) {
        self.sessions.remove(&point_id);
    }

    /// Garbage-collect expired sessions.
    pub fn gc(&mut self, now: u64) {
        let expired: Vec<u32> = self
            .sessions
            .iter()
            .filter(|(_, s)| now > s.expiry_epoch)
            .map(|(p, _)| *p)
            .collect();
        for p in expired {
            self.sessions.remove(&p);
        }
    }

    fn is_active(&self, point_id: u32, now: u64) -> bool {
        match self.sessions.get(&point_id) {
            Some(s) => now <= s.expiry_epoch,
            None => false,
        }
    }

    /// Decide how a command should be routed.
    pub fn decide(&self, cmd: &AuthorizedCommand, now: u64) -> OverrideDecision {
        let under_override = self.is_active(cmd.point_id(), now);
        match (cmd.actor_kind(), under_override) {
            (ActorKind::Emergency, _) => OverrideDecision::EmergencyBypass,
            (_, true) => OverrideDecision::SupersededByOverride,
            (_, false) => OverrideDecision::Normal,
        }
    }

    /// Human-readable audit line for an emergency activation.
    pub fn audit_activate(&self, s: &OverrideSession) -> String {
        format!(
            "EMERGENCY_OVERRIDE point={} issued={} expiry={}",
            s.point_id, s.issued_epoch, s.expiry_epoch
        )
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

    fn make_cmd(actor: ActorKind, point: u32) -> AuthorizedCommand {
        AuthorizedCommand::from_parts(
            1,
            point,
            ControlCommand::Single(SingleCommand::On),
            actor,
            [0; 16],
            [0; 16],
            1,
            [0; 16],
            1,
            1,
            [0; 16],
        )
    }

    #[test]
    fn test_only_emergency_actor_can_activate() {
        let mut auth = OverrideAuthority::new();
        assert!(auth.activate(10, 60, 1_000, ActorKind::Operator).is_none());
        assert!(auth.activate(10, 60, 1_000, ActorKind::Emergency).is_some());
    }

    #[test]
    fn test_emergency_command_bypasses_pdp() {
        let auth = OverrideAuthority::new();
        let cmd = make_cmd(ActorKind::Emergency, 10);
        assert_eq!(auth.decide(&cmd, 1_000), OverrideDecision::EmergencyBypass);
    }

    #[test]
    fn test_non_emergency_superseded_when_override_active() {
        let mut auth = OverrideAuthority::new();
        auth.activate(10, 60, 1_000, ActorKind::Emergency);
        let cmd = make_cmd(ActorKind::Agent, 10);
        assert_eq!(
            auth.decide(&cmd, 1_000),
            OverrideDecision::SupersededByOverride
        );
    }

    #[test]
    fn test_non_emergency_normal_when_no_override() {
        let auth = OverrideAuthority::new();
        let cmd = make_cmd(ActorKind::Agent, 10);
        assert_eq!(auth.decide(&cmd, 1_000), OverrideDecision::Normal);
    }

    #[test]
    fn test_override_expires() {
        let mut auth = OverrideAuthority::new();
        auth.activate(10, 60, 1_000, ActorKind::Emergency);
        let cmd = make_cmd(ActorKind::Agent, 10);
        assert_eq!(auth.decide(&cmd, 1_061), OverrideDecision::Normal);
    }

    #[test]
    fn test_deactivate() {
        let mut auth = OverrideAuthority::new();
        auth.activate(10, 60, 1_000, ActorKind::Emergency);
        auth.deactivate(10);
        let cmd = make_cmd(ActorKind::Agent, 10);
        assert_eq!(auth.decide(&cmd, 1_000), OverrideDecision::Normal);
    }

    #[test]
    fn test_gc_removes_expired() {
        let mut auth = OverrideAuthority::new();
        auth.activate(10, 60, 1_000, ActorKind::Emergency);
        auth.gc(1_061);
        let cmd = make_cmd(ActorKind::Agent, 10);
        assert_eq!(auth.decide(&cmd, 1_061), OverrideDecision::Normal);
    }
}
