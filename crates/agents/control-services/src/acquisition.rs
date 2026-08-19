//! 蓝图 45 WP-45.2：采集数据面——质量位映射 / 限幅+死区 / 定点化纪律。
//!
//! - 协议读结果 → `SampleQuality` + `IntegrityStatus`（复用 safety-validator
//!   既有枚举与 `TelemetryTrustEvaluator` 语义，**禁新造**；质量位由本模块
//!   归一，具体协议状态（sv smpCnt、GOOSE TTL 等）在上层翻译为本模块枚举）；
//! - 工程量滤镜：值域 [min,max] 限幅（超限 → Suspect(OutOfRange)）+ deadband
//!   死区（内不刷新缓存）；
//! - 定点化纪律：电量累计/金额判定用定点（`MilliCounter`，MilliYuan 先例），
//!   f64 仅限瞬时工程量——禁 f64 累计漂移。

use eneros_safety_validator::{IntegrityStatus, QualityReason, SampleQuality};

/// 协议读结果（采集层归一枚举；具体协议状态在此翻译）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquisitionOutcome {
    /// 正常读取。
    Good,
    /// 超时（无响应/连接失败）。
    Timeout,
    /// 坏帧/校验失败（CRC/长度/语义）。
    BadFrame,
    /// 异常码（Modbus exception / ASDU 异常应答）。
    Exception,
    /// 序号异常（sv `SampleStatus` smpCnt Duplicate/Jump；重放/乱序）。
    SequenceAnomaly,
    /// 订阅过期（GOOSE TTL / SV 通道超时）。
    SubscriptionStale,
}

impl AcquisitionOutcome {
    /// 质量位映射（45.2）：
    /// - Good → Good（唯一可进控制链）；
    /// - 超时/坏帧/异常码/序号异常 → Bad(IntegrityFailure)（拒绝）；
    /// - 订阅过期（GOOSE TTL 等）→ Suspect(Stale)（可观察但降权）。
    pub fn sample_quality(self) -> SampleQuality {
        match self {
            AcquisitionOutcome::Good => SampleQuality::Good,
            AcquisitionOutcome::SubscriptionStale => SampleQuality::Suspect(QualityReason::Stale),
            AcquisitionOutcome::Timeout
            | AcquisitionOutcome::BadFrame
            | AcquisitionOutcome::Exception
            | AcquisitionOutcome::SequenceAnomaly => {
                SampleQuality::Bad(QualityReason::IntegrityFailure)
            }
        }
    }
}

/// 采集链默认完整性（无独立认证链 → `Unverified`；`require_integrity`
/// 时被评估器降级拒绝——诚实不声称 Verified）。
pub const fn acquisition_integrity() -> IntegrityStatus {
    IntegrityStatus::Unverified
}

/// 工程量滤镜（45.2 限幅 + 死区；45.4 采集循环消费）。
///
/// 语义：
/// - 值域 [min,max]（点表 `range` 派生）：超出 → 拒纳该值为缓存并标记
///   对应的质量（`Suspect(OutOfRange)`），不更新缓存；
/// - 非有限（NaN/±inf）→ 拒绝并标记 `Bad(NotFinite)`；
/// - deadband 内（|v - last| ≤ deadband）→ `Hold`（不刷新缓存不更新）；
/// - 有效且跨死区 → `Use(v)`（更新缓存，质量由上层按 Good 处理）。
#[derive(Debug, Clone)]
pub struct EngineeringFilter {
    /// 值域 [min,max]（点表 `range`）。
    pub range: Option<(f64, f64)>,
    /// 工程量死区（`|v - last| ≤ deadband` 视为无变化）。
    pub deadband: f64,
    /// 最后刷新值（死区基带）。
    last: Option<f64>,
}

/// 滤镜决策。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FilterDecision {
    /// 有效且跨死区：采用 `f64`（缓存已更新）。
    Use(f64),
    /// deadband 内：不刷新缓存。
    Hold,
    /// 拒绝并标记质量（缓存未更新）。
    Reject(SampleQuality),
}

impl EngineeringFilter {
    /// 构造（range/deadband 来自点表；`deadband` 负值按 0 处理）。
    pub fn new(range: Option<(f64, f64)>, deadband: f64) -> Self {
        Self {
            range,
            deadband: if deadband.is_finite() && deadband > 0.0 {
                deadband
            } else {
                0.0
            },
            last: None,
        }
    }

    /// 应用滤镜（见模块注释语义）。
    pub fn apply(&mut self, engineering: f64) -> FilterDecision {
        if !engineering.is_finite() {
            return FilterDecision::Reject(SampleQuality::Bad(QualityReason::NotFinite));
        }
        if let Some((min, max)) = self.range {
            if engineering < min || engineering > max {
                return FilterDecision::Reject(SampleQuality::Suspect(QualityReason::OutOfRange));
            }
        }
        if let Some(last) = self.last {
            let delta = (engineering - last).abs();
            if delta <= self.deadband {
                return FilterDecision::Hold;
            }
        }
        self.last = Some(engineering);
        FilterDecision::Use(engineering)
    }
}

/// 定点累计（MilliYuan 先例：电量累计/金额判定禁 f64 累积漂移；
/// f64 仅限瞬时工程量）。单位 = 千分之一（milli = 0.001）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MilliCounter(pub i64);

impl MilliCounter {
    /// 零值。
    pub const ZERO: Self = Self(0);

    /// 瞬时工程量 → 定点毫单位增量（非有限/超 i64 → None，fail-closed）。
    pub fn from_engineering(v: f64) -> Option<Self> {
        if !v.is_finite() {
            return None;
        }
        let milli = v * 1_000.0;
        if milli < i64::MIN as f64 || milli > i64::MAX as f64 {
            return None;
        }
        // 就近取整（no_std 无 f64::round；as 截断 + 0.5·sgn 四舍五入）。
        let rounded = (milli + 0.5 * milli.signum()) as i64;
        Some(Self(rounded))
    }

    /// 定点累计（溢出 → None，fail-closed）。
    pub fn accumulate(&mut self, delta: Self) -> Option<()> {
        self.0 = self.0.checked_add(delta.0)?;
        Some(())
    }

    /// 转 f64 工程量（仅用于展示/上报瞬时通道；判定用定点本体）。
    pub fn to_engineering_f64(self) -> f64 {
        self.0 as f64 / 1_000.0
    }
}

#[cfg(test)]
mod tests {
    use eneros_safety_validator::{
        EvaluatorConfig, RequiredPoint, RequiredPointCoverage, StateBindings, TelemetrySample,
        TelemetryTrustEvaluator, TrustRejection,
    };

    use super::*;

    // ---- 45.2 DoD：协议读结果 → 质量位 ----

    #[test]
    fn acquisition_outcome_quality_mapping() {
        // Good → Good（唯一可进控制链）。
        assert_eq!(
            AcquisitionOutcome::Good.sample_quality(),
            SampleQuality::Good
        );
        // 超时/坏帧/异常码/序号异常 → Bad（拒绝）。
        for bad in [
            AcquisitionOutcome::Timeout,
            AcquisitionOutcome::BadFrame,
            AcquisitionOutcome::Exception,
            AcquisitionOutcome::SequenceAnomaly,
        ] {
            assert!(
                matches!(bad.sample_quality(), SampleQuality::Bad(_)),
                "{bad:?}"
            );
        }
        // 订阅过期（GOOSE TTL）→ Suspect(Stale)。
        assert_eq!(
            AcquisitionOutcome::SubscriptionStale.sample_quality(),
            SampleQuality::Suspect(QualityReason::Stale)
        );
        // 完整性默认 Unverified（require_integrity 时被降级拒绝，不谎称）。
        assert_eq!(acquisition_integrity(), IntegrityStatus::Unverified);
    }

    // ---- 45.2 DoD：限幅/死区（溢出/NaN/超限 → 对应质量位） ----

    #[test]
    fn filter_rejects_non_finite_as_bad() {
        let mut f = EngineeringFilter::new(Some((-40.0, 40.0)), 0.0);
        assert_eq!(
            f.apply(f64::NAN),
            FilterDecision::Reject(SampleQuality::Bad(QualityReason::NotFinite))
        );
        assert_eq!(
            f.apply(f64::INFINITY),
            FilterDecision::Reject(SampleQuality::Bad(QualityReason::NotFinite))
        );
        // 缓存未被污染（死区基带仍空）。
        assert!(f.last.is_none());
        assert_eq!(f.apply(10.0), FilterDecision::Use(10.0));
    }

    #[test]
    fn filter_rejects_out_of_range_as_suspect() {
        let mut f = EngineeringFilter::new(Some((-40.0, 40.0)), 0.0);
        assert_eq!(
            f.apply(-50.0),
            FilterDecision::Reject(SampleQuality::Suspect(QualityReason::OutOfRange))
        );
        assert_eq!(
            f.apply(50.0),
            FilterDecision::Reject(SampleQuality::Suspect(QualityReason::OutOfRange))
        );
        // 边界含端点值（min ≤ v ≤ max 合法）。
        assert_eq!(f.apply(40.0), FilterDecision::Use(40.0));
    }

    #[test]
    fn filter_deadband_holds_within_band() {
        let mut f = EngineeringFilter::new(None, 0.5);
        assert_eq!(f.apply(10.0), FilterDecision::Use(10.0));
        // 10.2 距 10.0 ≤ 0.5 → Hold（不刷新缓存）。
        assert_eq!(f.apply(10.2), FilterDecision::Hold);
        assert_eq!(f.apply(10.49), FilterDecision::Hold);
        // 跨死区 → Use 并刷新。
        assert_eq!(f.apply(10.51), FilterDecision::Use(10.51));
    }

    // ---- 45.2 DoD：定点化纪律（MilliYuan 先例） ----

    #[test]
    fn milli_counter_fixed_point_accumulates_without_drift() {
        // f64 累积 (0.1 + 0.2 + 0.3) 有二进制误差；定点毫单位精确。
        let mut total = MilliCounter::ZERO;
        for delta in [0.1f64, 0.2, 0.3] {
            total
                .accumulate(MilliCounter::from_engineering(delta).unwrap())
                .unwrap();
        }
        assert_eq!(total, MilliCounter(600)); // 600 milli = 0.6 精确
        assert_eq!(total.to_engineering_f64(), 0.6);

        // 溢出 fail-closed。
        let mut big = MilliCounter(i64::MAX);
        assert!(big.accumulate(MilliCounter(1)).is_none());
        // 非有限 → None。
        assert!(MilliCounter::from_engineering(f64::NAN).is_none());
        assert!(MilliCounter::from_engineering(f64::INFINITY).is_none());
    }

    // ---- 45.2 DoD：uncertain/bad 点被 TelemetryTrustEvaluator 拒/降权
    //      （复用既有语义，不改评估器） ----

    fn trust_config() -> EvaluatorConfig {
        EvaluatorConfig {
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
        }
    }

    fn sample(
        point: u32,
        value: f64,
        seq: u64,
        now: u64,
        quality: SampleQuality,
    ) -> TelemetrySample {
        TelemetrySample {
            point,
            value,
            source: 1,
            source_epoch: 0,
            measured_at_ms: now - 50,
            received_at_ms: now,
            quality,
            sequence: seq,
            integrity: IntegrityStatus::Verified,
            mapping_version: 1,
        }
    }

    #[test]
    fn all_good_passes_evaluator() {
        let mut eval = TelemetryTrustEvaluator::new(trust_config());
        let now = 10_000;
        let samples = vec![
            sample(1, 380.0, 1, now, SampleQuality::Good),
            sample(2, 10.0, 1, now, SampleQuality::Good),
            sample(3, 50.0, 1, now, SampleQuality::Good),
            sample(4, 0.6, 1, now, SampleQuality::Good),
        ];
        assert!(eval.evaluate(&samples, now).is_ok());
    }

    /// 负例：一个 uncertain（SubscriptionStale → Suspect）采样点 →
    /// 整快照被评估器拒绝（BadQuality；零部分通过——fail-closed）。
    #[test]
    fn uncertain_point_rejected_by_trust_evaluator() {
        let mut eval = TelemetryTrustEvaluator::new(trust_config());
        let now = 10_000;
        let mut samples = vec![
            sample(1, 380.0, 1, now, SampleQuality::Good),
            sample(2, 10.0, 1, now, SampleQuality::Good),
            sample(3, 50.0, 1, now, SampleQuality::Good),
            sample(4, 0.6, 1, now, SampleQuality::Good),
        ];
        // 订阅过期 → Suspect(Stale)（经 45.2 映射）。
        samples[0].quality = AcquisitionOutcome::SubscriptionStale.sample_quality();
        assert_eq!(
            samples[0].quality,
            SampleQuality::Suspect(QualityReason::Stale)
        );
        assert!(matches!(
            eval.evaluate(&samples, now),
            Err(TrustRejection::BadQuality { point: 1, .. })
        ));
    }

    /// 负例：bad（Timeout → Bad）采样点 → 拒绝。
    #[test]
    fn bad_point_rejected_by_trust_evaluator() {
        let mut eval = TelemetryTrustEvaluator::new(trust_config());
        let now = 10_000;
        let mut samples = vec![
            sample(1, 380.0, 1, now, SampleQuality::Good),
            sample(2, 10.0, 1, now, SampleQuality::Good),
            sample(3, 50.0, 1, now, SampleQuality::Good),
            sample(4, 0.6, 1, now, SampleQuality::Good),
        ];
        samples[3].quality = AcquisitionOutcome::Timeout.sample_quality();
        assert!(matches!(
            eval.evaluate(&samples, now),
            Err(TrustRejection::BadQuality { point: 4, .. })
        ));
    }
}
