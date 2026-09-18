//! Engine integration for the durable plan owner. This is a child of engine so
//! it can reuse the unique command chain without exposing mutable internals.
use eneros_agent_sdk::snap_input_wire::TrustedInput;
use eneros_decision_contract::{CandidatePlan, CommandBatch, CommandIntent};

use super::*;
use crate::held_plan::{DurablePlans, HeldPlan};
use crate::saga::{BatchState, DriveReport};

/// ADR-0054:29–30：**持有期间保留证书引用的冻结输入与事件**的装配面。
///
/// hold = 新计划准入前登记（fail-closed：false ⇒ 调用方必须拒绝准入——
/// 必要输入可能被回收时不得继续依赖该输入签发）；release = 计划终态且
/// 无未决后解除（幂等；失败可重试——释放是回收面不是安全面）。钉集是
/// RAM 派生态，唯一持久事实 = 持有计划本身：重启后由
/// [`DispatchEngine::set_input_retainer`] / `install_held_plans` 按
/// 非终态计划重派生。
pub trait HeldInputRetainer {
    /// 登记快照钉（幂等）。false = 面不可用/容量满。
    fn hold(&mut self, snapshot_id: [u8; 16]) -> bool;
    /// 解除快照钉（幂等）。false = 未钉/面不可用（可重试）。
    fn release(&mut self, snapshot_id: [u8; 16]) -> bool;
}

/// fail-closed 保留面：一切 hold 拒绝（装配缺失时**不得**静默放行准入——
/// ADR-0054:29–30 的反面 enforcement）。
pub struct NoRetention;
impl HeldInputRetainer for NoRetention {
    fn hold(&mut self, _snapshot_id: [u8; 16]) -> bool {
        false
    }
    fn release(&mut self, _snapshot_id: [u8; 16]) -> bool {
        false
    }
}

impl DispatchEngine {
    /// Assembly supplies an already opened registered region. No RAM fallback
    /// is permitted for production multi-step plans.
    pub fn install_held_plans(&mut self, plans: DurablePlans) {
        self.repin_held_inputs();
        self.held_plans = Some(plans);
        self.repin_held_inputs();
    }

    /// 装配保留面（ADR-0054:29–30）。装配后立即按当前非终态计划重派生
    /// 钉集（断电恢复：钉是 RAM 态，重启后从这里重建）。
    pub fn set_input_retainer(&mut self, retainer: alloc::boxed::Box<dyn HeldInputRetainer>) {
        self.input_retainer = Some(retainer);
        self.repin_held_inputs();
    }

    /// 供装配查询：保留面是否已装配（未装配 ⇒ 生产应装 NoRetention 而非
    /// 留空——留空仅限 host 测试）。
    pub fn input_retainer_installed(&self) -> bool {
        self.input_retainer.is_some()
    }

    /// 按当前**非终态**计划重派生钉集（安装/装配两处调用；失败计数不
    /// 静默——已有计划的钉失败不回滚计划本身，只登记可观测事实：后续
    /// drive 的 fail-closed 面仍是输入有效性判据）。
    fn repin_held_inputs(&mut self) {
        let Some(store) = &self.held_plans else {
            return;
        };
        let ids: Vec<[u8; 16]> = store
            .plans()
            .unwrap_or_default()
            .iter()
            .filter(|p| !p.terminal())
            .filter_map(|p| {
                eneros_agent_sdk::snap_input_wire::TrustedInput::decode(&p.input)
                    .map(|i| i.facts().snapshot_id)
            })
            .collect();
        for id in ids {
            if let Some(retainer) = self.input_retainer.as_mut() {
                if !retainer.hold(id) {
                    self.held_pin_failures = self.held_pin_failures.saturating_add(1);
                }
            }
        }
    }

    pub fn block_unavailable_held_store(&mut self) {
        self.held_store_unavailable = true;
    }

    pub(super) fn held_device_busy(
        &self,
        device: u32,
        owner: Option<eneros_decision_contract::PlanId>,
    ) -> Result<bool, DispatchError> {
        if self.held_store_unavailable {
            return Err(DispatchError::IntentPersistFailed(String::from(
                "held store unavailable; control blocked",
            )));
        }
        let Some(store) = &self.held_plans else {
            return Ok(false);
        };
        let plans = store.plans().map_err(DispatchError::IntentPersistFailed)?;
        if owner.is_some()
            && plans
                .iter()
                .find(|p| !p.terminal() || p.unresolved())
                .map(|p| *p.plan.id())
                == owner
        {
            return Ok(false);
        }
        Ok(plans.iter().any(|p| {
            Some(*p.plan.id()) != owner
                && (!p.terminal() || p.unresolved())
                && p.batches
                    .iter()
                    .any(|b| b.intents.iter().any(|i| u32::from(i.device_id()) == device))
        }))
    }

    fn held_receipt(&self, plan: &HeldPlan) -> Result<DispatchReceipt, DispatchError> {
        let reports: Vec<_> = plan
            .batches
            .iter()
            .map(|b| b.saga().map(|s| s.report()))
            .collect::<Result<_, _>>()
            .map_err(DispatchError::IntentPersistFailed)?;
        let state = if reports
            .iter()
            .all(|r| r.final_state == BatchState::Completed)
        {
            DispatchState::Committed
        } else if plan.unresolved() {
            DispatchState::Indeterminate
        } else if plan.terminal() {
            DispatchState::Failed
        } else {
            DispatchState::Persisted
        };
        Ok(DispatchReceipt {
            command_id: plan.plan.id().0,
            correlation_id: plan.plan.id().0,
            final_state: state,
            state_path: alloc::vec![DispatchState::Validated, DispatchState::Persisted, state],
            committed_at_ms: state.is_committed().then(|| (self.now_fn)()),
            latency_ms: 0,
            note: Some(alloc::format!(
                "held-plan batches={} state={state:?}",
                reports.len()
            )),
        })
    }

    pub(super) fn accept_held_plan(
        &mut self,
        plan: &CandidatePlan,
        certificate: crate::certify::CertifierReply,
        input: &TrustedInput,
        telemetry: &TrustedTelemetry,
        actor: &AuthenticatedActor,
        batches: &[CommandBatch],
    ) -> Result<DispatchReceipt, DispatchError> {
        if self.held_store_unavailable || self.control_mode != crate::arbiter::ControlMode::Auto {
            return Err(DispatchError::NotAuthorized(String::from(
                "held plan storage/control mode unavailable",
            )));
        }
        let existing = self
            .held_plans
            .as_ref()
            .ok_or_else(|| DispatchError::IntentPersistFailed(String::from("held store absent")))?
            .plans()
            .map_err(DispatchError::IntentPersistFailed)?;
        if let Some(previous) = existing.iter().find(|p| p.plan.id() == plan.id()) {
            if previous.plan.encode_v1() != plan.encode_v1() {
                return Err(DispatchError::IdempotencyConflict(String::from(
                    "held plan identity reused",
                )));
            }
            return self.held_receipt(previous);
        }
        // An unresolved physical effect blocks admission. A healthy active plan
        // may have one queued successor; only the active plan can write.
        if existing.iter().any(|p| p.unresolved()) || !self.journal.pending().is_empty() {
            return Err(DispatchError::NotAuthorized(String::from(
                "held admission needs reconciliation",
            )));
        }
        // ADR-0054:29–30：准入前先钉住证书引用的冻结输入（fail-closed——
        // 钉面不可用/容量满 ⇒ 拒绝准入：必要输入可能被回收时不得继续
        // 依赖该输入签发）。
        let snapshot_id = input.facts().snapshot_id;
        if let Some(retainer) = self.input_retainer.as_mut() {
            if !retainer.hold(snapshot_id) {
                self.held_pin_failures = self.held_pin_failures.saturating_add(1);
                return Err(DispatchError::NotAuthorized(String::from(
                    "input retention unavailable: held plan needs pinned frozen snapshot",
                )));
            }
        }
        if let Err(e) = self
            .held_plans
            .as_mut()
            .ok_or_else(|| DispatchError::IntentPersistFailed(String::from("held store absent")))?
            .accept(plan, certificate, input.encode(), *actor, batches)
        {
            // 持久化失败回滚钉（不留永久钉——释放面与准入面对称）。
            if let Some(retainer) = self.input_retainer.as_mut() {
                let _ = retainer.release(snapshot_id);
            }
            return Err(DispatchError::IntentPersistFailed(e));
        }
        eneros_runtime::println!(
            "[svc-control-dispatch] PLAN-HELD plan={:02x?} batches={} intents={}",
            &plan.id().0[..4],
            batches.len(),
            batches.iter().map(|b| b.intents().len()).sum::<usize>()
        );
        self.drive_held_plans(Some(input), Some(telemetry))?;
        let held = self
            .held_plans
            .as_ref()
            .ok_or_else(|| DispatchError::IntentPersistFailed(String::from("held store absent")))?
            .plans()
            .map_err(DispatchError::IntentPersistFailed)?;
        let accepted = held
            .iter()
            .find(|p| p.plan.id() == plan.id())
            .ok_or_else(|| {
                DispatchError::Internal(String::from("accepted held plan disappeared"))
            })?;
        self.held_receipt(accepted)
    }

    /// Independent periodic tick: no proposal is needed. Recovery is read-only,
    /// and changing/losing the frozen input cancels every unsent command.
    pub fn drive_held_plans(
        &mut self,
        input: Option<&TrustedInput>,
        telemetry: Option<&TrustedTelemetry>,
    ) -> Result<Vec<DriveReport>, DispatchError> {
        let Some(store) = &self.held_plans else {
            return Ok(Vec::new());
        };
        let plans = store.plans().map_err(DispatchError::IntentPersistFailed)?;
        let mut reports = Vec::new();
        let Some(plan) = plans.iter().find(|p| !p.terminal()) else {
            return Ok(reports);
        };
        let now = (self.now_fn)();
        let allowed = !plan.recovered
            && input
                .zip(telemetry)
                .map(|(input, telemetry)| {
                    input.encode() == plan.input
                        && telemetry.is_valid_at(now)
                        && self
                            .admit_trusted_input(&plan.plan, Some(input.facts()), now)
                            .is_ok()
                        && crate::certify::verify_reply(
                            &plan.plan,
                            &plan.plan.encode_v1(),
                            &plan.certificate,
                            self.certifier
                                .verifier_ring()
                                .unwrap_or(&self.certifier_ring),
                            now,
                        )
                        .is_ok()
                })
                .unwrap_or(false);
        let mut earlier_failed = false;
        for batch in &plan.batches {
            let mut saga = self
                .held_plans
                .as_ref()
                .ok_or_else(|| {
                    DispatchError::IntentPersistFailed(String::from("held store absent"))
                })?
                .saga(batch)
                .map_err(DispatchError::IntentPersistFailed)?;
            if saga.state().is_terminal() {
                earlier_failed |= saga.state() != BatchState::Completed;
                continue;
            }
            if !allowed || earlier_failed {
                let reason = if plan.recovered {
                    "restart: old monotonic window cancelled; replan requires fresh certification"
                } else if earlier_failed {
                    "earlier batch failed"
                } else {
                    "trusted input or certificate no longer valid"
                };
                saga.cancel_pending(reason, now)
                    .map_err(DispatchError::IntentPersistFailed)?;
            }
            let report = if allowed && !earlier_failed {
                let telemetry = telemetry.ok_or(DispatchError::TelemetryExpired)?;
                let mut executor = EngineSagaExecutor::new(
                    self,
                    telemetry,
                    &plan.actor,
                    *plan.plan.id(),
                    *plan.plan.input_digest(),
                    CandidatePlan::plan_wire_sha256(&plan.plan),
                    plan.plan.ttl_ms(),
                    crate::compiler::is_legacy_single_command_plan(&plan.plan),
                    "pcs",
                    5_000,
                );
                saga.drive(&mut executor, now)
            } else {
                saga.drive(&mut ReadOnlyRecovery { engine: self }, now)
            }
            .map_err(DispatchError::IntentPersistFailed)?;
            eneros_runtime::println!(
                "[svc-control-dispatch] HELD-BATCH batch={:02x?} state={:?} events={}",
                &report.batch_id[..4],
                report.final_state,
                report.events
            );
            for command in &report.commands {
                eneros_runtime::println!("[svc-control-dispatch] HELD-COMMAND intent={:02x?} state={:?} attempts={} readback={:?} reason={}", &command.intent_id[..4], command.state, command.attempts, command.readback, command.reason.as_deref().unwrap_or("-"));
            }
            // One compiled batch per valid tick bounds work. Invalid/recovery
            // ticks may cancel at most the admitted bounded plan's commands.
            reports.push(report);
            if allowed && !earlier_failed {
                break;
            }
        }
        self.finalize_held_plans();
        Ok(reports)
    }

    /// 终局收尾（ADR-0054:29–30 + 15–16）：计划全部批终态后
    /// ① 打印一次性 `HELD-PLAN-DONE` 证据行（含逐命令终态计数——验收
    ///    从原始工件重算守恒，不搜词形凑数）；
    /// ② 解除该计划证书引用的冻结输入钉（终态且无未决；未决计划保留
    ///    钉至对账收敛）。释放失败保留在册，下个 tick 重试（幂等）。
    fn finalize_held_plans(&mut self) {
        let Some(store) = &self.held_plans else {
            return;
        };
        let plans = match store.plans() {
            Ok(p) => p,
            Err(_) => return,
        };
        struct PlanFinality {
            plan_id: eneros_decision_contract::PlanId,
            snapshot_id: [u8; 16],
            terminal: bool,
            unresolved: bool,
            reports: alloc::vec::Vec<DriveReport>,
        }
        let mut actions: Vec<PlanFinality> = Vec::new();
        for plan in &plans {
            let reports: alloc::vec::Vec<DriveReport> = plan
                .batches
                .iter()
                .map(|b| b.saga().map(|s| s.report()))
                .collect::<Result<_, _>>()
                .unwrap_or_default();
            let all_terminal = reports
                .iter()
                .all(|r| r.final_state != BatchState::Executing);
            if !all_terminal {
                continue;
            }
            let snapshot_id = eneros_agent_sdk::snap_input_wire::TrustedInput::decode(&plan.input)
                .map(|i| i.facts().snapshot_id);
            if let Some(snapshot_id) = snapshot_id {
                actions.push(PlanFinality {
                    plan_id: *plan.plan.id(),
                    snapshot_id,
                    terminal: plan.terminal(),
                    unresolved: plan.unresolved(),
                    reports,
                });
            }
        }
        for PlanFinality {
            plan_id,
            snapshot_id,
            terminal,
            unresolved,
            reports,
        } in actions
        {
            let committed = reports
                .iter()
                .all(|r| r.final_state == BatchState::Completed);
            // 逐命令全态计数（Applied / Blocked+Pending / 其余非完成——
            // 守恒口径 = 三桶之和恒等于命令总数）。
            let (applied, blocked, failed) =
                reports
                    .iter()
                    .fold((0usize, 0usize, 0usize), |(a, b, f), r| {
                        let total = r.commands.len();
                        let ap = r
                            .commands
                            .iter()
                            .filter(|c| c.state == crate::saga::CmdState::Applied)
                            .count();
                        let bl = r
                            .commands
                            .iter()
                            .filter(|c| {
                                matches!(
                                    c.state,
                                    crate::saga::CmdState::Blocked | crate::saga::CmdState::Pending
                                )
                            })
                            .count();
                        (a + ap, b + bl, f + total - ap - bl)
                    });
            if !self.held_done_logged.contains(&plan_id) {
                self.held_done_logged.push(plan_id);
                if self.held_done_logged.len() > 16 {
                    self.held_done_logged.remove(0);
                }
                eneros_runtime::println!(
                    "[svc-control-dispatch] HELD-PLAN-DONE plan={:02x?} batches={} applied={} blocked={} failed={} state={}",
                    &plan_id.0[..4],
                    reports.len(),
                    applied,
                    blocked,
                    failed,
                    if committed { "Committed" } else { "Terminal" }
                );
            }
            // 释放面：终态且无未决（未决 = 对账中——钉保留至收敛）。
            if terminal && !unresolved && !self.released_holds.contains(&snapshot_id) {
                let released = self
                    .input_retainer
                    .as_mut()
                    .map(|r| r.release(snapshot_id))
                    .unwrap_or(false);
                if released {
                    self.released_holds.push(snapshot_id);
                    if self.released_holds.len() > 16 {
                        self.released_holds.remove(0);
                    }
                }
            }
        }
    }

    /// Read-only per-command query from the durable source of truth.
    pub fn held_plan_reports(&self) -> Result<Vec<DriveReport>, DispatchError> {
        let Some(store) = &self.held_plans else {
            return Ok(Vec::new());
        };
        store
            .plans()
            .map_err(DispatchError::IntentPersistFailed)?
            .iter()
            .flat_map(|p| &p.batches)
            .map(|b| {
                b.saga()
                    .map(|s| s.report())
                    .map_err(DispatchError::IntentPersistFailed)
            })
            .collect()
    }
}
struct ReadOnlyRecovery<'a> {
    engine: &'a mut DispatchEngine,
}
impl SagaExecutor for ReadOnlyRecovery<'_> {
    fn applied_durable(
        &mut self,
        intent: &CommandIntent,
        readback: Option<f64>,
    ) -> Result<(), String> {
        let correlation =
            crate::compiler::plan_command_correlation(intent.plan_id(), intent.intent_id(), false);
        self.engine
            .settle_saga_applied(intent, correlation, readback)
    }

    fn execute(&mut self, _: &CommandIntent, _: u32) -> CmdOutcome {
        CmdOutcome::Rejected {
            reason: String::from("recovery is read-only"),
        }
    }
    fn reconcile(&mut self, intent: &CommandIntent) -> ReconcileOutcome {
        self.engine.reconcile_saga_intent(intent, 5_000)
    }
    fn compensate(&mut self, _: &CommandIntent, _: f64) -> CmdOutcome {
        CmdOutcome::Rejected {
            reason: String::from("recovery cannot authorize compensation"),
        }
    }
}

impl DispatchEngine {
    /// The caller has durably recorded two independent reads as Applied in the
    /// held Saga. Update the original intent before releasing its journal lock.
    pub(super) fn settle_saga_applied(
        &mut self,
        intent: &CommandIntent,
        correlation: [u8; 16],
        readback: Option<f64>,
    ) -> Result<(), String> {
        let digest = eneros_protocol_abstract::write::semantic_command_digest(
            u32::from(intent.device_id()),
            intent.point_id(),
            CommandValue::ActivePowerKw(intent.power_kw()),
        );
        let record = match self.journal.lookup(&correlation, &digest) {
            None => return Ok(()),
            Some(JournalHit::Conflict) => {
                return Err(String::from("recovery journal semantic conflict"))
            }
            Some(JournalHit::Current(record)) => record,
        };
        if record.status == ExecutionStatus::AppliedConfirmed {
            return Ok(());
        }
        let Some(actual) = readback.filter(|value| {
            value.is_finite()
                && (*value - intent.power_kw()).abs()
                    <= eneros_control_gateway::PCS_SETPOINT_READBACK_TOL_KW
        }) else {
            return Err(String::from(
                "recovery requires durable independent readback",
            ));
        };
        let original = self
            .intent_log
            .find(&record.command_id)
            .ok_or_else(|| String::from("recovery intent missing"))?;
        if original.plan_sha256.as_ref() != Some(intent.plan_sha256())
            || original.command_digest != Some(digest)
            || record.command_digest != digest
        {
            return Err(String::from("recovery intent identity unproven"));
        }
        let now = (self.now_fn)();
        if record.status != ExecutionStatus::Reconciling {
            self.journal
                .transition(
                    &correlation,
                    record.status,
                    ExecutionStatus::Reconciling,
                    now,
                    None,
                )
                .map_err(|e| alloc::format!("recovery journal: {e:?}"))?;
        }
        self.intent_log
            .update_state(&record.command_id, DispatchState::Committed)
            .map_err(|e| alloc::format!("recovery persist: {e:?}"))?;
        // The Saga stores the readback evidence. Do not manufacture a transport
        // receipt digest: the lost ACK remains lost in the original record.
        self.journal
            .transition(
                &correlation,
                ExecutionStatus::Reconciling,
                ExecutionStatus::AppliedConfirmed,
                now,
                None,
            )
            .map_err(|e| alloc::format!("recovery journal: {e:?}"))?;
        self.last_confirmed.insert(
            (u32::from(intent.device_id()), intent.point_id()),
            (actual, now),
        );
        self.arbiter.clear_in_flight(u32::from(intent.device_id()));
        Ok(())
    }
}
