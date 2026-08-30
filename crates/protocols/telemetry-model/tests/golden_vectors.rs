//! 蓝图 61 v0.314.29 §4.5.1：golden vectors（tests/vectors/blueprint61/）。
//!
//! 双向锁定：规范示例 encode == 冻结 hex 文件；hex 文件 decode == 规范
//! 示例。字段集/布局/摘要任一漂移即编译后测试红。

use eneros_telemetry_model::domain::{BlobRef, Classification, DomainEvent};
use eneros_telemetry_model::event::{
    quality_bits, IntegrityTag, ObservationEvent, ObservationValue,
};
use eneros_telemetry_model::unit_catalog;

const VEC_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../tests/vectors/blueprint61"
);

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|x| format!("{:02x}", x)).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn read(name: &str) -> String {
    let raw = std::fs::read_to_string(format!("{VEC_DIR}/{name}")).expect("golden file");
    raw.trim().to_string()
}

/// 规范 ObservationEvent 示例（与 ADR-0047 示例同参）。
pub fn canonical_observation() -> ObservationEvent {
    ObservationEvent {
        tenant_id: 1,
        site_id: 1,
        source_id: 2,
        source_epoch: 3,
        sequence: 42,
        event_time_ns: 1_700_000_000_000_000_000,
        ingest_time_ns: 1_700_000_000_050_000_000,
        asset_id: 1,
        device_id: 1,
        unit_code: unit_catalog::code_of(eneros_telemetry_model::PhysicalUnit::Kilowatts),
        point_id: 10,
        schema_id: 1,
        schema_version: 1,
        quality_bits: quality_bits::VALID,
        mapping_version: 1,
        value: ObservationValue::F64(50.0),
        integrity: IntegrityTag::LocalTrustedEdge,
        clock_id: 1,
        flags: 0,
    }
}

fn canonical_domain() -> DomainEvent {
    DomainEvent::new(
        [0xA1; 16],
        1,
        1,
        7,
        2,
        100,
        1_700_000_000_000_000_000,
        1_700_000_000_100_000_000,
        5,
        1,
        Classification::Internal,
        0,
        [0xC1; 16],
        [0xC2; 16],
        vec![0x01, 0x02, 0x03, 0x04],
    )
    .unwrap()
}

fn canonical_blob() -> BlobRef {
    BlobRef {
        store_id: 1,
        object_id: [0xB2; 16],
        size_bytes: 1 << 20,
        sha256: [0xCC; 32],
        media_type_id: 2,
        encryption_key_id: 0,
        expires_at_ns: 0,
    }
}

#[test]
fn observation_v1_golden() {
    let ev = canonical_observation();
    let wire = ev.encode_v1();
    // encode == 文件。
    assert_eq!(hex(&wire), read("observation-v1.golden"));
    // 文件 decode == 规范示例。
    assert_eq!(
        ObservationEvent::decode_v1(&unhex(&read("observation-v1.golden"))).unwrap(),
        ev
    );
    // event_id 向量（SHA-256 前 16B——字段序冻结的证据）。
    assert_eq!(hex(&ev.event_id()), read("observation-event-id.golden"));
    // reserved 非 0 的变体必须拒绝（向量语义自证）。
    let mut bad = unhex(&read("observation-v1.golden"));
    bad[122] = 1;
    assert!(ObservationEvent::decode_v1(&bad).is_err());
}

#[test]
fn domain_event_v1_golden() {
    let e = canonical_domain();
    assert_eq!(hex(&e.encode()), read("domain-event-v1.golden"));
    assert_eq!(
        DomainEvent::decode(&unhex(&read("domain-event-v1.golden"))).unwrap(),
        e
    );
}

#[test]
fn blob_ref_v1_golden() {
    let b = canonical_blob();
    assert_eq!(hex(&b.encode()), read("blob-ref-v1.golden"));
    assert_eq!(
        BlobRef::decode(&unhex(&read("blob-ref-v1.golden"))).unwrap(),
        b
    );
}
