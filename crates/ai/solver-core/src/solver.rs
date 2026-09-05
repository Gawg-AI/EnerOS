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

    /// 设置时间上限（秒）——类型化注入（蓝图 67 WP-67.4）。
    ///
    /// `set_param` stringly 通道下，整数值 0/1 被 classify 成 Bool 发给
    /// double 选项 → 真 HiGHS 1.7.2 实测 `ParamSetFailed`（[01] §2.4
    /// 实弹坐实）。数值参数一律走本方法；默认 Err = 诚实失败（不支持的
    /// 后端不得静默装成功）。
    fn set_time_limit_s(&mut self, _limit_s: f64) -> Result<(), SolverError> {
        Err(SolverError::ParamError(alloc::string::String::from(
            "set_time_limit_s unsupported by backend",
        )))
    }

    /// 设置 MIP 相对间隙——类型化注入（蓝图 67 WP-67.4；语义同
    /// [`Solver::set_time_limit_s`]）。
    fn set_mip_rel_gap(&mut self, _gap: f64) -> Result<(), SolverError> {
        Err(SolverError::ParamError(alloc::string::String::from(
            "set_mip_rel_gap unsupported by backend",
        )))
    }

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
