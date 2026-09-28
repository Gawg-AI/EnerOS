//! Engine integration for the durable plan owner. Reuses the unique command
//! chain through engine's crate-visible face without exposing mutable
//! internals. C1 转正（2026-09-24）：crate 级真模块（lib.rs 声明）——原
//! `#[path]` 暗挂 + `use super::*` 全吸 engine 私有命名空间；现 engine
//! 公开面经 `crate::engine::*` glob，依赖类型显式 import。
//!
//! 「持有」概念的读者入口在本文件与 [`crate::held_plan`]/[`crate::hold_registry`]
//! （durable 路径，ADR-0054 唯一实现）。会话级持有原型 plan_hold.rs 已删
//! （2026-09-25，R16-A / 蓝图 75 WP-75.1.0；REG-70.11-HOLD-UNWIRED 销项
//! ——其 doc 自认的持久化归宿 WP-70.11.4 由 ADR-0054 durable 路径落地，
//! 绕过了该原型）。
use alloc::string::String;
use alloc::vec::Vec;

use eneros_agent_sdk::snap_input_wire::TrustedInput;
use eneros_decision_contract::{CandidatePlan, CommandBatch, CommandIntent};
use eneros_safety_validator::TrustedTelemetry;

use crate::dispatch::{AuthenticatedActor, DispatchError, DispatchReceipt, DispatchState};
use crate::engine::*;
use crate::held_plan::{DurablePlans, HeldPlan};
use crate::journal::{ExecutionStatus, JournalHit};
use crate::saga::{BatchState, DriveReport};
use crate::saga::{CmdOutcome, ReconcileOutcome, SagaExecutor};
use crate::settle::{ExecutionCtx, SettleTrigger};

/// ADR-0054:29–30：**持有期间保留证书引用的冻结输入与事件**的装配面。
///
/// hold = 新计划准入前登记（fail-closed：false ⇒ 调用方必须拒绝准入——
/// 必要输入可能被回收时不得继续依赖该输入签发）；release = 计划终态且
/// 无未决后解除（幂等；失败可重试——释放是回收面不是安全面）。钉集是
/// RAM 派生态，唯一持久事实 = 持有计划本身：重启后由 owner 装配
/// （[`HeldOwner::assemble`] + [`DispatchEngine::install_held_owner`]）
/// 按存活引用计划重派生。
pub trait HeldInputRetainer {
    /// 登记快照钉（幂等）。false = 面不可用/容量满。
    fn hold(&mut self, snapshot_id: [u8; 16]) -> bool;
    /// 解除快照钉（幂等）。false = 未钉/面不可用（可重试）。
    fn release(&mut self, snapshot_id: [u8; 16]) -> bool;
    /// 完整活跃钉集同步（蓝图 77 卷一 WP-77.1.3）：把远端本世代的钉集
    /// **原子确认**为 `ids`（dispatch 从持久计划派生的 `!terminal ||
    /// unresolved` 全集）。false = 同步未确认（远端验证失败/传输失败）
    /// ——engine 置 stale 态阻断新持有准入，后续 held tick 重试。
    fn sync(&mut self, ids: &[[u8; 16]]) -> bool;
}

/// fail-closed 保留面：一切 hold 拒绝（装配缺失时**不得**静默放行准入——
/// ADR-0054:29–30 的反面 enforcement）。sync 同理拒绝——若空返回 true，
/// telemetry 侧屏障将在无任何保护的情况下解锁（远端没有任何钉集确认）。
pub struct NoRetention;
impl HeldInputRetainer for NoRetention {
    fn hold(&mut self, _snapshot_id: [u8; 16]) -> bool {
        false
    }
    fn release(&mut self, _snapshot_id: [u8; 16]) -> bool {
        false
    }
    fn sync(&mut self, _ids: &[[u8; 16]]) -> bool {
        false
    }
}

/// 蓝图 77 卷一 WP-77.1.1：held 回执 note 标签（[`Self::held_receipt`]
/// note 前缀 = dispatch 对 EA 的「本 tick 曾接纳持有计划」反馈词形——
/// EA 据此为同 tick 内直接终态的计划补记活跃史；两端共用本常量，不手抄
/// 字符串）。
pub const HELD_RECEIPT_NOTE_TAG: &str = "held-plan";

/// R16-N（蓝图 75 WP-75.1.6）：held 计划时域预检（**纯判定**，自
/// run_typed held 分叉下沉——原三重校验嵌在 349 行函数体中不可单测）。
/// 判据与拒绝词形逐字保持：计划时域终点超遥测有效期 / 超证书有效期 /
/// 任一意图有效期超遥测有效期 ⇒ `ProposalInvalid("held horizon exceeds
/// trusted telemetry")`。既有行为锁：typed_chain
/// `held_plan_horizon_beyond_control_window_rejected`。调用点位置不动
/// （accept_held_plan 准入前）；不触 typed 链类型合同与 correlation
/// 派生（ADR-0048）。
pub(crate) fn held_horizon_violation(
    plan: &CandidatePlan,
    telemetry: &TrustedTelemetry,
    certificate: &eneros_decision_contract::CertifiedPlan,
    batches: &[CommandBatch],
) -> Option<DispatchError> {
    let horizon_end = plan
        .horizon_start_ms()
        .saturating_add(plan.step_ns().saturating_mul(u64::from(plan.step_count())) / 1_000_000);
    if horizon_end > telemetry.valid_until_ms
        || horizon_end > certificate.valid_until_ms()
        || batches
            .iter()
            .flat_map(|b| b.intents())
            .any(|i| i.valid_until_ms() > telemetry.valid_until_ms)
    {
        return Some(DispatchError::ProposalInvalid(String::from(
            "held horizon exceeds trusted telemetry",
        )));
    }
    None
}

/// 蓝图 77 卷一 WP-77.1.2：持有计划 owner 的**单次装配单元**——durable
/// store 与保留面一并交付，拆除 `install_held_plans` / `set_input_retainer`
/// 两步装配的次序窗口（旧形态：store 先在、retainer 未装 ⇒
/// `accept_held_plan` 的 `if let Some(retainer)` 允许无钉准入——ADR-0054:29–30
/// 的反面，蓝图 §1.3 实锤的公开入口缺口）。装配期即从持久计划枚举存活
/// 引用并重钉；store 读错、输入解码错、钉拒绝任一发生 ⇒ **显式未就绪**
/// （engine 侧阻断新持有准入与新设备写；旧计划仍只读对账/取消）——
/// 绝不把错误折叠成空集。
///
/// 替换事务：engine 侧允许整体重装 owner；被替换装配可能遗留在远端钉面
/// 的旧 ID 由 WP-77.1.3 的完整活跃集同步收敛（每个 held tick 全量确认，
/// 重启必重建——本单元不另设逐 ID 撤销事务）。
pub struct HeldOwner {
    pub(crate) plans: DurablePlans,
    pub(crate) retainer: alloc::boxed::Box<dyn HeldInputRetainer>,
    /// 装配就绪态：false = 恢复枚举/重钉失败（fail-closed）。
    pub(crate) ready: bool,
    /// 装配期枚举的存活引用集（`!terminal \|\| unresolved`）——install 时
    /// 首发 [`HeldInputRetainer::sync`]（蓝图「启动及每次 held tick」的
    /// 启动半边）。
    pub(crate) live_ids: Vec<[u8; 16]>,
}

impl HeldOwner {
    /// 一次交付两项（store + retainer）。装配即断电恢复：按持久计划
    /// 枚举 `!terminal || unresolved` 的快照 ID 逐 ID 重钉——**与
    /// finalize/存活引用同一口径**（蓝图 §1.4：repin 的 `!terminal()`
    /// 与 finalize 的 `!terminal() || unresolved()` 两套口径不得分叉，
    /// 未决计划的输入同样必须受剪枝保护）。
    pub fn assemble(
        plans: DurablePlans,
        retainer: alloc::boxed::Box<dyn HeldInputRetainer>,
    ) -> Self {
        let mut owner = Self {
            plans,
            retainer,
            ready: true,
            live_ids: Vec::new(),
        };
        match owner.plans.plans() {
            Ok(stored) => {
                for plan in &stored {
                    if plan.terminal() && !plan.unresolved() {
                        continue;
                    }
                    // 解码错 = 输入身份不可证 ⇒ 未就绪（旧 filter_map 静默
                    // 跳过 = 把损坏折成「无引用」）。
                    let Some(input) =
                        eneros_agent_sdk::snap_input_wire::TrustedInput::decode(&plan.input)
                    else {
                        owner.ready = false;
                        continue;
                    };
                    let id = input.facts().snapshot_id;
                    // WP-77.1.3 启动同步集：装配期枚举的存活引用全集
                    //（install 首发全量同步的输入）。
                    if !owner.live_ids.contains(&id) {
                        owner.live_ids.push(id);
                    }
                    if !owner.retainer.hold(id) {
                        owner.ready = false;
                    }
                }
            }
            Err(_) => owner.ready = false,
        }
        owner
    }
}

impl DispatchEngine {
    /// 蓝图 77 卷一 WP-77.1.2：owner 单次装配（store + 保留面一体交付；
    /// 两步装配 API 退役）。装配期恢复失败 ⇒ `held_retention_fault`：
    /// 新持有准入与新设备写全部阻断（只读对账仍可），证据行可观测。
    pub fn install_held_owner(&mut self, owner: crate::held_engine::HeldOwner) {
        self.held_retention_fault = !owner.ready;
        self.input_retainer = Some(owner.retainer);
        self.held_plans = Some(owner.plans);
        if self.held_retention_fault {
            eneros_runtime::println!(
                "[control-dispatch] HELD-RETENTION-FAULT reason=assembly-repin-failed（新准入/新设备写阻断；只读对账继续）"
            );
        }
        // WP-77.1.3 复查轮补口（全局对抗 B1）：**启动同步点**——蓝图「在
        // 启动及每次 held tick 的同步点重试」的启动半边。install 首发全量
        // 同步（集 = 装配期枚举的存活引用全集）；失败置 stale（与 tick 同
        // 口径：只阻新持有准入），首个 held tick 起重试收敛。原 1c49e590
        // 声称此机制但未落盘（分批应用断点）——本提交补齐并订正登记。
        let live_ids = owner.live_ids.clone();
        let synced = self
            .input_retainer
            .as_mut()
            .map(|r| r.sync(&live_ids))
            .unwrap_or(true);
        self.held_sync_stale = !synced;
        if !synced {
            eneros_runtime::println!(
                "[control-dispatch] HELD-SYNC-STALE n={} reason=install（启动同步未确认——新持有准入阻断）",
                live_ids.len()
            );
        }
    }

    /// 供装配查询：保留面是否已装配（未装配 ⇒ 生产应装 NoRetention 而
    /// 非留空——留空仅限 host 测试）。
    pub fn input_retainer_installed(&self) -> bool {
        self.input_retainer.is_some()
    }

    /// F70-04：待重试释放数（观测面——空闲重试收敛的可观测事实）。
    pub fn pending_release_count(&self) -> usize {
        self.holds.pending_count()
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
        // WP-77.1.2：装配保留故障 ⇒ 一切新设备写阻断（旧计划只读对账）。
        if self.held_retention_fault {
            return Err(DispatchError::IntentPersistFailed(String::from(
                "held retention fault; control blocked",
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
                "{} batches={} state={state:?}",
                HELD_RECEIPT_NOTE_TAG,
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
        // WP-77.1.2：装配期恢复失败（枚举/重钉）⇒ 准入拒绝（fail-closed）。
        if self.held_retention_fault {
            return Err(DispatchError::NotAuthorized(String::from(
                "held retention fault: assembly repin failed; admissions blocked",
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
        // WP-77.1.3：世代同步未确认 ⇒ **新**持有准入阻断（远端钉集未确认
        // 时准入即承诺保留——与屏障合同相悖）。置于同 ID 重放检查之后：
        // 重放回原回执不是新投递，不受 stale 影响。
        if self.held_sync_stale {
            return Err(DispatchError::NotAuthorized(String::from(
                "held snapshot sync pending: retention barrier unconfirmed",
            )));
        }
        // ADR-0054:29–30：准入前先钉住证书引用的冻结输入（fail-closed——
        // 钉面不可用/容量满 ⇒ 拒绝准入：必要输入可能被回收时不得继续
        // 依赖该输入签发）。WP-77.1.2：hold 门**无条件**经过——retainer
        // 由 owner 装配保证在位（NoRetention = 显式拒绝实现）；旧
        // `if let Some(retainer)` 在两步装配窗口期允许无钉准入（蓝图 §1.3
        // 公开入口缺口）。
        let snapshot_id = input.facts().snapshot_id;
        let Some(retainer) = self.input_retainer.as_mut() else {
            return Err(DispatchError::Internal(String::from(
                "held owner assembly incomplete: retainer absent",
            )));
        };
        if !retainer.hold(snapshot_id) {
            self.holds.note_pin_failure();
            return Err(DispatchError::NotAuthorized(String::from(
                "input retention unavailable: held plan needs pinned frozen snapshot",
            )));
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
        // 词形单源 = evidence 表（蓝图 75 WP-75.1.2，R16-G）。
        eneros_runtime::println!(
            "{}",
            crate::evidence::plan_held(
                &plan.id().0,
                batches.len(),
                batches.iter().map(|b| b.intents().len()).sum::<usize>(),
            )
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

    /// 蓝图 78 卷一 WP-78.1.3：**内部执行核**（原 pub 面——外部测试
    /// 绕过生产周期同步的直接驱动入口已收窄）。生产周期与行为测试一律
    /// 穿 [`Self::held_tick_cycle`]（sync 先行 + drive + finalize 的唯一
    /// 序）。恢复语义不变：只读对账，冻结输入变更/丢失取消未发步。
    fn drive_held_plans(
        &mut self,
        input: Option<&TrustedInput>,
        telemetry: Option<&TrustedTelemetry>,
    ) -> Result<Vec<DriveReport>, DispatchError> {
        self.drive_held_plans_counted(input, telemetry)
            .map(|(reports, _)| reports)
    }

    /// 蓝图 78 卷一 WP-78.1.3：held 生产周期的**私有单点**（原
    /// `ControlDispatch::tick_held` 内联编排迁入现有 held_engine module
    /// ——序的所有权归 held_engine，trait 面只委托）。重评 → drive 前
    /// 存活集同步 → drive → 收尾（drive 后活跃数）；返回值 = drive 完成
    /// 后时点的在途计划数（EA 据此抑制 System1，ADR-0054）。
    pub(crate) fn held_tick_cycle(
        &mut self,
        input: Option<&TrustedInput>,
        telemetry: Option<&TrustedTelemetry>,
    ) -> Result<usize, DispatchError> {
        // 蓝图 77 卷一 WP-77.1.3（复查轮 B3 重构）：**同步先行**——drive
        // 的 allowed 判据消费**本 tick** 的同步结果（同步失败 = 本 tick 即
        // 只读，不滞后一拍）；同步集 = drive 前的存活引用（drive 终态化
        // 引发的释放由下一 tick 收敛——过保留方向）。每个 held tick 全量
        // 确认（重启必重建；不冒用发送方 epoch 充当服务世代）。失败 ⇒
        // stale：新持有准入阻断 + 在途计划只读对账/取消（蓝图 §2.4）。
        // WP-77.1.2 复查轮补口（B4）：装配保留故障逐 tick 重评（先于
        // 同步——恢复后本 tick 的 stale 判据即用新状态）。
        self.re_evaluate_held_retention();
        let live_ids = self.live_held_snapshot_ids()?;
        // retainer 缺席 = owner 未装配 = held 路径整体不可达（accept 在
        // 「held store absent」先拒）——此处 true 只是无对象可同步的空
        // 事实，不是把未知折成已确认（无后续消费方依赖它放行任何动作）。
        let synced = self
            .input_retainer
            .as_mut()
            .map(|r| r.sync(&live_ids))
            .unwrap_or(true);
        self.held_sync_stale = !synced;
        if !synced {
            eneros_runtime::println!(
                "[control-dispatch] HELD-SYNC-STALE n={}（远端世代钉集未确认——新准入阻断+在途只读）",
                live_ids.len()
            );
        }
        // 蓝图 77 卷一 WP-77.1.1：活跃计数单读化——drive 内部收尾的同一
        // 读取派生（drive 完成后时点），读错显式传播。
        let (_, active) = self.drive_held_plans_counted(input, telemetry)?;
        Ok(active)
    }

    /// 蓝图 77 卷一 WP-77.1.1：drive + **drive 完成后时点的活跃计数**一次
    /// 返回。旧 `ControlDispatch::tick_held` 在 drive 后第二次 `plans()`
    /// 读取并把失败 `unwrap_or(0)` 折叠成活跃 0——读错折零会让 EA 放行
    /// System1（条件性故障，与 HELD_TICK 短帧同族）。现计数由
    /// [`Self::finalize_held_plans`] 的同一读取派生（drive 完成后时点），
    /// 读错显式传播，折叠面结构性消除（plans() 镜像读的失败源 =
    /// poisoned ⇒ 写失败必已先传播——本重构把该防御面从「静默折零」
    /// 收紧为「不可能折零」）。
    #[allow(clippy::type_complexity)] // (reports, 同步集, 活跃数) 三元产物——中间聚合，无跨 fn 传播
    pub(crate) fn drive_held_plans_counted(
        &mut self,
        input: Option<&TrustedInput>,
        telemetry: Option<&TrustedTelemetry>,
    ) -> Result<(Vec<DriveReport>, usize), DispatchError> {
        let Some(store) = &self.held_plans else {
            return Ok((Vec::new(), 0));
        };
        let plans = store.plans().map_err(DispatchError::IntentPersistFailed)?;
        let mut reports = Vec::new();
        let Some(plan) = plans.iter().find(|p| !p.terminal()) else {
            // F70-04（R2/§7.2.4）：无非终态计划（空闲）也要执行收尾——
            // 最后一份计划的释放失败不依赖新提案存在：待释放项在后续
            // 实际调度到的周期继续重试（旧实现在此提前 return，跳过
            // `finalize_held_plans`，空闲周期内保留持续泄漏、可耗尽钉位
            // 并阻断后续准入）。
            let active = self
                .finalize_held_plans()
                .map_err(DispatchError::IntentPersistFailed)?;
            return Ok((reports, active));
        };
        let now = (self.now_fn)();
        // WP-77.1.3 复查轮补口（全局对抗 B3）：同步未确认 ⇒ 在途计划只
        // 允许安全的只读对账/取消（蓝图 §2.4 原句）——未发步 cancel、
        // 零新投递；恢复后新计划再准入。
        let allowed = !plan.recovered
            && !self.held_sync_stale
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
                } else if self.held_sync_stale {
                    "retention sync pending; pending steps cancelled (replan after sync recovers)"
                } else {
                    "trusted input or certificate no longer valid"
                };
                saga.cancel_pending(reason, now)
                    .map_err(DispatchError::IntentPersistFailed)?;
            }
            let report = if allowed && !earlier_failed {
                let telemetry = telemetry.ok_or(DispatchError::TelemetryExpired)?;
                // R13-B：投递标签/清算超时单源（计划级 DeliveryContext，
                // 每批一次构造；R14-A 起经 Default 收敛到 saga_exec 单点）。
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
                    crate::engine::DeliveryContext::default(),
                );
                saga.drive(&mut executor, now)
            } else {
                saga.drive(
                    &mut ReadOnlyRecovery {
                        engine: self,
                        legacy_single: crate::compiler::is_legacy_single_command_plan(&plan.plan),
                    },
                    now,
                )
            }
            .map_err(DispatchError::IntentPersistFailed)?;
            // 词形单源 = evidence 表（WP-75.1.2）。
            eneros_runtime::println!(
                "{}",
                crate::evidence::held_batch(
                    &report.batch_id[..4],
                    &report.final_state,
                    report.events,
                    &plan.plan.id().0,
                )
            );
            for command in &report.commands {
                // F70-05：HELD-COMMAND 携带完整 intent id + 所属完整
                // plan id——逐计划守恒/Applied/读回都按同一计划归属重算，
                // 不再靠日志位置关联（字段追加在行尾，既有词形/正则不受
                // 影响）。
                // recheck RR2（§8.2.7 投递 trace）：追加 dev/pt/val 绑定
                // 锚——外部 transcript 的 WRITE/READ 按 (addr, 值, 时序)
                // 归属到本 intent，跨计划事务不得互相补数。
                // 词形单源 = evidence 表（WP-75.1.2）。
                eneros_runtime::println!(
                    "{}",
                    crate::evidence::held_command(
                        &command.intent_id,
                        &command.state,
                        command.attempts,
                        &command.readback,
                        command.reason.as_deref().unwrap_or("-"),
                        &plan.plan.id().0,
                        command.device,
                        command.point,
                        command.value_kw,
                    )
                );
            }
            // One compiled batch per valid tick bounds work. Invalid/recovery
            // ticks may cancel at most the admitted bounded plan's commands.
            reports.push(report);
            if allowed && !earlier_failed {
                break;
            }
        }
        let active = self
            .finalize_held_plans()
            .map_err(DispatchError::IntentPersistFailed)?;
        Ok((reports, active))
    }

    /// WP-77.1.2 复查轮补口（全局对抗 B4）：装配保留故障的**重评通道**。
    /// 故障只在 install 置位且无复位路径——boot 次序瞬态（telemetry 面
    /// lazy-open 未完成 ⇒ hold 被拒）即可永久砖死全部新控制写（其余每个
    /// 失败面都有重试至收敛通道：release→pending 队列、sync→每 tick）。
    /// 每个 held tick 调用一次：存活引用逐一重钉全数成功（且 store 可读、
    /// 输入可解；无存活引用亦然——保留已无关）⇒ 故障清除。真介质故障
    /// 重试持续失败 ⇒ 故障保持（fail-closed 方向不变）。
    pub(crate) fn re_evaluate_held_retention(&mut self) {
        if !self.held_retention_fault {
            return;
        }
        let Some(store) = &self.held_plans else {
            return;
        };
        let Ok(stored) = store.plans() else {
            return; // store 读错 = 状态未知 = 故障保持
        };
        let mut all_pinned = true;
        for plan in &stored {
            if plan.terminal() && !plan.unresolved() {
                continue;
            }
            let Some(input) = eneros_agent_sdk::snap_input_wire::TrustedInput::decode(&plan.input)
            else {
                all_pinned = false;
                break;
            };
            let pinned = match self.input_retainer.as_mut() {
                Some(r) => r.hold(input.facts().snapshot_id),
                None => false,
            };
            if !pinned {
                all_pinned = false;
                break;
            }
        }
        if all_pinned {
            self.held_retention_fault = false;
            eneros_runtime::println!(
                "[control-dispatch] HELD-RETENTION-RECOVERED reason=repin-retry-ok（装配保留故障清除——准入/设备写恢复）"
            );
        }
    }

    /// 当前存活引用的快照 ID 集（蓝图 77 卷一 WP-77.1.3 复查轮补口：tick
    /// 的**同步先行**输入——drive 前快照；drive 终态化引发的释放由下一
    /// tick 收敛，过保留方向）。读错显式传播（不折空集）。
    pub(crate) fn live_held_snapshot_ids(&self) -> Result<Vec<[u8; 16]>, DispatchError> {
        let Some(store) = &self.held_plans else {
            return Ok(Vec::new());
        };
        let plans = store.plans().map_err(DispatchError::IntentPersistFailed)?;
        let mut ids: Vec<[u8; 16]> = Vec::new();
        for plan in &plans {
            if plan.terminal() && !plan.unresolved() {
                continue;
            }
            // 解码失败跳过与 live_refs/finalize 同口径（不可达面：装载期
            // 已验 + RAM 镜像仅经验证提交变异——REG-77-I 登记）。
            if let Some(input) =
                eneros_agent_sdk::snap_input_wire::TrustedInput::decode(&plan.input)
            {
                let id = input.facts().snapshot_id;
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        Ok(ids)
    }

    /// 终局收尾（ADR-0054:29–30 + 15–16）：计划全部批终态后
    /// ① 打印一次性 `HELD-PLAN-DONE` 证据行（含逐命令终态计数——验收
    ///    从原始工件重算守恒，不搜词形凑数）；
    /// ② 按快照**存活引用**解除钉（F70-04/§7.2.3：快照仍被其他在途/
    ///    未决计划引用 ⇒ 保留；无引用才释放——不是"有一份计划结束"就
    ///    整体释放）。释放失败进有界待释放队列，空闲周期也持续重试
    ///    （§7.2.4）；快照被新一轮合法持有时队列条目撤销（§7.2.6）。
    /// 返回 **drive 完成后时点的在途（非终态）计划数**（蓝图 77 卷一
    /// WP-77.1.1：HELD_TICK 活跃计数单源——plans 读取失败显式传播，
    /// 绝不折叠成 0。同步集改由 [`Self::live_held_snapshot_ids`] 在
    /// drive 前派生——同步先行，见 `ControlDispatch::tick_held`）。
    fn finalize_held_plans(&mut self) -> Result<usize, String> {
        let Some(store) = &self.held_plans else {
            return Ok(0);
        };
        let plans = store.plans()?;
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
                // 词形单源 = evidence 表（WP-75.1.2）。
                eneros_runtime::println!(
                    "{}",
                    crate::evidence::held_plan_done(
                        &plan_id.0,
                        reports.len(),
                        applied,
                        blocked,
                        failed,
                        if committed { "Committed" } else { "Terminal" },
                    )
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
        // WP-77.1.1：drive 完成后时点的在途计数（与上方同一 plans 读取
        // 派生——释放只动钉面不动计划终态性；读错已在入口显式传播）。
        Ok(plans.iter().filter(|p| !p.terminal()).count())
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
    /// R14-B：与投递路径同一派生事实（is_legacy_single_command_plan），
    /// 原 false 硬编码依赖「held 计划均多命令」的准入不变式而无断言——
    /// 不变式变化时此处会静默派生错键，改为构造点随计划派生。
    legacy_single: bool,
}
impl SagaExecutor for ReadOnlyRecovery<'_> {
    fn applied_durable(
        &mut self,
        intent: &CommandIntent,
        readback: Option<f64>,
    ) -> Result<(), String> {
        let (correlation, digest) =
            crate::compiler::intent_journal_identity(intent.plan_id(), intent, self.legacy_single);
        self.engine
            .settle_saga_applied(intent, correlation, digest, readback)
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
    /// R14-B：身份对 (correlation, digest) 由调用方经
    /// `compiler::intent_journal_identity` 同点派生后整对传入——本函数
    /// 不再自行补算 digest（原对内两半分处两函数，漂移不可见）。
    ///
    /// R16-B（蓝图 75 WP-75.1.1）：本函数收缩为「身份预检 + 归一化」——
    /// lookup / 幂等 / readback 有限差校验 / intent 身份三元组 / 条件转移
    /// →Reconciling（表行表达不了条件转移，归一化是其职责）；终局四段
    /// 写序（intent Committed → journal 终局确认 → 记账基线 →
    /// 在途清账）经一次 `settle_command`
    /// （SettleDispatch 编排点）走 `SettleTrigger::RecoveredApplied` 表行
    /// 执行（结算写序单源回到决策表；失败传播、错误词形逐字保留）。
    /// 守卫：`engine::tests::recovered_settle_write_order_lives_in_settle_table_only`
    ///（守卫针描述用词形化指称——标识符字面会自咬，R14 教训）。
    pub(super) fn settle_saga_applied(
        &mut self,
        intent: &CommandIntent,
        correlation: [u8; 16],
        digest: [u8; 32],
        readback: Option<f64>,
    ) -> Result<(), String> {
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
        // The Saga stores the readback evidence. Do not manufacture a transport
        // receipt digest: the lost ACK remains lost in the original record.
        // （表行 RecoveredApplied：无回执摘要、不重复记账——差异裁决见表行
        // 注释。）ctx.started = 归一化时刻：journal 时间戳与记账基线
        // 同源（与原手写路径的 `now` 一致）。
        let ctx = ExecutionCtx {
            started: now,
            command_id: record.command_id,
            correlation,
            response: None,
            energy: 0.0,
            setpoint_kw: Some(actual),
            target: (u32::from(intent.device_id()), intent.point_id()),
        };
        let mut states = Vec::new();
        self.settle_command(SettleTrigger::RecoveredApplied, &mut states, &ctx)
            .map_err(|e| match e {
                DispatchError::IntentPersistFailed(s) => s,
                other => alloc::format!("recovery: {other:?}"),
            })?;
        Ok(())
    }
}
