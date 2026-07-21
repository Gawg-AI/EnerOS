//! MockSolver 默认实现（D2/D10）.
//!
//! 纯 Rust，零 `unsafe`，零外部依赖。返回预设结果，用于测试与开发.

use crate::error::SolverError;
use crate::problem::LpProblem;
use crate::result::{SolveResult, SolveStatus};
use crate::solver::{Solver, SolverStatus};

/// Mock 求解器.
///
/// 默认返回 `SolveStatus::Optimal` + `objective_value=0.0` + `solution=vec![]`；
/// 可通过 `with_result` 自定义返回结果。无 params 缓存（D3）.
pub struct MockSolver {
    /// 预设结果.
    preset_result: SolveResult,
    /// 末次注入的热启动解（v0.103.0 增量）.
    pub warm_start: Option<alloc::vec::Vec<f64>>,
}

impl MockSolver {
    /// 创建默认 Mock 求解器.
    pub fn new() -> Self {
        Self {
            preset_result: SolveResult {
                status: SolveStatus::Optimal,
                objective_value: 0.0,
                solution: alloc::vec![],
                elapsed_ms: 0,
                dual_solution: None,
            },
            warm_start: None,
        }
    }

    /// 创建自定义 Mock 求解器.
    pub fn with_result(result: SolveResult) -> Self {
        Self {
            preset_result: result,
            warm_start: None,
        }
    }
}

impl Default for MockSolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Solver for MockSolver {
    fn solve(&mut self, _problem: &LpProblem, _now_ms: u64) -> Result<SolveResult, SolverError> {
        Ok(self.preset_result.clone())
    }

    fn name(&self) -> &'static str {
        "MockSolver"
    }

    fn version(&self) -> &'static str {
        "0.1.0"
    }

    fn set_param(&mut self, _key: &str, _value: &str) -> Result<(), SolverError> {
        Ok(())
    }

    fn status(&self) -> SolverStatus {
        SolverStatus::Idle
    }

    fn set_warm_start(&mut self, solution: &[f64]) -> Result<(), SolverError> {
        self.warm_start = Some(alloc::vec::Vec::from(solution));
        Ok(())
    }
}
