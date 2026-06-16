//! Multi-agent action conflict detection and resolution.
//!
//! In a multi-agent AgentOS, several agents may simultaneously issue control
//! actions that touch the same device or set conflicting parameter values.
//! This module detects those conflicts and resolves them with a **deterministic
//! total-ordering** so the system can never deadlock on a tie.
//!
//! # Resolution chain (highest precedence first)
//!
//! 1. **Authority priority** — the highest [`AuthorityLevel`] wins. If two
//!    agents tie at the same level, fall through.
//! 2. **Time priority** — the action with the earliest timestamp wins
//!    ("first-come, first-served"). If they tie, fall through.
//! 3. **Topology proximity** — the agent whose jurisdiction is electrically
//!    closest to the contested device wins (requires a [`ProximityProvider`];
//!    when none is supplied this stage is skipped).
//! 4. **Agent ID tie-break** — lexicographically smallest agent id wins.
//!
//! Because step 4 is a total order on unique agent ids, resolution *always*
//! produces a single winner — there is no path to a forced `ManualResolution`
//! fallback for genuine conflicts. `ManualResolution` is now reserved only for
//! explicit `manual_only()` mode.

use std::collections::HashMap;
use eneros_core::{AuthorityLevel, ElementId};
use serde::{Deserialize, Serialize};
use crate::agent::AgentAction;

/// Type of action conflict
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictType {
    /// Two agents want to control the same device in opposite ways
    DeviceControlConflict,
    /// Two agents want to set conflicting values on the same device+parameter
    ParameterConflict,
    /// Two agents want to execute mutually exclusive operations
    MutualExclusion,
}

/// A detected conflict between agent actions.
///
/// All parallel vectors (`agent_ids`, `authority_levels`, `conflicting_actions`,
/// `timestamps`, `jurisdiction_bus_ids`) are indexed identically — index `i`
/// describes the i-th party to the conflict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionConflict {
    /// IDs of the agents involved in the conflict
    pub agent_ids: Vec<String>,
    /// Authority levels of the involved agents
    pub authority_levels: Vec<AuthorityLevel>,
    /// The conflicting actions
    pub conflicting_actions: Vec<AgentAction>,
    /// Timestamps of each party's action (monotonic; earliest wins on tie)
    pub timestamps: Vec<u64>,
    /// Jurisdiction bus id of each party (used for topology proximity)
    pub jurisdiction_bus_ids: Vec<Option<ElementId>>,
    /// The single device id all parties are contesting
    pub device_id: ElementId,
    /// Optional parameter key (when the conflict is on a specific parameter)
    pub parameter_key: Option<String>,
    /// Type of conflict
    pub conflict_type: ConflictType,
    /// Description of the conflict
    pub description: String,
    /// Resolved action (set after resolution)
    pub resolution: Option<ConflictResolution>,
}

/// Resolution of a conflict
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictResolution {
    /// Winning agent ID
    pub winner_id: String,
    /// Winning action
    pub winning_action: AgentAction,
    /// Resolution strategy used
    pub strategy: ResolutionStrategy,
    /// Reason for the resolution
    pub reason: String,
    /// Agent ids that lost this conflict (recorded for audit / notification)
    pub loser_ids: Vec<String>,
}

/// Strategy used to resolve a conflict
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolutionStrategy {
    /// Higher authority level wins
    AuthorityPriority,
    /// Agent with closer jurisdiction wins
    TopologyProximity,
    /// First action received wins
    TimePriority,
    /// Deterministic tie-break by agent id (final fallback, never deadlocks)
    AgentIdTieBreak,
    /// Manual resolution required
    ManualResolution,
}

/// Agent action with metadata for conflict detection.
#[derive(Debug, Clone)]
pub struct TaggedAction {
    /// Agent ID that produced this action
    pub agent_id: String,
    /// Agent's authority level
    pub authority_level: AuthorityLevel,
    /// The action itself
    pub action: AgentAction,
    /// Target device ID (if applicable). Actions without a target device are
    /// never considered conflicting.
    pub target_device_id: Option<ElementId>,
    /// Timestamp of the action (monotonic clock; smaller = earlier)
    pub timestamp: u64,
    /// Bus id this agent has jurisdiction over — used for topology-proximity
    /// resolution. `None` means "unknown / no fixed jurisdiction".
    pub jurisdiction_bus_id: Option<ElementId>,
    /// Parameter key being set, when the action targets a specific parameter
    /// of the device (e.g. `"voltage_setpoint"`, `"tap_position"`). Two actions
    /// on the same device but different parameter keys do **not** conflict.
    pub parameter_key: Option<String>,
}

/// Topology-proximity provider — returns the graph hop-distance (number of
/// branches) between two buses.
///
/// This is injected (rather than depending on the `eneros-topology` crate
/// directly) to keep the agent crate free of a hard topology dependency and to
/// make the resolver unit-testable with a trivial mock. A distance of `0`
/// means "same bus"; `None` means "no path / unknown", treated as maximal.
pub trait ProximityProvider: Send + Sync {
    /// Hop distance between `from` and `to`. Smaller = closer.
    fn hop_distance(&self, from: ElementId, to: ElementId) -> Option<u32>;
}

/// A simple no-op proximity provider that reports every distance as unknown,
/// effectively disabling topology-proximity resolution. Used as the default.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoProximity;

impl ProximityProvider for NoProximity {
    fn hop_distance(&self, _from: ElementId, _to: ElementId) -> Option<u32> {
        None
    }
}

/// One entry in the resolution audit trail produced by [`ActionConflictResolver::process_with_audit`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolutionAuditEntry {
    /// Device that was contested
    pub device_id: ElementId,
    /// Optional parameter key
    pub parameter_key: Option<String>,
    /// Conflict type
    pub conflict_type: ConflictType,
    /// How it was resolved
    pub strategy: ResolutionStrategy,
    /// Winning agent
    pub winner_id: String,
    /// Losing agents (already dropped from the action stream)
    pub loser_ids: Vec<String>,
    /// Human-readable reason
    pub reason: String,
}

/// Result of [`ActionConflictResolver::process_with_audit`].
#[derive(Debug, Clone)]
pub struct ProcessResult {
    /// Actions surviving conflict resolution (winners + non-conflicting).
    pub surviving_actions: Vec<TaggedAction>,
    /// Ordered audit trail of every conflict that was resolved.
    pub audit_trail: Vec<ResolutionAuditEntry>,
}

/// Action conflict resolver — detects and resolves conflicts between agent actions.
pub struct ActionConflictResolver {
    /// Whether to automatically resolve conflicts
    auto_resolve: bool,
    /// Optional topology-proximity provider for the proximity strategy.
    proximity: Box<dyn ProximityProvider>,
}

impl ActionConflictResolver {
    /// Create a new resolver with auto-resolution enabled and no topology
    /// proximity (the time / authority / id tie-break chain still applies).
    pub fn new() -> Self {
        Self {
            auto_resolve: true,
            proximity: Box::new(NoProximity),
        }
    }

    /// Create a resolver with a topology-proximity provider.
    pub fn with_proximity(proximity: Box<dyn ProximityProvider>) -> Self {
        Self {
            auto_resolve: true,
            proximity,
        }
    }

    /// Create a resolver without auto-resolution (conflicts require manual resolution)
    pub fn manual_only() -> Self {
        Self {
            auto_resolve: false,
            proximity: Box::new(NoProximity),
        }
    }

    /// Detect conflicts among a set of tagged actions.
    ///
    /// Two actions conflict when they target the **same device** and either:
    ///   - both omit a `parameter_key` (raw device-control conflict), or
    ///   - share the same `parameter_key` (same parameter set differently).
    ///
    /// Actions on the same device but different parameter keys are allowed to
    /// coexist (e.g. setting a generator's MW output and its voltage setpoint
    /// are independent).
    pub fn detect_conflicts(&self, actions: &[TaggedAction]) -> Vec<ActionConflict> {
        // Group by (device_id, parameter_key). parameter_key None is its own bucket.
        let mut buckets: HashMap<(ElementId, Option<String>), Vec<&TaggedAction>> = HashMap::new();
        for action in actions {
            if let Some(device_id) = action.target_device_id {
                buckets
                    .entry((device_id, action.parameter_key.clone()))
                    .or_default()
                    .push(action);
            }
        }

        let mut conflicts = Vec::new();
        for ((device_id, parameter_key), parties) in &buckets {
            if parties.len() < 2 {
                continue; // No contention.
            }

            let conflict_type = if parameter_key.is_some() {
                ConflictType::ParameterConflict
            } else {
                ConflictType::DeviceControlConflict
            };

            let agent_ids: Vec<String> = parties.iter().map(|a| a.agent_id.clone()).collect();
            let authority_levels: Vec<AuthorityLevel> =
                parties.iter().map(|a| a.authority_level).collect();
            let conflicting_actions: Vec<AgentAction> =
                parties.iter().map(|a| a.action.clone()).collect();
            let timestamps: Vec<u64> = parties.iter().map(|a| a.timestamp).collect();
            let jurisdiction_bus_ids: Vec<Option<ElementId>> =
                parties.iter().map(|a| a.jurisdiction_bus_id).collect();

            let desc = if let Some(pk) = parameter_key {
                format!("{} agents contesting device {} parameter {:?}", parties.len(), device_id, pk)
            } else {
                format!("{} agents contesting control of device {}", parties.len(), device_id)
            };

            conflicts.push(ActionConflict {
                agent_ids,
                authority_levels,
                conflicting_actions,
                timestamps,
                jurisdiction_bus_ids,
                device_id: *device_id,
                parameter_key: parameter_key.clone(),
                conflict_type,
                description: desc,
                resolution: None,
            });
        }

        conflicts
    }

    /// Resolve a conflict using the deterministic resolution chain.
    ///
    /// Chain: authority → time → topology proximity → agent-id tie-break.
    /// Because agent ids are unique and totally ordered, a genuine conflict is
    /// **always** resolvable here — `ManualResolution` is only used in
    /// `manual_only()` mode.
    pub fn resolve(&self, conflict: ActionConflict) -> ActionConflict {
        if !self.auto_resolve {
            return ActionConflict {
                resolution: Some(ConflictResolution {
                    winner_id: String::new(),
                    winning_action: AgentAction::NoOp,
                    strategy: ResolutionStrategy::ManualResolution,
                    reason: "Auto-resolution disabled, manual resolution required".to_string(),
                    loser_ids: conflict.agent_ids.clone(),
                }),
                ..conflict
            };
        }

        // Select a single winner with the full deterministic chain.
        let (winner_idx, strategy, reason) = self.select_winner(&conflict);
        let loser_ids: Vec<String> = conflict
            .agent_ids
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != winner_idx)
            .map(|(_, id)| id.clone())
            .collect();

        ActionConflict {
            resolution: Some(ConflictResolution {
                winner_id: conflict.agent_ids[winner_idx].clone(),
                winning_action: conflict.conflicting_actions[winner_idx].clone(),
                strategy,
                reason,
                loser_ids,
            }),
            ..conflict
        }
    }

    /// Deterministic winner selection. Returns `(winner_index, strategy, reason)`.
    ///
    /// Each stage filters to the best-scoring parties; if more than one
    /// remains, the next stage breaks the tie. The final agent-id stage is a
    /// total order, so exactly one winner always emerges.
    fn select_winner(&self, conflict: &ActionConflict) -> (usize, ResolutionStrategy, String) {
        let n = conflict.agent_ids.len();
        debug_assert!(n >= 2, "select_winner requires a real conflict");

        // Stage 1: authority — keep only max-authority parties.
        let max_auth = conflict.authority_levels.iter().copied().max().unwrap();
        let mut candidates: Vec<usize> = (0..n)
            .filter(|&i| conflict.authority_levels[i] == max_auth)
            .collect();

        if candidates.len() == 1 {
            let w = candidates[0];
            return (
                w,
                ResolutionStrategy::AuthorityPriority,
                format!(
                    "Agent {} has highest authority ({:?})",
                    conflict.agent_ids[w], max_auth
                ),
            );
        }

        // Stage 2: time — keep only the earliest-timestamp parties.
        let min_ts = candidates
            .iter()
            .map(|&i| conflict.timestamps[i])
            .min()
            .unwrap();
        candidates.retain(|&i| conflict.timestamps[i] == min_ts);
        if candidates.len() == 1 {
            let w = candidates[0];
            return (
                w,
                ResolutionStrategy::TimePriority,
                format!(
                    "Agent {} tied on authority but acted first (t={})",
                    conflict.agent_ids[w], min_ts
                ),
            );
        }

        // Stage 3: topology proximity (only meaningful if every remaining
        // candidate has a jurisdiction bus AND a provider is configured).
        if let Some(w) = self.try_proximity_winner(conflict, &candidates) {
            return (
                w,
                ResolutionStrategy::TopologyProximity,
                format!(
                    "Agent {} has jurisdiction closest to device {}",
                    conflict.agent_ids[w], conflict.device_id
                ),
            );
        }

        // Stage 4: agent-id tie-break (total order — always resolves).
        let w = candidates
            .iter()
            .copied()
            .min_by_key(|&i| conflict.agent_ids[i].clone())
            .unwrap();
        (
            w,
            ResolutionStrategy::AgentIdTieBreak,
            format!(
                "Full tie on authority/time/proximity; agent {} wins by deterministic id ordering",
                conflict.agent_ids[w]
            ),
        )
    }

    /// Topology-proximity stage. Returns the single closest candidate, or `None`
    /// if proximity cannot decide (provider returns no distance, candidates
    /// have no jurisdiction bus, or there is a tie in distance).
    fn try_proximity_winner(
        &self,
        conflict: &ActionConflict,
        candidates: &[usize],
    ) -> Option<usize> {
        // The device's own location. We approximate the device's bus by the
        // device id (devices are typically addressed on their bus id). When a
        // candidate has a jurisdiction bus, measure hop distance to the device.
        let target_bus = conflict.device_id;

        let mut best: Option<(usize, u32)> = None;
        for &i in candidates {
            let jur = conflict.jurisdiction_bus_ids[i]?;
            let dist = self.proximity.hop_distance(jur, target_bus)?;
            match best {
                None => best = Some((i, dist)),
                Some((_, bd)) if dist < bd => best = Some((i, dist)),
                Some((_, bd)) if dist == bd => return None, // proximity tie → defer
                _ => {}
            }
        }
        best.map(|(i, _)| i)
    }

    /// Process a batch of tagged actions: detect conflicts, resolve, return
    /// surviving actions. Losers are dropped per (device, parameter) bucket.
    pub fn process(&self, actions: Vec<TaggedAction>) -> Vec<TaggedAction> {
        self.process_with_audit(actions).surviving_actions
    }

    /// Like [`process`](Self::process) but also returns an audit trail of every
    /// resolution. Use this in production deployments where conflict decisions
    /// must be traceable.
    pub fn process_with_audit(&self, actions: Vec<TaggedAction>) -> ProcessResult {
        let conflicts = self.detect_conflicts(&actions);

        if conflicts.is_empty() {
            return ProcessResult {
                surviving_actions: actions,
                audit_trail: Vec::new(),
            };
        }

        // Resolve every conflict.
        let resolved: Vec<ActionConflict> = conflicts.into_iter().map(|c| self.resolve(c)).collect();

        // Build a per-(device,parameter) loser set so we drop *only* the losing
        // party on the contested bucket. An agent acting on another device, or
        // on a different parameter of the same device, is unaffected.
        let mut loser_keys: Vec<(String, ElementId, Option<String>)> = Vec::new();
        let mut audit_trail = Vec::new();

        for c in &resolved {
            if let Some(ref r) = c.resolution {
                for loser in &r.loser_ids {
                    loser_keys.push((loser.clone(), c.device_id, c.parameter_key.clone()));
                }
                audit_trail.push(ResolutionAuditEntry {
                    device_id: c.device_id,
                    parameter_key: c.parameter_key.clone(),
                    conflict_type: c.conflict_type.clone(),
                    strategy: r.strategy.clone(),
                    winner_id: r.winner_id.clone(),
                    loser_ids: r.loser_ids.clone(),
                    reason: r.reason.clone(),
                });
            }
        }

        let mut surviving = Vec::with_capacity(actions.len());
        for action in actions {
            let device_id = match action.target_device_id {
                Some(d) => d,
                None => {
                    surviving.push(action); // No target → never conflicting.
                    continue;
                }
            };
            let is_loser = loser_keys.iter().any(|(agent, dev, pk)| {
                *agent == action.agent_id && *dev == device_id && *pk == action.parameter_key
            });
            if !is_loser {
                surviving.push(action);
            }
        }

        ProcessResult {
            surviving_actions: surviving,
            audit_trail,
        }
    }
}

impl Default for ActionConflictResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_core::AuthorityLevel;

    fn tagged(
        agent_id: &str,
        authority: AuthorityLevel,
        device_id: Option<u64>,
        timestamp: u64,
    ) -> TaggedAction {
        TaggedAction {
            agent_id: agent_id.to_string(),
            authority_level: authority,
            action: AgentAction::NoOp,
            target_device_id: device_id,
            timestamp,
            jurisdiction_bus_id: None,
            parameter_key: None,
        }
    }

    fn tagged_full(
        agent_id: &str,
        authority: AuthorityLevel,
        device_id: Option<u64>,
        timestamp: u64,
        jurisdiction: Option<u64>,
        parameter: Option<&str>,
    ) -> TaggedAction {
        TaggedAction {
            agent_id: agent_id.to_string(),
            authority_level: authority,
            action: AgentAction::NoOp,
            target_device_id: device_id,
            timestamp,
            jurisdiction_bus_id: jurisdiction,
            parameter_key: parameter.map(|s| s.to_string()),
        }
    }

    // ===== detection =====

    #[test]
    fn test_no_conflict_single_action() {
        let resolver = ActionConflictResolver::new();
        let actions = vec![tagged("a1", AuthorityLevel::Operator, Some(1), 0)];
        assert!(resolver.detect_conflicts(&actions).is_empty());
    }

    #[test]
    fn test_no_conflict_different_devices() {
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            tagged("a1", AuthorityLevel::Operator, Some(1), 0),
            tagged("a2", AuthorityLevel::Operator, Some(2), 0),
        ];
        assert!(resolver.detect_conflicts(&actions).is_empty());
    }

    #[test]
    fn test_conflict_same_device() {
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            tagged("a1", AuthorityLevel::Operator, Some(1), 0),
            tagged("a2", AuthorityLevel::Operator, Some(1), 1),
        ];
        let conflicts = resolver.detect_conflicts(&actions);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].conflict_type, ConflictType::DeviceControlConflict);
        assert_eq!(conflicts[0].device_id, 1);
        assert_eq!(conflicts[0].timestamps, vec![0, 1]);
    }

    #[test]
    fn test_different_parameters_do_not_conflict() {
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            tagged_full("a1", AuthorityLevel::Operator, Some(1), 0, None, Some("mw")),
            tagged_full("a2", AuthorityLevel::Operator, Some(1), 1, None, Some("voltage")),
        ];
        // Same device, different parameters → independent, no conflict.
        assert!(resolver.detect_conflicts(&actions).is_empty());
    }

    #[test]
    fn test_same_parameter_conflicts() {
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            tagged_full("a1", AuthorityLevel::Operator, Some(1), 0, None, Some("tap")),
            tagged_full("a2", AuthorityLevel::Operator, Some(1), 1, None, Some("tap")),
        ];
        let conflicts = resolver.detect_conflicts(&actions);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].conflict_type, ConflictType::ParameterConflict);
        assert_eq!(conflicts[0].parameter_key.as_deref(), Some("tap"));
    }

    #[test]
    fn test_multiparty_conflict_detected() {
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            tagged("a1", AuthorityLevel::Operator, Some(1), 0),
            tagged("a2", AuthorityLevel::Operator, Some(1), 1),
            tagged("a3", AuthorityLevel::Supervisor, Some(1), 2),
        ];
        let conflicts = resolver.detect_conflicts(&actions);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].agent_ids.len(), 3);
    }

    // ===== authority resolution =====

    #[test]
    fn test_resolve_by_authority() {
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            tagged("a1", AuthorityLevel::Operator, Some(1), 0),
            tagged("a2", AuthorityLevel::Supervisor, Some(1), 1),
        ];
        let c = resolver.detect_conflicts(&actions).into_iter().next().unwrap();
        let r = resolver.resolve(c).resolution.unwrap();
        assert_eq!(r.winner_id, "a2");
        assert_eq!(r.strategy, ResolutionStrategy::AuthorityPriority);
        assert_eq!(r.loser_ids, vec!["a1".to_string()]);
    }

    #[test]
    fn test_emergency_beats_supervisor() {
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            tagged("a1", AuthorityLevel::Supervisor, Some(1), 0),
            tagged("a2", AuthorityLevel::Emergency, Some(1), 1),
        ];
        let c = resolver.detect_conflicts(&actions).into_iter().next().unwrap();
        let r = resolver.resolve(c).resolution.unwrap();
        assert_eq!(r.winner_id, "a2");
        assert_eq!(r.strategy, ResolutionStrategy::AuthorityPriority);
    }

    #[test]
    fn test_multiparty_authority_winner_drops_all_losers() {
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            tagged("a1", AuthorityLevel::Operator, Some(1), 0),
            tagged("a2", AuthorityLevel::Operator, Some(1), 1),
            tagged("a3", AuthorityLevel::Supervisor, Some(1), 2),
        ];
        let r = resolver.process_with_audit(actions);
        assert_eq!(r.surviving_actions.len(), 1);
        assert_eq!(r.surviving_actions[0].agent_id, "a3");
        assert_eq!(r.audit_trail.len(), 1);
        let a = &r.audit_trail[0];
        assert_eq!(a.winner_id, "a3");
        assert!(a.loser_ids.contains(&"a1".to_string()) && a.loser_ids.contains(&"a2".to_string()));
    }

    // ===== time resolution (previously broken — the core BUG-4 fix) =====

    #[test]
    fn test_resolve_by_time_when_authority_ties() {
        // The historical bug: two equal-authority agents → authority ties →
        // resolve_by_time returned None → forced manual resolution.
        // Now: authority ties, earliest timestamp (50 < 100) wins → a2.
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            tagged("a1", AuthorityLevel::Operator, Some(1), 100),
            tagged("a2", AuthorityLevel::Operator, Some(1), 50),
        ];
        let c = resolver.detect_conflicts(&actions).into_iter().next().unwrap();
        let r = resolver.resolve(c).resolution.unwrap();
        assert_eq!(r.winner_id, "a2"); // earliest timestamp wins
        assert_eq!(r.strategy, ResolutionStrategy::TimePriority);
    }

    #[test]
    fn test_earliest_timestamp_wins() {
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            tagged("late", AuthorityLevel::Operator, Some(1), 999),
            tagged("early", AuthorityLevel::Operator, Some(1), 1),
            tagged("mid", AuthorityLevel::Operator, Some(1), 500),
        ];
        let c = resolver.detect_conflicts(&actions).into_iter().next().unwrap();
        let r = resolver.resolve(c).resolution.unwrap();
        assert_eq!(r.winner_id, "early");
        assert_eq!(r.strategy, ResolutionStrategy::TimePriority);
    }

    // ===== topology proximity =====

    /// Mock provider: hand-authored hop distances for deterministic tests.
    struct MockProximity {
        table: HashMap<(u64, u64), u32>,
    }

    impl ProximityProvider for MockProximity {
        fn hop_distance(&self, from: ElementId, to: ElementId) -> Option<u32> {
            self.table
                .get(&(from, to))
                .copied()
                .or_else(|| self.table.get(&(to, from)).copied())
        }
    }

    #[test]
    fn test_topology_proximity_breaks_time_tie() {
        // Two equal-authority, equal-timestamp agents, but a2 is closer.
        let mut table = HashMap::new();
        table.insert((5, 1), 4u32); // a1 jurisdiction bus 5 → device 1: 4 hops
        table.insert((2, 1), 1u32); // a2 jurisdiction bus 2 → device 1: 1 hop
        let resolver = ActionConflictResolver::with_proximity(Box::new(MockProximity { table }));

        let actions = vec![
            tagged_full("a1", AuthorityLevel::Operator, Some(1), 10, Some(5), None),
            tagged_full("a2", AuthorityLevel::Operator, Some(1), 10, Some(2), None),
        ];
        let c = resolver.detect_conflicts(&actions).into_iter().next().unwrap();
        let r = resolver.resolve(c).resolution.unwrap();
        assert_eq!(r.winner_id, "a2");
        assert_eq!(r.strategy, ResolutionStrategy::TopologyProximity);
    }

    #[test]
    fn test_proximity_tie_defers_to_id_tiebreak() {
        // Equal authority, time, AND equal hop distance → id tie-break.
        let mut table = HashMap::new();
        table.insert((5, 1), 2u32);
        table.insert((6, 1), 2u32);
        let resolver = ActionConflictResolver::with_proximity(Box::new(MockProximity { table }));

        let actions = vec![
            tagged_full("zoe", AuthorityLevel::Operator, Some(1), 10, Some(5), None),
            tagged_full("amy", AuthorityLevel::Operator, Some(1), 10, Some(6), None),
        ];
        let c = resolver.detect_conflicts(&actions).into_iter().next().unwrap();
        let r = resolver.resolve(c).resolution.unwrap();
        assert_eq!(r.winner_id, "amy"); // lexicographically smaller
        assert_eq!(r.strategy, ResolutionStrategy::AgentIdTieBreak);
    }

    #[test]
    fn test_no_proximity_provider_skips_proximity_stage() {
        // Default resolver has NoProximity → equal auth/time falls through to id tie-break.
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            tagged("zoe", AuthorityLevel::Operator, Some(1), 10),
            tagged("amy", AuthorityLevel::Operator, Some(1), 10),
        ];
        let c = resolver.detect_conflicts(&actions).into_iter().next().unwrap();
        let r = resolver.resolve(c).resolution.unwrap();
        assert_eq!(r.winner_id, "amy");
        assert_eq!(r.strategy, ResolutionStrategy::AgentIdTieBreak);
    }

    // ===== manual mode =====

    #[test]
    fn test_manual_only_resolver() {
        let resolver = ActionConflictResolver::manual_only();
        let actions = vec![
            tagged("a1", AuthorityLevel::Supervisor, Some(1), 0),
            tagged("a2", AuthorityLevel::Operator, Some(1), 1),
        ];
        let c = resolver.detect_conflicts(&actions).into_iter().next().unwrap();
        let r = resolver.resolve(c).resolution.unwrap();
        assert_eq!(r.strategy, ResolutionStrategy::ManualResolution);
    }

    // ===== process / audit / cross-device safety =====

    #[test]
    fn test_process_filters_losers() {
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            tagged("a1", AuthorityLevel::Operator, Some(1), 0),
            tagged("a2", AuthorityLevel::Supervisor, Some(1), 1),
            tagged("a3", AuthorityLevel::Operator, Some(2), 2), // different device
        ];
        let result = resolver.process(actions);
        assert_eq!(result.len(), 2);
        let ids: Vec<&str> = result.iter().map(|a| a.agent_id.as_str()).collect();
        assert!(ids.contains(&"a2") && ids.contains(&"a3") && !ids.contains(&"a1"));
    }

    #[test]
    fn test_agent_on_two_devices_only_loses_one() {
        // Regression: the old `process()` matched losers by agent_id alone,
        // so an agent that lost on device 1 could be wrongly stripped from a
        // *different* non-conflicting device it also controlled.
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            // a1 loses device 1 to a2 (supervisor).
            tagged("a1", AuthorityLevel::Operator, Some(1), 0),
            tagged("a2", AuthorityLevel::Supervisor, Some(1), 1),
            // a1 ALSO acts on device 2 uncontested — must survive.
            tagged("a1", AuthorityLevel::Operator, Some(2), 2),
        ];
        let result = resolver.process(actions);
        // Survivors: a2 (device1 winner) + a1 (device2 uncontested) = 2 actions.
        assert_eq!(result.len(), 2);
        let devices_for_a1: Vec<u64> = result
            .iter()
            .filter(|a| a.agent_id == "a1")
            .filter_map(|a| a.target_device_id)
            .collect();
        assert_eq!(devices_for_a1, vec![2], "a1 must survive on device 2");
    }

    #[test]
    fn test_same_agent_different_parameters_both_survive() {
        // Same agent, same device, different parameters — both independent.
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            tagged_full("a1", AuthorityLevel::Operator, Some(1), 0, None, Some("mw")),
            tagged_full("a1", AuthorityLevel::Operator, Some(1), 1, None, Some("voltage")),
        ];
        let result = resolver.process(actions);
        assert_eq!(result.len(), 2, "different parameters are not a conflict");
    }

    #[test]
    fn test_audit_trail_recorded() {
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            tagged("a1", AuthorityLevel::Operator, Some(1), 0),
            tagged("a2", AuthorityLevel::Supervisor, Some(1), 1),
        ];
        let r = resolver.process_with_audit(actions);
        assert_eq!(r.audit_trail.len(), 1);
        let a = &r.audit_trail[0];
        assert_eq!(a.device_id, 1);
        assert_eq!(a.strategy, ResolutionStrategy::AuthorityPriority);
        assert_eq!(a.winner_id, "a2");
        assert_eq!(a.loser_ids, vec!["a1".to_string()]);
        assert!(!a.reason.is_empty());
    }

    #[test]
    fn test_no_target_action_survives() {
        // Actions without a target device are never conflicting and always pass through.
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            tagged("a1", AuthorityLevel::Operator, None, 0),
            tagged("a2", AuthorityLevel::Operator, None, 1),
        ];
        let result = resolver.process(actions);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_default_resolver() {
        let resolver = ActionConflictResolver::default();
        let actions = vec![tagged("a1", AuthorityLevel::Operator, None, 0)];
        assert!(resolver.detect_conflicts(&actions).is_empty());
    }
}
