//! 蓝图 61 v0.314.28：typed 产物链集成测试（[06] §3.6）。
//!
//! 2026-09-24 C2 收口后本套件全部走 typed 入口（facade `submit(proposal)`
//! 已退役；LegacyMigration 轨夹具经 `support::legacy_candidate` 构造
//! 派生身份候选）：
//! - typed 入口（submit_typed(CandidatePlan)）参考闭环的值与证据
//!   （同计划重提交命中同一幂等记录）；
//! - typed 入口拒绝过期计划；
//! - `submit_typed` 走过渡认证证书 + CommandIntent/CommandBatch
//!   （profile 资源视图 scope 校验在批构造内）；
//! - Required 策略下无身份/跨代候选整批拒绝（差分负例族，
//!   `trusted_input_admission_rejects_mismatch_family`——不受本收口
//!   影响，全程 typed 入口）。
mod support;

use std::cell::RefCell;
use std::rc::Rc;

use eneros_control_dispatch::adapter::proposal_to_candidate_unbound;
use eneros_control_dispatch::intent::{
    InMemoryIntentStore, IntentRecord, IntentStore, IntentStoreError,
};
use eneros_control_dispatch::{
    AuthenticatedActor, ControlDispatch, DeliveryChannel, DispatchConfig, DispatchEngine,
    DispatchError,
};
use eneros_control_gateway::chain::ControlChain;
use eneros_control_gateway::envelope::ActorKind;
use eneros_control_gateway::lease::LeaseScope;
use eneros_control_gateway::policy::{CommandClass, Policy, PolicyRule};
use eneros_control_gateway::safety::{SafetyPoint, SafetyRole};
use eneros_control_gateway::wire::ControlEnvelope;
use eneros_control_gateway::ControlProfile;
use eneros_dual_brain::audit::DecisionAudit;
use eneros_dual_brain::DecisionPath;
use eneros_telemetry_model::point_spec::PointSpec;
use eneros_telemetry_model::PhysicalUnit;
/// R16-O：全 Ratio 是本套件的少数派现行形态（七套件中唯一）——显式
/// 包装保留，不吃 support 默认（code-review 复核订正）。
fn trusted_telemetry() -> eneros_safety_validator::TrustedTelemetry {
    support::trusted_telemetry_with(&[
        eneros_safety_validator::PointUnit::Volt,
        eneros_safety_validator::PointUnit::Ampere,
        eneros_safety_validator::PointUnit::Hertz,
        eneros_safety_validator::PointUnit::Ratio,
        eneros_safety_validator::PointUnit::Ratio,
        eneros_safety_validator::PointUnit::Ratio,
    ])
}

const KEY: &[u8] = b"typed-chain-test-key-0123456789ab";
const LEASE_ID: [u8; 16] = [0x11; 16];
const EPOCH_SEED: u64 = 7;
const ISSUER: [u8; 16] = [0x22; 16];

fn now_ms() -> u64 {
    1_700_000_000_000
}

fn test_profile() -> ControlProfile {
    ControlProfile {
        lease_ttl_secs: 600,
        ..ControlProfile::ai_pcs_setpoint(1, 10, 4, LEASE_ID, EPOCH_SEED)
    }
}

fn build_chain() -> ControlChain {
    let profile = test_profile();
    let mut chain = eneros_control_gateway::chain::ControlChain::from_profile(KEY, &profile);
    let _ = profile.issue_lease(&mut chain, now_ms() / 1000);
    chain
}

fn proposal(power_kw: f64) -> eneros_dual_brain::ProposedControlPlan {
    eneros_dual_brain::ProposedControlPlan {
        plan_id: String::from("req-typed-1"),
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
            solve_status: eneros_energy_lp_model::result::SolveStatus::Optimal,
        },
        power_kw,
        target_device: String::from("pcs"),
        ttl_secs: 60,
        issued_at_ms: now_ms(),
        input_digest: Some([0xAB; 32]),
        model_digest: Some([0xCD; 32]),
        latency: Default::default(),
        safety_clamped: false,
        feedback: None,
    }
}

/// in-process 通道（同 dispatch_ingress_chain 形态；统计投递次数）。
struct CountingChannel {
    ingress: Rc<RefCell<eneros_device_agent::EnvelopeIngress>>,
    devices: Rc<RefCell<eneros_device_agent::DeviceRegistry>>,
    delivered: Rc<RefCell<u64>>,
}

impl DeliveryChannel for CountingChannel {
    fn deliver(
        &mut self,
        target: &str,
        payload: &[u8],
        timeout_ms: u64,
    ) -> Result<Vec<u8>, eneros_control_dispatch::ChannelError> {
        let _ = target;
        let _ = timeout_ms;
        *self.delivered.borrow_mut() += 1;
        let env = ControlEnvelope::decode(payload)
            .map_err(|e| eneros_control_dispatch::ChannelError::Protocol(format!("{e:?}")))?;
        if !env.verify_payload_digest() {
            // 篡改被摘要检出 → Denied 帧（写未发生）。
            return Ok(eneros_device_agent::OutcomeFrame {
                code: eneros_device_agent::CODE_DENIED,
                class: eneros_device_agent::DenyClass::Hmac.code(),
            }
            .encode()
            .to_vec());
        }
        let out =
            self.ingress
                .borrow_mut()
                .process(&mut self.devices.borrow_mut(), payload, now_ms());
        Ok(eneros_device_agent::outcome_to_frame(&out)
            .encode()
            .to_vec())
    }
}

fn device_side() -> (
    Rc<RefCell<eneros_device_agent::EnvelopeIngress>>,
    Rc<RefCell<eneros_device_agent::DeviceRegistry>>,
) {
    let mut policy = Policy::new(3);
    let _ = policy.replace_rules(vec![PolicyRule {
        actor_kind: Some(ActorKind::Ai),
        devices: vec![1],
        points: vec![10],
        kinds: vec![CommandClass::Setpoint],
        time_window: (0, 0),
        obligations: vec![],
    }]);
    let scope = LeaseScope {
        devices: vec![1],
        points: vec![10],
        max_class: 1,
    };
    let ingress = eneros_device_agent::EnvelopeIngress::new(
        KEY,
        LEASE_ID,
        scope,
        600,
        EPOCH_SEED,
        policy,
        60,
        &[(
            1,
            10,
            SafetyPoint {
                role: SafetyRole::PccBreaker,
                spec: PointSpec {
                    point_id: 10,
                    device_id: 1,
                    unit: PhysicalUnit::Kilowatts,
                    safe_min: -500.0,
                    safe_max: 500.0,
                    max_ramp_per_sec: None,
                    direction: eneros_telemetry_model::point_spec::ControlDirection::Discrete,
                    remote_controllable: true,
                },
            },
        )],
        now_ms(),
    );
    let mut device = eneros_device_agent::MockPointDevice::new(1)
        .with_point("grid_up", 1.0)
        .with_point("soc_pct", 60.0)
        .with_point("dc_bus_v", 600.0)
        .with_point("overcurrent", 0.0)
        .with_point("overtemp", 0.0)
        .with_point_at("power", 0.0, 10);
    device.touch(now_ms());
    let mut point_map = std::collections::BTreeMap::new();
    point_map.insert(String::from("power"), 10u32);
    point_map.insert(String::from("grid_up"), 1u32);
    point_map.insert(String::from("soc_pct"), 2u32);
    point_map.insert(String::from("dc_bus_v"), 3u32);
    point_map.insert(String::from("overcurrent"), 4u32);
    point_map.insert(String::from("overtemp"), 5u32);
    let mut devices = eneros_device_agent::DeviceRegistry::new();
    devices
        .register(
            "pcs",
            eneros_device_agent::DeviceType::Pcs,
            1,
            Box::new(device),
            point_map,
        )
        .expect("静态合法登记");
    (
        Rc::new(RefCell::new(ingress)),
        Rc::new(RefCell::new(devices)),
    )
}

fn engine_with(channel: Box<dyn DeliveryChannel>) -> DispatchEngine {
    let config = support::test_config_legacy(support::TestConfigSeed {
        chain: build_chain(),
        key: KEY.to_vec(),
        profile: test_profile(),
        issuer_service: ISSUER,
        fencing_token: 1 + EPOCH_SEED,
        now_fn: now_ms,
        intent_log: Box::new(eneros_control_dispatch::InMemoryIntentStore::new(64)),
        delivery: channel,
    });
    DispatchEngine::new(DispatchConfig {
        audit_sinks: eneros_control_dispatch::AuditSinks {
            audit: Some(DecisionAudit::new([0x40; 32])),
            ..config.audit_sinks
        },
        ..config
    })
}

fn actor() -> AuthenticatedActor {
    AuthenticatedActor {
        kind: ActorKind::Ai,
        service: ISSUER,
        session_epoch: 0,
    }
}

// （2026-09-24 C2 收口退役）原 `facade_and_typed_share_identity`：
// facade 与 typed 入口的跨路径幂等差分（同一 proposal 两路径
// correlation/幂等键逐字节相同）。facade 入口（`engine::run`）退役后
// 两路径合一，该测试退化为同路径自比——差分对象消失，测试随退役面
// 一并删除。单路径幂等（同计划重提交命中同一幂等记录）由
// journal_crash_matrix 的 idempotent 系列承担；「Required 拒无身份
// 候选」的差分负例见
// `trusted_input_admission_rejects_mismatch_family`（①无身份臂——
// 全程 submit_typed/submit_trusted，不受本收口影响）。

/// typed 入口独立可用：CandidatePlan → Committed（一次投递 + 读回）。
#[test]
fn typed_entry_commits() {
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0u64));
    let channel = CountingChannel {
        ingress,
        devices,
        delivered: delivered.clone(),
    };
    let mut engine = engine_with(Box::new(channel));

    let p = proposal(-40.0);
    let c = proposal_to_candidate_unbound(&p, 1).expect("adapter");
    // 槽位断言（蓝图 65 WP-65.2.4：adapter 侧 objective_value/ttl 槽位
    // ——源值在命名字段上对照；issued/ttl 同型相邻位互换在此不可通过）。
    assert_eq!(
        c.objective_value(),
        p.schedule.objective_value,
        "objective 槽位（源 = schedule.objective_value）"
    );
    assert_eq!(
        c.ttl_ms(),
        u64::from(p.ttl_secs.max(1)) * 1000,
        "ttl 槽位（源 = ttl_secs）"
    );
    assert_eq!(
        c.issued_at_ms(),
        p.issued_at_ms,
        "issued 槽位（源 = issued_at）"
    );
    assert_ne!(c.issued_at_ms(), c.ttl_ms(), "issued/ttl 分立");
    let r = engine
        .submit_typed(&c, &trusted_telemetry(), &actor())
        .expect("typed submit");
    assert!(r.final_state.is_committed());
    assert_eq!(*delivered.borrow(), 1);
}

/// typed 入口拒绝过期计划（构造期窗口即非法 → ProposalInvalid）。
#[test]
fn typed_entry_rejects_expired_plan() {
    let (ingress, devices) = device_side();
    let channel = CountingChannel {
        ingress,
        devices,
        delivered: Rc::new(RefCell::new(0u64)),
    };
    let mut engine = engine_with(Box::new(channel));

    let p = proposal(50.0);
    let mut c = proposal_to_candidate_unbound(&p, 1).expect("adapter");
    // 过期：issued_at 远早于 now 且 ttl 短——直接经 wire 往返改写不可行
    // （构造器校验 ttl>0），用早期 issued_at 重造（命名字段逐槽位从
    // getter 取值——字段名自证不换位）。
    let expired = eneros_decision_contract::CandidatePlan::from_parts(
        eneros_decision_contract::CandidatePlanParts {
            id: *c.id(),
            snapshot_id: c.snapshot_id(),
            objective_digest: *c.objective_digest(),
            resource_catalog_version: c.resource_catalog_version(),
            horizon_start_ms: c.horizon_start_ms(),
            step_ns: c.step_ns(),
            step_count: c.step_count(),
            solver_status: c.solver_status(),
            objective_value: c.objective_value(),
            relative_gap: c.relative_gap(),
            max_primal_residual: c.max_primal_residual(),
            elapsed_ms: c.elapsed_ms(),
            model_digest: *c.model_digest(),
            input_digest: *c.input_digest(),
            assignments: c.assignments().to_vec(),
            issued_at_ms: now_ms() - 120_000,
            ttl_ms: 60_000,
        },
    )
    .expect("expired plan constructible");
    let err = engine
        .submit_typed(&expired, &trusted_telemetry(), &actor())
        .unwrap_err();
    assert!(matches!(err, DispatchError::ProposalInvalid(_)));
    let _ = &mut c; // 保持变量使用
}

// ---- WP-5.1（v0.314.41）：多步计划确定性编译 + not_before 逐 tick 投递 ----

/// 可推进时钟（fn 指针约束 → 静态原子；仅本组用例推进）。
static TICK_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1_700_000_000_000);

fn tick_now() -> u64 {
    TICK_MS.load(std::sync::atomic::Ordering::Relaxed)
}

/// 与 CountingChannel 同构，但设备侧时钟 = 可推进 TICK（与引擎同源——
/// 信封时间性校验两侧必须同钟）。
struct TickCountingChannel {
    ingress: Rc<RefCell<eneros_device_agent::EnvelopeIngress>>,
    devices: Rc<RefCell<eneros_device_agent::DeviceRegistry>>,
    delivered: Rc<RefCell<u64>>,
}

impl DeliveryChannel for TickCountingChannel {
    fn deliver(
        &mut self,
        _target: &str,
        payload: &[u8],
        _timeout_ms: u64,
    ) -> Result<Vec<u8>, eneros_control_dispatch::ChannelError> {
        *self.delivered.borrow_mut() += 1;
        let out =
            self.ingress
                .borrow_mut()
                .process(&mut self.devices.borrow_mut(), payload, tick_now());
        Ok(eneros_device_agent::outcome_to_frame(&out)
            .encode()
            .to_vec())
    }
}

fn two_step_plan() -> eneros_decision_contract::CandidatePlan {
    use eneros_decision_contract::{
        Assignment, CandidatePlan, CandidatePlanParts, PlanId, SnapshotId, SolveStatusKind,
    };
    let a = |step: u32, kw: f64| Assignment {
        asset_id: 1,
        device_id: 1,
        point_id: 10,
        power_kw: kw,
        unit_code: 6,
        step_index: step,
    };
    let tick = TICK_MS.load(std::sync::atomic::Ordering::Relaxed);
    CandidatePlan::from_parts(CandidatePlanParts {
        id: PlanId([0x71; 16]),
        snapshot_id: SnapshotId([2u8; 16]),
        objective_digest: [0xAA; 32],
        resource_catalog_version: 1,
        horizon_start_ms: tick,
        step_ns: 10_000_000_000, // 10s 步长
        step_count: 2,
        solver_status: SolveStatusKind::Optimal,
        objective_value: 1.0,
        relative_gap: 0.0,
        max_primal_residual: 0.0,
        elapsed_ms: 10,
        model_digest: [0xBB; 32],
        input_digest: [0xCC; 32],
        assignments: vec![a(1, 10.0), a(2, 20.0)],
        issued_at_ms: tick,
        ttl_ms: 60_000,
    })
    .expect("合法两步计划")
}

/// 两步计划：首拍只投到期步 1；步 2 未到期不投；推进时钟后重提交同计划
/// ——步 1 幂等命中（不重投）、步 2 到期投递。总投递恰 2 次（确定性
/// 重编译 + journal 幂等 = 无重复副作用）。
#[test]
fn typed_multi_step_plan_delivers_due_commands_per_tick() {
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0u64));
    let channel = TickCountingChannel {
        ingress,
        devices,
        delivered: delivered.clone(),
    };
    // 与 engine_with 同构，仅时钟可推进。
    let config = support::test_config_legacy(support::TestConfigSeed {
        chain: build_chain(),
        key: KEY.to_vec(),
        profile: test_profile(),
        issuer_service: ISSUER,
        fencing_token: 1 + EPOCH_SEED,
        now_fn: tick_now,
        intent_log: Box::new(eneros_control_dispatch::InMemoryIntentStore::new(64)),
        delivery: Box::new(channel),
    });
    let config = DispatchConfig {
        audit_sinks: eneros_control_dispatch::AuditSinks {
            audit: Some(DecisionAudit::new([0x40; 32])),
            ..config.audit_sinks
        },
        ..config
    };
    let mut engine = DispatchEngine::new(config);
    let plan = two_step_plan();

    // 第 1 拍（t=base）：步 1 到期投递；步 2 未到期跳过。
    let r1 = engine
        .submit_typed(&plan, &trusted_telemetry(), &actor())
        .expect("第 1 拍提交");
    assert!(r1.final_state.is_committed());
    assert_eq!(*delivered.borrow(), 1, "步 2 未到期不投递");

    // 第 2 拍（t=base+10s）：步 1 幂等命中（零投递），步 2 到期投递。
    TICK_MS.fetch_add(10_000, std::sync::atomic::Ordering::Relaxed);
    let r2 = engine
        .submit_typed(&plan, &trusted_telemetry(), &actor())
        .expect("第 2 拍提交");
    assert!(
        r2.final_state.is_committed(),
        "step2 receipt: {:?} note={:?}",
        r2.final_state,
        r2.note
    );
    assert_eq!(*delivered.borrow(), 2, "步 1 不重投、步 2 投递");

    // 第 3 拍（时钟不动）：两步全部幂等命中——零新投递。
    let r3 = engine
        .submit_typed(&plan, &trusted_telemetry(), &actor())
        .expect("第 3 拍提交");
    assert!(r3.final_state.is_committed());
    assert_eq!(
        *delivered.borrow(),
        2,
        "重放零投递（幂等键 sub_index 稳定）"
    );
}

// ---- WP-5.3 复查回归：[10] §3.6 故障矩阵「intent/audit 磁盘满」----

/// 落盘故障注入存储：`fail_append` 期间 append 返回 Unavailable（磁盘满
/// 形态），过后恢复。
struct FlakyStore {
    inner: InMemoryIntentStore,
    fail_append: std::cell::Cell<bool>,
}

impl IntentStore for FlakyStore {
    fn append(&mut self, rec: &IntentRecord) -> Result<(), IntentStoreError> {
        if self.fail_append.get() {
            return Err(IntentStoreError::Unavailable(String::from("disk full")));
        }
        self.inner.append(rec)
    }
    fn update_state(
        &mut self,
        command_id: &[u8; 16],
        state: eneros_control_dispatch::DispatchState,
    ) -> Result<(), IntentStoreError> {
        self.inner.update_state(command_id, state)
    }
    fn find(&self, command_id: &[u8; 16]) -> Option<IntentRecord> {
        self.inner.find(command_id)
    }
    fn pending(&self) -> Vec<IntentRecord> {
        self.inner.pending()
    }
}

/// 磁盘满（append 失败）时：journal 授权记录必须终局为 Failed（「授权后
/// 未投递即终局」——与安全/预算拒绝同律），不得遗留悬空 Authorized 非终态
/// ——否则同源（SiteOptimizer 生产链）续跑会被 Arbiter 的 MergeInSolver
/// 永久拒绝（在途登记自愈失效 → 锁死至重启）。
#[test]
fn append_failure_terminates_journal_and_resubmission_not_arbiter_locked() {
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0u64));
    let channel = CountingChannel {
        ingress,
        devices,
        delivered: delivered.clone(),
    };
    let profile = test_profile();
    let store = FlakyStore {
        inner: InMemoryIntentStore::new(64),
        fail_append: std::cell::Cell::new(true),
    };
    let base = support::test_config_legacy(support::TestConfigSeed {
        chain: build_chain(),
        key: KEY.to_vec(),
        profile,
        issuer_service: ISSUER,
        fencing_token: 1 + EPOCH_SEED,
        now_fn: now_ms,
        intent_log: Box::new(eneros_control_dispatch::InMemoryIntentStore::new(64)),
        delivery: Box::new(channel),
    });
    let mut engine = DispatchEngine::new(DispatchConfig {
        audit_sinks: eneros_control_dispatch::AuditSinks {
            audit: Some(DecisionAudit::new([0x40; 32])),
            intent_log: Box::new(store),
            ..base.audit_sinks
        },
        ..base
    });

    let p = proposal(50.0);
    let c = proposal_to_candidate_unbound(&p, 1).expect("adapter");
    // 第一拍：落盘失败 → 零投递 Err（journal 记录终局 Failed）。
    let e1 = engine
        .submit_typed(&c, &trusted_telemetry(), &actor())
        .unwrap_err();
    assert!(
        matches!(e1, DispatchError::IntentPersistFailed(_)),
        "磁盘满必须显式失败：{e1:?}"
    );
    assert_eq!(*delivered.borrow(), 0, "落盘失败零投递");
    // 第二拍（磁盘恢复）：同源续跑不被 Arbiter 拒绝——返回原终态
    // Failed（零投递；重放不复活失败命令），而不是 ProposalInvalid
    // 「merge-in-solver」锁死。
    let r2 = engine
        .submit_typed(&c, &trusted_telemetry(), &actor())
        .expect("同源续跑放行（不得锁死）");
    assert_eq!(
        r2.final_state,
        eneros_control_dispatch::DispatchState::Failed,
        "重放返回原终态 Failed（不假成功、不重复投递）"
    );
    assert_eq!(*delivered.borrow(), 0, "Failed 重放零投递");
}

// ============================================================================
// 蓝图 70.8 WP-70.8.4：可信输入准入（只验证、不改写）
// ============================================================================

/// 冻结快照身份事实（测试：与计划身份逐字段同代）。
fn trusted_facts(
    snapshot_id: [u8; 16],
    catalog_version: u64,
    produced_at_ns: u64,
) -> eneros_agent_sdk::snap_input_wire::SnapshotFacts {
    eneros_agent_sdk::snap_input_wire::SnapshotFacts {
        snapshot_id,
        provenance_root: [0x5A; 32],
        cut_time_ns: produced_at_ns,
        produced_at_ns,
        catalog_version,
        catalog_digest8: [1, 2, 3, 4, 5, 6, 7, 8],
        accepted: 7,
        missing: 0,
        completeness: eneros_agent_sdk::snap_input_wire::COMPLETE,
    }
}

/// 由冻结快照身份构造的**绑定**候选（生产形态：身份 + 摘要同源）。
fn bound_plan(
    snapshot_id: [u8; 16],
    catalog_version: u64,
    horizon_ms: u64,
) -> eneros_decision_contract::CandidatePlan {
    use eneros_control_dispatch::adapter::{proposal_to_candidate_bound, CandidateInputs};
    use eneros_dual_brain::ProposedControlPlan;
    let p = ProposedControlPlan::single_entry(
        String::from("bound-1"),
        DecisionPath::Fast,
        -30.0,
        0.5,
        eneros_energy_lp_model::result::SolveStatus::Optimal,
        60,
        horizon_ms,
        Some([0xAB; 32]),
    );
    let inputs = CandidateInputs::from_snapshot(snapshot_id, catalog_version, horizon_ms, 0, 1);
    proposal_to_candidate_bound(&p, &inputs, (1, 1, 10, 6), 6).expect("绑定候选构造")
}

/// 生产策略（Required）下：绑定候选 + 同代事实 → 通过准入并提交成功。
#[test]
fn trusted_input_admission_accepts_same_generation_candidate() {
    let (ingress, devices) = device_side();
    let channel = CountingChannel {
        ingress,
        devices,
        delivered: Rc::new(RefCell::new(0u64)),
    };
    let config = support::with_fixture_certifier(support::test_config(support::TestConfigSeed {
        chain: build_chain(),
        key: KEY.to_vec(),
        profile: test_profile(),
        issuer_service: ISSUER,
        fencing_token: 1 + EPOCH_SEED,
        now_fn: now_ms,
        intent_log: Box::new(InMemoryIntentStore::new(64)),
        delivery: Box::new(channel),
    }));
    assert_eq!(
        config.trusted_input,
        eneros_control_dispatch::TrustedInputPolicy::Required,
        "生产缺省 = Required（fail-closed）"
    );
    let mut engine = DispatchEngine::new(config);
    let produced_at_ns = now_ms() * 1_000_000;
    let facts = trusted_facts([0x33; 16], 1, produced_at_ns);
    let plan = bound_plan([0x33; 16], 1, now_ms());
    let input = eneros_agent_sdk::snap_input_wire::TrustedInput::new(facts, [0u8; 144]);
    let r = engine
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .expect("同代候选必须准入");
    assert!(r.final_state.is_committed());
}

/// 负例族（任一不匹配 → 整批拒绝、零投递）：
/// ① 无身份（Required）；② 快照不同代；③ 目录版本不同代；④ 时域起点
/// 不符；⑤ 摘要被篡改；⑥ 快照不完整。
#[test]
fn trusted_input_admission_rejects_mismatch_family() {
    fn engine_required() -> DispatchEngine {
        let (ingress, devices) = device_side();
        let channel = CountingChannel {
            ingress,
            devices,
            delivered: Rc::new(RefCell::new(0u64)),
        };
        DispatchEngine::new(support::test_config(support::TestConfigSeed {
            chain: build_chain(),
            key: KEY.to_vec(),
            profile: test_profile(),
            issuer_service: ISSUER,
            fencing_token: 1 + EPOCH_SEED,
            now_fn: now_ms,
            intent_log: Box::new(InMemoryIntentStore::new(64)),
            delivery: Box::new(channel),
        }))
    }
    let produced_at_ns = now_ms() * 1_000_000;
    let good = || {
        eneros_agent_sdk::snap_input_wire::TrustedInput::new(
            trusted_facts([0x33; 16], 1, produced_at_ns),
            [0u8; 144],
        )
    };

    // ① 无身份（legacy submit_typed 在 Required 下必须拒绝）。
    let mut e = engine_required();
    let plan = bound_plan([0x33; 16], 1, now_ms());
    let err = e
        .submit_typed(&plan, &trusted_telemetry(), &actor())
        .expect_err("缺可信输入必须拒绝");
    assert!(
        matches!(err, DispatchError::ProposalInvalid(ref m) if m.contains("缺可信输入身份")),
        "{err:?}"
    );

    // ② 快照不同代。
    let mut e = engine_required();
    let input = eneros_agent_sdk::snap_input_wire::TrustedInput::new(
        trusted_facts([0x44; 16], 1, produced_at_ns),
        [0u8; 144],
    );
    let err = e
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .expect_err("跨代快照必须拒绝");
    assert!(
        matches!(err, DispatchError::ProposalInvalid(ref m) if m.contains("不同代")),
        "{err:?}"
    );

    // ③ 目录版本不同代。
    let mut e = engine_required();
    let input = eneros_agent_sdk::snap_input_wire::TrustedInput::new(
        trusted_facts([0x33; 16], 2, produced_at_ns),
        [0u8; 144],
    );
    let err = e
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .expect_err("跨目录版本必须拒绝");
    assert!(
        matches!(err, DispatchError::ProposalInvalid(ref m) if m.contains("目录版本")),
        "{err:?}"
    );

    // ④ 时域起点不符（快照产出时刻 ≠ 计划时域起点）。
    let mut e = engine_required();
    let input = eneros_agent_sdk::snap_input_wire::TrustedInput::new(
        trusted_facts([0x33; 16], 1, produced_at_ns + 5_000_000_000),
        [0u8; 144],
    );
    let err = e
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .expect_err("时域不符必须拒绝");
    assert!(
        matches!(err, DispatchError::ProposalInvalid(ref m) if m.contains("时域起点")),
        "{err:?}"
    );

    // ⑤ 摘要被篡改（同快照但 objective_digest 与公式不符）。
    let mut e = engine_required();
    // 直接经 from_parts 构造同身份但摘要错误的计划（绕过构造器自检，
    // 模拟"伪造/陈旧摘要"到达入口）。
    let tampered = {
        let mut parts = eneros_decision_contract::CandidatePlanParts {
            id: *plan.id(),
            snapshot_id: plan.snapshot_id(),
            objective_digest: [0xEE; 32],
            resource_catalog_version: plan.resource_catalog_version(),
            horizon_start_ms: plan.horizon_start_ms(),
            step_ns: plan.step_ns(),
            step_count: plan.step_count(),
            solver_status: plan.solver_status(),
            objective_value: plan.objective_value(),
            relative_gap: plan.relative_gap(),
            max_primal_residual: plan.max_primal_residual(),
            elapsed_ms: plan.elapsed_ms(),
            model_digest: *plan.model_digest(),
            input_digest: *plan.input_digest(),
            assignments: plan.assignments().to_vec(),
            issued_at_ms: plan.issued_at_ms(),
            ttl_ms: plan.ttl_ms(),
        };
        parts.objective_digest = [0xEE; 32];
        eneros_decision_contract::CandidatePlan::from_parts(parts).expect("篡改摘要计划可构造")
    };
    let err = e
        .submit_trusted(&tampered, &good(), &trusted_telemetry(), &actor())
        .expect_err("摘要篡改必须拒绝");
    assert!(
        matches!(err, DispatchError::ProposalInvalid(ref m) if m.contains("目标摘要")),
        "{err:?}"
    );

    // ⑥ 快照不完整。
    let mut e = engine_required();
    let mut facts = trusted_facts([0x33; 16], 1, produced_at_ns);
    facts.missing = 1;
    facts.completeness = eneros_agent_sdk::snap_input_wire::INCOMPLETE;
    let input = eneros_agent_sdk::snap_input_wire::TrustedInput::new(facts, [0u8; 144]);
    let err = e
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .expect_err("不完整快照必须拒绝");
    assert!(
        matches!(err, DispatchError::ProposalInvalid(ref m) if m.contains("不完整")),
        "{err:?}"
    );
}

/// R10：批首命令结果未知时不能继续写另一资产，更不能用末条成功覆盖未知。
#[test]
fn multi_asset_unknown_stops_batch_and_is_not_reported_committed() {
    use eneros_control_dispatch::DispatchState;
    use eneros_decision_contract::{
        Assignment, CandidatePlan, CandidatePlanParts, PlanId, SnapshotId, SolveStatusKind,
    };
    struct Registry;
    impl eneros_decision_contract::ResourceRegistry for Registry {
        fn asset_exists(&self, id: u64) -> bool {
            matches!(id, 1 | 2)
        }
        fn asset_control_binding(&self, id: u64) -> Option<(u16, u32, u16)> {
            match id {
                1 => Some((1, 10, 6)),
                2 => Some((2, 20, 6)),
                _ => None,
            }
        }
        fn catalog_version(&self) -> u64 {
            1
        }
    }
    struct Channel(Rc<RefCell<usize>>);
    impl DeliveryChannel for Channel {
        fn deliver(
            &mut self,
            _: &str,
            _: &[u8],
            _: u64,
        ) -> Result<Vec<u8>, eneros_control_dispatch::ChannelError> {
            let mut n = self.0.borrow_mut();
            *n += 1;
            if *n == 1 {
                Ok(vec![0xff])
            } else {
                Ok(eneros_control_gateway::wire::OutcomeFrame::new(
                    eneros_control_gateway::wire::OutcomeCode::Applied,
                    0,
                )
                .encode()
                .to_vec())
            }
        }
    }
    let count = Rc::new(RefCell::new(0));
    let mut config = support::test_config_legacy(support::TestConfigSeed {
        chain: build_chain(),
        key: KEY.to_vec(),
        profile: test_profile(),
        issuer_service: ISSUER,
        fencing_token: 1 + EPOCH_SEED,
        now_fn: now_ms,
        intent_log: Box::new(InMemoryIntentStore::new(64)),
        delivery: Box::new(Channel(count.clone())),
    });
    let profile = ControlProfile {
        lease_ttl_secs: 600,
        ..ControlProfile::ai_pcs_setpoint(2, 20, 4, [0x48; 16], EPOCH_SEED)
    };
    let mut chain = ControlChain::from_profile(KEY, &profile);
    profile.issue_lease(&mut chain, now_ms() / 1000).unwrap();
    config
        .extra_routes
        .push(eneros_control_dispatch::AssetRoute {
            asset_id: 2,
            profile,
            chain,
        });
    config.resources = Box::new(Registry);
    let mut contracts = support::test_contract_view(1, 1);
    contracts.insert(
        2,
        eneros_control_dispatch::compiler::PlanContract {
            class: 2,
            readback_point_id: 1005,
            reversible: 1,
            min: -500.0,
            max: 500.0,
            settle_ms: 0,
            compensable: false,
        },
    );
    config.contracts = Box::new(contracts);
    let mut engine = DispatchEngine::new(config);
    let plan = CandidatePlan::from_parts(CandidatePlanParts {
        id: PlanId([0x79; 16]),
        snapshot_id: SnapshotId([2; 16]),
        objective_digest: [0xaa; 32],
        resource_catalog_version: 1,
        horizon_start_ms: now_ms(),
        step_ns: 10_000_000_000,
        step_count: 1,
        solver_status: SolveStatusKind::Optimal,
        objective_value: 1.0,
        relative_gap: 0.0,
        max_primal_residual: 0.0,
        elapsed_ms: 1,
        model_digest: [0xbb; 32],
        input_digest: [0xcc; 32],
        assignments: vec![
            Assignment {
                asset_id: 1,
                device_id: 1,
                point_id: 10,
                power_kw: 5.0,
                unit_code: 6,
                step_index: 1,
            },
            Assignment {
                asset_id: 2,
                device_id: 2,
                point_id: 20,
                power_kw: 5.0,
                unit_code: 6,
                step_index: 1,
            },
        ],
        issued_at_ms: now_ms(),
        ttl_ms: 60_000,
    })
    .unwrap();
    let receipt = engine
        .submit_typed(&plan, &trusted_telemetry(), &actor())
        .unwrap();
    assert_eq!(
        receipt.final_state,
        DispatchState::Indeterminate,
        "未知不能被后继成功覆盖"
    );
    assert_eq!(*count.borrow(), 1, "未知后必须阻断批内后继");
    assert_eq!(engine.committed_count(), 0);
    let replay = engine
        .submit_typed(&plan, &trusted_telemetry(), &actor())
        .unwrap();
    assert_eq!(replay.final_state, DispatchState::Indeterminate);
    assert_eq!(
        *count.borrow(),
        1,
        "same-key read must not resend or advance successor"
    );
}

#[test]
fn irreversible_plan_requires_manual_approval_before_any_delivery() {
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0));
    let mut config = support::test_config_legacy(support::TestConfigSeed {
        chain: build_chain(),
        key: KEY.to_vec(),
        profile: test_profile(),
        issuer_service: ISSUER,
        fencing_token: 1 + EPOCH_SEED,
        now_fn: now_ms,
        intent_log: Box::new(InMemoryIntentStore::new(64)),
        delivery: Box::new(CountingChannel {
            ingress,
            devices,
            delivered: delivered.clone(),
        }),
    });
    config.contracts = Box::new(support::test_contract_view(1, 0));
    let mut engine = DispatchEngine::new(config);
    let result = engine.submit_typed(
        &support::legacy_candidate(&proposal(5.0)),
        &trusted_telemetry(),
        &actor(),
    );
    assert!(
        matches!(result, Err(DispatchError::NotAuthorized(ref why)) if why.contains("manual approval")),
        "irreversible plan result: {result:?}"
    );
    assert_eq!(*delivered.borrow(), 0);
}

#[test]
fn higher_priority_intent_cannot_write_over_unknown_plan_effect() {
    struct UnknownChannel(Rc<RefCell<usize>>);
    impl DeliveryChannel for UnknownChannel {
        fn deliver(
            &mut self,
            _: &str,
            _: &[u8],
            _: u64,
        ) -> Result<Vec<u8>, eneros_control_dispatch::ChannelError> {
            *self.0.borrow_mut() += 1;
            Ok(vec![0xff])
        }
    }
    let delivered = Rc::new(RefCell::new(0));
    let mut engine = engine_with(Box::new(UnknownChannel(delivered.clone())));
    let first = engine
        .submit_typed(
            &support::legacy_candidate(&proposal(5.0)),
            &trusted_telemetry(),
            &actor(),
        )
        .unwrap();
    assert_eq!(
        first.final_state,
        eneros_control_dispatch::DispatchState::Indeterminate
    );
    let intent = eneros_protocol_abstract::write::PointWriteIntent::new(
        "grid",
        1,
        10,
        eneros_upa_model::PointValue::Float(10.0),
        1,
        now_ms(),
    );
    let result = engine.submit_intent(&intent, &trusted_telemetry(), &actor(), 5_000);
    assert!(
        matches!(result, Err(DispatchError::NotAuthorized(ref why)) if why.contains("reconcile")),
        "unknown effect must settle before preemption: {result:?}"
    );
    assert_eq!(
        *delivered.borrow(),
        1,
        "new key must not overwrite an unknown physical effect"
    );
}

#[test]
fn restart_pending_journal_blocks_a_new_command_key() {
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0));
    let mut store = InMemoryIntentStore::new(64);
    store
        .append(&IntentRecord {
            command_id: [0x81; 16],
            correlation_id: [0x82; 16],
            issued_at_ms: now_ms() - 1000,
            expires_at_ms: now_ms() + 10_000,
            target_device: "pcs".into(),
            power_kw: 5.0,
            input_digest: None,
            state: eneros_control_dispatch::DispatchState::Indeterminate,
            command_digest: Some(eneros_protocol_abstract::write::semantic_command_digest(
                1,
                10,
                eneros_telemetry_model::command::CommandValue::ActivePowerKw(5.0),
            )),
            attempt: 1,
            receipt_digest: None,
            plan_sha256: Some([0x83; 32]),
        })
        .unwrap();
    let mut engine = DispatchEngine::new(support::test_config_legacy(support::TestConfigSeed {
        chain: build_chain(),
        key: KEY.to_vec(),
        profile: test_profile(),
        issuer_service: ISSUER,
        fencing_token: 1 + EPOCH_SEED,
        now_fn: now_ms,
        intent_log: Box::new(store),
        delivery: Box::new(CountingChannel {
            ingress,
            devices,
            delivered: delivered.clone(),
        }),
    }));
    let result = engine.submit_typed(
        &support::legacy_candidate(&proposal(10.0)),
        &trusted_telemetry(),
        &actor(),
    );
    assert!(
        result.is_err(),
        "new plan must wait for recovered unknown: {result:?}"
    );
    assert_eq!(*delivered.borrow(), 0);
}

thread_local! { static HELD_CLOCK: std::cell::Cell<u64> = const { std::cell::Cell::new(1_700_000_000_000) }; }
fn held_now() -> u64 {
    HELD_CLOCK.with(|clock| clock.get())
}

#[derive(Clone)]
struct HeldMedia(Rc<RefCell<eneros_durable_state::MemIo>>);
impl eneros_durable_state::PersistIo for HeldMedia {
    fn read(
        &mut self,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<(), eneros_durable_state::StoreError> {
        self.0.borrow_mut().read(offset, bytes)
    }
    fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<(), eneros_durable_state::StoreError> {
        self.0.borrow_mut().write(offset, bytes)
    }
    fn flush(&mut self) -> Result<(), eneros_durable_state::StoreError> {
        self.0.borrow_mut().flush()
    }
    fn capacity(&self) -> u64 {
        self.0.borrow().capacity
    }
}
struct HeldChannel {
    inner: CountingChannel,
    lose_ack: bool,
    actual: Rc<std::cell::Cell<f64>>,
}
impl DeliveryChannel for HeldChannel {
    fn deliver(
        &mut self,
        _: &str,
        payload: &[u8],
        _: u64,
    ) -> Result<Vec<u8>, eneros_control_dispatch::ChannelError> {
        *self.inner.delivered.borrow_mut() += 1;
        let out = self.inner.ingress.borrow_mut().process(
            &mut self.inner.devices.borrow_mut(),
            payload,
            held_now(),
        );
        let frame = eneros_device_agent::outcome_to_frame(&out);
        if frame.code == eneros_device_agent::CODE_APPLIED {
            let signed = ControlEnvelope::decode(payload)
                .unwrap()
                .decode_inner()
                .unwrap();
            if let eneros_telemetry_model::command::CommandValue::ActivePowerKw(power) =
                signed.value
            {
                self.actual.set(power);
            }
        }
        if self.lose_ack {
            self.lose_ack = false;
            return Err(eneros_control_dispatch::ChannelError::Timeout);
        }
        Ok(eneros_device_agent::outcome_to_frame(&out)
            .encode()
            .to_vec())
    }
    fn readback(
        &mut self,
        _device: u32,
        _point: u32,
        _timeout_ms: u64,
    ) -> Result<f64, eneros_control_dispatch::ChannelError> {
        // 设备读回漂移 +0.05 kW（容差 0.1 内、数值 ≠ 请求设定值）——
        // 让「Saga 记录的是独立读回值还是设定值回显」可判别。
        Ok(self.actual.get() + 0.05)
    }
}

fn held_fixture(media: HeldMedia, delivered: Rc<RefCell<u64>>) -> DispatchEngine {
    held_fixture_fault(media, delivered, false)
}
fn held_fixture_fault(
    media: HeldMedia,
    delivered: Rc<RefCell<u64>>,
    lose_ack: bool,
) -> DispatchEngine {
    held_fixture_retained_fault(
        media,
        delivered,
        lose_ack,
        Rc::new(RefCell::new(RetainerLog::default())),
    )
}
/// 蓝图 77 卷一 WP-77.1.2：夹具默认注入**有记录的真 retainer**——持有
/// 路径不再经「store 先在、retainer 缺席」的装配窗口（该窗口曾允许无钉
/// 准入）。需要检视钉账的测试传入自己的 log。
fn held_fixture_retained(
    media: HeldMedia,
    delivered: Rc<RefCell<u64>>,
    log: Rc<RefCell<RetainerLog>>,
) -> DispatchEngine {
    held_fixture_retained_fault(media, delivered, false, log)
}
fn held_fixture_retained_fault(
    media: HeldMedia,
    delivered: Rc<RefCell<u64>>,
    lose_ack: bool,
    log: Rc<RefCell<RetainerLog>>,
) -> DispatchEngine {
    held_fixture_with_retainer_fault(media, delivered, lose_ack, Box::new(RecordingRetainer(log)))
}
/// 自定义 retainer 变体（FailingRelease / FlickerRelease / RefuseHold 等
/// 专项替身——负例按行为显式注入）。
fn held_fixture_with_retainer(
    media: HeldMedia,
    delivered: Rc<RefCell<u64>>,
    retainer: Box<dyn eneros_control_dispatch::HeldInputRetainer>,
) -> DispatchEngine {
    held_fixture_with_retainer_fault(media, delivered, false, retainer)
}
fn held_fixture_with_retainer_fault(
    media: HeldMedia,
    delivered: Rc<RefCell<u64>>,
    lose_ack: bool,
    retainer: Box<dyn eneros_control_dispatch::HeldInputRetainer>,
) -> DispatchEngine {
    held_fixture_channels_retained(
        media,
        delivered,
        lose_ack,
        Rc::new(std::cell::Cell::new(0.0)),
        Box::new(InMemoryIntentStore::new(64)),
        retainer,
    )
}
/// 5 参直通变体（FaultWrites/Cut 等自定义介质夹具用；默认记录 retainer）。
fn held_fixture_channels<Io: eneros_durable_state::PersistIo + 'static>(
    media: Io,
    delivered: Rc<RefCell<u64>>,
    lose_ack: bool,
    actual: Rc<std::cell::Cell<f64>>,
    intent_log: Box<dyn IntentStore>,
) -> DispatchEngine {
    held_fixture_channels_retained(
        media,
        delivered,
        lose_ack,
        actual,
        intent_log,
        Box::new(RecordingRetainer(Rc::new(RefCell::new(
            RetainerLog::default(),
        )))),
    )
}
/// WP-77.1.2：store 与保留面**一次交付**（`HeldOwner::assemble` →
/// `install_held_owner`）——两步装配 API 已退役。
fn held_fixture_channels_retained<Io: eneros_durable_state::PersistIo + 'static>(
    media: Io,
    delivered: Rc<RefCell<u64>>,
    lose_ack: bool,
    actual: Rc<std::cell::Cell<f64>>,
    intent_log: Box<dyn IntentStore>,
    retainer: Box<dyn eneros_control_dispatch::HeldInputRetainer>,
) -> DispatchEngine {
    let (ingress, devices) = device_side();
    let config = support::test_config(support::TestConfigSeed {
        chain: build_chain(),
        key: KEY.to_vec(),
        profile: test_profile(),
        issuer_service: ISSUER,
        fencing_token: 1 + EPOCH_SEED,
        now_fn: held_now,
        intent_log,
        delivery: Box::new(HeldChannel {
            lose_ack,
            actual,
            inner: CountingChannel {
                ingress,
                devices,
                delivered,
            },
        }),
    });
    let mut engine = DispatchEngine::new(support::with_fixture_certifier(config));
    let plans = eneros_control_dispatch::held_plan::DurablePlans::open(media, 0, 131_072).unwrap();
    engine.install_held_owner(eneros_control_dispatch::HeldOwner::assemble(
        plans, retainer,
    ));
    engine
}
fn held_candidate() -> (
    eneros_decision_contract::CandidatePlan,
    eneros_agent_sdk::snap_input_wire::TrustedInput,
) {
    held_candidate_tuned(5_000_000_000, 20_000)
}

/// 参数化持有候选（步长 ns / ttl ms）——时域合同红绿测共用夹具。
fn held_candidate_tuned(
    step_ns: u64,
    ttl_ms: u64,
) -> (
    eneros_decision_contract::CandidatePlan,
    eneros_agent_sdk::snap_input_wire::TrustedInput,
) {
    held_candidate_steps(4, step_ns, ttl_ms)
}

/// WP-77.1.2：步数可变候选（1 步 = legacy 单命令轨——held_device_busy
/// 判据的注入面）。
fn held_candidate_steps(
    steps: u32,
    step_ns: u64,
    ttl_ms: u64,
) -> (
    eneros_decision_contract::CandidatePlan,
    eneros_agent_sdk::snap_input_wire::TrustedInput,
) {
    use eneros_agent_sdk::snap_input_wire::{SnapshotFacts, TrustedInput, COMPLETE};
    use eneros_decision_contract::{
        Assignment, CandidatePlan, CandidatePlanParts, PlanId, SnapshotId, SolveStatusKind,
    };
    let facts = SnapshotFacts {
        snapshot_id: [0x52; 16],
        provenance_root: [0xCC; 32],
        cut_time_ns: now_ms() * 1_000_000,
        produced_at_ns: now_ms() * 1_000_000,
        catalog_version: 1,
        catalog_digest8: [5; 8],
        accepted: 6,
        missing: 0,
        completeness: COMPLETE,
    };
    let plan = CandidatePlan::from_parts(CandidatePlanParts {
        id: PlanId([0x72; 16]),
        snapshot_id: SnapshotId(facts.snapshot_id),
        objective_digest: eneros_decision_contract::objective_binding_v1(
            &facts.snapshot_id,
            1,
            now_ms(),
            step_ns,
            steps,
        ),
        resource_catalog_version: 1,
        horizon_start_ms: now_ms(),
        step_ns,
        step_count: steps,
        solver_status: SolveStatusKind::Optimal,
        objective_value: 1.0,
        relative_gap: 0.0,
        max_primal_residual: 0.0,
        elapsed_ms: 10,
        model_digest: [0xBB; 32],
        input_digest: facts.provenance_root,
        assignments: (1..=steps)
            .map(|step| Assignment {
                asset_id: 1,
                device_id: 1,
                point_id: 10,
                power_kw: f64::from(step),
                unit_code: 6,
                step_index: step,
            })
            .collect(),
        issued_at_ms: now_ms(),
        ttl_ms,
    })
    .unwrap();
    (plan, TrustedInput::new(facts, [0; 144]))
}

#[test]
fn durable_plan_submitted_once_runs_four_steps_and_restart_cancels_unsent() {
    use eneros_control_dispatch::saga::{BatchState, CmdState};
    let (plan, input) = held_candidate();
    for restart in [false, true] {
        HELD_CLOCK.with(|clock| clock.set(now_ms()));
        let media = HeldMedia(Rc::new(RefCell::new(eneros_durable_state::MemIo::new(
            131_072,
        ))));
        let delivered = Rc::new(RefCell::new(0));
        let mut engine = held_fixture(media.clone(), delivered.clone());
        let receipt = engine
            .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
            .unwrap();
        assert_eq!(
            receipt.final_state,
            eneros_control_dispatch::DispatchState::Persisted,
            "accepted future steps cannot claim whole-plan Committed"
        );
        assert_eq!(*delivered.borrow(), 1);
        if restart {
            drop(engine);
            // A new process whose monotonic clock restarted cannot reinterpret
            // the old plan windows and send their pending suffix.
            HELD_CLOCK.with(|clock| clock.set(10));
            engine = held_fixture(media.clone(), delivered.clone());
            engine.drive_held_plans(None, None).unwrap();
            assert_eq!(*delivered.borrow(), 1);
            let report = engine.held_plan_reports().unwrap().remove(0);
            assert_eq!(report.final_state, BatchState::Failed);
            assert_eq!(report.commands[0].state, CmdState::Applied);
            assert!(report.commands[1..]
                .iter()
                .all(|c| c.state == CmdState::Blocked && c.attempts == 0));
        } else {
            for step in 1..=3 {
                HELD_CLOCK.with(|clock| clock.set(now_ms() + step * 5_000));
                engine
                    .drive_held_plans(Some(&input), Some(&trusted_telemetry()))
                    .unwrap();
                assert_eq!(
                    *delivered.borrow(),
                    step + 1,
                    "independent tick drives the retained future command"
                );
            }
            let report = engine.held_plan_reports().unwrap().remove(0);
            assert_eq!(report.final_state, BatchState::Completed);
            assert!(report
                .commands
                .iter()
                .all(|c| c.state == CmdState::Applied && c.attempts == 1));
            engine
                .drive_held_plans(Some(&input), Some(&trusted_telemetry()))
                .unwrap();
            assert_eq!(*delivered.borrow(), 4);
            drop(engine);
            let reopened = held_fixture(media, delivered.clone());
            assert_eq!(
                reopened.held_plan_reports().unwrap()[0],
                report,
                "durable query survives restart"
            );
        }
    }
}

/// ADR-0054:34–35 时域合同红绿测：多步计划时域不得越过可信输入/证书
/// 有效期（30s 控制窗是安全事实——不为 4×15s 配置放宽）。
/// ① 红：4×15s=60s 时域 > 30s 窗 → 提交即拒（fail-closed，不静默截断）；
/// ② 绿（边界）：时域末恰等于有效期（4×7.5s=30s == valid_until）→ 受理；
/// ③ 过期输入：有效期后 tick → 未发步取消、零新写（输入过期 ≠ 决策被拒
///    之外的任何形态）。
/// 变异靶：放宽 CONTROL_HORIZON_MS/夹具窗到 60s → ① 变绿（60≤60）→ 本测红。
#[test]
fn held_plan_horizon_beyond_control_window_rejected() {
    // 夹具遥测 valid_until = now + 30_000（control_horizon_ms 30s）。
    let valid_until = trusted_telemetry().valid_until_ms;
    let now = now_ms();
    assert_eq!(valid_until, now + 30_000);

    // ① 4×15s=60s > 30s：提交即拒。
    let (over, input_over) = held_candidate_tuned(15_000_000_000, 60_000);
    let media = HeldMedia(Rc::new(RefCell::new(eneros_durable_state::MemIo::new(
        131_072,
    ))));
    let delivered = Rc::new(RefCell::new(0));
    let mut engine = held_fixture(media, delivered.clone());
    let err = engine
        .submit_trusted(&over, &input_over, &trusted_telemetry(), &actor())
        .expect_err("时域越过控制窗必须拒绝");
    assert!(
        format!("{err:?}").contains("held horizon exceeds trusted telemetry"),
        "具体拒绝面：{err:?}"
    );
    assert_eq!(*delivered.borrow(), 0);
    assert!(
        engine.held_plan_reports().unwrap().is_empty(),
        "被拒计划不得入持有存储"
    );

    // ② 边界：4×7.5s=30s == valid_until（≤ 语义）→ 受理。
    let (edge, input_edge) = held_candidate_tuned(7_500_000_000, 30_000);
    let media2 = HeldMedia(Rc::new(RefCell::new(eneros_durable_state::MemIo::new(
        131_072,
    ))));
    let delivered2 = Rc::new(RefCell::new(0));
    let mut engine2 = held_fixture(media2, delivered2.clone());
    let receipt = engine2
        .submit_trusted(&edge, &input_edge, &trusted_telemetry(), &actor())
        .expect("时域末 == 有效期必须受理（≤）");
    assert_eq!(
        receipt.final_state,
        eneros_control_dispatch::DispatchState::Persisted
    );

    // ③ 过期输入：tick 时钟越过 valid_until → 未发步取消、零新写。
    HELD_CLOCK.with(|clock| clock.set(valid_until + 1_000));
    engine2
        .drive_held_plans(Some(&input_edge), Some(&trusted_telemetry()))
        .unwrap();
    let report = engine2.held_plan_reports().unwrap().remove(0);
    use eneros_control_dispatch::saga::{BatchState, CmdState};
    assert_eq!(report.final_state, BatchState::Failed);
    assert!(
        report.commands[1..]
            .iter()
            .all(|c| c.state == CmdState::Blocked
                && c.reason
                    .as_deref()
                    .is_some_and(|r| r.contains("no longer valid"))),
        "过期输入的未发步必须以可信理由取消：{:?}",
        report.commands
    );
    // 首步在受理时已发（1 次）；过期 tick 零新写。
    assert_eq!(*delivered2.borrow(), 1);
}

#[test]
fn durable_plan_readback_settles_unknown_journal_before_next_step() {
    HELD_CLOCK.with(|clock| clock.set(now_ms()));
    let media = HeldMedia(Rc::new(RefCell::new(eneros_durable_state::MemIo::new(
        131_072,
    ))));
    let delivered = Rc::new(RefCell::new(0));
    let mut engine = held_fixture_fault(media, delivered.clone(), true);
    let (plan, input) = held_candidate();
    engine
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .unwrap();
    assert_eq!(*delivered.borrow(), 1);
    assert!(
        engine.pending_intents().is_empty(),
        "double independent readback must durably settle the unknown command journal"
    );
    HELD_CLOCK.with(|clock| clock.set(now_ms() + 5_000));
    engine
        .drive_held_plans(Some(&input), Some(&trusted_telemetry()))
        .unwrap();
    assert_eq!(
        *delivered.borrow(),
        2,
        "settled first step does not block the next authorized step"
    );
}

/// R16-B 差异锁（蓝图 75 WP-75.1.1）：恢复对账终局（RecoveredApplied
/// 表行，恢复路径经 settle_saga_applied → settle_command 收表）**有意**
/// 不重复记账——committed_count 不变、gate 零 record。journal/intent
/// 照常推进终局（对账事实落盘：pending 清空），但恢复路径绝不重放正常
/// Applied 行的记账面（gate.record/committed_count/states.push）。本测
/// 在现行为下即绿——它锁的是「有意差异」，防未来有人好心补记账。
/// 变异咬合②：settle_command RecoveredApplied 分支偷加
/// `committed_count += 1` 或 `gate.record` ⇒ 本测红。
#[test]
fn recovered_applied_settle_does_not_double_book() {
    HELD_CLOCK.with(|clock| clock.set(now_ms()));
    let media = HeldMedia(Rc::new(RefCell::new(eneros_durable_state::MemIo::new(
        131_072,
    ))));
    let delivered = Rc::new(RefCell::new(0));
    let mut engine = held_fixture_fault(media, delivered.clone(), true);
    let (plan, input) = held_candidate();
    engine
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .unwrap();
    assert_eq!(*delivered.borrow(), 1, "设备已写（ACK 丢失形态）");
    // 同 tick 内恢复对账已把 unknown journal 结算到终局（独立回读匹配）。
    assert!(
        engine.pending_intents().is_empty(),
        "恢复结算已把 journal/intent 推进终局"
    );
    // 差异锁本体：恢复结算**不重复记账**。
    assert_eq!(
        engine.committed_count(),
        0,
        "恢复不重复记账（有意差异，R16-B）"
    );
    assert_eq!(
        engine.gate_used_kwh(),
        0.0,
        "恢复不触发 gate.record（有意差异，R16-B）"
    );
}

/// ADR-0054:15–16：新鲜 Applied 必须携带**实际回读**证据（Saga 事件持久
/// 化、HQR1 可查），且读回值 = 独立读回（设备漂移 +0.05），不是请求
/// 设定值回显。变异靶：post_apply_readback 改为回填 intent.power_kw()
/// → 断言 Some(power + 0.05) 红；删读回调用 → 断言 Some(...) 红。
#[test]
fn applied_commands_carry_independent_readback_not_setpoint() {
    use eneros_control_dispatch::saga::{BatchState, CmdState};
    HELD_CLOCK.with(|clock| clock.set(now_ms()));
    let media = HeldMedia(Rc::new(RefCell::new(eneros_durable_state::MemIo::new(
        131_072,
    ))));
    let delivered = Rc::new(RefCell::new(0));
    let mut engine = held_fixture(media, delivered.clone());
    let (plan, input) = held_candidate();
    engine
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .unwrap();
    for step in 1..=3u32 {
        HELD_CLOCK.with(|clock| clock.set(now_ms() + u64::from(step) * 5_000));
        engine
            .drive_held_plans(Some(&input), Some(&trusted_telemetry()))
            .unwrap();
    }
    let report = engine.held_plan_reports().unwrap().remove(0);
    assert_eq!(report.final_state, BatchState::Completed);
    for (i, c) in report.commands.iter().enumerate() {
        assert_eq!(c.state, CmdState::Applied, "全部四步 Applied");
        let power = f64::from(i as u32 + 1);
        assert_eq!(
            c.readback,
            Some(power + 0.05),
            "第 {} 步回读必须是独立读值（power+0.05），不是设定值 {} 回显",
            i + 1,
            power
        );
    }
}

#[derive(Default)]
struct RetainerLog {
    held: Vec<[u8; 16]>,
    released: Vec<[u8; 16]>,
    refuse_hold: bool,
    /// WP-77.1.3：全量同步账（调用次数 + 最近一次确认集）。
    sync_calls: usize,
    last_sync_set: Vec<[u8; 16]>,
    sync_ok: bool,
}
struct RecordingRetainer(Rc<RefCell<RetainerLog>>);
impl eneros_control_dispatch::HeldInputRetainer for RecordingRetainer {
    fn hold(&mut self, snapshot_id: [u8; 16]) -> bool {
        let mut log = self.0.borrow_mut();
        if log.refuse_hold {
            return false;
        }
        if !log.held.contains(&snapshot_id) {
            log.held.push(snapshot_id);
        }
        true
    }
    fn release(&mut self, snapshot_id: [u8; 16]) -> bool {
        let mut log = self.0.borrow_mut();
        log.released.push(snapshot_id);
        log.held.retain(|h| *h != snapshot_id);
        true
    }
    fn sync(&mut self, ids: &[[u8; 16]]) -> bool {
        // WP-77.1.3：同步 = 钉集原子确认（测试替身：记账并按 sync_ok 表达
        // 远端确认成败）。
        let mut log = self.0.borrow_mut();
        log.sync_calls += 1;
        log.last_sync_set = ids.to_vec();
        let ok = log.sync_ok;
        if ok {
            log.held.clear();
            for id in ids {
                if !log.held.contains(id) {
                    log.held.push(*id);
                }
            }
        }
        ok
    }
}

/// ADR-0054:29–30 红绿测（引擎侧保留面）：
/// ① 准入即钉：submit 成功 ⇒ hold(快照 ID) 已登记；
/// ② 完成即释：全部步 Applied 后 ⇒ release 被调、钉集清空；
/// ③ fail-closed：钉面拒收 ⇒ 准入被拒、零投递、零持有；
/// ④ 断电重钉：重启重开（同介质）+ 装配保留面 ⇒ 非终态计划的快照
///    被重新钉住（钉是 RAM 派生态——唯一持久事实 = 持有计划本身）。
#[test]
fn held_plan_pins_input_and_releases_on_completion() {
    let log = Rc::new(RefCell::new(RetainerLog::default()));
    HELD_CLOCK.with(|clock| clock.set(now_ms()));
    let media = HeldMedia(Rc::new(RefCell::new(eneros_durable_state::MemIo::new(
        131_072,
    ))));
    let delivered = Rc::new(RefCell::new(0));
    let mut engine = held_fixture_retained(media, delivered.clone(), log.clone());
    let (plan, input) = held_candidate();
    let expected_id = input.facts().snapshot_id;
    engine
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .expect("钉面在位 ⇒ 准入");
    assert_eq!(log.borrow().held, vec![expected_id], "准入即钉");
    for step in 1..=3u64 {
        HELD_CLOCK.with(|clock| clock.set(now_ms() + step * 5_000));
        engine
            .drive_held_plans(Some(&input), Some(&trusted_telemetry()))
            .unwrap();
    }
    // 最后一次 drive 触发 finalize：终态且无未决 ⇒ release。
    assert!(
        log.borrow().released.contains(&expected_id),
        "完成即释：{:?}",
        log.borrow().released
    );
    assert!(log.borrow().held.is_empty(), "钉集清空");
}

#[test]
fn held_admission_fails_closed_when_retention_refuses() {
    let log = Rc::new(RefCell::new(RetainerLog {
        refuse_hold: true,
        ..RetainerLog::default()
    }));
    HELD_CLOCK.with(|clock| clock.set(now_ms()));
    let media = HeldMedia(Rc::new(RefCell::new(eneros_durable_state::MemIo::new(
        131_072,
    ))));
    let delivered = Rc::new(RefCell::new(0));
    let mut engine = held_fixture_retained(media, delivered.clone(), log.clone());
    let (plan, input) = held_candidate();
    let err = engine
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .expect_err("钉面拒收 ⇒ 准入必须 fail-closed");
    assert!(
        format!("{err:?}").contains("input retention unavailable"),
        "具体拒绝面：{err:?}"
    );
    assert_eq!(*delivered.borrow(), 0);
    assert!(
        engine.held_plan_reports().unwrap().is_empty(),
        "被拒计划不得入持有存储"
    );
}

/// 蓝图 77 卷一 WP-77.1.2 重写（原「先 store 后 retainer 的装配顺序变体」
/// 历史回归——旧期待"无钉先成功提交"正是蓝图 §1.3 的公开入口缺口）：
/// **完整装配（store+retainer 一体）下提交 → 钉住；掉电（RAM 钉丢失）→
/// 重开同介质 → 完整装配 → 非终态计划按存活引用重钉；同 ID 重放回原
/// 回执、零新投递。**
#[test]
fn held_input_repin_after_restart() {
    let log = Rc::new(RefCell::new(RetainerLog::default()));
    HELD_CLOCK.with(|clock| clock.set(now_ms()));
    let media = HeldMedia(Rc::new(RefCell::new(eneros_durable_state::MemIo::new(
        131_072,
    ))));
    let delivered = Rc::new(RefCell::new(0));
    let (plan, input) = held_candidate();
    let expected_id = input.facts().snapshot_id;
    {
        let mut engine = held_fixture_retained(media.clone(), delivered.clone(), log.clone());
        engine
            .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
            .unwrap();
        assert_eq!(log.borrow().held, vec![expected_id], "准入即钉");
    }
    log.borrow_mut().held.clear(); // 模拟掉电：RAM 钉集丢失
    let mut reopened = held_fixture_retained(media, delivered.clone(), log.clone());
    assert!(reopened.held_plan_reports().is_ok(), "非终态计划在介质上");
    assert_eq!(
        log.borrow().held,
        vec![expected_id],
        "重启 + owner 完整装配 ⇒ 非终态计划引用的快照被重新钉住"
    );
    // 既有同 ID 重放：不重新执行、不产生第二次物理投递。
    let sent_before = *delivered.borrow();
    let receipt = reopened
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .unwrap();
    assert_eq!(
        receipt.final_state,
        eneros_control_dispatch::DispatchState::Persisted,
        "重放回原回执（accepted 计划不重复投递）"
    );
    assert_eq!(*delivered.borrow(), sent_before, "重放零新投递");
}

/// 蓝图 77 卷一 WP-77.1.3（dispatch 侧）：每个 held tick 发送**完整活跃
/// 集**同步（`!terminal || unresolved` 全集）；远端未确认 ⇒ stale ⇒ 新
/// 持有准入阻断（同 ID 重放不受累）；确认恢复后准入恢复。
/// 变异靶：删 tick_held 同步调用 / stale 门 → 本测红。
#[test]
fn held_tick_syncs_full_live_set_and_stale_blocks_admission() {
    HELD_CLOCK.with(|clock| clock.set(now_ms()));
    let media = HeldMedia(Rc::new(RefCell::new(eneros_durable_state::MemIo::new(
        131_072,
    ))));
    let delivered = Rc::new(RefCell::new(0));
    let log = Rc::new(RefCell::new(RetainerLog::default()));
    let (plan, input) = held_candidate();
    let sid = input.facts().snapshot_id;
    let mut engine = held_fixture_retained(media.clone(), delivered.clone(), log.clone());
    engine
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .unwrap();
    // held tick：全量活跃集同步（在途计划 ⇒ {sid}）。
    let active = engine
        .tick_held(Some(&input), Some(&trusted_telemetry()))
        .unwrap();
    assert_eq!(active, 1);
    let l = log.borrow();
    assert_eq!(l.sync_calls, 1, "held tick 必须发送全量同步");
    assert_eq!(l.last_sync_set, vec![sid], "同步集 = 存活引用全集");
    drop(l);

    // 空载 tick（计划全部终态后）⇒ 空集同步（冷启动屏障解除语义）。
    for step in 1..=3u64 {
        HELD_CLOCK.with(|clock| clock.set(now_ms() + step * 5_000));
        engine
            .drive_held_plans(Some(&input), Some(&trusted_telemetry()))
            .unwrap();
    }
    HELD_CLOCK.with(|clock| clock.set(now_ms() + 16_000));
    let active = engine
        .tick_held(Some(&input), Some(&trusted_telemetry()))
        .unwrap();
    assert_eq!(active, 0);
    let l = log.borrow();
    assert_eq!(l.sync_calls, 2);
    assert!(l.last_sync_set.is_empty(), "无活跃计划 ⇒ 空集同步");
    drop(l);

    // 远端未确认（sync_ok=false）⇒ stale ⇒ 新准入被拒；同 ID 重放不受累。
    // （时间保持 < 计划 TTL 20s——过期拒绝先于 stale 门，会掩蔽判据。）
    log.borrow_mut().sync_ok = false;
    HELD_CLOCK.with(|clock| clock.set(now_ms() + 17_000));
    let active = engine
        .tick_held(Some(&input), Some(&trusted_telemetry()))
        .unwrap();
    assert_eq!(active, 0, "stale 不折零——计数/对账照常");
    // 新计划（不同 id）⇒ stale 拒绝。
    let err = engine
        .submit_trusted(
            &renamed_plan(&plan, 0x7E),
            &input,
            &trusted_telemetry(),
            &actor(),
        )
        .expect_err("同步未确认 ⇒ 新持有准入必须被拒");
    assert!(
        format!("{err:?}").contains("held snapshot sync pending"),
        "stale 拒绝面：{err:?}"
    );
    // 同 ID 重放（原计划已终态在册）不受 stale 影响——重放回原回执。
    let receipt = engine
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .expect("重放不受 stale 影响");
    assert_eq!(
        receipt.final_state,
        eneros_control_dispatch::DispatchState::Committed
    );
    // 同步恢复 ⇒ 新准入恢复（容量语义：终态在册计划被换代挤出）。
    log.borrow_mut().sync_ok = true;
    HELD_CLOCK.with(|clock| clock.set(now_ms() + 18_000));
    engine
        .tick_held(Some(&input), Some(&trusted_telemetry()))
        .unwrap();
    let receipt = engine
        .submit_trusted(
            &renamed_plan(&plan, 0x7E),
            &input,
            &trusted_telemetry(),
            &actor(),
        )
        .expect("同步确认后新准入恢复");
    assert!(
        matches!(
            receipt.final_state,
            eneros_control_dispatch::DispatchState::Persisted
                | eneros_control_dispatch::DispatchState::Committed
        ),
        "准入恢复（晚时钟提交可能同 tick 终态）：{:?}",
        receipt.final_state
    );
}

/// 蓝图 77 卷一 WP-77.1.2：**装配未完成 ⇒ 提交被拒，完成后可恢复并重钉**。
/// 未完成形态 = 介质上已有存活计划、而钉面在**装配期重钉**时拒绝 ⇒
/// owner 未就绪 ⇒ 新准入 fail-closed（store 计划数与设备写计数均不增加）；
/// 完整装配重开同介质 ⇒ 恢复重钉 + 同 ID 重放回原回执。
/// 变异靶：HeldOwner::assemble 折叠重钉失败（照常 ready）或删除
/// `held_retention_fault` 准入门 → 本测红。
#[test]
fn held_assembly_incomplete_rejects_submission_until_ready() {
    HELD_CLOCK.with(|clock| clock.set(now_ms()));
    let media = HeldMedia(Rc::new(RefCell::new(eneros_durable_state::MemIo::new(
        131_072,
    ))));
    let delivered = Rc::new(RefCell::new(0));
    let (plan, input) = held_candidate();
    let sid = input.facts().snapshot_id;
    {
        // 健康装配：计划入持久 store（装配期重钉成功）。
        let log = Rc::new(RefCell::new(RetainerLog::default()));
        let mut engine = held_fixture_retained(media.clone(), delivered.clone(), log);
        engine
            .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
            .unwrap();
        assert_eq!(*delivered.borrow(), 1);
    }
    {
        // 装配未就绪：存活计划在场而装配期重钉被拒 ⇒ 故障态。
        let log = Rc::new(RefCell::new(RetainerLog {
            refuse_hold: true,
            ..RetainerLog::default()
        }));
        let mut engine = held_fixture_with_retainer(
            media.clone(),
            delivered.clone(),
            Box::new(RecordingRetainer(log)),
        );
        let err = engine
            .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
            .expect_err("装配未就绪 ⇒ 提交必须被拒");
        assert!(
            format!("{err:?}").contains("held retention fault"),
            "显式保留故障拒绝面：{err:?}"
        );
        // 遗留单命令轨（System1 兜底/协议写同链）同样被保留故障阻断——
        // 钉失败阻断的不只是计划准入，还有一切新设备写。
        let (single, single_input) = held_candidate_steps(1, 5_000_000_000, 20_000);
        let err2 = engine
            .submit_trusted(&single, &single_input, &trusted_telemetry(), &actor())
            .expect_err("单命令轨同样被保留故障阻断");
        assert!(
            format!("{err2:?}").contains("held retention fault"),
            "单命令轨阻断面：{err2:?}"
        );
        assert_eq!(*delivered.borrow(), 1, "设备写计数不增加（无新投递）");
        let reports = engine.held_plan_reports().unwrap();
        assert_eq!(reports.len(), 1, "store 计划数不增加（仅重启前那份）");
    }
    // 完整装配（可钉）⇒ 重开同介质：装配期恢复重钉 + 提交可恢复。
    let ok_log = Rc::new(RefCell::new(RetainerLog::default()));
    let mut ready = held_fixture_retained(media, delivered.clone(), ok_log.clone());
    assert_eq!(
        ok_log.borrow().held,
        vec![sid],
        "完整装配 ⇒ 存活计划的快照在装配期被重钉"
    );
    let sent_before = *delivered.borrow();
    let receipt = ready
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .expect("装配完成后可恢复提交（同 ID 重放）");
    assert_eq!(
        receipt.final_state,
        eneros_control_dispatch::DispatchState::Persisted
    );
    assert_eq!(*delivered.borrow(), sent_before, "重放零新投递");
}

// ============================================================
// F70-04（R1/R2）：共享输入保留所有权与可靠释放（H01～H08 host 面）
// ============================================================

/// 计划改号（改写 plan id 字节 = 不同计划、同输入快照——共享保留的
/// 对抗形态）。与审查复现（held-repro）同构造。
fn renamed_plan(
    plan: &eneros_decision_contract::CandidatePlan,
    id: u8,
) -> eneros_decision_contract::CandidatePlan {
    let mut bytes = plan.encode_v1();
    bytes[8..24].fill(id);
    eneros_decision_contract::CandidatePlan::decode_v1(&bytes).unwrap()
}

/// H01/R1 红转绿：两份不同计划共享同一快照；第三份因容量满被拒——
/// 失败回滚**不得**解除前两份在途计划的保留（旧实现无条件 release 同
/// 一快照 ID，集合去重无引用计数 ⇒ 在途计划的输入钉被一并撤掉）。
#[test]
fn h01_capacity_rejection_keeps_live_plans_pinned() {
    let log = Rc::new(RefCell::new(RetainerLog::default()));
    HELD_CLOCK.with(|clock| clock.set(now_ms()));
    let media = HeldMedia(Rc::new(RefCell::new(eneros_durable_state::MemIo::new(
        131_072,
    ))));
    let mut engine = held_fixture_retained(media, Rc::new(RefCell::new(0)), log.clone());
    let (plan, input) = held_candidate();
    engine
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .unwrap();
    engine
        .submit_trusted(
            &renamed_plan(&plan, 0x73),
            &input,
            &trusted_telemetry(),
            &actor(),
        )
        .unwrap();
    let error = engine
        .submit_trusted(
            &renamed_plan(&plan, 0x74),
            &input,
            &trusted_telemetry(),
            &actor(),
        )
        .unwrap_err();
    assert!(format!("{error:?}").contains("capacity full"), "{error:?}");
    assert!(
        engine
            .held_plan_reports()
            .unwrap()
            .iter()
            .any(|r| !r.final_state.is_terminal()),
        "已有非终态计划在场"
    );
    assert!(
        log.borrow().held.contains(&input.facts().snapshot_id),
        "在途计划的输入钉不得被第三份的失败回滚解除（R1）"
    );
    assert!(
        !log.borrow().released.contains(&input.facts().snapshot_id),
        "共享快照仍被在途计划引用——不得 release"
    );
}

/// H02/R1（recheck RR1 重订）：第二份共享计划因**真实持久写失败**被拒
/// （`StoreError::WriteFailed` → store poisoned——不是容量面）⇒ 回滚查询
/// `store.plans()` 读取失败（状态未知），**不得**折叠成零引用释放第一
/// 份在途计划的钉（旧实现 `unwrap_or_default()` 恰在此误释放）。
#[derive(Clone)]
struct FailWritesHeldMedia {
    inner: HeldMedia,
    fail_writes: Rc<std::cell::Cell<bool>>,
}
impl eneros_durable_state::PersistIo for FailWritesHeldMedia {
    fn read(
        &mut self,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<(), eneros_durable_state::StoreError> {
        self.inner.read(offset, bytes)
    }
    fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<(), eneros_durable_state::StoreError> {
        if self.fail_writes.get() {
            return Err(eneros_durable_state::StoreError::WriteFailed);
        }
        self.inner.write(offset, bytes)
    }
    fn flush(&mut self) -> Result<(), eneros_durable_state::StoreError> {
        self.inner.flush()
    }
    fn capacity(&self) -> u64 {
        self.inner.capacity()
    }
}

#[test]
fn h02_real_persist_failure_preserves_existing_pin() {
    let log = Rc::new(RefCell::new(RetainerLog::default()));
    HELD_CLOCK.with(|clock| clock.set(now_ms()));
    let media = FailWritesHeldMedia {
        inner: HeldMedia(Rc::new(RefCell::new(eneros_durable_state::MemIo::new(
            131_072,
        )))),
        fail_writes: Rc::new(std::cell::Cell::new(false)),
    };
    let mut engine = held_fixture_channels_retained(
        media.clone(),
        Rc::new(RefCell::new(0)),
        false,
        Rc::new(std::cell::Cell::new(0.0)),
        Box::new(InMemoryIntentStore::new(64)),
        Box::new(RecordingRetainer(log.clone())),
    );
    let (plan, input) = held_candidate();
    engine
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .unwrap();
    let sid = input.facts().snapshot_id;
    // 武装介质写失败：第二份（同快照、不同计划）的持久提交真实失败。
    media.fail_writes.set(true);
    let error = engine
        .submit_trusted(
            &renamed_plan(&plan, 0x99),
            &input,
            &trusted_telemetry(),
            &actor(),
        )
        .unwrap_err();
    let text = format!("{error:?}");
    assert!(
        text.contains("held storage") && text.contains("write-failed"),
        "必须是真实持久写失败（poisoned），实得 {text}"
    );
    assert!(
        !text.contains("capacity"),
        "容量拒绝路径不经本缺陷（H01 已覆盖）"
    );
    // 钉集：第一份在途计划的保留不得被回滚解除（RR1 核心断言）。
    assert!(
        log.borrow().held.contains(&sid),
        "读取失败 = 引用状态未知 ⇒ 不得按零引用释放（在途计划的钉保持）"
    );
    assert!(
        !log.borrow().released.contains(&sid),
        "真实持久写失败后误 release = RR1 缺陷形态"
    );
    assert_eq!(
        log.borrow().held.iter().filter(|h| **h == sid).count(),
        1,
        "钉集恰一份所有权（幂等集合，不累计）"
    );
}

/// 蓝图 77 卷一 WP-77.1.1（引擎侧判据回归锁）：held store poisoned ⇒
/// `tick_held` 必须传播 Err——**绝不折叠成活跃 0**（EA 据此放行 System1
/// 的条件性故障面）。注：plans() 镜像读的失败源 = poisoned ⇒ 写失败必已
/// 先传播，旧 `unwrap_or(0)` 折叠在现结构下不可达；本重构（finalize 单读
/// 化 + 显式传播）把该面从「静默折零」收紧为「不可能折零」，本测锁
/// poisoned 后 tick_held ≠ Ok(_) 的接口事实。
#[test]
fn held_store_poison_fails_held_tick_not_active_zero() {
    HELD_CLOCK.with(|clock| clock.set(now_ms()));
    let media = FailWritesHeldMedia {
        inner: HeldMedia(Rc::new(RefCell::new(eneros_durable_state::MemIo::new(
            131_072,
        )))),
        fail_writes: Rc::new(std::cell::Cell::new(false)),
    };
    let delivered = Rc::new(RefCell::new(0));
    let mut engine = held_fixture_channels_retained(
        media.clone(),
        delivered,
        false,
        Rc::new(std::cell::Cell::new(0.0)),
        Box::new(InMemoryIntentStore::new(64)),
        Box::new(RecordingRetainer(Rc::new(RefCell::new(
            RetainerLog::default(),
        )))),
    );
    let (plan, input) = held_candidate();
    engine
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .unwrap();
    // 武装介质写失败：任何持久写失败都会 poison store（fail-closed）。
    media.fail_writes.set(true);
    let poisoned = engine
        .submit_trusted(
            &renamed_plan(&plan, 0x9A),
            &input,
            &trusted_telemetry(),
            &actor(),
        )
        .is_err();
    assert!(poisoned, "写失败必须使提交失败（poison 前置）");
    // poisoned 后：活跃查询面必须 Err（读错不折零）。
    let tick = engine.tick_held(Some(&input), Some(&trusted_telemetry()));
    assert!(
        tick.is_err(),
        "store 读失败必须传播 Err——不得折叠成活跃 0，实得 {tick:?}"
    );
}

/// H02（容量面回归保留）：第三份容量拒绝的回滚不解除前两份在途计划的
/// 保留（H01 同族；与上面的真实写失败路径互补）。
#[test]
fn h02_capacity_rollback_keeps_owners() {
    let log = Rc::new(RefCell::new(RetainerLog::default()));
    HELD_CLOCK.with(|clock| clock.set(now_ms()));
    let media = HeldMedia(Rc::new(RefCell::new(eneros_durable_state::MemIo::new(
        131_072,
    ))));
    let mut engine = held_fixture_retained(media, Rc::new(RefCell::new(0)), log.clone());
    let (plan, input) = held_candidate();
    engine
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .unwrap();
    let sid = input.facts().snapshot_id;
    let err = engine
        .submit_trusted(
            &renamed_plan(&plan, 0x99),
            &input,
            &trusted_telemetry(),
            &actor(),
        )
        .err();
    // 容量面或持久面拒绝均可——关键断言在钉集：
    let _ = err;
    assert_eq!(
        log.borrow().held.iter().filter(|h| **h == sid).count(),
        1,
        "钉集恰一份所有权（幂等集合，不累计）"
    );
    assert!(
        !log.borrow().released.contains(&sid),
        "第一份仍在途——快照不得被释放"
    );
}

/// H03/§7.2.3：一份结束、另一份在途 ⇒ 保留；最后一份也结束 ⇒ 无引用
/// 且无未决后最终释放（释放以「快照仍被谁需要」为准，不以「有一份
/// 计划结束」为准）。
#[test]
fn h03_shared_snapshot_released_only_after_last_plan_finishes() {
    let log = Rc::new(RefCell::new(RetainerLog::default()));
    HELD_CLOCK.with(|clock| clock.set(now_ms()));
    let media = HeldMedia(Rc::new(RefCell::new(eneros_durable_state::MemIo::new(
        131_072,
    ))));
    let delivered = Rc::new(RefCell::new(0));
    let mut engine = held_fixture_retained(media, delivered.clone(), log.clone());
    let (plan, input) = held_candidate();
    let sid = input.facts().snapshot_id;
    let plan2 = renamed_plan(&plan, 0x55);
    engine
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .unwrap();
    engine
        .submit_trusted(&plan2, &input, &trusted_telemetry(), &actor())
        .unwrap();
    // 推进：每 tick 一份计划。第一份完成后，第二份仍在途 ⇒ 保留。
    for step in 1..=4u64 {
        HELD_CLOCK.with(|clock| clock.set(now_ms() + step * 5_000));
        engine
            .drive_held_plans(Some(&input), Some(&trusted_telemetry()))
            .unwrap();
        if step <= 3 {
            assert!(
                !log.borrow().released.contains(&sid),
                "第二份仍在途（step={step}）——共享快照不得提前释放"
            );
        }
    }
    // 第二份也终态 ⇒ 无引用无未决 ⇒ 释放恰一次。
    assert!(log.borrow().released.contains(&sid), "最后一份结束后释放");
    assert!(
        log.borrow().held.is_empty(),
        "钉集归零：{:?}",
        log.borrow().held
    );
}

/// H04/R2 红转绿：最后一份计划释放失败后，**空闲 tick**（无非终态
/// 计划）必须继续重试——旧实现空闲路径提前 return 跳过 finalize，
/// 保留持续泄漏。
struct FailingRelease(Rc<RefCell<usize>>);
impl eneros_control_dispatch::HeldInputRetainer for FailingRelease {
    fn hold(&mut self, _: [u8; 16]) -> bool {
        true
    }
    fn release(&mut self, _: [u8; 16]) -> bool {
        *self.0.borrow_mut() += 1;
        *self.0.borrow() > 1
    }
    fn sync(&mut self, _: &[[u8; 16]]) -> bool {
        true
    }
}

#[test]
fn h04_release_failure_retries_on_idle_ticks() {
    let count = Rc::new(RefCell::new(0));
    HELD_CLOCK.with(|clock| clock.set(now_ms()));
    let media = HeldMedia(Rc::new(RefCell::new(eneros_durable_state::MemIo::new(
        131_072,
    ))));
    let mut engine = held_fixture_with_retainer(
        media,
        Rc::new(RefCell::new(0)),
        Box::new(FailingRelease(count.clone())),
    );

    let (plan, input) = held_candidate();
    engine
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .unwrap();
    for step in 1..=3u64 {
        HELD_CLOCK.with(|clock| clock.set(now_ms() + step * 5_000));
        engine
            .drive_held_plans(Some(&input), Some(&trusted_telemetry()))
            .unwrap();
    }
    assert_eq!(*count.borrow(), 1, "首次释放恰一次（失败）");
    // 空闲 tick（计划已全部终态、无新提案）⇒ 必须重试并收敛。
    engine
        .drive_held_plans(Some(&input), Some(&trusted_telemetry()))
        .unwrap();
    assert_eq!(*count.borrow(), 2, "失败释放必须在后续周期重试（R2）");
    // 已收敛：再空闲 tick 不再重发（幂等缓存生效）。
    engine
        .drive_held_plans(Some(&input), Some(&trusted_telemetry()))
        .unwrap();
    assert_eq!(*count.borrow(), 2, "收敛后不得重复释放");
}

/// H05/§7.2.5：release 已生效但回应丢失（远端实际已解除、本地看到
/// false）⇒ 重试按"确认已无本次持有"收敛；不泄漏、不影响其他持有人。
struct FlickerRelease {
    calls: Rc<RefCell<usize>>,
    pins: Rc<RefCell<Vec<[u8; 16]>>>,
}
impl eneros_control_dispatch::HeldInputRetainer for FlickerRelease {
    fn hold(&mut self, id: [u8; 16]) -> bool {
        if !self.pins.borrow().contains(&id) {
            self.pins.borrow_mut().push(id);
        }
        true
    }
    fn release(&mut self, id: [u8; 16]) -> bool {
        *self.calls.borrow_mut() += 1;
        let mut pins = self.pins.borrow_mut();
        match pins.iter().position(|p| *p == id) {
            // 第一次：实际解除但"回应丢失"（false）；重试：absent ⇒
            // 幂等确认 true（远端语义，与 event_face 对齐）。
            Some(idx) => {
                pins.swap_remove(idx);
                false
            }
            None => true,
        }
    }
    fn sync(&mut self, ids: &[[u8; 16]]) -> bool {
        *self.pins.borrow_mut() = ids.to_vec();
        true
    }
}

#[test]
fn h05_release_reply_lost_converges_idempotently() {
    let flicker = FlickerRelease {
        calls: Rc::new(RefCell::new(0)),
        pins: Rc::new(RefCell::new(Vec::new())),
    };
    HELD_CLOCK.with(|clock| clock.set(now_ms()));
    let media = HeldMedia(Rc::new(RefCell::new(eneros_durable_state::MemIo::new(
        131_072,
    ))));
    let mut engine = held_fixture_with_retainer(
        media,
        Rc::new(RefCell::new(0)),
        Box::new(flicker_hoist(&flicker)),
    );

    let (plan, input) = held_candidate();
    let sid = input.facts().snapshot_id;
    engine
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .unwrap();
    for step in 1..=3u64 {
        HELD_CLOCK.with(|clock| clock.set(now_ms() + step * 5_000));
        engine
            .drive_held_plans(Some(&input), Some(&trusted_telemetry()))
            .unwrap();
    }
    // 第一次 release "回应丢失"（远端已实际解除）；下一个（空闲）tick
    // 重试 ⇒ absent 幂等确认 ⇒ 收敛。钉集必须为空（不泄漏）。
    engine
        .drive_held_plans(Some(&input), Some(&trusted_telemetry()))
        .unwrap();
    assert!(
        flicker.pins.borrow().is_empty(),
        "回应丢失后必须幂等收敛，钉集不泄漏"
    );
    let calls_after_converge = *flicker.calls.borrow();
    engine
        .drive_held_plans(Some(&input), Some(&trusted_telemetry()))
        .unwrap();
    assert_eq!(
        *flicker.calls.borrow(),
        calls_after_converge,
        "收敛后不再重发"
    );
    let _ = sid;
}

/// H05 夹具提升（FlickerRelease 的克隆面——Rc 内部可变，全引擎共享一态）。
fn flicker_hoist(f: &FlickerRelease) -> FlickerRelease {
    FlickerRelease {
        calls: f.calls.clone(),
        pins: f.pins.clone(),
    }
}

/// H06/§7.2.6：同一快照 hold→release→再次 hold（新计划）→release——
/// 每轮均可正确释放；旧完成缓存不得抑制新一轮；重复提交同计划不累计
/// 引用。
#[test]
fn h06_reheld_snapshot_releases_each_round() {
    let log = Rc::new(RefCell::new(RetainerLog::default()));
    HELD_CLOCK.with(|clock| clock.set(now_ms()));
    let media = HeldMedia(Rc::new(RefCell::new(eneros_durable_state::MemIo::new(
        131_072,
    ))));
    let mut engine = held_fixture_retained(media, Rc::new(RefCell::new(0)), log.clone());
    let (plan, input) = held_candidate();
    let sid = input.facts().snapshot_id;
    // 第一轮：提交→完成→释放。
    engine
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .unwrap();
    // 重复提交同计划：幂等回执，不产生新引用/新钉。
    engine
        .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
        .unwrap();
    for step in 1..=3u64 {
        HELD_CLOCK.with(|clock| clock.set(now_ms() + step * 5_000));
        engine
            .drive_held_plans(Some(&input), Some(&trusted_telemetry()))
            .unwrap();
    }
    assert!(log.borrow().released.contains(&sid), "第一轮释放");
    assert_eq!(
        log.borrow().held.iter().filter(|h| **h == sid).count(),
        0,
        "重复提交不累计引用"
    );
    // 第二轮：新计划重新持有同一快照→完成→必须再次释放（旧缓存失效）。
    // 注：本夹具的第二轮计划在准入 drive 内即可整体完成——hold 与
    // release 都发生在同一 submit 内；判据 = 「每轮都发生 release」，
    // 不以提交后 pin 仍在场为断言（同步完成时 pin 已被正确收尾）。
    log.borrow_mut().released.clear();
    let plan2 = renamed_plan(&plan, 0x66);
    engine
        .submit_trusted(&plan2, &input, &trusted_telemetry(), &actor())
        .unwrap();
    for step in 4..=7u64 {
        HELD_CLOCK.with(|clock| clock.set(now_ms() + step * 5_000));
        engine
            .drive_held_plans(Some(&input), Some(&trusted_telemetry()))
            .unwrap();
    }
    assert!(
        log.borrow().released.contains(&sid),
        "第二轮完成后必须再次释放（旧 released_holds 缓存代次失效——H06）"
    );
    assert!(log.borrow().held.is_empty(), "钉集归零");
}

/// H08（host 面）：dispatch 重启（同介质重开）+ 装配保留面 ⇒ 未决计划的
/// 持有按持久状态重建（既有 `held_input_repin_after_restart` 的共享形态
/// 变体：两份在途、重启后两份引用都在、恢复不重发旧命令）。
#[test]
fn h08_restart_rebuilds_shared_pins_without_resend() {
    let log = Rc::new(RefCell::new(RetainerLog::default()));
    HELD_CLOCK.with(|clock| clock.set(now_ms()));
    let media = HeldMedia(Rc::new(RefCell::new(eneros_durable_state::MemIo::new(
        131_072,
    ))));
    let delivered = Rc::new(RefCell::new(0));
    let (plan, input) = held_candidate();
    let sid = input.facts().snapshot_id;
    {
        let mut engine = held_fixture_retained(media.clone(), delivered.clone(), log.clone());
        engine
            .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
            .unwrap();
        engine
            .submit_trusted(
                &renamed_plan(&plan, 0x77),
                &input,
                &trusted_telemetry(),
                &actor(),
            )
            .unwrap();
    }
    let sent_before = *delivered.borrow();
    log.borrow_mut().held.clear(); // 掉电：RAM 钉集丢失
    let mut reopened = held_fixture_retained(media, delivered.clone(), log.clone());
    assert_eq!(
        log.borrow().held.iter().filter(|h| **h == sid).count(),
        1,
        "共享快照按持久计划重建（集合语义恰一份）"
    );
    // 恢复 tick（旧单调窗失效——只读对账，不重发旧命令）。
    reopened.drive_held_plans(None, None).unwrap();
    assert_eq!(*delivered.borrow(), sent_before, "恢复不得重发旧命令");
}

#[derive(Clone)]
struct CutHeldMedia {
    bytes: Rc<RefCell<Vec<u8>>>,
    remaining: Rc<std::cell::Cell<Option<usize>>>,
}
impl eneros_durable_state::PersistIo for CutHeldMedia {
    fn read(
        &mut self,
        offset: u64,
        out: &mut [u8],
    ) -> Result<(), eneros_durable_state::StoreError> {
        let bytes = self.bytes.borrow();
        let start = offset as usize;
        let part = bytes
            .get(start..start + out.len())
            .ok_or(eneros_durable_state::StoreError::OutOfSpace)?;
        out.copy_from_slice(part);
        Ok(())
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), eneros_durable_state::StoreError> {
        let take = self
            .remaining
            .get()
            .map(|left| left.min(data.len()))
            .unwrap_or(data.len());
        self.bytes.borrow_mut()[offset as usize..offset as usize + take]
            .copy_from_slice(&data[..take]);
        if let Some(left) = self.remaining.get() {
            self.remaining.set(Some(left - take));
            if take < data.len() {
                return Err(eneros_durable_state::StoreError::WriteFailed);
            }
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<(), eneros_durable_state::StoreError> {
        Ok(())
    }
    fn capacity(&self) -> u64 {
        self.bytes.borrow().len() as u64
    }
}
#[derive(Clone)]
struct SharedHeldIntents(Rc<RefCell<InMemoryIntentStore>>);
impl IntentStore for SharedHeldIntents {
    fn append(&mut self, record: &IntentRecord) -> Result<(), IntentStoreError> {
        self.0.borrow_mut().append(record)
    }
    fn update_state(
        &mut self,
        id: &[u8; 16],
        state: eneros_control_dispatch::DispatchState,
    ) -> Result<(), IntentStoreError> {
        self.0.borrow_mut().update_state(id, state)
    }
    fn find(&self, id: &[u8; 16]) -> Option<IntentRecord> {
        self.0.borrow().find(id)
    }
    fn pending(&self) -> Vec<IntentRecord> {
        self.0.borrow().pending()
    }
}

#[test]
fn held_plan_100_torn_write_points_never_resend_or_lose_confirmed_prefix() {
    use eneros_control_dispatch::saga::CmdState;
    let mut before_send = 0;
    let mut after_send = 0;
    let mut failed_after_send = 0;
    for cut in 0..100 {
        HELD_CLOCK.with(|clock| clock.set(now_ms()));
        let media = CutHeldMedia {
            bytes: Rc::new(RefCell::new(vec![0; 131_072])),
            remaining: Rc::new(std::cell::Cell::new(None)),
        };
        let delivered = Rc::new(RefCell::new(0));
        let actual = Rc::new(std::cell::Cell::new(0.0));
        let intents = SharedHeldIntents(Rc::new(RefCell::new(InMemoryIntentStore::new(64))));
        let mut engine = held_fixture_channels(
            media.clone(),
            delivered.clone(),
            false,
            actual.clone(),
            Box::new(intents.clone()),
        );
        let (plan, input) = held_candidate();
        engine
            .submit_trusted(&plan, &input, &trusted_telemetry(), &actor())
            .unwrap();
        assert_eq!(*delivered.borrow(), 1);
        assert_eq!(actual.get(), 1.0);
        media.remaining.set(Some(cut * 97));
        HELD_CLOCK.with(|clock| clock.set(now_ms() + 5_000));
        let result = engine.drive_held_plans(Some(&input), Some(&trusted_telemetry()));
        let sent = *delivered.borrow();
        if sent == 1 {
            before_send += 1;
        } else {
            after_send += 1;
            if result.is_err() {
                failed_after_send += 1;
            }
        }
        assert!(sent <= 2);
        drop(engine);
        media.remaining.set(None);
        HELD_CLOCK.with(|clock| clock.set(10));
        let mut recovered = held_fixture_channels(
            media,
            delivered.clone(),
            false,
            actual.clone(),
            Box::new(intents),
        );
        recovered.drive_held_plans(None, None).unwrap();
        assert_eq!(
            *delivered.borrow(),
            sent,
            "cut={cut}: cold recovery must never send"
        );
        let report = recovered.held_plan_reports().unwrap().remove(0);
        assert_eq!(
            report.commands[0].state,
            CmdState::Applied,
            "cut={cut}: confirmed prefix preserved"
        );
        assert!(report.commands[2..]
            .iter()
            .all(|command| command.state == CmdState::Blocked && command.attempts == 0));
        if sent == 2 {
            assert_eq!(
                report.commands[1].state,
                CmdState::Applied,
                "cut={cut}: sent effect independently recovered"
            );
            assert_eq!(actual.get(), 2.0);
        } else {
            assert_eq!(report.commands[1].state, CmdState::Blocked);
            assert_eq!(actual.get(), 1.0);
        }
        assert!(
            recovered.pending_intents().is_empty(),
            "cut={cut}: no unreconciled journal after proven readback"
        );
    }
    assert!(
        before_send > 0 && after_send > 0 && failed_after_send > 0,
        "campaign must cut both sides of real device writes"
    );
}
