//! 热启动候选解与可行性投影（蓝图 §4.1/§4.4，D9）.

use alloc::vec::Vec;

use eneros_solver_core::problem::{LpProblem, VarType};

/// 热启动候选解（蓝图 §4.1）.
#[derive(Debug, Clone)]
pub struct CandidateSolution {
    /// 连续变量初始值（按 problem 中连续列顺序）.
    pub continuous: Vec<f64>,
    /// 整数变量初始值（按 problem 中整数/0-1 列顺序，0/1）.
    pub integer: Vec<i32>,
    /// 置信度（蓝图公式：整数列累积 `1−|v−0.5|·2` 后除以 num_vars）.
    pub confidence: f64,
}

impl CandidateSolution {
    /// 构造.
    pub fn new(continuous: Vec<f64>, integer: Vec<i32>, confidence: f64) -> Self {
        Self {
            continuous,
            integer,
            confidence,
        }
    }

    /// 可行性投影（蓝图 §4.4，D9）：连续列 clamp 到 `[lower_bounds, upper_bounds]`；
    /// 整数列与 confidence 不动（C27）。按 problem 列序遍历 var_types，
    /// 连续列依序消费 continuous。
    pub fn project(&mut self, problem: &LpProblem) {
        let mut ci = 0usize;
        for (col, var_type) in problem.var_types.iter().enumerate() {
            if *var_type != VarType::Continuous {
                continue;
            }
            if ci >= self.continuous.len() {
                break;
            }
            let v = self.continuous[ci];
            self.continuous[ci] = v
                .max(problem.lower_bounds[col])
                .min(problem.upper_bounds[col]);
            ci += 1;
        }
    }

    /// 按 `var_types` 逐列合并为完整解向量（len == variables.len()）：
    /// Continuous 取 continuous 序、Binary/Integer 取 integer 序（i32 → f64）。
    /// 候选不足处以 0.0 补齐（空候选安全，C33）。
    pub fn to_solution(&self, problem: &LpProblem) -> Vec<f64> {
        let mut solution = Vec::with_capacity(problem.variables.len());
        let mut ci = 0usize;
        let mut ii = 0usize;
        for var_type in &problem.var_types {
            match var_type {
                VarType::Continuous => {
                    solution.push(self.continuous.get(ci).copied().unwrap_or(0.0));
                    ci += 1;
                }
                VarType::Binary | VarType::Integer => {
                    solution.push(self.integer.get(ii).copied().unwrap_or(0) as f64);
                    ii += 1;
                }
            }
        }
        solution
    }
}

#[cfg(test)]
mod tests {
    use eneros_solver_core::problem::{ConstraintMatrix, ObjectiveSense};

    use super::*;

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

    /// TC1：构造后三字段与入参一致.
    #[test]
    fn tc1_new_fields() {
        let c = CandidateSolution::new(vec![1.0, 2.0], vec![1, 0], 0.9);
        assert_eq!(c.continuous, vec![1.0, 2.0]);
        assert_eq!(c.integer, vec![1, 0]);
        assert_eq!(c.confidence, 0.9);
    }

    /// TC2：投影 clamp 上界（12.0 → 10.0）与下界（3.0 → 5.0）.
    #[test]
    fn tc2_project_clamp_bounds() {
        let p = problem(
            &[VarType::Continuous, VarType::Continuous],
            &[0.0, 5.0],
            &[10.0, 20.0],
        );
        let mut c = CandidateSolution::new(vec![12.0, 3.0], vec![], 0.5);
        c.project(&p);
        assert_eq!(c.continuous, vec![10.0, 5.0]);
    }

    /// TC3：投影 clamp 连续列，但整数列与 confidence 不动（C27）.
    #[test]
    fn tc3_project_keeps_integer_and_confidence() {
        let p = problem(
            &[VarType::Continuous, VarType::Binary],
            &[0.0, 0.0],
            &[1.0, 1.0],
        );
        let mut c = CandidateSolution::new(vec![-1.0], vec![1], 0.77);
        c.project(&p);
        assert_eq!(c.continuous, vec![0.0]);
        assert_eq!(c.integer, vec![1]);
        assert_eq!(c.confidence, 0.77);
    }

    /// TC4：混排 4 列 [C,B,C,I] 按列序合并为完整解向量.
    #[test]
    fn tc4_to_solution_mixed_order() {
        let p = problem(
            &[
                VarType::Continuous,
                VarType::Binary,
                VarType::Continuous,
                VarType::Integer,
            ],
            &[0.0; 4],
            &[10.0; 4],
        );
        let c = CandidateSolution::new(vec![1.0, 2.0], vec![1, 0], 0.5);
        assert_eq!(c.to_solution(&p), vec![1.0, 1.0, 2.0, 0.0]);
    }

    /// TC5：任意混排 5 列，to_solution 长度 == num_vars.
    #[test]
    fn tc5_to_solution_len_equals_num_vars() {
        let p = problem(
            &[
                VarType::Binary,
                VarType::Continuous,
                VarType::Integer,
                VarType::Continuous,
                VarType::Binary,
            ],
            &[0.0; 5],
            &[10.0; 5],
        );
        let c = CandidateSolution::new(vec![1.0, 2.0], vec![1, 0, 1], 0.5);
        assert_eq!(c.to_solution(&p).len(), p.variables.len());
    }

    /// TC6：全连续问题，to_solution == continuous 克隆.
    #[test]
    fn tc6_to_solution_all_continuous() {
        let p = problem(&[VarType::Continuous; 3], &[0.0; 3], &[10.0; 3]);
        let c = CandidateSolution::new(vec![1.5, 2.5, 3.5], vec![], 1.0);
        assert_eq!(c.to_solution(&p), c.continuous.clone());
    }

    /// TC7：全整数问题（0-1 列），to_solution 由 integer 逐列转 f64.
    #[test]
    fn tc7_to_solution_all_integer() {
        let p = problem(&[VarType::Binary; 2], &[0.0; 2], &[1.0; 2]);
        let c = CandidateSolution::new(vec![], vec![1, 0], 0.6);
        assert_eq!(c.to_solution(&p), vec![1.0, 0.0]);
    }

    /// TC8：空候选安全（C33）：不 panic，缺失列补 0.0；project 空候选不 panic.
    #[test]
    fn tc8_empty_candidate_safe() {
        let p = problem(&[VarType::Continuous; 2], &[0.0; 2], &[10.0; 2]);
        let mut c = CandidateSolution::new(vec![], vec![], 0.0);
        assert_eq!(c.to_solution(&p), vec![0.0, 0.0]);
        c.project(&p);
        assert!(c.continuous.is_empty());
    }
}
