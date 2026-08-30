//! 蓝图 61 v0.314.24（61.24-a）六红测 + 幂等属性：执行状态与幂等真值。
//!
//! 唯一不变量：`Committed == AppliedConfirmed == 独立设备回读在容差内
//! 匹配已授权命令`。幂等命中必须返回真实原状态，不得把 InFlight/
//! Indeterminate/Rejected 升级为 Committed。
//!
//! 覆盖（蓝图 05 §2.4/§2.5）：
//! - W1 投递前崩溃 → 重启重放：零投递、非 Committed；
//! - W2 投递后 journal 持久化失败 → Indeterminate、重启重放零投递；
//! - W3 ACK 后读回前崩溃 → 重启重放：零投递、非 Committed；
//! - R4 Indeterminate 重放 → 仍 Indeterminate，不盲发不升级；
//! - R5 同 key 不同 digest → 冲突拒绝，零投递；
//! - R6 DryRun → 永不写 Applied、重放不 Committed；
//! - 属性：同 key 100 次重放 = 一次设备副作用 + 原结果。
mod support;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use eneros_control_dispatch::durable_intent::DurableIntentStore;
use eneros_control_dispatch::{
    AuthenticatedActor, ControlDispatch, DeliveryChannel, DispatchConfig, DispatchEngine,
    DispatchState, FeedbackPointMapping, InMemoryIntentStore, IntentRecord, IntentStore,
    IntentStoreError,
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
use eneros_durable_state::{PersistIo, StoreError};
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

/// 带DryRun 义务的策略（R6 用）。
fn build_chain_dryrun() -> ControlChain {
    let profile = test_profile();
    let mut chain = ControlChain::from_profile(KEY, &profile);
    let _ = profile.issue_lease(&mut chain, now_ms() / 1000);
    let policy = chain.policy_mut();
    let _ = policy.replace_rules(vec![PolicyRule {
        actor_kind: Some(ActorKind::Ai),
        devices: vec![1],
        points: vec![10],
        kinds: vec![CommandClass::Setpoint],
        time_window: (0, 0),
        obligations: vec![eneros_control_gateway::policy::Obligation::DryRun],
    }]);
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
        good(5, 1.0, 1), // grid_up
        good(6, 0.0, 1), // overcurrent（overtemp 同点别名——装配现状）
    ];
    ev.evaluate(&samples, now).expect("trusted telemetry")
}

fn proposal(power_kw: f64) -> ProposedControlPlan {
    proposal_with_id("req-1700000000000-1", power_kw)
}

/// 同 plan_id 不同载荷 = 同幂等键不同内容（R5 冲突用例）。
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

/// in-process 通道（同 dispatch_ingress_chain harness；投递计数外部可观测）。
struct IngressChannel {
    ingress: Rc<RefCell<eneros_device_agent::EnvelopeIngress>>,
    devices: Rc<RefCell<eneros_device_agent::DeviceRegistry>>,
    now_ms: u64,
    delivered: Rc<RefCell<u64>>,
    fail: Option<eneros_control_dispatch::ChannelError>,
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
        // 直接转发 V2 ControlEnvelope 字节（61 v0.314.25 主路径）。
        let out =
            self.ingress
                .borrow_mut()
                .process(&mut self.devices.borrow_mut(), payload, self.now_ms);
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

// ============================================================================
// durable journal 介质（同 durable_intent 测试的 SharedMemIo 模式）
// ============================================================================

struct SharedMemIo {
    media: Rc<RefCell<Vec<u8>>>,
    capacity: u64,
}

impl SharedMemIo {
    fn new(media: Rc<RefCell<Vec<u8>>>, capacity: u64) -> Self {
        let mut m = media.borrow_mut();
        if m.len() < capacity as usize {
            m.resize(capacity as usize, 0u8);
        }
        drop(m);
        Self { media, capacity }
    }
}

impl PersistIo for SharedMemIo {
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), StoreError> {
        let m = self.media.borrow();
        let start = offset as usize;
        if start >= m.len() {
            buf.fill(0);
            return Ok(());
        }
        let n = m.len().saturating_sub(start).min(buf.len());
        buf[..n].copy_from_slice(&m[start..start + n]);
        for b in buf[n..].iter_mut() {
            *b = 0;
        }
        Ok(())
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), StoreError> {
        let mut m = self.media.borrow_mut();
        let start = offset as usize;
        let end = start + data.len();
        if end as u64 > self.capacity {
            return Err(StoreError::WriteFailed);
        }
        if m.len() < end {
            m.resize(end, 0u8);
        }
        m[start..end].copy_from_slice(data);
        Ok(())
    }
    fn flush(&mut self) -> Result<(), StoreError> {
        Ok(())
    }
    fn capacity(&self) -> u64 {
        self.capacity
    }
}

fn durable_store(media: Rc<RefCell<Vec<u8>>>) -> DurableIntentStore<SharedMemIo> {
    let mut s = DurableIntentStore::new(SharedMemIo::new(media, 1_000_000), 0, 512, 64);
    s.open().expect("open");
    s
}

/// 注入 update_state 失败的 IntentStore 包装（W2：投递后 journal 持久化失败）。
struct FailingUpdateStore<S> {
    inner: S,
    fail_update_state: Cell<bool>,
}

impl<S: IntentStore> IntentStore for FailingUpdateStore<S> {
    fn append(&mut self, rec: &IntentRecord) -> Result<(), IntentStoreError> {
        self.inner.append(rec)
    }
    fn update_state(
        &mut self,
        command_id: &[u8; 16],
        state: DispatchState,
    ) -> Result<(), IntentStoreError> {
        if self.fail_update_state.get() {
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

fn engine_with(
    chain: ControlChain,
    channel: Box<dyn DeliveryChannel>,
    intent_log: Box<dyn IntentStore>,
) -> DispatchEngine {
    let profile = test_profile();
    let config = DispatchConfig {
        chain,
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
        audit: Some(DecisionAudit::new([0x61; 32])),
        intent_log,
        delivery: channel,
        resources: Box::new(support::TestResourceView {
            asset_id: u64::from(profile.device_id),
            device_id: profile.device_id as u16,
            point_id: profile.point_id,
            unit_code: eneros_telemetry_model::unit_catalog::code_of(profile.unit),
            catalog_version: 1,
        }),
        feedback_mapping: FeedbackPointMapping {
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

fn channel_for(
    ingress: &Rc<RefCell<eneros_device_agent::EnvelopeIngress>>,
    devices: &Rc<RefCell<eneros_device_agent::DeviceRegistry>>,
    delivered: &Rc<RefCell<u64>>,
) -> IngressChannel {
    IngressChannel {
        ingress: Rc::clone(ingress),
        devices: Rc::clone(devices),
        now_ms: now_ms(),
        delivered: Rc::clone(delivered),
        fail: None,
    }
}

fn actor() -> AuthenticatedActor {
    AuthenticatedActor {
        kind: ActorKind::Ai,
        service: ISSUER,
        session_epoch: 0,
    }
}

/// journal key = correlation = plan_id 前 16B（引擎 `correlation_of` 同源）。
fn plan_correlation(plan_id: &str) -> [u8; 16] {
    let mut id = [0u8; 16];
    let b = plan_id.as_bytes();
    let n = core::cmp::min(16, b.len());
    id[..n].copy_from_slice(&b[..n]);
    id
}

// ============================================================================
// W1：投递前崩溃（durable 留下 Persisted 记录）→ 重启重放零投递、非 Committed
// ============================================================================

#[test]
fn w1_crash_after_persist_replay_never_delivers_nor_commits() {
    let media = Rc::new(RefCell::new(Vec::new()));
    // 「崩溃后」世界：durable journal 里只有一条 Persisted 意图
    //（进程在步骤 6 后、投递前死亡——投递未发生）。
    {
        let mut store = durable_store(Rc::clone(&media));
        store
            .append(&IntentRecord {
                command_id: [0x11; 16],
                correlation_id: plan_correlation("req-1700000000000-1"),
                issued_at_ms: now_ms(),
                expires_at_ms: now_ms() + 60_000,
                target_device: String::from("pcs"),
                power_kw: 5.0,
                input_digest: None,
                state: DispatchState::Persisted,
                command_digest: None,
                attempt: 1,
                receipt_digest: None,
                plan_sha256: None,
            })
            .expect("seed persisted intent");
    }
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0u64));
    let ch = channel_for(&ingress, &devices, &delivered);
    let mut engine = engine_with(
        build_chain(),
        Box::new(ch),
        Box::new(durable_store(Rc::clone(&media))),
    );
    let tel = trusted_telemetry();
    // 同幂等键重放（plan_id 相同 → correlation 相同）。
    let r = engine
        .submit(&proposal(5.0), &tel, &actor(), 5000)
        .expect("replay must not hard-fail");
    assert_ne!(
        r.final_state,
        DispatchState::Committed,
        "投递前崩溃的重放绝不能 Committed：{r:?}"
    );
    assert_eq!(
        *delivered.borrow(),
        0,
        "in-flight 重放必须零投递（对账，不盲发）"
    );
}

// ============================================================================
// W2：投递后 journal 持久化失败 → Indeterminate；重启重放零投递、非 Committed
// ============================================================================

#[test]
fn w2_post_delivery_journal_failure_is_indeterminate() {
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0u64));
    let ch = channel_for(&ingress, &devices, &delivered);
    let media = Rc::new(RefCell::new(Vec::new()));
    let store = FailingUpdateStore {
        inner: durable_store(Rc::clone(&media)),
        fail_update_state: Cell::new(true),
    };
    let mut engine = engine_with(build_chain(), Box::new(ch), Box::new(store));
    let tel = trusted_telemetry();
    let r1 = engine
        .submit(&proposal(5.0), &tel, &actor(), 5000)
        .expect("submit");
    assert_ne!(
        r1.final_state,
        DispatchState::Committed,
        "投递后 journal 持久化失败不得报告 Committed（无法证明已确认）：{r1:?}"
    );
    assert_eq!(*delivered.borrow(), 1, "投递确实发生了（恰好一次）");

    // 重启（同介质、无注入失败）：重放同 key → 零投递、非 Committed。
    let ch2 = channel_for(&ingress, &devices, &delivered);
    let mut engine2 = engine_with(
        build_chain(),
        Box::new(ch2),
        Box::new(durable_store(Rc::clone(&media))),
    );
    let r2 = engine2
        .submit(&proposal(5.0), &tel, &actor(), 5000)
        .expect("replay");
    assert_ne!(r2.final_state, DispatchState::Committed, "{r2:?}");
    assert_eq!(*delivered.borrow(), 1, "重启重放不得再次投递");
}

// ============================================================================
// W3：ACK 后读回前崩溃（durable 留下 Sent/Indeterminate）→ 重启重放非 Committed
// ============================================================================

#[test]
fn w3_crash_after_ack_before_readback_never_commits() {
    let media = Rc::new(RefCell::new(Vec::new()));
    // 「崩溃后」世界：mark_sent 已落（投递可能已发生、读回未确认）。
    {
        let mut store = durable_store(Rc::clone(&media));
        let cmd_id = [0x33; 16];
        store
            .append(&IntentRecord {
                command_id: cmd_id,
                correlation_id: plan_correlation("req-1700000000000-1"),
                issued_at_ms: now_ms(),
                expires_at_ms: now_ms() + 60_000,
                target_device: String::from("pcs"),
                power_kw: 5.0,
                input_digest: None,
                state: DispatchState::Persisted,
                command_digest: None,
                attempt: 1,
                receipt_digest: None,
                plan_sha256: None,
            })
            .expect("seed");
        store
            .update_state(&cmd_id, DispatchState::Indeterminate)
            .expect("mark sent");
    }
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0u64));
    let ch = channel_for(&ingress, &devices, &delivered);
    let mut engine = engine_with(
        build_chain(),
        Box::new(ch),
        Box::new(durable_store(Rc::clone(&media))),
    );
    let tel = trusted_telemetry();
    let r = engine
        .submit(&proposal(5.0), &tel, &actor(), 5000)
        .expect("replay");
    assert_ne!(
        r.final_state,
        DispatchState::Committed,
        "ACK 后读回前崩溃：未确认就是未确认：{r:?}"
    );
    assert_eq!(*delivered.borrow(), 0, "重放零投递");
}

// ============================================================================
// R4：Indeterminate 重放 → 仍 Indeterminate，不升级、不盲发
// ============================================================================

#[test]
fn r4_indeterminate_replay_stays_indeterminate() {
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0u64));
    let mut ch = channel_for(&ingress, &devices, &delivered);
    ch.fail = Some(eneros_control_dispatch::ChannelError::Timeout);
    let mut engine = engine_with(
        build_chain(),
        Box::new(ch),
        Box::new(InMemoryIntentStore::new(64)),
    );
    let tel = trusted_telemetry();
    let r1 = engine
        .submit(&proposal(5.0), &tel, &actor(), 5000)
        .expect("submit");
    assert_eq!(r1.final_state, DispatchState::Indeterminate);
    // 同 key 重放（同进程，链缓存命中 Authorized）。
    let r2 = engine
        .submit(&proposal(5.0), &tel, &actor(), 5000)
        .expect("replay");
    assert_eq!(
        r2.final_state,
        DispatchState::Indeterminate,
        "Indeterminate 重放必须保持 Indeterminate（对账），不得升级 Committed：{r2:?}"
    );
}

// ============================================================================
// R5：同 key 不同 digest → 幂等冲突，零投递
// ============================================================================

#[test]
fn r5_same_key_different_digest_is_conflict() {
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0u64));
    let ch = channel_for(&ingress, &devices, &delivered);
    let mut engine = engine_with(
        build_chain(),
        Box::new(ch),
        Box::new(InMemoryIntentStore::new(64)),
    );
    let tel = trusted_telemetry();
    let r1 = engine
        .submit(&proposal_with_id("plan-A", 5.0), &tel, &actor(), 5000)
        .expect("first");
    let _ = r1;
    let before = *delivered.borrow();
    // 同 plan_id（同幂等键）不同功率 → 冲突：拒绝且零投递。
    let r2 = engine.submit(&proposal_with_id("plan-A", 60.0), &tel, &actor(), 5000);
    assert!(r2.is_err(), "同 key 不同 digest 必须拒绝：{r2:?}");
    assert_eq!(*delivered.borrow(), before, "冲突命令必须零投递");
}

// ============================================================================
// R6：DryRun 永不写 Applied；DryRun 重放不 Committed、零投递
// ============================================================================

#[test]
fn r6_dryrun_never_commits_even_on_replay() {
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0u64));
    let ch = channel_for(&ingress, &devices, &delivered);
    let mut engine = engine_with(
        build_chain_dryrun(),
        Box::new(ch),
        Box::new(InMemoryIntentStore::new(64)),
    );
    let tel = trusted_telemetry();
    let r1 = engine.submit(&proposal(5.0), &tel, &actor(), 5000);
    assert!(r1.is_err(), "DryRun 义务必须拦截执行");
    assert_eq!(*delivered.borrow(), 0, "DryRun 零投递");
    // 重放（链缓存命中）：仍不得 Committed/投递。
    let r2 = engine.submit(&proposal(5.0), &tel, &actor(), 5000);
    assert!(
        r2.is_err(),
        "DryRun 重放不得升级为 Committed：{:?}",
        r2.ok()
    );
    assert_eq!(*delivered.borrow(), 0, "DryRun 重放零投递");
}

// ============================================================================
// R7：投递后通道协议错误 = 结果不明（无法证明未写）→ Indeterminate，不假失败
// ============================================================================

#[test]
fn r7_channel_protocol_error_after_deliver_is_indeterminate() {
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0u64));
    let media = Rc::new(RefCell::new(Vec::new()));
    let mut ch = channel_for(&ingress, &devices, &delivered);
    // 通道在信封已送出后回执丢失（协议违例）——写是否发生不可证明。
    ch.fail = Some(eneros_control_dispatch::ChannelError::Protocol(
        String::from("chunked: response lost"),
    ));
    let mut engine = engine_with(
        build_chain(),
        Box::new(ch),
        Box::new(durable_store(Rc::clone(&media))),
    );
    let tel = trusted_telemetry();
    let r1 = engine
        .submit(&proposal(5.0), &tel, &actor(), 5000)
        .expect("投递后通道错误不得表现为前置 Err（会误导为未投递）");
    assert_eq!(
        r1.final_state,
        DispatchState::Indeterminate,
        "无法证明未写 = Indeterminate：{r1:?}"
    );
    // 「重启」后重放（durable 恢复 + journal 播种）：不得盲发。
    let delivered_before = *delivered.borrow();
    let ch2 = channel_for(&ingress, &devices, &delivered);
    let mut engine2 = engine_with(
        build_chain(),
        Box::new(ch2),
        Box::new(durable_store(Rc::clone(&media))),
    );
    let r2 = engine2
        .submit(&proposal(5.0), &tel, &actor(), 5000)
        .expect("replay");
    assert_ne!(r2.final_state, DispatchState::Committed, "{r2:?}");
    assert_eq!(
        *delivered.borrow(),
        delivered_before,
        "结果不明后的重放零投递"
    );
}

// ============================================================================
// 属性：同 key 100 次重放 = 一次设备副作用 + 原结果（Committed）
// ============================================================================

#[test]
fn property_replay_100x_single_side_effect() {
    let (ingress, devices) = device_side();
    let delivered = Rc::new(RefCell::new(0u64));
    let ch = channel_for(&ingress, &devices, &delivered);
    let mut engine = engine_with(
        build_chain(),
        Box::new(ch),
        Box::new(InMemoryIntentStore::new(64)),
    );
    let tel = trusted_telemetry();
    let r1 = engine
        .submit(&proposal(5.0), &tel, &actor(), 5000)
        .expect("first");
    assert_eq!(r1.final_state, DispatchState::Committed);
    for i in 0..100 {
        let r = engine
            .submit(&proposal(5.0), &tel, &actor(), 5000)
            .expect("replay must succeed");
        assert_eq!(r.final_state, DispatchState::Committed, "replay {i}: {r:?}");
    }
    assert_eq!(*delivered.borrow(), 1, "100 次重放只允许一次设备副作用");
}
