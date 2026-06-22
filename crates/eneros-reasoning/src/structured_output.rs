use eneros_core::StructuredAction;
use serde::{Deserialize, Serialize};
pub use eneros_constraint::projector::{ActionModification, ProjectionResult, WhatIfResult, NetworkSimulator, FeasibilityProjector};

/// Structured action output from reasoning — replaces `Vec<String>` in ReasoningOutput
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredActionOutput {
    /// Reasoning chain (LLM's thought process, for audit)
    pub reasoning_chain: String,
    /// Confidence [0, 1]
    pub confidence: f64,
    /// Recommended actions (structured enum, not text)
    pub actions: Vec<StructuredAction>,
    /// Preconditions the LLM believes must be satisfied
    pub preconditions: Vec<String>,
}

impl StructuredActionOutput {
    pub fn new(reasoning_chain: &str, confidence: f64) -> Self {
        Self {
            reasoning_chain: reasoning_chain.to_string(),
            confidence: confidence.clamp(0.0, 1.0),
            actions: Vec::new(),
            preconditions: Vec::new(),
        }
    }

    pub fn with_action(mut self, action: StructuredAction) -> Self {
        self.actions.push(action);
        self
    }

    pub fn with_precondition(mut self, precondition: &str) -> Self {
        self.preconditions.push(precondition.to_string());
        self
    }
}

/// Complete decision result from the constrained decision pipeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionResult {
    /// Final executed action (after projection and validation), None if rejected
    pub executed_action: Option<StructuredAction>,
    /// Original LLM proposal
    pub original_proposal: StructuredAction,
    /// Projection result
    pub projection: ProjectionResult,
    /// Validation verdict
    pub verdict: eneros_core::ActionVerdict,
    /// Number of LLM retries (0 = first attempt accepted)
    pub retries: u32,
    /// Audit trail entries
    pub audit_entries: Vec<DecisionAuditEntry>,
}

/// Single audit entry in the decision pipeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionAuditEntry {
    /// Pipeline stage name
    pub stage: String,
    /// Description of what happened at this stage
    pub description: String,
    /// Duration of this stage in microseconds
    pub duration_us: u64,
}

impl DecisionResult {
    /// Was the action ultimately executed?
    pub fn is_executed(&self) -> bool {
        self.executed_action.is_some()
    }

    /// Was the action rejected?
    pub fn is_rejected(&self) -> bool {
        self.executed_action.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_structured_action_output_new() {
        let output = StructuredActionOutput::new("test reasoning", 0.8);
        assert_eq!(output.reasoning_chain, "test reasoning");
        assert!((output.confidence - 0.8).abs() < f64::EPSILON);
        assert!(output.actions.is_empty());
        assert!(output.preconditions.is_empty());
    }

    #[test]
    fn test_structured_action_output_with_action() {
        let output = StructuredActionOutput::new("test", 0.9)
            .with_action(StructuredAction::ShedLoad { zone_id: 1, amount_mw: 50.0 })
            .with_action(StructuredAction::StartGenerator { gen_id: 2, target_mw: 100.0 });
        assert_eq!(output.actions.len(), 2);
    }

    #[test]
    fn test_structured_action_output_with_precondition() {
        let output = StructuredActionOutput::new("test", 0.9)
            .with_precondition("Voltage must be above 0.95 pu");
        assert_eq!(output.preconditions.len(), 1);
    }

    #[test]
    fn test_structured_action_output_confidence_clamped() {
        let output = StructuredActionOutput::new("test", 1.5);
        assert!((output.confidence - 1.0).abs() < f64::EPSILON);
        let output2 = StructuredActionOutput::new("test", -0.5);
        assert!((output2.confidence - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_projection_result_feasible() {
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let result = ProjectionResult::Feasible(action.clone());
        assert!(result.is_feasible());
        assert!(!result.is_projected());
        assert!(!result.is_infeasible());
        assert!(result.feasible_action().is_some());
    }

    #[test]
    fn test_projection_result_projected() {
        let original = StructuredAction::StartGenerator { gen_id: 1, target_mw: 300.0 };
        let projected = StructuredAction::StartGenerator { gen_id: 1, target_mw: 200.0 };
        let result = ProjectionResult::Projected {
            original: original.clone(),
            projected: projected.clone(),
            modifications: vec![ActionModification {
                parameter: "target_mw".to_string(),
                original_value: 300.0,
                projected_value: 200.0,
                reason: "Generator rated capacity 200MW".to_string(),
            }],
        };
        assert!(!result.is_feasible());
        assert!(result.is_projected());
        assert!(!result.is_infeasible());
        assert!(result.feasible_action().is_some());
    }

    #[test]
    fn test_projection_result_infeasible() {
        let action = StructuredAction::IsolateFault { upstream_switch: 1, downstream_switch: 2 };
        let result = ProjectionResult::Infeasible {
            original: action,
            violated_constraints: vec!["interlocking: breaker 1 is closed".to_string()],
            suggested_alternatives: vec![],
        };
        assert!(!result.is_feasible());
        assert!(!result.is_projected());
        assert!(result.is_infeasible());
        assert!(result.feasible_action().is_none());
    }

    #[test]
    fn test_decision_result_executed() {
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let result = DecisionResult {
            executed_action: Some(action),
            original_proposal: StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 },
            projection: ProjectionResult::Feasible(StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 }),
            verdict: eneros_core::ActionVerdict::Approved,
            retries: 0,
            audit_entries: vec![],
        };
        assert!(result.is_executed());
        assert!(!result.is_rejected());
    }

    #[test]
    fn test_decision_result_rejected() {
        let action = StructuredAction::IsolateFault { upstream_switch: 1, downstream_switch: 2 };
        let result = DecisionResult {
            executed_action: None,
            original_proposal: action,
            projection: ProjectionResult::Infeasible {
                original: StructuredAction::IsolateFault { upstream_switch: 1, downstream_switch: 2 },
                violated_constraints: vec!["interlocking violation".to_string()],
                suggested_alternatives: vec![],
            },
            verdict: eneros_core::ActionVerdict::Rejected("interlocking violation".to_string()),
            retries: 0,
            audit_entries: vec![],
        };
        assert!(!result.is_executed());
        assert!(result.is_rejected());
    }

    #[test]
    fn test_structured_action_output_serde_roundtrip() {
        let output = StructuredActionOutput::new("reasoning", 0.85)
            .with_action(StructuredAction::ShedLoad { zone_id: 1, amount_mw: 50.0 })
            .with_precondition("System in Alert state");
        let json = serde_json::to_string(&output).unwrap();
        let deserialized: StructuredActionOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.reasoning_chain, "reasoning");
        assert_eq!(deserialized.actions.len(), 1);
        assert_eq!(deserialized.preconditions.len(), 1);
    }
}
