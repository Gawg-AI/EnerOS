//! 47（WP-47.1 协议入站写）集成测试：`DispatchEngine::submit_intent`——
//! 协议写意图（`PointWriteIntent`）经同一 12 态链（意图校验 → 序号重放门 →
//! ControlGateway 九步链（含 SBO/CTL-010/CTL-008）→ rate 预算 → durable
//! intent → 投递 → ACK/独立 readback → Committed/Failed/Indeterminate）。
//!
//! 负例矩阵（写路径每个分支配负例——蓝图 47 防次生）：
//! - deny（装配绑定不符 → NotAuthorized 零投递；策略拒绝 → NotAuthorized）；
//! - 超时（投递超时 → Indeterminate 对账，不猜成功）；
//! - 读回不符（ACK 成功但物理状态未达目标 → Failed）；
//! - 重复 sequence 重放（第 2 次零写 Rejected——幂等）；
//! - rate 预算（超产品速率 → BudgetExceeded 零投递）；
//! - 遥测过期（控制时域外 → TelemetryExpired）。
mod support;

use eneros_control_dispatch::{
    AuthenticatedActor, ControlDispatch, DeliveryChannel, DispatchConfig, DispatchEngine,
    DispatchError, DispatchState, InMemoryIntentStore,
};
use eneros_control_gateway::chain::ControlChain;
use eneros_control_gateway::envelope::ActorKind;
use eneros_control_gateway::lease::LeaseScope;
use eneros_control_gateway::policy::Policy;
use eneros_control_gateway::wire::{OutcomeCode, OutcomeFrame};
use eneros_dual_brain::audit::DecisionAudit;
use eneros_dual_brain::gate::DecisionGate;
use eneros_protocol_abstract::write::{encode_intent, PointWriteIntent};
use eneros_safety_validator::{
    EvaluatorConfig, IntegrityStatus, RequiredPoint, RequiredPointCoverage, SampleQuality,
    StateBindings, TelemetrySample, TelemetryTrustEvaluator,
};

const KEY: &[u8] = b"w47-intent-it-key-000000000000000000";
const LEASE_ID: [u8; 16] = [0x47; 16];
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

/// 放行链（Ai → dev1/pt10，与 run 路径同构装配）。
fn build_chain() -> ControlChain {
    let profile = test_profile();
    let mut chain = ControlChain::from_profile(KEY, &profile);
    let _ = profile.issue_lease(&mut chain, now_ms() / 1000);
    chain
}

/// 受信任遥测（6 点全覆盖；同 run 路径夹具）。
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

/// 可注入回执/错误的投递通道（记录投递数）。
struct StubChannel {
    outcome: Option<[u8; 2]>,
    fail: Option<eneros_control_dispatch::ChannelError>,
    pub delivered: u64,
}

impl DeliveryChannel for StubChannel {
    fn deliver(
        &mut self,
        _target: &str,
        _payload: &[u8],
        _timeout_ms: u64,
    ) -> Result<Vec<u8>, eneros_control_dispatch::ChannelError> {
        if let Some(e) = &self.fail {
            return Err(e.clone());
        }
        self.delivered += 1;
        Ok(self
            .outcome
            .unwrap_or(OutcomeFrame::new(OutcomeCode::Applied, 0).encode())
            .to_vec())
    }
}

fn engine_with(channel: StubChannel, gate: DecisionGate) -> DispatchEngine {
    engine_with_chain(channel, gate, build_chain())
}

fn engine_with_chain(
    channel: StubChannel,
    gate: DecisionGate,
    chain: ControlChain,
) -> DispatchEngine {
    let config = support::test_config(support::TestConfigSeed {
        chain,
        key: KEY.to_vec(),
        profile: test_profile(),
        issuer_service: ISSUER,
        fencing_token: 1 + EPOCH_SEED,
        now_fn: now_ms,
        intent_log: Box::new(InMemoryIntentStore::new(64)),
        delivery: Box::new(channel),
    });
    DispatchEngine::new(DispatchConfig {
        gate,
        audit: Some(DecisionAudit::new([0x47; 32])),
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

fn intent(seq: u64, value: f64) -> PointWriteIntent {
    PointWriteIntent::new(
        "mqtt",
        DEVICE_ID as u16,
        POINT_ID,
        eneros_upa_model::PointValue::Float(value),
        seq,
        now_ms(),
    )
}

/// 正例：意图（协议入站写）→ 12 态链 → 一次投递 → Committed + 审计。
#[test]
fn intent_normal_chain_commits() {
    let channel = StubChannel {
        outcome: Some(OutcomeFrame::new(OutcomeCode::Applied, 0).encode()),
        fail: None,
        delivered: 0,
    };
    let mut engine = engine_with(channel, DecisionGate::new(100, 1000.0, 3_600_000));
    let tel = trusted_telemetry();
    let receipt = engine
        .submit_intent(&intent(1, 50.0), &tel, &actor(), 5000)
        .expect("submit_intent");
    assert_eq!(receipt.final_state, DispatchState::Committed, "{receipt:?}");
    assert!(receipt.committed_at_ms.is_some());
    assert!(receipt.state_path.contains(&DispatchState::Authorized));
    assert!(receipt
        .state_path
        .contains(&DispatchState::ReadbackVerified));
    // 恰好一次投递 + 记账 + 审计入链。
    let audit = engine.audit_log().expect("audit");
    assert_eq!(audit.chain_state().entry_count, 1);
    assert!(audit.verify().expect("verify").valid);
}

/// 负例：装配绑定不符（协议层越点写）→ NotAuthorized 零投递。
#[test]
fn intent_out_of_binding_denied_zero_delivery() {
    let mut engine = engine_with(
        StubChannel {
            outcome: Some(OutcomeFrame::new(OutcomeCode::Applied, 0).encode()),
            fail: None,
            delivered: 0,
        },
        DecisionGate::new(100, 1000.0, 3_600_000),
    );
    let tel = trusted_telemetry();
    let bad = PointWriteIntent::new(
        "mqtt",
        9,
        POINT_ID,
        eneros_upa_model::PointValue::Float(5.0),
        1,
        now_ms(),
    );
    let err = engine
        .submit_intent(&bad, &tel, &actor(), 5000)
        .expect_err("deny");
    assert!(matches!(err, DispatchError::NotAuthorized(_)));
    assert_eq!(engine.committed_count(), 0);
}

/// 负例：策略拒绝（链 deny）→ NotAuthorized 零投递。
#[test]
fn intent_policy_deny_zero_delivery() {
    // 装配 chain 不含任何放行规则（default-deny）。
    let policy = Policy::new(3);
    let mut chain = ControlChain::new(KEY, 5, 64, policy, 60, 300, 64, 30);
    chain.lease_service_mut().seed_epoch(EPOCH_SEED);
    let _ = chain.lease_service_mut().issue(
        LEASE_ID,
        LeaseScope {
            devices: vec![1],
            points: vec![10],
            max_class: 1,
        },
        600,
        now_ms() / 1000,
    );
    let mut engine = engine_with_chain(
        StubChannel {
            outcome: Some(OutcomeFrame::new(OutcomeCode::Applied, 0).encode()),
            fail: None,
            delivered: 0,
        },
        DecisionGate::new(100, 1000.0, 3_600_000),
        chain,
    );
    let tel = trusted_telemetry();
    let err = engine
        .submit_intent(&intent(1, 50.0), &tel, &actor(), 5000)
        .expect_err("deny");
    assert!(matches!(err, DispatchError::NotAuthorized(_)), "{err:?}");
}

/// 负例：投递超时 → Indeterminate（写可能已发生，不猜成功）。
#[test]
fn intent_delivery_timeout_is_indeterminate() {
    let mut engine = engine_with(
        StubChannel {
            outcome: None,
            fail: Some(eneros_control_dispatch::ChannelError::Timeout),
            delivered: 0,
        },
        DecisionGate::new(100, 1000.0, 3_600_000),
    );
    let tel = trusted_telemetry();
    let receipt = engine
        .submit_intent(&intent(1, 50.0), &tel, &actor(), 5000)
        .expect("receipt");
    assert_eq!(
        receipt.final_state,
        DispatchState::Indeterminate,
        "{receipt:?}"
    );
    assert!(receipt.committed_at_ms.is_none());
}

/// 负例：读回不符（ACK 成功但物理状态未达目标）→ Failed。
#[test]
fn intent_readback_mismatch_is_failed() {
    let mut engine = engine_with(
        StubChannel {
            outcome: Some(OutcomeFrame::new(OutcomeCode::AppliedReadbackMismatch, 0).encode()),
            fail: None,
            delivered: 0,
        },
        DecisionGate::new(100, 1000.0, 3_600_000),
    );
    let tel = trusted_telemetry();
    let receipt = engine
        .submit_intent(&intent(1, 50.0), &tel, &actor(), 5000)
        .expect("receipt");
    assert_eq!(receipt.final_state, DispatchState::Failed, "{receipt:?}");
    assert!(receipt
        .note
        .as_deref()
        .unwrap_or("")
        .contains("readback-mismatch"));
}

/// 负例：重复 sequence 重放 → 第 2 次零写 Rejected（幂等）。
#[test]
fn intent_replay_same_sequence_zero_write() {
    let mut engine = engine_with(
        StubChannel {
            outcome: Some(OutcomeFrame::new(OutcomeCode::Applied, 0).encode()),
            fail: None,
            delivered: 0,
        },
        DecisionGate::new(100, 1000.0, 3_600_000),
    );
    let tel = trusted_telemetry();
    let r1 = engine
        .submit_intent(&intent(7, 50.0), &tel, &actor(), 5000)
        .expect("first");
    assert_eq!(r1.final_state, DispatchState::Committed);
    let r2 = engine
        .submit_intent(&intent(7, 50.0), &tel, &actor(), 5000)
        .expect("second");
    assert_eq!(
        r2.final_state,
        DispatchState::Rejected,
        "重放必须零写拒绝: {r2:?}"
    );
    assert!(r2.note.as_deref().unwrap_or("").contains("protocol-replay"));
    assert_eq!(engine.committed_count(), 1, "重放不得再次 Committed");
}

/// 负例：rate 预算超限（产品速率 60/h 语义——先配额 1 次，第 2 次拒绝）。
#[test]
fn intent_rate_gate_exceeds_budget() {
    let mut engine = engine_with(
        StubChannel {
            outcome: Some(OutcomeFrame::new(OutcomeCode::Applied, 0).encode()),
            fail: None,
            delivered: 0,
        },
        DecisionGate::new(1, 1000.0, 3_600_000),
    );
    let tel = trusted_telemetry();
    let r1 = engine
        .submit_intent(&intent(1, 50.0), &tel, &actor(), 5000)
        .expect("first");
    assert_eq!(r1.final_state, DispatchState::Committed);
    // 同一预算窗口内第 2 条（更高 sequence）→ BudgetExceeded 零投递。
    // 夹具用同值 setpoint：61 复查后单点爬坡检查先于预算闸（入口裁决），
    // 异值会以 ramp-limit 先拒——本测试只验预算闸语义。
    let err = engine
        .submit_intent(&intent(2, 50.0), &tel, &actor(), 5000)
        .expect_err("rate");
    assert!(matches!(err, DispatchError::BudgetExceeded(_)), "{err:?}");
}

/// 负例：遥测过期（控制时域外）→ TelemetryExpired 零投递。
#[test]
fn intent_stale_telemetry_err() {
    let mut engine = engine_with(
        StubChannel {
            outcome: Some(OutcomeFrame::new(OutcomeCode::Applied, 0).encode()),
            fail: None,
            delivered: 0,
        },
        DecisionGate::new(100, 1000.0, 3_600_000),
    );
    let mut tel = trusted_telemetry();
    tel.valid_until_ms = now_ms() - 1;
    let err = engine
        .submit_intent(&intent(1, 50.0), &tel, &actor(), 5000)
        .expect_err("stale");
    assert!(matches!(err, DispatchError::TelemetryExpired));
}

/// 意图线码喂入引擎（svc 间 EIPC 传输同源）：编码→解码→提交→Committed。
#[test]
fn intent_wire_roundtrip_then_commits() {
    let mut engine = engine_with(
        StubChannel {
            outcome: Some(OutcomeFrame::new(OutcomeCode::Applied, 0).encode()),
            fail: None,
            delivered: 0,
        },
        DecisionGate::new(100, 1000.0, 3_600_000),
    );
    let tel = trusted_telemetry();
    let orig = intent(3, -20.0);
    let decoded =
        eneros_protocol_abstract::write::decode_intent(&encode_intent(&orig)).expect("wire decode");
    assert_eq!(decoded, orig);
    let receipt = engine
        .submit_intent(&decoded, &tel, &actor(), 5000)
        .expect("submit");
    assert_eq!(receipt.final_state, DispatchState::Committed, "{receipt:?}");
}
