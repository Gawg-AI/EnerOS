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
    WatermarkTracker,
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
    /// source → 首见 (epoch, seq) 钉扎（更低 epoch = 重放/回滚 →
    /// quarantine）——W3：WatermarkTracker 单源（首见 advance，钉值
    /// 不随 epoch 升级更新——adapter 重启后旧 epoch 重放仍被拦）。
    watermarks: WatermarkTracker,
    /// 事件汇（event_id 去重 + 同源 watermark 复核——ingestion-core 单源）。
    sink: InMemorySink,
    /// 隔离区（未注册/非法事件存档；控制 evaluator 零消费）。
    quarantine: QuarantineStore,
    /// 影子评估器（与生产同配置的独立实例；68.2 起 per-source epoch
    /// 自管——与生产评估器同律，无钉扎同步）。
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
    /// 部分批跳过数（批内未含全部必需点，68.2 起不评估不产拒绝——
    /// 可观测不静默）。
    pub shadow_uncovered: u64,
    /// 最近一次影子评估是否通过。
    last_shadow_ok: Option<bool>,
    /// 最近一次影子评估的逐点值（与生产 TrustedTelemetry.samples 对照）。
    last_shadow_values: Option<BTreeMap<u32, f64>>,
    /// 最近钉扎 (source, epoch)（首钉/升级观测面；68.2 起评估器
    /// per-source 自管，无生产同步消费——诊断/测试读面）。
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
            watermarks: WatermarkTracker::new(),
            sink: InMemorySink::new(SHADOW_BATCH_CAP * 4),
            quarantine: QuarantineStore::new(SHADOW_BATCH_CAP),
            shadow_evaluator: TelemetryTrustEvaluator::new(eval_config.clone()),
            accepted: 0,
            rejected: 0,
            quarantined: 0,
            shadow_match: 0,
            shadow_mismatch: 0,
            shadow_uncovered: 0,
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
    /// `converted`（v0.314.30 生产消费面出参）= 接受事件的旧样本视图
    /// （OBS = 生产轨；调用方 upsert 进生产缓冲）。
    /// `accepted_events`（68.4）：接受事件本体出参（OBS 事件面
    /// StoreSink 追加 + SnapshotModule 投影的唯一输入源——与 converted
    /// 同一接受集，逐事件对应）。
    pub fn ingest_frames(
        &mut self,
        _source_id: u32,
        frames: &[u8],
        now_ms: u64,
        converted: &mut Vec<TelemetrySample>,
        accepted_events: &mut Vec<ObservationEvent>,
    ) -> (usize, usize) {
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
            // epoch 钉扎（首见为准；回退 = 重放/回滚 → quarantine）——
            // W3：tracker 单源；判据算式唯一实现 = ingestion-core
            // WatermarkTracker::check_epoch_floor（63.3 WP-63.3.1——原
            // 手写回退比较删除）。首见 advance
            // 即钉（先于 sink 复核——与旧手写钉扎表现状一致：后续被
            // 去重拒绝的帧同样完成钉扎）；钉值不随 epoch 升级更新。
            let pinned = self.watermarks.pinned(event.source_id);
            let expected = pinned.map_or(event.source_epoch, |(e, _)| e);
            if let Err(reason) = WatermarkTracker::check_epoch_floor(event.source_epoch, expected) {
                self.quarantine(reason, Some(chunk));
                rejected += 1;
                continue;
            }
            let first_pin = pinned.is_none();
            if first_pin {
                self.watermarks.advance(&event);
            }
            // 到此域 = {==, >}（回退已被 check_epoch_floor 门拒）——
            // "新 epoch" = 不等。蓝图 68.2（F13）：影子评估器 per-source
            // epoch 自管（与生产评估器同改同律）——原
            // `restore(event.source_epoch, 0, 清态)` 钉扎同步删除（"最后
            // 钉扎者赢"毒化机制本体）；epoch 前跳（adapter 重启）由评估器
            // max 吸收，序号基线随 (source, epoch) 空间自动失效。
            if first_pin || event.source_epoch != expected {
                self.last_pinned = Some((event.source_id, event.source_epoch));
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
            accepted_events.push(event);
            if let Some(sample) = convert_to_legacy(&event) {
                batch.push(sample);
            }
        }
        // 影子评估（与生产同配置；结论只存对照面，零下游）。68.2：仅当
        // 批内已含全部必需点才评估——多源上行后单批只含单源点位，对全量
        // 覆盖面评估结构性 MissingPoints（QEMU 窗实弹 11 次，全为
        // MissingPoints、零 epoch 类），无对照意义 → 跳过并计数（不静默）。
        if !batch.is_empty() {
            if self.shadow_evaluator.covers_coverage(&batch) {
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
            } else {
                self.shadow_uncovered += 1;
            }
        }
        converted.extend(batch); // REG-70.15-R09-R6 根因修复（2026-09-15）：批后**排空**收件箱环。
                                 // 本环仅承担批内 event_id 去重；跨批回放/回退保护由 WatermarkTracker
                                 // 独立承载（`sequence <= last` 恒拒，永不排空）。旧语义无人 drain ⇒
                                 // cap=1024 处 Overflow 全拒 ⇒ 快照停建 ⇒ EA 恒 snapshot-stale
                                 // （vol89 R6 四次复现根因；宿主装配长窗同样会触达）。
        let _ = self.sink.drain_frames();

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

    /// 最近钉扎的 (source, epoch)（68.2 起为纯观测面——评估器 per-source
    /// 自管后无生产同步消费；None = 尚无 OBS 帧）。
    pub fn last_pinned(&self) -> Option<(u32, u64)> {
        self.last_pinned
    }
}

/// config `[[source]]` + 点表 → 影子 descriptor 集（bin 装配单源）。
///
/// `allowed_schemas` = 该来源点域内点表的已注册 schema 集合（0 除外）。
/// 收集/硬编码单源 `descriptor_of` 工厂（W3）；本处过滤口径 = **点表
/// 范围**（point_id ∈ [point_min, point_max]，上界 0 = 不设限）。
pub fn descriptors_from_config(
    sources: &[SourceRecord],
    points: &[PointRecord],
) -> Vec<SourceDescriptor> {
    sources
        .iter()
        .map(|s| {
            eneros_ingestion_core::descriptor_of(s, points, |p| {
                let in_range =
                    p.point_id >= s.point_min && (s.point_max == 0 || p.point_id <= s.point_max);
                if in_range {
                    Some(p.identity.schema_id)
                } else {
                    None
                }
            })
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

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use eneros_agent_sdk::synth_feed::{synth_frames, SYNTH_FEED};

    use super::*;

    /// 实弹 `configs/points.toml` 的宿主面编译（合成源注册与馈电元数据的
    /// 唯一事实 = 签名配置；测试直接锁真实仓库配置，不设影子夹具）。
    fn live_points_table() -> eneros_config_bundle::points::PointsTable {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../configs/points.toml");
        let text = std::fs::read_to_string(path).expect("live configs/points.toml");
        let compiled = eneros_config_bundle::toml_compile::compile_points(&text)
            .expect("live points.toml must compile");
        eneros_config_bundle::points::decode_points(&compiled.payload, &compiled.config_id)
            .expect("decode compiled points")
    }

    /// REG-70.8-SYNTH-NO-OBS 红测（注册面）：合成源（source 1）必须注册为
    /// OBS 生产轨来源，且授权面覆盖合成馈电声明点跨度。
    #[test]
    fn synth_source_is_registered_for_obs_track() {
        let table = live_points_table();
        let src = table
            .sources
            .iter()
            .find(|s| s.source_id == 1)
            .expect("source 1（root 合成源）必须注册为 OBS 来源——否则合成装配的快照链不可证");
        assert!(
            src.point_min <= 1 && (src.point_max == 0 || src.point_max >= 1009),
            "来源 1 授权面须覆盖合成馈电声明点跨度 1..=1009（含 PCS-2 回读 1005），实际 {:?}",
            (src.point_min, src.point_max)
        );
        assert!(
            src.allow_control,
            "合成馈电必须允许进控制快照（测试装配主链）"
        );
        assert!(src.require_integrity, "合成源与生产来源同完整性纪律");
    }

    /// REG-70.8-SYNTH-NO-OBS 红测（受理面）：合成馈电批次经真实 shadow
    /// 全受理，转换集覆盖快照必收点集（1..7 + 1001..1004）。
    #[test]
    fn synth_obs_frames_are_accepted_end_to_end() {
        let table = live_points_table();
        let descriptors = descriptors_from_config(&table.sources, &table.points);
        let eval = crate::telemetry::evaluator_config(1, alloc::vec::Vec::new());
        let mut shadow = ObsShadow::new(descriptors, eval);
        let flat: alloc::vec::Vec<u8> = synth_frames(0, 1_000_000_000).concat();
        let mut converted = alloc::vec::Vec::new();
        let mut accepted_events = alloc::vec::Vec::new();
        let (accepted, rejected) =
            shadow.ingest_frames(1, &flat, 2_000, &mut converted, &mut accepted_events);
        assert_eq!(
            (accepted, rejected),
            (SYNTH_FEED.len(), 0),
            "合成源 OBS 批次必须全受理（红 = UnregisteredSource 隔离）"
        );
        let pts: std::collections::BTreeSet<u32> = converted.iter().map(|s| s.point).collect();
        for p in [1u32, 2, 3, 4, 5, 6, 7, 1001] {
            assert!(pts.contains(&p), "快照必收点 {p} 不在合成馈电转换集");
        }
    }

    /// REG-70.15-R09-R6 根因红测（2026-09-15）：影子收件箱（cap=1024）在
    /// 无采集上传半环的装配（qemu-test-synth）必须不楔死——**消费后排空**
    /// 语义。旧实现无人 drain ⇒ 第 ~94 批起 `Overflow` 全拒 ⇒ 快照停建 ⇒
    /// EA 恒 snapshot-stale ⇒ 提案链停（vol89 R6 四次复现的根因）。
    /// 变异靶：去掉 obs_ingest 的 drain_frames 调用 → 本测红。
    #[test]
    fn obs_ingest_does_not_wedge_at_sink_capacity() {
        let table = live_points_table();
        let descriptors = descriptors_from_config(&table.sources, &table.points);
        let eval = crate::telemetry::evaluator_config(1, alloc::vec::Vec::new());
        let mut shadow = ObsShadow::new(descriptors, eval);
        // 100 批 × 12 事件 = 1200 > cap 1024：全程必须零拒绝。
        let mut total_accepted = 0usize;
        let mut total_rejected = 0usize;
        for batch in 0..100u64 {
            let flat: alloc::vec::Vec<u8> = synth_frames(batch, 1_000_000_000 + batch).concat();
            let mut converted = alloc::vec::Vec::new();
            let mut accepted_events = alloc::vec::Vec::new();
            let (a, r) = shadow.ingest_frames(
                1,
                &flat,
                2_000 + batch,
                &mut converted,
                &mut accepted_events,
            );
            total_accepted += a;
            total_rejected += r;
        }
        assert_eq!(
            (total_accepted, total_rejected),
            (1200, 0),
            "影子收件箱不得在容量处楔死（Overflow = 消费后排空缺失）"
        );
    }

    /// 镜像锁：synth_feed 馈电表逐点对照真实配置声明面（device/unit/asset/
    /// schema）——配置改版而馈电未跟 = 红（root 不持配置，镜像一致性靠本锁）。
    #[test]
    fn synth_feed_mirror_locks_live_config() {
        let table = live_points_table();
        assert_eq!(
            SYNTH_FEED.len(),
            12,
            "馈电声明点全集 = 网关 1..7 + PCS-1 1001..1004 + PCS-2 1005"
        );
        for sp in SYNTH_FEED.iter() {
            let rec = table
                .points
                .iter()
                .find(|p| p.point_id == sp.point_id)
                .unwrap_or_else(|| panic!("馈电点 {} 未在 live 配置声明", sp.point_id));
            assert_eq!(
                rec.device_id, sp.device_id,
                "点 {} device 漂移",
                sp.point_id
            );
            assert_eq!(
                rec.identity.unit_code, sp.unit_code,
                "点 {} unit_code 漂移",
                sp.point_id
            );
            assert_eq!(
                rec.identity.asset_id, sp.asset_id,
                "点 {} asset 漂移",
                sp.point_id
            );
            assert_eq!(
                rec.identity.schema_id,
                eneros_agent_sdk::synth_feed::SYNTH_SCHEMA_ID,
                "点 {} schema 漂移",
                sp.point_id
            );
        }
        let src = table
            .sources
            .iter()
            .find(|s| s.source_id == 1)
            .expect("source 1 行存在");
        assert_eq!(
            src.protocol,
            eneros_config_bundle::points::PointProtocol::Synth,
            "source 1 协议必须是 synth（身份标记）"
        );
    }
}
