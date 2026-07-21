//! 优化问题容器与编译器（D2/D3/D9）.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_solver_core::error::SolverError;
use eneros_solver_core::problem::{ConstraintMatrix, LpProblem, ObjectiveSense};

use crate::constraint::Constraint;
use crate::expr::LinearExpr;
use crate::variable::Variable;

/// 优化问题容器 + Builder + 编译器.
pub struct OptProblem {
    /// 变量列表.
    pub variables: Vec<Variable>,
    /// 变量名→索引映射（D2：BTreeMap）.
    var_map: BTreeMap<String, usize>,
    /// 目标函数.
    pub objective: Option<LinearExpr>,
    /// 目标方向（复用 v0.64.0 ObjectiveSense，D3）.
    sense: ObjectiveSense,
    /// 约束列表.
    pub constraints: Vec<Constraint>,
    /// 约束名称列表.
    constraint_names: Vec<String>,
}

impl OptProblem {
    /// 创建空问题.
    pub fn new() -> Self {
        Self {
            variables: Vec::new(),
            var_map: BTreeMap::new(),
            objective: None,
            sense: ObjectiveSense::Minimize,
            constraints: Vec::new(),
            constraint_names: Vec::new(),
        }
    }

    /// 添加变量并返回索引（分配 index 字段）.
    pub fn add_var(&mut self, mut var: Variable) -> usize {
        let idx = self.variables.len();
        var.index = Some(idx);
        self.var_map.insert(var.name.clone(), idx);
        self.variables.push(var);
        idx
    }

    /// 按名称获取变量.
    pub fn var(&self, name: &str) -> Option<&Variable> {
        self.var_map.get(name).map(|&idx| &self.variables[idx])
    }

    /// 设置目标函数（最小化，链式）.
    pub fn minimize(mut self, expr: LinearExpr) -> Self {
        self.objective = Some(expr);
        self.sense = ObjectiveSense::Minimize;
        self
    }

    /// 设置目标函数（最大化，链式）.
    pub fn maximize(mut self, expr: LinearExpr) -> Self {
        self.objective = Some(expr);
        self.sense = ObjectiveSense::Maximize;
        self
    }

    /// 添加约束.
    pub fn add_constraint(&mut self, name: &str, constraint: Constraint) -> &mut Self {
        self.constraints.push(constraint);
        self.constraint_names.push(String::from(name));
        self
    }

    /// 编译为 `LpProblem` 矩阵格式（D9：复用 v0.64.0 LpProblem/SolverError）.
    pub fn compile(&self) -> Result<LpProblem, SolverError> {
        let num_var = self.variables.len();
        let num_con = self.constraints.len();

        // 变量边界和类型
        let lower_bounds: Vec<f64> = self.variables.iter().map(|v| v.lower_bound).collect();
        let upper_bounds: Vec<f64> = self.variables.iter().map(|v| v.upper_bound).collect();
        let var_types: Vec<_> = self.variables.iter().map(|v| v.var_type).collect();

        // 目标函数系数
        let objective = match &self.objective {
            Some(expr) => {
                let mut obj = alloc::vec![0.0f64; num_var];
                for (&idx, &coeff) in &expr.terms {
                    if idx < num_var {
                        obj[idx] = coeff;
                    }
                }
                obj
            }
            None => alloc::vec![0.0f64; num_var],
        };

        // 约束矩阵（CSR 格式）
        let mut row_start: Vec<i32> = Vec::with_capacity(num_con + 1);
        let mut col_index: Vec<i32> = Vec::new();
        let mut values: Vec<f64> = Vec::new();
        let mut rhs_lower: Vec<f64> = Vec::with_capacity(num_con);
        let mut rhs_upper: Vec<f64> = Vec::with_capacity(num_con);

        row_start.push(0);
        for con in &self.constraints {
            let (expr, lo, hi) = match con {
                Constraint::Le(e, rhs) => (e, f64::NEG_INFINITY, *rhs),
                Constraint::Ge(e, rhs) => (e, *rhs, f64::INFINITY),
                Constraint::Eq(e, rhs) => (e, *rhs, *rhs),
                Constraint::Range(e, lo, hi) => (e, *lo, *hi),
            };
            // BTreeMap 遍历顺序确定（D2 优势）
            for (&idx, &coeff) in &expr.terms {
                if coeff.abs() >= 1e-12 {
                    col_index.push(idx as i32);
                    values.push(coeff);
                }
            }
            rhs_lower.push(lo);
            rhs_upper.push(hi);
            row_start.push(col_index.len() as i32);
        }

        Ok(LpProblem {
            variables: self.variables.iter().map(|v| v.name.clone()).collect(),
            lower_bounds,
            upper_bounds,
            var_types,
            objective,
            sense: self.sense,
            constraints: ConstraintMatrix::new(num_con, values.len(), row_start, col_index, values),
            rhs_lower,
            rhs_upper,
        })
    }
}

impl Default for OptProblem {
    fn default() -> Self {
        Self::new()
    }
}
