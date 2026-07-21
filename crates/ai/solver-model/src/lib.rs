//! EnerOS 优化问题建模框架（v0.65.0，P1-J Solver 第二层）.
//!
//! 构建 LP 问题的 Rust DSL：变量（`VarBuilder` 链式构建）、线性表达式
//! （`LinearExpr` + 运算符重载）、约束（`Constraint` 枚举）、优化问题容器
//! （`OptProblem` + Builder + `compile()` 编译器）。`compile()` 将 DSL 转换
//! 为 v0.64.0 的 `LpProblem` 矩阵格式，传给 `Solver` 求解。
//!
//! # 核心类型
//!
//! - [`variable::Variable`] / [`variable::VarBuilder`] — 决策变量 + 链式构建器
//! - [`expr::LinearExpr`] — 线性表达式（`Add`/`Sub`/`Mul<f64>` 运算符重载）
//! - [`constraint::Constraint`] — 约束枚举（Le/Ge/Eq/Range）
//! - [`problem::OptProblem`] — 优化问题容器 + Builder + `compile()` 编译器
//!
//! # 偏差声明（D1~D12，Karpathy "Think Before Coding"）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | 运算符重载使用 `core::ops::{Add, Sub, Mul}`（no_std 兼容），非 `std::ops` |
//! | **D2** | `LinearExpr.terms` + `OptProblem.var_map` 使用 `alloc::collections::BTreeMap`（no_std 可用 + 确定性遍历），非 `HashMap`（需 `std` 或第三方 `hashbrown`，引入依赖过度 — Simplicity First） |
//! | **D3** | 复用 v0.64.0 `eneros_solver_core::problem::{VarType, ObjectiveSense, LpProblem, ConstraintMatrix}` + `eneros_solver_core::error::SolverError`，不重定义 |
//! | **D4** | `Variable.index: Option<usize>`（编译前 None，编译后 Some(idx)）与蓝图一致 |
//! | **D5** | `f64::INFINITY` / `f64::NEG_INFINITY` 使用 `core::f64` 常量（no_std 可用） |
//! | **D6** | `VarBuilder` 链式方法返回 `Self`（Builder 模式核心表达力） |
//! | **D7** | 端到端测试用 `MockSolver`（v0.64.0）而非真实 HiGHS（需 `highs-ffi` feature + 编译库，超出 v0.65.0 范围） |
//! | **D8** | crate 位置 `crates/ai/solver-model/`（AI 子系统；项目规则 §2.3.1）；依赖 `eneros-solver-core`（path = "../solver-core"） |
//! | **D9** | `compile()` 返回 `Result<LpProblem, SolverError>`，复用 v0.64.0 `SolverError::InvalidProblem(String)` 覆盖编译错误 |
//! | **D10** | 纯 safe Rust，零 `unsafe`、零外部 C 库 |
//! | **D11** | 无 `[features]` 段，无 FFI 需求 |
//! | **D12** | `LinearExpr` 派生 `Default`（`BTreeMap` 和 `f64` 都实现 `Default`） |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，无外部依赖（除 `eneros-solver-core`），
//! 可交叉编译到 `aarch64-unknown-none`。默认不引入任何 `std::*`，
//! 不调用 `panic!` / `todo!` / `unimplemented!`。

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod constraint;
pub mod expr;
pub mod problem;
pub mod variable;

// 重导出常用类型，方便外部使用
pub use constraint::Constraint;
pub use expr::LinearExpr;
pub use problem::OptProblem;
pub use variable::{VarBuilder, Variable};

#[cfg(test)]
mod tests {
    use eneros_solver_core::mock::MockSolver;
    use eneros_solver_core::problem::VarType;
    use eneros_solver_core::result::SolveStatus;
    use eneros_solver_core::solver::Solver;

    use crate::constraint::Constraint;
    use crate::expr::LinearExpr;
    use crate::problem::OptProblem;
    use crate::variable::{VarBuilder, Variable};

    fn make_var_with_idx(name: &str, idx: usize) -> Variable {
        let mut v = VarBuilder::new(name).build();
        v.index = Some(idx);
        v
    }

    #[test]
    fn t1_var_builder_default() {
        let v = VarBuilder::new("x").build();
        assert_eq!(v.name, "x");
        assert_eq!(v.lower_bound, 0.0);
        assert!(v.upper_bound.is_infinite() && v.upper_bound > 0.0);
        assert_eq!(v.var_type, VarType::Continuous);
        assert!(v.index.is_none());
    }

    #[test]
    fn t2_var_builder_chain_methods() {
        let v = VarBuilder::new("p").range(0.0, 10.0).integer().build();
        assert_eq!(v.lower_bound, 0.0);
        assert_eq!(v.upper_bound, 10.0);
        assert_eq!(v.var_type, VarType::Integer);

        let b = VarBuilder::new("b").binary().build();
        assert_eq!(b.var_type, VarType::Binary);
        assert_eq!(b.lower_bound, 0.0);
        assert_eq!(b.upper_bound, 1.0);
    }

    #[test]
    fn t3_variable_field_access_and_clone() {
        let v = VarBuilder::new("x").range(-1.0, 1.0).build();
        let cloned = v.clone();
        assert_eq!(v.name, cloned.name);
        assert_eq!(v.lower_bound, cloned.lower_bound);
        assert_eq!(v.upper_bound, cloned.upper_bound);
    }

    #[test]
    fn t4_linear_expr_new_and_default() {
        let e1 = LinearExpr::new();
        let e2 = LinearExpr::default();
        assert!(e1.terms.is_empty());
        assert!(e2.terms.is_empty());
        assert_eq!(e1.constant, 0.0);
    }

    #[test]
    fn t5_linear_expr_from_var() {
        let v = make_var_with_idx("x", 0);
        let e = LinearExpr::from_var(&v);
        assert_eq!(e.terms.len(), 1);
        assert_eq!(e.terms[&0], 1.0);
    }

    #[test]
    fn t6_linear_expr_add_term_accumulate_and_remove_zero() {
        let mut e = LinearExpr::new();
        e.add_term(0, 1.0);
        e.add_term(0, 2.0); // 累加 → 3.0
        assert_eq!(e.terms[&0], 3.0);
        e.add_term(0, -3.0); // 归零 → 自动移除
        assert!(!e.terms.contains_key(&0));
    }

    #[test]
    fn t7_linear_expr_scale() {
        let mut e = LinearExpr::new();
        e.add_term(0, 1.0);
        e.add_term(1, 2.0);
        e.constant = 3.0;
        let scaled = e.scale(2.0);
        assert_eq!(scaled.terms[&0], 2.0);
        assert_eq!(scaled.terms[&1], 4.0);
        assert_eq!(scaled.constant, 6.0);
    }

    #[test]
    fn t8_linear_expr_add() {
        let mut a = LinearExpr::new();
        a.add_term(0, 1.0);
        let mut b = LinearExpr::new();
        b.add_term(0, 2.0);
        b.add_term(1, 3.0);
        let c = a.add(&b);
        assert_eq!(c.terms[&0], 3.0);
        assert_eq!(c.terms[&1], 3.0);
    }

    #[test]
    fn t9_linear_expr_sub() {
        let mut a = LinearExpr::new();
        a.add_term(0, 5.0);
        let mut b = LinearExpr::new();
        b.add_term(0, 2.0);
        b.add_term(1, 1.0);
        let c = a.sub(&b);
        assert_eq!(c.terms[&0], 3.0);
        assert_eq!(c.terms[&1], -1.0);
    }

    #[test]
    fn t10_operator_add() {
        let mut a = LinearExpr::new();
        a.add_term(0, 1.0);
        let mut b = LinearExpr::new();
        b.add_term(0, 2.0);
        let c = a + b;
        assert_eq!(c.terms[&0], 3.0);
    }

    #[test]
    fn t11_operator_sub() {
        let mut a = LinearExpr::new();
        a.add_term(0, 5.0);
        let mut b = LinearExpr::new();
        b.add_term(0, 2.0);
        let c = a - b;
        assert_eq!(c.terms[&0], 3.0);
    }

    #[test]
    fn t12_operator_mul_f64() {
        let mut a = LinearExpr::new();
        a.add_term(0, 1.0);
        a.add_term(1, 2.0);
        let c = a * 2.0;
        assert_eq!(c.terms[&0], 2.0);
        assert_eq!(c.terms[&1], 4.0);
    }

    #[test]
    fn t13_operator_combination() {
        let mut a = LinearExpr::new();
        a.add_term(0, 1.0);
        let mut b = LinearExpr::new();
        b.add_term(0, 1.0);
        b.add_term(1, 1.0);
        let mut d = LinearExpr::new();
        d.add_term(1, 1.0);
        // a + b*2 - d = 1*x0 + (1*x0 + 1*x1)*2 - 1*x1 = 3*x0 + 1*x1
        let r = a + b * 2.0 - d;
        assert_eq!(r.terms[&0], 3.0);
        assert_eq!(r.terms[&1], 1.0);
    }

    #[test]
    fn t14_constraint_variants() {
        let mut e = LinearExpr::new();
        e.add_term(0, 1.0);
        let _le = Constraint::Le(e.clone(), 10.0);
        let _ge = Constraint::Ge(e.clone(), 0.0);
        let _eq = Constraint::Eq(e.clone(), 5.0);
        let _range = Constraint::Range(e, 0.0, 10.0);
        // 编译时确认枚举变体可构造
    }

    #[test]
    fn t15_opt_problem_new_empty() {
        let p = OptProblem::new();
        assert!(p.variables.is_empty());
        assert!(p.var("x").is_none());
    }

    #[test]
    fn t16_opt_problem_add_var_and_lookup() {
        let mut p = OptProblem::new();
        let v = VarBuilder::new("x").build();
        let idx = p.add_var(v);
        assert_eq!(idx, 0);
        assert!(p.var("x").is_some());
        assert_eq!(p.var("x").unwrap().index, Some(0));
        assert!(p.var("y").is_none());
    }

    #[test]
    fn t17_opt_problem_minimize_maximize_chain() {
        let mut e = LinearExpr::new();
        e.add_term(0, 1.0);
        let p = OptProblem::new().maximize(e);
        // 仅验证链式调用编译通过
        assert!(p.objective.is_some());
    }

    #[test]
    fn t18_opt_problem_add_constraint() {
        let mut p = OptProblem::new();
        let mut e = LinearExpr::new();
        e.add_term(0, 1.0);
        p.add_constraint("c1", Constraint::Le(e, 10.0));
        assert_eq!(p.constraints.len(), 1);
    }

    #[test]
    fn t19_opt_problem_compile_with_vars_and_constraint() {
        let mut p = OptProblem::new();
        p.add_var(VarBuilder::new("x").range(0.0, 10.0).build());
        p.add_var(VarBuilder::new("y").range(0.0, 10.0).build());
        let mut obj = LinearExpr::new();
        obj.add_term(0, 1.0);
        obj.add_term(1, 2.0);
        p = p.maximize(obj);
        let mut c = LinearExpr::new();
        c.add_term(0, 1.0);
        c.add_term(1, 1.0);
        p.add_constraint("cap", Constraint::Le(c, 5.0));

        let lp = p.compile().unwrap();
        assert_eq!(lp.variables.len(), 2);
        assert_eq!(lp.constraints.num_rows, 1);
        assert_eq!(lp.constraints.num_nz, 2);
        assert_eq!(lp.rhs_upper[0], 5.0);
        assert!(lp.rhs_lower[0].is_infinite() && lp.rhs_lower[0] < 0.0);
    }

    #[test]
    fn t20_opt_problem_compile_empty() {
        let p = OptProblem::new();
        let lp = p.compile().unwrap();
        assert!(lp.variables.is_empty());
        assert_eq!(lp.constraints.num_rows, 0);
        assert_eq!(lp.constraints.num_nz, 0);
    }

    #[test]
    fn t21_opt_problem_compile_csr_format() {
        let mut p = OptProblem::new();
        p.add_var(VarBuilder::new("x").build());
        p.add_var(VarBuilder::new("y").build());
        let mut c1 = LinearExpr::new();
        c1.add_term(0, 1.0);
        c1.add_term(1, 1.0);
        p.add_constraint("c1", Constraint::Le(c1, 5.0));
        let mut c2 = LinearExpr::new();
        c2.add_term(0, 2.0);
        c2.add_term(1, 3.0);
        p.add_constraint("c2", Constraint::Ge(c2, 1.0));

        let lp = p.compile().unwrap();
        // CSR: row_start.len() == num_rows + 1
        assert_eq!(lp.constraints.row_start.len(), 3); // 2 行 + 1
        assert_eq!(lp.constraints.col_index.len(), 4); // 4 非零元素
        assert_eq!(lp.constraints.values.len(), 4);
    }

    #[test]
    fn t22_end_to_end_with_mock_solver() {
        let mut p = OptProblem::new();
        p.add_var(VarBuilder::new("x").range(0.0, 10.0).build());
        p.add_var(VarBuilder::new("y").range(0.0, 10.0).build());
        let mut obj = LinearExpr::new();
        obj.add_term(0, 1.0);
        obj.add_term(1, 2.0);
        p = p.maximize(obj);
        let mut c = LinearExpr::new();
        c.add_term(0, 1.0);
        c.add_term(1, 1.0);
        p.add_constraint("cap", Constraint::Le(c, 5.0));

        let lp = p.compile().unwrap();
        let mut solver = MockSolver::new();
        let result = solver.solve(&lp, 0).unwrap();
        assert_eq!(result.status, SolveStatus::Optimal);
    }
}
