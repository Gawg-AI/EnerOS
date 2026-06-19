use serde::{Deserialize, Serialize};

use crate::agentos_types::{
    ActionVerdict, AuthorityLevel, Jurisdiction, PowerObservation, StructuredAction,
    SystemOperatingState,
};

/// Single audit entry in the pipeline
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Serializable subset of `DecisionContext` for IPC.
///
/// The full `DecisionContext` (in eneros-gateway) carries `DeviceStates` and
/// other non-serializable / cross-crate fields. This core variant keeps only
/// the fields that can be serialized for inter-process communication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionContextCore {
    /// Who is requesting this action
    pub authority: AuthorityLevel,
    /// What scope the agent has
    pub jurisdiction: Jurisdiction,
    /// Current system operating state
    pub system_state: SystemOperatingState,
    /// Current power system observation (voltages, flows, frequency)
    pub observation: Option<PowerObservation>,
    /// Agent ID that proposed the action
    pub agent_id: String,
    /// Reasoning or justification for the action
    pub reasoning: String,
}

/// Serializable subset of `EnhancedPipelineDecision` for IPC.
///
/// The full `EnhancedPipelineDecision` (in eneros-gateway) carries
/// `DecomposedAction`, `ProjectionResult`, `PreConditionResult`, etc. which
/// would create circular dependencies if moved to eneros-core. This core
/// variant keeps only the serializable fields needed for inter-process
/// communication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionResultCore {
    /// Whether the action was executed
    pub executed: bool,
    /// Original action proposed
    pub original_action: StructuredAction,
    /// The action that was actually executed (None if rejected)
    pub executed_action: Option<StructuredAction>,
    /// Validation verdict
    pub verdict: ActionVerdict,
    /// Audit trail from each pipeline stage
    pub audit: Vec<PipelineAuditEntry>,
    /// Total pipeline latency in microseconds
    pub total_latency_us: u64,
    /// Error message (if any)
    pub error: Option<String>,
}
