//! Solver trait 与运行时状态（D1/D8）.

use crate::error::SolverError;
use crate::problem::LpProblem;
use crate::result::SolveResult;

/// 求解器运行时状态（区别于 `SolveStatus` 求解结果状态）.
#[derive(Debug, Clone, PartialEq)]
pub enum SolverStatus {
    /// 空闲.
    Idle,
    /// 求解中.
    Solving,
    /// 错误.
    Error,
}

/// 求解器统一抽象.
///
/// 所有 LP/MIP 求解器实现此 trait。trait 不要求 `Send + Sync`
/// （与 v0.59.0 `LlmEngine` 一致；HiGHS 对象非线程安全）。
pub trait Solver {
    /// 求解优化问题.
    ///
    /// `now_ms` 参数用于计算 `elapsed_ms`（替代 `Instant::now()`，D1，
    /// 参考 v0.57.0 `now_ns` 模式）。
    fn solve(&mut self, problem: &LpProblem, now_ms: u64) -> Result<SolveResult, SolverError>;

    /// 获取求解器名称（D8：`&'static str` 避免 alloc）.
    fn name(&self) -> &'static str;

    /// 获取求解器版本（D8：`&'static str` 避免 alloc）.
    fn version(&self) -> &'static str;

    /// 设置求解器参数.
    fn set_param(&mut self, key: &str, value: &str) -> Result<(), SolverError>;

    /// 获取求解器运行时状态.
    fn status(&self) -> SolverStatus;

    /// 注入热启动初始解（v0.103.0 增量，D8）.
    ///
    /// 默认 no-op：不支持热启动的求解器静默忽略，保证向后兼容（非 BREAKING）。
    /// `solution` 为完整解向量（长度 == 问题变量数，连续/整数列已按 var_types 合并）。
    fn set_warm_start(&mut self, _solution: &[f64]) -> Result<(), SolverError> {
        Ok(())
    }
}
