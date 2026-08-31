//! HiGHS 真实求解器后端（feature = "highs-ffi"，AI-004/005）.
//!
//! 通过 FFI 调用静态链接的 HiGHS C 库执行真实 LP/MILP 求解。
//! 这是 MASTER §G7 要求的"可复现 static backend build + 真实最小求解系统测试"。
//!
//! # 资源管理
//!
//! `HighsSolver` 持有 HiGHS 不透明指针（`*mut c_void`），通过 RAII 管理：

//! - `new()` 调用 `Highs_create()`
//! - `Drop` 调用 `Highs_destroy()`
//! - 无 `Clone`（指针唯一所有权）
//!
//! # no_std 合规
//!
//! 本模块仅使用 `alloc::*` + `core::*` + FFI。`elapsed_ms` 设为 0
//! （no_std 无时钟；调用方可在外部测量墙钟时间）。

#![cfg(feature = "highs-ffi")]

/// 参数值分类（v0.202.0 P0-20：set_param 类型分发）。
#[derive(Debug, Clone, PartialEq)]
pub enum ParamValue {
    /// 布尔参数（true/on/1/false/off/0）。
    Bool(bool),
    /// 整型参数（纯整数串）。
    Int(i32),
    /// 双精度参数（含小数点/指数）。
    Double(f64),
    /// 字符串参数（其他）。
    String(alloc::string::String),
}

/// 分类 `set_param` 的值字符串 → HiGHS option setter 类型。
///
/// - 布尔字面量 → `Bool`
/// - 纯整数串（如 "30"、"3"）→ `Int`（`Highs_setIntOptionValue`）
/// - 含小数/指数的数字（如 "0.01"、"1e-3"）→ `Double`
///   （`Highs_setDoubleOptionValue`）
/// - 其他 → `String`
pub fn classify_param_value(value: &str) -> ParamValue {
    match value {
        "true" | "on" => return ParamValue::Bool(true),
        "false" | "off" => return ParamValue::Bool(false),
        _ => {}
    }
    if let Ok(i) = value.parse::<i32>() {
        // 注意："0"/"1" 已被上面布尔分支捕获（HiGHS 布尔选项接受 0/1）。
        if value == "0" || value == "1" {
            return ParamValue::Bool(value == "1");
        }
        return ParamValue::Int(i);
    }
    if let Ok(d) = value.parse::<f64>() {
        return ParamValue::Double(d);
    }
    ParamValue::String(value.to_string())
}

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::ffi::c_void;

use crate::error::SolverError;
use crate::ffi as highs_ffi;
use crate::problem::{ObjectiveSense, VarType};
use crate::result::{SolveResult, SolveStatus};
use crate::solver::{Solver, SolverStatus};

/// HiGHS 真实求解器（静态链接 C 库）.
///
/// 通过 FFI 调用 HiGHS C API 执行 LP/MILP 求解。不派生 `Clone`（唯一指针所有权）。
pub struct HighsSolver {
    handle: *mut c_void,
    status: SolverStatus,
}

// SAFETY: HiGHS 句柄非线程安全，但 EnerOS 单线程运行。不实现 Send/Sync。
// （与 MockSolver 一致：Solver trait 不要求 Send+Sync）

/// 读取 HiGHS info 的 mip_gap（失败/非有限/负 → 0.0；未报告 = 精确语义）。
fn read_mip_gap(handle: *mut c_void) -> f64 {
    let field = b"mip_gap\0";
    let mut value = 0.0f64;
    // SAFETY: handle 非 null；field 以 NUL 结尾；value 有效指针。
    let status = unsafe { highs_ffi::Highs_getDoubleInfoValue(handle, field.as_ptr(), &mut value) };
    if status == 0 && value.is_finite() && value >= 0.0 {
        value
    } else {
        0.0
    }
}

impl HighsSolver {
    /// 创建 HiGHS 求解器实例。
    ///
    /// 调用 `Highs_create()` 获取不透明句柄。失败返回 `FfiError`。
    pub fn new() -> Result<Self, SolverError> {
        // SAFETY: Highs_create 是安全的 C 函数，无前置条件。
        let handle = unsafe { highs_ffi::Highs_create() };
        if handle.is_null() {
            return Err(SolverError::FfiError(String::from(
                "Highs_create() returned null",
            )));
        }

        // 禁用 HiGHS 输出（静默运行）。
        let silent = b"output_flag\0";
        // SAFETY: handle 非 null（已检查），silent 是有效 C 字符串。
        let _ = unsafe { highs_ffi::Highs_setBoolOptionValue(handle, silent.as_ptr(), 0) };

        Ok(Self {
            handle,
            status: SolverStatus::Idle,
        })
    }
}

impl Default for HighsSolver {
    fn default() -> Self {
        Self::new().expect("Highs_create failed")
    }
}

impl Drop for HighsSolver {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: handle 由 Highs_create 返回，未在其他地方释放（RAII 唯一所有权）。
            unsafe { highs_ffi::Highs_destroy(self.handle) };
            self.handle = core::ptr::null_mut();
        }
    }
}

impl Solver for HighsSolver {
    fn solve(
        &mut self,
        problem: &crate::problem::LpProblem,
        _now_ms: u64,
    ) -> Result<SolveResult, SolverError> {
        self.status = SolverStatus::Solving;

        let num_col = problem.variables.len() as highs_ffi::HighsInt;
        let num_row = problem.constraints.num_rows as highs_ffi::HighsInt;
        let num_nz = problem.constraints.num_nz as highs_ffi::HighsInt;

        // 基本校验
        if problem.lower_bounds.len() != problem.variables.len()
            || problem.upper_bounds.len() != problem.variables.len()
            || problem.objective.len() != problem.variables.len()
            || problem.var_types.len() != problem.variables.len()
        {
            self.status = SolverStatus::Error;
            return Err(SolverError::InvalidProblem(String::from(
                "variable array length mismatch",
            )));
        }
        if problem.rhs_lower.len() != problem.constraints.num_rows
            || problem.rhs_upper.len() != problem.constraints.num_rows
        {
            self.status = SolverStatus::Error;
            return Err(SolverError::InvalidProblem(String::from(
                "rhs array length mismatch",
            )));
        }
        if problem.constraints.row_start.len() != problem.constraints.num_rows + 1 {
            self.status = SolverStatus::Error;
            return Err(SolverError::InvalidProblem(String::from(
                "row_start length != num_rows + 1",
            )));
        }

        let a_format = highs_ffi::MATRIX_FORMAT_ROWWISE;
        let sense = match problem.sense {
            ObjectiveSense::Minimize => highs_ffi::OBJ_SENSE_MINIMIZE,
            ObjectiveSense::Maximize => highs_ffi::OBJ_SENSE_MAXIMIZE,
        };
        let offset = 0.0f64;

        // 判断是否有整数变量 → MILP vs LP
        let has_integer = problem
            .var_types
            .iter()
            .any(|vt| matches!(vt, VarType::Integer | VarType::Binary));

        let pass_result = if has_integer {
            // 构造 integrality 数组
            let integrality: Vec<highs_ffi::HighsInt> = problem
                .var_types
                .iter()
                .map(|vt| match vt {
                    VarType::Continuous => highs_ffi::VAR_TYPE_CONTINUOUS,
                    VarType::Integer | VarType::Binary => highs_ffi::VAR_TYPE_INTEGER,
                })
                .collect();

            // SAFETY: handle 非 null，所有指针指向有效 Vec 底层数组，
            // 长度与 num_col/num_row/num_nz 一致。
            unsafe {
                highs_ffi::Highs_passMip(
                    self.handle,
                    num_col,
                    num_row,
                    num_nz,
                    a_format,
                    sense,
                    offset,
                    problem.objective.as_ptr(),
                    problem.lower_bounds.as_ptr(),
                    problem.upper_bounds.as_ptr(),
                    problem.rhs_lower.as_ptr(),
                    problem.rhs_upper.as_ptr(),
                    problem.constraints.row_start.as_ptr(),
                    problem.constraints.col_index.as_ptr(),
                    problem.constraints.values.as_ptr(),
                    integrality.as_ptr(),
                )
            }
        } else {
            // SAFETY: 同上，无 integrality 参数。
            unsafe {
                highs_ffi::Highs_passLp(
                    self.handle,
                    num_col,
                    num_row,
                    num_nz,
                    a_format,
                    sense,
                    offset,
                    problem.objective.as_ptr(),
                    problem.lower_bounds.as_ptr(),
                    problem.upper_bounds.as_ptr(),
                    problem.rhs_lower.as_ptr(),
                    problem.rhs_upper.as_ptr(),
                    problem.constraints.row_start.as_ptr(),
                    problem.constraints.col_index.as_ptr(),
                    problem.constraints.values.as_ptr(),
                )
            }
        };

        if pass_result < 0 {
            self.status = SolverStatus::Error;
            return Err(SolverError::PassFailed(pass_result));
        }

        // 求解
        // SAFETY: handle 非 null，模型已成功传入。
        let run_result = unsafe { highs_ffi::Highs_run(self.handle) };
        if run_result < 0 {
            self.status = SolverStatus::Error;
            return Err(SolverError::RunFailed(run_result));
        }

        // 获取模型状态
        // SAFETY: handle 非 null。
        let model_status = unsafe { highs_ffi::Highs_getModelStatus(self.handle) };

        let status = match model_status {
            highs_ffi::MODEL_STATUS_OPTIMAL => SolveStatus::Optimal,
            highs_ffi::MODEL_STATUS_INFEASIBLE => SolveStatus::Infeasible,
            highs_ffi::MODEL_STATUS_UNBOUNDED => SolveStatus::Unbounded,
            highs_ffi::MODEL_STATUS_UNBOUNDED_OR_INFEASIBLE => SolveStatus::Unbounded,
            _ => SolveStatus::Error(alloc::format!("HiGHS model_status={}", model_status)),
        };

        // 获取目标值
        // SAFETY: handle 非 null。
        let objective_value = unsafe { highs_ffi::Highs_getObjectiveValue(self.handle) };

        // 相对 MIP gap（MILP 有效；LP 恒 0——蓝图61 [09] §3.5 gap gate）。
        let mip_gap = read_mip_gap(self.handle);

        // 获取解向量
        let mut col_value = vec![0.0f64; num_col as usize];
        let mut col_dual = vec![0.0f64; num_col as usize];
        let mut row_value = vec![0.0f64; num_row as usize];
        let mut row_dual = vec![0.0f64; num_row as usize];

        // SAFETY: handle 非 null，数组已分配正确大小。
        let _ = unsafe {
            highs_ffi::Highs_getSolution(
                self.handle,
                col_value.as_mut_ptr(),
                col_dual.as_mut_ptr(),
                row_value.as_mut_ptr(),
                row_dual.as_mut_ptr(),
            )
        };

        self.status = SolverStatus::Idle;

        Ok(SolveResult {
            status,
            objective_value,
            solution: col_value,
            elapsed_ms: 0, // no_std 无时钟；调用方可外部测量
            dual_solution: Some(col_dual),
            mip_gap,
        })
    }

    fn name(&self) -> &'static str {
        "HiGHS"
    }

    fn version(&self) -> &'static str {
        "1.7.2"
    }

    fn set_param(&mut self, key: &str, value: &str) -> Result<(), SolverError> {
        // v0.202.0（P0-20）：区分布尔/数值/字符串参数，数值走
        // Highs_setDoubleOptionValue / Highs_setIntOptionValue。
        let c_key = alloc::format!("{}\0", key);
        let c_key_ptr = c_key.as_ptr();
        let status = match classify_param_value(value) {
            ParamValue::Bool(b) => {
                // SAFETY: handle 非 null，c_key 以 NUL 结尾。
                unsafe {
                    highs_ffi::Highs_setBoolOptionValue(
                        self.handle,
                        c_key_ptr,
                        if b { 1 } else { 0 },
                    )
                }
            }
            ParamValue::Int(i) => {
                // SAFETY: handle 非 null，c_key 以 NUL 结尾。
                unsafe { highs_ffi::Highs_setIntOptionValue(self.handle, c_key_ptr, i) }
            }
            ParamValue::Double(d) => {
                // SAFETY: handle 非 null，c_key 以 NUL 结尾。
                unsafe { highs_ffi::Highs_setDoubleOptionValue(self.handle, c_key_ptr, d) }
            }
            ParamValue::String(s) => {
                let c_val = alloc::format!("{}\0", s);
                // SAFETY: handle 非 null，c_key/c_val 以 NUL 结尾。
                unsafe {
                    highs_ffi::Highs_setStringOptionValue(self.handle, c_key_ptr, c_val.as_ptr())
                }
            }
        };
        if status < 0 {
            self.status = SolverStatus::Error;
            return Err(SolverError::ParamSetFailed(String::from(key)));
        }
        Ok(())
    }

    fn status(&self) -> SolverStatus {
        self.status.clone()
    }

    fn set_warm_start(&mut self, solution: &[f64]) -> Result<(), SolverError> {
        // v0.204.0（P0-19）：真实覆写——经 Highs_setSolution 注入初始解。
        // 失败真实报错（不再静默 Ok）。
        if solution.is_empty() {
            return Err(SolverError::ParamError(
                "set_warm_start: empty solution".to_string(),
            ));
        }
        // SAFETY: handle 非 null；solution 是连续 f64 切片（C API 期望
        // 与列数等长的数组）。row_value/col_dual/row_dual 传 NULL
        // （只注入原始解，HiGHS 接受）。
        let status = unsafe {
            highs_ffi::Highs_setSolution(
                self.handle,
                solution.as_ptr(),
                core::ptr::null(),
                core::ptr::null(),
                core::ptr::null(),
            )
        };
        if status < 0 {
            self.status = SolverStatus::Error;
            return Err(SolverError::FfiError(
                "Highs_setSolution failed".to_string(),
            ));
        }
        Ok(())
    }
}

// ============================================================================
// Tests — 真实 HiGHS 求解测试（AI-005 独立交叉验证）
// ============================================================================

#[cfg(all(test, feature = "highs-ffi"))]
mod tests {
    use super::*;
    use crate::problem::{ConstraintMatrix, LpProblem, ObjectiveSense, VarType};
    use crate::result::SolveStatus;
    use crate::solver::Solver;

    /// T1：真实 LP 求解 — 最大化 3x + 2y，约束 x+y<=4, 2x+y<=5.
    ///
    /// 已知最优解：x=1, y=3, objective=9.
    #[test]
    fn t1_real_lp_solve_maximize() {
        let mut solver = HighsSolver::new().expect("Highs_create");

        // 2 变量, 2 约束
        // 最大化 3x + 2y
        // s.t.  x + y <= 4  (row 0: 0 <= x+y <= 4)
        //       2x + y <= 5 (row 1: 0 <= 2x+y <= 5)
        //       0 <= x, 0 <= y
        let problem = LpProblem {
            variables: vec![String::from("x"), String::from("y")],
            lower_bounds: vec![0.0, 0.0],
            upper_bounds: vec![1e30, 1e30], // 无上界
            var_types: vec![VarType::Continuous, VarType::Continuous],
            objective: vec![3.0, 2.0],
            sense: ObjectiveSense::Maximize,
            constraints: ConstraintMatrix::new(
                2,
                4,
                vec![0, 2, 4],            // row_start
                vec![0, 1, 0, 1],         // col_index
                vec![1.0, 1.0, 2.0, 1.0], // values
            ),
            rhs_lower: vec![0.0, 0.0], // 无下界
            rhs_upper: vec![4.0, 5.0],
        };

        let result = solver.solve(&problem, 0).expect("solve should succeed");

        assert_eq!(result.status, SolveStatus::Optimal, "LP should be optimal");
        assert!(
            (result.objective_value - 9.0).abs() < 1e-6,
            "objective should be 9.0, got {}",
            result.objective_value
        );
        assert_eq!(result.solution.len(), 2);
        assert!(
            (result.solution[0] - 1.0).abs() < 1e-6,
            "x should be 1.0, got {}",
            result.solution[0]
        );
        assert!(
            (result.solution[1] - 3.0).abs() < 1e-6,
            "y should be 3.0, got {}",
            result.solution[1]
        );
        // 对偶解应存在
        assert!(result.dual_solution.is_some());
    }

    /// T2：真实 LP 求解 — 最小化 2x + 3y，约束 x+y>=1.
    ///
    /// 已知最优解：x=1, y=0, objective=2.
    #[test]
    fn t2_real_lp_solve_minimize() {
        let mut solver = HighsSolver::new().expect("Highs_create");

        // 最小化 2x + 3y
        // s.t. x + y >= 1  (row 0: 1 <= x+y <= inf)
        //      0 <= x <= 1, 0 <= y <= 1
        let problem = LpProblem {
            variables: vec![String::from("x"), String::from("y")],
            lower_bounds: vec![0.0, 0.0],
            upper_bounds: vec![1.0, 1.0],
            var_types: vec![VarType::Continuous, VarType::Continuous],
            objective: vec![2.0, 3.0],
            sense: ObjectiveSense::Minimize,
            constraints: ConstraintMatrix::new(1, 2, vec![0, 2], vec![0, 1], vec![1.0, 1.0]),
            rhs_lower: vec![1.0],  // x+y >= 1
            rhs_upper: vec![1e30], // 无上界
        };

        let result = solver.solve(&problem, 0).expect("solve should succeed");

        assert_eq!(result.status, SolveStatus::Optimal);
        assert!(
            (result.objective_value - 2.0).abs() < 1e-6,
            "objective should be 2.0, got {}",
            result.objective_value
        );
        assert!(
            (result.solution[0] - 1.0).abs() < 1e-6,
            "x should be 1.0, got {}",
            result.solution[0]
        );
        assert!(
            (result.solution[1] - 0.0).abs() < 1e-6,
            "y should be 0.0, got {}",
            result.solution[1]
        );
    }

    /// T3：真实 MILP 求解 — 整数约束改变最优解.
    ///
    /// 最大化 3x + 2y，约束 x+y<=4.5, 2x+y<=5, x,y 整数.
    /// LP 松弛最优：x=0.5, y=4, obj=9.5
    /// MILP 最优：x=1, y=3, obj=9
    #[test]
    fn t3_real_milp_solve_integer() {
        let mut solver = HighsSolver::new().expect("Highs_create");

        let problem = LpProblem {
            variables: vec![String::from("x"), String::from("y")],
            lower_bounds: vec![0.0, 0.0],
            upper_bounds: vec![1e30, 1e30],
            var_types: vec![VarType::Integer, VarType::Integer],
            objective: vec![3.0, 2.0],
            sense: ObjectiveSense::Maximize,
            constraints: ConstraintMatrix::new(
                2,
                4,
                vec![0, 2, 4],
                vec![0, 1, 0, 1],
                vec![1.0, 1.0, 2.0, 1.0],
            ),
            rhs_lower: vec![0.0, 0.0],
            rhs_upper: vec![4.5, 5.0],
        };

        let result = solver.solve(&problem, 0).expect("solve should succeed");

        assert_eq!(result.status, SolveStatus::Optimal);
        // MILP 最优 obj=9 (x=1, y=3)，不是 LP 松弛的 9.5
        assert!(
            (result.objective_value - 9.0).abs() < 1e-6,
            "MILP objective should be 9.0, got {}",
            result.objective_value
        );
        // 验证整数性
        assert!(
            (result.solution[0] - result.solution[0].round()).abs() < 1e-6,
            "x should be integer, got {}",
            result.solution[0]
        );
        assert!(
            (result.solution[1] - result.solution[1].round()).abs() < 1e-6,
            "y should be integer, got {}",
            result.solution[1]
        );
    }

    /// T4：不可行问题检测.
    #[test]
    fn t4_infeasible_problem() {
        let mut solver = HighsSolver::new().expect("Highs_create");

        // x >= 5 AND x <= 3 → 不可行
        let problem = LpProblem {
            variables: vec![String::from("x")],
            lower_bounds: vec![5.0],
            upper_bounds: vec![3.0], // 下界 > 上界 → 不可行
            var_types: vec![VarType::Continuous],
            objective: vec![1.0],
            sense: ObjectiveSense::Minimize,
            constraints: ConstraintMatrix::new(0, 0, vec![0], vec![], vec![]),
            rhs_lower: vec![],
            rhs_upper: vec![],
        };

        let result = solver.solve(&problem, 0).expect("solve should not crash");

        // HiGHS 应报告不可行
        assert!(
            result.status == SolveStatus::Infeasible || result.status == SolveStatus::Unbounded,
            "expected Infeasible/Unbounded, got {:?}",
            result.status
        );
    }

    /// T5：多次求解同一实例（验证句柄复用）.
    #[test]
    fn t5_repeated_solve_same_instance() {
        let mut solver = HighsSolver::new().expect("Highs_create");

        let problem = LpProblem {
            variables: vec![String::from("x"), String::from("y")],
            lower_bounds: vec![0.0, 0.0],
            upper_bounds: vec![10.0, 10.0],
            var_types: vec![VarType::Continuous, VarType::Continuous],
            objective: vec![1.0, 1.0],
            sense: ObjectiveSense::Maximize,
            constraints: ConstraintMatrix::new(1, 2, vec![0, 2], vec![0, 1], vec![1.0, 1.0]),
            rhs_lower: vec![0.0],
            rhs_upper: vec![5.0],
        };

        // 第一次求解
        let r1 = solver.solve(&problem, 0).expect("first solve");
        assert_eq!(r1.status, SolveStatus::Optimal);

        // 第二次求解（同一实例）
        let r2 = solver.solve(&problem, 0).expect("second solve");
        assert_eq!(r2.status, SolveStatus::Optimal);

        // 两次结果应一致
        assert!(
            (r1.objective_value - r2.objective_value).abs() < 1e-6,
            "repeated solve should give same objective"
        );
    }

    /// T6：求解器名称和版本.
    #[test]
    fn t6_name_version() {
        let solver = HighsSolver::new().expect("Highs_create");
        assert_eq!(solver.name(), "HiGHS");
        assert_eq!(solver.version(), "1.7.2");
        assert_eq!(solver.status(), SolverStatus::Idle);
    }

    // ======================================================================
    // v0.202.0 (P0-20)：数值参数透传（set_param 类型分发）
    // ======================================================================
    #[test]
    fn p020_classify_boolean() {
        assert_eq!(classify_param_value("true"), ParamValue::Bool(true));
        assert_eq!(classify_param_value("on"), ParamValue::Bool(true));
        assert_eq!(classify_param_value("off"), ParamValue::Bool(false));
        assert_eq!(classify_param_value("1"), ParamValue::Bool(true));
        assert_eq!(classify_param_value("0"), ParamValue::Bool(false));
    }

    #[test]
    fn p020_classify_numeric() {
        // time_limit=30 → Int（Highs_setIntOptionValue）。
        assert_eq!(classify_param_value("30"), ParamValue::Int(30));
        // mip_rel_gap=0.01 → Double（Highs_setDoubleOptionValue）。
        assert_eq!(classify_param_value("0.01"), ParamValue::Double(0.01));
        assert_eq!(classify_param_value("1e-3"), ParamValue::Double(0.001));
    }

    #[test]
    fn p020_classify_string() {
        assert_eq!(
            classify_param_value("solver"),
            ParamValue::String("solver".to_string())
        );
    }

    #[test]
    fn p020_day_ahead_values_pass_classify() {
        // day_ahead.rs 注入的 `format!("{}", time_limit_s)` / `format!("{}", mip_rel_gap)`。
        // time_limit_s 是整型 → "30" → Int；mip_rel_gap 是 f64 → "0.01" → Double。
        assert_eq!(classify_param_value("30"), ParamValue::Int(30));
        assert_eq!(classify_param_value("0.01"), ParamValue::Double(0.01));
    }
}
