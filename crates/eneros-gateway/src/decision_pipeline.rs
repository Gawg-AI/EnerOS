use std::sync::Arc;
use std::time::Instant;
use parking_lot::RwLock;
use eneros_core::{ActionVerdict, AuthorityLevel, Jurisdiction, StructuredAction, SystemOperatingState};
use eneros_constraint::projector::{FeasibilityProjector, ProjectionResult, WhatIfResult};
use crate::constraint_validator::ConstraintAwareValidator;
use crate::gateway::SafetyGateway;
use crate::command::{Command, CommandType, CommandPriority};
use crate::precondition::PreConditionChecker;
use crate::postcondition::PostConditionVerifier;
use crate::decomposer::ActionDecomposer;
use crate::pipeline_types::{
    DecisionContext, EnhancedPipelineDecision, PipelineAuditEntry,
    PipelineStatistics,
};

/// Constrained decision pipeline — ensures every action satisfies physical constraints.
///
/// The pipeline enforces deterministic, constraint-driven decision-making through
/// a fixed sequence of stages:
///
/// 1. **Pre-condition check** — Is the action legally/physically attemptable?
/// 2. **Feasibility projection** — Can the action be made feasible (possibly with modification)?
/// 3. **Constraint validation** — Does the action pass the 6-step validation pipeline?
/// 4. **Action decomposition** — Break composite actions into atomic steps
/// 5. **Execution** — Send the command through the safety gateway
/// 6. **Post-condition verification** — Did the action produce the expected outcome?
/// 7. **Rollback planning** — Prepare rollback if post-conditions fail
pub struct ConstrainedDecisionPipeline {
    projector: Arc<FeasibilityProjector>,
    validator: Arc<ConstraintAwareValidator>,
    gateway: Arc<SafetyGateway>,
    /// Pre-condition checker
    precondition_checker: PreConditionChecker,
    /// Post-condition verifier
    postcondition_verifier: PostConditionVerifier,
    /// Pipeline statistics
    statistics: RwLock<PipelineStatistics>,
}

/// Legacy result type — kept for backward compatibility
#[derive(Debug, Clone)]
pub struct PipelineDecision {
    /// The action that was actually executed (None if rejected)
    pub executed_action: Option<StructuredAction>,
    /// Original action proposed
    pub original_action: StructuredAction,
    /// Projection result
    pub projection: ProjectionResult,
    /// Validation verdict
    pub verdict: ActionVerdict,
    /// Audit entries from each pipeline stage
    pub audit: Vec<PipelineAuditEntry>,
}

/// Legacy audit entry — kept for backward compatibility
#[derive(Debug, Clone)]
pub struct LegacyPipelineAuditEntry {
    pub stage: String,
    pub description: String,
    pub duration_us: u64,
}

impl ConstrainedDecisionPipeline {
    pub fn new(
        projector: Arc<FeasibilityProjector>,
        validator: Arc<ConstraintAwareValidator>,
        gateway: Arc<SafetyGateway>,
    ) -> Self {
        Self {
            projector,
            validator,
            gateway,
            precondition_checker: PreConditionChecker::new(),
            postcondition_verifier: PostConditionVerifier::new(),
            statistics: RwLock::new(PipelineStatistics::default()),
        }
    }

    /// Create with custom pre-condition checker
    pub fn with_precondition_checker(
        projector: Arc<FeasibilityProjector>,
        validator: Arc<ConstraintAwareValidator>,
        gateway: Arc<SafetyGateway>,
        checker: PreConditionChecker,
    ) -> Self {
        Self {
            projector,
            validator,
            gateway,
            precondition_checker: checker,
            postcondition_verifier: PostConditionVerifier::new(),
            statistics: RwLock::new(PipelineStatistics::default()),
        }
    }

    /// Create with custom post-condition verifier
    pub fn with_postcondition_verifier(
        projector: Arc<FeasibilityProjector>,
        validator: Arc<ConstraintAwareValidator>,
        gateway: Arc<SafetyGateway>,
        verifier: PostConditionVerifier,
    ) -> Self {
        Self {
            projector,
            validator,
            gateway,
            precondition_checker: PreConditionChecker::new(),
            postcondition_verifier: verifier,
            statistics: RwLock::new(PipelineStatistics::default()),
        }
    }

    /// Process a single action through the enhanced pipeline using DecisionContext
    pub fn decide_enhanced(
        &self,
        action: &StructuredAction,
        ctx: &DecisionContext,
    ) -> EnhancedPipelineDecision {
        let pipeline_start = Instant::now();
        let mut audit = Vec::new();

        // ── Stage 1: Pre-condition check ──
        let start = Instant::now();
        let pre_result = self.precondition_checker.check(action, ctx);
        let pre_duration = start.elapsed().as_micros() as u64;

        audit.push(PipelineAuditEntry {
            stage: "precondition".to_string(),
            description: if pre_result.satisfied {
                format!("All {} pre-conditions passed", pre_result.checks.len())
            } else {
                format!("Pre-conditions FAILED: {}", pre_result.failure_summary.join("; "))
            },
            duration_us: pre_duration,
            passed: pre_result.satisfied,
        });

        if !pre_result.satisfied {
            let failure_msg = pre_result.failure_summary.join("; ");
            let total_latency = pipeline_start.elapsed().as_micros() as u64;
            self.record_stats(total_latency, &ActionVerdict::Rejected(
                failure_msg.clone()
            ), false);
            return EnhancedPipelineDecision {
                executed_action: None,
                original_action: action.clone(),
                decomposition: None,
                projection: ProjectionResult::Feasible(action.clone()),
                pre_conditions: pre_result,
                post_conditions: None,
                verdict: ActionVerdict::Rejected(failure_msg),
                rollback_plan: None,
                audit,
                total_latency_us: total_latency,
            };
        }

        // ── Stage 2: Feasibility projection ──
        let start = Instant::now();
        let projection = self.projector.project(action);
        let proj_duration = start.elapsed().as_micros() as u64;

        audit.push(PipelineAuditEntry {
            stage: "projection".to_string(),
            description: format_projection_result(&projection),
            duration_us: proj_duration,
            passed: !projection.is_infeasible(),
        });

        // Get the feasible action (if any)
        let feasible_action = match &projection {
            ProjectionResult::Feasible(a) => a.clone(),
            ProjectionResult::Projected { projected, .. } => projected.clone(),
            ProjectionResult::Infeasible { suggested_alternatives, .. } => {
                if let Some(alt) = suggested_alternatives.first() {
                    let alt_projection = self.projector.project(alt);
                    match alt_projection.feasible_action() {
                        Some(a) => a.clone(),
                        None => {
                            let total_latency = pipeline_start.elapsed().as_micros() as u64;
                            self.record_stats(total_latency, &ActionVerdict::Rejected(
                                "Action infeasible and no alternative found".to_string()
                            ), projection.is_projected());
                            return EnhancedPipelineDecision {
                                executed_action: None,
                                original_action: action.clone(),
                                decomposition: None,
                                projection,
                                pre_conditions: pre_result,
                                post_conditions: None,
                                verdict: ActionVerdict::Rejected(
                                    "Action infeasible and no alternative found".to_string()
                                ),
                                rollback_plan: None,
                                audit,
                                total_latency_us: total_latency,
                            };
                        }
                    }
                } else {
                    let total_latency = pipeline_start.elapsed().as_micros() as u64;
                    self.record_stats(total_latency, &ActionVerdict::Rejected(
                        "Action infeasible with no alternatives".to_string()
                    ), projection.is_projected());
                    return EnhancedPipelineDecision {
                        executed_action: None,
                        original_action: action.clone(),
                        decomposition: None,
                        projection,
                        pre_conditions: pre_result,
                        post_conditions: None,
                        verdict: ActionVerdict::Rejected(
                            "Action infeasible with no alternatives".to_string()
                        ),
                        rollback_plan: None,
                        audit,
                        total_latency_us: total_latency,
                    };
                }
            }
        };

        // ── Stage 3: Constraint validation (6-step pipeline) ──
        let start = Instant::now();
        let action_desc = format_structured_action(&feasible_action);
        let (target_zone, target_device) = extract_targets(&feasible_action);
        let verdict = self.validator.validate(
            &action_desc,
            ctx.authority,
            &ctx.jurisdiction,
            ctx.system_state,
            target_zone,
            target_device,
            ctx.device_states.as_ref(),
        );
        let val_duration = start.elapsed().as_micros() as u64;

        audit.push(PipelineAuditEntry {
            stage: "validation".to_string(),
            description: format_verdict(&verdict),
            duration_us: val_duration,
            passed: !matches!(verdict, ActionVerdict::Rejected(_)),
        });

        // ── Stage 4: Action decomposition ──
        let start = Instant::now();
        let decomposition = ActionDecomposer::decompose(&feasible_action);
        let rollback_plan = ActionDecomposer::rollback_plan(&decomposition);
        let decomp_duration = start.elapsed().as_micros() as u64;

        audit.push(PipelineAuditEntry {
            stage: "decomposition".to_string(),
            description: if decomposition.is_multi_step() {
                format!("Decomposed into {} steps (atomic={})", decomposition.step_count(), decomposition.atomic)
            } else {
                "Single-step action, no decomposition needed".to_string()
            },
            duration_us: decomp_duration,
            passed: true,
        });

        // ── Stage 5 & 6: Execute and verify ──
        match &verdict {
            ActionVerdict::Approved | ActionVerdict::EmergencyBypassed { .. } => {
                let start = Instant::now();

                // Execute each step of the decomposition
                let mut execution_ok = true;
                for step in &decomposition.steps {
                    let cmd = structured_action_to_command(&step.action);
                    if let Err(e) = self.gateway.execute_command(cmd) {
                        execution_ok = false;
                        audit.push(PipelineAuditEntry {
                            stage: "execution".to_string(),
                            description: format!("Step {} FAILED: {}", step.step_index, e),
                            duration_us: start.elapsed().as_micros() as u64,
                            passed: false,
                        });
                        break;
                    }
                }

                let exec_duration = start.elapsed().as_micros() as u64;

                if execution_ok {
                    audit.push(PipelineAuditEntry {
                        stage: "execution".to_string(),
                        description: format!(
                            "Action executed successfully ({} step{})",
                            decomposition.step_count(),
                            if decomposition.step_count() > 1 { "s" } else { "" }
                        ),
                        duration_us: exec_duration,
                        passed: true,
                    });
                }

                // ── Stage 6: Post-condition verification ──
                let post_conditions = if execution_ok {
                    let what_if = self.projector.simulate(&feasible_action);
                    let start = Instant::now();
                    let post_result = self.postcondition_verifier.verify(
                        &feasible_action, &what_if, ctx,
                    );
                    let post_duration = start.elapsed().as_micros() as u64;

                    audit.push(PipelineAuditEntry {
                        stage: "postcondition".to_string(),
                        description: if post_result.satisfied {
                            "All post-conditions satisfied".to_string()
                        } else {
                            format!(
                                "Post-conditions FAILED: {} new violations, {} worsened",
                                post_result.new_violations.len(),
                                post_result.worsened_violations.len()
                            )
                        },
                        duration_us: post_duration,
                        passed: post_result.satisfied,
                    });

                    Some(post_result)
                } else {
                    None
                };

                let total_latency = pipeline_start.elapsed().as_micros() as u64;
                self.record_stats(total_latency, &verdict, projection.is_projected());

                EnhancedPipelineDecision {
                    executed_action: Some(feasible_action),
                    original_action: action.clone(),
                    decomposition: Some(decomposition),
                    projection,
                    pre_conditions: pre_result,
                    post_conditions,
                    verdict,
                    rollback_plan: Some(rollback_plan),
                    audit,
                    total_latency_us: total_latency,
                }
            }
            ActionVerdict::Rejected(_) | ActionVerdict::PendingApproval { .. } => {
                let total_latency = pipeline_start.elapsed().as_micros() as u64;
                self.record_stats(total_latency, &verdict, projection.is_projected());

                EnhancedPipelineDecision {
                    executed_action: None,
                    original_action: action.clone(),
                    decomposition: Some(decomposition),
                    projection,
                    pre_conditions: pre_result,
                    post_conditions: None,
                    verdict,
                    rollback_plan: Some(rollback_plan),
                    audit,
                    total_latency_us: total_latency,
                }
            }
        }
    }

    /// Process a single action through the legacy pipeline (backward compatible)
    pub fn decide(
        &self,
        action: &StructuredAction,
        authority: AuthorityLevel,
        jurisdiction: &Jurisdiction,
        system_state: SystemOperatingState,
    ) -> PipelineDecision {
        let ctx = DecisionContext::new(authority, jurisdiction.clone(), system_state);
        let enhanced = self.decide_enhanced(action, &ctx);

        PipelineDecision {
            executed_action: enhanced.executed_action,
            original_action: enhanced.original_action,
            projection: enhanced.projection,
            verdict: enhanced.verdict,
            audit: enhanced.audit,
        }
    }

    /// Process multiple actions through the pipeline
    pub fn decide_batch(
        &self,
        actions: &[StructuredAction],
        authority: AuthorityLevel,
        jurisdiction: &Jurisdiction,
        system_state: SystemOperatingState,
    ) -> Vec<PipelineDecision> {
        actions.iter()
            .map(|a| self.decide(a, authority, jurisdiction, system_state))
            .collect()
    }

    /// Process multiple actions through the enhanced pipeline
    pub fn decide_batch_enhanced(
        &self,
        actions: &[StructuredAction],
        ctx: &DecisionContext,
    ) -> Vec<EnhancedPipelineDecision> {
        actions.iter()
            .map(|a| self.decide_enhanced(a, ctx))
            .collect()
    }

    /// Get pipeline statistics
    pub fn statistics(&self) -> PipelineStatistics {
        self.statistics.read().clone()
    }

    /// Reset pipeline statistics
    pub fn reset_statistics(&self) {
        *self.statistics.write() = PipelineStatistics::default();
    }

    /// Record statistics for a decision
    fn record_stats(&self, latency_us: u64, verdict: &ActionVerdict, was_projected: bool) {
        let mut stats = self.statistics.write();
        stats.record_decision(latency_us);
        match verdict {
            ActionVerdict::Approved => stats.approved += 1,
            ActionVerdict::Rejected(_) => stats.rejected += 1,
            ActionVerdict::PendingApproval { .. } => stats.pending_approval += 1,
            ActionVerdict::EmergencyBypassed { .. } => stats.emergency_bypassed += 1,
        }
        if was_projected {
            stats.projected += 1;
        }
    }
}

/// Internal helper: simulate action via projector's simulator
trait ProjectorSimulate {
    fn simulate(&self, action: &StructuredAction) -> WhatIfResult;
}

impl ProjectorSimulate for FeasibilityProjector {
    fn simulate(&self, action: &StructuredAction) -> WhatIfResult {
        // Use the projector's internal simulator
        // We need to call project and extract the What-If from it
        // Since projector doesn't expose simulate directly, we use a workaround:
        // The projector already does simulation internally, so we call project
        // and reconstruct a WhatIfResult from the ProjectionResult
        let result = self.project(action);
        match result {
            ProjectionResult::Feasible(_) => WhatIfResult {
                applicable: true,
                converged: true,
                voltage_violations: vec![],
                thermal_violations: vec![],
                all_constraints_satisfied: true,
                summary: "Feasible".to_string(),
            },
            ProjectionResult::Projected { .. } => WhatIfResult {
                applicable: true,
                converged: true,
                voltage_violations: vec![],
                thermal_violations: vec![],
                all_constraints_satisfied: true,
                summary: "Projected to feasible".to_string(),
            },
            ProjectionResult::Infeasible { violated_constraints, .. } => {
                let voltage_violations: Vec<(u64, f64, f64)> = violated_constraints.iter()
                    .filter(|v| v.contains("Voltage"))
                    .filter_map(|v| {
                        // Parse "Voltage violation: Bus X voltage Y.YYY pu < Z.ZZZ pu limit"
                        let parts: Vec<&str> = v.split_whitespace().collect();
                        let bus: u64 = parts.get(3)?.parse().ok()?;
                        let voltage: f64 = parts.get(5)?.parse().ok()?;
                        let limit: f64 = parts.get(9)?.parse().ok()?;
                        Some((bus, voltage, limit))
                    })
                    .collect();
                let thermal_violations: Vec<(u64, f64, f64)> = violated_constraints.iter()
                    .filter(|v| v.contains("Thermal"))
                    .filter_map(|v| {
                        let parts: Vec<&str> = v.split_whitespace().collect();
                        let branch: u64 = parts.get(3)?.parse().ok()?;
                        let loading: f64 = parts.get(5)?.trim_end_matches('%').parse().ok()?;
                        let limit: f64 = parts.get(9)?.trim_end_matches('%').parse().ok()?;
                        Some((branch, loading, limit))
                    })
                    .collect();
                let converged = !violated_constraints.iter().any(|v| v.contains("did not converge"));
                WhatIfResult {
                    applicable: true,
                    converged,
                    voltage_violations,
                    thermal_violations,
                    all_constraints_satisfied: false,
                    summary: violated_constraints.join("; "),
                }
            }
        }
    }
}

/// Convert a StructuredAction to a Command for the SafetyGateway
pub fn structured_action_to_command(action: &StructuredAction) -> Command {
    match action {
        StructuredAction::StartGenerator { gen_id, target_mw } => {
            Command::new(
                CommandType::GeneratorSetpoint,
                *gen_id,
                CommandPriority::Normal,
                &format!("Set generator {} to {:.1} MW", gen_id, target_mw),
            )
        }
        StructuredAction::ShedLoad { zone_id, amount_mw } => {
            Command::new(
                CommandType::LoadShedding,
                *zone_id as u64,
                CommandPriority::High,
                &format!("Shed {:.1} MW from zone {}", amount_mw, zone_id),
            )
        }
        StructuredAction::ExecuteDevice { device_id, operation, value } => {
            let cmd_type = match operation.as_str() {
                "close" | "合闸" => CommandType::SwitchToggle,
                "open" | "分闸" => CommandType::SwitchToggle,
                "adjust_reactive" => CommandType::GeneratorSetpoint,
                _ => CommandType::SwitchToggle,
            };
            let priority = if operation == "open" || operation == "分闸" {
                CommandPriority::High
            } else {
                CommandPriority::Normal
            };
            Command::new(
                cmd_type,
                *device_id,
                priority,
                &format!("{} device {} value {:.2}", operation, device_id, value),
            )
        }
        StructuredAction::IsolateFault { upstream_switch, downstream_switch } => {
            Command::new(
                CommandType::SwitchToggle,
                *upstream_switch,
                CommandPriority::Critical,
                &format!("Isolate fault: open switches {} and {}", upstream_switch, downstream_switch),
            )
        }
        StructuredAction::CloseTieSwitch { switch_id } => {
            Command::new(
                CommandType::SwitchToggle,
                *switch_id,
                CommandPriority::High,
                &format!("Close tie switch {}", switch_id),
            )
        }
        StructuredAction::NotifyAgent { .. } => {
            Command::new(
                CommandType::GeneratorSetpoint,
                0,
                CommandPriority::Low,
                "notify (no command)",
            )
        }
    }
}

pub fn format_structured_action(action: &StructuredAction) -> String {
    match action {
        StructuredAction::StartGenerator { gen_id, target_mw } =>
            format!("Start generator {} to {:.1} MW", gen_id, target_mw),
        StructuredAction::ShedLoad { zone_id, amount_mw } =>
            format!("Shed {:.1} MW from zone {}", amount_mw, zone_id),
        StructuredAction::ExecuteDevice { device_id, operation, value } =>
            format!("{} device {} value {:.2}", operation, device_id, value),
        StructuredAction::IsolateFault { upstream_switch, downstream_switch } =>
            format!("Isolate fault: switches {} and {}", upstream_switch, downstream_switch),
        StructuredAction::CloseTieSwitch { switch_id } =>
            format!("Close tie switch {}", switch_id),
        StructuredAction::NotifyAgent { agent_id, message } =>
            format!("Notify agent {}: {}", agent_id, message),
    }
}

fn format_projection_result(result: &ProjectionResult) -> String {
    match result {
        ProjectionResult::Feasible(_) => "Action feasible as-is".to_string(),
        ProjectionResult::Projected { modifications, .. } => {
            let mods: Vec<String> = modifications.iter()
                .map(|m| format!("{}: {:.2} -> {:.2} ({})", m.parameter, m.original_value, m.projected_value, m.reason))
                .collect();
            format!("Action projected: {}", mods.join("; "))
        }
        ProjectionResult::Infeasible { violated_constraints, .. } => {
            format!("Action infeasible: {}", violated_constraints.join("; "))
        }
    }
}

fn format_verdict(verdict: &ActionVerdict) -> String {
    match verdict {
        ActionVerdict::Approved => "Approved".to_string(),
        ActionVerdict::Rejected(reason) => format!("Rejected: {}", reason),
        ActionVerdict::PendingApproval { approver_level, reason } =>
            format!("Pending approval from {:?}: {}", approver_level, reason),
        ActionVerdict::EmergencyBypassed { bypassed_checks, reason } =>
            format!("Emergency bypassed ({:?}): {}", bypassed_checks, reason),
    }
}

pub fn extract_targets(action: &StructuredAction) -> (Option<u32>, Option<u64>) {
    match action {
        StructuredAction::ShedLoad { zone_id, .. } => (Some(*zone_id), None),
        StructuredAction::StartGenerator { gen_id, .. } => (None, Some(*gen_id)),
        StructuredAction::ExecuteDevice { device_id, .. } => (None, Some(*device_id)),
        StructuredAction::IsolateFault { upstream_switch, .. } => (None, Some(*upstream_switch)),
        StructuredAction::CloseTieSwitch { switch_id } => (None, Some(*switch_id)),
        StructuredAction::NotifyAgent { .. } => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_constraint::ConstraintEngine;
    use eneros_constraint::projector::NetworkSimulator;

    struct MockSimulator;
    impl NetworkSimulator for MockSimulator {
        fn simulate_action(&self, _action: &StructuredAction) -> WhatIfResult {
            WhatIfResult {
                applicable: true,
                converged: true,
                voltage_violations: vec![],
                thermal_violations: vec![],
                all_constraints_satisfied: true,
                summary: "OK".to_string(),
            }
        }
        fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
            vec![(1, 0.0, 200.0)]
        }
        fn current_voltages(&self) -> Vec<(u64, f64)> {
            vec![(1, 1.02)]
        }
    }

    fn make_pipeline() -> ConstrainedDecisionPipeline {
        let projector = Arc::new(FeasibilityProjector::new(Arc::new(MockSimulator)));
        let constraint_engine = Arc::new(ConstraintEngine::new());
        let gateway = Arc::new(SafetyGateway::new(100));
        let validator = Arc::new(ConstraintAwareValidator::with_default_interlocking(
            constraint_engine, gateway.clone(),
        ));
        ConstrainedDecisionPipeline::new(projector, validator, gateway)
    }

    // ── Legacy API tests ──

    #[test]
    fn test_pipeline_feasible_action_approved() {
        let pipeline = make_pipeline();
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let result = pipeline.decide(
            &action,
            AuthorityLevel::Supervisor,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );
        assert!(result.executed_action.is_some());
        assert!(result.audit.len() >= 2);
    }

    #[test]
    fn test_pipeline_observer_rejected() {
        let pipeline = make_pipeline();
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let result = pipeline.decide(
            &action,
            AuthorityLevel::Observer,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );
        assert!(result.executed_action.is_none());
        assert!(matches!(result.verdict, ActionVerdict::Rejected(_)));
    }

    #[test]
    fn test_pipeline_high_risk_requires_supervisor() {
        let pipeline = make_pipeline();
        let action = StructuredAction::ShedLoad { zone_id: 1, amount_mw: 50.0 };
        let result = pipeline.decide(
            &action,
            AuthorityLevel::Operator,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );
        assert!(result.executed_action.is_none() || matches!(result.verdict, ActionVerdict::PendingApproval { .. }));
    }

    #[test]
    fn test_pipeline_batch() {
        let pipeline = make_pipeline();
        let actions = vec![
            StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 },
            StructuredAction::NotifyAgent { agent_id: "test".to_string(), message: "hello".to_string() },
        ];
        let results = pipeline.decide_batch(
            &actions,
            AuthorityLevel::Supervisor,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );
        assert_eq!(results.len(), 2);
    }

    // ── Enhanced API tests ──

    #[test]
    fn test_enhanced_pipeline_feasible_action() {
        let pipeline = make_pipeline();
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );
        let result = pipeline.decide_enhanced(&action, &ctx);
        assert!(result.executed_action.is_some());
        assert!(result.pre_conditions.satisfied);
        assert!(result.decomposition.is_some());
        assert!(result.rollback_plan.is_some());
        assert!(result.total_latency_us > 0);
        // Should have: precondition + projection + validation + decomposition + execution + postcondition
        assert!(result.audit.len() >= 5, "Expected >= 5 audit entries, got {}", result.audit.len());
    }

    #[test]
    fn test_enhanced_pipeline_observer_rejected() {
        let pipeline = make_pipeline();
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = DecisionContext::new(
            AuthorityLevel::Observer,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );
        let result = pipeline.decide_enhanced(&action, &ctx);
        assert!(result.executed_action.is_none());
        assert!(!result.pre_conditions.satisfied);
        assert!(matches!(result.verdict, ActionVerdict::Rejected(_)));
    }

    #[test]
    fn test_enhanced_pipeline_isolate_fault_decomposed() {
        let pipeline = make_pipeline();
        let action = StructuredAction::IsolateFault {
            upstream_switch: 10,
            downstream_switch: 20,
        };
        let ctx = DecisionContext::new(
            AuthorityLevel::Emergency,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Emergency,
        ).with_device_states(crate::interlocking::DeviceStates::default());
        let result = pipeline.decide_enhanced(&action, &ctx);
        // IsolateFault should be decomposed into 2 steps
        if let Some(ref decomp) = result.decomposition {
            assert!(decomp.is_multi_step());
            assert_eq!(decomp.step_count(), 2);
        }
    }

    #[test]
    fn test_enhanced_pipeline_statistics() {
        let pipeline = make_pipeline();
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );
        let _ = pipeline.decide_enhanced(&action, &ctx);
        let stats = pipeline.statistics();
        assert_eq!(stats.total_decisions, 1);
        assert!(stats.avg_latency_us > 0);
    }

    #[test]
    fn test_enhanced_pipeline_batch() {
        let pipeline = make_pipeline();
        let actions = vec![
            StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 },
            StructuredAction::NotifyAgent { agent_id: "test".to_string(), message: "hello".to_string() },
        ];
        let ctx = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );
        let results = pipeline.decide_batch_enhanced(&actions, &ctx);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_format_structured_action() {
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let desc = format_structured_action(&action);
        assert!(desc.contains("100.0 MW"));
    }

    #[test]
    fn test_extract_targets() {
        let action = StructuredAction::ShedLoad { zone_id: 5, amount_mw: 50.0 };
        let (zone, device) = extract_targets(&action);
        assert_eq!(zone, Some(5));
        assert_eq!(device, None);

        let action2 = StructuredAction::StartGenerator { gen_id: 10, target_mw: 100.0 };
        let (zone2, device2) = extract_targets(&action2);
        assert_eq!(zone2, None);
        assert_eq!(device2, Some(10));
    }
}
