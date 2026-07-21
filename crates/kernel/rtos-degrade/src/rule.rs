//! 降级规则 trait.
//!
//! [`DegradeRule`] 是降级引擎的核心抽象：每条规则根据 [`DegradeContext`]
//! 评估是否触发降级，返回 [`DegradeMode`] 或 `None`。

use crate::context::DegradeContext;
use crate::mode::DegradeMode;

/// 降级规则 trait（D6：不要求 Send + Sync）.
///
/// 每条规则有名称、优先级（u8，值越大优先级越高）、评估函数。
/// 引擎按优先级降序遍历规则，首个返回 `Some(mode)` 的规则决定降级模式。
pub trait DegradeRule {
    /// 规则名称（人类可读）。
    fn name(&self) -> &str;

    /// 优先级（0~255，值越大优先级越高）。
    fn priority(&self) -> u8;

    /// 评估上下文，返回触发的降级模式或 `None`（未触发）。
    fn evaluate(&self, ctx: &DegradeContext) -> Option<DegradeMode>;
}
