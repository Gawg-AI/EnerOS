//! 降级引擎统计与报告.
//!
//! [`DegradeStats`] 累计模式切换次数与评估次数（D7：不使用 AtomicU64，普通 u64）。
//! [`DegradeReport`] 描述单次评估结果。

use crate::mode::DegradeMode;

/// 降级引擎累计统计（D7：普通 u64，非 AtomicU64）.
#[derive(Debug, Clone, Default)]
pub struct DegradeStats {
    /// 模式切换总次数。
    pub mode_switch_count: u64,
    /// 评估总次数。
    pub evaluations_count: u64,
    /// 当前（最近一次）模式。
    pub last_mode: DegradeMode,
    /// 最近一次模式切换时间（纳秒）。
    pub last_mode_switch_ns: u64,
}

/// 单次评估报告.
#[derive(Debug, Clone, Default)]
pub struct DegradeReport {
    /// 评估得出的新模式。
    pub new_mode: DegradeMode,
    /// 模式是否发生切换。
    pub mode_changed: bool,
    /// 是否执行了下发动作。
    pub action_taken: bool,
}
