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

use alloc::boxed::Box;
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

/// 45.5：58B 定长样本 wire 收归 agent-sdk `sample_wire`（单一事实；
/// root/svc-telemetry/协议服务共同消费）。此处 re-export 保持既有调用面。
pub use eneros_agent_sdk::sample_wire::{
    decode_sample, encode_sample, integrity_code, quality_code, SAMPLE_WIRE_LEN,
};
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
    /// WP-2：历史落盘接缝（None=未装配，行为与旧版完全一致）。
    history: Option<Box<dyn TelemetryHistorySink>>,
    /// WP-2：成功写出的历史批次数（观测）。
    pub history_written: u64,
    /// WP-2：历史缓冲满被 DropOldest 丢弃的记录数（观测，不静默）。
    pub history_dropped: u64,
    /// WP-2：sink 写出失败的批次数（观测；不影响 ingest ack）。
    pub history_errors: u64,
}

impl TelemetryService {
    /// 构造（评估器配置）。
    pub fn new(config: EvaluatorConfig) -> Self {
        Self {
            evaluator: TelemetryTrustEvaluator::new(config),
            buffer: VecDeque::new(),
            ingested: 0,
            rejected: 0,
            history: None,
            history_written: 0,
            history_dropped: 0,
            history_errors: 0,
        }
    }

    /// 装配历史落盘接缝（WP-2；服务装配期调用一次）。
    pub fn set_history_sink(&mut self, sink: Box<dyn TelemetryHistorySink>) {
        self.history = Some(sink);
    }

    /// 批次边界 flush：把当前缓冲（即本轮 ingest 接受集）写往历史接缝。
    ///
    /// 失败语义 fail-visible：错误只计数、不吞掉、也**不反向影响**
    /// ingest 的 ack——采集与历史是两条独立生死线。无 sink 时为零开销
    /// 直通（与未接线版本行为一致）。
    fn flush_history(&mut self) {
        let Some(sink) = self.history.as_mut() else {
            return;
        };
        let batch: Vec<HistoryRecord> = self
            .buffer
            .iter()
            .map(|s| HistoryRecord {
                timestamp_ms: s.measured_at_ms,
                device_id: s.source,
                point: s.point,
                value: s.value,
            })
            .collect();
        if batch.is_empty() {
            return;
        }
        match sink.write_batch(&batch) {
            Ok(()) => {
                self.history_written += 1;
                // SPEC-GAP-CLOSURE WP-2/C2：QEMU 串口证据行（grep 连续性纪律）。
                eneros_runtime::println!("[svc-telemetry] TSDB-COMMIT batch items={}", batch.len());
            }
            Err(()) => {
                self.history_errors += 1;
                eneros_runtime::println!(
                    "[svc-telemetry] TSDB-COMMIT-FAIL errors={}",
                    self.history_errors
                );
            }
        }
        // DropOldest 计数由具体缓冲实现内部累计后经 here 汇报：
        // 为避免 trait 面膨胀，丢弃统计下沉到 HistoryBuffer 自身字段，
        // 服务侧只计批次级成败（见模块尾设计注记）。
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

// ============================================================================
// SPEC-GAP-CLOSURE-2026-08-26 · WP-2：历史落盘接缝（host 波）
//
// 磁盘 TSDB（eneros-tsdb）绑定 eneros_fs::Lfs × BlockDevice，而目标镜像中
// 块句柄仅由 svc-storage 持有（41 卷移交先例）——svc-telemetry 的磁盘挂载
// 架构属 owner 裁决项 D-TSDB-2，本波不擅自开辟访问路径。落地形状：
// - [`TelemetryHistorySink`]：批次写接缝（磁盘适配器将来即插即用）；
// - [`HistoryBuffer`]：缺省内存环回放缓冲（容量守恒、DropOldest 可观测），
//   字段与 tsdb 的 TimeSeriesPoint 对齐以便后续直映射。
// 内存纪律（R2）：缺省容量 512 条 ×32B ≈16KiB ≪ 服务堆预算；扩容 MUST
// 先出静态内存审计表。
// ============================================================================

/// 缺省历史容量（条）。
pub const DEFAULT_HISTORY_CAP: usize = 512;

/// 单条历史记录（plain-data；字段语义对齐 eneros-tsdb TimeSeriesPoint）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HistoryRecord {
    /// 测量时刻（单调毫秒，取样本 measured_at_ms）。
    pub timestamp_ms: u64,
    /// 来源实例 ID（取样本 source；磁盘适配器映射 DeviceId 用）。
    pub device_id: u32,
    /// 统一点 ID。
    pub point: u32,
    /// 采样值。
    pub value: f64,
}

/// 历史落盘接缝：每轮 TEL_INGEST 接受后调用一次 `write_batch`。
///
/// 实现方约定：失败返回 `Err(())` 即可（服务侧计数），不得 panic、不得
/// 阻塞采集路径。磁盘适配器（D-TSDB-2 后）与本内存环均实现本 trait。
pub trait TelemetryHistorySink {
    // 二元失败是接缝契约（服务侧只计成败、不解释原因）；错误类型留空是
    // 刻意为之，非占位——历史路径不向采集链回传细节。
    #[allow(clippy::result_unit_err)]
    fn write_batch(&mut self, batch: &[HistoryRecord]) -> Result<(), ()>;
}

/// 有界历史缓冲（内存环；满时 DropOldest + 计数——不静默，语义对齐
/// drivers/tsdb ring.rs / 22 §2.W3.3）。
pub struct HistoryBuffer {
    buf: VecDeque<HistoryRecord>,
    cap: usize,
    /// 满'后丢弃的记录总数（指标源数据）。
    pub dropped: u64,
}

impl HistoryBuffer {
    /// 新建（cap=0 回退 1——不允许不可用缓冲，同 tsdb ring 纪律）。
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            buf: VecDeque::new(),
            dropped: 0,
        }
    }

    /// 当前记录数。
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// 读回最近 n 条（时间升序；n > len 时给全量）。重启持久性依赖
    /// D-TSDB-2 落地前的进程内排障/对账通道。
    pub fn last(&self, n: usize) -> Vec<HistoryRecord> {
        let start = self.buf.len().saturating_sub(n);
        self.buf.iter().skip(start).copied().collect()
    }
}

impl TelemetryHistorySink for HistoryBuffer {
    fn write_batch(&mut self, batch: &[HistoryRecord]) -> Result<(), ()> {
        for r in batch {
            if self.buf.len() >= self.cap {
                self.buf.pop_front();
                self.dropped += 1;
            }
            self.buf.push_back(*r);
        }
        Ok(())
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
            // WP-2（SPEC-GAP-CLOSURE）：批次边界落历史——磁盘 TSDB 适配器
            // 待 D-TSDB-2 架构裁决（块句柄现仅 svc-storage 持有），当前以
            // HistoryBuffer 内存环为缺省实现。
            svc.flush_history();
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

    // ===== SPEC-GAP-CLOSURE-2026-08-26 · WP-2 host 波判据 =====

    use super::{HistoryBuffer, HistoryRecord, TelemetryHistorySink, DEFAULT_HISTORY_CAP};

    /// 构造样本（field 布局对齐 safety-validator::TelemetrySample）。
    fn sample(point: u32, value: f64, ts_ms: u64) -> TelemetrySample {
        TelemetrySample {
            point,
            value,
            source: 7,
            source_epoch: 0,
            measured_at_ms: ts_ms,
            received_at_ms: ts_ms + 1,
            quality: eneros_safety_validator::SampleQuality::Good,
            sequence: 0,
            integrity: eneros_safety_validator::IntegrityStatus::Verified,
            mapping_version: 0,
        }
    }

    fn rec(id: u64) -> HistoryRecord {
        HistoryRecord {
            timestamp_ms: id * 100,
            device_id: 7,
            point: id as u32,
            value: id as f64,
        }
    }

    /// WP-2/C1-a：批次边界 flush——ingest 后当前缓冲整批写往 sink，
    /// 字段逐项映射正确（point/value/measured_at→timestamp/source→device）。
    #[test]
    fn wp2_ingest_flush_routes_batch_to_sink() {
        use alloc::rc::Rc;
        use core::cell::RefCell;

        #[derive(Default)]
        struct ProbeSink(Rc<RefCell<Vec<HistoryRecord>>>);
        impl TelemetryHistorySink for ProbeSink {
            fn write_batch(&mut self, b: &[HistoryRecord]) -> Result<(), ()> {
                self.0.borrow_mut().extend_from_slice(b);
                Ok(())
            }
        }

        let captured = Rc::new(RefCell::new(Vec::new()));
        let mut svc = service();
        svc.set_history_sink(Box::new(ProbeSink(captured.clone())));
        let batch = [
            sample(1, 11.5, 1_000),
            sample(4, 0.9, 1_100),
            sample(5, 22.0, 1_200),
        ];
        svc.clear();
        let accepted = svc.ingest(&batch);
        assert_eq!(accepted, 3);
        svc.flush_history();

        assert_eq!(svc.history_written, 1);
        assert_eq!(svc.history_errors, 0);
        let got = captured.borrow().clone();
        assert_eq!(got.len(), 3);
        let want: Vec<(u64, u32, u32, u64)> =
            [(1_000, 7, 1, 11), (1_100, 7, 4, 12), (1_200, 7, 5, 22)]
                .iter()
                .map(|&(ts, dev, pt, _)| (ts, dev, pt, pt as u64))
                .collect();
        for (r, &(ts, dev, pt, _)) in got.iter().zip(want.iter()) {
            assert_eq!((r.timestamp_ms, r.device_id, r.point), (ts, dev, pt));
        }
        assert_eq!(got[0].value, 11.5);
        assert_eq!(got[1].value, 0.9);
        assert_eq!(got[2].value, 22.0);
    }

    /// WP-2/C1-b：ring 满 → DropOldest + 计数可见；读回升序一致。
    #[test]
    fn wp2_history_ring_dropoldest_visible_and_readback_consistent() {
        let mut ring = HistoryBuffer::new(4);
        for i in 1..=6u64 {
            let _ = ring.write_batch(&[rec(i)]);
        }
        assert_eq!(ring.len(), 4);
        assert_eq!(ring.dropped, 2, "DropOldest 必须计数（不静默）");
        let got = ring.last(10);
        let want: Vec<u32> = (3..=6).map(|v| v as u32).collect();
        assert_eq!(
            got.iter().map(|r| r.point).collect::<Vec<_>>(),
            want,
            "读回 = 最近 4 条且时间升序"
        );
        // cap=0 回退 1（不允许不可用缓冲）。
        let mut zero = HistoryBuffer::new(0);
        zero.write_batch(&[rec(1)]).unwrap();
        assert_eq!(zero.len(), 1);
    }

    /// WP-2/C1-c：sink 失败 fail-visible——错误计数、ingest ack 不受影响、
    /// 不 panic。
    #[test]
    fn wp2_sink_error_counted_never_blocks_ingest() {
        struct FailSink;
        impl TelemetryHistorySink for FailSink {
            fn write_batch(&mut self, _b: &[HistoryRecord]) -> Result<(), ()> {
                Err(())
            }
        }
        let mut svc = service();
        svc.set_history_sink(Box::new(FailSink));
        svc.clear();
        svc.ingest(&[sample(1, 5.0, 100)]);
        svc.flush_history();
        assert_eq!(svc.history_errors, 1);
        assert_eq!(svc.history_written, 0);
        // 未装配 sink 时零开销直通（行为与未接线版本一致）。
        let mut plain = service();
        plain.clear();
        plain.ingest(&[sample(1, 5.0, 100)]);
        plain.flush_history();
        assert_eq!(plain.history_written, 0);
        assert_eq!(plain.history_errors, 0);
        assert!(
            DEFAULT_HISTORY_CAP * core::mem::size_of::<HistoryRecord>() <= 32 * 1024,
            "缺省内存环必须守 R2 预算（≤32KiB）；扩容先出静态审计表"
        );
    }
}
