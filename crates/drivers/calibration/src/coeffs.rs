//! 校准系数（v0.51.1）.
//!
//! 定义 [`CalibCoeffs`] — 电能表校准系数集合，包含 CT/PT 变比、相位校正、
//! 电压/电流偏置与校准时间戳。提供 `apply_voltage` / `apply_current` 方法
//! 将原始采样值映射为校准后工程量。

/// 电能表校准系数
///
/// 描述一只电能表从原始采样到工程量所需的全部线性校正参数。
/// 校准流程（[`crate::func::calibrate_meter`]）依据标准源读数与被校表读数
/// 推导出本结构体，持久化后由协议栈在每次抄表时应用。
#[derive(Debug, Clone, PartialEq)]
pub struct CalibCoeffs {
    /// CT 变比（如 1000/5 = 200.0）
    pub ct_ratio: f64,
    /// PT 变比（如 10000/100 = 100.0）
    pub pt_ratio: f64,
    /// 相位校正（度）
    pub phase_correction: f64,
    /// 电压偏置（V）
    pub offset_voltage: f64,
    /// 电流偏置（A）
    pub offset_current: f64,
    /// 校准时间戳（ms）
    pub calibrated_at: u64,
}

impl Default for CalibCoeffs {
    fn default() -> Self {
        Self {
            ct_ratio: 1.0,
            pt_ratio: 1.0,
            phase_correction: 0.0,
            offset_voltage: 0.0,
            offset_current: 0.0,
            calibrated_at: 0,
        }
    }
}

impl CalibCoeffs {
    /// 将原始电压采样值映射为校准后电压工程量。
    ///
    /// 公式：`raw * pt_ratio + offset_voltage`
    pub fn apply_voltage(&self, raw: f64) -> f64 {
        raw * self.pt_ratio + self.offset_voltage
    }

    /// 将原始电流采样值映射为校准后电流工程量。
    ///
    /// 公式：`raw * ct_ratio + offset_current`
    pub fn apply_current(&self, raw: f64) -> f64 {
        raw * self.ct_ratio + self.offset_current
    }
}
