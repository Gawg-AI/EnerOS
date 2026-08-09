//! Incident replay: command causal chain reconstruction (CTL-014).
//!
//! MASTER §9.5: "所有命令能由审计链还原"谁、何时、为什么、依据什么策略、
//! 发给谁、结果如何"。"
//!
//! MASTER §13.3: crash service collects "最近有界事件" and dump/source
//! association. This module provides the **command causal chain** recorder
//! and replay engine for the control path.
//!
//! # Design
//!
//! Every command that enters the control gateway is recorded as a chain of
//! [`CausalEvent`]s:
//!
//! ```text
//! Proposed → EnvelopeDecoded → PolicyDecided → LeaseBound →
//! SafetyValidated → Executed → FeedbackReconciled
//! ```
//!
//! Each event carries a monotonic timestamp, the command_id (correlation
//! key), and the relevant verdict/data. The recorder is a bounded ring —
//! it keeps the last N command chains (default 4096) for post-incident
//! replay.
//!
//! [`IncidentReplay::reconstruct`] takes a command_id and returns the full
//! causal chain, allowing an investigator to answer:
//! - WHO issued the command (actor identity)
//! - WHEN (monotonic + wall time)
//! - WHY (policy decision + obligation)
//! - WHAT policy version was applied
//! - WHETHER safety validated it
//! - WHAT the execution result was
//! - WHETHER feedback reconciled

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::envelope::ActorKind;
use crate::gateway::GatewayError;
use crate::policy::{Decision, DenyReason};
use crate::safety::SafetyVerdict;

// ============================================================================
// Causal event types
// ============================================================================

/// One event in a command's causal chain.
#[derive(Debug, Clone)]
pub struct CausalEvent {
    /// Monotonic timestamp (ns).
    pub monotonic_ns: u64,
    /// Wall-clock timestamp (epoch secs, may be 0 if unsynchronized).
    pub wall_secs: u64,
    /// The command_id (correlation key for the chain).
    pub command_id: [u8; 16],
    /// The kind of event.
    pub kind: CausalEventKind,
}

/// The kind of causal event.
#[derive(Debug, Clone)]
pub enum CausalEventKind {
    /// A command was proposed by an actor (Agent/operator/protocol).
    Proposed {
        actor: ActorKind,
        issuer: [u8; 16],
        device_id: u32,
        point_id: u32,
        sequence: u64,
    },
    /// The envelope was decoded (structural check passed).
    EnvelopeDecoded {
        policy_version: u32,
        config_version: u32,
    },
    /// The policy decision point reached a verdict.
    PolicyDecided { decision: DecisionRecord },
    /// The lease was bound (or denied).
    LeaseBound {
        lease_id: [u8; 16],
        fencing_token: u64,
    },
    /// The lease was denied.
    LeaseDenied { reason: String },
    /// The safety validator reached a verdict.
    SafetyValidated { verdict: SafetyVerdictRecord },
    /// The command was executed on the device/plant.
    Executed { outcome: ExecutionOutcomeRecord },
    /// Feedback was reconciled (or mismatched).
    FeedbackReconciled {
        matched: bool,
        expected: String,
        actual: String,
    },
    /// The command was rejected at some gate.
    Rejected { gate: RejectionGate, reason: String },
}

/// Policy decision recorded for replay.
#[derive(Debug, Clone)]
pub enum DecisionRecord {
    Permit,
    Deny(DenyReason),
}

impl From<&Decision> for DecisionRecord {
    fn from(d: &Decision) -> Self {
        match d {
            Decision::Allow { .. } => DecisionRecord::Permit,
            Decision::Deny(r) => DecisionRecord::Deny(*r),
        }
    }
}

/// Safety verdict recorded for replay.
#[derive(Debug, Clone)]
pub enum SafetyVerdictRecord {
    Safe,
    Unsafe(String),
}

impl From<&SafetyVerdict> for SafetyVerdictRecord {
    fn from(v: &SafetyVerdict) -> Self {
        match v {
            SafetyVerdict::Safe => SafetyVerdictRecord::Safe,
            SafetyVerdict::Unsafe(s) => SafetyVerdictRecord::Unsafe(s.clone()),
        }
    }
}

/// Execution outcome recorded for replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionOutcomeRecord {
    Applied,
    ActuatorStuck,
    PoweredDown,
    NotExecuted,
}

/// Which gate rejected the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectionGate {
    /// Envelope decode/HMAC failure.
    Gateway,
    /// Policy denied.
    Policy,
    /// Lease denied/expired/revoked.
    Lease,
    /// Safety validator denied.
    Safety,
    /// Idempotency dedup.
    Idempotency,
}

impl From<&GatewayError> for RejectionGate {
    fn from(e: &GatewayError) -> Self {
        match e {
            GatewayError::Unauthorized
            | GatewayError::Decode(_)
            | GatewayError::Expired
            | GatewayError::Future
            | GatewayError::Replayed
            | GatewayError::StaleFencing
            | GatewayError::BadDeviceId => RejectionGate::Gateway,
            GatewayError::NoLease | GatewayError::LeaseRevoked => RejectionGate::Lease,
            GatewayError::NoPolicy => RejectionGate::Policy,
        }
    }
}

// ============================================================================
// Command chain
// ============================================================================

/// The full causal chain for one command.
#[derive(Debug, Clone)]
pub struct CommandChain {
    pub command_id: [u8; 16],
    pub events: Vec<CausalEvent>,
}

impl CommandChain {
    /// The first event (Proposed), if any.
    pub fn proposed(&self) -> Option<&CausalEvent> {
        self.events.first()
    }

    /// The last event, if any.
    pub fn last(&self) -> Option<&CausalEvent> {
        self.events.last()
    }

    /// True if the chain ends with a rejection.
    pub fn was_rejected(&self) -> bool {
        self.events
            .last()
            .map(|e| matches!(e.kind, CausalEventKind::Rejected { .. }))
            .unwrap_or(false)
    }

    /// True if the chain reached execution and was applied.
    pub fn was_executed(&self) -> bool {
        self.events.iter().any(|e| {
            matches!(
                e.kind,
                CausalEventKind::Executed {
                    outcome: ExecutionOutcomeRecord::Applied
                }
            )
        })
    }

    /// The rejection reason, if rejected.
    pub fn rejection(&self) -> Option<(&RejectionGate, &str)> {
        self.events.iter().find_map(|e| {
            if let CausalEventKind::Rejected { gate, reason } = &e.kind {
                Some((gate, reason.as_str()))
            } else {
                None
            }
        })
    }

    /// Duration from first to last event (ns).
    pub fn duration_ns(&self) -> u64 {
        match (self.events.first(), self.events.last()) {
            (Some(first), Some(last)) => last.monotonic_ns.saturating_sub(first.monotonic_ns),
            _ => 0,
        }
    }
}

// ============================================================================
// Incident replay recorder
// ============================================================================

/// Maximum command chains kept in the ring (default).
const DEFAULT_MAX_CHAINS: usize = 4096;

/// The incident replay recorder. Owns a bounded map of command_id → chain.
///
/// This is the post-incident analysis component. It is NOT the audit store
/// (that is OBS-005/017). It is the **causal chain** reconstruction engine
/// that links audit events into a replayable timeline.
pub struct IncidentReplay {
    chains: BTreeMap<[u8; 16], CommandChain>,
    /// FIFO order for eviction.
    order: Vec<[u8; 16]>,
    max_chains: usize,
}

impl Default for IncidentReplay {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_CHAINS)
    }
}

impl IncidentReplay {
    pub fn new(max_chains: usize) -> Self {
        Self {
            chains: BTreeMap::new(),
            order: Vec::new(),
            max_chains,
        }
    }

    /// Record a causal event. If the command_id is new, a new chain is
    /// started. Events are appended in monotonic order.
    pub fn record(&mut self, event: CausalEvent) {
        let cmd_id = event.command_id;
        let chain = self.chains.entry(cmd_id).or_insert_with(|| CommandChain {
            command_id: cmd_id,
            events: Vec::new(),
        });
        chain.events.push(event);

        // Track order for eviction (only on first insert).
        if chain.events.len() == 1 {
            self.order.push(cmd_id);
            // Evict oldest if over capacity.
            if self.order.len() > self.max_chains {
                if let Some(oldest) = self.order.first().copied() {
                    self.order.remove(0);
                    self.chains.remove(&oldest);
                }
            }
        }
    }

    /// Reconstruct the full causal chain for a command.
    pub fn reconstruct(&self, command_id: &[u8; 16]) -> Option<&CommandChain> {
        self.chains.get(command_id)
    }

    /// All command_ids in the recorder.
    pub fn command_ids(&self) -> Vec<[u8; 16]> {
        self.chains.keys().copied().collect()
    }

    /// Number of chains currently stored.
    pub fn chain_count(&self) -> usize {
        self.chains.len()
    }

    /// Find all chains that were rejected at a specific gate.
    pub fn chains_rejected_at(&self, gate: RejectionGate) -> Vec<&CommandChain> {
        self.chains
            .values()
            .filter(|c| c.rejection().map(|(g, _)| *g == gate).unwrap_or(false))
            .collect()
    }

    /// Find all chains that were executed successfully.
    pub fn chains_executed(&self) -> Vec<&CommandChain> {
        self.chains.values().filter(|c| c.was_executed()).collect()
    }

    /// Summary statistics for telemetry.
    pub fn summary(&self) -> ReplaySummary {
        let total = self.chains.len();
        let executed = self.chains.values().filter(|c| c.was_executed()).count();
        let rejected = self.chains.values().filter(|c| c.was_rejected()).count();
        let max_duration = self
            .chains
            .values()
            .map(|c| c.duration_ns())
            .max()
            .unwrap_or(0);
        ReplaySummary {
            total_chains: total,
            executed,
            rejected,
            max_duration_ns: max_duration,
        }
    }
}

/// Summary statistics for the replay recorder.
#[derive(Debug, Clone, Copy)]
pub struct ReplaySummary {
    pub total_chains: usize,
    pub executed: usize,
    pub rejected: usize,
    pub max_duration_ns: u64,
}

// ============================================================================
// Convenience: record a full command lifecycle
// ============================================================================

impl IncidentReplay {
    /// Record the "Proposed" event.
    #[allow(clippy::too_many_arguments)]
    pub fn record_proposed(
        &mut self,
        command_id: [u8; 16],
        monotonic_ns: u64,
        wall_secs: u64,
        actor: ActorKind,
        issuer: [u8; 16],
        device_id: u32,
        point_id: u32,
        sequence: u64,
    ) {
        self.record(CausalEvent {
            monotonic_ns,
            wall_secs,
            command_id,
            kind: CausalEventKind::Proposed {
                actor,
                issuer,
                device_id,
                point_id,
                sequence,
            },
        });
    }

    /// Record the "PolicyDecided" event.
    pub fn record_policy(
        &mut self,
        command_id: [u8; 16],
        monotonic_ns: u64,
        wall_secs: u64,
        decision: &Decision,
    ) {
        self.record(CausalEvent {
            monotonic_ns,
            wall_secs,
            command_id,
            kind: CausalEventKind::PolicyDecided {
                decision: decision.into(),
            },
        });
    }

    /// Record the "SafetyValidated" event.
    pub fn record_safety(
        &mut self,
        command_id: [u8; 16],
        monotonic_ns: u64,
        wall_secs: u64,
        verdict: &SafetyVerdict,
    ) {
        self.record(CausalEvent {
            monotonic_ns,
            wall_secs,
            command_id,
            kind: CausalEventKind::SafetyValidated {
                verdict: verdict.into(),
            },
        });
    }

    /// Record the "Executed" event.
    pub fn record_executed(
        &mut self,
        command_id: [u8; 16],
        monotonic_ns: u64,
        wall_secs: u64,
        outcome: ExecutionOutcomeRecord,
    ) {
        self.record(CausalEvent {
            monotonic_ns,
            wall_secs,
            command_id,
            kind: CausalEventKind::Executed { outcome },
        });
    }

    /// Record the "Rejected" event.
    pub fn record_rejected(
        &mut self,
        command_id: [u8; 16],
        monotonic_ns: u64,
        wall_secs: u64,
        gate: RejectionGate,
        reason: &str,
    ) {
        self.record(CausalEvent {
            monotonic_ns,
            wall_secs,
            command_id,
            kind: CausalEventKind::Rejected {
                gate,
                reason: String::from(reason),
            },
        });
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;
    use crate::envelope::ActorKind;
    use crate::policy::DenyReason;

    #[test]
    fn test_record_and_reconstruct_full_chain() {
        let mut replay = IncidentReplay::default();
        let cmd_id = [0x01; 16];

        replay.record_proposed(cmd_id, 1_000, 100, ActorKind::Operator, [0; 16], 1, 10, 1);
        replay.record_policy(
            cmd_id,
            1_100,
            100,
            &Decision::Allow {
                obligations: Vec::new(),
            },
        );
        replay.record_safety(cmd_id, 1_200, 100, &SafetyVerdict::Safe);
        replay.record_executed(cmd_id, 1_300, 100, ExecutionOutcomeRecord::Applied);

        let chain = replay.reconstruct(&cmd_id).expect("chain must exist");
        assert_eq!(chain.events.len(), 4);
        assert!(chain.was_executed());
        assert!(!chain.was_rejected());
        assert_eq!(chain.duration_ns(), 300);
    }

    #[test]
    fn test_rejected_chain() {
        let mut replay = IncidentReplay::default();
        let cmd_id = [0x02; 16];

        replay.record_proposed(cmd_id, 1_000, 100, ActorKind::Agent, [0; 16], 1, 10, 1);
        replay.record_policy(
            cmd_id,
            1_100,
            100,
            &Decision::Deny(DenyReason::NoMatchingRule),
        );
        replay.record_rejected(
            cmd_id,
            1_150,
            100,
            RejectionGate::Policy,
            "no matching rule",
        );

        let chain = replay.reconstruct(&cmd_id).expect("chain must exist");
        assert!(chain.was_rejected());
        assert!(!chain.was_executed());
        let (gate, reason) = chain.rejection().expect("must have rejection");
        assert_eq!(*gate, RejectionGate::Policy);
        assert!(reason.contains("no matching rule"));
    }

    #[test]
    fn test_safety_rejection() {
        let mut replay = IncidentReplay::default();
        let cmd_id = [0x03; 16];

        replay.record_proposed(cmd_id, 1_000, 100, ActorKind::Agent, [0; 16], 1, 10, 1);
        replay.record_policy(
            cmd_id,
            1_100,
            100,
            &Decision::Allow {
                obligations: Vec::new(),
            },
        );
        replay.record_safety(
            cmd_id,
            1_200,
            100,
            &SafetyVerdict::Unsafe(String::from("UCA-PCC-001")),
        );
        replay.record_rejected(cmd_id, 1_250, 100, RejectionGate::Safety, "UCA-PCC-001");

        let chain = replay.reconstruct(&cmd_id).expect("chain must exist");
        assert!(chain.was_rejected());
        let (gate, _) = chain.rejection().unwrap();
        assert_eq!(*gate, RejectionGate::Safety);
    }

    #[test]
    fn test_ring_eviction() {
        let mut replay = IncidentReplay::new(3); // small ring
        for i in 0u8..5 {
            let cmd_id = [i; 16];
            replay.record_proposed(
                cmd_id,
                1_000 + i as u64,
                100,
                ActorKind::Operator,
                [0; 16],
                1,
                10,
                i as u64,
            );
        }
        // Only last 3 should remain.
        assert_eq!(replay.chain_count(), 3);
        // Oldest (id=[0;16] and [1;16]) should be evicted.
        assert!(replay.reconstruct(&[0; 16]).is_none());
        assert!(replay.reconstruct(&[1; 16]).is_none());
        assert!(replay.reconstruct(&[2; 16]).is_some());
        assert!(replay.reconstruct(&[3; 16]).is_some());
        assert!(replay.reconstruct(&[4; 16]).is_some());
    }

    #[test]
    fn test_chains_rejected_at_gate() {
        let mut replay = IncidentReplay::default();
        // Chain 1: rejected at policy.
        replay.record_rejected([1; 16], 1_000, 100, RejectionGate::Policy, "rule miss");
        // Chain 2: rejected at safety.
        replay.record_rejected([2; 16], 2_000, 100, RejectionGate::Safety, "UCA");
        // Chain 3: rejected at lease.
        replay.record_rejected([3; 16], 3_000, 100, RejectionGate::Lease, "expired");

        assert_eq!(replay.chains_rejected_at(RejectionGate::Policy).len(), 1);
        assert_eq!(replay.chains_rejected_at(RejectionGate::Safety).len(), 1);
        assert_eq!(replay.chains_rejected_at(RejectionGate::Lease).len(), 1);
    }

    #[test]
    fn test_summary_statistics() {
        let mut replay = IncidentReplay::default();
        // 2 executed, 1 rejected.
        replay.record_executed([1; 16], 1_000, 100, ExecutionOutcomeRecord::Applied);
        replay.record_executed([2; 16], 2_000, 100, ExecutionOutcomeRecord::Applied);
        replay.record_rejected([3; 16], 3_000, 100, RejectionGate::Policy, "denied");

        let s = replay.summary();
        assert_eq!(s.total_chains, 3);
        assert_eq!(s.executed, 2);
        assert_eq!(s.rejected, 1);
    }

    #[test]
    fn test_command_chain_duration() {
        let mut replay = IncidentReplay::default();
        let cmd_id = [0xAA; 16];
        replay.record_proposed(cmd_id, 5_000, 100, ActorKind::Operator, [0; 16], 1, 10, 1);
        replay.record_executed(cmd_id, 5_500, 100, ExecutionOutcomeRecord::Applied);
        let chain = replay.reconstruct(&cmd_id).unwrap();
        assert_eq!(chain.duration_ns(), 500);
    }

    #[test]
    fn test_actuator_stuck_recorded() {
        let mut replay = IncidentReplay::default();
        let cmd_id = [0xBB; 16];
        replay.record_proposed(cmd_id, 1_000, 100, ActorKind::Operator, [0; 16], 1, 10, 1);
        replay.record_executed(cmd_id, 1_200, 100, ExecutionOutcomeRecord::ActuatorStuck);

        let chain = replay.reconstruct(&cmd_id).unwrap();
        // ActuatorStuck means NOT Applied.
        assert!(!chain.was_executed());
    }
}
