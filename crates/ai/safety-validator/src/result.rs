//! 校验结果类型（Severity / Violation / ValidationResult）.
//!
//! D8：`Severity` 派生 `Debug + Clone + Copy + PartialEq`（需 `==` 比较）。
//! D9：`ValidationResult` / `Violation` 仅派生 `Debug + Clone`（不派生 `PartialEq`，
//!     Karpathy "Simplicity First"：当前测试不需要）。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_energy_lp_model::result::ScheduleResult;

/// 违规严重等级.
///
/// - `Info`：信息性提示，不截断。
/// - `Warning`：警告，不截断。
/// - `Critical`：临界违规，截断到安全边界，继续执行后续规则。
/// - `Fatal`：致命违规，截断后立即终止后续规则，`passed = false`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Severity {
    /// 信息性提示。
    Info,
    /// 警告。
    Warning,
    /// 临界：截断 + 继续。
    Critical,
    /// 致命：截断 + 终止。
    Fatal,
}

/// 单条违规记录.
#[derive(Debug, Clone)]
pub struct Violation {
    /// 触发规则名（如 `"electrical_safety"` / `"protection_coordination"`）。
    pub rule: String,
    /// 时段索引。
    pub period: usize,
    /// 违规字段名（如 `"charge_power"` / `"soc"` / `"ramp_rate"`）。
    pub field: String,
    /// 原始值。
    pub original_value: f64,
    /// 安全值（截断后值）。
    pub safe_value: f64,
    /// 严重等级。
    pub severity: Severity,
}

/// 校验结果.
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// 是否通过（无致命违规即视为通过）。
    pub passed: bool,
    /// 是否发生截断。
    pub clamped: bool,
    /// 截断后的调度方案（仅当 `clamped = true` 时为 `Some`）。
    pub clamped_schedule: Option<ScheduleResult>,
    /// 累积违规列表。
    pub violations: Vec<Violation>,
}
