//! 精度等级（v0.51.1）.
//!
//! 定义 [`AccuracyClass`] 枚举，覆盖电能计量常见 4 个精度等级
//! （0.2S / 0.5S / 1.0 / 2.0），并提供最大允许误差查询与合格判定。

/// 电能表精度等级
///
/// 对应 GB/T 17215.321 与 IEC 62053 系列标准定义的电能表精度等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccuracyClass {
    /// 0.2S 级（高精度关口表）
    Class0_2S,
    /// 0.5S 级
    Class0_5S,
    /// 1.0 级
    Class1_0,
    /// 2.0 级
    Class2_0,
}

impl AccuracyClass {
    /// 返回该精度等级允许的最大误差百分比。
    pub fn max_error_pct(&self) -> f64 {
        match self {
            AccuracyClass::Class0_2S => 0.2,
            AccuracyClass::Class0_5S => 0.5,
            AccuracyClass::Class1_0 => 1.0,
            AccuracyClass::Class2_0 => 2.0,
        }
    }
}

/// 判定给定误差百分比是否落在指定精度等级允许范围内。
///
/// `|error_pct| <= class.max_error_pct()` 时返回 `true`。
pub fn is_within_class(error_pct: f64, class: AccuracyClass) -> bool {
    error_pct.abs() <= class.max_error_pct()
}
