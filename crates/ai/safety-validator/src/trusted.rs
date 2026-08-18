//! 蓝图 40 WP-40.1：可信遥测（`TrustedTelemetry`）——控制输入的信任边界。
//!
//! 采集链的原始样本（[`TelemetrySample`]）必须经 [`TelemetryTrustEvaluator`]
//! 逐项校验后才可进入决策/控制链：
//!
//! 1. required points 全覆盖；
//! 2. 时间单调、freshness（`max_age_ms`）、max skew；
//! 3. source 白名单与 source epoch（断线重连后旧 epoch 拒绝）；
//! 4. 质量（Good 才可用）、范围、单位（经映射版本强制）、NaN/Inf；
//! 5. sequence/replay（重复/乱序拒绝）；
//! 6. 多源冲突（同点多源严格拒绝，不猜优先级）；
//! 7. 设备/点映射版本（mapping version 不匹配拒绝）；
//! 8. integrity/authentication（要求 Verified 时未验证样本拒绝）。
//!
//! 输出只能是 [`TrustedTelemetry`] 或结构化 [`TrustRejection`]——缺数据时
//! 允许维持最后安全输出/进入 safe mode，但**不得**制造"正常默认状态"
//! 继续发新控制（P0-09 修复：`SystemState` 已无 `Default`，见 [`super::state`]）。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_upa_model::point::PointId;

use super::state::SystemState;

/// 样本质量（蓝图 40 模型；命名避免与已废弃的 3 态 `Quality` 混淆，
/// 且与 upa-model `PointQuality` 为不同层——本类型是可信评估层的结论）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleQuality {
    /// 好品质（唯一可进入控制链的品质）。
    Good,
    /// 可疑（存在风险但未定论）——控制输入拒绝。
    Suspect(QualityReason),
    /// 坏品质——拒绝。
    Bad(QualityReason),
    /// 未评估/未知——拒绝。
    Unknown,
}

impl SampleQuality {
    /// 是否可用于控制（仅 `Good`）。
    pub fn is_usable(&self) -> bool {
        matches!(self, SampleQuality::Good)
    }
}

/// 质量降级原因（可枚举、可审计）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityReason {
    /// 超过最大新鲜度窗口。
    Stale,
    /// 超出量程。
    OutOfRange,
    /// 非有限值（NaN/Inf）。
    NotFinite,
    /// 重放/乱序。
    Replay,
    /// 多源冲突。
    SourceConflict,
    /// 点映射版本过期。
    MappingStale,
    /// 量纲不符（由映射版本契约强制）。
    BadUnit,
    /// 完整性/认证失败。
    IntegrityFailure,
    /// 来源能力不足。
    CapabilityMissing,
    /// 来源 epoch 不匹配（断线重连）。
    SourceEpochMismatch,
}

/// 样本完整性状态（采集链认证结果；`require_integrity` 时仅 `Verified`
/// 可进入控制链）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityStatus {
    /// 已认证（采集链 HMAC/签名/会话认证通过）。
    Verified,
    /// 未认证（无认证链覆盖）。
    Unverified,
    /// 不可验证（认证机制缺失/降级）。
    Unverifiable,
}

/// 采样值单位（映射版本契约的一部分——点必须按声明单位上报）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointUnit {
    Volt,
    Ampere,
    Hertz,
    /// 无量纲比例（SOC 0.0~1.0）。
    Ratio,
    Percent,
    Watt,
    Kilowatt,
    Other,
}

/// 单点采集样本（原始输入；由 TelemetryService 从 39 ProtocolPlugin/
/// DriverServiceHandle 订阅构造）。
#[derive(Debug, Clone, Copy)]
pub struct TelemetrySample {
    /// 统一点 ID（upa-model）。
    pub point: PointId,
    /// 采样值（声明单位，见 RequiredPoint.unit）。
    pub value: f64,
    /// 来源 ID（采集链实例）。
    pub source: u32,
    /// 来源 epoch（采集链断线重连计数；旧 epoch 样本拒绝）。
    pub source_epoch: u64,
    /// 测量时刻（单调毫秒）。
    pub measured_at_ms: u64,
    /// 接收时刻（单调毫秒；用于单调性检查）。
    pub received_at_ms: u64,
    /// 质量（采集链上报 + 评估层结论；控制输入要求 Good）。
    pub quality: SampleQuality,
    /// 序号（每点单调递增；重放/乱序拒绝）。
    pub sequence: u64,
    /// 完整性状态。
    pub integrity: IntegrityStatus,
    /// 采集链引用的设备/点映射版本（评估器 mapping_version 比对）。
    pub mapping_version: u64,
}

/// 必需点（覆盖声明 + 校验参数）。
#[derive(Debug, Clone)]
pub struct RequiredPoint {
    /// 点 ID。
    pub point: PointId,
    /// 量程下限（含）。
    pub min: f64,
    /// 量程上限（含）。
    pub max: f64,
    /// 最大新鲜度（ms）——`now - measured_at <= max_age_ms` 才可用。
    pub max_age_ms: u64,
    /// 声明单位。
    pub unit: PointUnit,
}

/// 必需点覆盖集合。
#[derive(Debug, Clone)]
pub struct RequiredPointCoverage {
    /// 必需点列表（全有才产生 TrustedTelemetry）。
    pub points: Vec<RequiredPoint>,
}

impl RequiredPointCoverage {
    /// 空覆盖（永不通过——fail-closed）。
    pub fn empty() -> Self {
        Self { points: Vec::new() }
    }

    /// 查找覆盖声明（返回 None = 未覆盖）。
    pub fn find(&self, point: PointId) -> Option<&RequiredPoint> {
        self.points.iter().find(|p| p.point == point)
    }
}

/// 点 → SystemState 字段绑定（评估器用受信任样本构造 `SystemState`）。
#[derive(Debug, Clone, Copy)]
pub struct StateBindings {
    /// 母线电压点。
    pub voltage: PointId,
    /// 母线电流点。
    pub current: PointId,
    /// 系统频率点。
    pub frequency: PointId,
    /// SOC 点。
    pub soc: PointId,
}

/// 评估器配置。
#[derive(Debug, Clone)]
pub struct EvaluatorConfig {
    /// 必需点覆盖。
    pub coverage: RequiredPointCoverage,
    /// SystemState 字段绑定。
    pub bindings: StateBindings,
    /// 设备/点映射版本（评估器创建时注入；样本侧不匹配拒绝）。
    pub mapping_version: u64,
    /// 接收时刻最大偏差（同快照内单调性 + 跨快照不倒退）。
    pub max_skew_ms: u64,
    /// 允许的来源白名单（空 = 拒绝一切来源）。
    pub allowed_sources: Vec<u32>,
    /// 是否要求完整性 Verified。
    pub require_integrity: bool,
    /// 控制时域（ms）——`TrustedTelemetry.valid_until = now + horizon`。
    pub control_horizon_ms: u64,
}

/// 可信遥测（唯一可进入决策/控制链的状态载体）。
///
/// `state` 只由评估器从受信任样本构造；`provenance_digest` 为样本集合的
/// SM3 摘要（篡改可检出）；`valid_until_ms` 定义控制时域，执行时刻
/// `now > valid_until_ms` 必须重新评估（WP-40.6 步骤 2）。
#[derive(Debug, Clone)]
pub struct TrustedTelemetry {
    /// 电气状态（由受信任样本构造，非默认值）。
    pub state: SystemState,
    /// 本次评估覆盖的点集合。
    pub coverage: RequiredPointCoverage,
    /// 控制时域截止（单调毫秒）。
    pub valid_until_ms: u64,
    /// 样本集合 provenance 摘要（SM3）。
    pub provenance_digest: [u8; 32],
    /// 评估生成时刻（单调毫秒）。
    pub generated_at_ms: u64,
    /// 来源 epoch（本次评估所有样本的 epoch）。
    pub source_epoch: u64,
    /// 映射版本（本次评估使用的版本）。
    pub mapping_version: u64,
    /// 全部受信任样本值（点 → 值；dispatch 层据此构造反馈包/按点校验）。
    pub samples: BTreeMap<PointId, f64>,
}

impl TrustedTelemetry {
    /// 执行时刻是否仍在控制时域内（`now <= valid_until_ms`）。
    pub fn is_valid_at(&self, now_ms: u64) -> bool {
        now_ms <= self.valid_until_ms
    }
}

/// 结构化拒绝（缺数据/坏质量/过期/重放等——绝不用"健康默认值"掩盖）。
#[derive(Debug, Clone, PartialEq)]
pub enum TrustRejection {
    /// 无样本。
    NoSamples,
    /// 必需点缺失。
    MissingPoints { missing: Vec<PointId> },
    /// 样本过期。
    Stale {
        point: PointId,
        age_ms: u64,
        max_age_ms: u64,
    },
    /// 质量不可用。
    BadQuality {
        point: PointId,
        quality: SampleQuality,
    },
    /// 非有限值。
    NotFinite { point: PointId },
    /// 越界。
    OutOfRange {
        point: PointId,
        value: f64,
        min: f64,
        max: f64,
    },
    /// 重放/乱序。
    Replay {
        point: PointId,
        sequence: u64,
        last_sequence: u64,
    },
    /// 多源冲突（同点多源，严格拒绝）。
    SourceConflict { point: PointId, sources: Vec<u32> },
    /// 来源不在白名单。
    UnknownSource { point: PointId, source: u32 },
    /// 来源 epoch 不匹配（断线重连后旧 epoch 消息）。
    SourceEpochMismatch {
        point: PointId,
        epoch: u64,
        expected: u64,
    },
    /// 映射版本不匹配。
    MappingStale {
        point: PointId,
        mapping_version: u64,
        expected: u64,
    },
    /// 完整性未验证。
    IntegrityFailure {
        point: PointId,
        status: IntegrityStatus,
    },
    /// 接收时刻倒退/超偏差（时钟不单调）。
    ClockNotMonotonic {
        received_at_ms: u64,
        last_received_at_ms: u64,
    },
}

/// 可信遥测评估器（WP-40.1 验证器）。
///
/// 有状态：跨快照记住每点最后序号与最后接收时刻（重启后经
/// `TelemetryService` 持久化恢复——服务层职责，见蓝图 40 WP-40.2）。
#[derive(Debug, Clone)]
pub struct TelemetryTrustEvaluator {
    config: EvaluatorConfig,
    /// 每点最后接受序号。
    last_sequences: BTreeMap<PointId, u64>,
    /// 跨快照最后接收时刻（单调性基线）。
    last_received_at_ms: u64,
    /// 当前来源 epoch（断线重连后递增）。
    epoch: u64,
}

impl TelemetryTrustEvaluator {
    /// 创建评估器。
    pub fn new(config: EvaluatorConfig) -> Self {
        Self {
            config,
            last_sequences: BTreeMap::new(),
            last_received_at_ms: 0,
            epoch: 0,
        }
    }

    /// 来源 epoch 推进（采集链断线重连后调用——旧 epoch 样本全部拒绝）。
    pub fn advance_epoch(&mut self) {
        self.epoch = self.epoch.saturating_add(1);
    }

    /// 当前来源 epoch。
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// 从持久化状态恢复（重启后防旧消息被误接受）。
    pub fn restore(
        &mut self,
        epoch: u64,
        last_received_at_ms: u64,
        last_sequences: BTreeMap<PointId, u64>,
    ) {
        self.epoch = epoch;
        self.last_received_at_ms = last_received_at_ms;
        self.last_sequences = last_sequences;
    }

    /// 评估样本集合：全部通过 → `TrustedTelemetry`；任何一项不通过 →
    /// 结构化拒绝（零部分通过）。
    pub fn evaluate(
        &mut self,
        samples: &[TelemetrySample],
        now_ms: u64,
    ) -> Result<TrustedTelemetry, TrustRejection> {
        if samples.is_empty() {
            return Err(TrustRejection::NoSamples);
        }

        // 1. 接收时刻单调性（同快照 + 跨快照）。
        let mut snapshot_max_received = 0u64;
        for s in samples {
            if s.received_at_ms < self.last_received_at_ms {
                return Err(TrustRejection::ClockNotMonotonic {
                    received_at_ms: s.received_at_ms,
                    last_received_at_ms: self.last_received_at_ms,
                });
            }
            snapshot_max_received = snapshot_max_received.max(s.received_at_ms);
        }
        // max skew：同快照内最大接收偏差超限拒绝。
        let min_received = samples
            .iter()
            .map(|s| s.received_at_ms)
            .min()
            .unwrap_or(snapshot_max_received);
        if snapshot_max_received.saturating_sub(min_received) > self.config.max_skew_ms {
            return Err(TrustRejection::ClockNotMonotonic {
                received_at_ms: snapshot_max_received,
                last_received_at_ms: min_received,
            });
        }

        // 2. 覆盖检查：必需点是否全部出现且唯一（多源冲突严格拒绝）。
        let mut missing = Vec::new();
        let mut by_point: BTreeMap<PointId, &TelemetrySample> = BTreeMap::new();
        for s in samples {
            if self.config.coverage.find(s.point).is_none() {
                // 非必需点：评估层忽略（采集侧自证由上层负责）；不参与控制。
                continue;
            }
            match by_point.get(&s.point) {
                None => {
                    by_point.insert(s.point, s);
                }
                Some(prev) if prev.source != s.source => {
                    return Err(TrustRejection::SourceConflict {
                        point: s.point,
                        sources: alloc::vec![prev.source, s.source],
                    });
                }
                Some(_) => {
                    // 同源重复样本：视为重放（快照应每点一值）。
                    return Err(TrustRejection::Replay {
                        point: s.point,
                        sequence: s.sequence,
                        last_sequence: self
                            .last_sequences
                            .get(&s.point)
                            .copied()
                            .unwrap_or(s.sequence.saturating_sub(1)),
                    });
                }
            }
        }
        for req in &self.config.coverage.points {
            if !by_point.contains_key(&req.point) {
                missing.push(req.point);
            }
        }
        if !missing.is_empty() {
            return Err(TrustRejection::MissingPoints { missing });
        }

        // 3. 逐点校验。
        let mut values: BTreeMap<PointId, f64> = BTreeMap::new();
        for req in &self.config.coverage.points {
            let s = by_point[&req.point];
            // 来源白名单。
            if !self.config.allowed_sources.contains(&s.source) {
                return Err(TrustRejection::UnknownSource {
                    point: req.point,
                    source: s.source,
                });
            }
            // 来源 epoch。
            if s.source_epoch != self.epoch {
                return Err(TrustRejection::SourceEpochMismatch {
                    point: req.point,
                    epoch: s.source_epoch,
                    expected: self.epoch,
                });
            }
            // 映射版本。
            if s.mapping_version != self.config.mapping_version {
                return Err(TrustRejection::MappingStale {
                    point: req.point,
                    mapping_version: s.mapping_version,
                    expected: self.config.mapping_version,
                });
            }
            // 质量。
            if !s.quality.is_usable() {
                return Err(TrustRejection::BadQuality {
                    point: req.point,
                    quality: s.quality,
                });
            }
            // 完整性。
            if self.config.require_integrity && s.integrity != IntegrityStatus::Verified {
                return Err(TrustRejection::IntegrityFailure {
                    point: req.point,
                    status: s.integrity,
                });
            }
            // 有限值。
            if !s.value.is_finite() {
                return Err(TrustRejection::NotFinite { point: req.point });
            }
            // 量程。
            if s.value < req.min || s.value > req.max {
                return Err(TrustRejection::OutOfRange {
                    point: req.point,
                    value: s.value,
                    min: req.min,
                    max: req.max,
                });
            }
            // 新鲜度。
            let age = now_ms.saturating_sub(s.measured_at_ms);
            if age > req.max_age_ms {
                return Err(TrustRejection::Stale {
                    point: req.point,
                    age_ms: age,
                    max_age_ms: req.max_age_ms,
                });
            }
            // 序号（重放/乱序）。
            let last_seq = self.last_sequences.get(&req.point).copied().unwrap_or(0);
            if s.sequence <= last_seq {
                return Err(TrustRejection::Replay {
                    point: req.point,
                    sequence: s.sequence,
                    last_sequence: last_seq,
                });
            }
            values.insert(req.point, s.value);
        }

        // 4. 构造 SystemState（只从受信任样本——无默认值路径）。
        let state = SystemState::new(
            values[&self.config.bindings.voltage],
            values[&self.config.bindings.current],
            values[&self.config.bindings.frequency],
            values[&self.config.bindings.soc],
            now_ms,
        );

        // 5. provenance 摘要：SM3 覆盖全部样本的规范编码（定长、无歧义）。
        let digest = provenance_digest(samples, self.config.mapping_version, self.epoch);

        // 6. 推进状态。
        self.last_received_at_ms = snapshot_max_received;
        for (point, s) in &by_point {
            self.last_sequences.insert(*point, s.sequence);
        }

        Ok(TrustedTelemetry {
            state,
            coverage: self.config.coverage.clone(),
            valid_until_ms: now_ms.saturating_add(self.config.control_horizon_ms),
            provenance_digest: digest,
            generated_at_ms: now_ms,
            source_epoch: self.epoch,
            mapping_version: self.config.mapping_version,
            samples: values,
        })
    }
}

/// 样本携带的映射版本（采集链在发送时打上点映射版本戳，评估器比对）。
///
/// 说明：多版本映射演进时由采集链按点映射表填充；当前产品点映射为 v1。
///
/// 规范编码（定长字段、全小端、无长度前缀歧义）→ SM3。
pub fn provenance_digest(
    samples: &[TelemetrySample],
    mapping_version: u64,
    epoch: u64,
) -> [u8; 32] {
    let mut buf = alloc::vec![0u8; 8 + 8 + 56 * samples.len()];
    buf[0..8].copy_from_slice(&mapping_version.to_le_bytes());
    buf[8..16].copy_from_slice(&epoch.to_le_bytes());
    for (i, s) in samples.iter().enumerate() {
        let base = 16 + i * 56;
        buf[base..base + 4].copy_from_slice(&s.point.to_le_bytes());
        buf[base + 4..base + 12].copy_from_slice(&s.value.to_le_bytes());
        buf[base + 12..base + 16].copy_from_slice(&s.source.to_le_bytes());
        buf[base + 16..base + 24].copy_from_slice(&s.source_epoch.to_le_bytes());
        buf[base + 24..base + 32].copy_from_slice(&s.measured_at_ms.to_le_bytes());
        buf[base + 32..base + 40].copy_from_slice(&s.sequence.to_le_bytes());
        buf[base + 40] = quality_code(s.quality);
        buf[base + 41] = integrity_code(s.integrity);
        buf[base + 48..base + 56].copy_from_slice(&s.mapping_version.to_le_bytes());
    }
    eneros_crypto::sm3_hash(&buf)
}

fn quality_code(q: SampleQuality) -> u8 {
    match q {
        SampleQuality::Good => 0,
        SampleQuality::Suspect(_) => 1,
        SampleQuality::Bad(_) => 2,
        SampleQuality::Unknown => 3,
    }
}

fn integrity_code(i: IntegrityStatus) -> u8 {
    match i {
        IntegrityStatus::Verified => 0,
        IntegrityStatus::Unverified => 1,
        IntegrityStatus::Unverifiable => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good(
        point: PointId,
        value: f64,
        sequence: u64,
        measured_at_ms: u64,
        received_at_ms: u64,
    ) -> TelemetrySample {
        TelemetrySample {
            point,
            value,
            source: 1,
            source_epoch: 0,
            measured_at_ms,
            received_at_ms,
            quality: SampleQuality::Good,
            sequence,
            integrity: IntegrityStatus::Verified,
            mapping_version: 1,
        }
    }

    fn evaluator() -> TelemetryTrustEvaluator {
        TelemetryTrustEvaluator::new(EvaluatorConfig {
            coverage: RequiredPointCoverage {
                points: alloc::vec![
                    RequiredPoint {
                        point: 1,
                        min: 0.0,
                        max: 1000.0,
                        max_age_ms: 5000,
                        unit: PointUnit::Volt
                    },
                    RequiredPoint {
                        point: 2,
                        min: -1000.0,
                        max: 1000.0,
                        max_age_ms: 5000,
                        unit: PointUnit::Ampere
                    },
                    RequiredPoint {
                        point: 3,
                        min: 45.0,
                        max: 55.0,
                        max_age_ms: 5000,
                        unit: PointUnit::Hertz
                    },
                    RequiredPoint {
                        point: 4,
                        min: 0.0,
                        max: 1.0,
                        max_age_ms: 5000,
                        unit: PointUnit::Ratio
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
            allowed_sources: alloc::vec![1],
            require_integrity: true,
            control_horizon_ms: 30_000,
        })
    }

    fn snapshot(now: u64) -> Vec<TelemetrySample> {
        alloc::vec![
            good(1, 380.0, 1, now - 100, now),
            good(2, 10.0, 1, now - 100, now),
            good(3, 50.0, 1, now - 100, now),
            good(4, 0.6, 1, now - 100, now),
        ]
    }

    #[test]
    fn evaluate_trusted_snapshot_builds_state() {
        let mut ev = evaluator();
        let t = ev.evaluate(&snapshot(10_000), 10_000).expect("trusted");
        assert_eq!(t.state.voltage_v, 380.0);
        assert_eq!(t.state.current_a, 10.0);
        assert_eq!(t.state.frequency_hz, 50.0);
        assert_eq!(t.state.soc_pct, 0.6);
        assert_eq!(t.valid_until_ms, 40_000);
        assert_eq!(t.provenance_digest.len(), 32);
        assert!(t.is_valid_at(39_999));
        assert!(!t.is_valid_at(40_001));
        // 摘要随值变化（篡改可检出）。
        let mut tampered = snapshot(10_000);
        tampered[0].value = 381.0;
        for s in tampered.iter_mut() {
            s.sequence = 2;
        }
        let t2 = ev.evaluate(&tampered, 10_000).expect("trusted");
        assert_ne!(t.provenance_digest, t2.provenance_digest);
        // 同序号重放必须拒绝（评估器有状态）。
        assert!(matches!(
            ev.evaluate(&tampered, 10_000).unwrap_err(),
            TrustRejection::Replay { .. }
        ));
    }

    #[test]
    fn no_samples_rejected() {
        let mut ev = evaluator();
        assert!(matches!(
            ev.evaluate(&[], 10_000),
            Err(TrustRejection::NoSamples)
        ));
    }

    #[test]
    fn missing_point_rejected() {
        let mut ev = evaluator();
        let mut s = snapshot(10_000);
        s.retain(|x| x.point != 4);
        let err = ev.evaluate(&s, 10_000).unwrap_err();
        assert!(
            matches!(err, TrustRejection::MissingPoints { missing } if missing == alloc::vec![4])
        );
    }

    #[test]
    fn stale_rejected() {
        let mut ev = evaluator();
        let mut s = snapshot(10_000);
        s[0].measured_at_ms = 1_000; // age 9000 > 5000
        assert!(matches!(
            ev.evaluate(&s, 10_000).unwrap_err(),
            TrustRejection::Stale { point: 1, .. }
        ));
    }

    #[test]
    fn bad_quality_rejected() {
        let mut ev = evaluator();
        let mut s = snapshot(10_000);
        s[2].quality = SampleQuality::Bad(QualityReason::IntegrityFailure);
        assert!(matches!(
            ev.evaluate(&s, 10_000).unwrap_err(),
            TrustRejection::BadQuality { point: 3, .. }
        ));
    }

    #[test]
    fn nan_inf_rejected() {
        let mut ev = evaluator();
        let mut s = snapshot(10_000);
        s[1].value = f64::NAN;
        assert!(matches!(
            ev.evaluate(&s, 10_000).unwrap_err(),
            TrustRejection::NotFinite { point: 2 }
        ));
        let mut s2 = snapshot(10_000);
        s2[3].value = f64::INFINITY;
        assert!(matches!(
            ev.evaluate(&s2, 10_000).unwrap_err(),
            TrustRejection::NotFinite { point: 4 }
        ));
    }

    #[test]
    fn out_of_range_rejected() {
        let mut ev = evaluator();
        let mut s = snapshot(10_000);
        s[3].value = 1.5;
        assert!(matches!(
            ev.evaluate(&s, 10_000).unwrap_err(),
            TrustRejection::OutOfRange { point: 4, .. }
        ));
    }

    #[test]
    fn replay_rejected() {
        let mut ev = evaluator();
        let _ = ev.evaluate(&snapshot(10_000), 10_000).expect("first");
        // 同序号重放（旧消息）。
        let err = ev.evaluate(&snapshot(10_000), 11_000).unwrap_err();
        assert!(matches!(err, TrustRejection::Replay { point: 1, .. }));
        // 序号递增但接收时刻倒退 → 时钟不单调。
        let mut s = snapshot(11_000);
        for x in s.iter_mut() {
            x.sequence = 2;
            x.received_at_ms = 9_000;
        }
        assert!(matches!(
            ev.evaluate(&s, 11_000).unwrap_err(),
            TrustRejection::ClockNotMonotonic { .. }
        ));
    }

    #[test]
    fn source_conflict_rejected() {
        let mut ev = evaluator();
        let mut s = snapshot(10_000);
        let mut dup = good(1, 381.0, 1, 9_900, 10_000);
        dup.source = 2;
        s.push(dup);
        assert!(matches!(
            ev.evaluate(&s, 10_000).unwrap_err(),
            TrustRejection::SourceConflict { point: 1, .. }
        ));
    }

    #[test]
    fn unknown_source_rejected() {
        let mut ev = evaluator();
        let mut s = snapshot(10_000);
        s[0].source = 99;
        assert!(matches!(
            ev.evaluate(&s, 10_000).unwrap_err(),
            TrustRejection::UnknownSource { point: 1, .. }
        ));
    }

    #[test]
    fn stale_epoch_rejected_after_reconnect() {
        let mut ev = evaluator();
        let _ = ev.evaluate(&snapshot(10_000), 10_000).expect("epoch 0");
        ev.advance_epoch(); // 断线重连
                            // 旧 epoch 样本必须拒绝（不误接受旧消息）。
        let err = ev.evaluate(&snapshot(20_000), 20_000).unwrap_err();
        assert!(matches!(
            err,
            TrustRejection::SourceEpochMismatch { point: 1, .. }
        ));
        // 新 epoch 样本接受。
        let mut s = snapshot(20_000);
        for x in s.iter_mut() {
            x.source_epoch = 1;
            x.sequence = 2;
        }
        let t = ev.evaluate(&s, 20_000).expect("epoch 1 trusted");
        assert_eq!(t.source_epoch, 1);
    }

    #[test]
    fn integrity_required_rejected() {
        let mut ev = evaluator();
        let mut s = snapshot(10_000);
        s[0].integrity = IntegrityStatus::Unverified;
        assert!(matches!(
            ev.evaluate(&s, 10_000).unwrap_err(),
            TrustRejection::IntegrityFailure { point: 1, .. }
        ));
    }

    #[test]
    fn restore_state_rejects_old_epoch() {
        let mut ev = evaluator();
        ev.restore(5, 100_000, BTreeMap::new());
        let err = ev.evaluate(&snapshot(200_000), 200_000).unwrap_err();
        assert!(matches!(
            err,
            TrustRejection::SourceEpochMismatch {
                epoch: 0,
                expected: 5,
                ..
            }
        ));
        let mut s = snapshot(200_000);
        for x in s.iter_mut() {
            x.source_epoch = 5;
        }
        let _ = ev.evaluate(&s, 200_000).expect("restored epoch accepted");
    }

    #[test]
    fn required_point_unit_documented_in_coverage() {
        let ev = evaluator();
        let soc = ev.config.coverage.find(4).expect("soc covered");
        assert_eq!(soc.unit, PointUnit::Ratio);
    }
}
