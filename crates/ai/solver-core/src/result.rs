//! 求解结果与状态（D12）.

use alloc::string::String;
use alloc::vec::Vec;

/// 求解状态.
#[derive(Debug, Clone, PartialEq)]
pub enum SolveStatus {
    /// 最优解.
    Optimal,
    /// 次优解.
    Suboptimal,
    /// 不可行.
    Infeasible,
    /// 无界.
    Unbounded,
    /// 超时.
    Timeout,
    /// 错误（含错误消息）.
    Error(String),
}

/// 求解结果.
#[derive(Debug, Clone)]
pub struct SolveResult {
    /// 求解状态.
    pub status: SolveStatus,
    /// 目标函数值.
    pub objective_value: f64,
    /// 变量解值.
    pub solution: Vec<f64>,
    /// 求解耗时（毫秒，由 `now_ms` 参数计算，D1）.
    pub elapsed_ms: u64,
    /// 对偶解（影子价格）；MockSolver 返回 None.
    pub dual_solution: Option<Vec<f64>>,
}

impl SolveResult {
    /// 便捷构造：最优解 + 无对偶解.
    pub fn optimal(objective_value: f64, solution: Vec<f64>) -> Self {
        Self {
            status: SolveStatus::Optimal,
            objective_value,
            solution,
            elapsed_ms: 0,
            dual_solution: None,
        }
    }
}
