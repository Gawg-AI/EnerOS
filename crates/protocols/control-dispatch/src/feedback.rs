//! 蓝图 40 WP-40.6 步骤 2 的反馈构造：受信任遥测 → `FeedbackBundle`
//! （CTL-010 链的反馈输入）。
//!
//! 只把**真实受信任样本**填入反馈包；覆盖缺失的点以 `QualityFlag::Invalid`
//! 显式未知表达（链的 reconcile 会把未知样本判为不可信 → 安全校验拒绝），
//! 绝不制造"正常默认值"（P0-09/FIX-24.R3）。

use eneros_control_gateway::chain::FeedbackBundle;
use eneros_control_gateway::reconcile::FeedbackSample;
use eneros_safety_validator::TrustedTelemetry;
use eneros_telemetry_model::quality::QualityFlag;
use eneros_upa_model::point::PointId;

/// 反馈点映射（svc-telemetry 覆盖声明的一部分——点 ID 必须真实存在）。
#[derive(Debug, Clone, Copy)]
pub struct FeedbackPointMapping {
    /// 电网状态点（0/1）。
    pub grid_up: PointId,
    /// SOC 点（0..100）。
    pub soc: PointId,
    /// 直流母线电压点（V）。
    pub dc_bus_v: PointId,
    /// 过流标志点（0/1）。
    pub overcurrent: PointId,
    /// 过温标志点（0/1）。
    pub overtemp: PointId,
}

/// 生产反馈点映射——**单源**（SPEC-SERIES-2026-08-29/R4 WP-R4.1）。
///
/// wire 槽序 = 本结构字段声明序：`grid_up → soc → dc_bus_v → overcurrent
/// → overtemp`（144B 受信 wire 的反馈段，control-services::telemetry
/// `encode_trusted_with/decode_trusted_with` 与本装配三方消费此常量；
/// 反馈点位改动只改这里，不再各处手抄）。
///
/// ⚠ 已知别名（体检 A3 登记）：`overtemp` 与 `overcurrent` 暂指向同一点位
/// 6——QEMU 从站仅发布 6 个点位（0x0100–0x0105，点 6 = 告警标志），过温
/// 未接入，两信号在链上取值恒同。矫正前置 = 设备新增过温点 + 覆盖表 +
/// QEMU 全链复验（overtemp 不可信 → HZ-THERM-001 拒充，见 spec R4 §3
/// WP-R4.4），**不得在本常量之外私自改键**。
pub const PROD_FEEDBACK_MAPPING: FeedbackPointMapping = FeedbackPointMapping {
    grid_up: 5,
    soc: 4,
    dc_bus_v: 1,
    overcurrent: 6,
    overtemp: 6,
};

/// 受信任遥测 → 反馈包构造器。
pub struct TrustedFeedbackBuilder {
    mapping: FeedbackPointMapping,
}

impl TrustedFeedbackBuilder {
    /// 构造（mapping 与评估器覆盖一致——缺失点按未覆盖处理）。
    pub fn new(mapping: FeedbackPointMapping) -> Self {
        Self { mapping }
    }

    /// 从受信任遥测构造反馈包。
    ///
    /// 覆盖的点 → `Good` 真实值；未覆盖/缺失的点 → `Invalid`（显式未知，
    /// 链按不可信拒绝相关安全判断）。
    pub fn build(&self, telemetry: &TrustedTelemetry, now_secs: u64) -> FeedbackBundle {
        let sample = |point: PointId| -> FeedbackSample {
            match telemetry.samples.get(&point) {
                Some(v) if v.is_finite() => FeedbackSample {
                    value: *v,
                    quality: QualityFlag::Good,
                    timestamp_epoch: now_secs,
                },
                _ => FeedbackSample {
                    value: 0.0,
                    quality: QualityFlag::Invalid,
                    timestamp_epoch: now_secs,
                },
            }
        };
        let grid = sample(self.mapping.grid_up);
        let soc = sample(self.mapping.soc);
        let dc = sample(self.mapping.dc_bus_v);
        let oc = sample(self.mapping.overcurrent);
        let ot = sample(self.mapping.overtemp);
        FeedbackBundle {
            grid_up: grid,
            soc_pct: soc,
            dc_bus_v: dc,
            overcurrent: oc,
            overtemp: ot,
            maintenance_lockout: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use eneros_safety_validator::{
        EvaluatorConfig, IntegrityStatus, RequiredPoint, RequiredPointCoverage, SampleQuality,
        StateBindings, TelemetrySample, TelemetryTrustEvaluator,
    };

    use super::*;

    fn evaluator() -> TelemetryTrustEvaluator {
        TelemetryTrustEvaluator::new(EvaluatorConfig {
            coverage: RequiredPointCoverage {
                points: alloc::vec![
                    RequiredPoint {
                        point: 1,
                        min: 0.0,
                        max: 1000.0,
                        max_age_ms: 5000,
                        unit: eneros_safety_validator::PointUnit::Volt
                    },
                    RequiredPoint {
                        point: 2,
                        min: 0.0,
                        max: 1.0,
                        max_age_ms: 5000,
                        unit: eneros_safety_validator::PointUnit::Ratio
                    },
                    RequiredPoint {
                        point: 3,
                        min: 0.0,
                        max: 100.0,
                        max_age_ms: 5000,
                        unit: eneros_safety_validator::PointUnit::Percent
                    },
                    RequiredPoint {
                        point: 4,
                        min: 0.0,
                        max: 1000.0,
                        max_age_ms: 5000,
                        unit: eneros_safety_validator::PointUnit::Volt
                    },
                    RequiredPoint {
                        point: 5,
                        min: 0.0,
                        max: 1.0,
                        max_age_ms: 5000,
                        unit: eneros_safety_validator::PointUnit::Other
                    },
                    RequiredPoint {
                        point: 6,
                        min: 0.0,
                        max: 1.0,
                        max_age_ms: 5000,
                        unit: eneros_safety_validator::PointUnit::Other
                    },
                    RequiredPoint {
                        point: 7,
                        min: -1000.0,
                        max: 1000.0,
                        max_age_ms: 5000,
                        unit: eneros_safety_validator::PointUnit::Ampere
                    },
                    RequiredPoint {
                        point: 8,
                        min: 45.0,
                        max: 55.0,
                        max_age_ms: 5000,
                        unit: eneros_safety_validator::PointUnit::Hertz
                    },
                ],
            },
            bindings: StateBindings {
                voltage: 1,
                current: 7,
                frequency: 8,
                soc: 3,
            },
            mapping_version: 1,
            max_skew_ms: 1000,
            allowed_sources: alloc::vec![1],
            require_integrity: true,
            control_horizon_ms: 30_000,
        })
    }

    fn sample(point: u32, value: f64, seq: u64, now: u64) -> TelemetrySample {
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

    #[test]
    fn builds_good_bundle_from_trusted_samples() {
        let mut ev = evaluator();
        let now = 10_000;
        let samples = alloc::vec![
            sample(1, 380.0, 1, now),
            sample(2, 1.0, 1, now),
            sample(3, 60.0, 1, now),
            sample(4, 600.0, 1, now),
            sample(5, 0.0, 1, now),
            sample(6, 0.0, 1, now),
            sample(7, 10.0, 1, now),
            sample(8, 50.0, 1, now),
        ];
        let t = ev.evaluate(&samples, now).expect("trusted");
        let b = TrustedFeedbackBuilder::new(FeedbackPointMapping {
            grid_up: 2,
            soc: 3,
            dc_bus_v: 4,
            overcurrent: 5,
            overtemp: 6,
        })
        .build(&t, 10);
        assert_eq!(b.grid_up.quality, QualityFlag::Good);
        assert_eq!(b.grid_up.value, 1.0);
        assert_eq!(b.soc_pct.value, 60.0);
        assert_eq!(b.dc_bus_v.value, 600.0);
        assert_eq!(b.overcurrent.value, 0.0);
        assert_eq!(b.overtemp.value, 0.0);
        assert!(!b.maintenance_lockout);
    }

    #[test]
    fn missing_points_are_explicitly_untrusted() {
        let mut ev = evaluator();
        let now = 10_000;
        // 全覆盖（1-8）但映射引用覆盖之外的点（9/10 = 保护标志未接入）。
        let samples = alloc::vec![
            sample(1, 380.0, 1, now),
            sample(2, 1.0, 1, now),
            sample(3, 60.0, 1, now),
            sample(4, 600.0, 1, now),
            sample(5, 0.0, 1, now),
            sample(6, 0.0, 1, now),
            sample(7, 10.0, 1, now),
            sample(8, 50.0, 1, now),
        ];
        let t = ev.evaluate(&samples, now).expect("trusted");
        // 映射引用覆盖之外的点（如保护标志尚未接入采集链）→ 显式未知。
        let b = TrustedFeedbackBuilder::new(FeedbackPointMapping {
            grid_up: 2,
            soc: 3,
            dc_bus_v: 4,
            overcurrent: 9,
            overtemp: 10,
        })
        .build(&t, 10);
        // 覆盖的点 Good；未覆盖的点 Invalid（不制造"无过流"默认值）。
        assert_eq!(b.soc_pct.quality, QualityFlag::Good);
        assert_eq!(b.overcurrent.quality, QualityFlag::Invalid);
        assert_eq!(b.overtemp.quality, QualityFlag::Invalid);
    }

    /// SPEC-R4 WP-R4.1：生产反馈映射单源值冻结（5/4/1/6/6）。6/6 别名
    /// 现状见常量文档；矫正走 spec R4 WP-R4.4（QEMU 门槛），不得私自改键。
    #[test]
    fn prod_feedback_mapping_frozen() {
        let m = PROD_FEEDBACK_MAPPING;
        assert_eq!(m.grid_up, 5);
        assert_eq!(m.soc, 4);
        assert_eq!(m.dc_bus_v, 1);
        assert_eq!(m.overcurrent, 6);
        assert_eq!(m.overtemp, 6);
    }
}
