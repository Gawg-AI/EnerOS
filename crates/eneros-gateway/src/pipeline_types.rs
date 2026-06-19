use std::sync::atomic::{AtomicU64, Ordering};
use eneros_core::{
    ActionVerdict, AuthorityLevel, Jurisdiction, PowerObservation, StructuredAction,
    SystemOperatingState, DecisionContextCore, DecisionResultCore,
};
use eneros_constraint::projector::ProjectionResult;
use crate::interlocking::DeviceStates;

// Re-export PipelineAuditEntry from eneros-core (shared IPC schema).
pub use eneros_core::pipeline_types::PipelineAuditEntry;

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

/// Pipeline statistics (atomic version, v0.8.0 — M5).
///
/// All counters are stored as `AtomicU64` so they can be updated from
/// multiple threads without a surrounding `RwLock`. Use [`snapshot()`](Self::snapshot)
/// to obtain a plain `u64`-fielded [`PipelineStatisticsSnapshot`] for
/// serialization / display.
#[derive(Debug, Default)]
pub struct PipelineStatistics {
    /// Total decisions processed
    pub total_decisions: AtomicU64,
    /// Decisions that were approved
    pub approved: AtomicU64,
    /// Decisions that were rejected
    pub rejected: AtomicU64,
    /// Decisions that were projected (modified)
    pub projected: AtomicU64,
    /// Decisions that required emergency bypass
    pub emergency_bypassed: AtomicU64,
    /// Decisions that required approval
    pub pending_approval: AtomicU64,
    /// Pre-condition check failures
    pub precondition_failures: AtomicU64,
    /// Post-condition verification failures
    pub postcondition_failures: AtomicU64,
    /// Rollbacks triggered
    pub rollbacks_triggered: AtomicU64,
    /// Rollbacks that completed successfully (v0.6.0)
    pub rollbacks_succeeded: AtomicU64,
    /// Rollbacks that failed (v0.6.0)
    pub rollbacks_failed: AtomicU64,
    /// Average decision latency in microseconds
    pub avg_latency_us: AtomicU64,
    /// Total latency in microseconds
    pub total_latency_us: AtomicU64,
    /// Maximum latency in microseconds
    pub max_latency_us: AtomicU64,
}

/// Immutable snapshot of [`PipelineStatistics`] with plain `u64` fields.
///
/// Produced by [`PipelineStatistics::snapshot()`]. Suitable for
/// serialization and JSON export (field names match the legacy struct).
#[derive(Debug, Clone, Default)]
pub struct PipelineStatisticsSnapshot {
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
    /// Rollbacks that completed successfully (v0.6.0)
    pub rollbacks_succeeded: u64,
    /// Rollbacks that failed (v0.6.0)
    pub rollbacks_failed: u64,
    /// Average decision latency in microseconds
    pub avg_latency_us: u64,
    /// Total latency in microseconds
    pub total_latency_us: u64,
    /// Maximum latency in microseconds
    pub max_latency_us: u64,
}

impl PipelineStatistics {
    /// Record a decision's latency and update derived counters.
    ///
    /// Uses `Ordering::Relaxed` for all atomic operations — these are
    /// best-effort statistics counters and do not require strong ordering.
    pub fn record_decision(&self, latency_us: u64) {
        let total = self.total_decisions.fetch_add(1, Ordering::Relaxed) + 1;
        let total_latency = self.total_latency_us.fetch_add(latency_us, Ordering::Relaxed) + latency_us;
        self.max_latency_us.fetch_max(latency_us, Ordering::Relaxed);
        let avg = total_latency.checked_div(total).unwrap_or(0);
        self.avg_latency_us.store(avg, Ordering::Relaxed);
    }

    /// Atomically reset all counters to zero.
    pub fn reset(&self) {
        self.total_decisions.store(0, Ordering::Relaxed);
        self.approved.store(0, Ordering::Relaxed);
        self.rejected.store(0, Ordering::Relaxed);
        self.projected.store(0, Ordering::Relaxed);
        self.emergency_bypassed.store(0, Ordering::Relaxed);
        self.pending_approval.store(0, Ordering::Relaxed);
        self.precondition_failures.store(0, Ordering::Relaxed);
        self.postcondition_failures.store(0, Ordering::Relaxed);
        self.rollbacks_triggered.store(0, Ordering::Relaxed);
        self.rollbacks_succeeded.store(0, Ordering::Relaxed);
        self.rollbacks_failed.store(0, Ordering::Relaxed);
        self.avg_latency_us.store(0, Ordering::Relaxed);
        self.total_latency_us.store(0, Ordering::Relaxed);
        self.max_latency_us.store(0, Ordering::Relaxed);
    }

    /// Take a consistent-ish snapshot of all counters as plain `u64`s.
    ///
    /// Each field is loaded independently with `Ordering::Relaxed`, so the
    /// snapshot is not atomic across fields — concurrent updates may be
    /// partially reflected. This is acceptable for statistics display.
    pub fn snapshot(&self) -> PipelineStatisticsSnapshot {
        PipelineStatisticsSnapshot {
            total_decisions: self.total_decisions.load(Ordering::Relaxed),
            approved: self.approved.load(Ordering::Relaxed),
            rejected: self.rejected.load(Ordering::Relaxed),
            projected: self.projected.load(Ordering::Relaxed),
            emergency_bypassed: self.emergency_bypassed.load(Ordering::Relaxed),
            pending_approval: self.pending_approval.load(Ordering::Relaxed),
            precondition_failures: self.precondition_failures.load(Ordering::Relaxed),
            postcondition_failures: self.postcondition_failures.load(Ordering::Relaxed),
            rollbacks_triggered: self.rollbacks_triggered.load(Ordering::Relaxed),
            rollbacks_succeeded: self.rollbacks_succeeded.load(Ordering::Relaxed),
            rollbacks_failed: self.rollbacks_failed.load(Ordering::Relaxed),
            avg_latency_us: self.avg_latency_us.load(Ordering::Relaxed),
            total_latency_us: self.total_latency_us.load(Ordering::Relaxed),
            max_latency_us: self.max_latency_us.load(Ordering::Relaxed),
        }
    }
}

/// Result of executing a rollback plan (v0.6.0 — S6).
///
/// When post-conditions fail and `RollbackPlan::can_auto_rollback()` is true,
/// the pipeline automatically executes the rollback steps in reverse order.
/// This struct records the outcome of that execution.
#[derive(Debug, Clone)]
pub struct RollbackExecution {
    /// Whether the overall rollback completed successfully
    pub succeeded: bool,
    /// Number of rollback steps attempted
    pub steps_attempted: usize,
    /// Number of rollback steps that succeeded
    pub steps_succeeded: usize,
    /// Error message if the rollback failed (None on full success)
    pub error: Option<String>,
    /// Total rollback execution duration in microseconds
    pub duration_us: u64,
}

impl RollbackExecution {
    /// Create a successful rollback execution result.
    pub fn success(steps: usize, duration_us: u64) -> Self {
        Self {
            succeeded: true,
            steps_attempted: steps,
            steps_succeeded: steps,
            error: None,
            duration_us,
        }
    }

    /// Create a failed rollback execution result.
    pub fn failure(steps_attempted: usize, steps_succeeded: usize, error: String, duration_us: u64) -> Self {
        Self {
            succeeded: false,
            steps_attempted,
            steps_succeeded,
            error: Some(error),
            duration_us,
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
    /// Rollback execution result (v0.6.0 — S6).
    /// Set when post-conditions fail and auto-rollback was triggered.
    pub rollback_executed: Option<RollbackExecution>,
    /// Audit trail from each pipeline stage
    pub audit: Vec<PipelineAuditEntry>,
    /// Total pipeline latency in microseconds
    pub total_latency_us: u64,
}

impl From<&DecisionContext> for DecisionContextCore {
    fn from(ctx: &DecisionContext) -> Self {
        Self {
            authority: ctx.authority,
            jurisdiction: ctx.jurisdiction.clone(),
            system_state: ctx.system_state,
            observation: ctx.observation.clone(),
            agent_id: ctx.agent_id.clone(),
            reasoning: ctx.reasoning.clone(),
        }
    }
}

impl From<&DecisionContextCore> for DecisionContext {
    /// 从 IPC 友好的 `DecisionContextCore` 重建 `DecisionContext`。
    ///
    /// `DecisionContext` 比 `DecisionContextCore` 多出 `device_states` 字段
    /// （来自 `interlocking`，不可跨 crate 序列化），此处默认为 `None`。
    /// 调用方如需进行 interlocking 检查，应在重建后通过
    /// `with_device_states()` 显式注入设备状态。
    fn from(core: &DecisionContextCore) -> Self {
        Self {
            authority: core.authority,
            jurisdiction: core.jurisdiction.clone(),
            system_state: core.system_state,
            observation: core.observation.clone(),
            device_states: None,
            agent_id: core.agent_id.clone(),
            reasoning: core.reasoning.clone(),
        }
    }
}

impl From<&EnhancedPipelineDecision> for DecisionResultCore {
    fn from(decision: &EnhancedPipelineDecision) -> Self {
        let error = match &decision.verdict {
            ActionVerdict::Rejected(reason) => Some(reason.clone()),
            _ => None,
        };
        Self {
            executed: decision.executed_action.is_some(),
            original_action: decision.original_action.clone(),
            executed_action: decision.executed_action.clone(),
            verdict: decision.verdict.clone(),
            audit: decision.audit.clone(),
            total_latency_us: decision.total_latency_us,
            error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_pipeline_statistics_default_is_zero() {
        let stats = PipelineStatistics::default();
        let snap = stats.snapshot();
        assert_eq!(snap.total_decisions, 0);
        assert_eq!(snap.approved, 0);
        assert_eq!(snap.rollbacks_triggered, 0);
        assert_eq!(snap.max_latency_us, 0);
    }

    #[test]
    fn test_record_decision_updates_latency() {
        let stats = PipelineStatistics::default();
        stats.record_decision(100);
        stats.record_decision(300);
        let snap = stats.snapshot();
        assert_eq!(snap.total_decisions, 2);
        assert_eq!(snap.total_latency_us, 400);
        assert_eq!(snap.max_latency_us, 300);
        assert_eq!(snap.avg_latency_us, 200);
    }

    #[test]
    fn test_reset_clears_all_counters() {
        let stats = PipelineStatistics::default();
        stats.record_decision(50);
        stats.approved.fetch_add(3, Ordering::Relaxed);
        stats.reset();
        let snap = stats.snapshot();
        assert_eq!(snap.total_decisions, 0);
        assert_eq!(snap.approved, 0);
        assert_eq!(snap.total_latency_us, 0);
    }

    /// Concurrency test: 8 threads × 1000 fetch_add on `total_decisions`.
    /// The final value must be exactly 8000.
    #[test]
    fn test_concurrent_fetch_add_total_decisions() {
        let stats = Arc::new(PipelineStatistics::default());
        let mut handles = Vec::new();
        for _ in 0..8 {
            let s = Arc::clone(&stats);
            handles.push(thread::spawn(move || {
                for _ in 0..1000 {
                    s.total_decisions.fetch_add(1, Ordering::Relaxed);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(stats.snapshot().total_decisions, 8000);
    }

    /// Concurrency test: `snapshot()` must not panic while other threads
    /// are concurrently updating the counters.
    #[test]
    fn test_snapshot_does_not_panic_under_concurrent_updates() {
        let stats = Arc::new(PipelineStatistics::default());
        let mut handles = Vec::new();

        // Writer threads: hammer every counter.
        for _ in 0..4 {
            let s = Arc::clone(&stats);
            handles.push(thread::spawn(move || {
                for i in 0..1000u64 {
                    s.total_decisions.fetch_add(1, Ordering::Relaxed);
                    s.approved.fetch_add(1, Ordering::Relaxed);
                    s.rejected.fetch_add(1, Ordering::Relaxed);
                    s.total_latency_us.fetch_add(i, Ordering::Relaxed);
                    s.max_latency_us.fetch_max(i, Ordering::Relaxed);
                }
            }));
        }

        // Reader thread: repeatedly snapshot while writers run.
        let s = Arc::clone(&stats);
        handles.push(thread::spawn(move || {
            for _ in 0..1000 {
                let snap = s.snapshot();
                // Invariants that must always hold even mid-update:
                // - max_latency_us cannot exceed the largest value written (999)
                assert!(snap.max_latency_us <= 999 || snap.max_latency_us == 0);
                // - total_decisions cannot exceed total writes (4 * 1000)
                assert!(snap.total_decisions <= 4000);
            }
        }));

        for h in handles {
            h.join().unwrap();
        }
        // Final state is deterministic.
        let final_snap = stats.snapshot();
        assert_eq!(final_snap.total_decisions, 4000);
        assert_eq!(final_snap.approved, 4000);
        assert_eq!(final_snap.max_latency_us, 999);
    }
}
