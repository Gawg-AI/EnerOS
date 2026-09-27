//! 蓝图 62 卷 62.1（v0.315.0）结算路径行为锁。
//!
//! WP-62.1.1（D-2）：`deliver_and_settle` 的 `Unreachable` 专门臂是
//! Unreachable 终局的唯一实现——通用 `Err(e)` 臂内的同语义 if 块是
//! R7 对抗审查补丁叠加产生的复制残骸（match 顺序匹配保证不可达），
//! 删除前后本套件锁定活路径行为逐字节不变：
//! - `ChannelError::Unreachable` → `DispatchError::DeliveryUnreachable`；
//! - journal 终态 `Failed`（同 key 重放返回原终态、零投递）；
//! - durable intent 终态 `Failed`；
//! - audit 文案 `delivery-unreachable`。
//!
//! 夹具与 journal_crash_matrix 同构（引擎装配/可信遥测/提议面）。

mod support;

/// R16-O：单位集显式传参（点 4=Ratio、5/6=Other——走 RequiredPointCoverage 其他校验路径；行为逐字节保持）。
fn trusted_telemetry() -> eneros_safety_validator::TrustedTelemetry {
    support::trusted_telemetry_with(&[
        eneros_safety_validator::PointUnit::Volt,
        eneros_safety_validator::PointUnit::Ampere,
        eneros_safety_validator::PointUnit::Hertz,
        eneros_safety_validator::PointUnit::Ratio,
        eneros_safety_validator::PointUnit::Other,
        eneros_safety_validator::PointUnit::Other,
    ])
}

use std::cell::RefCell;
use std::rc::Rc;

use eneros_audit::{AuditError, AuditEvent, AuditLogger};
use eneros_control_dispatch::{
    AuthenticatedActor, ControlDispatch, DeliveryChannel, DispatchConfig, DispatchEngine,
    DispatchError, DispatchState, InMemoryIntentStore, IntentRecord, IntentStore, IntentStoreError,
};
use eneros_control_gateway::chain::ControlChain;
use eneros_control_gateway::envelope::ActorKind;
use eneros_dual_brain::{DecisionPath, ProposedControlPlan};

const KEY: &[u8] = b"w62-settlement-lock-key-000000000000000000";
const LEASE_ID: [u8; 16] = [0x62; 16];
const ISSUER: [u8; 16] = [0x41; 16];
const EPOCH_SEED: u64 = 7;
const DEVICE_ID: u32 = 1;
const POINT_ID: u32 = 10;

fn now_ms() -> u64 {
    1_700_000_000_000
}

fn test_profile() -> eneros_control_gateway::ControlProfile {
    eneros_control_gateway::ControlProfile {
        lease_ttl_secs: 600,
        ..eneros_control_gateway::ControlProfile::ai_pcs_setpoint(
            DEVICE_ID, POINT_ID, 4, LEASE_ID, EPOCH_SEED,
        )
    }
}

fn build_chain() -> ControlChain {
    let profile = test_profile();
    let mut chain = ControlChain::from_profile(KEY, &profile);
    let _ = profile.issue_lease(&mut chain, now_ms() / 1000);
    chain
}

fn proposal(power_kw: f64) -> ProposedControlPlan {
    ProposedControlPlan {
        plan_id: String::from("req-1700000000000-1"),
        path: DecisionPath::Slow,
        schedule: eneros_energy_lp_model::result::ScheduleResult {
            schedule: vec![eneros_energy_lp_model::result::ScheduleEntry {
                period: 0,
                charge_power_kw: 0.0,
                discharge_power_kw: power_kw.max(0.0),
                net_power_kw: power_kw,
                soc_pct: 0.6,
                revenue_yuan: 0.0,
            }],
            total_revenue_yuan: 0.0,
            objective_value: 0.0,
            solve_status: eneros_solver_core::result::SolveStatus::Optimal,
        },
        power_kw,
        target_device: String::from("pcs"),
        ttl_secs: 60,
        issued_at_ms: now_ms(),
        input_digest: None,
        model_digest: None,
        latency: Default::default(),
        safety_clamped: false,
        feedback: None,
    }
}

/// 恒错通道：投递以注入的 `ChannelError` 失败（零设备副作用）。
struct FailingChannel {
    fail: eneros_control_dispatch::ChannelError,
    attempts: Rc<RefCell<u64>>,
}

impl DeliveryChannel for FailingChannel {
    fn deliver(
        &mut self,
        _target: &str,
        _payload: &[u8],
        _timeout_ms: u64,
    ) -> Result<Vec<u8>, eneros_control_dispatch::ChannelError> {
        *self.attempts.borrow_mut() += 1;
        Err(self.fail.clone())
    }
}

/// 观测型 intent 存储：捕获 append/update_state 调用序列。
struct SpyIntentStore {
    inner: InMemoryIntentStore,
    appends: Rc<RefCell<Vec<IntentRecord>>>,
    updates: Rc<RefCell<Vec<DispatchState>>>,
}

impl IntentStore for SpyIntentStore {
    fn append(&mut self, rec: &IntentRecord) -> Result<(), IntentStoreError> {
        self.appends.borrow_mut().push(rec.clone());
        self.inner.append(rec)
    }
    fn update_state(
        &mut self,
        command_id: &[u8; 16],
        state: DispatchState,
    ) -> Result<(), IntentStoreError> {
        self.updates.borrow_mut().push(state);
        self.inner.update_state(command_id, state)
    }
    fn find(&self, command_id: &[u8; 16]) -> Option<IntentRecord> {
        self.inner.find(command_id)
    }
    fn pending(&self) -> Vec<IntentRecord> {
        self.inner.pending()
    }
}

/// durable-ack 注入失败存储（62 复查 62.1-A 写序锁专用）：对
/// `Committed` 更新返回 `Unavailable`——模拟 Applied 写序的
/// "intent 置 Committed 落盘失败"（v0.314.24 写序不变量：durable
/// 终态 ack 先行，成功后才 journal AppliedConfirmed）。
struct FailOnCommittedStore {
    inner: InMemoryIntentStore,
    fail_applied: Rc<RefCell<bool>>,
}

impl FailOnCommittedStore {
    fn new(fail_applied: Rc<RefCell<bool>>) -> Self {
        Self {
            inner: InMemoryIntentStore::new(64),
            fail_applied,
        }
    }
}

impl IntentStore for FailOnCommittedStore {
    fn append(&mut self, rec: &IntentRecord) -> Result<(), IntentStoreError> {
        self.inner.append(rec)
    }
    fn update_state(
        &mut self,
        command_id: &[u8; 16],
        state: DispatchState,
    ) -> Result<(), IntentStoreError> {
        if matches!(state, DispatchState::Committed) && *self.fail_applied.borrow() {
            return Err(IntentStoreError::Unavailable(String::from("injected")));
        }
        self.inner.update_state(command_id, state)
    }
    fn find(&self, command_id: &[u8; 16]) -> Option<IntentRecord> {
        self.inner.find(command_id)
    }
    fn pending(&self) -> Vec<IntentRecord> {
        self.inner.pending()
    }
}

/// Applied 成功通道：对端回 Applied 两相成功帧（`OutcomeFrame` =
/// `[code=0, class=0]`，见 `eneros_control_gateway::wire`）。
struct AppliedChannel;

impl DeliveryChannel for AppliedChannel {
    fn deliver(
        &mut self,
        _target: &str,
        _payload: &[u8],
        _timeout_ms: u64,
    ) -> Result<Vec<u8>, eneros_control_dispatch::ChannelError> {
        Ok([0x00, 0x00].to_vec()) // OutcomeCode::Applied
    }
}

/// 捕获型控制审计（audit_chain CaptureLogger 同款：真实链头推进 +
/// detail 流共享观测）。
struct CaptureAudit {
    chain: eneros_audit::HashChain,
    details: Rc<RefCell<Vec<String>>>,
}

impl CaptureAudit {
    fn new(details: Rc<RefCell<Vec<String>>>) -> Self {
        Self {
            chain: eneros_audit::HashChain {
                last_hash: [0x47; 32],
                last_seq: 0,
                entry_count: 0,
                genesis_hash: [0x47; 32],
            },
            details,
        }
    }
}

impl AuditLogger for CaptureAudit {
    fn log(&mut self, mut event: AuditEvent) -> Result<eneros_audit::ChainEntry, AuditError> {
        event.seq = self.chain.last_seq + 1;
        let entry = eneros_audit::ChainEntry {
            seq: event.seq,
            prev_hash: self.chain.last_hash,
            event_hash: [0; 32],
            combined_hash: [0; 32],
            timestamp_ms: event.timestamp_ms,
        };
        self.chain.last_hash = entry.combined_hash;
        self.chain.last_seq = entry.seq;
        self.chain.entry_count += 1;
        self.details
            .borrow_mut()
            .push(String::from(event.detail.as_str()));
        Ok(entry)
    }
    fn log_batch(
        &mut self,
        events: &[AuditEvent],
    ) -> Result<Vec<eneros_audit::ChainEntry>, AuditError> {
        events.iter().map(|e| self.log(e.clone())).collect()
    }
    fn chain_state(&self) -> &eneros_audit::HashChain {
        &self.chain
    }
}

fn actor() -> AuthenticatedActor {
    AuthenticatedActor {
        kind: ActorKind::Ai,
        service: ISSUER,
        session_epoch: 0,
    }
}

struct Harness {
    engine: DispatchEngine,
    appends: Rc<RefCell<Vec<IntentRecord>>>,
    updates: Rc<RefCell<Vec<DispatchState>>>,
    audit_details: Rc<RefCell<Vec<String>>>,
    attempts: Rc<RefCell<u64>>,
}

fn engine_unreachable() -> Harness {
    let attempts = Rc::new(RefCell::new(0u64));
    let appends = Rc::new(RefCell::new(Vec::new()));
    let updates = Rc::new(RefCell::new(Vec::new()));
    let audit_details = Rc::new(RefCell::new(Vec::new()));
    let profile = test_profile();
    let config = base_config(
        profile,
        &audit_details,
        Box::new(SpyIntentStore {
            inner: InMemoryIntentStore::new(64),
            appends: Rc::clone(&appends),
            updates: Rc::clone(&updates),
        }),
        Box::new(FailingChannel {
            fail: eneros_control_dispatch::ChannelError::Unreachable,
            attempts: Rc::clone(&attempts),
        }),
    );
    Harness {
        engine: DispatchEngine::new(config),
        appends,
        updates,
        audit_details,
        attempts,
    }
}

/// 装配基线（settlement_lock 套件统一夹具；intent_log/delivery 由调用方
/// 注入——Unreachable 行为锁与 Applied 写序锁共用同一引擎面）。
/// WP-64.3.3.1：字面迁移为 support::test_config 夹具 + 所变字段
/// （control_audit=CaptureAudit 是本套件判据面，结构更新覆盖）。
/// 资源视图（DEVICE_ID/POINT_ID）与夹具 profile 派生同构。
fn base_config(
    profile: eneros_control_gateway::ControlProfile,
    audit_details: &Rc<RefCell<Vec<String>>>,
    intent_log: Box<dyn IntentStore>,
    delivery: Box<dyn DeliveryChannel>,
) -> DispatchConfig {
    let base = support::test_config_legacy(support::TestConfigSeed {
        chain: build_chain(),
        key: KEY.to_vec(),
        profile,
        issuer_service: ISSUER,
        fencing_token: 1 + EPOCH_SEED,
        now_fn: now_ms,
        intent_log,
        delivery,
    });
    DispatchConfig {
        audit_sinks: eneros_control_dispatch::AuditSinks {
            control_audit: Some(Box::new(CaptureAudit::new(Rc::clone(audit_details)))),
            ..base.audit_sinks
        },
        ..base
    }
}

/// WP-62.1.1 行为锁：`Unreachable` = 可证明未投递 → 终局 Failed。
///
/// 断言面（D-2 删除前后必须逐字节不变）：
/// 1. `Err(DispatchError::DeliveryUnreachable)`；
/// 2. durable intent 终态 `Failed`（且此前无 Delivered/Committed 升级）；
/// 3. audit 文案 = `delivery-unreachable`；
/// 4. journal 终态 `Failed`（同 key 重放命中 journal → 原终态、零投递）。
#[test]
fn unreachable_delivery_settles_failed_and_never_retries() {
    let mut h = engine_unreachable();
    let tel = trusted_telemetry();

    let r = h
        .engine
        .submit_typed(&support::legacy_candidate(&proposal(5.0)), &tel, &actor());
    assert!(
        matches!(r, Err(DispatchError::DeliveryUnreachable)),
        "Unreachable 必须返回 DeliveryUnreachable：{r:?}"
    );
    assert_eq!(*h.attempts.borrow(), 1, "恰好一次投递尝试");
    assert_eq!(
        h.updates.borrow().last(),
        Some(&DispatchState::Failed),
        "durable intent 终态 Failed：{:?}",
        h.updates.borrow()
    );
    assert!(
        !h.updates
            .borrow()
            .iter()
            .any(|s| matches!(s, DispatchState::Delivered | DispatchState::Committed)),
        "Unreachable 不得出现 Delivered/Committed：{:?}",
        h.updates.borrow()
    );
    assert_eq!(
        h.audit_details.borrow().last().map(String::as_str),
        Some("delivery-unreachable"),
        "audit 文案逐字节：{:?}",
        h.audit_details.borrow()
    );

    // 同 key 重放：journal 命中原终态 Failed（证明 journal 已终局），
    // 零新投递。
    let r2 = h
        .engine
        .submit_typed(&support::legacy_candidate(&proposal(5.0)), &tel, &actor())
        .expect("journal 命中返回原结果不 hard-fail");
    assert_eq!(r2.final_state, DispatchState::Failed, "{r2:?}");
    assert_eq!(*h.attempts.borrow(), 1, "重放零投递");
}

/// R13-B：saga executor 投递上下文夹具（R14-A 起经 Default 与生产
/// held_engine 驱动路径同一标签/超时——单源构造点在 saga_exec）。
fn test_delivery() -> eneros_control_dispatch::engine::DeliveryContext {
    eneros_control_dispatch::engine::DeliveryContext::default()
}

/// WP-62.1.2 夹具：合法 CandidatePlan（input_digest [0x65;32] ≠ model/
/// objective 摘要——两摘要域语义分离）。
fn candidate_plan() -> eneros_decision_contract::CandidatePlan {
    use eneros_decision_contract::{
        Assignment, CandidatePlan, CandidatePlanParts, PlanId, SnapshotId, SolveStatusKind,
    };
    CandidatePlan::from_parts(CandidatePlanParts {
        id: PlanId([0x62; 16]),
        snapshot_id: SnapshotId([0x53; 16]),
        objective_digest: [0x63; 32],
        resource_catalog_version: 1,
        horizon_start_ms: now_ms(),
        step_ns: 900_000_000,
        step_count: 1,
        solver_status: SolveStatusKind::Optimal,
        objective_value: 1.0,
        relative_gap: 0.0,
        max_primal_residual: 0.0,
        elapsed_ms: 1,
        model_digest: [0x64; 32],
        input_digest: [0x65; 32],
        assignments: vec![Assignment {
            asset_id: 1,
            device_id: 1,
            point_id: 10,
            power_kw: 5.0,
            unit_code: 6,
            step_index: 1,
        }],
        issued_at_ms: now_ms(),
        ttl_ms: 60_000,
    })
    .expect("合法候选计划")
}

/// WP-62.1.2 夹具：saga 命令意图（saga_e4e5 intent() 同形）。
fn saga_intent(
    plan_id: eneros_decision_contract::PlanId,
) -> eneros_decision_contract::CommandIntent {
    saga_intent_for(plan_id, 1, 10)
}

fn saga_intent_for(
    plan_id: eneros_decision_contract::PlanId,
    device_id: u16,
    point_id: u32,
) -> eneros_decision_contract::CommandIntent {
    use eneros_decision_contract::{
        CommandIntent, CommandIntentParts, CompensationHint, IntentId, Precondition,
    };
    CommandIntent::from_parts(CommandIntentParts {
        intent_id: IntentId([0x71; 16]),
        plan_id,
        plan_sha256: [0xAB; 32],
        asset_id: 1,
        device_id,
        point_id,
        power_kw: 5.0,
        unit_code: 6,
        valid_from_ms: now_ms(),
        valid_until_ms: now_ms() + 60_000,
        precondition: Precondition::None,
        compensation: CompensationHint::None,
    })
    .expect("合法命令意图")
}

#[test]
fn saga_unknown_route_rejected_without_panic_or_intent_write() {
    use eneros_control_dispatch::engine::EngineSagaExecutor;
    use eneros_control_dispatch::saga::{CmdOutcome, SagaExecutor};
    let plan = candidate_plan();
    let mut h = engine_unreachable();
    let tel = trusted_telemetry();
    let act = actor();
    let mut exec = EngineSagaExecutor::new(
        &mut h.engine,
        &tel,
        &act,
        eneros_control_dispatch::engine::PlanBinding {
            plan_id: *plan.id(),
            input_digest: *plan.input_digest(),
            wire_sha: eneros_decision_contract::CandidatePlan::plan_wire_sha256(&plan),
            ttl_ms: 60_000,
            legacy_single: false,
        },
        test_delivery(),
    );
    let outcome = exec.execute(&saga_intent_for(*plan.id(), 8, 1009), 1);
    assert!(matches!(outcome, CmdOutcome::Rejected { .. }));
    assert!(h.appends.borrow().is_empty(), "未知路由不得持久化或投递");
}

/// WP-62.1.2（D-3）红→绿：`EngineSagaExecutor` 落进 CoreCommand 的
/// `plan_sha` 必须 = CandidatePlan wire SHA-256（单源
/// `CandidatePlan::plan_wire_sha256`），且与 `input_digest`（输入快照
/// provenance）语义分离。现状 :1944 传 `plan_input_digest` 冒充 → 红。
#[test]
fn saga_executor_binds_plan_wire_sha_not_input_digest() {
    use eneros_control_dispatch::engine::EngineSagaExecutor;
    use eneros_control_dispatch::saga::SagaExecutor;

    let plan = candidate_plan();
    let wire_sha = eneros_decision_contract::CandidatePlan::plan_wire_sha256(&plan);
    assert_ne!(
        wire_sha,
        *plan.input_digest(),
        "夹具自检：wire SHA 与 input_digest 必须可区分"
    );

    let mut h = engine_unreachable();
    let tel = trusted_telemetry();
    let act = actor();
    let intent = saga_intent(*plan.id());
    {
        let mut exec = EngineSagaExecutor::new(
            &mut h.engine,
            &tel,
            &act,
            eneros_control_dispatch::engine::PlanBinding {
                plan_id: *plan.id(),
                input_digest: *plan.input_digest(),
                wire_sha,
                ttl_ms: 60_000,
                legacy_single: false,
            },
            test_delivery(),
        );
        // 投递通道恒 Unreachable——命令仍在投递前完成 durable intent
        // append（步骤 6），plan_sha 已落记录；execute 结果与本判据无关。
        let _ = exec.execute(&intent, 1);
    }
    let appends = h.appends.borrow();
    assert_eq!(appends.len(), 1, "{appends:?}");
    assert_eq!(
        appends[0].plan_sha256,
        Some(wire_sha),
        "plan_sha 必须是 wire SHA-256；若 = input_digest 则 D-3 漂移仍在"
    );
    assert_eq!(
        appends[0].input_digest,
        Some(*plan.input_digest()),
        "input_digest 语义保留（输入快照 provenance）"
    );
}

/// 附件证据：Failed 前唯一 durable intent 记录 = 步骤 6 的 Persisted
/// append（后续只有状态更新，不重复 append）。
#[test]
fn unreachable_delivery_appends_exactly_one_persisted_record() {
    let mut h = engine_unreachable();
    let tel = trusted_telemetry();
    let _ = h
        .engine
        .submit_typed(&support::legacy_candidate(&proposal(5.0)), &tel, &actor());
    let appends = h.appends.borrow();
    assert_eq!(appends.len(), 1, "{appends:?}");
    assert_eq!(appends[0].power_kw, 5.0);
    // append 时点状态 = Persisted；终态 Failed 由 update_state 落到
    // store 内同一条记录。
    assert_eq!(appends[0].state, DispatchState::Persisted);
    let final_rec = h.updates.borrow().last().copied();
    assert_eq!(final_rec, Some(DispatchState::Failed));
}

/// 62 复查（62.1-A）Applied 写序端到端锁：durable 终态 ack（intent 置
/// Committed）失败时，journal **不得**被推至 AppliedConfirmed（外部不得见
/// Committed/成功）——v0.314.24 写序不变量 "durable 终态 ack 先行，成功后才
/// journal AppliedConfirmed" 的因果门。
///
/// **62.1-A 缺口已闭合**（蓝图 76 卷三 WP-76.3.4）：Applied 分支不再手写两相，
/// 而是遍历 settle 表行的 `durable` 序——表行即写序权威。本测试因此转为
/// **端到端双保险**（与源结构锁 `engine.rs::applied_write_order_follows_table_row`
/// 配对：后者咬「写序从哪读」的形态，本只咬「失败不得推进 journal」的因果）。
/// 表驱动化的最终实证 = 把 Applied 行两臂换位：本测试立即红（收编前该变异
/// 零行为变化，因为分支不读表）。
#[test]
fn applied_write_order_durable_ack_failure_never_confirms_journal() {
    // 负控制：ack 成功 → 正常 Committed。
    let ok_flag = Rc::new(RefCell::new(false));
    let mut ok_engine = DispatchEngine::new(base_config(
        test_profile(),
        &Rc::new(RefCell::new(Vec::new())),
        Box::new(FailOnCommittedStore::new(Rc::clone(&ok_flag))),
        Box::new(AppliedChannel),
    ));
    let tel = trusted_telemetry();
    let ok_receipt = ok_engine
        .submit_typed(&support::legacy_candidate(&proposal(5.0)), &tel, &actor())
        .expect("ack 成功路径必须 Ok");
    assert_eq!(
        ok_receipt.final_state,
        DispatchState::Committed,
        "ack 成功 → AppliedConfirmed → Committed（写序第一相成功）"
    );

    // 注入：intent 置 Committed 失败（durable 终态 ack 落盘失败）。
    let fail_flag = Rc::new(RefCell::new(true));
    let audit_details = Rc::new(RefCell::new(Vec::new()));
    let mut engine = DispatchEngine::new(base_config(
        test_profile(),
        &audit_details,
        Box::new(FailOnCommittedStore::new(Rc::clone(&fail_flag))),
        Box::new(AppliedChannel),
    ));
    let r = engine
        .submit_typed(&support::legacy_candidate(&proposal(5.0)), &tel, &actor())
        .expect("Applied 失败路径返回 Ok(receipt)（Indeterminate 对账，不 hard-fail）");
    // 不伪造成功：终态 Indeterminate（AppliedDurableFailed → journal
    // TransportAcked→Indeterminate），绝无 Committed。
    assert_eq!(
        r.final_state,
        DispatchState::Indeterminate,
        "durable ack 失败 → Indeterminate 对账（不伪造 AppliedConfirmed）：{r:?}"
    );
    assert!(
        !r.state_path.contains(&DispatchState::Committed),
        "外部状态路径不得含 Committed（journal 未被推至 AppliedConfirmed）：{:?}",
        r.state_path
    );
    // 同 key 重放：journal 未到 AppliedConfirmed → 原终态 Indeterminate，
    // 零重投（幂等真值未被破坏）。
    let r2 = engine
        .submit_typed(&support::legacy_candidate(&proposal(5.0)), &tel, &actor())
        .expect("重放返回原终态不 hard-fail");
    assert_eq!(
        r2.final_state,
        DispatchState::Indeterminate,
        "重放命中 journal Indeterminate（无 AppliedConfirmed 污染）：{r2:?}"
    );
}

#[test]
fn saga_reconcile_missing_journal_never_sends() {
    use eneros_control_dispatch::engine::EngineSagaExecutor;
    use eneros_control_dispatch::saga::{ReconcileOutcome, SagaExecutor};
    let plan = candidate_plan();
    let mut h = engine_unreachable();
    let tel = trusted_telemetry();
    let act = actor();
    let mut exec = EngineSagaExecutor::new(
        &mut h.engine,
        &tel,
        &act,
        eneros_control_dispatch::engine::PlanBinding {
            plan_id: *plan.id(),
            input_digest: *plan.input_digest(),
            wire_sha: eneros_decision_contract::CandidatePlan::plan_wire_sha256(&plan),
            ttl_ms: 60_000,
            legacy_single: false,
        },
        test_delivery(),
    );
    let outcome = exec.reconcile(&saga_intent(*plan.id()));
    assert!(matches!(outcome, ReconcileOutcome::ResolvedManual { .. }));
    assert!(
        h.appends.borrow().is_empty(),
        "readback-first must never enter the write core"
    );
}

// 75 全局复查轮：夹具默认单位集方向守卫（code-review H1 防回潮）——
// support 默认形态恰含尾两位 Other（六套件现行域覆盖；全 Ratio 包装由
// typed_chain 显式传参、在己文件）。H1 曾实证：unit 为评估器纯元数据，
// 方向翻转零断言红——唯源扫描有牙。针运行期拼接（扫描目标仅 support.rs，
// 其内完整针形恰 2 处 = 默认数组尾位本体）。
#[test]
fn support_default_units_keep_other_tail() {
    let src = include_str!("support.rs");
    let needle = concat!("PointUnit::", "Other");
    assert_eq!(
        src.matches(needle).count(),
        2,
        "support 默认单位集方向 = [Volt,Ampere,Hertz,Ratio,Other,Other]——\
         方向翻转/顺手统一即红（H1 防回潮）"
    );
}
