//! Engine integration for the durable plan owner. This is a child of engine so
//! it can reuse the unique command chain without exposing mutable internals.
use eneros_agent_sdk::snap_input_wire::TrustedInput;
use eneros_decision_contract::{CandidatePlan, CommandBatch, CommandIntent};

use super::*;
use crate::held_plan::{DurablePlans, HeldPlan};
use crate::saga::{BatchState, DriveReport};

impl DispatchEngine {
    /// Assembly supplies an already opened registered region. No RAM fallback
    /// is permitted for production multi-step plans.
    pub fn install_held_plans(&mut self, plans: DurablePlans) {
        self.held_plans = Some(plans);
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
        self.held_plans
            .as_mut()
            .ok_or_else(|| DispatchError::IntentPersistFailed(String::from("held store absent")))?
            .accept(plan, certificate, input.encode(), *actor, batches)
            .map_err(DispatchError::IntentPersistFailed)?;
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
                eneros_runtime::println!("[svc-control-dispatch] HELD-COMMAND intent={:02x?} state={:?} attempts={} readback={:?}", &command.intent_id[..4], command.state, command.attempts, command.readback);
            }
            // One compiled batch per valid tick bounds work. Invalid/recovery
            // ticks may cancel at most the admitted bounded plan's commands.
            reports.push(report);
            if allowed && !earlier_failed {
                break;
            }
        }
        Ok(reports)
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
