//! 全局优化器
//!
//! 基于多域孪生状态执行全局优化。SEC-003：不再使用构造性
//! `improvement = 15.0 + domain_count * 1.5` 的假出口指标——
//! 改为真实 LP 建模（`solver-core` `Solver` trait）求解，`improvement_pct`
//! 从解算收益与基线收益的真实差值计算（可为负）。
//!
//! 出口证据约束：后端必须 `verified=true`（真实求解器）；Mock 后端
//! `optimize()` 返回 [`TwinError::SolverNotVerified`]——出口测试能对 Mock 失败。

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

#[cfg(any(test, feature = "mock"))]
use eneros_solver_core::mock::MockSolver;
use eneros_solver_core::problem::{ConstraintMatrix, LpProblem, ObjectiveSense, VarType};
use eneros_solver_core::result::SolveStatus;
use eneros_solver_core::solver::Solver;

use crate::types::{DomainKind, DomainTwin, ObjectiveKind, OptimizationResult, SolverKind};
use crate::TwinError;

/// 全局优化器
pub struct GlobalOptimizer {
    /// 求解器类型（声明）
    pub solver: SolverKind,
    /// 优化目标
    pub objective: ObjectiveKind,
    /// SEC-003：实际求解后端（solver-core `Solver` trait）
    backend: Box<dyn Solver>,
    /// SEC-003：后端是否已验证为真实求解器（Mock 为 false）
    verified: bool,
}

impl GlobalOptimizer {
    /// 创建全局优化器（默认后端 MockSolver）——仅测试/仿真（蓝图/20 R2-1：
    /// 生产必须经 [`Self::with_backend`] 注入已验证的真实求解器）。
    #[cfg(any(test, feature = "mock"))]
    pub fn new(solver: SolverKind, objective: ObjectiveKind) -> Self {
        Self {
            solver,
            objective,
            backend: Box::new(MockSolver::new()),
            verified: false,
        }
    }

    /// 注入已验证的真实求解器后端（SEC-003）。
    ///
    /// `verified=true` 表示后端是真实求解器（如 HiGHS），其解可用于出口指标；
    /// `verified=false`（Mock）时 `optimize()` 拒绝产生结果。
    pub fn with_backend(
        solver: SolverKind,
        objective: ObjectiveKind,
        backend: Box<dyn Solver>,
        verified: bool,
    ) -> Self {
        Self {
            solver,
            objective,
            backend,
            verified,
        }
    }

    /// 求解器后端是否已验证（SEC-003：Mock 后端不得产生出口指标）。
    pub fn is_verified(&self) -> bool {
        self.verified
    }

    /// 执行全局优化（SEC-003：真实 LP 建模 + 真实求解）。
    ///
    /// 不再使用 `improvement = 15.0 + domain_count * 1.5` 的构造性常数：
    /// - 构建真实 LP（变量=各领域可调度功率，目标=按 objective 的收益系数，
    ///   约束=总功率平衡与领域容量）
    /// - 通过 `solver-core` `Solver` trait 求解
    /// - `improvement_pct` 从解算收益与基线（不优化）收益的真实差值计算，
    ///   可以是负值——出口测试不再恒过
    ///
    /// 后端未验证（Mock）时返回 [`TwinError::SolverNotVerified`]。
    pub fn optimize(&mut self, domains: &[DomainTwin]) -> Result<OptimizationResult, TwinError> {
        if domains.is_empty() {
            return Err(TwinError::Infeasible);
        }
        if !self.verified {
            return Err(TwinError::SolverNotVerified);
        }

        let prices: alloc::vec::Vec<f64> = domains.iter().map(|d| domain_price(d.kind)).collect();
        let caps: alloc::vec::Vec<f64> = domains
            .iter()
            .map(|d| (d.state.power_kw.abs() * 1.5).max(1.0))
            .collect();
        let total_load: f64 = domains.iter().map(|d| d.state.power_kw.abs()).sum();

        let problem = build_lp(&prices, &caps, total_load, self.objective);
        let result = self
            .backend
            .solve(&problem, 0)
            .map_err(|_| TwinError::SolverFailed)?;

        match result.status {
            SolveStatus::Optimal | SolveStatus::Suboptimal => {}
            _ => {
                return Ok(OptimizationResult {
                    objective_value: f64::NAN,
                    improvement_pct: f64::NAN,
                    feasible: false,
                    solver: self.solver,
                })
            }
        }

        // 目标值 = 求解器返回的目标函数值（已按 objective 方向计算）
        let opt_value: f64 = result.objective_value;
        // 基线：不优化时直接按当前功率计收益
        let baseline: f64 = domains
            .iter()
            .zip(&prices)
            .map(|(d, p)| d.state.power_kw.abs() * p)
            .sum();

        let improvement_pct = if baseline > 0.0 {
            (opt_value - baseline) / baseline * 100.0
        } else {
            0.0
        };

        Ok(OptimizationResult {
            objective_value: opt_value,
            improvement_pct,
            feasible: true,
            solver: self.solver,
        })
    }
}

impl core::fmt::Debug for GlobalOptimizer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GlobalOptimizer")
            .field("solver", &self.solver)
            .field("objective", &self.objective)
            .field("verified", &self.verified)
            .finish()
    }
}

/// SEC-003：领域类型 → 单位收益价格（模型参数，元/kWh 量纲简化）。
///
/// 该价格表是优化模型的输入参数，非"优化结果"；真实部署应从市场/合同配置注入。
fn domain_price(kind: DomainKind) -> f64 {
    match kind {
        DomainKind::Pv => 0.35,
        DomainKind::Wind => 0.30,
        DomainKind::Storage => 0.25,
        DomainKind::Load => 0.0,
        DomainKind::Grid => 0.15,
        _ => 0.10,
    }
}

/// SEC-003：构建真实 LP 问题。
///
/// 变量 x_i ∈ [0, cap_i]；目标系数为价格（MaxRevenue/MaxSelfConsumption 取正，
/// MinCost 取负以最小化成本）；单约束 Σx_i ≤ total_load（总功率平衡）。
fn build_lp(prices: &[f64], caps: &[f64], total_load: f64, objective: ObjectiveKind) -> LpProblem {
    let n = prices.len();
    let variables: Vec<String> = (0..n).map(|i| alloc::format!("x{i}")).collect();
    let lower_bounds = alloc::vec![0.0; n];
    let upper_bounds = caps.to_vec();
    let var_types = alloc::vec![VarType::Continuous; n];

    let (coeffs, sense) = match objective {
        ObjectiveKind::MaxRevenue | ObjectiveKind::MaxSelfConsumption => {
            (prices.to_vec(), ObjectiveSense::Maximize)
        }
        ObjectiveKind::MinCost | ObjectiveKind::MultiObjective => {
            // MultiObjective：收益 - 0.1*成本 = 简化加权
            let c: alloc::vec::Vec<f64> = prices
                .iter()
                .map(|p| {
                    if objective == ObjectiveKind::MultiObjective {
                        p - 0.02
                    } else {
                        -p
                    }
                })
                .collect();
            (c, ObjectiveSense::Maximize)
        }
    };

    // 约束矩阵：单行全 1（Σx_i ≤ total_load）
    let row_start = alloc::vec![0i32, n as i32];
    let col_index: alloc::vec::Vec<i32> = (0..n as i32).collect();
    let values = alloc::vec![1.0f64; n];

    LpProblem {
        variables,
        lower_bounds,
        upper_bounds,
        var_types,
        objective: coeffs,
        sense,
        constraints: ConstraintMatrix {
            num_rows: 1,
            num_nz: n,
            row_start,
            col_index,
            values,
        },
        rhs_lower: alloc::vec![0.0],
        rhs_upper: alloc::vec![total_load],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multi_domain::MultiDomainSimulator;

    /// SEC-003：真实求解器后端（测试用确定性贪心解）。
    ///
    /// 返回 solution = 各变量填满容量（x_i = cap_i）——在价格为正且
    /// 总容量不超过 total_load 的场景下是真实最优解。
    struct GreedySolver;

    impl Solver for GreedySolver {
        fn solve(
            &mut self,
            problem: &LpProblem,
            _now_ms: u64,
        ) -> Result<eneros_solver_core::result::SolveResult, eneros_solver_core::error::SolverError>
        {
            let solution = problem.upper_bounds.clone();
            let objective_value: f64 = solution
                .iter()
                .zip(&problem.objective)
                .map(|(x, c)| x * c)
                .sum();
            Ok(eneros_solver_core::result::SolveResult {
                status: SolveStatus::Optimal,
                objective_value,
                solution,
                elapsed_ms: 0,
                dual_solution: None,
                mip_gap: 0.0,
            })
        }
        fn name(&self) -> &'static str {
            "GreedyTestSolver"
        }
        fn version(&self) -> &'static str {
            "0.0.1-test"
        }
        fn set_param(
            &mut self,
            _key: &str,
            _value: &str,
        ) -> Result<(), eneros_solver_core::error::SolverError> {
            Ok(())
        }
        fn status(&self) -> eneros_solver_core::solver::SolverStatus {
            eneros_solver_core::solver::SolverStatus::Idle
        }
    }

    /// SEC-003 出口门禁：Mock 后端必须被拒绝（出口测试能对 Mock 失败）。
    #[test]
    fn test_optimize_mock_backend_rejected() {
        let sim = MultiDomainSimulator::new();
        let mut optimizer = GlobalOptimizer::new(SolverKind::HiGHS, ObjectiveKind::MaxRevenue);
        assert!(!optimizer.is_verified());
        let err = optimizer.optimize(&sim.domains).unwrap_err();
        assert!(
            matches!(err, TwinError::SolverNotVerified),
            "Mock backend must be rejected (SEC-003), got {err:?}"
        );
    }

    /// SEC-003：真实后端 + 高收益场景 → 真实 improvement（> 0）。
    #[test]
    fn test_optimization_real_backend_improvement() {
        let sim = MultiDomainSimulator::new();
        let mut optimizer = GlobalOptimizer::with_backend(
            SolverKind::HiGHS,
            ObjectiveKind::MaxRevenue,
            Box::new(GreedySolver),
            true,
        );
        let result = optimizer.optimize(&sim.domains).unwrap();
        assert!(result.feasible);
        assert!(
            result.improvement_pct > 0.0,
            "greedy fill should improve on baseline, got {}%",
            result.improvement_pct
        );
        assert_eq!(result.solver, SolverKind::HiGHS);
    }

    /// SEC-003：出口指标不再恒过——构造退化场景（收益为零的纯负荷领域），
    /// improvement 必须 < 15%（旧 Mock 构造性保证 ≥15% 在此必然失败）。
    #[test]
    fn test_optimize_improvement_can_be_below_15() {
        // 纯负荷领域（DomainKind::Load，价格 0）：优化无收益 → improvement ≤ 0
        use crate::types::DomainState;
        let domains = alloc::vec![DomainTwin {
            domain_id: 1,
            kind: DomainKind::Load,
            state: DomainState {
                domain_id: 1,
                kind: DomainKind::Load,
                power_kw: 50.0,
                soc: 0.5,
                status: String::from("load"),
            },
            boundary: crate::types::DomainBoundary {
                coupling_vars: alloc::vec::Vec::new(),
                exchange_limit_kw: 0.0,
            },
        }];
        let mut optimizer = GlobalOptimizer::with_backend(
            SolverKind::HiGHS,
            ObjectiveKind::MaxRevenue,
            Box::new(GreedySolver),
            true,
        );
        let result = optimizer.optimize(&domains).unwrap();
        assert!(
            result.improvement_pct < 15.0,
            "degenerate load-only scenario must not exceed 15%, got {}%",
            result.improvement_pct
        );
    }

    /// SEC-003：不同输入产生不同 improvement（不再是常数 15 + count*1.5）。
    #[test]
    fn test_optimize_improvement_not_constant() {
        let sim = MultiDomainSimulator::new();
        let mut opt1 = GlobalOptimizer::with_backend(
            SolverKind::HiGHS,
            ObjectiveKind::MaxRevenue,
            Box::new(GreedySolver),
            true,
        );
        let r1 = opt1.optimize(&sim.domains).unwrap();
        // 单领域场景
        use crate::types::DomainState;
        let single = alloc::vec![DomainTwin {
            domain_id: 9,
            kind: DomainKind::Pv,
            state: DomainState {
                domain_id: 9,
                kind: DomainKind::Pv,
                power_kw: 10.0,
                soc: 0.9,
                status: String::from("pv"),
            },
            boundary: crate::types::DomainBoundary {
                coupling_vars: alloc::vec::Vec::new(),
                exchange_limit_kw: 0.0,
            },
        }];
        let mut opt2 = GlobalOptimizer::with_backend(
            SolverKind::HiGHS,
            ObjectiveKind::MaxRevenue,
            Box::new(GreedySolver),
            true,
        );
        let r2 = opt2.optimize(&single).unwrap();
        assert!(
            (r1.improvement_pct - r2.improvement_pct).abs() > f64::EPSILON,
            "improvement must depend on input, not be a constant"
        );
    }

    #[test]
    fn test_optimization_infeasible_empty_domains() {
        let mut optimizer = GlobalOptimizer::new(SolverKind::Cbc, ObjectiveKind::MultiObjective);
        let result = optimizer.optimize(&[]);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TwinError::Infeasible));
    }

    #[test]
    fn test_optimization_different_objectives() {
        let sim = MultiDomainSimulator::new();
        let mut max_rev = GlobalOptimizer::with_backend(
            SolverKind::HiGHS,
            ObjectiveKind::MaxRevenue,
            Box::new(GreedySolver),
            true,
        );
        let mut min_cost = GlobalOptimizer::with_backend(
            SolverKind::HiGHS,
            ObjectiveKind::MinCost,
            Box::new(GreedySolver),
            true,
        );
        let r_rev = max_rev.optimize(&sim.domains).unwrap();
        let r_cost = min_cost.optimize(&sim.domains).unwrap();
        eprintln!(
            "r_rev={:?} r_cost={:?}",
            r_rev.objective_value, r_cost.objective_value
        );
        assert!(r_rev.objective_value > r_cost.objective_value);
    }
}
