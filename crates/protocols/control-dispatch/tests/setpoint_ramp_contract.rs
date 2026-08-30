//! 蓝图 61 复查 FIX（2026-08-30）：单点连续 setpoint 爬坡合同 + ControlContract。
//!
//! 判据（蓝图 61 §3.4）：
//! - ControlContract 由 ControlProfile 单源派生（值域/tag/unit/爬坡/回读/容差）；
//! - 单点 setpoint 与最近 AppliedConfirmed 比较，真实 elapsed 换算
//!   （合同 max_ramp_per_sec）；超限拒绝零投递；
//! - 首命令无参照放行；同值重确认 Δ=0 放行；时间流逝后斜率达标放行。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use eneros_control_dispatch::{
    AuthenticatedActor, ControlDispatch, DeliveryChannel, DispatchConfig, DispatchEngine,
    DispatchState, FeedbackPointMapping, InMemoryIntentStore,
};
use eneros_control_gateway::chain::ControlChain;
use eneros_control_gateway::envelope::ActorKind;
use eneros_control_gateway::lease::LeaseScope;
use eneros_control_gateway::policy::{CommandClass, Policy, PolicyRule};
use eneros_control_gateway::safety::{SafetyPoint, SafetyRole};
use eneros_control_gateway::wire::ControlEnvelope;
use eneros_dual_brain::gate::DecisionGate;
use eneros_dual_brain::{DecisionPath, ProposedControlPlan};
use eneros_safety_validator::{
    EvaluatorConfig, IntegrityStatus, RequiredPoint, RequiredPointCoverage, SampleQuality,
    StateBindings, TelemetrySample, TelemetryTrustEvaluator,
};
use eneros_telemetry_model::point_spec::PointSpec;
use eneros_telemetry_model::unit::PhysicalUnit;

const KEY: &[u8] = b"w61-dispatch-it-key-00000000000000000000";
const LEASE_ID: [u8; 16] = [0x61; 16];
const ISSUER: [u8; 16] = [0x41; 16];
const EPOCH_SEED: u64 = 7;
const DEVICE_ID: u32 = 1;
const POINT_ID: u32 = 10;

/// 共享虚拟时钟（文件内测试互斥保护——见 `LOCK`）。
static CLOCK: AtomicU64 = AtomicU64::new(1_700_000_000_000);
static LOCK: Mutex<()> = Mutex::new(());

fn now_ms() -> u64 {
    CLOCK.load(Ordering::Relaxed)
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

fn trusted_telemetry() -> eneros_safety_validator::TrustedTelemetry {
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
                    unit: eneros_safety_validator::PointUnit::Other,
                },
                RequiredPoint {
                    point: 6,
                    min: 0.0,
                    max: 1.0,
                    max_age_ms: 5000,
                    unit: eneros_safety_validator::PointUnit::Other,
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

fn proposal_with_id(plan_id: &str, power_kw: f64) -> ProposedControlPlan {
    ProposedControlPlan {
        plan_id: String::from(plan_id),
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
        safety_violations: 0,
        safety_clamped: false,
        feedback: None,
    }
}

/// in-process 通道（同 journal_crash_matrix harness；投递计数外部可观测）。
struct IngressChannel {
    ingress: Rc<RefCell<eneros_device_agent::EnvelopeIngress>>,
    devices: Rc<RefCell<eneros_device_agent::DeviceRegistry>>,
    delivered: Rc<RefCell<u64>>,
}

impl DeliveryChannel for IngressChannel {
    fn deliver(
        &mut self,
        _target: &str,
        payload: &[u8],
        _timeout_ms: u64,
    ) -> Result<Vec<u8>, eneros_control_dispatch::ChannelError> {
        *self.delivered.borrow_mut() += 1;
        let env = ControlEnvelope::decode(payload)
            .map_err(|e| eneros_control_dispatch::ChannelError::Protocol(format!("{e:?}")))?;
        if !env.verify_payload_digest() {
            return Ok(eneros_device_agent::OutcomeFrame {
                code: eneros_device_agent::CODE_DENIED,
                class: eneros_device_agent::DenyClass::Hmac.code(),
            }
            .encode()
            .to_vec());
        }
        // 实时取共享虚拟时钟（时间流逝用例：信封/ skew 判据须与引擎同钟）。
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

fn engine_with() -> (DispatchEngine, Rc<RefCell<u64>>) {
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0u64));
    let ch = IngressChannel {
        ingress: Rc::clone(&ingress),
        devices: Rc::clone(&devices),
        delivered: Rc::clone(&delivered),
    };
    let config = DispatchConfig {
        chain: build_chain(),
        key: KEY.to_vec(),
        profile: test_profile(),
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
        audit: None,
        intent_log: Box::new(InMemoryIntentStore::new(64)),
        delivery: Box::new(ch),
        feedback_mapping: FeedbackPointMapping {
            grid_up: 5,
            soc: 4,
            dc_bus_v: 1,
            overcurrent: 6,
            overtemp: 6,
        },
        now_fn: now_ms,
    };
    (DispatchEngine::new(config), delivered)
}

fn actor() -> AuthenticatedActor {
    AuthenticatedActor {
        kind: ActorKind::Ai,
        service: ISSUER,
        session_epoch: 0,
    }
}

// ============================================================================
// FIX-5：ControlContract 单源派生
// ============================================================================

#[test]
fn contract_from_pcs_profile_is_complete() {
    let c = test_profile()
        .control_contract()
        .expect("Setpoint 档案必有合同");
    assert_eq!(c.point_id, POINT_ID);
    assert_eq!(c.device_id, DEVICE_ID as u16);
    assert_eq!(c.min, -500.0);
    assert_eq!(c.max, 500.0);
    // ActivePowerKw：tag=2、unit_code=1（kW）。
    assert_eq!(c.command_tag, 2);
    assert_eq!(c.unit_code, 1);
    // 爬坡合同 = 200 kW/min 换算。
    let expected = eneros_control_gateway::PCS_SETPOINT_MAX_RAMP_PER_SEC;
    assert!((c.max_ramp_per_sec - expected).abs() < 1e-9);
    assert!((expected - 200.0 / 60.0).abs() < 1e-9);
    // 回读合同：独立回读点 + 容差单源（writer 侧同取此值）。
    assert_eq!(c.readback_point_id, POINT_ID);
    assert!((c.tolerance_abs - eneros_control_gateway::PCS_SETPOINT_READBACK_TOL_KW).abs() < 1e-9);
    assert_eq!(c.required_feedback_set_id, 1);
    assert_eq!(c.settle_timeout_ms, 3_000);
}

#[test]
fn switch_profile_contract_has_no_ramp() {
    let profile = eneros_control_gateway::ControlProfile::ai_pcc_single(1, 11, 4, LEASE_ID, 7);
    let c = profile.control_contract().expect("Switch 档案必有合同");
    assert_eq!(c.command_tag, 1); // SwitchPosition
    assert_eq!(c.unit_code, 0); // 无量纲
    assert!(c.max_ramp_per_sec.is_infinite(), "离散命令无爬坡语义");
}

#[test]
fn read_class_has_no_executable_contract() {
    let profile = eneros_control_gateway::ControlProfile {
        command_class: CommandClass::Read,
        ..eneros_control_gateway::ControlProfile::ai_pcs_setpoint(
            DEVICE_ID, POINT_ID, 4, LEASE_ID, EPOCH_SEED,
        )
    };
    assert!(profile.control_contract().is_none(), "Read 无执行合同");
}

// ============================================================================
// FIX-4：单点 setpoint 爬坡（vs 最近 AppliedConfirmed，真实 elapsed）
// ============================================================================

#[test]
fn first_command_has_no_baseline_and_commits() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut engine, delivered) = engine_with();
    let tel = trusted_telemetry();
    let r = engine
        .submit(&proposal_with_id("plan-A", 5.0), &tel, &actor(), 5000)
        .expect("首命令无爬坡参照 → 放行");
    assert_eq!(r.final_state, DispatchState::Committed);
    assert_eq!(*delivered.borrow(), 1);
}

#[test]
fn ramp_violation_rejected_zero_delivery() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut engine, delivered) = engine_with();
    let tel = trusted_telemetry();
    let _ = engine
        .submit(&proposal_with_id("plan-A", 5.0), &tel, &actor(), 5000)
        .expect("基线 5 kW Committed");
    let before = *delivered.borrow();
    // 同拍 5→200：|Δ|=195 kW，dt 下限 1s → 195 kW/s ≫ 3.33 kW/s 合同限。
    let r = engine.submit(&proposal_with_id("plan-B", 200.0), &tel, &actor(), 5000);
    assert!(
        matches!(
            &r,
            Err(eneros_control_dispatch::DispatchError::SafetyRejected(note))
                if note.contains("ramp-limit")
        ),
        "超爬坡必须 SafetyRejected 且文案可观测：{r:?}"
    );
    assert_eq!(*delivered.borrow(), before, "超爬坡命令必须零投递");
}

#[test]
fn same_setpoint_delta_zero_recommits() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut engine, delivered) = engine_with();
    let tel = trusted_telemetry();
    let _ = engine
        .submit(&proposal_with_id("plan-A", 5.0), &tel, &actor(), 5000)
        .expect("基线 5 kW Committed");
    // 不同 key、同值：Δ=0 → 爬坡检查通过（E4 周期性同值 setpoint 场景）。
    let r = engine
        .submit(&proposal_with_id("plan-B", 5.0), &tel, &actor(), 5000)
        .expect("同值重确认 Δ=0 放行");
    assert_eq!(r.final_state, DispatchState::Committed);
    assert_eq!(*delivered.borrow(), 2);
}

#[test]
fn ramp_ok_after_real_time_passes() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut engine, delivered) = engine_with();
    let tel = trusted_telemetry();
    let _ = engine
        .submit(&proposal_with_id("plan-A", 5.0), &tel, &actor(), 5000)
        .expect("基线 5 kW Committed");
    // 真实时钟前进 59 s（< lease/intent TTL）：15 kW / 59 s ≈ 0.254 kW/s
    // < 3.33 kW/s 合同限 → 放行。遥测按新时刻重建（新鲜度判据独立）。
    CLOCK.fetch_add(59_000, Ordering::Relaxed);
    let tel = trusted_telemetry();
    let r = engine
        .submit(&proposal_with_id("plan-B", 20.0), &tel, &actor(), 5000)
        .expect("时间流逝后斜率达标放行");
    assert_eq!(r.final_state, DispatchState::Committed);
    assert_eq!(*delivered.borrow(), 2);
}
