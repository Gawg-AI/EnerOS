//! 校准函数（v0.51.1）.
//!
//! 提供 [`calibrate_meter`] — 依据标准源推导校准系数并判定合格性，
//! [`verify_accuracy`] — 校验读数是否满足目标精度等级，
//! 以及 [`DefaultCalibrator`] — [`MeterCalibration`] 的默认实现。

use crate::accuracy::{is_within_class, AccuracyClass};
use crate::coeffs::CalibCoeffs;
use crate::result::{CalibResult, MeterReading};
use crate::trait_def::MeterCalibration;

/// 默认校准器
///
/// 实现 [`MeterCalibration`]，提供标准的系数应用、误差测量与精度分级逻辑。
/// [`calibrate_meter`] 与 [`verify_accuracy`] 内部使用本结构体。
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultCalibrator;

impl MeterCalibration for DefaultCalibrator {
    fn apply_coefficients(&self, reading: &MeterReading, coeffs: &CalibCoeffs) -> MeterReading {
        let voltage = coeffs.apply_voltage(reading.voltage);
        let current = coeffs.apply_current(reading.current);
        // 校准后功率由校正后的电压/电流重算（假设功率因数已隐含在采样中）。
        let power = voltage * current;
        // 电度量按变比乘积缩放（无偏置项）。
        let energy = reading.energy * coeffs.ct_ratio * coeffs.pt_ratio;
        MeterReading {
            voltage,
            current,
            power,
            energy,
        }
    }

    fn measure_error(&self, measured: &MeterReading, reference: &MeterReading) -> f64 {
        if reference.power == 0.0 {
            return 0.0;
        }
        (measured.power - reference.power) / reference.power * 100.0
    }

    fn classify_accuracy(&self, error_pct: f64) -> AccuracyClass {
        let abs_err = error_pct.abs();
        if abs_err <= AccuracyClass::Class0_2S.max_error_pct() {
            AccuracyClass::Class0_2S
        } else if abs_err <= AccuracyClass::Class0_5S.max_error_pct() {
            AccuracyClass::Class0_5S
        } else if abs_err <= AccuracyClass::Class1_0.max_error_pct() {
            AccuracyClass::Class1_0
        } else {
            AccuracyClass::Class2_0
        }
    }
}

/// 校准电能表
///
/// 依据被校表读数 `measured` 与标准源读数 `reference`，结合给定 CT/PT 变比，
/// 推导出电压/电流偏置，使校准后读数逼近标准源。返回完整校准结果。
///
/// - `before_error_pct`：未应用系数时的功率误差百分比
/// - `after_error_pct`：应用推导系数后的功率误差百分比
/// - `passed`：`after_error_pct` 是否落在 `target_class` 允许范围内
pub fn calibrate_meter(
    ct_ratio: f64,
    pt_ratio: f64,
    measured: &MeterReading,
    reference: &MeterReading,
    target_class: AccuracyClass,
    now_ms: u64,
) -> CalibResult {
    let calibrator = DefaultCalibrator;

    // 校准前误差（原始读数 vs 标准源）
    let before_error_pct = calibrator.measure_error(measured, reference);

    // 推导偏置：使 apply 后的电压/电流逼近标准源读数
    let offset_voltage = reference.voltage - measured.voltage * pt_ratio;
    let offset_current = reference.current - measured.current * ct_ratio;

    let coeffs = CalibCoeffs {
        ct_ratio,
        pt_ratio,
        phase_correction: 0.0,
        offset_voltage,
        offset_current,
        calibrated_at: now_ms,
    };

    // 校准后读数与误差
    let calibrated = calibrator.apply_coefficients(measured, &coeffs);
    let after_error_pct = calibrator.measure_error(&calibrated, reference);
    let passed = is_within_class(after_error_pct, target_class);

    CalibResult {
        before_error_pct,
        after_error_pct,
        target_class,
        passed,
        coeffs,
        measured_at: now_ms,
    }
}

/// 校验精度
///
/// 计算 `measured` 相对 `reference` 的功率误差，判定是否落在 `target_class` 允许范围内。
pub fn verify_accuracy(
    measured: &MeterReading,
    reference: &MeterReading,
    target_class: AccuracyClass,
) -> bool {
    let calibrator = DefaultCalibrator;
    let error_pct = calibrator.measure_error(measured, reference);
    is_within_class(error_pct, target_class)
}
