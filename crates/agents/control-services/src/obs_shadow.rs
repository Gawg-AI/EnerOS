//! 蓝图 61 v0.314.27 §2.5.5：观测事件影子评估（svc-telemetry）。
//!
//! V1 `ObservationEvent`（128B wire）经 [`ObsShadow::ingest_frames`] 准入：
//! 来源注册核验（config `[[source]]` 编译）→ epoch 钉扎回退检测 →
//! `InMemorySink`（event_id 去重 + sequence 单调复核）→ 转换为旧
//! `TelemetrySample` 视图 → **影子评估器**（与生产同配置的独立实例）评估。
//!
//! **零控制消费**：影子 `TrustedTelemetry` 只用于与生产 verdict 对照
//! （[`ObsShadow::compare_production`]，snapshot 期调用），计数 + 串口行
//! 外不进入任何下游——生产 snapshot 只读生产评估器/缓冲。未注册事件可存
//! quarantine（类型隔离：取出的只是审计元组）。
//!
//! 兼容轨：旧 58B `TelemetrySample` INGEST 路径不动（[06] §2.5.4
//! "保持旧 TelemetrySample 兼容读"）；影子路径与本模块共存至 .30 切换。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_config_bundle::points::PointRecord;
use eneros_config_bundle::points::SourceRecord;
use eneros_ingestion_core::{
    InMemorySink, IngestReject, ObservationSink, QuarantineStore, SourceDescriptor, SourceRegistry,
};
use eneros_safety_validator::{
    EvaluatorConfig, IntegrityStatus, SampleQuality, TelemetrySample, TelemetryTrustEvaluator,
    TrustedTelemetry,
};
use eneros_telemetry_model::event::{quality_bits, IntegrityTag, ObservationEvent};

/// 影子评估器样本缓冲上限（每批转换样本；与生产 MAX_BUFFERED_SAMPLES 同量级）。
const SHADOW_BATCH_CAP: usize = 256;

/// 观测影子（svc-telemetry 持有；None = 影子路径未装配）。
pub struct ObsShadow {
    /// 已注册来源（config `[[source]]` 编译 → descriptor）。
    registry: SourceRegistry,
    /// source → 首见 epoch（更低 epoch = 重放/回滚 → quarantine）。
    pinned_epochs: BTreeMap<u32, u64>,
    /// 事件汇（event_id 去重 + 同源 watermark 复核——ingestion-core 单源）。
    sink: InMemorySink,
    /// 隔离区（未注册/非法事件存档；控制 evaluator 零消费）。
    quarantine: QuarantineStore,
    /// 影子评估器（与生产同配置的独立实例；epoch 经 restore 对齐钉扎值）。
    shadow_evaluator: TelemetryTrustEvaluator,
    // —— 计数（可观测；不静默）——
    /// 影子接受事件数。
    pub accepted: u64,
    /// 影子拒绝事件数（decode/注册/epoch/去重/单调）。
    pub rejected: u64,
    /// quarantine 现存条数（累计走 [`Self::quarantine_total`]）。
    pub quarantined: u64,
    /// 影子 vs 生产 verdict 一致次数。
    pub shadow_match: u64,
    /// 影子 vs 生产 verdict 分歧次数（不静默，串口行）。
    pub shadow_mismatch: u64,
    /// 最近一次影子评估是否通过。
    last_shadow_ok: Option<bool>,
    /// 最近一次影子评估的逐点值（与生产 TrustedTelemetry.samples 对照）。
    last_shadow_values: Option<BTreeMap<u32, f64>>,
    /// 最近钉扎 (source, epoch)（生产评估器同步源）。
    last_pinned: Option<(u32, u64)>,
}

impl ObsShadow {
    /// 装配（descriptor 集合 + 与生产一致的评估器配置）。
    pub fn new(descriptors: Vec<SourceDescriptor>, eval_config: EvaluatorConfig) -> Self {
        let mut registry = SourceRegistry::new();
        for d in descriptors {
            registry.register(d, 0);
        }
        Self {
            registry,
            pinned_epochs: BTreeMap::new(),
            sink: InMemorySink::new(SHADOW_BATCH_CAP * 4),
            quarantine: QuarantineStore::new(SHADOW_BATCH_CAP),
            shadow_evaluator: TelemetryTrustEvaluator::new(eval_config.clone()),
            accepted: 0,
            rejected: 0,
            quarantined: 0,
            shadow_match: 0,
            shadow_mismatch: 0,
            last_shadow_ok: None,
            last_shadow_values: None,
            last_pinned: None,
        }
    }

    /// 来源是否已注册（config 编译面核验）。
    pub fn is_registered(&self, source_id: u32) -> bool {
        self.registry.is_registered(source_id)
    }

    /// OBS 批次准入（wire = count × 128B 帧）。
    ///
    /// 返回 (accepted, rejected)；一切拒绝进 quarantine（有界）且计数。
    pub fn ingest_frames(&mut self, _source_id: u32, frames: &[u8], now_ms: u64) -> (usize, usize) {
        let mut accepted = 0usize;
        let mut rejected = 0usize;
        let mut batch: Vec<TelemetrySample> = Vec::new();
        for chunk in frames.chunks(eneros_telemetry_model::OBSERVATION_WIRE_LEN) {
            if chunk.len() != eneros_telemetry_model::OBSERVATION_WIRE_LEN {
                self.quarantine(IngestReject::InvalidEvent, None);
                rejected += 1;
                continue;
            }
            let event = match ObservationEvent::decode_v1(chunk) {
                Ok(e) => e,
                Err(_) => {
                    self.quarantine(IngestReject::InvalidEvent, None);
                    rejected += 1;
                    continue;
                }
            };
            // 来源注册核验（影子侧独立复核——adapter 伪造/误配不放大）。
            if !self.registry.is_registered(event.source_id) {
                let reason = IngestReject::UnregisteredSource {
                    source_id: event.source_id,
                };
                self.quarantine(reason, Some(chunk));
                rejected += 1;
                continue;
            }
            // epoch 钉扎（首见为准；回退 = 重放/回滚 → quarantine）。
            let first_pin = !self.pinned_epochs.contains_key(&event.source_id);
            let expected = *self
                .pinned_epochs
                .entry(event.source_id)
                .or_insert(event.source_epoch);
            if event.source_epoch < expected {
                let reason = IngestReject::EpochRegression {
                    epoch: event.source_epoch,
                    expected,
                };
                self.quarantine(reason, Some(chunk));
                rejected += 1;
                continue;
            }
            if first_pin || event.source_epoch > expected {
                // 首钉 / 新 epoch（adapter 重启）：影子评估器 epoch 对齐
                // （restore = 生产评估器同步语义，epoch 变更即新序号空间）。
                self.last_pinned = Some((event.source_id, event.source_epoch));
                self.shadow_evaluator
                    .restore(event.source_epoch, 0, BTreeMap::new());
            }
            // sink 复核（event_id 去重 + 同源 sequence 单调；真 watermark
            // 语义与生产切换共用同一实现——ingestion-core 单源）。
            if let Err(reason) = self.sink.append(&event) {
                self.quarantine(reason, Some(chunk));
                rejected += 1;
                continue;
            }
            self.accepted += 1;
            accepted += 1;
            if let Some(sample) = convert_to_legacy(&event) {
                batch.push(sample);
            }
        }
        // 影子评估（与生产同配置；结论只存对照面，零下游）。
        if !batch.is_empty() {
            match self.shadow_evaluator.evaluate(&batch, now_ms) {
                Ok(t) => {
                    self.last_shadow_ok = Some(true);
                    self.last_shadow_values = Some(t.samples.clone());
                }
                Err(e) => {
                    self.last_shadow_ok = Some(false);
                    self.last_shadow_values = None;
                    eneros_runtime::println!("[svc-telemetry] OBS-SHADOW eval reject: {e:?}");
                }
            }
        }
        (accepted, rejected)
    }

    /// snapshot 期对照：生产 verdict vs 影子 verdict（计数 + 串口行；
    /// **零控制消费**——影子结果不参与生产裁决）。
    pub fn compare_production(&mut self, prod: &TrustedTelemetry) {
        let Some(shadow_ok) = self.last_shadow_ok else {
            return; // 影子尚无结论（无 OBS 批次）→ 不对照不计分歧。
        };
        let Some(shadow_values) = &self.last_shadow_values else {
            if shadow_ok {
                // 影子通过但无样本可比（非 F64 事件批）→ 只比 verdict。
                self.shadow_match += 1;
                eneros_runtime::println!("[svc-telemetry] OBS-SHADOW MATCH verdict-only");
            }
            return;
        };
        if !shadow_ok {
            self.shadow_mismatch += 1;
            eneros_runtime::println!(
                "[svc-telemetry] OBS-SHADOW MISMATCH reason=shadow-rejected-prod-accepted"
            );
            return;
        }
        // 逐点值对照（交集；位级一致 = 语义等价）。
        let mut compared = 0usize;
        let mut mismatched = 0usize;
        for (point, sv) in shadow_values {
            if let Some(pv) = prod.samples.get(point) {
                compared += 1;
                if sv.to_bits() != pv.to_bits() {
                    mismatched += 1;
                }
            }
        }
        if compared > 0 && mismatched == 0 {
            self.shadow_match += 1;
            eneros_runtime::println!("[svc-telemetry] OBS-SHADOW MATCH points={compared}");
        } else {
            self.shadow_mismatch += 1;
            eneros_runtime::println!(
                "[svc-telemetry] OBS-SHADOW MISMATCH compared={compared} mismatched={mismatched}"
            );
        }
    }

    fn quarantine(&mut self, reason: IngestReject, wire: Option<&[u8]>) {
        self.quarantined += 1;
        let mut buf = [0u8; eneros_telemetry_model::OBSERVATION_WIRE_LEN];
        if let Some(w) = wire {
            if w.len() == buf.len() {
                buf.copy_from_slice(w);
            }
        }
        self.quarantine.push(reason, &buf);
    }

    /// quarantine 累计计数（观测）。
    pub fn quarantine_total(&self) -> u64 {
        self.quarantined
    }

    /// quarantine 现存条数（审计读面；控制路径无此入口）。
    pub fn quarantine_len(&self) -> usize {
        self.quarantine.len()
    }

    /// 最近影子 verdict（诊断读面）。
    pub fn last_shadow_ok(&self) -> Option<bool> {
        self.last_shadow_ok
    }

    /// 最近钉扎的 (source, epoch)（供生产评估器 epoch 同步——观测身份
    /// 与 registry 分配同源；None = 尚无 OBS 帧）。
    pub fn last_pinned(&self) -> Option<(u32, u64)> {
        self.last_pinned
    }
}

/// config `[[source]]` + 点表 → 影子 descriptor 集（bin 装配单源）。
///
/// `allowed_schemas` = 该来源点域内点表的已注册 schema 集合（0 除外）。
pub fn descriptors_from_config(
    sources: &[SourceRecord],
    points: &[PointRecord],
) -> Vec<SourceDescriptor> {
    sources
        .iter()
        .map(|s| {
            let mut schemas: Vec<u32> = Vec::new();
            for p in points {
                let in_range =
                    p.point_id >= s.point_min && (s.point_max == 0 || p.point_id <= s.point_max);
                if in_range && p.identity.schema_id != 0 && !schemas.contains(&p.identity.schema_id)
                {
                    schemas.push(p.identity.schema_id);
                }
            }
            SourceDescriptor {
                source_id: s.source_id,
                tenant_id: s.tenant_id,
                site_id: s.site_id,
                allow_control: s.allow_control,
                require_integrity: s.require_integrity,
                point_range: (s.point_min, s.point_max),
                max_events_per_s: s.max_events_per_s,
                allowed_schemas: schemas,
                clock_id: 1,
                enabled: true,
            }
        })
        .collect()
}

/// ObservationEvent → 旧 TelemetrySample 视图（影子对照专用；非 F64 值
/// 事件返回 None——旧评估器是浮点模型，[06] §2.5.4 兼容读语义）。
pub fn convert_to_legacy(ev: &ObservationEvent) -> Option<TelemetrySample> {
    let ObservationValueRef::F64(v) = value_ref(&ev.value) else {
        return None;
    };
    let quality = if ev.quality_bits & quality_bits::VALID != 0 {
        if ev.quality_bits
            & (quality_bits::INVALID | quality_bits::BLOCKED | quality_bits::OVERFLOW)
            == 0
        {
            if ev.quality_bits & (quality_bits::UNCERTAIN | quality_bits::SUBSTITUTED) != 0 {
                SampleQuality::Suspect(eneros_safety_validator::QualityReason::CapabilityMissing)
            } else {
                SampleQuality::Good
            }
        } else {
            SampleQuality::Bad(eneros_safety_validator::QualityReason::CapabilityMissing)
        }
    } else {
        SampleQuality::Bad(eneros_safety_validator::QualityReason::CapabilityMissing)
    };
    let integrity = match ev.integrity {
        IntegrityTag::Unverified => IntegrityStatus::Unverified,
        IntegrityTag::Authenticated | IntegrityTag::Signed | IntegrityTag::LocalTrustedEdge => {
            IntegrityStatus::Verified
        }
    };
    Some(TelemetrySample {
        point: ev.point_id,
        value: v,
        source: ev.source_id,
        source_epoch: ev.source_epoch,
        measured_at_ms: ev.event_time_ns / 1_000_000,
        received_at_ms: ev.ingest_time_ns / 1_000_000,
        quality,
        sequence: ev.sequence,
        integrity,
        mapping_version: ev.mapping_version,
    })
}

/// ObservationValue 的借用视图（避免 Copy 约束噪音）。
enum ObservationValueRef {
    F64(f64),
    Other,
}

fn value_ref(v: &eneros_telemetry_model::event::ObservationValue) -> ObservationValueRef {
    use eneros_telemetry_model::event::ObservationValue as V;
    match *v {
        V::F64(f) => ObservationValueRef::F64(f),
        _ => ObservationValueRef::Other,
    }
}
