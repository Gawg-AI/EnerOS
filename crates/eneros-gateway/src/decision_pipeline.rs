use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use eneros_core::event::{Event, EventPayload, EventType};
use eneros_core::{ActionVerdict, AuthorityLevel, Jurisdiction, PowerObservation, StructuredAction, SystemOperatingState};
use eneros_constraint::projector::{FeasibilityProjector, ProjectionResult, WhatIfResult};
use eneros_eventbus::EventBus;
use crate::constraint_validator::ConstraintAwareValidator;
use crate::gateway::SafetyGateway;
use crate::command::{Command, CommandType, CommandPriority, DeviceValue};
use crate::precondition::PreConditionChecker;
use crate::postcondition::PostConditionVerifier;
use crate::decomposer::ActionDecomposer;
use crate::pipeline_types::{
    DecisionContext, EnhancedPipelineDecision, PipelineAuditEntry,
    PipelineStatistics, PipelineStatisticsSnapshot, RollbackExecution, RollbackPlan, RollbackStrategy,
};
use crate::watchdog::{WatchdogAction, WatchdogTimeoutRecord};
use crate::decision_cache::{DecisionCache, DecisionCacheStats, mark_cache_hit};

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
    /// Watchdog 超时处理策略（T029-10）。默认 Alert。
    ///
    /// 决定 watchdog 超时时管线执行的动作：Log/Alert/Degrade/Restart/Rollback。
    watchdog_action: WatchdogAction,
    /// 可选的 EventBus 引用，用于 Alert 策略发送告警事件（T029-10）。
    event_bus: Option<Arc<EventBus>>,
    /// 决策结果缓存（T029-15）。
    ///
    /// 启用后，相同输入（动作 + 上下文关键字段）在 TTL 内直接返回缓存结果，
    /// 避免重复执行完整决策管线。使用 with_cache() 配置。
    cache: Option<Arc<DecisionCache>>,
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
            watchdog_action: WatchdogAction::default(),
            event_bus: None,
            cache: None,
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
            watchdog_action: WatchdogAction::default(),
            event_bus: None,
            cache: None,
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
            watchdog_action: WatchdogAction::default(),
            event_bus: None,
            cache: None,
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
            watchdog_action: WatchdogAction::default(),
            event_bus: None,
            cache: None,
        }
    }

    /// Attach a watchdog timer to monitor command execution (v0.7.0).
    ///
    /// When set, each command in Stage 5 is registered with the watchdog.
    /// If a command exceeds `command_timeout`, the watchdog fires its
    /// timeout callback. The pipeline itself does not abort the command
    /// (the gateway's executor handles that); the watchdog is for
    /// observability and triggering external alerts/rollback.
    ///
    /// 超时处理策略由 `watchdog_action` 字段控制（T029-10），默认 `Alert`。
    /// 使用 `with_watchdog_action()` 可覆盖策略。
    pub fn with_watchdog(
        mut self,
        watchdog: Arc<crate::watchdog::WatchdogTimer>,
        command_timeout: Duration,
    ) -> Self {
        self.watchdog = Some(watchdog);
        self.command_timeout = command_timeout;
        self
    }

    /// 设置 watchdog 超时处理策略（T029-10）。
    ///
    /// 必须先调用 `with_watchdog()` 配置 watchdog，否则策略不生效。
    pub fn with_watchdog_action(mut self, action: WatchdogAction) -> Self {
        self.watchdog_action = action;
        self
    }

    /// 设置 EventBus 引用，用于 Alert 策略发送告警事件（T029-10）。
    pub fn with_event_bus(mut self, bus: Arc<EventBus>) -> Self {
        self.event_bus = Some(bus);
        self
    }

    /// 配置决策结果缓存（T029-15）。
    ///
    /// 启用后，相同输入（动作 + 上下文关键字段）在 TTL 内直接返回缓存结果，
    /// 避免重复执行完整决策管线。缓存使用 LRU + TTL 双策略淘汰。
    ///
    /// 缓存键基于 `StructuredAction` + `AuthorityLevel` + `Jurisdiction` +
    /// `SystemOperatingState` + `agent_id` 的哈希，不包含实时遥测字段
    ///（`observation` / `device_states` / `reasoning`），调用方应确保
    /// TTL 足够短以避免在实时状态变化后命中过期决策。
    pub fn with_cache(mut self, cache: Arc<DecisionCache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// 获取决策缓存统计快照（T029-15）。
    ///
    /// 返回 `None` 表示未配置缓存。可用于 API 查询缓存命中率、
    /// 当前条目数、淘汰次数等可观测指标。
    pub fn cache_stats(&self) -> Option<DecisionCacheStats> {
        self.cache.as_ref().map(|c| c.stats())
    }

    /// Process a single action through the enhanced pipeline using DecisionContext.
    ///
    /// 如果配置了决策缓存（T029-15），首先查询缓存：
    /// - 命中：直接返回缓存结果，附加 `cache_hit` 审计条目，延迟通常 < 100µs
    /// - 未命中：执行完整决策管线，将结果写入缓存后返回
    ///
    /// 缓存键基于动作 + 上下文关键字段的哈希，不包含实时遥测字段。
    /// 所有决策结果（包括 Rejected）都会被缓存，因为相同输入会产出相同结论。
    pub async fn decide_enhanced(
        &self,
        action: &StructuredAction,
        ctx: &DecisionContext,
    ) -> EnhancedPipelineDecision {
        // 未配置缓存时直接执行完整管线
        let cache = match &self.cache {
            None => return self.decide_enhanced_uncached(action, ctx).await,
            Some(c) => Arc::clone(c),
        };

        // 计算缓存键并查询缓存
        let key = DecisionCache::compute_key(action, ctx);
        let cache_start = Instant::now();
        if let Some(cached) = cache.get(key) {
            let hit_latency_us = cache_start.elapsed().as_micros() as u64;
            return mark_cache_hit(cached, hit_latency_us);
        }

        // 缓存未命中：执行完整决策管线
        let decision = self.decide_enhanced_uncached(action, ctx).await;

        // 写入缓存（所有决策结果都缓存，包括 Rejected，因为相同输入会产出相同结论）
        cache.insert(key, decision.clone());

        decision
    }

    /// 执行完整决策管线（无缓存）。
    ///
    /// 这是 `decide_enhanced` 的内部实现，执行 precondition → projection →
    /// validation → decomposition → execution → postcondition → rollback 全流程。
    async fn decide_enhanced_uncached(
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
                // Watchdog 超时策略状态（T029-10）
                let mut watchdog_degraded = false;
                let mut watchdog_rollback = false;
                for step in &decomposition.steps {
                    let cmd = structured_action_to_command(&step.action);

                    // 注册 watchdog guard（T029-10：使用 register_with_action）。
                    // guard 在循环迭代结束时 drop，若命令按时完成则取消 watchdog。
                    // 超时回调记录到 WatchdogTimeoutRecord，管线在 execute_command
                    // 返回后检查记录并执行异步动作（回滚/降级）。
                    let timeout_record = Arc::new(WatchdogTimeoutRecord::new());
                    let _watchdog_guard = self.watchdog.as_ref().map(|wd| {
                        let op_id = format!(
                            "cmd-step-{}-{}",
                            step.step_index,
                            chrono::Utc::now().timestamp_millis()
                        );
                        let record_clone = timeout_record.clone();
                        let op_id_for_cb = op_id.clone();
                        let action = self.watchdog_action;
                        let event_bus = self.event_bus.clone();
                        wd.register_with_action(
                            op_id,
                            self.command_timeout,
                            Box::new(move || {
                                record_clone.record(op_id_for_cb.clone());
                                Self::handle_watchdog_timeout(
                                    action,
                                    &op_id_for_cb,
                                    event_bus.as_ref(),
                                );
                            }),
                        )
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

                    // 检查 watchdog 是否在命令执行期间超时（T029-10）
                    if timeout_record.timed_out() {
                        let op_id = timeout_record.op_id().unwrap_or_default();
                        let timeout_msg = format!(
                            "Step {} watchdog timeout (action={:?}, op={})",
                            step.step_index, self.watchdog_action, op_id
                        );
                        match self.watchdog_action {
                            WatchdogAction::Log | WatchdogAction::Alert => {
                                // 回调已处理日志/事件，继续执行后续步骤
                                audit.push(PipelineAuditEntry {
                                    stage: "execution".to_string(),
                                    description: timeout_msg,
                                    duration_us: start.elapsed().as_micros() as u64,
                                    passed: true,
                                });
                            }
                            WatchdogAction::Degrade => {
                                // 降级：跳过剩余步骤，使用默认值
                                audit.push(PipelineAuditEntry {
                                    stage: "execution".to_string(),
                                    description: format!(
                                        "{} — degrading: skipping remaining steps",
                                        timeout_msg
                                    ),
                                    duration_us: start.elapsed().as_micros() as u64,
                                    passed: true,
                                });
                                watchdog_degraded = true;
                                break;
                            }
                            WatchdogAction::Restart => {
                                // 重启：中止执行
                                audit.push(PipelineAuditEntry {
                                    stage: "execution".to_string(),
                                    description: format!(
                                        "{} — restart requested",
                                        timeout_msg
                                    ),
                                    duration_us: start.elapsed().as_micros() as u64,
                                    passed: false,
                                });
                                execution_ok = false;
                                execution_error = timeout_msg;
                                break;
                            }
                            WatchdogAction::Rollback => {
                                // 回滚：中止执行，稍后触发回滚
                                audit.push(PipelineAuditEntry {
                                    stage: "execution".to_string(),
                                    description: format!(
                                        "{} — rollback triggered",
                                        timeout_msg
                                    ),
                                    duration_us: start.elapsed().as_micros() as u64,
                                    passed: false,
                                });
                                execution_ok = false;
                                execution_error = timeout_msg;
                                watchdog_rollback = true;
                                break;
                            }
                        }
                    }
                }

                let exec_duration = start.elapsed().as_micros() as u64;

                if execution_ok && !watchdog_degraded {
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

                // Watchdog 降级处理：跳过 postcondition，返回降级结果（T029-10）
                if watchdog_degraded {
                    let total_latency = pipeline_start.elapsed().as_micros() as u64;
                    self.record_stats(total_latency, &verdict, projection.is_projected());
                    return EnhancedPipelineDecision {
                        executed_action: Some(feasible_action.clone()),
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
                    };
                }

                // Watchdog 回滚处理：执行回滚计划后返回 Rejected（T029-10）
                if watchdog_rollback {
                    let mut rb_executed: Option<RollbackExecution> = None;
                    if rollback_plan.can_auto_rollback() {
                        let rb_execution = self.execute_rollback_plan(&rollback_plan).await;
                        let steps_attempted = rb_execution.steps_attempted;
                        let steps_succeeded = rb_execution.steps_succeeded;
                        let rb_duration = rb_execution.duration_us;
                        let rb_succeeded = rb_execution.succeeded;
                        audit.push(PipelineAuditEntry {
                            stage: "rollback".to_string(),
                            description: format!(
                                "Watchdog rollback executed: {} steps attempted, {} succeeded",
                                steps_attempted, steps_succeeded
                            ),
                            duration_us: rb_duration,
                            passed: rb_succeeded,
                        });
                        rb_executed = Some(rb_execution);
                    } else {
                        audit.push(PipelineAuditEntry {
                            stage: "rollback".to_string(),
                            description: format!(
                                "Watchdog rollback requested but auto-rollback not allowed (strategy={:?})",
                                rollback_plan.strategy
                            ),
                            duration_us: 0,
                            passed: false,
                        });
                    }

                    let total_latency = pipeline_start.elapsed().as_micros() as u64;
                    self.record_stats(
                        total_latency,
                        &ActionVerdict::Rejected(execution_error.clone()),
                        projection.is_projected(),
                    );
                    return EnhancedPipelineDecision {
                        executed_action: None,
                        original_action: action.clone(),
                        decomposition: Some(decomposition),
                        projection,
                        pre_conditions: pre_result,
                        post_conditions: None,
                        verdict: ActionVerdict::Rejected(execution_error),
                        rollback_plan: Some(rollback_plan),
                        rollback_executed: rb_executed,
                        audit,
                        total_latency_us: total_latency,
                    };
                }

                // If execution failed (non-watchdog), reject the action
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
                        let rb_execution = self.execute_rollback_plan(&rollback_plan).await;
                        let steps_attempted = rb_execution.steps_attempted;
                        let steps_succeeded = rb_execution.steps_succeeded;
                        let rb_duration = rb_execution.duration_us;
                        let rb_succeeded = rb_execution.succeeded;
                        audit.push(PipelineAuditEntry {
                            stage: "rollback".to_string(),
                            description: format!(
                                "Auto-rollback executed: {} steps attempted, {} succeeded",
                                steps_attempted, steps_succeeded
                            ),
                            duration_us: rb_duration,
                            passed: rb_succeeded,
                        });
                        rollback_executed = Some(rb_execution);
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

    /// 执行回滚计划（T029-10 提取，供 postcondition 失败和 watchdog 超时共用）。
    ///
    /// 按逆序执行回滚步骤，根据 `RollbackStrategy` 处理失败：
    /// - `BestEffort`：跳过失败步骤，继续执行
    /// - 其他策略：遇到失败即停止
    async fn execute_rollback_plan(&self, rollback_plan: &RollbackPlan) -> RollbackExecution {
        let rb_start = Instant::now();
        self.statistics.rollbacks_triggered.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(
            "Executing rollback ({} steps, strategy={:?})",
            rollback_plan.steps.len(),
            rollback_plan.strategy
        );

        let mut steps_succeeded = 0usize;
        let mut steps_attempted = 0usize;
        let mut rb_error: Option<String> = None;

        // 按逆序执行回滚步骤（后进先出）
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
                    // BestEffort 策略下继续；其他策略遇到失败即停止
                    if rollback_plan.strategy != RollbackStrategy::BestEffort {
                        break;
                    }
                }
            }
        }

        let rb_duration = rb_start.elapsed().as_micros() as u64;
        match rb_error {
            None => {
                self.statistics.rollbacks_succeeded.fetch_add(1, Ordering::Relaxed);
                tracing::info!(
                    "Rollback completed successfully ({} steps, {} µs)",
                    steps_succeeded, rb_duration
                );
                RollbackExecution::success(steps_succeeded, rb_duration)
            }
            Some(err) => {
                self.statistics.rollbacks_failed.fetch_add(1, Ordering::Relaxed);
                tracing::error!(
                    "Rollback FAILED after {}/{} steps: {}",
                    steps_succeeded, steps_attempted, err
                );
                RollbackExecution::failure(
                    steps_attempted,
                    steps_succeeded,
                    err,
                    rb_duration,
                )
            }
        }
    }

    /// Watchdog 超时回调的即时处理（T029-10）。
    ///
    /// 在同步回调上下文中执行：记录日志、发送 Alert 事件。
    /// 异步动作（回滚执行、降级跳过）由管线在 `execute_command` 返回后处理。
    fn handle_watchdog_timeout(
        action: WatchdogAction,
        op_id: &str,
        event_bus: Option<&Arc<EventBus>>,
    ) {
        match action {
            WatchdogAction::Log => {
                tracing::warn!("Watchdog timeout (Log): {}", op_id);
            }
            WatchdogAction::Alert => {
                tracing::error!("Watchdog timeout (Alert): {}", op_id);
                if let Some(bus) = event_bus {
                    let event = Event::new(
                        EventType::SystemAlarm,
                        "decision_pipeline",
                        EventPayload::Message(format!("Watchdog timeout: {}", op_id)),
                    );
                    if let Err(e) = bus.publish(event) {
                        tracing::error!("Failed to publish watchdog alert event: {}", e);
                    }
                }
            }
            WatchdogAction::Degrade => {
                tracing::error!("Watchdog timeout (Degrade): {}", op_id);
            }
            WatchdogAction::Restart => {
                tracing::error!("Watchdog timeout (Restart): {}", op_id);
            }
            WatchdogAction::Rollback => {
                tracing::error!("Watchdog timeout (Rollback): {}", op_id);
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
    use crate::watchdog::WatchdogTimer;
    use eneros_core::Result as CoreResult;
    use eneros_core::event::EventType;
    use eneros_eventbus::EventBus;
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

    // ── Watchdog action strategy tests (T029-10) ──

    /// 延迟执行器：在执行命令前 sleep 指定时长，用于触发 watchdog 超时。
    /// 同时记录所有执行的命令以供测试验证。
    struct DelayingExecutor {
        delay: Duration,
        commands: Arc<Mutex<Vec<Command>>>,
    }

    impl DelayingExecutor {
        fn new(delay: Duration) -> (Self, Arc<Mutex<Vec<Command>>>) {
            let cmds = Arc::new(Mutex::new(Vec::new()));
            (Self { delay, commands: cmds.clone() }, cmds)
        }
    }

    #[async_trait]
    impl CommandExecutor for DelayingExecutor {
        async fn execute(&self, command: &Command) -> CoreResult<ExecutionResult> {
            self.commands.lock().await.push(command.clone());
            // 真实延迟，使 watchdog 超时在 execute_command 期间触发
            tokio::time::sleep(self.delay).await;
            Ok(ExecutionResult::ok(
                format!("Executed command {} type {:?}", command.id, command.command_type),
                self.delay,
            ))
        }

        async fn read_back(&self, _command: &Command) -> Option<eneros_device::adapter::DataValue> {
            None
        }
    }

    /// 构建 watchdog 集成测试管线：短超时 + 延迟执行器 + 可配置策略
    fn make_watchdog_pipeline(
        action: WatchdogAction,
        delay: Duration,
        timeout: Duration,
        event_bus: Option<Arc<EventBus>>,
    ) -> (ConstrainedDecisionPipeline, Arc<Mutex<Vec<Command>>>, Arc<WatchdogTimer>) {
        let watchdog = Arc::new(WatchdogTimer::with_check_interval(
            timeout,
            Duration::from_millis(5),
        ));
        let (delay_exec, recorded_cmds) = DelayingExecutor::new(delay);
        let gateway = Arc::new(SafetyGateway::with_executor(100, Arc::new(delay_exec)));
        let projector = Arc::new(FeasibilityProjector::new(Arc::new(MockSimulator)));
        let constraint_engine = Arc::new(ConstraintEngine::new());
        let validator = Arc::new(ConstraintAwareValidator::with_default_interlocking(
            constraint_engine, gateway.clone(),
        ));
        let mut pipeline = ConstrainedDecisionPipeline::new(projector, validator, gateway)
            .with_watchdog(watchdog.clone(), timeout)
            .with_watchdog_action(action);
        if let Some(bus) = event_bus {
            pipeline = pipeline.with_event_bus(bus);
        }
        (pipeline, recorded_cmds, watchdog)
    }

    /// 测试 Log 策略：超时触发后仅记录日志，执行继续
    #[tokio::test]
    async fn test_watchdog_log_strategy() {
        let (pipeline, _cmds, watchdog) = make_watchdog_pipeline(
            WatchdogAction::Log,
            Duration::from_millis(100),
            Duration::from_millis(20),
            None,
        );
        let handle = watchdog.start();

        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );
        let result = pipeline.decide_enhanced(&action, &ctx).await;

        // Log 策略下执行继续，executed_action 应为 Some
        assert!(result.executed_action.is_some(), "Log strategy should continue execution");

        // 审计应包含 watchdog 超时记录
        let has_timeout = result.audit.iter().any(|a|
            a.stage == "execution" && a.description.contains("watchdog timeout")
        );
        assert!(has_timeout, "audit should contain watchdog timeout entry for Log strategy");

        watchdog.stop();
        handle.await.unwrap();
    }

    /// 测试 Alert 策略：超时触发后发送告警事件到 EventBus
    #[tokio::test]
    async fn test_watchdog_alert_strategy_publishes_event() {
        let event_bus = Arc::new(EventBus::new(16));
        let mut receiver = event_bus.subscribe();

        let (pipeline, _cmds, watchdog) = make_watchdog_pipeline(
            WatchdogAction::Alert,
            Duration::from_millis(100),
            Duration::from_millis(20),
            Some(event_bus),
        );
        let handle = watchdog.start();

        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );
        let result = pipeline.decide_enhanced(&action, &ctx).await;

        // 验证告警事件被发送到 EventBus
        let event_result = tokio::time::timeout(
            Duration::from_millis(500),
            receiver.recv(),
        ).await;
        assert!(event_result.is_ok(), "should receive alert event from EventBus");
        let event = event_result.unwrap().unwrap();
        assert_eq!(event.event_type, EventType::SystemAlarm,
            "event type should be SystemAlarm");

        // Alert 策略下执行继续
        assert!(result.executed_action.is_some(), "Alert strategy should continue execution");

        // 审计应包含 watchdog 超时记录
        let has_timeout = result.audit.iter().any(|a|
            a.stage == "execution" && a.description.contains("watchdog timeout")
        );
        assert!(has_timeout, "audit should contain watchdog timeout entry for Alert strategy");

        watchdog.stop();
        handle.await.unwrap();
    }

    /// 测试 Degrade 策略：超时触发后跳过剩余步骤
    #[tokio::test]
    async fn test_watchdog_degrade_strategy_skips_remaining_steps() {
        // IsolateFault 分解为 2 步，第一步超时后应跳过第二步
        let (pipeline, recorded_cmds, watchdog) = make_watchdog_pipeline(
            WatchdogAction::Degrade,
            Duration::from_millis(100),
            Duration::from_millis(20),
            None,
        );
        let handle = watchdog.start();

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

        // Degrade 策略下 executed_action 应为 Some（降级执行，非失败）
        assert!(result.executed_action.is_some(),
            "Degrade strategy should set executed_action (degraded, not failed)");

        // post_conditions 应为 None（跳过了 postcondition 验证）
        assert!(result.post_conditions.is_none(),
            "Degrade strategy should skip postcondition verification");

        // 审计应包含降级记录
        let has_degrade = result.audit.iter().any(|a|
            a.stage == "execution" && a.description.contains("degrading")
        );
        assert!(has_degrade, "audit should contain degrade entry");

        // 只应执行第一步（第二步被跳过）
        let cmds = recorded_cmds.lock().await;
        assert_eq!(cmds.len(), 1,
            "Degrade should execute only 1 step (skipping remaining), got {}", cmds.len());

        watchdog.stop();
        handle.await.unwrap();
    }

    /// 测试 Rollback 策略：超时触发后执行回滚计划
    #[tokio::test]
    async fn test_watchdog_rollback_strategy_triggers_rollback() {
        // IsolateFault 分解为 2 步，有回滚计划
        let (pipeline, recorded_cmds, watchdog) = make_watchdog_pipeline(
            WatchdogAction::Rollback,
            Duration::from_millis(100),
            Duration::from_millis(20),
            None,
        );
        let handle = watchdog.start();

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

        // Rollback 策略下 executed_action 应为 None（执行被中止）
        assert!(result.executed_action.is_none(),
            "Rollback strategy should not set executed_action");

        // verdict 应为 Rejected
        assert!(matches!(result.verdict, ActionVerdict::Rejected(_)),
            "Rollback strategy should result in Rejected verdict");

        // rollback_executed 应为 Some（回滚被执行）
        let rb_exec = result.rollback_executed.as_ref()
            .expect("rollback should be executed on watchdog Rollback strategy");
        assert!(rb_exec.steps_attempted > 0,
            "rollback should have attempted at least 1 step");

        // 审计应包含 watchdog 回滚记录
        let has_watchdog_rollback = result.audit.iter().any(|a|
            a.stage == "rollback" && a.description.contains("Watchdog rollback")
        );
        assert!(has_watchdog_rollback, "audit should contain watchdog rollback entry");

        // 应执行了 1 个原始命令 + 回滚步骤
        let cmds = recorded_cmds.lock().await;
        assert!(cmds.len() >= 2,
            "should execute at least 1 original + 1 rollback command, got {}", cmds.len());

        // 统计应反映回滚
        let stats = pipeline.statistics();
        assert_eq!(stats.rollbacks_triggered, 1,
            "rollbacks_triggered should be 1");

        watchdog.stop();
        handle.await.unwrap();
    }

    /// 测试 Restart 策略：超时触发后中止执行，返回 Rejected
    #[tokio::test]
    async fn test_watchdog_restart_strategy() {
        let (pipeline, _cmds, watchdog) = make_watchdog_pipeline(
            WatchdogAction::Restart,
            Duration::from_millis(100),
            Duration::from_millis(20),
            None,
        );
        let handle = watchdog.start();

        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );
        let result = pipeline.decide_enhanced(&action, &ctx).await;

        // Restart 策略下 executed_action 应为 None（执行被中止）
        assert!(result.executed_action.is_none(),
            "Restart strategy should not set executed_action");

        // verdict 应为 Rejected
        assert!(matches!(result.verdict, ActionVerdict::Rejected(_)),
            "Restart strategy should result in Rejected verdict");

        // 不应执行回滚
        assert!(result.rollback_executed.is_none(),
            "Restart strategy should not trigger rollback");

        // 审计应包含 restart 记录
        let has_restart = result.audit.iter().any(|a|
            a.stage == "execution" && a.description.contains("restart")
        );
        assert!(has_restart, "audit should contain restart entry");

        watchdog.stop();
        handle.await.unwrap();
    }

    // ── 决策缓存集成测试（T029-15）──

    /// 构建带缓存的决策管线
    fn make_cached_pipeline(
        max_size: usize,
        ttl: Duration,
    ) -> (ConstrainedDecisionPipeline, Arc<DecisionCache>) {
        let cache = Arc::new(DecisionCache::new(max_size, ttl));
        let projector = Arc::new(FeasibilityProjector::new(Arc::new(MockSimulator)));
        let constraint_engine = Arc::new(ConstraintEngine::new());
        let gateway = Arc::new(SafetyGateway::new(100));
        let validator = Arc::new(ConstraintAwareValidator::with_default_interlocking(
            constraint_engine, gateway.clone(),
        ));
        let pipeline = ConstrainedDecisionPipeline::new(projector, validator, gateway)
            .with_cache(cache.clone());
        (pipeline, cache)
    }

    /// 测试缓存命中：首次调用未命中并写入缓存，第二次调用命中缓存
    #[tokio::test]
    async fn test_pipeline_cache_hit_returns_cached_result() {
        let (pipeline, cache) = make_cached_pipeline(64, Duration::from_secs(60));
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );

        // 首次调用：缓存未命中，执行完整管线
        let result1 = pipeline.decide_enhanced(&action, &ctx).await;
        assert!(!result1.audit.iter().any(|a| a.stage == "cache_hit"),
            "first call should not have cache_hit audit");
        assert_eq!(cache.misses(), 1, "first call should be a miss");
        assert_eq!(cache.hits(), 0, "no hits yet");
        assert_eq!(cache.len(), 1, "cache should have 1 entry after first call");

        // 第二次调用：缓存命中，附加 cache_hit 审计条目
        let result2 = pipeline.decide_enhanced(&action, &ctx).await;
        let has_cache_hit = result2.audit.iter().any(|a| a.stage == "cache_hit");
        assert!(has_cache_hit, "second call should have cache_hit audit entry");
        assert_eq!(cache.hits(), 1, "second call should be a hit");
        assert_eq!(cache.misses(), 1, "misses unchanged");

        // 两次调用的 verdict 应一致
        assert_eq!(
            std::mem::discriminant(&result1.verdict),
            std::mem::discriminant(&result2.verdict),
            "cached verdict should match original"
        );
    }

    /// 测试缓存未命中时执行完整管线（审计条目完整）
    #[tokio::test]
    async fn test_pipeline_cache_miss_executes_full_pipeline() {
        let (pipeline, _cache) = make_cached_pipeline(64, Duration::from_secs(60));
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );

        let result = pipeline.decide_enhanced(&action, &ctx).await;

        // 完整管线应包含 precondition + projection + validation + decomposition + execution + postcondition
        assert!(result.audit.len() >= 5,
            "uncached call should have full audit trail, got {} entries", result.audit.len());
        assert!(result.total_latency_us > 0, "uncached call should have real latency");
    }

    /// 测试缓存统计：hits、misses、len 正确追踪
    #[tokio::test]
    async fn test_pipeline_cache_stats_tracked() {
        let (pipeline, _cache) = make_cached_pipeline(64, Duration::from_secs(60));
        let ctx = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );

        // 两个不同动作
        let action1 = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let action2 = StructuredAction::StartGenerator { gen_id: 2, target_mw: 100.0 };

        // action1: miss
        let _ = pipeline.decide_enhanced(&action1, &ctx).await;
        // action1: hit
        let _ = pipeline.decide_enhanced(&action1, &ctx).await;
        // action2: miss
        let _ = pipeline.decide_enhanced(&action2, &ctx).await;
        // action1: hit again
        let _ = pipeline.decide_enhanced(&action1, &ctx).await;

        let stats = pipeline.cache_stats().expect("cache should be configured");
        assert_eq!(stats.hits, 2, "should have 2 hits");
        assert_eq!(stats.misses, 2, "should have 2 misses");
        assert_eq!(stats.len, 2, "should have 2 cached entries");
        assert!((stats.hit_rate - 0.5).abs() < 1e-9, "hit rate should be 0.5");
    }

    /// 测试未配置缓存时 cache_stats() 返回 None，且不附加 cache_hit 审计
    #[tokio::test]
    async fn test_pipeline_no_cache_when_not_configured() {
        let pipeline = make_pipeline();
        assert!(pipeline.cache_stats().is_none(), "no cache should return None");

        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );

        // 多次调用都不应有 cache_hit
        let result1 = pipeline.decide_enhanced(&action, &ctx).await;
        let result2 = pipeline.decide_enhanced(&action, &ctx).await;

        assert!(!result1.audit.iter().any(|a| a.stage == "cache_hit"),
            "uncached pipeline should never produce cache_hit");
        assert!(!result2.audit.iter().any(|a| a.stage == "cache_hit"),
            "uncached pipeline should never produce cache_hit");
    }

    /// 测试缓存命中走缓存路径而非完整管线
    ///
    /// 在测试环境（mock 组件）中完整管线延迟可能仅几十 µs，与缓存查找
    /// 延迟处于同一量级。因此不比较绝对延迟，而是验证：
    /// 1. 缓存命中附加 `cache_hit` 审计条目
    /// 2. 缓存命中延迟不超过完整管线延迟
    #[tokio::test]
    async fn test_pipeline_cache_hit_uses_cache_path() {
        let (pipeline, _cache) = make_cached_pipeline(64, Duration::from_secs(60));
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );

        // 首次调用（未命中）：完整管线
        let uncached = pipeline.decide_enhanced(&action, &ctx).await;
        let uncached_latency = uncached.total_latency_us;
        assert!(!uncached.audit.iter().any(|a| a.stage == "cache_hit"),
            "first call should not have cache_hit");

        // 第二次调用（命中）：缓存路径
        let cached = pipeline.decide_enhanced(&action, &ctx).await;
        let cached_latency = cached.total_latency_us;

        // 缓存命中应附加 cache_hit 审计条目
        assert!(cached.audit.iter().any(|a| a.stage == "cache_hit"),
            "second call should have cache_hit audit entry");

        // 缓存命中延迟不应超过完整管线延迟
        //（在快速机器上可能相等，但绝不应更慢）
        assert!(cached_latency <= uncached_latency,
            "cached latency ({}) should not exceed uncached latency ({})",
            cached_latency, uncached_latency);
    }

    /// 测试不同动作产生不同缓存条目
    #[tokio::test]
    async fn test_pipeline_cache_different_actions_separate_entries() {
        let (pipeline, cache) = make_cached_pipeline(64, Duration::from_secs(60));
        let ctx = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );

        let action1 = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let action2 = StructuredAction::StartGenerator { gen_id: 2, target_mw: 200.0 };

        // 两个不同动作都未命中
        let _ = pipeline.decide_enhanced(&action1, &ctx).await;
        let _ = pipeline.decide_enhanced(&action2, &ctx).await;

        assert_eq!(cache.len(), 2, "two different actions should create 2 entries");
        assert_eq!(cache.misses(), 2, "both should be misses");

        // 重复调用应命中
        let _ = pipeline.decide_enhanced(&action1, &ctx).await;
        let _ = pipeline.decide_enhanced(&action2, &ctx).await;

        assert_eq!(cache.hits(), 2, "both should hit on second call");
    }

    /// 测试缓存 TTL 过期后重新执行完整管线
    #[tokio::test]
    async fn test_pipeline_cache_ttl_expiration() {
        let (pipeline, cache) = make_cached_pipeline(64, Duration::from_millis(50));
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );

        // 首次调用：未命中
        let _ = pipeline.decide_enhanced(&action, &ctx).await;
        assert_eq!(cache.misses(), 1);

        // 立即再次调用：命中
        let _ = pipeline.decide_enhanced(&action, &ctx).await;
        assert_eq!(cache.hits(), 1);

        // 等待 TTL 过期
        tokio::time::sleep(Duration::from_millis(60)).await;

        // 过期后调用：未命中，重新执行管线
        let result = pipeline.decide_enhanced(&action, &ctx).await;
        assert!(!result.audit.iter().any(|a| a.stage == "cache_hit"),
            "expired entry should not produce cache_hit");
        assert_eq!(cache.misses(), 2, "should be a miss after TTL expiration");
        assert_eq!(cache.expirations(), 1, "should have 1 expiration");
    }

    /// 测试缓存命中率 > 60%（模拟重复决策场景）
    #[tokio::test]
    async fn test_pipeline_cache_hit_rate_above_60_percent() {
        let (pipeline, cache) = make_cached_pipeline(256, Duration::from_secs(60));
        let ctx = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );

        // 模拟 Agent 决策模式：10 个不同动作，每个重复 10 次（90% 重复率）
        // 模拟方式：随机穿插调用，但总共有 10 个唯一动作 × 10 次重复 = 100 次调用
        let actions: Vec<StructuredAction> = (0..10u64)
            .map(|i| StructuredAction::StartGenerator { gen_id: i, target_mw: 100.0 })
            .collect();

        // 每个动作调用 10 次
        for _ in 0..10 {
            for action in &actions {
                let _ = pipeline.decide_enhanced(action, &ctx).await;
            }
        }

        let stats = cache.stats();
        // 100 次调用：10 次未命中 + 90 次命中 = 90% 命中率
        assert_eq!(stats.misses, 10, "should have 10 misses (one per unique action)");
        assert_eq!(stats.hits, 90, "should have 90 hits");
        assert!(stats.hit_rate > 0.6,
            "hit rate {:.2} should be > 60%", stats.hit_rate);
        assert_eq!(stats.len, 10, "should have 10 cached entries");
    }
}
