//! 求解 gate（[09] §3.5）：backend 状态映射 + 解向量验收 + 残差/整数性
//! /gap/deadline 判定。**任何不合格结果在 parse 前拒绝**。

use eneros_solver_core::result::{SolveResult, SolveStatus};

use crate::builder::BuiltModel;
use crate::tolerance::ToleranceProfile;
use crate::PlanError;

/// gate 裁决（全部数值进 CandidatePlan）。
#[derive(Debug, Clone, PartialEq)]
pub struct GateVerdict {
    pub status: eneros_decision_contract::SolveStatusKind,
    pub objective_value: f64,
    pub relative_gap: f64,
    pub max_primal_residual: f64,
    pub max_integrality_residual: f64,
}

pub struct SolveGate {
    profile: ToleranceProfile,
}

impl SolveGate {
    pub fn new(profile: ToleranceProfile) -> Self {
        Self { profile }
    }

    /// 求解结果判定（[09] §3.5 状态映射 + 残差 gate）。
    ///
    /// - backend error → Rejected
    /// - Infeasible / Unbounded / Error → Rejected
    /// - Timeout（无 incumbent 概念，一律拒绝——登记：SolveResult 无
    ///   incumbent 字段，保守 fail-closed）
    /// - Optimal → 残差 gate
    /// - Suboptimal → 策略 gap + 残差 + deadline gate
    pub(crate) fn evaluate(
        &self,
        result: &SolveResult,
        built: &BuiltModel,
        elapsed_ms: u64,
        budget_ms: u32,
    ) -> Result<GateVerdict, PlanError> {
        // 1. 状态映射。
        let status = match &result.status {
            SolveStatus::Optimal => eneros_decision_contract::SolveStatusKind::Optimal,
            SolveStatus::Suboptimal => {
                if !self.profile.accepts_suboptimal() {
                    return Err(PlanError::GapExceeded {
                        gap: 0.0,
                        limit: 0.0,
                    });
                }
                eneros_decision_contract::SolveStatusKind::Suboptimal
            }
            SolveStatus::Infeasible => return Err(PlanError::Rejected("infeasible".into())),
            SolveStatus::Unbounded => return Err(PlanError::Rejected("unbounded".into())),
            SolveStatus::Timeout => {
                return Err(PlanError::Rejected("timeout without incumbent".into()))
            }
            SolveStatus::Error(m) => return Err(PlanError::Rejected(m.clone())),
        };

        // 2. 解向量验收（长度精确相等 + 全有限——不 default-fill）。
        if result.solution.len() != built.problem.variables.len() {
            return Err(PlanError::MalformedSolution("length mismatch"));
        }
        if result.solution.iter().any(|v| !v.is_finite()) {
            return Err(PlanError::MalformedSolution("non-finite value"));
        }
        if !result.objective_value.is_finite() {
            return Err(PlanError::MalformedSolution("objective non-finite"));
        }

        // 3. 残差 gate（planner 独立重算——不采信 backend 自述）。
        let max_primal_residual = primal_residual(&built.problem, &result.solution);
        let scale = model_scale(&built.problem);
        let limit = self.profile.primal_limit(scale);
        if max_primal_residual > limit {
            return Err(PlanError::ResidualExceeded {
                primal: max_primal_residual,
                limit,
            });
        }
        // 4. 整数性残差（binary/integer 列；目录种类为闭枚举，直接判定）。
        let mut max_int = 0.0f64;
        for col in built.catalog.columns() {
            if col.kind.is_discrete() {
                let v = result.solution[col.index];
                max_int = max_int.max((v - v.round()).abs());
            }
        }
        if max_int > self.profile.integrality_residual {
            return Err(PlanError::IntegralityExceeded {
                residual: max_int,
                limit: self.profile.integrality_residual,
            });
        }
        // 5. gap gate（Suboptimal 需 backend 报告 gap；**未报告 = 无限大**
        // ——fail-closed，不当作 0 精确语义。Optimal 的最优性由 solver
        // 状态自证，gap 恒 0——不透传 backend 原始值：LP 模型的 info
        // mip_gap = kHighInf(1e30) 哨兵，透传即伪证据）。
        let gap = match (status, result.mip_gap) {
            (eneros_decision_contract::SolveStatusKind::Optimal, _) => 0.0,
            (eneros_decision_contract::SolveStatusKind::Suboptimal, Some(g)) => g,
            (eneros_decision_contract::SolveStatusKind::Suboptimal, None) => f64::INFINITY,
        };
        if status == eneros_decision_contract::SolveStatusKind::Suboptimal
            && gap > self.profile.max_relative_gap
        {
            return Err(PlanError::GapExceeded {
                gap: gap.min(f64::MAX / 4.0),
                limit: self.profile.max_relative_gap,
            });
        }
        // 6. deadline gate：budget 已由调用方扣除下游 margin
        // （budget = req.budget_ms() − margin），故 elapsed ≤ budget 即
        // elapsed + margin ≤ 总预算（[09] §3.5）。
        if elapsed_ms > budget_ms as u64 {
            return Err(PlanError::DeadlineExceeded {
                elapsed_ms,
                budget_ms,
            });
        }
        Ok(GateVerdict {
            status,
            objective_value: result.objective_value,
            relative_gap: gap,
            max_primal_residual,
            max_integrality_residual: max_int,
        })
    }
}

/// 最大原始残差：行违约 + 列边界违约（未缩放）。
pub fn primal_residual(problem: &eneros_solver_core::problem::LpProblem, x: &[f64]) -> f64 {
    let mut max_viol = 0.0f64;
    // 列边界。
    for (i, &v) in x.iter().enumerate() {
        let lo = problem.lower_bounds.get(i).copied().unwrap_or(f64::MIN);
        let hi = problem.upper_bounds.get(i).copied().unwrap_or(f64::MAX);
        max_viol = max_viol.max((lo - v).max(0.0)).max((v - hi).max(0.0));
    }
    // 行（CSR）。
    let m = &problem.constraints;
    for row in 0..m.num_rows {
        let s = m.row_start[row] as usize;
        let e = m.row_start[row + 1] as usize;
        let mut lhs = 0.0;
        for k in s..e {
            let col = m.col_index[k] as usize;
            if let Some(&v) = x.get(col) {
                lhs += m.values[k] * v;
            }
        }
        let lo = problem.rhs_lower.get(row).copied().unwrap_or(f64::MIN);
        let hi = problem.rhs_upper.get(row).copied().unwrap_or(f64::MAX);
        max_viol = max_viol.max((lo - lhs).max(0.0)).max((lhs - hi).max(0.0));
    }
    max_viol
}

/// 模型规模基准（残差缩放用：max(|rhs|, |bound|, 1)）。
///
/// **|v| ≥ 1e30 视为无穷排除**：builder 以 ±f64::MAX/4 之类的巨大有限
/// 值标记"无约束侧"（HiGHS kHighsInf=1e30 同惯例）——收进 scale 会把
/// 残差阈值放大到 ~1e300，gate 永不可拒。
pub fn model_scale(problem: &eneros_solver_core::problem::LpProblem) -> f64 {
    const INF_SENTINEL: f64 = 1e30;
    let mut scale = 1.0f64;
    for v in problem.rhs_upper.iter().chain(problem.rhs_lower.iter()) {
        if v.is_finite() && v.abs() < INF_SENTINEL {
            scale = scale.max(v.abs());
        }
    }
    for v in problem
        .upper_bounds
        .iter()
        .chain(problem.lower_bounds.iter())
    {
        if v.is_finite() && v.abs() < INF_SENTINEL {
            scale = scale.max(v.abs());
        }
    }
    scale
}

#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests {
    use std::string::String;

    use eneros_solver_core::problem::{ConstraintMatrix, LpProblem, ObjectiveSense, VarType};

    use super::*;

    #[test]
    fn primal_residual_detects_violation() {
        // x + y ≤ 4, x,y ≥ 0；x = y = 3 → 行违约 2。
        let problem = LpProblem {
            variables: vec![String::from("x"), String::from("y")],
            lower_bounds: vec![0.0, 0.0],
            upper_bounds: vec![10.0, 10.0],
            var_types: vec![VarType::Continuous, VarType::Continuous],
            objective: vec![1.0, 1.0],
            sense: ObjectiveSense::Minimize,
            constraints: ConstraintMatrix::new(1, 2, vec![0, 2], vec![0, 1], vec![1.0, 1.0]),
            rhs_lower: vec![f64::MIN / 4.0],
            rhs_upper: vec![4.0],
        };
        assert!((primal_residual(&problem, &[1.0, 2.0]) - 0.0).abs() < 1e-12);
        assert!((primal_residual(&problem, &[3.0, 3.0]) - 2.0).abs() < 1e-12);
        // 列违约：x = −1 → 1。
        assert!((primal_residual(&problem, &[-1.0, 0.0]) - 1.0).abs() < 1e-12);
        // 哨兵 rhs 不进 scale（否则阈值 ~1e300，gate 永不可拒）。
        assert!((model_scale(&problem) - 10.0).abs() < 1e-12);
    }

    /// 残差 gate 实效性：违约解必须被拒（阈值不被哨兵值击穿）。
    #[test]
    fn gate_rejects_violated_solution() {
        let problem = LpProblem {
            variables: vec![String::from("x"), String::from("y")],
            lower_bounds: vec![0.0, 0.0],
            upper_bounds: vec![10.0, 10.0],
            var_types: vec![VarType::Continuous, VarType::Continuous],
            objective: vec![1.0, 1.1],
            sense: ObjectiveSense::Minimize,
            constraints: ConstraintMatrix::new(1, 2, vec![0, 2], vec![0, 1], vec![1.0, 1.0]),
            rhs_lower: vec![f64::MIN / 4.0],
            rhs_upper: vec![4.0],
        };
        let built = crate::builder::built_model_for_test(problem);
        let result = SolveResult {
            status: SolveStatus::Optimal,
            objective_value: 0.0,
            solution: vec![3.0, 3.0], // 行违约 2 ≫ 阈值
            elapsed_ms: 0,
            dual_solution: None,
            mip_gap: Some(0.0),
        };
        let gate = SolveGate::new(crate::tolerance::ToleranceProfile::l3());
        let r = gate.evaluate(&result, &built, 0, 1_000);
        assert!(
            matches!(r, Err(PlanError::ResidualExceeded { primal, .. }) if primal > 1.0),
            "违约解必须被残差 gate 拒绝，got {r:?}"
        );
    }
}
