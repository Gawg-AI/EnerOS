//! MeterCalibration trait（v0.51.1）.
//!
//! 定义 [`MeterCalibration`] trait — 电能表校准的统一抽象，提供系数应用、
//! 误差测量与精度分级三个核心操作。
//!
//! > 模块名为 `trait_def` 而非 `trait`，因 `trait` 是 Rust 关键字。

use crate::accuracy::AccuracyClass;
use crate::coeffs::CalibCoeffs;
use crate::result::MeterReading;

/// 电能表校准抽象
///
/// 实现者负责：将校准系数应用到原始读数、测量被校表与标准源之间的误差、
/// 根据误差判定精度等级。默认实现见 [`crate::func::DefaultCalibrator`]。
pub trait MeterCalibration {
    /// 将校准系数应用到读数，返回校准后读数。
    fn apply_coefficients(&self, reading: &MeterReading, coeffs: &CalibCoeffs) -> MeterReading;

    /// 测量被校表与标准源之间的误差百分比。
    ///
    /// 返回 `(measured.power - reference.power) / reference.power * 100`。
    fn measure_error(&self, measured: &MeterReading, reference: &MeterReading) -> f64;

    /// 根据误差百分比判定可达到的最高精度等级。
    fn classify_accuracy(&self, error_pct: f64) -> AccuracyClass;
}
