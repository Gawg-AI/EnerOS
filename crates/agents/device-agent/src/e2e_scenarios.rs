//! 蓝图/24 §8.2 第三层端到端场景证据（`#[cfg(test)]`，host 集成先行）。
//!
//! QEMU spawn 现状声明（蓝图/24 §8.2/§7.1 WP-24.5 回退条款）：device-agent
//! 尚无 EL0 service 装配（root-task/services 未 spawn），runner 为 host 二进制
//! （llama.cpp，模型 491MB 无法进 init bundle，ADR-0036 §5）——**QEMU 三场景
//! 暂不可复算**，以 host 集成证据先行并显式声明；QEMU 场景挂入蓝图/22 系列
//! 验收（场景→证据→可复算格式对齐）。
//!
//! 场景：
//! ① AI 决策 → 链 → 设备写成功（含回读）；② 篡改信封被拒且审计留痕；
//! ③ 断网（无云侧）本地预置租约写成功；④ §6.6 全部指标 telemetry 导出可见。

#![cfg(test)]

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use eneros_control_gateway::envelope::{ActorKind, EnvelopeBuilder};
use eneros_control_gateway::lease::LeaseScope;
use eneros_control_gateway::policy::{CommandClass, Policy, PolicyRule};
use eneros_control_gateway::safety::{SafetyPoint, SafetyRole};
use eneros_telemetry_model::command::{ControlCommand, SingleCommand};
use eneros_telemetry_model::point_spec::{ControlDirection, PointSpec};
use eneros_telemetry_model::unit::PhysicalUnit;

use crate::delivery::DeliveryFrame;
use crate::device_type::DeviceType;
use crate::ingress::{
    verify_handshake_response, EnvelopeIngress, IngressOutcome, METRIC_DENIED_HMAC,
    METRIC_WRITE_ALLOWED,
};
use crate::registry::MockPointDevice;
use crate::{DeviceAgent, MockCommandSource};

const KEY: &[u8] = b"w24-e2e-scenario-key-0000000000000000000";
const LEASE_ID: [u8; 16] = [0x51; 16];
const ISSUER: [u8; 16] = [0x52; 16];
const EPOCH_SEED: u64 = 3; // 平台重启计数（模拟）
const POLICY_VER: u64 = 2;
const NOW_MS: u64 = 1_700_000_000_000;

fn now_secs() -> u64 {
    NOW_MS / 1000
}

/// 装配收口点 + 设备（PCS：power_setpoint 点 10）。
fn assembled_agent() -> (DeviceAgent, [u8; 16], u64) {
    let policy = Policy::new(POLICY_VER).with_rule(PolicyRule {
        actor_kind: Some(ActorKind::Ai),
        devices: vec![1],
        points: vec![10],
        kinds: vec![CommandClass::Switch],
        time_window: (0, 0),
        obligations: vec![],
    });
    let safety = vec![(
        1u32,
        10u32,
        SafetyPoint {
            role: SafetyRole::PccBreaker,
            spec: PointSpec {
                point_id: 10,
                device_id: 1,
                unit: PhysicalUnit::Kilowatts,
                safe_min: -500.0,
                safe_max: 500.0,
                max_ramp_per_sec: None,
                direction: ControlDirection::Discrete,
                remote_controllable: true,
            },
        },
    )];
    let ingress = EnvelopeIngress::new(
        KEY,
        LEASE_ID,
        LeaseScope {
            devices: vec![1],
            points: vec![10],
            max_class: 1,
        },
        3_600,
        EPOCH_SEED,
        policy,
        60,
        &safety,
        NOW_MS,
    );
    // 显式 point_id 10（与链策略/租约/信封目标一致）。
    let dev = MockPointDevice::new(1).with_point_at("power_setpoint", 0.0, 10);
    let mut pm = BTreeMap::new();
    if let Some(id) = dev.point_id_of("power_setpoint") {
        pm.insert(String::from("power_setpoint"), id);
    }

    let mut agent =
        DeviceAgent::new_with_ingress("e2e", Box::new(MockCommandSource::new()), 0, ingress);
    agent
        .registry_mut()
        .register("pcs", DeviceType::Pcs, 1, Box::new(dev), pm);
    let (_, token) = verify_handshake_response(
        KEY,
        &agent.ingress.handshake_response().expect("本地租约握手"),
    )
    .expect("握手 HMAC");
    (agent, LEASE_ID, token)
}

/// 构造签名信封（签发侧语义：与 runner ChainCommandSink 同构）。
fn signed_envelope(token: u64, seq: u64, idem: [u8; 16], power: f64) -> Vec<u8> {
    let mut id_bytes = [0u8; 16];
    id_bytes[..8].copy_from_slice(&seq.to_le_bytes());
    let env = EnvelopeBuilder::new(
        id_bytes,
        1,
        10,
        ControlCommand::Single(SingleCommand::On),
        now_secs(),
        now_secs() + 30,
        ISSUER,
    )
    .lease(LEASE_ID, token)
    .policy_version(POLICY_VER)
    .sequence(seq)
    .actor_kind(ActorKind::Ai)
    .idempotency_key(idem)
    .build(KEY)
    .encode();
    DeliveryFrame {
        envelope: env,
        power_kw: power,
    }
    .encode()
    .to_vec()
}

/// 场景 ①：AI 决策 → 链 → 设备写成功（含回读）。
#[test]
fn scenario1_ai_decision_to_device_write_with_readback() {
    let (mut agent, _lease, token) = assembled_agent();
    let payload = signed_envelope(token, 1, [0x01; 16], 50.0);
    let out = agent.process_envelope(&payload, NOW_MS);
    assert!(
        matches!(out, IngressOutcome::Applied { power_kw: 50.0, .. }),
        "场景①：AI 信封必须落写成功: {out:?}"
    );
    // 回读：设备 power_setpoint 值 == 50.0（经 PointAccess 读回）。
    let info = agent.registry_mut().get_mut("pcs").expect("pcs");
    let dp = info.adapter.read_point(10).expect("read");
    assert_eq!(
        dp.value,
        eneros_upa_model::PointValue::Float(50.0),
        "回读必须与写入一致"
    );
    assert_eq!(agent.ingress.metric_total(METRIC_WRITE_ALLOWED, NOW_MS), 1);
    // 审计留痕：LeaseGranted + HandshakeCompleted + WriteExecuted。
    assert_eq!(agent.ingress.audit_entry_count(), 3);
}

/// 场景 ②：篡改信封被拒且审计留痕。
#[test]
fn scenario2_tampered_envelope_denied_with_audit_trail() {
    let (mut agent, _lease, token) = assembled_agent();
    let mut payload = signed_envelope(token, 1, [0x02; 16], 50.0);
    payload[45] ^= 0xFF; // 篡改信封 device_id 字节（HMAC 覆盖区）
    let out = agent.process_envelope(&payload, NOW_MS);
    assert!(
        matches!(
            out,
            IngressOutcome::Denied {
                class: crate::ingress::DenyClass::Hmac,
                ..
            }
        ),
        "场景②：篡改信封必须被拒: {out:?}"
    );
    assert_eq!(agent.ingress.metric_total(METRIC_DENIED_HMAC, NOW_MS), 1);
    // 审计留痕：WriteDenied 事件已入链（LeaseGranted + HandshakeCompleted
    // + WriteDenied）。
    assert_eq!(agent.ingress.audit_entry_count(), 3, "篡改拒绝必须审计留痕");
    // 设备状态不变。
    let info = agent.registry_mut().get_mut("pcs").unwrap();
    assert_eq!(
        info.adapter.read_point(10).unwrap().value,
        eneros_upa_model::PointValue::Float(0.0)
    );
}

/// 场景 ③：断网（无云侧）本地预置租约写成功——本链路自始至终无云侧参与：
/// 本地租约本地签发（重启计数为种子）、本地握手、本地复验落写。
#[test]
fn scenario3_local_lease_write_success_without_cloud() {
    let (mut agent, lease_id, token) = assembled_agent();
    // 断网语义：不接触任何外部通道（无 runner/无云）；本地租约即信任根。
    let handshake = agent
        .ingress
        .handshake_response()
        .expect("本地握手（断网可用）");
    let (got_lease, got_token) = verify_handshake_response(KEY, &handshake).expect("hmac");
    assert_eq!(got_lease, lease_id);
    assert_eq!(got_token, EPOCH_SEED + 1, "token 以重启计数为种子铸造");

    let payload = signed_envelope(token, 1, [0x03; 16], -25.0);
    let out = agent.process_envelope(&payload, NOW_MS);
    assert!(
        matches!(
            out,
            IngressOutcome::Applied {
                power_kw: -25.0,
                ..
            }
        ),
        "场景③：断网本地预置租约必须写成功: {out:?}"
    );
    assert_eq!(agent.ingress.metric_total(METRIC_WRITE_ALLOWED, NOW_MS), 1);
}

/// 场景 ④：§6.6 全部指标在 telemetry 导出可见（OtlpExporter 真实导出路径）。
#[test]
fn scenario4_all_metrics_visible_in_telemetry_export() {
    use eneros_telemetry_queue::persist::OtlpExporter;
    use eneros_telemetry_queue::queue::{TelemetryCategory, TelemetryEvent};

    let (mut agent, _lease, token) = assembled_agent();
    // 驱动一次允许 + 一次拒绝，让各指标出现。
    let ok_payload = signed_envelope(token, 1, [0x04; 16], 10.0);
    let _ = agent.process_envelope(&ok_payload, NOW_MS);
    let mut bad_payload = signed_envelope(token, 2, [0x05; 16], 10.0);
    bad_payload[45] ^= 0xFF;
    let _ = agent.process_envelope(&bad_payload, NOW_MS);
    agent.ingress.record_legacy_denied(NOW_MS);

    // §6.6 指标清单（11 项）。
    let metric_names = [
        crate::ingress::METRIC_WRITE_ALLOWED,
        crate::ingress::METRIC_DENIED_DECODE,
        crate::ingress::METRIC_DENIED_HMAC,
        crate::ingress::METRIC_DENIED_LEASE,
        crate::ingress::METRIC_DENIED_STALE_TOKEN,
        crate::ingress::METRIC_DENIED_POLICY,
        crate::ingress::METRIC_DENIED_SAFETY,
        crate::ingress::METRIC_DENIED_MISMATCH,
        crate::ingress::METRIC_DENIED_LEGACY,
        crate::ingress::METRIC_IDEMPOTENT_HIT,
        crate::ingress::METRIC_FAILED_PROTOCOL,
    ];

    // 指标 → TelemetryEvent（PerformanceMetrics）→ OtlpExporter（真实导出）。
    let exported: std::rc::Rc<std::cell::RefCell<Vec<String>>> =
        std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink = exported.clone();
    let mut exporter = OtlpExporter::new(move |line: &str| {
        sink.borrow_mut().push(String::from(line));
        Ok(())
    });
    for name in metric_names {
        let value = agent.ingress.metric_total(name, NOW_MS);
        let event = TelemetryEvent {
            id: 0,
            category: TelemetryCategory::PerformanceMetrics,
            timestamp: now_secs(),
            payload: format!("{name}={value}"),
        };
        exporter.export(&event).expect("导出成功");
    }
    // 全部 11 项指标名在导出载荷中可见（含 0 值——已注册即可观测）。
    let all = exported.borrow().join("\n");
    for name in metric_names {
        assert!(all.contains(name), "指标 {name} 必须经 telemetry 导出可见");
    }
    // 数值正确性：allowed=1、denied_hmac=1、legacy=1。
    assert!(all.contains("eneros_write_allowed_total=1"));
    assert!(all.contains("eneros_write_denied_hmac_total=1"));
    assert!(all.contains("eneros_write_denied_legacy_total=1"));
    assert_eq!(exporter.exported_count(), 11);
}
