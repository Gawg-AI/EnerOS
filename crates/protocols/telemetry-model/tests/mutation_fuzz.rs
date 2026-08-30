//! 蓝图 61 v0.314.29 §4.5.2：确定性 mutation fuzz——解码器对任意字节
//! 无 panic/越界/无界分配（[06] §4.6 退出 gate 的 E1 等价面；cargo-fuzz
//! 目标归 fuzz/ 工具链卷）。
//!
//! 策略：规范 wire 种子 + xorshift64 确定性变异（1..=8 字节随机覆写、
//! 截断、超长扩展），每轮 decode 必须返回 Ok/Err——panic 即失败。

use eneros_telemetry_model::domain::{Classification, DomainEvent};
use eneros_telemetry_model::event::{ObservationEvent, ObservationValue};

struct Xorshift(u64);

impl Xorshift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

fn mutate(seed: u64, base: &[u8], rounds: usize, f: &mut dyn FnMut(&[u8])) {
    let mut rng = Xorshift(seed);
    for _ in 0..rounds {
        let mut buf = base.to_vec();
        let n = 1 + (rng.next() as usize) % 8;
        for _ in 0..n {
            let idx = (rng.next() as usize) % buf.len();
            buf[idx] = (rng.next() & 0xFF) as u8;
        }
        match rng.next() % 8 {
            0..=2 => {
                // 截断（保持 ≥1 字节）。
                let keep = 1 + (rng.next() as usize) % buf.len();
                buf.truncate(keep);
            }
            3 => {
                // 超长扩展（≤8KB——无界分配防护的直接证伪面）。
                buf.extend_from_slice(&vec![0u8; 8192]);
            }
            _ => {}
        }
        f(&buf);
    }
}

#[test]
fn observation_decode_never_panics() {
    let ev = ObservationEvent {
        tenant_id: 1,
        site_id: 1,
        source_id: 2,
        source_epoch: 3,
        sequence: 42,
        event_time_ns: 1_000,
        ingest_time_ns: 2_000,
        asset_id: 1,
        device_id: 1,
        unit_code: 6,
        point_id: 10,
        schema_id: 1,
        schema_version: 1,
        quality_bits: 1,
        mapping_version: 1,
        value: ObservationValue::F64(50.0),
        integrity: eneros_telemetry_model::event::IntegrityTag::LocalTrustedEdge,
        clock_id: 1,
        flags: 0,
    };
    let base = ev.encode_v1();
    let mut accepted = 0u32;
    mutate(0xE51_5EED, &base, 50_000, &mut |buf| {
        if ObservationEvent::decode_v1(buf).is_ok() {
            accepted += 1;
        }
    });
    assert!(accepted > 0, "变异空间必须覆盖可接受路径");
}

#[test]
fn domain_event_decode_never_panics() {
    let e = DomainEvent::new(
        [1; 16],
        1,
        1,
        2,
        1,
        1,
        1_000,
        2_000,
        1,
        1,
        Classification::Internal,
        0,
        [3; 16],
        [4; 16],
        vec![0xDE, 0xAD, 0xBE, 0xEF],
    )
    .unwrap();
    let base = e.encode();
    mutate(0xDEAD_BEEF, &base, 50_000, &mut |buf| {
        let _ = DomainEvent::decode(buf);
    });
}

#[test]
fn blob_ref_decode_never_panics() {
    let b = eneros_telemetry_model::domain::BlobRef {
        store_id: 1,
        object_id: [2; 16],
        size_bytes: 1024,
        sha256: [3; 32],
        media_type_id: 1,
        encryption_key_id: 0,
        expires_at_ns: 0,
    };
    let base = b.encode().to_vec();
    mutate(0xB10B_5EED, &base, 20_000, &mut |buf| {
        let _ = eneros_telemetry_model::domain::BlobRef::decode(buf);
    });
}
