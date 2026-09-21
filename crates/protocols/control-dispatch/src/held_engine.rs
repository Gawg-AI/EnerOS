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

    /// F70-04：待重试释放数（观测面——空闲重试收敛的可观测事实）。
    pub fn pending_release_count(&self) -> usize {
        self.holds.pending_count()
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
                    self.holds.note_pin_failure();
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
                self.holds.note_pin_failure();
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
            // F70-04（R1/§7.2.2）：持久化失败只撤回**本次新取得**的持有，
            // 不撤销仍在途计划的所有权——快照仍被其他非终态/未决计划
            // 引用时**不得** release（旧实现无条件 release，第三份计划被
            // 拒后前两份在途计划的保留被一并解除，后续回收可吃掉其输入）。
            // recheck RR1：引用计数经真实 `store.plans()` 查询——**读取
            // 失败（poisoned/介质异常/store 缺席）= 状态未知 = 不得按零
            // 引用释放**（`unwrap_or_default()` 会把查询失败折叠成空集 ⇒
            // 误判零引用 ⇒ 在途计划的钉被撤）。过度保留 fail-closed；介质
            // 恢复后由存活引用重算收敛。
            if self.live_snapshot_ref_count(snapshot_id) == Some(0) {
                if let Some(retainer) = self.input_retainer.as_mut() {
                    let _ = retainer.release(snapshot_id);
                }
            }
            return Err(DispatchError::IntentPersistFailed(e));
        }
        // F70-04（§7.2.6）：本快照进入新一轮持有（代次 +1）——旧代次的
        // 完成缓存/待重试归属失效。代次递增与有界淘汰（≤16，淘汰最旧键；
        // 淘汰至多引发一次幂等重释放，不损正确性）收拢进 HoldRegistry
        //（Deepening-v1 §A.2）。
        self.holds.admit(snapshot_id);
        // F70-05（R5/§8.2.1）：完整 16B plan id = 机器身份（4B 前缀只供
        // 阅读——前缀碰撞的不同计划不得合并账目）。
        eneros_runtime::println!(
            "[svc-control-dispatch] PLAN-HELD plan={:02x?} batches={} intents={}",
            plan.id().0,
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
            // F70-04（R2/§7.2.4）：无非终态计划（空闲）也要执行收尾——
            // 最后一份计划的释放失败不依赖新提案存在：待释放项在后续
            // 实际调度到的周期继续重试（旧实现在此提前 return，跳过
            // `finalize_held_plans`，空闲周期内保留持续泄漏、可耗尽钉位
            // 并阻断后续准入）。
            self.finalize_held_plans();
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
                // R13-B：投递标签/清算超时单源（计划级 DeliveryContext，
                // 每批一次构造——与收拢前每批一次字面 String 构造等价）。
                let mut executor = EngineSagaExecutor::new(
                    self,
                    telemetry,
                    &plan.actor,
                    crate::engine::PlanBinding {
                        plan_id: *plan.plan.id(),
                        input_digest: *plan.plan.input_digest(),
                        wire_sha: CandidatePlan::plan_wire_sha256(&plan.plan),
                        ttl_ms: plan.plan.ttl_ms(),
                        legacy_single: crate::compiler::is_legacy_single_command_plan(&plan.plan),
                    },
                    crate::engine::DeliveryContext {
                        target_label: alloc::string::String::from(
                            crate::engine::STATION_PLAN_TARGET_LABEL,
                        ),
                        settle_timeout_ms: crate::engine::HELD_DELIVERY_SETTLE_MS,
                    },
                );
                saga.drive(&mut executor, now)
            } else {
                saga.drive(&mut ReadOnlyRecovery { engine: self }, now)
            }
            .map_err(DispatchError::IntentPersistFailed)?;
            eneros_runtime::println!(
                "[svc-control-dispatch] HELD-BATCH batch={:02x?} state={:?} events={} plan={:02x?}",
                &report.batch_id[..4],
                report.final_state,
                report.events,
                plan.plan.id().0,
            );
            for command in &report.commands {
                // F70-05：HELD-COMMAND 携带完整 intent id + 所属完整
                // plan id——逐计划守恒/Applied/读回都按同一计划归属重算，
                // 不再靠日志位置关联（字段追加在行尾，既有词形/正则不受
                // 影响）。
                // recheck RR2（§8.2.7 投递 trace）：追加 dev/pt/val 绑定
                // 锚——外部 transcript 的 WRITE/READ 按 (addr, 值, 时序)
                // 归属到本 intent，跨计划事务不得互相补数。
                eneros_runtime::println!("[svc-control-dispatch] HELD-COMMAND intent={:02x?} state={:?} attempts={} readback={:?} reason={} plan={:02x?} dev=0x{:x} pt={} val={}", command.intent_id, command.state, command.attempts, command.readback, command.reason.as_deref().unwrap_or("-"), plan.plan.id().0, command.device, command.point, command.value_kw);
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
    /// ② 按快照**存活引用**解除钉（F70-04/§7.2.3：快照仍被其他在途/
    ///    未决计划引用 ⇒ 保留；无引用才释放——不是"有一份计划结束"就
    ///    整体释放）。释放失败进有界待释放队列，空闲周期也持续重试
    ///    （§7.2.4）；快照被新一轮合法持有时队列条目撤销（§7.2.6）。
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
        // 存活引用（§7.2.1 事实源 = 持久计划本身）：在途（非终态）或
        // 未决（对账中）计划各记一份引用——钉集/释放全部由此派生，
        // 不另立计数。
        let mut live_refs: alloc::collections::BTreeMap<[u8; 16], usize> =
            alloc::collections::BTreeMap::new();
        for plan in &plans {
            if plan.terminal() && !plan.unresolved() {
                continue;
            }
            if let Some(input) =
                eneros_agent_sdk::snap_input_wire::TrustedInput::decode(&plan.input)
            {
                *live_refs.entry(input.facts().snapshot_id).or_insert(0) += 1;
            }
        }
        // §7.2.6 两笔剪除（代次失效 + 存活引用）与重试集转入 pass 私有集，
        // 收拢为释放协议入口（Deepening-v1 §A.2 / §11.3：协议归属同状态）。
        self.holds.begin_finalize_pass(&live_refs);

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
            if self.holds.mark_done_if_first(plan_id) {
                eneros_runtime::println!(
                    "[svc-control-dispatch] HELD-PLAN-DONE plan={:02x?} batches={} applied={} blocked={} failed={} state={}",
                    plan_id.0,
                    reports.len(),
                    applied,
                    blocked,
                    failed,
                    if committed { "Committed" } else { "Terminal" }
                );
            }
            // 释放面（§7.2.3）：`terminal && !unresolved` 是**计划**事实，
            // 留在调用方；钉住账目的四条合取判据（本代次未记完成 / 本
            // pass 未入队 / 不在重试集 / 无存活引用）连同释放效应、事实
            // 登记与失败入队，单点收在 `HoldRegistry::release_if_idle`。
            if terminal && !unresolved {
                self.holds
                    .release_if_idle(snapshot_id, &live_refs, &mut self.input_retainer);
            }
        }
        // 待释放重试协议末步（§7.2.4 不依赖新提案 / §7.2.5 远端幂等至
        // 收敛；含收尾合并与本 pass 新失败在前的截断序）。
        self.holds.retry_pending(&mut self.input_retainer);
    }

    /// 快照的**存活引用**数（在途 = 非终态，或未决 = 对账中；事实源 =
    /// 持久计划本身——钉集是它的 RAM 派生态，§7.2.1）。
    ///
    /// recheck RR1：返回 `Option<usize>`——`None` = **状态未知**（store
    /// 缺席 / `plans()` 读取失败（poisoned、介质异常））。读取失败不得
    /// 折叠成空集当零引用（旧 `unwrap_or_default()` 的缺陷形态：真实
    /// write-failed 使 store poisoned 后，回滚查询被误解为 0 引用并
    /// release，另一份在途计划的输入保留被撤销）。调用方对 `None`
    /// fail-closed：不释放（过度保留可恢复，误释放不可恢复）。
    fn live_snapshot_ref_count(&self, snapshot_id: [u8; 16]) -> Option<usize> {
        let store = self.held_plans.as_ref()?;
        let plans = store.plans().ok()?;
        Some(
            plans
                .iter()
                .filter(|p| !p.terminal() || p.unresolved())
                .filter(|p| {
                    eneros_agent_sdk::snap_input_wire::TrustedInput::decode(&p.input)
                        .map(|i| i.facts().snapshot_id == snapshot_id)
                        .unwrap_or(false)
                })
                .count(),
        )
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
        self.engine
            .reconcile_saga_intent(intent, crate::engine::HELD_DELIVERY_SETTLE_MS)
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
