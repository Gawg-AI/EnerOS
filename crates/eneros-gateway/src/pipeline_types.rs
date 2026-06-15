use eneros_core::{
    AuthorityLevel, Jurisdiction, PowerObservation, StructuredAction, SystemOperatingState,
};
use eneros_constraint::projector::ProjectionResult;
use crate::interlocking::DeviceStates;

/// Complete decision context carried through the pipeline.
/// Provides all the information each stage needs to make deterministic decisions.
#[derive(Debug, Clone)]
pub struct DecisionContext {
    /// Who is requesting this action
    pub authority: AuthorityLevel,
    /// What scope the agent has
    pub jurisdiction: Jurisdiction,
    /// Current system operating state
    pub system_state: SystemOperatingState,
    /// Current power system observation (voltages, flows, frequency)
    pub observation: Option<PowerObservation>,
    /// Current device states for interlocking checks
    pub device_states: Option<DeviceStates>,
    /// Agent ID that proposed the action
    pub agent_id: String,
    /// Reasoning or justification for the action
    pub reasoning: String,
}

impl DecisionContext {
    /// Create a minimal context
    pub fn new(
        authority: AuthorityLevel,
        jurisdiction: Jurisdiction,
        system_state: SystemOperatingState,
    ) -> Self {
        Self {
            authority,
            jurisdiction,
            system_state,
            observation: None,
            device_states: None,
            agent_id: String::new(),
            reasoning: String::new(),
        }
    }

    /// Builder: attach power observation
    pub fn with_observation(mut self, obs: PowerObservation) -> Self {
        self.observation = Some(obs);
        self
    }

    /// Builder: attach device states
    pub fn with_device_states(mut self, states: DeviceStates) -> Self {
        self.device_states = Some(states);
        self
    }

    /// Builder: set agent ID
    pub fn with_agent_id(mut self, agent_id: &str) -> Self {
        self.agent_id = agent_id.to_string();
        self
    }

    /// Builder: set reasoning
    pub fn with_reasoning(mut self, reasoning: &str) -> Self {
        self.reasoning = reasoning.to_string();
        self
    }

    /// Effective authority considering system state
    pub fn effective_authority(&self) -> AuthorityLevel {
        self.authority.effective_level(self.system_state.is_emergency())
    }
}

/// Pre-condition check result
#[derive(Debug, Clone)]
pub struct PreConditionResult {
    /// Whether all pre-conditions are satisfied
    pub satisfied: bool,
    /// Individual check results
    pub checks: Vec<PreConditionCheck>,
    /// Summary of failures
    pub failure_summary: Vec<String>,
}

/// Individual pre-condition check
#[derive(Debug, Clone)]
pub struct PreConditionCheck {
    /// Name of the check
    pub name: String,
    /// Whether the check passed
    pub passed: bool,
    /// Description of what was checked
    pub description: String,
    /// Reason for failure (if failed)
    pub failure_reason: Option<String>,
}

impl PreConditionResult {
    pub fn passed() -> Self {
        Self {
            satisfied: true,
            checks: Vec::new(),
            failure_summary: Vec::new(),
        }
    }

    pub fn failed(checks: Vec<PreConditionCheck>) -> Self {
        let failure_summary: Vec<String> = checks
            .iter()
            .filter(|c| !c.passed)
            .filter_map(|c| c.failure_reason.clone())
            .collect();
        let satisfied = checks.iter().all(|c| c.passed);
        Self {
            satisfied,
            checks,
            failure_summary,
        }
    }

    pub fn add_check(&mut self, check: PreConditionCheck) {
        if !check.passed {
            self.satisfied = false;
            if let Some(ref reason) = check.failure_reason {
                self.failure_summary.push(reason.clone());
            }
        }
        self.checks.push(check);
    }
}

/// Post-condition verification result
#[derive(Debug, Clone)]
pub struct PostConditionResult {
    /// Whether all post-conditions are satisfied after the action
    pub satisfied: bool,
    /// Individual verification results
    pub verifications: Vec<PostConditionVerification>,
    /// New violations introduced by the action
    pub new_violations: Vec<String>,
    /// Worsened existing violations
    pub worsened_violations: Vec<String>,
}

/// Individual post-condition verification
#[derive(Debug, Clone)]
pub struct PostConditionVerification {
    /// Name of the verification
    pub name: String,
    /// Whether the verification passed
    pub passed: bool,
    /// Description of the result
    pub description: String,
}

impl PostConditionResult {
    pub fn satisfied(verifications: Vec<PostConditionVerification>) -> Self {
        let new_violations = Vec::new();
        let worsened_violations = Vec::new();
        let satisfied = verifications.iter().all(|v| v.passed);
        Self {
            satisfied,
            verifications,
            new_violations,
            worsened_violations,
        }
    }

    pub fn with_violations(
        verifications: Vec<PostConditionVerification>,
        new_violations: Vec<String>,
        worsened_violations: Vec<String>,
    ) -> Self {
        let satisfied = verifications.iter().all(|v| v.passed)
            && new_violations.is_empty()
            && worsened_violations.is_empty();
        Self {
            satisfied,
            verifications,
            new_violations,
            worsened_violations,
        }
    }
}

/// Decomposed action step
#[derive(Debug, Clone)]
pub struct ActionStep {
    /// Step index (0-based)
    pub step_index: usize,
    /// The action for this step
    pub action: StructuredAction,
    /// Description of what this step does
    pub description: String,
    /// Whether this step is critical (failure stops the sequence)
    pub critical: bool,
    /// Estimated execution time in milliseconds
    pub estimated_duration_ms: u64,
}

/// Action decomposition result
#[derive(Debug, Clone)]
pub struct DecomposedAction {
    /// Original composite action
    pub original: StructuredAction,
    /// Ordered sequence of steps
    pub steps: Vec<ActionStep>,
    /// Whether the steps must be executed atomically (all-or-nothing)
    pub atomic: bool,
    /// Description of the decomposition
    pub description: String,
}

impl DecomposedAction {
    /// Create a single-step decomposition (no decomposition needed)
    pub fn single(action: StructuredAction) -> Self {
        Self {
            original: action.clone(),
            steps: vec![ActionStep {
                step_index: 0,
                action,
                description: "Direct execution".to_string(),
                critical: true,
                estimated_duration_ms: 100,
            }],
            atomic: false,
            description: "Single action, no decomposition needed".to_string(),
        }
    }

    /// Get the total number of steps
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    /// Check if this is a multi-step action
    pub fn is_multi_step(&self) -> bool {
        self.steps.len() > 1
    }
}

/// Rollback step — inverse of an ActionStep
#[derive(Debug, Clone)]
pub struct RollbackStep {
    /// The action to undo
    pub undo_action: StructuredAction,
    /// Description of the rollback
    pub description: String,
    /// Step index this rollback corresponds to
    pub for_step_index: usize,
}

/// Rollback plan for a decomposed action
#[derive(Debug, Clone)]
pub struct RollbackPlan {
    /// Rollback steps in reverse order of execution
    pub steps: Vec<RollbackStep>,
    /// Whether automatic rollback is allowed
    pub auto_rollback: bool,
    /// Maximum time to wait before triggering rollback (ms)
    pub timeout_ms: u64,
    /// Description of the rollback strategy
    pub strategy: RollbackStrategy,
}

/// Rollback strategy
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RollbackStrategy {
    /// Rollback all completed steps in reverse order
    FullRollback,
    /// Only rollback the failed step and continue
    PartialRollback,
    /// No automatic rollback — require manual intervention
    ManualOnly,
    /// Best-effort rollback — skip steps that fail to rollback
    BestEffort,
}

impl RollbackPlan {
    /// Create a no-rollback plan (manual intervention required)
    pub fn manual_only() -> Self {
        Self {
            steps: Vec::new(),
            auto_rollback: false,
            timeout_ms: 0,
            strategy: RollbackStrategy::ManualOnly,
        }
    }

    /// Create a full rollback plan
    pub fn full_rollback(steps: Vec<RollbackStep>) -> Self {
        Self {
            steps,
            auto_rollback: true,
            timeout_ms: 5000,
            strategy: RollbackStrategy::FullRollback,
        }
    }

    /// Check if rollback is possible
    pub fn can_auto_rollback(&self) -> bool {
        self.auto_rollback && !self.steps.is_empty()
    }

    /// Get rollback steps for a given failure point
    pub fn rollback_from(&self, failed_step_index: usize) -> Vec<&RollbackStep> {
        self.steps
            .iter()
            .filter(|s| s.for_step_index < failed_step_index)
            .collect()
    }
}

/// Pipeline statistics
#[derive(Debug, Clone, Default)]
pub struct PipelineStatistics {
    /// Total decisions processed
    pub total_decisions: u64,
    /// Decisions that were approved
    pub approved: u64,
    /// Decisions that were rejected
    pub rejected: u64,
    /// Decisions that were projected (modified)
    pub projected: u64,
    /// Decisions that required emergency bypass
    pub emergency_bypassed: u64,
    /// Decisions that required approval
    pub pending_approval: u64,
    /// Pre-condition check failures
    pub precondition_failures: u64,
    /// Post-condition verification failures
    pub postcondition_failures: u64,
    /// Rollbacks triggered
    pub rollbacks_triggered: u64,
    /// Average decision latency in microseconds
    pub avg_latency_us: u64,
    /// Total latency in microseconds
    pub total_latency_us: u64,
    /// Maximum latency in microseconds
    pub max_latency_us: u64,
}

impl PipelineStatistics {
    pub fn record_decision(&mut self, latency_us: u64) {
        self.total_decisions += 1;
        self.total_latency_us += latency_us;
        self.max_latency_us = self.max_latency_us.max(latency_us);
        if self.total_decisions > 0 {
            self.avg_latency_us = self.total_latency_us.checked_div(self.total_decisions).unwrap_or(0);
        }
    }
}

/// Enhanced pipeline decision result
#[derive(Debug, Clone)]
pub struct EnhancedPipelineDecision {
    /// The action that was actually executed (None if rejected)
    pub executed_action: Option<StructuredAction>,
    /// Original action proposed
    pub original_action: StructuredAction,
    /// Decomposed action steps (if multi-step)
    pub decomposition: Option<DecomposedAction>,
    /// Projection result from feasibility projector
    pub projection: ProjectionResult,
    /// Pre-condition check result
    pub pre_conditions: PreConditionResult,
    /// Post-condition verification result (if action was executed)
    pub post_conditions: Option<PostConditionResult>,
    /// Validation verdict
    pub verdict: eneros_core::ActionVerdict,
    /// Rollback plan (if applicable)
    pub rollback_plan: Option<RollbackPlan>,
    /// Audit trail from each pipeline stage
    pub audit: Vec<PipelineAuditEntry>,
    /// Total pipeline latency in microseconds
    pub total_latency_us: u64,
}

/// Single audit entry in the pipeline
#[derive(Debug, Clone)]
pub struct PipelineAuditEntry {
    /// Stage name
    pub stage: String,
    /// Description of what happened at this stage
    pub description: String,
    /// Duration of this stage in microseconds
    pub duration_us: u64,
    /// Whether this stage passed or failed
    pub passed: bool,
}
