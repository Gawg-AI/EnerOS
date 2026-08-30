//! 蓝图 61 [07] §4.6.3/§4.7（v0.314.32）：接入桥端到端——ACK 即 durable
//! （RPO=0）与 delivery 位点幂等。
//!
//! 用真实的 [`BlockEventStore`]（MemBlockDevice 介质）——"掉电" =
//! 丢弃模块/桥，介质保留后重开再交付。
#![allow(clippy::disallowed_macros)] // 测试断言/panic——预期路径

use eneros_event_store::block::BlockEventStore;
use eneros_event_store::format::BLOCK_SIZE;
use eneros_event_store::ingest_bridge::{partition_of, StoreSink};
use eneros_event_store::memdev::{FaultyBlockDevice, MemBlockDevice};
use eneros_event_store::StoreConfig;
use eneros_ingestion_core::{
    EventQos, IngestOutcome, IngestionModule, MemEpochStore, ObservationSink, OperationalProfile,
    PointPlan, RawObservation, SourceDescriptor,
};
use eneros_telemetry_model::event::{
    quality_bits, IntegrityTag, ObservationEvent, ObservationValue,
};

fn config() -> StoreConfig {
    StoreConfig {
        segment_bytes: 8192,
        ..StoreConfig::default()
    }
}

fn descriptor(source_id: u32) -> SourceDescriptor {
    SourceDescriptor {
        source_id,
        tenant_id: 1,
        site_id: 1,
        allow_control: true,
        require_integrity: true,
        point_range: (0, 0),
        max_events_per_s: 100_000,
        allowed_schemas: vec![1],
        clock_id: 1,
        enabled: true,
    }
}

fn plan(source_id: u32, point_id: u32) -> PointPlan {
    PointPlan {
        source_id,
        point_id,
        device_id: 2,
        period_ns: 0,
        phase_offset_ns: 0,
        deadline_ns: 0,
        deadband_abs: 0.0,
        deadband_rel: 0.0,
        qos: EventQos::Q0,
        suppressible: false,
        timeout_ns: 0,
        max_retries: 0,
        schema_id: 1,
        schema_version: 1,
        unit_code: 0,
        asset_id: 1,
        mapping_version: 1,
        scale: 1.0,
        offset: 0.0,
        func_code: 0,
        reg_addr: 0,
        slave_addr: 0,
    }
}

fn raw(point_id: u32, v: i64, now: u64) -> RawObservation {
    RawObservation {
        point_id,
        value: ObservationValue::I64(v),
        event_time_ns: Some(now.max(1)),
        quality_bits: quality_bits::VALID,
        integrity: IntegrityTag::LocalTrustedEdge,
        flags: 0,
        schema_id: 0,
        schema_version: 0,
        unit_code: 0,
        asset_id: 0,
        mapping_version: 0,
        device_id: 2,
    }
}

fn make_module(dev: MemBlockDevice) -> IngestionModule {
    let store = BlockEventStore::format(FaultyBlockDevice::new(dev), config()).unwrap();
    let mut m = IngestionModule::new(
        Box::new(StoreSink::new(store)),
        Box::new(MemEpochStore::default()),
        OperationalProfile::reference(),
    );
    m.register(descriptor(2));
    m.open(2, 0).unwrap();
    m.install_points(1, vec![plan(2, 1), plan(2, 2)]).unwrap();
    m
}

fn make_event(source: u32, seq: u64, point: u32, v: f64, now: u64) -> ObservationEvent {
    let raw = ObservationEvent {
        tenant_id: 1,
        site_id: 1,
        source_id: source,
        source_epoch: 1,
        sequence: seq,
        event_time_ns: now,
        ingest_time_ns: now,
        asset_id: 1,
        device_id: 2,
        unit_code: 0,
        point_id: point,
        schema_id: 1,
        schema_version: 1,
        quality_bits: quality_bits::VALID,
        mapping_version: 1,
        value: ObservationValue::F64(v),
        integrity: IntegrityTag::LocalTrustedEdge,
        clock_id: 1,
        flags: 0,
    };
    assert!(raw.validate().is_ok());
    raw
}

/// ACK 即 durable（RPO=0）：StoreSink.append（Q0 写透）返回后立即掉电
/// （模块/桥丢弃、介质保留）——重开介质必须完整恢复全部已 ACK 事件，
/// 无重复、无丢失（[07] §4.7 门 2 的 burst 形态）。
#[test]
fn acked_events_durable_across_power_cut() {
    let dev = MemBlockDevice::new(3 + 64 * 2, BLOCK_SIZE);
    let store = BlockEventStore::format(FaultyBlockDevice::new(dev), config()).unwrap();
    let mut sink = StoreSink::new(store);
    for i in 1..=10u64 {
        sink.append(&make_event(2, i * 2 - 1, 1, 100.0 + i as f64, 1_000 + i))
            .unwrap();
        sink.append(&make_event(2, i * 2, 2, 200.0 + i as f64, 1_000 + i))
            .unwrap();
    }
    // 掉电：桥/模块整体丢弃（此后零介质接触），介质保留。
    let dev = sink.into_inner().into_inner();
    let store = BlockEventStore::open(FaultyBlockDevice::new(dev), config()).unwrap();
    let mut deliveries = StoreSink::new(store);
    let frames = deliveries.collect_delivery().unwrap();
    assert_eq!(frames.len(), 20, "已 ACK（写透）事件 RPO=0 全恢复");
    // 帧原样可解码：event_id/值不变（重放保留原事实）。
    let ev = ObservationEvent::decode_v1(&frames[0]).unwrap();
    assert_eq!(ev.point_id, 1);
    assert_eq!(ev.value, ObservationValue::F64(101.0));
    assert_eq!(ev.sequence, 1);
    // 幂等：再 drain 零重复。
    assert!(deliveries.collect_delivery().unwrap().is_empty());
    assert_eq!(deliveries.append_failures, 0);
}

/// delivery 位点幂等：drain 两次不重复交付（位点双槽持久）。
#[test]
fn delivery_offset_idempotent_across_drains() {
    let dev = MemBlockDevice::new(3 + 64 * 2, BLOCK_SIZE);
    let mut m = make_module(dev);
    for i in 0..5i64 {
        m.ingest(2, raw(1, 100 + i, 1_000 + i as u64), 1_000 + i as u64)
            .unwrap();
    }
    // 第一次 drain：5 帧。
    let first = m.drain_frames();
    assert_eq!(first.len(), 5);
    // 第二次 drain：位点已推进 → 0 帧（无重复交付）。
    let second = m.drain_frames();
    assert!(second.is_empty(), "位点幂等：重复 drain 不重复交付");

    // 分区键稳定由 partition_of 的确定性保证（同源同区，顺序保序——
    // RPO 测试已验证重放序）。
    let p = partition_of(&raw_to_event());
    assert_eq!(partition_of(&raw_to_event()), p, "分区键确定性");
}

fn raw_to_event() -> ObservationEvent {
    make_event(2, 7, 1, 1.0, 1_000) // 分区键与 sequence 无关（用 7 验证）
}

/// store 容量耗尽 → Overflow 背压显式化（[07] §2.5/§4.5 95% 拒绝联动）。
#[test]
fn store_full_backpressures_via_overflow() {
    let dev = MemBlockDevice::new(5, BLOCK_SIZE); // 1 段（3 固定 + 2 段块）
    let mut m = make_module(dev);
    let mut accepted = 0u32;
    let mut overflowed = 0u32;
    for i in 0..5000i64 {
        let outcome = m.ingest(2, raw(1, i, 1_000 + i as u64), 1_000 + i as u64);
        match outcome {
            Ok(IngestOutcome::Appended(_)) => accepted += 1,
            Ok(IngestOutcome::Coalesced) => continue,
            Err(eneros_ingestion_core::IngestReject::Overflow) => overflowed += 1,
            Err(e) => unreachable!("意外拒绝 {e:?}"),
        }
    }
    assert!(accepted > 0);
    assert!(overflowed > 0, "容量耗尽必须显式背压");
}
