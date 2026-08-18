//! 蓝图 40 WP-40.2：TelemetryService（svc-telemetry 服务逻辑）。
//!
//! - 有界样本缓冲（去重：同点同序号拒绝；断线 epoch 推进后旧消息拒绝）；
//! - 快照：原子冻结覆盖 + provenance（经 `TelemetryTrustEvaluator`——
//!   覆盖/新鲜度/质量/范围/重放/完整性全校验）；
//! - 输出只能是 `TrustedTelemetry` 或结构化拒绝（零"健康默认值"）。
//!
//! 服务请求 wire（定长，版本化）：
//! - `TEL_INGEST`：样本批次（逐样本 8 字段定长）→ ack；
//! - `TEL_SNAPSHOT`：评估当前缓冲 → `TrustedTelemetry` wire 或拒绝码；
//! - `TEL_ADVANCE_EPOCH`：断线重连推进（旧消息拒绝）。

use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

use eneros_safety_validator::{
    EvaluatorConfig, TelemetrySample, TelemetryTrustEvaluator, TrustRejection, TrustedTelemetry,
};

/// 请求码。
pub const TEL_INGEST: u8 = 1;
pub const TEL_SNAPSHOT: u8 = 2;
pub const TEL_ADVANCE_EPOCH: u8 = 3;

/// 单个样本 wire 长度（定长：point 4 + value 8 + source 4 + source_epoch 8 +
/// measured 8 + received 8 + quality 1 + sequence 8 + integrity 1 + mapping 8）。
pub const SAMPLE_WIRE_LEN: usize = 58;
/// 样本缓冲上限（有界）。
pub const MAX_BUFFERED_SAMPLES: usize = 256;
/// 评估器配置（服务装配期注入；覆盖点由部署配置决定）。
pub const DEFAULT_CONTROL_HORIZON_MS: u64 = 30_000;

/// 快照响应（TrustedTelemetry wire）。
pub const SNAPSHOT_OK: u8 = 0;
pub const SNAPSHOT_REJECTED: u8 = 1;

/// TelemetryService 服务逻辑（host 可测；target 经 EIPC 循环调用）。
pub struct TelemetryService {
    evaluator: TelemetryTrustEvaluator,
    /// 有界样本缓冲（FIFO；满时拒绝新样本——不静默丢）。
    buffer: VecDeque<TelemetrySample>,
    /// 已收样本计数（观测）。
    pub ingested: u64,
    /// 被拒样本计数（去重/满）。
    pub rejected: u64,
}

impl TelemetryService {
    /// 构造（评估器配置）。
    pub fn new(config: EvaluatorConfig) -> Self {
        Self {
            evaluator: TelemetryTrustEvaluator::new(config),
            buffer: VecDeque::new(),
            ingested: 0,
            rejected: 0,
        }
    }

    /// 缓冲首样本 age（诊断：now - measured_at；空 → None）。
    pub fn first_sample_age_ms(&self, now_ms: u64) -> Option<u64> {
        self.buffer
            .front()
            .map(|s| now_ms.saturating_sub(s.measured_at_ms))
    }

    /// 当前来源 epoch。
    pub fn epoch(&self) -> u64 {
        self.evaluator.epoch()
    }

    /// 断线重连推进 epoch（旧 epoch 消息全部拒绝）。
    pub fn advance_epoch(&mut self) {
        self.evaluator.advance_epoch();
        self.buffer.clear();
    }

    /// 接收一批样本（去重：同点同序号拒绝；满 → 拒绝并计数）。
    pub fn ingest(&mut self, samples: &[TelemetrySample]) -> usize {
        let mut accepted = 0usize;
        for s in samples {
            // 去重：缓冲中已存在同点同序号。
            if self
                .buffer
                .iter()
                .any(|b| b.point == s.point && b.sequence == s.sequence)
            {
                self.rejected += 1;
                continue;
            }
            if self.buffer.len() >= MAX_BUFFERED_SAMPLES {
                self.rejected += 1;
                continue;
            }
            self.buffer.push_back(*s);
            accepted += 1;
        }
        self.ingested += accepted as u64;
        accepted
    }

    /// 清空样本缓冲（新一轮采集周期开始——旧周期样本不参与评估；
    /// QEMU 实弹：多轮样本累积导致同点重复 Replay / 跨周期 skew
    /// ClockNotMonotonic，快照必须原子冻结单周期样本）。
    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    /// 评估当前缓冲（快照原子冻结：评估器在拷贝上执行，不因并发 ingest
    /// 变化——单线程服务循环内天然原子；provenance 覆盖全部样本）。
    pub fn snapshot(&mut self, now_ms: u64) -> Result<TrustedTelemetry, TrustRejection> {
        let samples: Vec<TelemetrySample> = self.buffer.iter().copied().collect();
        self.evaluator.evaluate(&samples, now_ms)
    }
}

/// 样本 → wire（定长）。
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

/// wire → 样本（未知质量/完整性码 → None，fail-closed）。
pub fn decode_sample(buf: &[u8]) -> Option<TelemetrySample> {
    if buf.len() != SAMPLE_WIRE_LEN {
        return None;
    }
    use eneros_safety_validator::{IntegrityStatus, SampleQuality};
    let quality = match buf[40] {
        0 => SampleQuality::Good,
        1 => SampleQuality::Suspect(eneros_safety_validator::QualityReason::Stale),
        2 => SampleQuality::Bad(eneros_safety_validator::QualityReason::Stale),
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

fn quality_code(q: eneros_safety_validator::SampleQuality) -> u8 {
    match q {
        eneros_safety_validator::SampleQuality::Good => 0,
        eneros_safety_validator::SampleQuality::Suspect(_) => 1,
        eneros_safety_validator::SampleQuality::Bad(_) => 2,
        eneros_safety_validator::SampleQuality::Unknown => 3,
    }
}

fn integrity_code(i: eneros_safety_validator::IntegrityStatus) -> u8 {
    match i {
        eneros_safety_validator::IntegrityStatus::Verified => 0,
        eneros_safety_validator::IntegrityStatus::Unverified => 1,
        eneros_safety_validator::IntegrityStatus::Unverifiable => 2,
    }
}

/// TrustedTelemetry → wire（定长 144B：state 4×8 + ts 8 + valid 8 + digest
/// 32 + generated 8 + epoch 8 + mapping 8 + 反馈样本 5×8）。
pub const TRUSTED_WIRE_LEN: usize = 144;

pub fn encode_trusted(t: &TrustedTelemetry) -> Vec<u8> {
    let mut out = Vec::with_capacity(TRUSTED_WIRE_LEN);
    out.extend_from_slice(&t.state.voltage_v.to_le_bytes());
    out.extend_from_slice(&t.state.current_a.to_le_bytes());
    out.extend_from_slice(&t.state.frequency_hz.to_le_bytes());
    out.extend_from_slice(&t.state.soc_pct.to_le_bytes());
    out.extend_from_slice(&t.state.timestamp_ms.to_le_bytes());
    out.extend_from_slice(&t.valid_until_ms.to_le_bytes());
    out.extend_from_slice(&t.provenance_digest);
    out.extend_from_slice(&t.generated_at_ms.to_le_bytes());
    out.extend_from_slice(&t.source_epoch.to_le_bytes());
    out.extend_from_slice(&t.mapping_version.to_le_bytes());
    // 反馈样本（grid_up=5 / soc=4 / dc_bus_v=1 / oc=6 / ot=6 约定点位；
    // 缺失 → 0.0 且由链按不可信裁决——服务装配以覆盖为准）。
    out.extend_from_slice(&t.samples.get(&5).copied().unwrap_or(f64::NAN).to_le_bytes());
    out.extend_from_slice(&t.samples.get(&4).copied().unwrap_or(f64::NAN).to_le_bytes());
    out.extend_from_slice(&t.samples.get(&1).copied().unwrap_or(f64::NAN).to_le_bytes());
    out.extend_from_slice(&t.samples.get(&6).copied().unwrap_or(f64::NAN).to_le_bytes());
    out.extend_from_slice(&t.samples.get(&6).copied().unwrap_or(f64::NAN).to_le_bytes());
    out
}

/// wire → TrustedTelemetry（长度不符 → None）。
pub fn decode_trusted(buf: &[u8]) -> Option<TrustedTelemetry> {
    use eneros_safety_validator::RequiredPointCoverage;
    if buf.len() != TRUSTED_WIRE_LEN {
        return None;
    }
    let mut off = 0usize;
    let take8 = |off: &mut usize| -> Option<f64> {
        let v = f64::from_le_bytes(buf[*off..*off + 8].try_into().ok()?);
        *off += 8;
        Some(v)
    };
    let voltage = take8(&mut off)?;
    let current = take8(&mut off)?;
    let freq = take8(&mut off)?;
    let soc = take8(&mut off)?;
    let ts = u64::from_le_bytes(buf[off..off + 8].try_into().ok()?);
    off += 8;
    let valid_until = u64::from_le_bytes(buf[off..off + 8].try_into().ok()?);
    off += 8;
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&buf[off..off + 32]);
    off += 32;
    let generated = u64::from_le_bytes(buf[off..off + 8].try_into().ok()?);
    off += 8;
    let epoch = u64::from_le_bytes(buf[off..off + 8].try_into().ok()?);
    off += 8;
    let mapping = u64::from_le_bytes(buf[off..off + 8].try_into().ok()?);
    off += 8;
    // 反馈样本（约定点位 5/4/1/6/6；NaN = 未覆盖）。
    let mut samples = alloc::collections::BTreeMap::new();
    let read_fb = |off: &mut usize| -> Option<f64> {
        let v = f64::from_le_bytes(buf[*off..*off + 8].try_into().ok()?);
        *off += 8;
        Some(v)
    };
    let grid = read_fb(&mut off)?;
    let soc_val = read_fb(&mut off)?;
    let dc = read_fb(&mut off)?;
    let oc = read_fb(&mut off)?;
    let ot = read_fb(&mut off)?;
    for (p, v) in [(5u32, grid), (4, soc_val), (1, dc), (6, oc)] {
        if v.is_finite() {
            samples.insert(p, v);
        }
    }
    if ot.is_finite() {
        samples.insert(6, ot);
    }
    Some(TrustedTelemetry {
        state: eneros_safety_validator::SystemState::new(voltage, current, freq, soc, ts),
        coverage: RequiredPointCoverage::empty(),
        valid_until_ms: valid_until,
        provenance_digest: digest,
        generated_at_ms: generated,
        source_epoch: epoch,
        mapping_version: mapping,
        samples,
    })
}

/// 服务请求处理（EIPC handler 入口）。
pub fn handle_request(svc: &mut TelemetryService, req: &[u8], now_ms: u64) -> Vec<u8> {
    let Some(&code) = req.first() else {
        return vec![0xFF]; // 空请求 → 明确错误
    };
    match code {
        TEL_INGEST => {
            let body = &req[1..];
            let mut samples = Vec::new();
            let mut ok = true;
            for chunk in body.chunks(SAMPLE_WIRE_LEN) {
                match decode_sample(chunk) {
                    Some(s) => samples.push(s),
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                return vec![0xFF];
            }
            // INGEST = 新采集周期全量替换（QEMU 实弹：累积多轮样本导致
            // 快照评估同点重复 Replay/跨周期 ClockNotMonotonic）。
            svc.clear();
            let accepted = svc.ingest(&samples);
            vec![0, accepted as u8]
        }
        TEL_SNAPSHOT => match svc.snapshot(now_ms) {
            Ok(t) => {
                let mut out = vec![SNAPSHOT_OK];
                out.extend_from_slice(&encode_trusted(&t));
                out
            }
            Err(_) => vec![SNAPSHOT_REJECTED],
        },
        TEL_ADVANCE_EPOCH => {
            svc.advance_epoch();
            vec![0]
        }
        _ => vec![0xFF],
    }
}

#[cfg(test)]
mod tests {
    use eneros_safety_validator::{
        EvaluatorConfig, IntegrityStatus, RequiredPoint, RequiredPointCoverage, SampleQuality,
        StateBindings,
    };

    use super::*;

    fn service() -> TelemetryService {
        TelemetryService::new(EvaluatorConfig {
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
        })
    }

    fn good(point: u32, value: f64, seq: u64, now: u64) -> TelemetrySample {
        TelemetrySample {
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
        }
    }

    fn full(now: u64) -> Vec<TelemetrySample> {
        vec![
            good(1, 380.0, 1, now),
            good(2, 10.0, 1, now),
            good(3, 50.0, 1, now),
            good(4, 0.6, 1, now),
            good(5, 1.0, 1, now),
            good(6, 0.0, 1, now),
        ]
    }

    #[test]
    fn ingest_snapshot_roundtrip() {
        let mut svc = service();
        let now = 10_000;
        let accepted = svc.ingest(&full(now));
        assert_eq!(accepted, 6);
        assert_eq!(svc.ingested, 6);
        let t = svc.snapshot(now).expect("trusted");
        assert_eq!(t.state.voltage_v, 380.0);
        assert_eq!(t.state.soc_pct, 0.6);
        assert!(t.is_valid_at(now + 29_999));
    }

    #[test]
    fn duplicate_sample_rejected() {
        let mut svc = service();
        let now = 10_000;
        svc.ingest(&full(now));
        // 同点同序号重放 → 拒绝（去重）。
        let dup = vec![good(1, 999.0, 1, now)];
        assert_eq!(svc.ingest(&dup), 0);
        assert_eq!(svc.rejected, 1);
    }

    #[test]
    fn epoch_advance_rejects_old_messages() {
        let mut svc = service();
        let now = 10_000;
        svc.ingest(&full(now));
        svc.advance_epoch();
        assert_eq!(svc.epoch(), 1);
        // 旧 epoch 样本 → 评估拒绝（结构化）。
        let mut s = full(20_000);
        for x in s.iter_mut() {
            x.sequence = 2;
        }
        svc.ingest(&s);
        assert!(matches!(
            svc.snapshot(20_000),
            Err(TrustRejection::SourceEpochMismatch { .. })
        ));
    }

    #[test]
    fn sample_wire_roundtrip() {
        let s = good(7, 12.5, 3, 1000);
        let buf = encode_sample(&s);
        let d = decode_sample(&buf).expect("decode");
        assert_eq!(d.point, 7);
        assert_eq!(d.value, 12.5);
        assert_eq!(d.sequence, 3);
        assert_eq!(d.quality, SampleQuality::Good);
        assert_eq!(d.integrity, IntegrityStatus::Verified);
    }

    #[test]
    fn trusted_wire_roundtrip() {
        let mut svc = service();
        let now = 10_000;
        svc.ingest(&full(now));
        let t = svc.snapshot(now).expect("trusted");
        let buf = encode_trusted(&t);
        let d = decode_trusted(&buf).expect("decode");
        assert_eq!(d.state.voltage_v, 380.0);
        assert_eq!(d.valid_until_ms, t.valid_until_ms);
        assert_eq!(d.provenance_digest, t.provenance_digest);
    }

    #[test]
    fn request_handler_routes() {
        let mut svc = service();
        let now = 10_000;
        // INGEST。
        let mut req = vec![TEL_INGEST];
        for s in full(now) {
            req.extend_from_slice(&encode_sample(&s));
        }
        let r = handle_request(&mut svc, &req, now);
        assert_eq!(r, vec![0, 6]);
        // SNAPSHOT。
        let r = handle_request(&mut svc, &[TEL_SNAPSHOT], now);
        assert_eq!(r[0], SNAPSHOT_OK);
        assert_eq!(r.len(), 1 + TRUSTED_WIRE_LEN);
        // 空请求 → 错误码。
        assert_eq!(handle_request(&mut svc, &[], now), vec![0xFF]);
    }
}
