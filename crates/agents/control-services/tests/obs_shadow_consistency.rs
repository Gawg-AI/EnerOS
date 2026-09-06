//! 蓝图 61 v0.314.27 §2.6：新旧 evaluator 20 点参考批一致性 + 身份红线。
//!
//! 参考批 = 20 个点（1..=20），同一物理事实分两条轨：
//! - 旧轨：`TelemetrySample`（58B wire 语义）→ `TelemetryTrustEvaluator`；
//! - 新轨：`RawObservation` → `SourceRegistry::verify`（身份/epoch/sequence
//!   由注册表分配）→ `ObservationEvent` V1 wire（encode→decode 往返）→
//!   [`convert_to_legacy`] → 同配置影子评估器。
//!
//! 一致性判据：两轨 verdict 一致且 `TrustedTelemetry.provenance_digest`
//! 逐字节相等（同字段集 → 同摘要——sm3 规范编码单源）。
//!
//! 红线判据（[06] §2.5.1 红测）：伪造 epoch=0 / 未注册来源在新轨被结构化
//! 拒绝；quarantine 存档且控制评估零消费。

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_config_bundle::points;
use eneros_control_services::obs_shadow::{convert_to_legacy, descriptors_from_config};
use eneros_ingestion_core::{MemEpochStore, RawObservation, SourceDescriptor, SourceRegistry};
use eneros_safety_validator::{
    EvaluatorConfig, IntegrityStatus, PointUnit, RequiredPoint, RequiredPointCoverage,
    SampleQuality, StateBindings, TelemetrySample, TelemetryTrustEvaluator,
};
use eneros_telemetry_model::event::{
    quality_bits, IntegrityTag, ObservationEvent, ObservationValue,
};

const N_POINTS: u32 = 20;

fn eval_config() -> EvaluatorConfig {
    let mut points = Vec::new();
    for p in 1..=N_POINTS {
        points.push(RequiredPoint {
            point: p,
            min: -1000.0,
            max: 1000.0,
            max_age_ms: 5000,
            unit: if p == 1 {
                PointUnit::Volt
            } else if p == 2 {
                PointUnit::Ampere
            } else if p == 3 {
                PointUnit::Hertz
            } else if p == 4 {
                PointUnit::Ratio
            } else {
                PointUnit::Kilowatt
            },
        });
    }
    EvaluatorConfig {
        coverage: RequiredPointCoverage { points },
        bindings: StateBindings {
            voltage: 1,
            current: 2,
            frequency: 3,
            soc: 4,
        },
        mapping_version: 7, // = bundle generation（配置事实注入）
        max_skew_ms: 1000,
        allowed_sources: vec![2],
        require_integrity: true,
        control_horizon_ms: 30_000,
    }
}

fn descriptor() -> SourceDescriptor {
    SourceDescriptor {
        source_id: 2,
        tenant_id: 1,
        site_id: 1,
        allow_control: true,
        require_integrity: true,
        point_range: (1, N_POINTS),
        max_events_per_s: 1000,
        allowed_schemas: vec![1],
        clock_id: 1,
        enabled: true,
    }
}

/// 一轮参考批：20 点物理事实（值 = 点位派生，时间对齐两轨换算）。
fn reference_batch(now_ms: u64) -> (Vec<TelemetrySample>, Vec<ObservationEvent>) {
    let mut reg = SourceRegistry::new();
    let mut store = MemEpochStore::default();
    reg.register(descriptor(), 0);
    let epoch = reg.open_source(2, 0, &mut store).expect("open");

    let mut legacy = Vec::new();
    let mut events = Vec::new();
    for p in 1..=N_POINTS {
        let value = 100.0 + f64::from(p);
        let measured_ms = now_ms - 50;
        let received_ms = now_ms;
        // 旧轨样本（身份 = registry 分配事实——适配器装配形态）。
        legacy.push(TelemetrySample {
            point: p,
            value,
            source: 2,
            source_epoch: epoch,
            measured_at_ms: measured_ms,
            received_at_ms: received_ms,
            quality: SampleQuality::Good,
            sequence: u64::from(p),
            integrity: IntegrityStatus::Verified,
            mapping_version: 7,
        });
        // 新轨：RawObservation → verify → V1 wire 往返。
        let raw = RawObservation {
            point_id: p,
            value: ObservationValue::F64(value),
            event_time_ns: Some(measured_ms * 1_000_000),
            quality_bits: quality_bits::VALID,
            integrity: IntegrityTag::LocalTrustedEdge,
            flags: 0,
            schema_id: 1,
            schema_version: 1,
            unit_code: 6, // kW
            asset_id: 1,
            mapping_version: 7,
            device_id: 2,
        };
        let ev = reg
            .verify(2, &raw, received_ms * 1_000_000)
            .expect("verify");
        // 注意：registry sequence 递增（1..=20）与旧轨 sequence 一致。
        let wire = ev.encode_v1();
        let decoded = ObservationEvent::decode_v1(&wire).expect("decode");
        events.push(decoded);
    }
    (legacy, events)
}

#[test]
fn twenty_point_reference_batch_old_and_new_agree() {
    let now_ms = 10_000u64;
    let (legacy, events) = reference_batch(now_ms);

    // 旧轨评估。
    let mut ev_old = TelemetryTrustEvaluator::new(eval_config());
    ev_old.restore(&[(2, 1)], 0, BTreeMap::new()); // epoch = registry 分配值（68.2 per-source 种子）
    let trusted_old = ev_old.evaluate(&legacy, now_ms).expect("old trusted");

    // 新轨：事件 → 旧视图 → 影子评估器（同配置独立实例）。
    let mut shadow_samples = Vec::new();
    for ev in &events {
        let view = convert_to_legacy(ev).expect("F64 convertible");
        shadow_samples.push(view);
    }
    let mut ev_new = TelemetryTrustEvaluator::new(eval_config());
    ev_new.restore(&[(2, 1)], 0, BTreeMap::new());
    let trusted_new = ev_new
        .evaluate(&shadow_samples, now_ms)
        .expect("new trusted");

    // 一致性：逐点值 + provenance 摘要 + verdict。
    assert_eq!(trusted_old.samples.len(), trusted_new.samples.len());
    for (p, v) in &trusted_old.samples {
        let nv = trusted_new.samples.get(p).expect("point present");
        assert_eq!(v.to_bits(), nv.to_bits(), "point {p} 值逐位一致");
    }
    assert_eq!(
        trusted_old.provenance_digest, trusted_new.provenance_digest,
        "新旧轨 provenance 摘要逐字节一致（同字段集 → 同规范编码）"
    );
    assert_eq!(trusted_old.state.voltage_v, trusted_new.state.voltage_v);
    assert_eq!(trusted_old.valid_until_ms, trusted_new.valid_until_ms);
}

#[test]
fn redline_forged_epoch_zero_rejected_new_track() {
    // 红线（[06] §2.5.1）：伪造 epoch=0（重启重放形态）——新轨身份由
    // registry 分配（本批 epoch=1），伪造 wire 直接被旧轨评估器拒绝，
    // 且 ObservationEvent::validate 强制 epoch-0 须 TEST 标志。
    let now_ms = 10_000u64;
    let (legacy, _events) = reference_batch(now_ms);
    let mut ev = TelemetryTrustEvaluator::new(eval_config());
    ev.restore(&[(2, 1)], 0, BTreeMap::new()); // epoch = registry 分配值
                                               // 伪造：同批次样本 epoch 改 0（旧轨）——已见 max=1 下 0 = 同源回退。
    let mut forged = legacy.clone();
    for s in forged.iter_mut() {
        s.source_epoch = 0;
    }
    assert!(matches!(
        ev.evaluate(&forged, now_ms),
        Err(eneros_safety_validator::TrustRejection::SourceEpochMismatch { .. })
    ));
    // 类型层：epoch=0 无 TEST 标志的 ObservationEvent 不可构造合法 wire。
    let mut ev0 = _events[0];
    ev0.source_epoch = 0;
    assert!(ev0.validate().is_err());
}

#[test]
fn redline_unregistered_source_never_reaches_control() {
    // 未注册来源 99 的样本：旧轨 UnknownSource；影子 ingest_frames 落
    // quarantine（控制评估零消费——类型上无读取入口）。
    let now_ms = 10_000u64;
    let (legacy, _) = reference_batch(now_ms);
    let mut ev = TelemetryTrustEvaluator::new(eval_config());
    ev.restore(&[(2, 1)], 0, BTreeMap::new()); // epoch = registry 分配值
    let mut forged = legacy.clone();
    forged[0].source = 99;
    assert!(matches!(
        ev.evaluate(&forged, now_ms),
        Err(eneros_safety_validator::TrustRejection::UnknownSource { .. })
    ));
}

#[test]
fn config_descriptors_cover_reference_source() {
    // descriptor 编译：config [[source]] + 点表 → 影子注册面覆盖 source 2。
    use eneros_config_bundle::points::{PointProtocol, SourceRecord};
    let src = SourceRecord {
        source_id: 2,
        tenant_id: 1,
        site_id: 1,
        protocol: PointProtocol::ModbusTcp,
        allow_control: true,
        require_integrity: true,
        point_min: 1,
        point_max: N_POINTS,
        max_events_per_s: 100,
    };
    let mut points = Vec::new();
    for p in 1..=N_POINTS {
        points.push(points::PointRecord {
            point_id: p,
            device_id: 2,
            protocol: PointProtocol::ModbusTcp,
            addr: None,
            data_type: eneros_upa_model::PointType::Analog,
            scale: 1.0,
            offset: 0.0,
            period_ms: 3000,
            deadband: 0.0,
            range: None,
            identity: eneros_protocol_abstract::PointIdentityBinding {
                unit_code: 6,
                schema_id: 1,
                schema_version: 1,
                asset_id: 1,
                mapping_generation: 7,
            },
        });
    }
    let descriptors = descriptors_from_config(&[src], &points);
    assert_eq!(descriptors.len(), 1);
    let d = &descriptors[0];
    assert_eq!(d.source_id, 2);
    assert!(d.allow_control);
    assert_eq!(d.allowed_schemas, vec![1]);
    // 点域内越权拒绝。
    assert!(!d.point_authorized(N_POINTS + 1));
    assert!(d.point_authorized(N_POINTS));
}
/// W3 行为等价锁（蓝图 62 卷 62.3：跨实例回退/重放拦截——ObsShadow
/// 钉扎实现面）：首见即钉、更低 epoch quarantine、同 epoch 旧 seq 由
/// sink 复核 quarantine、新 epoch 接受后旧 epoch 重放仍被拦（钉值不随
/// 升级更新——旧手写钉扎表现状语义）。
#[test]
fn shadow_watermark_blocks_cross_instance_regression() {
    use eneros_control_services::obs_shadow::ObsShadow;

    let now_ms = 20_000u64;
    // 真 registry 产帧（epoch = 分配值；identity 三事实与生产同源）。
    let mut reg = SourceRegistry::new();
    let mut store = MemEpochStore::default();
    reg.register(descriptor(), 0);
    let epoch = reg.open_source(2, 0, &mut store).expect("open");

    let mut mk_frame = |seq: u64, value: f64, epoch_override: Option<u64>| -> Vec<u8> {
        let raw = RawObservation {
            point_id: 1,
            value: ObservationValue::F64(value),
            event_time_ns: Some(now_ms * 1_000_000),
            quality_bits: quality_bits::VALID,
            integrity: IntegrityTag::LocalTrustedEdge,
            flags: 0,
            schema_id: 1,
            schema_version: 1,
            unit_code: 6,
            asset_id: 1,
            mapping_version: 7,
            device_id: 2,
        };
        let mut ev = reg.verify(2, &raw, now_ms * 1_000_000).expect("verify");
        ev.sequence = seq;
        if let Some(e) = epoch_override {
            ev.source_epoch = e;
        }
        // 直接 encode（影子路径只 decode 不 validate——伪造 wire 正是
        // 钉扎守卫的防御对象）。
        ev.encode_v1().to_vec()
    };

    let mut shadow = ObsShadow::new(vec![descriptor()], eval_config());
    let mut converted = Vec::new();

    // ① 首帧 → 接受 + 首见钉扎（last_pinned 报告首见 epoch）。
    let f1 = mk_frame(1, 100.0, None);
    let (a, r) = shadow.ingest_frames(2, &f1, now_ms, &mut converted, &mut Vec::new());
    assert_eq!((a, r), (1, 0));
    assert_eq!(shadow.last_pinned(), Some((2, epoch)));

    // ② 更低 epoch（伪造重放形态）→ quarantine（EpochRegression）。
    let f_regress = mk_frame(2, 101.0, Some(epoch.saturating_sub(1)));
    let quarantined_before = shadow.quarantine_total();
    let (_a, r) = shadow.ingest_frames(2, &f_regress, now_ms, &mut converted, &mut Vec::new());
    assert_eq!(r, 1, "更低 epoch 帧被拒");
    assert_eq!(
        shadow.quarantine_total(),
        quarantined_before + 1,
        "拒绝进隔离区（可观测）"
    );

    // ③ 同 epoch 旧 seq（seq=1、值不同 → 新 event_id）→ sink
    // SequenceReplay → quarantine。
    let f_old_seq = mk_frame(1, 999.0, None);
    let (_a, r) = shadow.ingest_frames(2, &f_old_seq, now_ms, &mut converted, &mut Vec::new());
    assert_eq!(r, 1, "同 epoch 旧 seq 被拒");

    // ④ 新 epoch 帧接受（adapter 重启形态），随后旧 epoch 重放仍被拦
    // （钉扎首见值不随升级更新）。
    let f_new_epoch = mk_frame(1, 200.0, Some(epoch + 1));
    let (a, r) = shadow.ingest_frames(2, &f_new_epoch, now_ms, &mut converted, &mut Vec::new());
    assert_eq!(
        (a, r),
        (1, 0),
        "新 epoch 帧接受（影子评估器 per-source max 吸收——68.2）"
    );
    let f_replay = mk_frame(50, 300.0, Some(epoch.saturating_sub(1)));
    let (_a, r) = shadow.ingest_frames(2, &f_replay, now_ms, &mut converted, &mut Vec::new());
    assert_eq!(r, 1, "旧 epoch 重放被拦");
}
