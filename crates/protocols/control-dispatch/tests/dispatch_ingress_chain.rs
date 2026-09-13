//! 蓝图 40 WP-40.6/40.7/40.11/40.12 集成测试：唯一 ControlDispatch 全链
//! （DispatchEngine → in-process DeliveryChannel → device-agent
//! EnvelopeIngress → AuthorizedWriter → 独立 readback → 回执 → Committed）。
//!
//! 覆盖：正常正例（Committed + 一次落写 + 预算记账 + intent 更新）、
//! 读回不符（Failed）、读回不可得（Indeterminate）、投递超时（Indeterminate）、
//! 不可达（Err 零投递）、预算超限（Err 零投递）、遥测过期（Err）、
//! 篡改拒绝（Failed）、幂等重放（Committed 不重复写）。
mod support;

use std::cell::RefCell;
use std::rc::Rc;

use eneros_control_dispatch::{
    AuthenticatedActor, ControlDispatch, DeliveryChannel, DispatchConfig, DispatchEngine,
    DispatchError, DispatchState, InMemoryIntentStore,
};
use eneros_control_gateway::chain::ControlChain;
use eneros_control_gateway::envelope::ActorKind;
use eneros_control_gateway::lease::LeaseScope;
use eneros_control_gateway::policy::{CommandClass, Policy, PolicyRule};
use eneros_control_gateway::safety::{SafetyPoint, SafetyRole};
use eneros_control_gateway::wire::ControlEnvelope;
use eneros_dual_brain::audit::DecisionAudit;
use eneros_dual_brain::gate::DecisionGate;
use eneros_dual_brain::{DecisionPath, ProposedControlPlan};
use eneros_safety_validator::{
    EvaluatorConfig, IntegrityStatus, RequiredPoint, RequiredPointCoverage, SampleQuality,
    StateBindings, TelemetrySample, TelemetryTrustEvaluator,
};
use eneros_telemetry_model::point_spec::PointSpec;
use eneros_telemetry_model::unit::PhysicalUnit;

const KEY: &[u8] = b"w40-dispatch-it-key-00000000000000000000";
const LEASE_ID: [u8; 16] = [0x40; 16];
const ISSUER: [u8; 16] = [0x41; 16];
const EPOCH_SEED: u64 = 7;
const DEVICE_ID: u32 = 1;
const POINT_ID: u32 = 10;

fn now_ms() -> u64 {
    1_700_000_000_000
}

/// 测试同源档案（蓝图 55：TTL 600/代次 4 为测试变体，值语义与旧手抄块一致）。
fn test_profile() -> eneros_control_gateway::ControlProfile {
    eneros_control_gateway::ControlProfile {
        lease_ttl_secs: 600,
        ..eneros_control_gateway::ControlProfile::ai_pcs_setpoint(
            DEVICE_ID, POINT_ID, 4, LEASE_ID, EPOCH_SEED,
        )
    }
}

/// 装配放行链（Ai → dev1/pt10）+ 租约（与 device-agent 侧同 seed/scope）。
fn build_chain() -> ControlChain {
    let profile = test_profile();
    let mut chain = ControlChain::from_profile(KEY, &profile);
    let _ = profile.issue_lease(&mut chain, now_ms() / 1000);
    chain
}

/// 受信任遥测（8 点全覆盖，含电流/频率——评估器构造）。
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
        good(5, 1.0, 1), // grid_up
        good(6, 0.0, 1), // overcurrent
    ];
    ev.evaluate(&samples, now).expect("trusted telemetry")
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

/// in-process 通道：模拟 device-agent 收口点（唯一真实路径——ingress）。
struct IngressChannel {
    ingress: Rc<RefCell<eneros_device_agent::EnvelopeIngress>>,
    devices: Rc<RefCell<eneros_device_agent::DeviceRegistry>>,
    now_ms: u64,
    /// 投递计数（外部可观测副作用）。
    delivered: Rc<RefCell<u64>>,
    /// 注入的通道失败（None = 正常）。
    fail: Option<eneros_control_dispatch::ChannelError>,
    /// 注入的载荷篡改（测试用）。
    tamper: bool,
}

impl DeliveryChannel for IngressChannel {
    fn deliver(
        &mut self,
        _target: &str,
        payload: &[u8],
        _timeout_ms: u64,
    ) -> Result<Vec<u8>, eneros_control_dispatch::ChannelError> {
        if let Some(e) = &self.fail {
            return Err(e.clone());
        }
        *self.delivered.borrow_mut() += 1;
        let mut payload = payload.to_vec();
        if self.tamper {
            // 篡改信封尾部字节（V2：尾部在内层信封域——摘要可检出）。
            let n = payload.len();
            payload[n - 1] ^= 0xFF;
        }
        // 解码 ControlEnvelope → 提取内层信封 + 功率 → ingress 处理。
        let env = ControlEnvelope::decode(&payload)
            .map_err(|e| eneros_control_dispatch::ChannelError::Protocol(format!("{e:?}")))?;
        if !env.verify_payload_digest() {
            // 篡改被 payload 摘要检出——返回 Denied 语义（写未发生）。
            return Ok(eneros_device_agent::OutcomeFrame {
                code: eneros_device_agent::CODE_DENIED,
                class: eneros_device_agent::DenyClass::Hmac.code(),
            }
            .encode()
            .to_vec());
        }
        // 直接转发 V2 ControlEnvelope 字节（61 v0.314.25：无旧格式回包——
        // ingress 主路径就是 ControlEnvelope 解码）。
        let out = self.ingress.borrow_mut().process(
            &mut self.devices.borrow_mut(),
            &payload,
            self.now_ms,
        );
        Ok(eneros_device_agent::outcome_to_frame(&out)
            .encode()
            .to_vec())
    }
}

/// device-agent 侧装配（MockPointDevice + 遥测点 + 控制点）。
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
    // 设备：遥测点（反馈用）+ 控制点 10。
    let mut device = eneros_device_agent::MockPointDevice::new(1)
        .with_point("grid_up", 1.0)
        .with_point("soc_pct", 60.0)
        .with_point("dc_bus_v", 600.0)
        .with_point("overcurrent", 0.0)
        .with_point("overtemp", 0.0)
        .with_point_at("power", 0.0, 10);
    // 反馈点时间戳新鲜（CTL-010 freshness——否则 reconcile 判 Stale 拒写）。
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

fn engine_with(channel: Box<dyn DeliveryChannel>, gate: DecisionGate) -> DispatchEngine {
    let config = support::test_config_legacy(support::TestConfigSeed {
        chain: build_chain(),
        key: KEY.to_vec(),
        profile: test_profile(),
        issuer_service: ISSUER,
        fencing_token: 1 + EPOCH_SEED,
        now_fn: now_ms,
        intent_log: Box::new(InMemoryIntentStore::new(64)),
        delivery: channel,
    });
    DispatchEngine::new(DispatchConfig {
        gate,
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

/// 正常正例：可信遥测 + 有效 proposal → 一次落写 + 独立读回 + Committed。
#[test]
fn normal_chain_commits_with_readback() {
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0u64));
    let channel = IngressChannel {
        ingress,
        devices,
        now_ms: now_ms(),
        delivered: delivered.clone(),
        fail: None,
        tamper: false,
    };
    let mut engine = engine_with(Box::new(channel), DecisionGate::new(100, 1000.0, 3_600_000));
    let tel = trusted_telemetry();
    let receipt = engine
        .submit(&proposal(5.0), &tel, &actor(), 5000)
        .expect("submit");
    assert_eq!(
        receipt.final_state,
        DispatchState::Committed,
        "{:?}",
        receipt
    );
    assert!(receipt.committed_at_ms.is_some());
    assert_eq!(*delivered.borrow(), 1, "必须恰好投递一次");
    assert_eq!(engine.committed_count(), 1);
    // confirmed dispatch 才记账（WP-40.3：record 移到 dispatch）。
    assert!(engine.gate_used_kwh() > 0.0, "Committed 后必须记账");
    // intent 终态更新为 Committed。
    let intent = engine
        .find_intent(&receipt.command_id)
        .expect("intent 已持久化");
    assert_eq!(intent.state, DispatchState::Committed);
    assert!(engine.pending_intents().is_empty(), "无未决意图");
    // 审计入链。
    let audit = engine.audit_log().expect("audit");
    assert_eq!(audit.chain_state().entry_count, 1);
    assert!(audit.verify().expect("verify").valid);
    // 状态路径完整（Proposed → … → Committed）。
    assert!(receipt
        .state_path
        .contains(&DispatchState::ReadbackVerified));
    assert!(receipt.state_path.contains(&DispatchState::Persisted));
}

/// 负例：投递超时 → Indeterminate（写可能已发生，不猜成功），intent 未决。
#[test]
fn delivery_timeout_is_indeterminate() {
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0u64));
    let delivered_obs = delivered.clone();
    let mut channel = IngressChannel {
        ingress,
        devices,
        now_ms: now_ms(),
        delivered,
        fail: None,
        tamper: false,
    };
    channel.fail = Some(eneros_control_dispatch::ChannelError::Timeout);
    let _delivered = delivered_obs;
    let mut engine = engine_with(Box::new(channel), DecisionGate::new(100, 1000.0, 3_600_000));
    let tel = trusted_telemetry();
    let receipt = engine
        .submit(&proposal(5.0), &tel, &actor(), 5000)
        .expect("submit");
    assert_eq!(receipt.final_state, DispatchState::Indeterminate);
    assert!(receipt.committed_at_ms.is_none());
    assert!(
        !engine.pending_intents().is_empty(),
        "Indeterminate 意图必须未决（对账）"
    );
    assert_eq!(engine.gate_used_kwh(), 0.0, "未确认不得记账");
}

/// 负例：通道不可达 → Err（前置失败零投递）。
#[test]
fn delivery_unreachable_is_err_zero_delivery() {
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0u64));
    let delivered_obs = delivered.clone();
    let mut channel = IngressChannel {
        ingress,
        devices,
        now_ms: now_ms(),
        delivered,
        fail: None,
        tamper: false,
    };
    channel.fail = Some(eneros_control_dispatch::ChannelError::Unreachable);
    let _delivered = delivered_obs;
    let mut engine = engine_with(Box::new(channel), DecisionGate::new(100, 1000.0, 3_600_000));
    let tel = trusted_telemetry();
    let err = engine
        .submit(&proposal(5.0), &tel, &actor(), 5000)
        .expect_err("unreachable");
    assert!(matches!(err, DispatchError::DeliveryUnreachable));
    assert_eq!(engine.committed_count(), 0);
}

/// 负例：预算超限 → Err 零投递（拒绝不消耗成功预算）。
#[test]
fn budget_exceeded_is_err_zero_delivery() {
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0u64));
    let delivered_obs = delivered.clone();
    let channel = IngressChannel {
        ingress,
        devices,
        now_ms: now_ms(),
        delivered,
        fail: None,
        tamper: false,
    };
    let mut engine = engine_with(Box::new(channel), DecisionGate::new(100, 0.0, 3_600_000));
    let delivered = delivered_obs;
    let tel = trusted_telemetry();
    let err = engine
        .submit(&proposal(5.0), &tel, &actor(), 5000)
        .expect_err("budget");
    assert!(matches!(err, DispatchError::BudgetExceeded(_)));
    assert_eq!(*delivered.borrow(), 0, "预算拒绝必须零投递");
    assert_eq!(
        engine.gate_rejected_energy(),
        1,
        "拒绝计数（失败预算另行统计）"
    );
}

/// 负例：遥测过期（控制时域外）→ Err 零投递。
#[test]
fn expired_telemetry_is_err() {
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0u64));
    let delivered_obs = delivered.clone();
    let channel = IngressChannel {
        ingress,
        devices,
        now_ms: now_ms(),
        delivered,
        fail: None,
        tamper: false,
    };
    let mut engine = engine_with(Box::new(channel), DecisionGate::new(100, 1000.0, 3_600_000));
    let delivered = delivered_obs;
    let mut tel = trusted_telemetry();
    tel.valid_until_ms = now_ms() - 1; // 已过控制时域
    let err = engine
        .submit(&proposal(5.0), &tel, &actor(), 5000)
        .expect_err("stale");
    assert!(matches!(err, DispatchError::TelemetryExpired));
    assert_eq!(*delivered.borrow(), 0);
}

/// 负例：功率越限 → Err 零投递。
#[test]
fn proposal_out_of_range_is_err() {
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0u64));
    let delivered_obs = delivered.clone();
    let channel = IngressChannel {
        ingress,
        devices,
        now_ms: now_ms(),
        delivered,
        fail: None,
        tamper: false,
    };
    let mut engine = engine_with(Box::new(channel), DecisionGate::new(100, 1000.0, 3_600_000));
    let delivered = delivered_obs;
    let tel = trusted_telemetry();
    let err = engine
        .submit(&proposal(9999.0), &tel, &actor(), 5000)
        .expect_err("power limit");
    assert!(matches!(err, DispatchError::ProposalInvalid(_)));
    assert_eq!(*delivered.borrow(), 0);
}

/// 负例：载荷篡改 → device-agent 拒绝（payload 摘要检出）→ Failed。
#[test]
fn tampered_payload_fails_closed() {
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0u64));
    let delivered_obs = delivered.clone();
    let channel = IngressChannel {
        ingress,
        devices,
        now_ms: now_ms(),
        delivered,
        fail: None,
        tamper: true,
    };
    let _delivered = delivered_obs;
    let mut engine = engine_with(Box::new(channel), DecisionGate::new(100, 1000.0, 3_600_000));
    let tel = trusted_telemetry();
    let receipt = engine
        .submit(&proposal(5.0), &tel, &actor(), 5000)
        .expect("submit");
    assert_eq!(receipt.final_state, DispatchState::Failed, "{:?}", receipt);
    assert!(receipt
        .note
        .as_deref()
        .unwrap_or("")
        .contains("device-denied"));
    assert_eq!(engine.committed_count(), 0, "篡改不得 Committed");
}

/// 幂等：同 command_id 重放 → 链幂等命中 → Committed 且不重复投递。
#[test]
fn idempotent_retransmit_does_not_rewrite() {
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0u64));
    let delivered_obs = delivered.clone();
    let channel = IngressChannel {
        ingress,
        devices,
        now_ms: now_ms(),
        delivered,
        fail: None,
        tamper: false,
    };
    let mut engine = engine_with(Box::new(channel), DecisionGate::new(100, 1000.0, 3_600_000));
    let delivered = delivered_obs;
    let tel = trusted_telemetry();
    let r1 = engine
        .submit(&proposal(5.0), &tel, &actor(), 5000)
        .expect("first");
    assert_eq!(r1.final_state, DispatchState::Committed);
    // 同 proposal（plan_id 同 → correlation 同 → idempotency key 同）。
    let r2 = engine
        .submit(&proposal(5.0), &tel, &actor(), 5000)
        .expect("second");
    assert_eq!(
        r2.final_state,
        DispatchState::Committed,
        "幂等重放返回原结果"
    );
    assert!(r2.note.as_deref().unwrap_or("").contains("idempotent"));
    assert_eq!(*delivered.borrow(), 1, "幂等不得重复投递");
}
