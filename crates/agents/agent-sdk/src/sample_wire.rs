//! 蓝图 45 WP-45.5：遥测样本 58B 定长 wire（agent-sdk 共享模块）。
//!
//! 单一事实：root `encode_sample_wire`（service.rs 旧平行实现）与
//! control-services `telemetry::encode_sample` 曾各自实现同一 58B 布局
//! （注释互相声明"布局对齐"——易漂移）。本模块把 58B 编码/解码收归一处
//! （agent-sdk），root / svc-telemetry / 协议服务共同消费——**协议服务侧
//! 发送端与 control-services 布局逐字节一致**（45 卷允许消除的重复，登记
//! ADR 级备注）。
//!
//! 布局（小端定长 58B）：point u32 + value f64 + source u32 + source_epoch
//! u64 + measured u64 + received u64 + quality u8 + sequence u64 +
//! integrity u8 + mapping_version u64。
//!
//! 质量/完整性码映射（svc-telemetry 解码侧单一事实）：
//! quality: 0=Good 1=Suspect 2=Bad 3=Unknown；integrity: 0=Verified
//! 1=Unverified 2=Unverifiable。

use eneros_safety_validator::{IntegrityStatus, QualityReason, SampleQuality, TelemetrySample};

/// 单样本 wire 长度（定长小端）。
pub const SAMPLE_WIRE_LEN: usize = 58;

/// 质量码（0=Good 1=Suspect 2=Bad 3=Unknown）。
pub fn quality_code(q: SampleQuality) -> u8 {
    match q {
        SampleQuality::Good => 0,
        SampleQuality::Suspect(_) => 1,
        SampleQuality::Bad(_) => 2,
        SampleQuality::Unknown => 3,
    }
}

/// 完整性码（0=Verified 1=Unverified 2=Unverifiable）。
pub fn integrity_code(i: IntegrityStatus) -> u8 {
    match i {
        IntegrityStatus::Verified => 0,
        IntegrityStatus::Unverified => 1,
        IntegrityStatus::Unverifiable => 2,
    }
}

/// 样本 → 58B wire。
pub fn encode_sample(s: &TelemetrySample) -> [u8; SAMPLE_WIRE_LEN] {
    let mut out = [0u8; SAMPLE_WIRE_LEN];
    out[0..4].copy_from_slice(&s.point.to_le_bytes());
    out[4..12].copy_from_slice(&s.value.to_le_bytes());
    out[12..16].copy_from_slice(&s.source.to_le_bytes());
    out[16..24].copy_from_slice(&s.source_epoch.to_le_bytes());
    out[24..32].copy_from_slice(&s.measured_at_ms.to_le_bytes());
    out[32..40].copy_from_slice(&s.received_at_ms.to_le_bytes());
    out[40] = quality_code(s.quality);
    out[41..49].copy_from_slice(&s.sequence.to_le_bytes());
    out[49] = integrity_code(s.integrity);
    out[50..58].copy_from_slice(&s.mapping_version.to_le_bytes());
    out
}

/// wire → 样本（长度不符/未知码 → None，fail-closed）。quality 仅还原为
/// 层位（Unknown 仍带定位）；svc-telemetry 评估层再按语义降权/拒。
pub fn decode_sample(buf: &[u8]) -> Option<TelemetrySample> {
    if buf.len() != SAMPLE_WIRE_LEN {
        return None;
    }
    let quality = match buf[40] {
        0 => SampleQuality::Good,
        1 => SampleQuality::Suspect(QualityReason::Stale),
        2 => SampleQuality::Bad(QualityReason::Stale),
        _ => SampleQuality::Unknown,
    };
    let integrity = match buf[49] {
        0 => IntegrityStatus::Verified,
        1 => IntegrityStatus::Unverified,
        _ => IntegrityStatus::Unverifiable,
    };
    Some(TelemetrySample {
        point: u32::from_le_bytes(buf[0..4].try_into().ok()?),
        value: f64::from_le_bytes(buf[4..12].try_into().ok()?),
        source: u32::from_le_bytes(buf[12..16].try_into().ok()?),
        source_epoch: u64::from_le_bytes(buf[16..24].try_into().ok()?),
        measured_at_ms: u64::from_le_bytes(buf[24..32].try_into().ok()?),
        received_at_ms: u64::from_le_bytes(buf[32..40].try_into().ok()?),
        quality,
        sequence: u64::from_le_bytes(buf[41..49].try_into().ok()?),
        integrity,
        mapping_version: u64::from_le_bytes(buf[50..58].try_into().ok()?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TelemetrySample {
        TelemetrySample {
            point: 7,
            value: 12.5,
            source: 1,
            source_epoch: 0,
            measured_at_ms: 950,
            received_at_ms: 1000,
            quality: SampleQuality::Good,
            sequence: 3,
            integrity: IntegrityStatus::Verified,
            mapping_version: 1,
        }
    }

    /// 58B 定长往返 + 逐字段一致（与 svc-telemetry 解码侧同布局）。
    #[test]
    fn roundtrip_all_fields() {
        let buf = encode_sample(&sample());
        assert_eq!(buf.len(), SAMPLE_WIRE_LEN);
        let d = decode_sample(&buf).expect("decode");
        assert_eq!(d.point, 7);
        assert_eq!(d.value, 12.5);
        assert_eq!(d.source, 1);
        assert_eq!(d.source_epoch, 0);
        assert_eq!(d.measured_at_ms, 950);
        assert_eq!(d.received_at_ms, 1000);
        assert_eq!(d.quality, SampleQuality::Good);
        assert_eq!(d.sequence, 3);
        assert_eq!(d.integrity, IntegrityStatus::Verified);
        assert_eq!(d.mapping_version, 1);
    }

    /// 同一布局两份实现曾经漂移的风险点：质量/完整性码映射固定。
    #[test]
    fn quality_and_integrity_codes_are_stable() {
        assert_eq!(quality_code(SampleQuality::Good), 0);
        assert_eq!(
            quality_code(SampleQuality::Suspect(QualityReason::Stale)),
            1
        );
        assert_eq!(quality_code(SampleQuality::Bad(QualityReason::Stale)), 2);
        assert_eq!(quality_code(SampleQuality::Unknown), 3);
        assert_eq!(integrity_code(IntegrityStatus::Verified), 0);
        assert_eq!(integrity_code(IntegrityStatus::Unverified), 1);
        assert_eq!(integrity_code(IntegrityStatus::Unverifiable), 2);
        // 未知码 → 解码 fail-closed（Unknown/Unverifiable）。
        let mut buf = encode_sample(&sample());
        buf[40] = 42;
        assert_eq!(decode_sample(&buf).unwrap().quality, SampleQuality::Unknown);
        buf[40] = 0;
        buf[49] = 9;
        assert_eq!(
            decode_sample(&buf).unwrap().integrity,
            IntegrityStatus::Unverifiable
        );
        // 长度不符 → None。
        assert!(decode_sample(&buf[..57]).is_none());
    }
}
