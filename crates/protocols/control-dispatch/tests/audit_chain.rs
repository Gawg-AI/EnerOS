//! 47（WP-47.4 DoD）：控制事件审计链——每个 Committed/Failed 控制事件
//! 入 SHA-256 链（security/audit D4 线）；`verify_log` PASS 且
//! `last_valid_prefix` 定位正确（尾部截断/损坏不掩盖）。
//!
//! 捕获日志器实现 [`AuditLogger`]（真实链头推进：seq 严格递增 + 组合
//! 哈希），同时产出 [`DurableAuditRecord`] 序列（Rc 共享）供
//! `verify_log`/`last_valid_prefix` 校验。
mod support;

use std::cell::RefCell;
use std::rc::Rc;

use eneros_audit::durable::{last_valid_prefix, verify_log, DurableAuditRecord};
use eneros_audit::{AuditError, AuditEvent, AuditLogger, ChainEntry, HashChain};
use eneros_control_dispatch::{
    AuthenticatedActor, ControlDispatch, DispatchConfig, DispatchEngine, DispatchState,
    InMemoryIntentStore,
};
use eneros_control_gateway::chain::ControlChain;
use eneros_control_gateway::envelope::ActorKind;
use eneros_control_gateway::wire::OutcomeCode;
use eneros_protocol_abstract::write::PointWriteIntent;
use eneros_safety_validator::{
    EvaluatorConfig, IntegrityStatus, RequiredPoint, RequiredPointCoverage, SampleQuality,
    StateBindings, TelemetrySample, TelemetryTrustEvaluator,
};

/// 锚（外部；不得来自待验证日志）。
const GENESIS: [u8; 32] = [0x47; 32];
const BOOT_ID: [u8; 16] = [0xAB; 16];
const KEY: &[u8] = b"w47-audit-it-key-0000000000000000000";

fn now_ms() -> u64 {
    1_700_000_000_000
}

/// 捕获日志器：真实链头推进 + durable 记录流（Rc 共享观测）。
struct CaptureLogger {
    chain: HashChain,
    records: Rc<RefCell<Vec<DurableAuditRecord>>>,
}

impl CaptureLogger {
    fn new(records: Rc<RefCell<Vec<DurableAuditRecord>>>) -> Self {
        Self {
            chain: HashChain {
                last_hash: GENESIS,
                last_seq: 0,
                entry_count: 0,
                genesis_hash: GENESIS,
            },
            records,
        }
    }
}

impl AuditLogger for CaptureLogger {
    fn log(&mut self, mut event: AuditEvent) -> Result<ChainEntry, AuditError> {
        event.seq = self.chain.last_seq + 1;
        let rec = DurableAuditRecord::new(self.chain.last_hash, event, BOOT_ID, None);
        let entry = ChainEntry {
            seq: rec.sequence,
            prev_hash: rec.previous_hash,
            event_hash: rec.record_hash,
            combined_hash: rec.record_hash,
            timestamp_ms: 0,
        };
        self.chain.last_hash = rec.record_hash;
        self.chain.last_seq = rec.sequence;
        self.chain.entry_count += 1;
        self.records.borrow_mut().push(rec);
        Ok(entry)
    }

    fn log_batch(&mut self, events: &[AuditEvent]) -> Result<Vec<ChainEntry>, AuditError> {
        events.iter().map(|e| self.log(e.clone())).collect()
    }

    fn chain_state(&self) -> &HashChain {
        &self.chain
    }
}

// ==================== 引擎夹具（同 intent_ingress_chain 同构） ====================

/// 测试同源档案（蓝图 55：绑定 pt1/TTL 600/代次 4，值语义与旧手抄块一致）。
fn test_profile() -> eneros_control_gateway::ControlProfile {
    eneros_control_gateway::ControlProfile {
        lease_ttl_secs: 600,
        ..eneros_control_gateway::ControlProfile::ai_pcs_setpoint(1, 1, 4, [0x47; 16], 7)
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
    ev.evaluate(
        &[
            good(1, 380.0, 1),
            good(2, 10.0, 1),
            good(3, 50.0, 1),
            good(4, 0.6, 1),
            good(5, 1.0, 1),
            good(6, 0.0, 1),
        ],
        now,
    )
    .expect("trusted telemetry")
}

struct StubChannel {
    outcome: [u8; 2],
    pub delivered: u64,
}

impl eneros_control_dispatch::DeliveryChannel for StubChannel {
    fn deliver(
        &mut self,
        _target: &str,
        _payload: &[u8],
        _timeout_ms: u64,
    ) -> Result<Vec<u8>, eneros_control_dispatch::ChannelError> {
        self.delivered += 1;
        Ok(self.outcome.to_vec())
    }
}

fn build_engine(control_audit: Box<dyn AuditLogger>, outcome_code: u8) -> DispatchEngine {
    let outcome = [outcome_code, 0];
    DispatchEngine::new(DispatchConfig {
        control_audit: Some(control_audit),
        ..support::test_config(support::TestConfigSeed {
            chain: build_chain(),
            key: KEY.to_vec(),
            profile: test_profile(),
            issuer_service: [0x41; 16],
            fencing_token: 8,
            now_fn: now_ms,
            intent_log: Box::new(InMemoryIntentStore::new(64)),
            delivery: Box::new(StubChannel {
                outcome,
                delivered: 0,
            }),
        })
    })
}

fn actor() -> AuthenticatedActor {
    AuthenticatedActor {
        kind: ActorKind::Ai,
        service: [0x41; 16],
        session_epoch: 0,
    }
}

fn intent(seq: u64) -> PointWriteIntent {
    PointWriteIntent::new(
        "mqtt",
        1,
        1,
        eneros_upa_model::PointValue::Float(50.0),
        seq,
        now_ms(),
    )
}

/// DoD 正例：Committed 控制事件入链（≥1 条）；verify_log PASS。
#[test]
fn committed_control_events_verify_log_pass() {
    let records: Rc<RefCell<Vec<DurableAuditRecord>>> = Rc::new(RefCell::new(Vec::new()));
    let capture = Box::new(CaptureLogger::new(Rc::clone(&records)));
    let mut engine = build_engine(capture, OutcomeCode::Applied.code());
    let tel = trusted_telemetry();
    let receipt = engine
        .submit_intent(&intent(1), &tel, &actor(), 5000)
        .expect("submit");
    assert_eq!(receipt.final_state, DispatchState::Committed, "{receipt:?}");
    let recs = records.borrow();
    assert!(!recs.is_empty(), "控制事件必须入链");
    // 全链校验（锚 = GENESIS）。
    verify_log(&recs, GENESIS).expect("verify_log 必须 PASS");
    // 事件类型覆盖：至少一条 WriteExecuted（成功链）。detail 含状态轨迹。
    let has_executed = recs
        .iter()
        .any(|r| r.event.event_type == eneros_audit::AuditEventType::WriteExecuted);
    assert!(has_executed, "Committed 链必须含 WriteExecuted 控制事件");
}

/// Exit Gate 项 1（收尾验收补证）：入链成功可观测——append 计数随控制
/// 事件递增、drop 恒零。QEMU 串口 AUDIT-CHAIN 行以此计数为据（无计数
/// 不打印，杜绝恒 ok 式假证据）。
#[test]
fn audit_append_count_observable() {
    let records: Rc<RefCell<Vec<DurableAuditRecord>>> = Rc::new(RefCell::new(Vec::new()));
    let mut engine = build_engine(
        Box::new(CaptureLogger::new(Rc::clone(&records))),
        OutcomeCode::Applied.code(),
    );
    assert_eq!(engine.audit_append_count(), 0, "初始零入链");
    assert_eq!(engine.audit_drop_count(), 0);
    let tel = trusted_telemetry();
    let receipt = engine
        .submit_intent(&intent(1), &tel, &actor(), 5000)
        .expect("submit");
    assert_eq!(receipt.final_state, DispatchState::Committed, "{receipt:?}");
    assert!(
        engine.audit_append_count() >= 1,
        "Committed 必须 ≥1 条成功入链（append 计数递增）"
    );
    assert_eq!(engine.audit_drop_count(), 0, "成功入链零丢弃");
}

/// DoD：Committed + Failed 双事件入链；尾部截断 → verify_log 拒绝且
/// last_valid_prefix 精确定位（不掩盖损坏）。
#[test]
fn failed_event_and_truncation_last_valid_prefix() {
    let records: Rc<RefCell<Vec<DurableAuditRecord>>> = Rc::new(RefCell::new(Vec::new()));
    let mut engine = build_engine(
        Box::new(CaptureLogger::new(Rc::clone(&records))),
        OutcomeCode::AppliedReadbackMismatch.code(),
    );
    let tel = trusted_telemetry();
    let receipt = engine
        .submit_intent(&intent(1), &tel, &actor(), 5000)
        .expect("submit");
    assert_eq!(receipt.final_state, DispatchState::Failed, "{receipt:?}");
    let recs = records.borrow();
    assert!(!recs.is_empty(), "Failed 控制事件必须入链");
    // 链完整 → verify PASS。
    verify_log(&recs, GENESIS).expect("完整链 verify_log PASS");
    let n = recs.len();
    assert!(n >= 1);
    // 尾部截断（模拟日志后半丢失）→ verify_log 拒绝。
    let truncated: Vec<DurableAuditRecord> = recs[..n / 2].to_vec();
    assert!(truncated.len() < n);
    assert!(
        verify_log(&truncated, GENESIS).is_err(),
        "截断链必须拒绝（不掩盖损坏）"
    );
    // last_valid_prefix 定位 = 截断串行中最后一个合法记录数。
    let prefix = last_valid_prefix(&truncated, GENESIS);
    assert_eq!(prefix, truncated.len(), "截断链合法前缀 = 现有条数");
    // 损坏（篡改 CRС/哈希）→ 定位在损坏点之前。
    let mut tampered = recs.clone();
    let last = tampered.last_mut().expect("last");
    last.crc ^= 0xFF;
    let prefix2 = last_valid_prefix(&tampered, GENESIS);
    assert_eq!(prefix2, n - 1, "尾部损坏 → 合法前缀定位到损坏前一条");
}
