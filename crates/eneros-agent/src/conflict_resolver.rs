use std::collections::HashMap;
use eneros_core::AuthorityLevel;
use serde::{Deserialize, Serialize};
use crate::agent::AgentAction;

/// Type of action conflict
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictType {
    /// Two agents want to control the same device in opposite ways
    DeviceControlConflict,
    /// Two agents want to set conflicting parameters
    ParameterConflict,
    /// Two agents want to execute mutually exclusive operations
    MutualExclusion,
}

/// A detected conflict between agent actions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionConflict {
    /// IDs of the agents involved in the conflict
    pub agent_ids: Vec<String>,
    /// Authority levels of the involved agents
    pub authority_levels: Vec<AuthorityLevel>,
    /// The conflicting actions
    pub conflicting_actions: Vec<AgentAction>,
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
    /// Manual resolution required
    ManualResolution,
}

/// Agent action with metadata for conflict detection
#[derive(Debug, Clone)]
pub struct TaggedAction {
    /// Agent ID that produced this action
    pub agent_id: String,
    /// Agent's authority level
    pub authority_level: AuthorityLevel,
    /// The action itself
    pub action: AgentAction,
    /// Target device ID (if applicable)
    pub target_device_id: Option<u64>,
    /// Timestamp of the action
    pub timestamp: u64,
}

/// Action conflict resolver — detects and resolves conflicts between agent actions
pub struct ActionConflictResolver {
    /// Whether to automatically resolve conflicts
    auto_resolve: bool,
}

impl ActionConflictResolver {
    /// Create a new resolver with auto-resolution enabled
    pub fn new() -> Self {
        Self { auto_resolve: true }
    }

    /// Create a resolver without auto-resolution (conflicts require manual resolution)
    pub fn manual_only() -> Self {
        Self { auto_resolve: false }
    }

    /// Detect conflicts among a set of tagged actions
    pub fn detect_conflicts(&self, actions: &[TaggedAction]) -> Vec<ActionConflict> {
        let mut conflicts = Vec::new();

        // Group actions by target device
        let mut device_actions: HashMap<u64, Vec<&TaggedAction>> = HashMap::new();
        for action in actions {
            if let Some(device_id) = action.target_device_id {
                device_actions.entry(device_id).or_default().push(action);
            }
        }

        // Check for conflicts on same device
        for (device_id, device_action_list) in &device_actions {
            if device_action_list.len() > 1 {
                // Multiple agents acting on the same device — potential conflict
                let agent_ids: Vec<String> = device_action_list.iter().map(|a| a.agent_id.clone()).collect();
                let authority_levels: Vec<AuthorityLevel> = device_action_list.iter().map(|a| a.authority_level).collect();
                let conflicting_actions: Vec<AgentAction> = device_action_list.iter().map(|a| a.action.clone()).collect();

                let conflict = ActionConflict {
                    agent_ids,
                    authority_levels,
                    conflicting_actions,
                    conflict_type: ConflictType::DeviceControlConflict,
                    description: format!("Multiple agents targeting device {}", device_id),
                    resolution: None,
                };
                conflicts.push(conflict);
            }
        }

        conflicts
    }

    /// Resolve a conflict using the appropriate strategy
    pub fn resolve(&self, conflict: ActionConflict) -> ActionConflict {
        if !self.auto_resolve {
            return ActionConflict {
                resolution: Some(ConflictResolution {
                    winner_id: String::new(),
                    winning_action: AgentAction::NoOp,
                    strategy: ResolutionStrategy::ManualResolution,
                    reason: "Auto-resolution disabled, manual resolution required".to_string(),
                }),
                ..conflict
            };
        }

        // Strategy 1: Authority priority — higher authority wins
        if let Some(resolution) = self.resolve_by_authority(&conflict) {
            return ActionConflict {
                resolution: Some(resolution),
                ..conflict
            };
        }

        // Strategy 2: Time priority — first action wins
        if let Some(resolution) = self.resolve_by_time(&conflict) {
            return ActionConflict {
                resolution: Some(resolution),
                ..conflict
            };
        }

        // Fallback: manual resolution
        ActionConflict {
            resolution: Some(ConflictResolution {
                winner_id: String::new(),
                winning_action: AgentAction::NoOp,
                strategy: ResolutionStrategy::ManualResolution,
                reason: "Could not auto-resolve conflict".to_string(),
            }),
            ..conflict
        }
    }

    /// Resolve by authority level — highest authority wins
    fn resolve_by_authority(&self, conflict: &ActionConflict) -> Option<ConflictResolution> {
        let max_auth = conflict.authority_levels.iter().max()?;
        let winner_idx = conflict.authority_levels.iter().position(|a| a == max_auth)?;

        // Only resolve if there's a clear winner (unique max authority)
        let max_count = conflict.authority_levels.iter().filter(|a| *a == max_auth).count();
        if max_count > 1 {
            return None; // Tie — cannot resolve by authority alone
        }

        Some(ConflictResolution {
            winner_id: conflict.agent_ids[winner_idx].clone(),
            winning_action: conflict.conflicting_actions[winner_idx].clone(),
            strategy: ResolutionStrategy::AuthorityPriority,
            reason: format!("Agent {} has highest authority level ({:?})", conflict.agent_ids[winner_idx], max_auth),
        })
    }

    /// Resolve by time priority — earliest action wins
    fn resolve_by_time(&self, _conflict: &ActionConflict) -> Option<ConflictResolution> {
        // In a real implementation, we'd use the timestamp field
        // For now, return None to indicate this strategy isn't available
        None
    }

    /// Process a batch of tagged actions: detect conflicts, resolve, return final actions
    pub fn process(&self, actions: Vec<TaggedAction>) -> Vec<TaggedAction> {
        let conflicts = self.detect_conflicts(&actions);

        if conflicts.is_empty() {
            return actions;
        }

        // Resolve all conflicts
        let resolved: Vec<ActionConflict> = conflicts.into_iter().map(|c| self.resolve(c)).collect();

        // Collect winning agent IDs
        let _winning_agents: Vec<&str> = resolved.iter()
            .filter_map(|c| c.resolution.as_ref().map(|r| r.winner_id.as_str()))
            .collect();

        // Filter out actions from losing agents in conflicts
        let mut result = Vec::new();
        for action in actions {
            let is_loser = resolved.iter().any(|c| {
                c.resolution.as_ref().is_some_and(|r| {
                    // The action's agent is in the conflict but is not the winner
                    c.agent_ids.contains(&action.agent_id) && r.winner_id != action.agent_id
                })
            });
            if !is_loser {
                result.push(action);
            }
        }

        result
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

    fn make_tagged_action(agent_id: &str, authority: AuthorityLevel, device_id: Option<u64>) -> TaggedAction {
        TaggedAction {
            agent_id: agent_id.to_string(),
            authority_level: authority,
            action: AgentAction::NoOp,
            target_device_id: device_id,
            timestamp: 0,
        }
    }

    #[test]
    fn test_no_conflict_single_action() {
        let resolver = ActionConflictResolver::new();
        let actions = vec![make_tagged_action("a1", AuthorityLevel::Operator, Some(1))];
        let conflicts = resolver.detect_conflicts(&actions);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_no_conflict_different_devices() {
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            make_tagged_action("a1", AuthorityLevel::Operator, Some(1)),
            make_tagged_action("a2", AuthorityLevel::Operator, Some(2)),
        ];
        let conflicts = resolver.detect_conflicts(&actions);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_conflict_same_device() {
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            make_tagged_action("a1", AuthorityLevel::Operator, Some(1)),
            make_tagged_action("a2", AuthorityLevel::Operator, Some(1)),
        ];
        let conflicts = resolver.detect_conflicts(&actions);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].conflict_type, ConflictType::DeviceControlConflict);
    }

    #[test]
    fn test_resolve_by_authority() {
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            make_tagged_action("a1", AuthorityLevel::Operator, Some(1)),
            make_tagged_action("a2", AuthorityLevel::Supervisor, Some(1)),
        ];
        let conflicts = resolver.detect_conflicts(&actions);
        let resolved = resolver.resolve(conflicts.into_iter().next().unwrap());
        assert!(resolved.resolution.is_some());
        let r = resolved.resolution.unwrap();
        assert_eq!(r.winner_id, "a2");
        assert_eq!(r.strategy, ResolutionStrategy::AuthorityPriority);
    }

    #[test]
    fn test_resolve_tie_goes_manual() {
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            make_tagged_action("a1", AuthorityLevel::Operator, Some(1)),
            make_tagged_action("a2", AuthorityLevel::Operator, Some(1)),
        ];
        let conflicts = resolver.detect_conflicts(&actions);
        let resolved = resolver.resolve(conflicts.into_iter().next().unwrap());
        let r = resolved.resolution.unwrap();
        assert_eq!(r.strategy, ResolutionStrategy::ManualResolution);
    }

    #[test]
    fn test_manual_only_resolver() {
        let resolver = ActionConflictResolver::manual_only();
        let actions = vec![
            make_tagged_action("a1", AuthorityLevel::Supervisor, Some(1)),
            make_tagged_action("a2", AuthorityLevel::Operator, Some(1)),
        ];
        let conflicts = resolver.detect_conflicts(&actions);
        let resolved = resolver.resolve(conflicts.into_iter().next().unwrap());
        let r = resolved.resolution.unwrap();
        assert_eq!(r.strategy, ResolutionStrategy::ManualResolution);
    }

    #[test]
    fn test_process_filters_losers() {
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            make_tagged_action("a1", AuthorityLevel::Operator, Some(1)),
            make_tagged_action("a2", AuthorityLevel::Supervisor, Some(1)),
            make_tagged_action("a3", AuthorityLevel::Operator, Some(2)), // different device, no conflict
        ];
        let result = resolver.process(actions);
        // a2 wins over a1 for device 1, a3 is unaffected
        assert_eq!(result.len(), 2);
        let agent_ids: Vec<&str> = result.iter().map(|a| a.agent_id.as_str()).collect();
        assert!(agent_ids.contains(&"a2"));
        assert!(agent_ids.contains(&"a3"));
    }

    #[test]
    fn test_emergency_beats_supervisor() {
        let resolver = ActionConflictResolver::new();
        let actions = vec![
            make_tagged_action("a1", AuthorityLevel::Supervisor, Some(1)),
            make_tagged_action("a2", AuthorityLevel::Emergency, Some(1)),
        ];
        let conflicts = resolver.detect_conflicts(&actions);
        let resolved = resolver.resolve(conflicts.into_iter().next().unwrap());
        let r = resolved.resolution.unwrap();
        assert_eq!(r.winner_id, "a2");
    }

    #[test]
    fn test_default_resolver() {
        let resolver = ActionConflictResolver::default();
        let actions = vec![make_tagged_action("a1", AuthorityLevel::Operator, None)];
        let conflicts = resolver.detect_conflicts(&actions);
        assert!(conflicts.is_empty());
    }
}
