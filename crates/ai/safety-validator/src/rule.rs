//! `SafetyRule` trait（D1：无 `Send + Sync` bound）.
//!
//! 蓝图原文 `pub trait SafetyRule: Send + Sync`（line 13987），但本 crate 在
//! no_std 单线程环境运行，`Send + Sync` 无意义；与 v0.59.0 `LlmEngine` /
//! v0.63.0 `PromptTemplate` / v0.64.0 `Solver` trait 一致移除。

use eneros_energy_lp_model::result::ScheduleResult;

use crate::result::ValidationResult;
use crate::state::SystemState;

/// 校验规则抽象（D1：无 `Send + Sync`）.
///
/// 实现者需提供 `name()` / `validate()`；`priority()` / `is_hard()` 提供默认值。
/// `priority()` 数值越小优先级越高（`SafetyValidator` 按 `priority()` 升序排序）。
pub trait SafetyRule {
    /// 规则名（如 `"electrical_safety"`）。
    fn name(&self) -> &str;

    /// 执行校验，返回 `ValidationResult`（含可能的截断调度方案）。
    fn validate(&self, schedule: &ScheduleResult, state: &SystemState) -> ValidationResult;

    /// 优先级（数值越小越靠前；默认 100）。
    fn priority(&self) -> u32 {
        100
    }

    /// 是否硬约束（默认 true）。
    fn is_hard(&self) -> bool {
        true
    }
}
