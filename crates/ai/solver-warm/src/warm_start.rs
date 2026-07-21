//! 热启动编排（`SolveContext` + `WarmStartProvider` + `WarmStarter` 降级链）.

use alloc::vec::Vec;

use eneros_solver_core::problem::LpProblem;
use eneros_solver_core::solver::Solver;
use eneros_solver_milp::DayAheadPlan;

use crate::candidate::CandidateSolution;
use crate::heuristic_net::WarmError;

/// 求解上下文（蓝图 §4.1）.
#[derive(Debug, Clone, Default)]
pub struct SolveContext {
    /// 负荷预测（MW，按周期）.
    pub load_forecast: Vec<f64>,
    /// 电价信号（按周期）.
    pub price_signal: Vec<f64>,
    /// 历史日前计划（复用 v0.102.0 `DayAheadPlan`）.
    pub history: Vec<DayAheadPlan>,
}

/// 热启动提供者（D5：无 Send + Sync bound，与 `Solver`/`LlmEngine` 惯例一致）.
pub trait WarmStartProvider {
    /// 生成热启动候选解.
    fn generate(
        &self,
        problem: &LpProblem,
        ctx: &SolveContext,
    ) -> Result<CandidateSolution, WarmError>;
}

/// 热启动编排器（蓝图 §4.3/§4.4，D8/D10）.
///
/// `plan_warm` 流程：生成 → 置信度阈值判定 → 投影合并 → 注入；
/// 生成失败/低置信/注入失败均回退冷启动，计数器字段可观测（D10）.
pub struct WarmStarter {
    /// 置信度阈值（构造注入，D8；`==` 阈值视为通过）.
    pub confidence_threshold: f64,
    /// 热启动注入成功次数.
    pub warm_used_count: u64,
    /// 低置信拒绝次数.
    pub warm_rejected_count: u64,
    /// 冷启动回退次数（生成失败 / 注入失败）.
    pub cold_fallback_count: u64,
}

impl WarmStarter {
    /// 构造（计数器清零）.
    pub fn new(confidence_threshold: f64) -> Self {
        Self {
            confidence_threshold,
            warm_used_count: 0,
            warm_rejected_count: 0,
            cold_fallback_count: 0,
        }
    }

    /// 规划热启动注入：返回 `Some(sol)` = 已注入；`None` = 回退冷启动.
    ///
    /// 流程（C51~C55）：`generate` Err → `cold_fallback_count += 1` 返回 None
    /// （不调 `set_warm_start`）；`confidence < threshold` →
    /// `warm_rejected_count += 1` 返回 None（不调 `set_warm_start`）；否则投影 +
    /// 合并 → `set_warm_start(&sol)` → `warm_used_count += 1` 返回 Some(sol)；
    /// 注入 Err 视同冷启动回退（`cold_fallback_count += 1` 返回 None）.
    pub fn plan_warm(
        &mut self,
        provider: &dyn WarmStartProvider,
        problem: &LpProblem,
        ctx: &SolveContext,
        solver: &mut dyn Solver,
    ) -> Option<Vec<f64>> {
        let mut candidate = match provider.generate(problem, ctx) {
            Ok(c) => c,
            Err(_) => {
                self.cold_fallback_count += 1;
                return None;
            }
        };
        if candidate.confidence < self.confidence_threshold {
            self.warm_rejected_count += 1;
            return None;
        }
        candidate.project(problem);
        let sol = candidate.to_solution(problem);
        match solver.set_warm_start(&sol) {
            Ok(()) => {
                self.warm_used_count += 1;
                Some(sol)
            }
            Err(_) => {
                self.cold_fallback_count += 1;
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use eneros_solver_core::error::SolverError;
    use eneros_solver_core::mock::MockSolver;
    use eneros_solver_core::problem::{ConstraintMatrix, ObjectiveSense, VarType};
    use eneros_solver_core::result::SolveResult;
    use eneros_solver_core::solver::SolverStatus;

    use super::*;
    use crate::heuristic_net::{HeuristicNet, MockEngine};

    fn problem(var_types: &[VarType], lower: &[f64], upper: &[f64]) -> LpProblem {
        let n = var_types.len();
        LpProblem {
            variables: (0..n).map(|i| alloc::format!("x{}", i)).collect(),
            lower_bounds: lower.to_vec(),
            upper_bounds: upper.to_vec(),
            var_types: var_types.to_vec(),
            objective: alloc::vec![0.0; n],
            sense: ObjectiveSense::Minimize,
            constraints: ConstraintMatrix::new(0, 0, alloc::vec![0], alloc::vec![], alloc::vec![]),
            rhs_lower: alloc::vec![],
            rhs_upper: alloc::vec![],
        }
    }

    /// 2 列混排问题 [Continuous, Binary]（界 [0,10] / [0,1]）.
    fn mixed_problem() -> LpProblem {
        problem(
            &[VarType::Continuous, VarType::Binary],
            &[0.0, 0.0],
            &[10.0, 1.0],
        )
    }

    /// 固定返回候选/错误的 provider stub.
    struct FixedProvider {
        result: Result<CandidateSolution, WarmError>,
    }

    impl WarmStartProvider for FixedProvider {
        fn generate(
            &self,
            _problem: &LpProblem,
            _ctx: &SolveContext,
        ) -> Result<CandidateSolution, WarmError> {
            self.result.clone()
        }
    }

    /// `set_warm_start` 永远 Err 的求解器 stub（C55 分支）.
    struct RejectSolver;

    impl Solver for RejectSolver {
        fn solve(
            &mut self,
            _problem: &LpProblem,
            _now_ms: u64,
        ) -> Result<SolveResult, SolverError> {
            Ok(SolveResult::optimal(0.0, vec![]))
        }
        fn name(&self) -> &'static str {
            "RejectSolver"
        }
        fn version(&self) -> &'static str {
            "0"
        }
        fn set_param(&mut self, _key: &str, _value: &str) -> Result<(), SolverError> {
            Ok(())
        }
        fn status(&self) -> SolverStatus {
            SolverStatus::Idle
        }
        fn set_warm_start(&mut self, _solution: &[f64]) -> Result<(), SolverError> {
            Err(SolverError::NotImplemented)
        }
    }

    /// TW21：成功注入 Some + `MockSolver.warm_start` 记录一致（C54/C56）.
    #[test]
    fn tw21_success_injected_and_recorded() {
        let provider = FixedProvider {
            result: Ok(CandidateSolution::new(vec![1.0], vec![1], 0.9)),
        };
        let mut starter = WarmStarter::new(0.5);
        let mut mock = MockSolver::new();
        let p = mixed_problem();
        let ctx = SolveContext::default();
        let sol = starter.plan_warm(&provider, &p, &ctx, &mut mock);
        assert_eq!(sol, Some(vec![1.0, 1.0]));
        assert_eq!(mock.warm_start, Some(vec![1.0, 1.0]));
        assert_eq!(starter.warm_used_count, 1);
        assert_eq!(starter.warm_rejected_count, 0);
        assert_eq!(starter.cold_fallback_count, 0);
    }

    /// TW22：低置信 → None + rejected==1，不调 `set_warm_start`（C52）.
    #[test]
    fn tw22_low_confidence_rejected() {
        let provider = FixedProvider {
            result: Ok(CandidateSolution::new(vec![1.0], vec![1], 0.3)),
        };
        let mut starter = WarmStarter::new(0.5);
        let mut mock = MockSolver::new();
        let p = mixed_problem();
        let ctx = SolveContext::default();
        let sol = starter.plan_warm(&provider, &p, &ctx, &mut mock);
        assert!(sol.is_none());
        assert_eq!(starter.warm_rejected_count, 1);
        assert_eq!(starter.warm_used_count, 0);
        assert!(mock.warm_start.is_none());
    }

    /// TW23：generate Err → None + fallback==1，不调 `set_warm_start`（C51）.
    #[test]
    fn tw23_generate_err_fallback() {
        let provider = FixedProvider {
            result: Err(WarmError::InferenceFailed(-1)),
        };
        let mut starter = WarmStarter::new(0.5);
        let mut mock = MockSolver::new();
        let p = mixed_problem();
        let ctx = SolveContext::default();
        let sol = starter.plan_warm(&provider, &p, &ctx, &mut mock);
        assert!(sol.is_none());
        assert_eq!(starter.cold_fallback_count, 1);
        assert_eq!(starter.warm_used_count, 0);
        assert!(mock.warm_start.is_none());
    }

    /// TW24：`set_warm_start` Err → 视同冷启动回退（C55）：None + fallback==1.
    #[test]
    fn tw24_inject_err_fallback() {
        let provider = FixedProvider {
            result: Ok(CandidateSolution::new(vec![1.0], vec![1], 0.9)),
        };
        let mut starter = WarmStarter::new(0.5);
        let mut solver = RejectSolver;
        let p = mixed_problem();
        let ctx = SolveContext::default();
        let sol = starter.plan_warm(&provider, &p, &ctx, &mut solver);
        assert!(sol.is_none());
        assert_eq!(starter.cold_fallback_count, 1);
        assert_eq!(starter.warm_used_count, 0);
    }

    /// TW25：计数器真值——三分支各调用 1 次后 used==1 / rejected==1 / fallback==1（C57）.
    #[test]
    fn tw25_counter_truth_table() {
        let p = mixed_problem();
        let ctx = SolveContext::default();
        let mut starter = WarmStarter::new(0.5);
        let mut mock = MockSolver::new();
        // 成功分支
        let ok = FixedProvider {
            result: Ok(CandidateSolution::new(vec![1.0], vec![1], 0.9)),
        };
        assert!(starter.plan_warm(&ok, &p, &ctx, &mut mock).is_some());
        // 低置信分支
        let low = FixedProvider {
            result: Ok(CandidateSolution::new(vec![1.0], vec![1], 0.1)),
        };
        assert!(starter.plan_warm(&low, &p, &ctx, &mut mock).is_none());
        // 生成 Err 分支
        let err = FixedProvider {
            result: Err(WarmError::InvalidDim),
        };
        assert!(starter.plan_warm(&err, &p, &ctx, &mut mock).is_none());
        assert_eq!(starter.warm_used_count, 1);
        assert_eq!(starter.warm_rejected_count, 1);
        assert_eq!(starter.cold_fallback_count, 1);
    }

    /// TW26：threshold 边界——confidence == threshold 视为通过（C53）.
    #[test]
    fn tw26_threshold_equal_passes() {
        let provider = FixedProvider {
            result: Ok(CandidateSolution::new(vec![1.0], vec![1], 0.5)),
        };
        let mut starter = WarmStarter::new(0.5);
        let mut mock = MockSolver::new();
        let p = mixed_problem();
        let ctx = SolveContext::default();
        let sol = starter.plan_warm(&provider, &p, &ctx, &mut mock);
        assert_eq!(sol, Some(vec![1.0, 1.0]));
        assert_eq!(starter.warm_used_count, 1);
        assert_eq!(starter.warm_rejected_count, 0);
    }

    /// TW27：返回解向量内容 == 投影合并结果（C56：12.0 clamp → 10.0）.
    #[test]
    fn tw27_solution_equals_projected_merge() {
        let provider = FixedProvider {
            result: Ok(CandidateSolution::new(vec![12.0], vec![1], 0.9)),
        };
        let mut starter = WarmStarter::new(0.5);
        let mut mock = MockSolver::new();
        let p = mixed_problem();
        let ctx = SolveContext::default();
        let sol = starter.plan_warm(&provider, &p, &ctx, &mut mock);
        // project：连续列 12.0 clamp 到上界 10.0；整数列 1 不动
        assert_eq!(sol, Some(vec![10.0, 1.0]));
        assert_eq!(mock.warm_start, Some(vec![10.0, 1.0]));
    }

    /// TW28：空 SolveContext（Default）可用，不 panic（C59）.
    #[test]
    fn tw28_empty_ctx_usable() {
        let provider = FixedProvider {
            result: Ok(CandidateSolution::new(vec![1.0], vec![1], 0.9)),
        };
        let mut starter = WarmStarter::new(0.5);
        let mut mock = MockSolver::new();
        let p = mixed_problem();
        let ctx = SolveContext::default();
        assert!(ctx.load_forecast.is_empty());
        assert!(ctx.price_signal.is_empty());
        assert!(ctx.history.is_empty());
        assert!(starter.plan_warm(&provider, &p, &ctx, &mut mock).is_some());
    }

    /// TW29：`WarmStarter::new` 计数器清零；WarmError 变体 Debug/Clone/PartialEq（C34/C50）.
    #[test]
    fn tw29_new_counters_zero_and_warm_error_variants() {
        let s = WarmStarter::new(0.5);
        assert_eq!(s.confidence_threshold, 0.5);
        assert_eq!(s.warm_used_count, 0);
        assert_eq!(s.warm_rejected_count, 0);
        assert_eq!(s.cold_fallback_count, 0);
        // WarmError：Clone / PartialEq / Debug
        let e = WarmError::InferenceFailed(-1);
        assert_eq!(e.clone(), WarmError::InferenceFailed(-1));
        assert_ne!(e, WarmError::InvalidDim);
        assert_ne!(WarmError::ModelLoadFailed, WarmError::InvalidDim);
        assert!(!alloc::format!("{:?}", e).is_empty());
    }

    /// TW30：provider dyn seam——`&dyn WarmStartProvider` 可传 `HeuristicNet<MockEngine>`（C58）.
    #[test]
    fn tw30_provider_dyn_seam() {
        let net = HeuristicNet::new(MockEngine::new(vec![0.9]));
        let provider: &dyn WarmStartProvider = &net;
        let mut starter = WarmStarter::new(0.1);
        let mut mock = MockSolver::new();
        let p = problem(&[VarType::Binary], &[0.0], &[1.0]);
        let ctx = SolveContext::default();
        let sol = starter.plan_warm(provider, &p, &ctx, &mut mock);
        assert_eq!(sol, Some(vec![1.0]));
        assert_eq!(mock.warm_start, Some(vec![1.0]));
        assert_eq!(starter.warm_used_count, 1);
    }
}
