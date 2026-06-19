use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use eneros_core::{ActionVerdict, AuthorityLevel, Jurisdiction, PowerObservation, StructuredAction, SystemOperatingState};
use eneros_constraint::projector::{FeasibilityProjector, ProjectionResult, WhatIfResult};
use crate::constraint_validator::ConstraintAwareValidator;
use crate::gateway::SafetyGateway;
use crate::command::{Command, CommandType, CommandPriority, DeviceValue};
use crate::precondition::PreConditionChecker;
use crate::postcondition::PostConditionVerifier;
use crate::decomposer::ActionDecomposer;
use crate::pipeline_types::{
    DecisionContext, EnhancedPipelineDecision, PipelineAuditEntry,
    PipelineStatistics, PipelineStatisticsSnapshot, RollbackExecution, RollbackStrategy,
};

/// Maximum time to wait for field observation before falling back to simulator.
/// SCADA/RTU reads beyond this are treated as "unavailable" to prevent
/// postcondition verification from blocking the decision pipeline.
const OBSERVATION_TIMEOUT: Duration = Duration::from_millis(500);

/// Async callback that returns the **current** field observation after execution.
///
/// In production this reads from SCADA / RTU / IEC-104 polling. In tests it
/// can be a mock that returns a canned `PowerObservation`. When `None`, the
/// pipeline falls back to simulator-based postcondition verification.
pub type ObservationProvider = Arc<dyn Fn() -> Option<PowerObservation> + Send + Sync>;

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
///
/// ## Post-condition data source
///
/// Stage 6 has two modes:
/// - **Field observation** (production): If an `ObservationProvider` is
///   configured, the postcondition is verified against **real measurements**
///   read back from SCADA/RTU after execution. This closes the loop
///   "execute → measure → verify" and catches cases where the device
///   NACK'd the command or the physical effect differed from the prediction.
/// - **Simulator prediction** (fallback): If no provider is configured,
///   the pipeline re-runs `simulate_action` as a best-effort prediction.
///   This is the legacy behavior and is inherently weaker because the
///   simulator is a pure function that does not know whether execution
///   actually changed the world.
pub struct ConstrainedDecisionPipeline {
    projector: Arc<FeasibilityProjector>,
    validator: Arc<ConstraintAwareValidator>,
    gateway: Arc<SafetyGateway>,
    /// Pre-condition checker
    precondition_checker: PreConditionChecker,
    /// Post-condition verifier
    postcondition_verifier: PostConditionVerifier,
    /// Pipeline statistics (v0.8.0 — M5: atomic, no RwLock)
    statistics: PipelineStatistics,
    /// Optional provider for post-execution field observations.
    ///
    /// When set, postcondition verification uses **real measurements** instead
    /// of simulator predictions, closing the execute→measure→verify loop.
    observation_provider: Option<ObservationProvider>,
    /// Optional watchdog timer for command execution (v0.7.0).
    ///
    /// When set, each command execution in Stage 5 is registered with the
    /// watchdog. If a command exceeds the timeout, the watchdog fires its
    /// callback (typically triggering rollback or alerting).
    watchdog: Option<Arc<crate::watchdog::WatchdogTimer>>,
    /// Per-command execution timeout (v0.7.0). Defaults to 500ms.
    command_timeout: Duration,
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
            statistics: PipelineStatistics::default(),
            observation_provider: None,
            watchdog: None,
            command_timeout: Duration::from_millis(500),
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
            statistics: PipelineStatistics::default(),
            observation_provider: None,
            watchdog: None,
            command_timeout: Duration::from_millis(500),
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
            statistics: PipelineStatistics::default(),
            observation_provider: None,
            watchdog: None,
            command_timeout: Duration::from_millis(500),
        }
    }

    /// Create with a field observation provider for production-grade
    /// postcondition verification.
    ///
    /// When set, Stage 6 reads **real measurements** from SCADA/RTU after
    /// execution instead of relying on the simulator's pure-function
    /// prediction. This closes the "execute → measure → verify" loop.
    pub fn with_observation_provider(
        projector: Arc<FeasibilityProjector>,
        validator: Arc<ConstraintAwareValidator>,
        gateway: Arc<SafetyGateway>,
        provider: ObservationProvider,
    ) -> Self {
        Self {
            projector,
            validator,
            gateway,
            precondition_checker: PreConditionChecker::new(),
            postcondition_verifier: PostConditionVerifier::new(),
            statistics: PipelineStatistics::default(),
            observation_provider: Some(provider),
            watchdog: None,
            command_timeout: Duration::from_millis(500),
        }
    }

    /// Attach a watchdog timer to monitor command execution (v0.7.0).
    ///
    /// When set, each command in Stage 5 is registered with the watchdog.
    /// If a command exceeds `command_timeout`, the watchdog fires its
    /// timeout callback. The pipeline itself does not abort the command
    /// (the gateway's executor handles that); the watchdog is for
    /// observability and triggering external alerts/rollback.
    pub fn with_watchdog(
        mut self,
        watchdog: Arc<crate::watchdog::WatchdogTimer>,
        command_timeout: Duration,
    ) -> Self {
        self.watchdog = Some(watchdog);
        self.command_timeout = command_timeout;
        self
    }

    /// Process a single action through the enhanced pipeline using DecisionContext
    pub async fn decide_enhanced(
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
                rollback_executed: None,
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
                                rollback_executed: None,
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
                        rollback_executed: None,
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
                let mut execution_error = String::new();
                for step in &decomposition.steps {
                    let cmd = structured_action_to_command(&step.action);

                    // Register a watchdog guard for this command (v0.7.0).
                    // The guard is dropped at the end of the loop iteration,
                    // which cancels the watchdog if the command completed in time.
                    let _watchdog_guard = self.watchdog.as_ref().map(|wd| {
                        let op_id = format!(
                            "cmd-step-{}-{}",
                            step.step_index,
                            chrono::Utc::now().timestamp_millis()
                        );
                        wd.register_with_timeout(op_id, self.command_timeout)
                    });

                    if let Err(e) = self.gateway.execute_command(cmd).await {
                        execution_ok = false;
                        execution_error = format!("Step {} FAILED: {}", step.step_index, e);
                        audit.push(PipelineAuditEntry {
                            stage: "execution".to_string(),
                            description: execution_error.clone(),
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

                // If execution failed, reject the action
                if !execution_ok {
                    let total_latency = pipeline_start.elapsed().as_micros() as u64;
                    self.record_stats(total_latency, &ActionVerdict::Rejected(
                        execution_error.clone()
                    ), projection.is_projected());
                    return EnhancedPipelineDecision {
                        executed_action: None,
                        original_action: action.clone(),
                        decomposition: Some(decomposition),
                        projection,
                        pre_conditions: pre_result,
                        post_conditions: None,
                        verdict: ActionVerdict::Rejected(execution_error),
                        rollback_plan: Some(rollback_plan),
                        rollback_executed: None,
                        audit,
                        total_latency_us: total_latency,
                    };
                }

                // ── Stage 6: Post-condition verification ──
                // Production path: read **real** field observations after execution
                // and verify against them. This closes the "execute → measure →
                // verify" loop, catching cases where the device NACK'd the command
                // or the physical effect differed from the prediction.
                //
                // Fallback: if no observation provider is configured, re-run the
                // simulator as a best-effort prediction (legacy behavior).
                let (post_what_if, postcondition_source) = if let Some(ref provider) = self.observation_provider {
                    // Wrap synchronous provider call in spawn_blocking + timeout
                    // to prevent SCADA/RTU I/O from blocking the async runtime.
                    let provider_clone = Arc::clone(provider);
                    let obs_result = tokio::time::timeout(
                        OBSERVATION_TIMEOUT,
                        tokio::task::spawn_blocking(move || provider_clone()),
                    ).await;
                    match obs_result {
                        Ok(Ok(Some(obs))) => {
                            let what_if = WhatIfResult::from_observation(
                                &obs,
                                self.postcondition_verifier.voltage_min,
                                self.postcondition_verifier.voltage_max,
                                self.postcondition_verifier.thermal_limit,
                            );
                            (what_if, "field_observation")
                        }
                        Ok(Ok(None)) => {
                            // Provider returned None — data unavailable, fall back
                            let what_if = self.projector.simulator().simulate_action(&feasible_action);
                            (what_if, "simulator_fallback")
                        }
                        Ok(Err(_)) | Err(_) => {
                            // spawn_blocking panicked or timed out — fall back to simulator
                            let what_if = self.projector.simulator().simulate_action(&feasible_action);
                            (what_if, "simulator_fallback")
                        }
                    }
                } else {
                    let what_if = self.projector.simulator().simulate_action(&feasible_action);
                    (what_if, "simulator_prediction")
                };

                let start = Instant::now();
                let post_result = self.postcondition_verifier.verify(
                    &feasible_action, &post_what_if, ctx,
                );
                let post_duration = start.elapsed().as_micros() as u64;

                audit.push(PipelineAuditEntry {
                    stage: "postcondition".to_string(),
                    description: if post_result.satisfied {
                        format!("All post-conditions satisfied (source: {})", postcondition_source)
                    } else {
                        format!(
                            "Post-conditions FAILED: {} new violations, {} worsened (source: {})",
                            post_result.new_violations.len(),
                            post_result.worsened_violations.len(),
                            postcondition_source
                        )
                    },
                    duration_us: post_duration,
                    passed: post_result.satisfied,
                });

                let total_latency = pipeline_start.elapsed().as_micros() as u64;
                self.record_stats(total_latency, &verdict, projection.is_projected());

                // Track postcondition failures
                let mut rollback_executed: Option<RollbackExecution> = None;
                if !post_result.satisfied {
                    self.statistics.postcondition_failures.fetch_add(1, Ordering::Relaxed);

                    // ── Stage 7: Auto-rollback execution (v0.6.0 — S6) ──
                    // When post-conditions fail and the rollback plan allows
                    // automatic rollback, execute the undo steps in reverse
                    // order to restore the system to its pre-action state.
                    if rollback_plan.can_auto_rollback() {
                        let rb_start = Instant::now();
                        self.statistics.rollbacks_triggered.fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(
                            "Post-condition failed; executing auto-rollback ({} steps, strategy={:?})",
                            rollback_plan.steps.len(),
                            rollback_plan.strategy
                        );

                        let mut steps_succeeded = 0usize;
                        let mut steps_attempted = 0usize;
                        let mut rb_error: Option<String> = None;

                        // Execute rollback steps in reverse order (last-in, first-out)
                        for rb_step in rollback_plan.steps.iter().rev() {
                            steps_attempted += 1;
                            let undo_cmd = structured_action_to_command(&rb_step.undo_action);
                            match self.gateway.execute_command(undo_cmd).await {
                                Ok(_) => {
                                    steps_succeeded += 1;
                                    tracing::info!(
                                        "Rollback step {} succeeded: {}",
                                        steps_attempted, rb_step.description
                                    );
                                }
                                Err(e) => {
                                    let msg = format!(
                                        "Rollback step {} FAILED: {} ({})",
                                        steps_attempted, e, rb_step.description
                                    );
                                    tracing::error!("{}", msg);
                                    rb_error = Some(msg);
                                    // For BestEffort strategy, continue; otherwise stop
                                    if rollback_plan.strategy != RollbackStrategy::BestEffort {
                                        break;
                                    }
                                }
                            }
                        }

                        let rb_duration = rb_start.elapsed().as_micros() as u64;
                        rollback_executed = Some(match rb_error {
                            None => {
                                self.statistics.rollbacks_succeeded.fetch_add(1, Ordering::Relaxed);
                                tracing::info!(
                                    "Auto-rollback completed successfully ({} steps, {} µs)",
                                    steps_succeeded, rb_duration
                                );
                                RollbackExecution::success(steps_succeeded, rb_duration)
                            }
                            Some(err) => {
                                self.statistics.rollbacks_failed.fetch_add(1, Ordering::Relaxed);
                                tracing::error!(
                                    "Auto-rollback FAILED after {}/{} steps: {}",
                                    steps_succeeded, steps_attempted, err
                                );
                                RollbackExecution::failure(
                                    steps_attempted,
                                    steps_succeeded,
                                    err,
                                    rb_duration,
                                )
                            }
                        });

                        audit.push(PipelineAuditEntry {
                            stage: "rollback".to_string(),
                            description: format!(
                                "Auto-rollback executed: {} steps attempted, {} succeeded",
                                steps_attempted, steps_succeeded
                            ),
                            duration_us: rb_duration,
                            passed: rollback_executed.as_ref().map(|r| r.succeeded).unwrap_or(false),
                        });
                    } else {
                        tracing::warn!(
                            "Post-condition failed but auto-rollback not allowed (strategy={:?})",
                            rollback_plan.strategy
                        );
                        audit.push(PipelineAuditEntry {
                            stage: "rollback".to_string(),
                            description: format!(
                                "Rollback skipped (auto_rollback={}, strategy={:?})",
                                rollback_plan.auto_rollback, rollback_plan.strategy
                            ),
                            duration_us: 0,
                            passed: false,
                        });
                    }
                }

                EnhancedPipelineDecision {
                    executed_action: Some(feasible_action),
                    original_action: action.clone(),
                    decomposition: Some(decomposition),
                    projection,
                    pre_conditions: pre_result,
                    post_conditions: Some(post_result),
                    verdict,
                    rollback_plan: Some(rollback_plan),
                    rollback_executed,
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
                    rollback_executed: None,
                    audit,
                    total_latency_us: total_latency,
                }
            }
        }
    }

    /// Process a single action through the legacy pipeline (backward compatible)
    pub async fn decide(
        &self,
        action: &StructuredAction,
        authority: AuthorityLevel,
        jurisdiction: &Jurisdiction,
        system_state: SystemOperatingState,
    ) -> PipelineDecision {
        let ctx = DecisionContext::new(authority, jurisdiction.clone(), system_state);
        let enhanced = self.decide_enhanced(action, &ctx).await;

        PipelineDecision {
            executed_action: enhanced.executed_action,
            original_action: enhanced.original_action,
            projection: enhanced.projection,
            verdict: enhanced.verdict,
            audit: enhanced.audit,
        }
    }

    /// Process multiple actions through the pipeline
    pub async fn decide_batch(
        &self,
        actions: &[StructuredAction],
        authority: AuthorityLevel,
        jurisdiction: &Jurisdiction,
        system_state: SystemOperatingState,
    ) -> Vec<PipelineDecision> {
        let mut results = Vec::with_capacity(actions.len());
        for a in actions {
            results.push(self.decide(a, authority, jurisdiction, system_state).await);
        }
        results
    }

    /// Process multiple actions through the enhanced pipeline
    pub async fn decide_batch_enhanced(
        &self,
        actions: &[StructuredAction],
        ctx: &DecisionContext,
    ) -> Vec<EnhancedPipelineDecision> {
        let mut results = Vec::with_capacity(actions.len());
        for a in actions {
            results.push(self.decide_enhanced(a, ctx).await);
        }
        results
    }

    /// Get pipeline statistics
    pub fn statistics(&self) -> PipelineStatisticsSnapshot {
        self.statistics.snapshot()
    }

    /// Expose the projector's `project` method for What-If analysis without
    /// executing the action. Used by the `POST /api/whatif` endpoint.
    pub fn project(&self, action: &StructuredAction) -> ProjectionResult {
        self.projector.project(action)
    }

    /// Reset pipeline statistics
    pub fn reset_statistics(&self) {
        self.statistics.reset();
    }

    /// Record statistics for a decision
    fn record_stats(&self, latency_us: u64, verdict: &ActionVerdict, was_projected: bool) {
        self.statistics.record_decision(latency_us);
        match verdict {
            ActionVerdict::Approved => { self.statistics.approved.fetch_add(1, Ordering::Relaxed); }
            ActionVerdict::Rejected(_) => { self.statistics.rejected.fetch_add(1, Ordering::Relaxed); }
            ActionVerdict::PendingApproval { .. } => { self.statistics.pending_approval.fetch_add(1, Ordering::Relaxed); }
            ActionVerdict::EmergencyBypassed { .. } => { self.statistics.emergency_bypassed.fetch_add(1, Ordering::Relaxed); }
        }
        if was_projected {
            self.statistics.projected.fetch_add(1, Ordering::Relaxed);
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
            let device_value = match operation.as_str() {
                "close" | "合闸" => Some(DeviceValue::Bool(true)),
                "open" | "分闸" => Some(DeviceValue::Bool(false)),
                "adjust_reactive" => Some(DeviceValue::Float64(*value)),
                _ => Some(DeviceValue::Float64(*value)),
            };
            let mut cmd = Command::new(
                cmd_type,
                *device_id,
                priority,
                &format!("{} device {} value {:.2}", operation, device_id, value),
            );
            // Set device routing for real execution
            cmd.device_id = Some(format!("device-{}", device_id));
            cmd.device_address = Some(format!("point-{}", device_id));
            cmd.device_value = device_value;
            cmd
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
    use eneros_constraint::projector::{NetworkSimulator, WhatIfResult};
    use async_trait::async_trait;
    use crate::executor::{CommandExecutor, ExecutionResult};
    use eneros_core::Result as CoreResult;
    use tokio::sync::Mutex;

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

    #[tokio::test]
    async fn test_pipeline_feasible_action_approved() {
        let pipeline = make_pipeline();
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let result = pipeline.decide(
            &action,
            AuthorityLevel::Supervisor,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        ).await;
        assert!(result.executed_action.is_some());
        assert!(result.audit.len() >= 2);
    }

    #[tokio::test]
    async fn test_pipeline_observer_rejected() {
        let pipeline = make_pipeline();
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let result = pipeline.decide(
            &action,
            AuthorityLevel::Observer,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        ).await;
        assert!(result.executed_action.is_none());
        assert!(matches!(result.verdict, ActionVerdict::Rejected(_)));
    }

    #[tokio::test]
    async fn test_pipeline_high_risk_requires_supervisor() {
        let pipeline = make_pipeline();
        let action = StructuredAction::ShedLoad { zone_id: 1, amount_mw: 50.0 };
        let result = pipeline.decide(
            &action,
            AuthorityLevel::Operator,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        ).await;
        assert!(result.executed_action.is_none() || matches!(result.verdict, ActionVerdict::PendingApproval { .. }));
    }

    #[tokio::test]
    async fn test_pipeline_batch() {
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
        ).await;
        assert_eq!(results.len(), 2);
    }

    // ── Enhanced API tests ──

    #[tokio::test]
    async fn test_enhanced_pipeline_feasible_action() {
        let pipeline = make_pipeline();
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );
        let result = pipeline.decide_enhanced(&action, &ctx).await;
        assert!(result.executed_action.is_some());
        assert!(result.pre_conditions.satisfied);
        assert!(result.decomposition.is_some());
        assert!(result.rollback_plan.is_some());
        assert!(result.total_latency_us > 0);
        // Should have: precondition + projection + validation + decomposition + execution + postcondition
        assert!(result.audit.len() >= 5, "Expected >= 5 audit entries, got {}", result.audit.len());
    }

    #[tokio::test]
    async fn test_enhanced_pipeline_observer_rejected() {
        let pipeline = make_pipeline();
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = DecisionContext::new(
            AuthorityLevel::Observer,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );
        let result = pipeline.decide_enhanced(&action, &ctx).await;
        assert!(result.executed_action.is_none());
        assert!(!result.pre_conditions.satisfied);
        assert!(matches!(result.verdict, ActionVerdict::Rejected(_)));
    }

    #[tokio::test]
    async fn test_enhanced_pipeline_isolate_fault_decomposed() {
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
        let result = pipeline.decide_enhanced(&action, &ctx).await;
        // IsolateFault should be decomposed into 2 steps
        if let Some(ref decomp) = result.decomposition {
            assert!(decomp.is_multi_step());
            assert_eq!(decomp.step_count(), 2);
        }
    }

    #[tokio::test]
    async fn test_enhanced_pipeline_statistics() {
        let pipeline = make_pipeline();
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );
        let _ = pipeline.decide_enhanced(&action, &ctx).await;
        let stats = pipeline.statistics();
        assert_eq!(stats.total_decisions, 1);
        assert!(stats.avg_latency_us > 0);
    }

    #[tokio::test]
    async fn test_enhanced_pipeline_batch() {
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
        let results = pipeline.decide_batch_enhanced(&actions, &ctx).await;
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

    // ── Auto-rollback tests (v0.6.0 — S6) ──

    /// Mock executor that records all executed commands for verification.
    struct RecordingExecutor {
        commands: Arc<Mutex<Vec<Command>>>,
    }

    impl RecordingExecutor {
        fn new() -> (Self, Arc<Mutex<Vec<Command>>>) {
            let cmds = Arc::new(Mutex::new(Vec::new()));
            (Self { commands: cmds.clone() }, cmds)
        }
    }

    #[async_trait]
    impl CommandExecutor for RecordingExecutor {
        async fn execute(&self, command: &Command) -> CoreResult<ExecutionResult> {
            self.commands.lock().await.push(command.clone());
            Ok(ExecutionResult::ok(
                format!("Executed command {} type {:?}", command.id, command.command_type),
                Duration::from_micros(100),
            ))
        }

        async fn read_back(&self, _command: &Command) -> Option<eneros_device::adapter::DataValue> {
            None
        }
    }

    /// Simulator that returns success on the first call (for projection) and
    /// a voltage violation on subsequent calls (to fail postcondition).
    struct FlakySimulator {
        call_count: std::sync::atomic::AtomicU32,
    }

    impl FlakySimulator {
        fn new() -> Self {
            Self {
                call_count: std::sync::atomic::AtomicU32::new(0),
            }
        }
    }

    impl NetworkSimulator for FlakySimulator {
        fn simulate_action(&self, _action: &StructuredAction) -> WhatIfResult {
            use std::sync::atomic::Ordering;
            let n = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;
            // First call: projection succeeds. Later calls: postcondition fails.
            if n <= 1 {
                WhatIfResult {
                    applicable: true,
                    converged: true,
                    voltage_violations: vec![],
                    thermal_violations: vec![],
                    all_constraints_satisfied: true,
                    summary: "OK".to_string(),
                }
            } else {
                WhatIfResult {
                    applicable: true,
                    converged: true,
                    voltage_violations: vec![(1, 0.90, 0.95)],
                    thermal_violations: vec![],
                    all_constraints_satisfied: false,
                    summary: "Voltage violation on bus 1".to_string(),
                }
            }
        }
        fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
            vec![(1, 0.0, 200.0)]
        }
        fn current_voltages(&self) -> Vec<(u64, f64)> {
            vec![(1, 1.02)]
        }
    }

    /// Build a pipeline with a flaky simulator that fails postcondition and a
    /// recording executor that tracks rollback commands.
    fn make_rollback_pipeline() -> (ConstrainedDecisionPipeline, Arc<Mutex<Vec<Command>>>) {
        let sim = FlakySimulator::new();
        let projector = Arc::new(FeasibilityProjector::new(Arc::new(sim)));
        let constraint_engine = Arc::new(ConstraintEngine::new());
        let (rec_exec, recorded_cmds) = RecordingExecutor::new();
        let gateway = Arc::new(SafetyGateway::with_executor(100, Arc::new(rec_exec)));
        let validator = Arc::new(ConstraintAwareValidator::with_default_interlocking(
            constraint_engine, gateway.clone(),
        ));
        let pipeline = ConstrainedDecisionPipeline::new(projector, validator, gateway);
        (pipeline, recorded_cmds)
    }

    #[tokio::test]
    async fn test_auto_rollback_triggered_on_postcondition_failure() {
        let (pipeline, recorded_cmds) = make_rollback_pipeline();
        // Use IsolateFault — it decomposes into 2 steps (atomic=true), so the
        // rollback plan will have undo steps and allow auto-rollback.
        let action = StructuredAction::IsolateFault {
            upstream_switch: 10,
            downstream_switch: 20,
        };
        let ctx = DecisionContext::new(
            AuthorityLevel::Emergency,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Emergency,
        ).with_device_states(crate::interlocking::DeviceStates::default());
        let result = pipeline.decide_enhanced(&action, &ctx).await;

        // Postcondition should have failed (flaky simulator returns violation)
        let post = result.post_conditions.as_ref().expect("postcondition should run");
        assert!(!post.satisfied, "Postcondition should fail");

        // Rollback plan should exist and allow auto-rollback
        let rb_plan = result.rollback_plan.as_ref().expect("rollback plan should exist");
        assert!(rb_plan.can_auto_rollback(), "auto-rollback should be allowed (steps={}, auto={}, strategy={:?})",
            rb_plan.steps.len(), rb_plan.auto_rollback, rb_plan.strategy);

        // Rollback should have been executed
        let rb_exec = result.rollback_executed.as_ref().expect("rollback should be executed");
        assert!(rb_exec.succeeded, "rollback should succeed");
        assert!(rb_exec.steps_attempted > 0, "at least one rollback step attempted");

        // The recording executor should have received the original commands + rollback commands
        // IsolateFault = 2 steps + 2 rollback steps = 4 commands minimum
        let cmds = recorded_cmds.lock().await;
        assert!(cmds.len() >= 4, "expected at least 4 commands (2 original + 2 rollback), got {}", cmds.len());

        // Statistics should reflect the rollback
        let stats = pipeline.statistics.snapshot();
        assert_eq!(stats.rollbacks_triggered, 1, "rollbacks_triggered should be 1");
        assert_eq!(stats.rollbacks_succeeded, 1, "rollbacks_succeeded should be 1");
        assert_eq!(stats.rollbacks_failed, 0, "rollbacks_failed should be 0");
        assert_eq!(stats.postcondition_failures, 1, "postcondition_failures should be 1");
    }

    #[tokio::test]
    async fn test_no_rollback_when_postcondition_satisfied() {
        // Use the normal (non-flaky) pipeline where postcondition passes
        let pipeline = make_pipeline();
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );
        let result = pipeline.decide_enhanced(&action, &ctx).await;

        // Postcondition should pass
        let post = result.post_conditions.as_ref().expect("postcondition should run");
        assert!(post.satisfied, "Postcondition should pass");

        // No rollback should have been executed
        assert!(result.rollback_executed.is_none(), "no rollback should be executed");

        let stats = pipeline.statistics.snapshot();
        assert_eq!(stats.rollbacks_triggered, 0, "no rollbacks should be triggered");
    }

    #[tokio::test]
    async fn test_rollback_audit_entry_added() {
        let (pipeline, _recorded_cmds) = make_rollback_pipeline();
        let action = StructuredAction::IsolateFault {
            upstream_switch: 10,
            downstream_switch: 20,
        };
        let ctx = DecisionContext::new(
            AuthorityLevel::Emergency,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Emergency,
        ).with_device_states(crate::interlocking::DeviceStates::default());
        let result = pipeline.decide_enhanced(&action, &ctx).await;

        // Should have a "rollback" audit entry
        let has_rollback_audit = result.audit.iter().any(|a| a.stage == "rollback");
        assert!(has_rollback_audit, "audit trail should contain a 'rollback' entry");
    }
}
