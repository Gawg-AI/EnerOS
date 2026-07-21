//! EnerOS LP 求解器集成（v0.64.0，P1-J Solver 起点）.
//!
//! 双脑架构（LLM + Solver）的 Solver 是"决策者"，负责在给定约束下求解
//! 优化问题（LP/MIP），输出最优解。本 crate 定义统一的 [`solver::Solver`]
//! trait、HiGHS C API FFI 绑定（feature-gated）与 [`mock::MockSolver`]（默认
//! 可用），为后续 v0.65.0 建模 DSL / v0.66.0 能源 LP / v0.67.0 安全校验 /
//! v0.68.0 意图解析奠定求解接口基础。
//!
//! v0.102.0 增量：`highs-ffi` feature 下 `HighsSolver` 支持 MILP（`Highs_passMip`
//! 整数性传参），纯连续问题仍走 `Highs_passLp` 路径零变化。
//!
//! v0.103.0 增量：`Solver` trait 追加默认方法 `set_warm_start`（非 BREAKING），
//! `HighsSolver` 经 `Highs_setSolution` 注入 MILP 热启动初始解。
//!
//! # 核心类型
//!
//! - [`solver::Solver`] — 求解器统一 trait（无 Send + Sync bound，D1）
//! - [`mock::MockSolver`] — 默认可用的 Mock 实现（D2/D10，纯 Rust）
//! - [`highs::HighsSolver`] — HiGHS C 库实现（feature = "highs-ffi"，D2/D5/D10）
//! - [`problem::LpProblem`] — LP 问题定义（D11）
//! - [`problem::ConstraintMatrix`] — CSR 格式约束矩阵（D11）
//! - [`problem::VarType`] / [`problem::ObjectiveSense`] — 变量类型/目标方向
//! - [`result::SolveResult`] / [`result::SolveStatus`] — 求解结果/状态
//! - [`solver::SolverStatus`] — 求解器运行时状态
//! - [`error::SolverError`] — 错误类型（D4）
//!
//! # 偏差声明（D1~D12，Karpathy "Think Before Coding"）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | no_std 合规：`alloc::string::String` / `alloc::vec::Vec` 替代 `std::*`；`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`；子模块不重复 no_std 声明；`solve()` 方法签名增加 `now_ms: u64` 参数用于计算 `elapsed_ms`（替代 `Instant::now()`，参考 v0.57.0 `now_ns` 模式） |
//! | **D2** | `MockSolver` 默认可用；`HighsSolver` + `ffi` 模块通过 `#[cfg(feature = "highs-ffi")]` 门控；`Cargo.toml` 声明 `[features] highs-ffi = []`（默认关闭）。参考 v0.59.0 `MockEngine` + `LlamaCppEngine` 模式 |
//! | **D3** | 移除 `params: HashMap<String, String>` 缓存字段（HiGHS 内部已存储参数；外部缓存重复状态，过度工程化 — Karpathy Simplicity First） |
//! | **D4** | `SolverError` 保留完整 7 变体（FfiError/PassFailed/RunFailed/ParamError/ParamSetFailed/InvalidProblem/NotImplemented）；默认构建（Mock）下 FFI 错误变体不可达，标 `#[allow(dead_code)]` |
//! | **D5** | `impl Drop for HighsSolver` 调用 `Highs_destroy`（feature-gated）；默认构建无 Drop 需求（RAII 资源管理仅 feature-gated 路径需要） |
//! | **D6** | 默认构建无 `build.rs`；`build.rs` 仅在 `highs-ffi` feature 启用时才需要（本版本暂不提供，留待真实集成时补充） |
//! | **D7** | 测试策略：Rust `MockSolver` 单元测试 T1~T18；真实 HiGHS FFI 测试需 `highs-ffi` feature + 编译库，超出 v0.64.0 单元测试范围（蓝图 §4.4 非瓶颈版本） |
//! | **D8** | `name()` / `version()` 方法返回 `&'static str`（MockSolver="MockSolver"/"0.1.0"，HighsSolver="HiGHS"/"1.7.2"）；避免 alloc |
//! | **D9** | crate 位置 `crates/ai/solver-core/`（AI 子系统；项目规则 §2.3.1）；不依赖 v0.59.0~v0.63.0 任何 crate（Solver 是独立基础层） |
//! | **D10** | 所有 `unsafe` FFI 代码门控在 `highs-ffi` feature 下；默认构建（MockSolver）零 `unsafe`、零外部依赖，可 `cargo test` 无需任何 C 库 |
//! | **D11** | `ConstraintMatrix` 独立结构体（`num_rows`/`num_nz`/`row_start: Vec<i32>`/`col_index: Vec<i32>`/`values: Vec<f64>` CSR 格式）；v0.65.0 DSL 编译目标 |
//! | **D12** | `SolveStatus` 派生 `PartialEq`（`alloc::string::String` 实现 `PartialEq`，`Error(String)` 变体可正常派生） |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，无外部依赖，可交叉编译到 `aarch64-unknown-none`。
//! 默认 feature 下不引入任何 `std::*`，不调用 `panic!` / `todo!` / `unimplemented!`。

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod error;
pub mod mock;
pub mod problem;
pub mod result;
pub mod solver;

#[cfg(feature = "highs-ffi")]
pub mod ffi;

#[cfg(feature = "highs-ffi")]
pub mod highs;

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec;

    use super::*;
    use crate::error::SolverError;
    use crate::mock::MockSolver;
    use crate::problem::{ConstraintMatrix, LpProblem, ObjectiveSense, VarType};
    use crate::result::{SolveResult, SolveStatus};
    use crate::solver::{Solver, SolverStatus};

    fn sample_problem() -> LpProblem {
        LpProblem {
            variables: vec![String::from("x"), String::from("y")],
            lower_bounds: vec![0.0, 0.0],
            upper_bounds: vec![10.0, 10.0],
            var_types: vec![VarType::Continuous, VarType::Continuous],
            objective: vec![1.0, 2.0],
            sense: ObjectiveSense::Maximize,
            constraints: ConstraintMatrix::new(1, 2, vec![0, 2], vec![0, 1], vec![1.0, 1.0]),
            rhs_lower: vec![0.0],
            rhs_upper: vec![5.0],
        }
    }

    #[test]
    fn t1_lp_problem_construction() {
        let p = sample_problem();
        assert_eq!(p.variables.len(), 2);
        assert_eq!(p.variables[0], "x");
        assert_eq!(p.lower_bounds[0], 0.0);
        assert_eq!(p.upper_bounds[1], 10.0);
        assert_eq!(p.objective[1], 2.0);
        assert_eq!(p.sense, ObjectiveSense::Maximize);
        assert_eq!(p.rhs_upper[0], 5.0);
    }

    #[test]
    fn t2_var_type_variants() {
        assert_ne!(VarType::Continuous, VarType::Integer);
        assert_ne!(VarType::Integer, VarType::Binary);
        assert_eq!(VarType::Continuous, VarType::Continuous);
    }

    #[test]
    fn t3_objective_sense_variants() {
        assert_ne!(ObjectiveSense::Minimize, ObjectiveSense::Maximize);
        assert_eq!(ObjectiveSense::Minimize, ObjectiveSense::Minimize);
    }

    #[test]
    fn t4_constraint_matrix_csr() {
        let m = ConstraintMatrix::new(
            2,
            4,
            vec![0, 2, 4],
            vec![0, 1, 0, 1],
            vec![1.0, 1.0, 2.0, 3.0],
        );
        assert_eq!(m.num_rows, 2);
        assert_eq!(m.num_nz, 4);
        assert_eq!(m.row_start.len(), 3); // num_rows + 1
        assert_eq!(m.col_index.len(), 4);
        assert_eq!(m.values.len(), 4);
    }

    #[test]
    fn t5_solve_status_optimal_infeasible() {
        assert_ne!(SolveStatus::Optimal, SolveStatus::Infeasible);
        assert_eq!(SolveStatus::Optimal, SolveStatus::Optimal);
    }

    #[test]
    fn t6_solve_status_error_with_string() {
        let a = SolveStatus::Error(String::from("foo"));
        let b = SolveStatus::Error(String::from("bar"));
        assert_ne!(a, b); // 不同 String 内容
        assert_eq!(a, SolveStatus::Error(String::from("foo"))); // 相同 String 内容
    }

    #[test]
    fn t7_solve_result_optimal_helper() {
        let r = SolveResult::optimal(42.0, vec![1.0, 2.0]);
        assert_eq!(r.status, SolveStatus::Optimal);
        assert_eq!(r.objective_value, 42.0);
        assert_eq!(r.solution, vec![1.0, 2.0]);
        assert_eq!(r.elapsed_ms, 0);
        assert!(r.dual_solution.is_none());
    }

    #[test]
    fn t8_solver_status_idle_solving() {
        assert_ne!(SolverStatus::Idle, SolverStatus::Solving);
        assert_eq!(SolverStatus::Idle, SolverStatus::Idle);
        assert_eq!(SolverStatus::Error, SolverStatus::Error);
    }

    #[test]
    fn t9_solver_error_pass_failed() {
        let e = SolverError::PassFailed(-1);
        let s = alloc::format!("{}", e);
        assert!(s.contains("pass failed"));
        assert!(s.contains("-1"));
    }

    #[test]
    fn t10_solver_error_invalid_problem_display() {
        let e = SolverError::InvalidProblem(String::from("var count mismatch"));
        let s = alloc::format!("{}", e);
        assert!(s.contains("invalid problem"));
        assert!(s.contains("var count mismatch"));
    }

    #[test]
    fn t11_mock_solver_name_version() {
        let m = MockSolver::new();
        assert_eq!(m.name(), "MockSolver");
        assert_eq!(m.version(), "0.1.0");
    }

    #[test]
    fn t12_mock_solver_status_idle() {
        let m = MockSolver::new();
        assert_eq!(m.status(), SolverStatus::Idle);
    }

    #[test]
    fn t13_mock_solver_set_param_ok() {
        let mut m = MockSolver::new();
        assert!(m.set_param("key", "val").is_ok());
    }

    #[test]
    fn t14_mock_solver_solve_optimal() {
        let mut m = MockSolver::new();
        let p = sample_problem();
        let result = m.solve(&p, 1000).unwrap();
        assert_eq!(result.status, SolveStatus::Optimal);
        assert_eq!(result.objective_value, 0.0);
        assert!(result.solution.is_empty());
    }

    #[test]
    fn t15_mock_solver_with_custom_result() {
        let custom = SolveResult {
            status: SolveStatus::Suboptimal,
            objective_value: 99.0,
            solution: vec![3.0, 4.0],
            elapsed_ms: 0,
            dual_solution: Some(vec![0.5, 0.6]),
        };
        let mut m = MockSolver::with_result(custom.clone());
        let p = sample_problem();
        let result = m.solve(&p, 2000).unwrap();
        assert_eq!(result.status, SolveStatus::Suboptimal);
        assert_eq!(result.objective_value, 99.0);
        assert_eq!(result.solution, vec![3.0, 4.0]);
        assert!(result.dual_solution.is_some());
    }

    #[test]
    fn t16_dyn_solver_trait_object() {
        let mut m = MockSolver::new();
        let s: &mut dyn Solver = &mut m;
        assert_eq!(s.name(), "MockSolver");
        let p = sample_problem();
        let result = s.solve(&p, 3000).unwrap();
        assert_eq!(result.status, SolveStatus::Optimal);
    }

    #[test]
    fn t17_mock_solver_multiple_calls_consistent() {
        let mut m = MockSolver::new();
        let p = sample_problem();
        let r1 = m.solve(&p, 100).unwrap();
        let r2 = m.solve(&p, 200).unwrap();
        assert_eq!(r1.status, r2.status);
        assert_eq!(r1.objective_value, r2.objective_value);
    }

    #[test]
    fn t18_lp_problem_full_construction_with_constraints() {
        let m = ConstraintMatrix::new(
            3,
            5,
            vec![0, 2, 3, 5],
            vec![0, 1, 0, 0, 1],
            vec![1.0, 1.0, 2.0, 3.0, 4.0],
        );
        let p = LpProblem {
            variables: vec![String::from("x"), String::from("y")],
            lower_bounds: vec![0.0, 0.0],
            upper_bounds: vec![10.0, 10.0],
            var_types: vec![VarType::Continuous, VarType::Integer],
            objective: vec![1.0, 2.0],
            sense: ObjectiveSense::Minimize,
            constraints: m,
            rhs_lower: vec![0.0, 0.0, 0.0],
            rhs_upper: vec![5.0, 10.0, 15.0],
        };
        assert_eq!(p.variables.len(), 2);
        assert_eq!(p.constraints.num_rows, 3);
        assert_eq!(p.constraints.num_nz, 5);
        assert_eq!(p.constraints.row_start.len(), 4);
        assert_eq!(p.var_types[1], VarType::Integer);
        assert_eq!(p.rhs_upper[2], 15.0);
    }

    #[test]
    fn t19_mock_solver_records_warm_start() {
        let mut m = MockSolver::new();
        assert!(m.warm_start.is_none());
        m.set_warm_start(&[1.0, 0.0, 2.0]).unwrap();
        assert_eq!(m.warm_start, Some(vec![1.0, 0.0, 2.0]));
        // 重复调用覆盖末次
        m.set_warm_start(&[9.0]).unwrap();
        assert_eq!(m.warm_start, Some(vec![9.0]));
    }

    #[test]
    fn t20_default_set_warm_start_noop() {
        // 自定义 stub 不覆写 set_warm_start → 默认 no-op（非 BREAKING 验证）.
        struct Stub;
        impl Solver for Stub {
            fn solve(&mut self, _p: &LpProblem, _now_ms: u64) -> Result<SolveResult, SolverError> {
                Ok(SolveResult::optimal(0.0, vec![]))
            }
            fn name(&self) -> &'static str {
                "Stub"
            }
            fn version(&self) -> &'static str {
                "0"
            }
            fn set_param(&mut self, _k: &str, _v: &str) -> Result<(), SolverError> {
                Ok(())
            }
            fn status(&self) -> SolverStatus {
                SolverStatus::Idle
            }
        }
        let mut s = Stub;
        assert!(s.set_warm_start(&[1.0, 2.0]).is_ok());
    }
}
