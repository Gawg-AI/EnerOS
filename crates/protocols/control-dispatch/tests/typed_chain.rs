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
