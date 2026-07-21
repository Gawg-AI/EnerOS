//! 校准读数与结果（v0.51.1）.
//!
//! 定义 [`MeterReading`] — 单次抄表读数，[`CalibResult`] — 一次校准的完整结果。

use crate::accuracy::AccuracyClass;
use crate::coeffs::CalibCoeffs;

/// 电能表读数
///
/// 一次抄表获得的四维读数。`power` 为有功功率（W），`energy` 为累计电度量（Wh）。
#[derive(Debug, Clone, PartialEq)]
pub struct MeterReading {
    /// 电压（V）
    pub voltage: f64,
    /// 电流（A）
    pub current: f64,
    /// 有功功率（W）
    pub power: f64,
    /// 累计电度量（Wh）
    pub energy: f64,
}

/// 校准结果
///
/// 记录一次校准前后的误差、目标精度等级、是否通过、推导出的校准系数
/// 与测量时间戳。
#[derive(Debug, Clone)]
pub struct CalibResult {
    /// 校准前误差百分比（未应用系数）
    pub before_error_pct: f64,
    /// 校准后误差百分比（已应用系数）
    pub after_error_pct: f64,
    /// 目标精度等级
    pub target_class: AccuracyClass,
    /// 是否通过目标精度等级
    pub passed: bool,
    /// 推导出的校准系数
    pub coeffs: CalibCoeffs,
    /// 测量时间戳（ms）
    pub measured_at: u64,
}
