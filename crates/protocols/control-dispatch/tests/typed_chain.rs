//! 蓝图 61 v0.314.28：typed 产物链集成测试（[06] §3.6）。
//!
//! - facade（submit(proposal)）与 typed 入口（submit_typed(CandidatePlan)）
//!   在参考闭环的值与证据一致——同一 proposal 两路径 correlation/幂等键
//!   逐字节相同（跨路径幂等 = 身份一致的强证明）；
//! - typed 入口拒绝过期计划；
//! - `submit_typed` 走过渡认证证书 + CommandIntent/CommandBatch
//!   （profile 资源视图 scope 校验在批构造内）。
mod support;

use std::cell::RefCell;
use std::rc::Rc;

use eneros_control_dispatch::adapter::proposal_to_candidate;
use eneros_control_dispatch::intent::{
    InMemoryIntentStore, IntentRecord, IntentStore, IntentStoreError,
};
use eneros_control_dispatch::{
    AuthenticatedActor, ControlDispatch, DeliveryChannel, DispatchConfig, DispatchEngine,
    DispatchError, DispatchReceipt,
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

fn trusted_telemetry() -> eneros_safety_validator::TrustedTelemetry {
    use eneros_safety_validator::{
        EvaluatorConfig, IntegrityStatus, RequiredPoint, RequiredPointCoverage, SampleQuality,
        StateBindings, TelemetrySample, TelemetryTrustEvaluator,
    };
    let mut ev = TelemetryTrustEvaluator::new(EvaluatorConfig {
        coverage: RequiredPointCoverage {
            points: vec![
                RequiredPoint {
                    point: 1,
                    min: 0.0,
                    max: 1000.0,
                    max_age_ms: 5000,
                    unit: eneros_safety_validator::PointUnit::Volt,
                },
                RequiredPoint {
                    point: 2,
                    min: -1000.0,
                    max: 1000.0,
                    max_age_ms: 5000,
                    unit: eneros_safety_validator::PointUnit::Ampere,
                },
                RequiredPoint {
                    point: 3,
                    min: 45.0,
                    max: 55.0,
                    max_age_ms: 5000,
                    unit: eneros_safety_validator::PointUnit::Hertz,
                },
                RequiredPoint {
                    point: 4,
                    min: 0.0,
                    max: 1.0,
                    max_age_ms: 5000,
                    unit: eneros_safety_validator::PointUnit::Ratio,
                },
                RequiredPoint {
                    point: 5,
                    min: 0.0,
                    max: 1.0,
                    max_age_ms: 5000,
                    unit: eneros_safety_validator::PointUnit::Ratio,
                },
                RequiredPoint {
                    point: 6,
                    min: 0.0,
                    max: 1.0,
                    max_age_ms: 5000,
                    unit: eneros_safety_validator::PointUnit::Ratio,
                },
            ],
        },
        bindings: StateBindings {
            voltage: 1,
            current: 2,
            frequency: 3,
            soc: 4,
        },
        mapping_version: 1,
        max_skew_ms: 1000,
        allowed_sources: vec![1],
        require_integrity: true,
        control_horizon_ms: 30_000,
    });
    let now = now_ms();
    let good = |point: u32, value: f64, seq: u64| TelemetrySample {
        point,
        value,
        source: 1,
        source_epoch: 0,
        measured_at_ms: now - 50,
        received_at_ms: now,
        quality: SampleQuality::Good,
        sequence: seq,
        integrity: IntegrityStatus::Verified,
        mapping_version: 1,
    };
    let samples = vec![
        good(1, 380.0, 1),
        good(2, 10.0, 1),
        good(3, 50.0, 1),
        good(4, 0.6, 1),
        good(5, 1.0, 1),
        good(6, 0.0, 1),
    ];
    ev.evaluate(&samples, now).expect("trusted telemetry")
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

/// facade 与 typed 入口在参考闭环值/证据一致：同一 proposal 两路径
/// correlation/幂等键逐字节相同——typed 提交命中 facade 的幂等记录
/// （零重复投递），强证明两链共享身份。
#[test]
fn facade_and_typed_share_identity() {
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0u64));
    let channel = CountingChannel {
        ingress,
        devices,
        delivered: delivered.clone(),
    };
    let mut engine = engine_with(Box::new(channel));

    let p = proposal(50.0);
    // 第一条：facade 路径（旧 proposal 形态）。
    let r1: DispatchReceipt = engine
        .submit(&p, &trusted_telemetry(), &actor(), 5_000)
        .expect("facade submit");
    // 第二条：typed 路径（同一 proposal 的 CandidatePlan——adapter 相同
    // 派生）→ 命中同一幂等记录，不重复投递。
    let c = proposal_to_candidate(&p, (1, 1, 10, 6), 6, 1).expect("adapter");
    let r2 = engine
        .submit_typed(&c, "pcs", &trusted_telemetry(), &actor(), 5_000)
        .expect("typed submit");
    assert_eq!(r1.final_state, r2.final_state);
    assert_eq!(*delivered.borrow(), 1, "跨路径幂等：一次投递");
}

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
    let c = proposal_to_candidate(&p, (1, 1, 10, 6), 6, 1).expect("adapter");
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
        .submit_typed(&c, "pcs", &trusted_telemetry(), &actor(), 5_000)
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
    let mut c = proposal_to_candidate(&p, (1, 1, 10, 6), 6, 1).expect("adapter");
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
        .submit_typed(&expired, "pcs", &trusted_telemetry(), &actor(), 5_000)
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
        .submit_typed(&plan, "pcs", &trusted_telemetry(), &actor(), 5_000)
        .expect("第 1 拍提交");
    assert!(r1.final_state.is_committed());
    assert_eq!(*delivered.borrow(), 1, "步 2 未到期不投递");

    // 第 2 拍（t=base+10s）：步 1 幂等命中（零投递），步 2 到期投递。
    TICK_MS.fetch_add(10_000, std::sync::atomic::Ordering::Relaxed);
    let r2 = engine
        .submit_typed(&plan, "pcs", &trusted_telemetry(), &actor(), 5_000)
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
        .submit_typed(&plan, "pcs", &trusted_telemetry(), &actor(), 5_000)
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
    let c = proposal_to_candidate(&p, (1, 1, 10, 6), 6, 1).expect("adapter");
    // 第一拍：落盘失败 → 零投递 Err（journal 记录终局 Failed）。
    let e1 = engine
        .submit_typed(&c, "pcs", &trusted_telemetry(), &actor(), 5_000)
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
        .submit_typed(&c, "pcs", &trusted_telemetry(), &actor(), 5_000)
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
        .submit_trusted(&plan, &input, "pcs", &trusted_telemetry(), &actor(), 5_000)
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
        .submit_typed(&plan, "pcs", &trusted_telemetry(), &actor(), 5_000)
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
        .submit_trusted(&plan, &input, "pcs", &trusted_telemetry(), &actor(), 5_000)
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
        .submit_trusted(&plan, &input, "pcs", &trusted_telemetry(), &actor(), 5_000)
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
        .submit_trusted(&plan, &input, "pcs", &trusted_telemetry(), &actor(), 5_000)
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
        .submit_trusted(
            &tampered,
            &good(),
            "pcs",
            &trusted_telemetry(),
            &actor(),
            5_000,
        )
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
        .submit_trusted(&plan, &input, "pcs", &trusted_telemetry(), &actor(), 5_000)
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
        .submit_typed(&plan, "pcs", &trusted_telemetry(), &actor(), 5000)
        .unwrap();
    assert_eq!(
        receipt.final_state,
        DispatchState::Indeterminate,
        "未知不能被后继成功覆盖"
    );
    assert_eq!(*count.borrow(), 1, "未知后必须阻断批内后继");
    assert_eq!(engine.committed_count(), 0);
    let replay = engine
        .submit_typed(&plan, "pcs", &trusted_telemetry(), &actor(), 5000)
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
    let result = engine.submit(&proposal(5.0), &trusted_telemetry(), &actor(), 5_000);
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
        .submit(&proposal(5.0), &trusted_telemetry(), &actor(), 5_000)
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
    let result = engine.submit(&proposal(10.0), &trusted_telemetry(), &actor(), 5_000);
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
    held_fixture_channels(
        media,
        delivered,
        lose_ack,
        Rc::new(std::cell::Cell::new(0.0)),
        Box::new(InMemoryIntentStore::new(64)),
    )
}
fn held_fixture_channels<Io: eneros_durable_state::PersistIo + 'static>(
    media: Io,
    delivered: Rc<RefCell<u64>>,
    lose_ack: bool,
    actual: Rc<std::cell::Cell<f64>>,
    intent_log: Box<dyn IntentStore>,
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
    engine.install_held_plans(
        eneros_control_dispatch::held_plan::DurablePlans::open(media, 0, 131_072).unwrap(),
    );
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
            4,
        ),
        resource_catalog_version: 1,
        horizon_start_ms: now_ms(),
        step_ns,
        step_count: 4,
        solver_status: SolveStatusKind::Optimal,
        objective_value: 1.0,
        relative_gap: 0.0,
        max_primal_residual: 0.0,
        elapsed_ms: 10,
        model_digest: [0xBB; 32],
        input_digest: facts.provenance_root,
        assignments: (1..=4)
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
            .submit_trusted(&plan, &input, "pcs", &trusted_telemetry(), &actor(), 5_000)
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
        .submit_trusted(
            &over,
            &input_over,
            "pcs",
            &trusted_telemetry(),
            &actor(),
            5_000,
        )
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
        .submit_trusted(
            &edge,
            &input_edge,
            "pcs",
            &trusted_telemetry(),
            &actor(),
            5_000,
        )
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
        .submit_trusted(&plan, &input, "pcs", &trusted_telemetry(), &actor(), 5_000)
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
        .submit_trusted(&plan, &input, "pcs", &trusted_telemetry(), &actor(), 5_000)
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
    let mut engine = held_fixture(media, delivered.clone());
    engine.set_input_retainer(Box::new(RecordingRetainer(log.clone())));
    let (plan, input) = held_candidate();
    let expected_id = input.facts().snapshot_id;
    engine
        .submit_trusted(&plan, &input, "pcs", &trusted_telemetry(), &actor(), 5_000)
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
    let mut engine = held_fixture(media, delivered.clone());
    engine.set_input_retainer(Box::new(RecordingRetainer(log.clone())));
    let (plan, input) = held_candidate();
    let err = engine
        .submit_trusted(&plan, &input, "pcs", &trusted_telemetry(), &actor(), 5_000)
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
        let mut engine = held_fixture(media.clone(), delivered.clone());
        // 装配顺序变体：install 先于 set_input_retainer（生产装配序）。
        engine
            .submit_trusted(&plan, &input, "pcs", &trusted_telemetry(), &actor(), 5_000)
            .unwrap();
    }
    log.borrow_mut().held.clear(); // 模拟掉电：RAM 钉集丢失
    let reopened = held_fixture(media, delivered.clone());
    assert!(reopened.held_plan_reports().is_ok(), "非终态计划在介质上");
    let mut reopened = reopened;
    reopened.set_input_retainer(Box::new(RecordingRetainer(log.clone())));
    assert_eq!(
        log.borrow().held,
        vec![expected_id],
        "重启 + 装配保留面 ⇒ 非终态计划引用的快照被重新钉住"
    );
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
            .submit_trusted(&plan, &input, "pcs", &trusted_telemetry(), &actor(), 5_000)
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
