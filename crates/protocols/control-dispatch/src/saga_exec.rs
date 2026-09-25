//! C1 拆分（2026-09-24）：计划投递执行三件套（`PlanBinding`/`DeliveryContext`/
//! `EngineSagaExecutor`）自 engine.rs 迁出。外部路径
//! `engine::{EngineSagaExecutor, PlanBinding, DeliveryContext}` 经 engine.rs
//! re-export 保持不变（集成测试/CONTEXT 词条路径不破）。
/// 计划派生绑定束（R13-B；ADR-0052 §4 构造收拢义务）。
/// D-3 教训载体：plan_input_digest 与 plan_sha 同为 [u8;32]，
/// 命名字段构造取代位置实参后互换在构造点即语义可见。
///
/// 可见性：`pub`（非蓝图原判 pub(crate)）——唯一 crate 外现实消费组 =
/// 本 crate 集成测试（tests/settlement_lock.rs D-3 红测需命名字段构造
/// input_digest/wire_sha 可区分对）；不构成对外稳定 API 承诺，形态随
/// 引擎内聚演进（R13-B 登记在案）。
pub struct PlanBinding {
    pub plan_id: eneros_decision_contract::PlanId,
    pub input_digest: [u8; 32],
    /// = CandidatePlan wire SHA-256（62 v0.315.0 WP-62.1.2；构造方经
    /// `CandidatePlan::plan_wire_sha256` 单源派生）。
    pub wire_sha: [u8; 32],
    pub ttl_ms: u64,
    pub legacy_single: bool,
}

/// 计划投递上下文（CONTEXT.md 词条；持久化=登记 R13-G 未实施）。
/// 随计划同生命周期流动：投递标签（STATION_PLAN_TARGET_LABEL 单源）+
/// 逐投递清算超时（HELD_DELIVERY_SETTLE_MS 单源，≠saga 批 deadline）。
/// 可见性同理 `PlanBinding`（集成测试构造面）。
pub struct DeliveryContext {
    pub target_label: alloc::string::String,
    pub settle_timeout_ms: u64,
}

impl Default for DeliveryContext {
    /// R14-A：站点计划投递默认上下文——两单源常量的唯一构造落点
    /// （原 held_engine 驱动路径与测试夹具的 struct 字面构造收敛于此，
    /// 值不变）。
    fn default() -> Self {
        Self {
            target_label: alloc::string::String::from(crate::engine::STATION_PLAN_TARGET_LABEL),
            settle_timeout_ms: crate::engine::HELD_DELIVERY_SETTLE_MS,
        }
    }
}

/// 蓝图 61 Phase 5 WP-5.2（[10] §3.3）：12 步合核 → [`SagaExecutor`]
/// 生产 adapter——命令批经 [`eneros_control_dispatch::saga::BatchSaga`]
/// 编排时，逐命令的授权/投递/读回/记账仍由唯一合核承载（不产生第二
/// 执行真相）。
///
/// > 状态：~~deferred（ADR-0052，蓝图 63 v0.316.0）~~ → **定向解冻**
/// > （ADR-0052 修订 2026-09-20：激活前置 §2b 经 ADR-0054 满足；仅
/// > 接入所需行为解冻，生产激活证据与时点见修订段）。
/// > 原 too_many_arguments allow 已随 R13-B 构造收拢删除
/// > （PlanBinding + DeliveryContext 命名字段构造，ADR-0052 §4 义务
/// > 执行）。
///
/// - `execute`：走 [`DispatchEngine::execute_core`]-equivalent 链
///   （PlanCommand 身份：幂等键 = intent_id 派生 / legacy 单命令 = PlanId
///   ——journal 同键防重复投递）；
/// - `reconcile`（[10] §3.4 readback-first）：只调用设备独立读取接口，
///   不重入执行链、不追加意图、不发送写命令；读值匹配后由 driver
///   再次独立读取确认，读失败交人工处理。
/// - `compensate`：补偿派生键（`"comp"` 域分隔）全新执行——新意图、
///   新授权、新 journal 条目，不改写历史。
pub struct EngineSagaExecutor<'a> {
    engine: &'a mut DispatchEngine,
    telemetry: &'a TrustedTelemetry,
    actor: &'a AuthenticatedActor,
    plan_id: eneros_decision_contract::PlanId,
    plan_input_digest: [u8; 32],
    /// CandidatePlan wire SHA-256（62 v0.315.0 WP-62.1.2；构造方经
    /// `CandidatePlan::plan_wire_sha256` 派生后显式传入）。
    plan_sha: [u8; 32],
    plan_ttl_ms: u64,
    legacy_single: bool,
    target_device: String,
    deadline_ms: u64,
}

use alloc::string::String;

use eneros_safety_validator::TrustedTelemetry;

use crate::dispatch::{AuthenticatedActor, DispatchError, DispatchReceipt, DispatchState};
use crate::engine::DispatchEngine;
use crate::engine::{CoreCommand, CoreIdentity};
use crate::saga::{CmdOutcome, ReconcileOutcome, SagaExecutor};

/// 能量窗估算单源（WP-64.3.2.3；R-64.e 边界内——`run_typed` 幂等命中
/// 路径与 [`EngineSagaExecutor::execute_intent`] 两体共用的 crate 内
/// 私有函数。62 卷「不给 executor 增删参数」冻结注记已随 ADR-0052
/// 定向解冻（修订 2026-09-20）失效；R13-B 已完成构造收拢，本函数
/// 签名不变）。
///
/// 语义冻结：`window_secs = (valid_until - now).min(ttl)/1000`（下限
/// `max(1)` 由调用方施加，保持原表达式位置）；能量上限形态 =
/// `|power| × 意图窗（valid_from..valid_until，小时）`。
pub(crate) fn intent_window_estimate(
    intent: &eneros_decision_contract::CommandIntent,
    now_ms: u64,
    ttl_ms: u64,
) -> (u64, f64) {
    let window_secs = intent.valid_until_ms().saturating_sub(now_ms).min(ttl_ms) / 1000;
    let window_h = (intent
        .valid_until_ms()
        .saturating_sub(intent.valid_from_ms()) as f64)
        / 3_600_000.0;
    let energy_kwh = intent.power_kw().abs() * window_h;
    (window_secs, energy_kwh)
}

/// correlation → 事件摘要证据（32B 证据位；SHA-256 域分隔）。
fn request_digest_of(correlation: [u8; 16]) -> [u8; 32] {
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(b"eneros.saga-request.v1");
    h.update(correlation);
    h.finalize().into()
}

impl<'a> EngineSagaExecutor<'a> {
    /// 构造收拢（R13-B / ADR-0052 §4 义务）：11 参 → 5 参——计划派生
    /// 绑定束 [`PlanBinding`] + 投递上下文 [`DeliveryContext`] 命名字段
    /// 构造（[u8;32] 同型互换在构造点即语义可见，62.1 D-3 防御）。
    pub fn new(
        engine: &'a mut DispatchEngine,
        telemetry: &'a TrustedTelemetry,
        actor: &'a AuthenticatedActor,
        binding: PlanBinding,
        delivery: DeliveryContext,
    ) -> Self {
        let PlanBinding {
            plan_id,
            input_digest,
            wire_sha,
            ttl_ms,
            legacy_single,
        } = binding;
        Self {
            engine,
            telemetry,
            actor,
            plan_id,
            plan_input_digest: input_digest,
            plan_sha: wire_sha,
            plan_ttl_ms: ttl_ms,
            legacy_single,
            target_device: delivery.target_label,
            deadline_ms: delivery.settle_timeout_ms,
        }
    }

    fn execute_intent(
        &mut self,
        intent: &eneros_decision_contract::CommandIntent,
        correlation: [u8; 16],
        power_override_kw: Option<f64>,
    ) -> Result<DispatchReceipt, DispatchError> {
        let started = (self.engine.now_fn)();
        if started < intent.valid_from_ms() || started >= intent.valid_until_ms() {
            return Err(DispatchError::ProposalInvalid(String::from(
                "saga command outside validity window",
            )));
        }
        let (window_secs, energy_est) = intent_window_estimate(intent, started, self.plan_ttl_ms);
        // 恢复的批可能引用已移除的路由：与普通意图入口同样拒绝，
        // 不能让未注册目标使整个服务 panic。
        let route_idx = self
            .engine
            .route_index_for(u32::from(intent.device_id()), intent.point_id())
            .ok_or_else(|| {
                DispatchError::NotAuthorized(alloc::format!(
                    "saga target device={} point={} 未绑定本 dispatch",
                    intent.device_id(),
                    intent.point_id()
                ))
            })?;
        self.engine.arbitrate_owned(
            crate::arbiter::CommandSource::SiteOptimizer,
            true,
            u32::from(intent.device_id()),
            Some(self.plan_id),
        )?;
        self.engine.execute_core(
            CoreCommand {
                identity: CoreIdentity::PlanCommand { correlation },
                ttl_secs: window_secs.max(1),
                power_kw: power_override_kw.unwrap_or_else(|| intent.power_kw()),
                target_device: self.target_device.clone(),
                input_digest: Some(self.plan_input_digest),
                energy_kwh: Some(energy_est),
                // 62 v0.315.0 WP-62.1.2（D-3）：wire SHA-256（原此处传
                // plan_input_digest——输入摘要冒充 wire 绑定摘要）。
                plan_sha: Some(self.plan_sha),
                idem_note: crate::engine::IdemNote::SagaReplay,
            },
            self.telemetry,
            self.actor,
            started,
            alloc::vec![DispatchState::Proposed, DispatchState::Validated],
            self.deadline_ms,
            route_idx,
        )
    }

    fn map_receipt(receipt: DispatchReceipt) -> CmdOutcome {
        let note = receipt.note.clone().unwrap_or_default();
        let digest: [u8; 32] = {
            use sha2::Digest as _;
            let mut h = sha2::Sha256::new();
            h.update(receipt.correlation_id);
            h.update([receipt.final_state.as_str().len() as u8]);
            h.update(receipt.final_state.as_str().as_bytes());
            h.finalize().into()
        };
        match receipt.final_state {
            DispatchState::Committed => CmdOutcome::Applied {
                receipt_digest: digest,
                readback: None,
            },
            DispatchState::Rejected => CmdOutcome::Rejected { reason: note },
            DispatchState::Failed => CmdOutcome::Failed { reason: note },
            DispatchState::Indeterminate => CmdOutcome::Unknown {
                request_digest: digest,
                reason: note,
            },
            other => CmdOutcome::Unknown {
                request_digest: digest,
                reason: alloc::format!("in-flight journal state: {other:?}"),
            },
        }
    }
}

impl SagaExecutor for EngineSagaExecutor<'_> {
    fn applied_durable(
        &mut self,
        intent: &eneros_decision_contract::CommandIntent,
        readback: Option<f64>,
    ) -> Result<(), String> {
        let (correlation, digest) =
            crate::compiler::intent_journal_identity(&self.plan_id, intent, self.legacy_single);
        self.engine
            .settle_saga_applied(intent, correlation, digest, readback)
    }

    fn execute(
        &mut self,
        intent: &eneros_decision_contract::CommandIntent,
        _attempt: u32,
    ) -> CmdOutcome {
        let correlation = crate::compiler::plan_command_correlation(
            &self.plan_id,
            intent.intent_id(),
            self.legacy_single,
        );
        match self.execute_intent(intent, correlation, None) {
            Ok(r) => {
                let mut outcome = Self::map_receipt(r);
                if let CmdOutcome::Applied { readback, .. } = &mut outcome {
                    *readback = self.engine.post_apply_readback(intent, self.deadline_ms);
                }
                outcome
            }
            Err(DispatchError::DeliveryTimeout) => CmdOutcome::Unknown {
                request_digest: request_digest_of(correlation),
                reason: String::from("delivery timeout"),
            },
            Err(e) => CmdOutcome::Rejected {
                reason: alloc::format!("{e:?}"),
            },
        }
    }

    fn reconcile(&mut self, intent: &eneros_decision_contract::CommandIntent) -> ReconcileOutcome {
        self.engine.reconcile_saga_intent(intent, self.deadline_ms)
    }

    fn compensate(
        &mut self,
        intent: &eneros_decision_contract::CommandIntent,
        restore_kw: f64,
    ) -> CmdOutcome {
        // 补偿 = 新意图、新授权、新 journal（[10] §3.3.9）：补偿幂等键
        // 独立派生（"comp" 域分隔），与原命令互不覆盖。
        let base = crate::compiler::plan_command_correlation(
            &self.plan_id,
            intent.intent_id(),
            self.legacy_single,
        );
        let correlation = {
            use sha2::Digest as _;
            let mut h = sha2::Sha256::new();
            h.update(b"eneros.saga-compensation.v1");
            h.update(base);
            let d: [u8; 32] = h.finalize().into();
            d[..16].try_into().expect("16B")
        };
        match self.execute_intent(intent, correlation, Some(restore_kw)) {
            Ok(r) => Self::map_receipt(r),
            Err(DispatchError::DeliveryTimeout) => CmdOutcome::Unknown {
                request_digest: request_digest_of(correlation),
                reason: String::from("compensation timeout"),
            },
            Err(e) => CmdOutcome::Rejected {
                reason: alloc::format!("{e:?}"),
            },
        }
    }
}
