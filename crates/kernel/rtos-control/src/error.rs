//! 控制闭环错误类型.
//!
//! 定义 [`ControlError`]，覆盖设定值无效、反馈读取失败、输出写入失败、
//! 约束违反、循环 Panic、引擎满载等错误场景.

/// 控制闭环错误（6 变体）.
///
/// 派生 `Debug`/`Clone`/`PartialEq`，便于在测试中精确匹配错误类型.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlError {
    /// 设定值无效（超出允许范围或格式错误）.
    SetpointInvalid,
    /// 反馈读取失败（传感器通信错误或数据点不存在）.
    FeedbackReadFailed,
    /// 输出写入失败（执行机构通信错误或数据点不存在）.
    OutputWriteFailed,
    /// 约束违反（输出超出安全限制）.
    ConstraintViolation,
    /// 循环 Panic（控制循环内部发生不可恢复错误）.
    LoopPanic,
    /// 引擎满载（注册的循环数超过容量）.
    EngineFull,
}

impl core::fmt::Display for ControlError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ControlError::SetpointInvalid => write!(f, "setpoint invalid"),
            ControlError::FeedbackReadFailed => write!(f, "feedback read failed"),
            ControlError::OutputWriteFailed => write!(f, "output write failed"),
            ControlError::ConstraintViolation => write!(f, "constraint violation"),
            ControlError::LoopPanic => write!(f, "loop panic"),
            ControlError::EngineFull => write!(f, "engine full"),
        }
    }
}
