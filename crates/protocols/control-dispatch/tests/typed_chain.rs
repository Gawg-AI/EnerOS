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
use eneros_dual_brain::gate::DecisionGate;
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
        safety_violations: 0,
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
    let profile = test_profile();
    let config = DispatchConfig {
        chain: build_chain(),
        key: KEY.to_vec(),
        profile,
        fencing_token: 1 + EPOCH_SEED,
        issuer_service: ISSUER,
        issuer_epoch: 0,
        target_endpoint: String::from("svc-device-agent"),
        max_power_kw: 500.0,
        safety_voltage_window: (340.0, 420.0),
        safety_freq_window: (49.5, 50.5),
        safety_ramp_kw_per_min: 200.0,
        gate: DecisionGate::new(100, 1000.0, 3_600_000),
        control_audit: None,
        audit: Some(DecisionAudit::new([0x40; 32])),
        intent_log: Box::new(eneros_control_dispatch::InMemoryIntentStore::new(64)),
        delivery: channel,
        resources: Box::new(support::TestResourceView {
            asset_id: 1,
            device_id: 1,
            point_id: profile.point_id,
            unit_code: eneros_telemetry_model::unit_catalog::code_of(profile.unit),
            catalog_version: 1,
        }),
        contracts: Box::new(support::test_contract_view(1, 1)),
        compiler_policy: support::test_compiler_policy(),
        control_mode: eneros_control_dispatch::arbiter::ControlMode::Auto,
        feedback_mapping: eneros_control_dispatch::FeedbackPointMapping {
            grid_up: 5,
            soc: 4,
            dc_bus_v: 1,
            overcurrent: 6,
            overtemp: 6,
        },
        now_fn: now_ms,
    };
    DispatchEngine::new(config)
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
    // （构造器校验 ttl>0），用早期 issued_at 重造。
    let expired = eneros_decision_contract::CandidatePlan::new(
        *c.id(),
        c.snapshot_id(),
        *c.objective_digest(),
        c.resource_catalog_version(),
        c.horizon_start_ms(),
        c.step_ns(),
        c.step_count(),
        c.solver_status(),
        c.objective_value(),
        c.relative_gap(),
        c.max_primal_residual(),
        c.elapsed_ms(),
        *c.model_digest(),
        *c.input_digest(),
        c.assignments().to_vec(),
        now_ms() - 120_000,
        60_000,
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
        Assignment, CandidatePlan, PlanId, SnapshotId, SolveStatusKind,
    };
    let a = |step: u32, kw: f64| Assignment {
        asset_id: 1,
        device_id: 1,
        point_id: 10,
        power_kw: kw,
        unit_code: 6,
        step_index: step,
    };
    CandidatePlan::new(
        PlanId([0x71; 16]),
        SnapshotId([2u8; 16]),
        [0xAA; 32],
        1,
        TICK_MS.load(std::sync::atomic::Ordering::Relaxed),
        10_000_000_000, // 10s 步长
        2,
        SolveStatusKind::Optimal,
        1.0,
        0.0,
        0.0,
        10,
        [0xBB; 32],
        [0xCC; 32],
        vec![a(1, 10.0), a(2, 20.0)],
        TICK_MS.load(std::sync::atomic::Ordering::Relaxed),
        60_000,
    )
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
    let profile = test_profile();
    let config = DispatchConfig {
        chain: build_chain(),
        key: KEY.to_vec(),
        profile,
        fencing_token: 1 + EPOCH_SEED,
        issuer_service: ISSUER,
        issuer_epoch: 0,
        target_endpoint: String::from("svc-device-agent"),
        max_power_kw: 500.0,
        safety_voltage_window: (340.0, 420.0),
        safety_freq_window: (49.5, 50.5),
        safety_ramp_kw_per_min: 200.0,
        gate: DecisionGate::new(100, 1000.0, 3_600_000),
        control_audit: None,
        audit: Some(DecisionAudit::new([0x40; 32])),
        intent_log: Box::new(eneros_control_dispatch::InMemoryIntentStore::new(64)),
        delivery: Box::new(channel),
        resources: Box::new(support::TestResourceView {
            asset_id: 1,
            device_id: 1,
            point_id: profile.point_id,
            unit_code: eneros_telemetry_model::unit_catalog::code_of(profile.unit),
            catalog_version: 1,
        }),
        contracts: Box::new(support::test_contract_view(1, 1)),
        compiler_policy: support::test_compiler_policy(),
        control_mode: eneros_control_dispatch::arbiter::ControlMode::Auto,
        feedback_mapping: eneros_control_dispatch::FeedbackPointMapping {
            grid_up: 5,
            soc: 4,
            dc_bus_v: 1,
            overcurrent: 6,
            overtemp: 6,
        },
        now_fn: tick_now,
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
    let mut engine = DispatchEngine::new(DispatchConfig {
        chain: build_chain(),
        key: KEY.to_vec(),
        profile,
        fencing_token: 1 + EPOCH_SEED,
        issuer_service: ISSUER,
        issuer_epoch: 0,
        target_endpoint: String::from("svc-device-agent"),
        max_power_kw: 500.0,
        safety_voltage_window: (340.0, 420.0),
        safety_freq_window: (49.5, 50.5),
        safety_ramp_kw_per_min: 200.0,
        gate: DecisionGate::new(100, 1000.0, 3_600_000),
        control_audit: None,
        audit: Some(DecisionAudit::new([0x40; 32])),
        intent_log: Box::new(store),
        delivery: Box::new(channel),
        resources: Box::new(support::TestResourceView {
            asset_id: 1,
            device_id: 1,
            point_id: profile.point_id,
            unit_code: eneros_telemetry_model::unit_catalog::code_of(profile.unit),
            catalog_version: 1,
        }),
        contracts: Box::new(support::test_contract_view(1, 1)),
        compiler_policy: support::test_compiler_policy(),
        control_mode: eneros_control_dispatch::arbiter::ControlMode::Auto,
        feedback_mapping: eneros_control_dispatch::FeedbackPointMapping {
            grid_up: 5,
            soc: 4,
            dc_bus_v: 1,
            overcurrent: 6,
            overtemp: 6,
        },
        now_fn: now_ms,
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
