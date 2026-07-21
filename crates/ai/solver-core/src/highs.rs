//! HiGHS 求解器 Rust 安全封装（D2/D5/D10）.
//!
//! 仅在 `highs-ffi` feature 启用时编译。RAII 管理 HiGHS 对象生命周期.

use alloc::ffi::CString;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr::NonNull;

use crate::error::SolverError;
use crate::problem::{LpProblem, ObjectiveSense, VarType};
use crate::result::{SolveResult, SolveStatus};
use crate::solver::{Solver, SolverStatus};

/// HiGHS 求解器.
///
/// RAII 管理 HiGHS 对象：`new()` 调用 `Highs_create`，`Drop` 调用 `Highs_destroy`。
/// 所有 FFI 调用包裹在 `unsafe` 块中并附 SAFETY 注释.
pub struct HighsSolver {
    /// HiGHS 对象句柄（NonNull 保证非空）.
    handle: NonNull<c_void>,
    /// 当前状态.
    status: SolverStatus,
}

impl HighsSolver {
    /// 创建新的 HiGHS 求解器实例.
    pub fn new() -> Result<Self, SolverError> {
        // SAFETY: `Highs_create` 是线程安全的 C 函数，返回有效指针或空指针.
        let handle = unsafe { crate::ffi::Highs_create() };
        let handle = NonNull::new(handle)
            .ok_or_else(|| SolverError::FfiError(String::from("Highs_create returned null")))?;
        Ok(Self {
            handle,
            status: SolverStatus::Idle,
        })
    }

    /// 设置求解时间限制.
    pub fn set_time_limit(&mut self, seconds: f64) -> Result<(), SolverError> {
        self.set_param("time_limit", &seconds.to_string())
    }

    /// 设置求解器方法（simplex_strategy）.
    pub fn set_method(&mut self, method: &str) -> Result<(), SolverError> {
        self.set_param("simplex_strategy", method)
    }

    /// HiGHS 状态码映射到 SolveStatus.
    fn map_status(status: c_int) -> SolveStatus {
        match status {
            7 => SolveStatus::Optimal,
            8 => SolveStatus::Infeasible,
            9 => SolveStatus::Unbounded,
            10 => SolveStatus::Suboptimal,
            _ => SolveStatus::Error(String::from("unknown highs status")),
        }
    }
}

impl Drop for HighsSolver {
    fn drop(&mut self) {
        // SAFETY: `handle` 是有效的 HiGHS 对象指针，由 `new` 创建.
        // `Highs_destroy` 释放资源，调用后不再使用 handle.
        unsafe {
            crate::ffi::Highs_destroy(self.handle.as_ptr());
        }
    }
}

impl Solver for HighsSolver {
    fn solve(&mut self, problem: &LpProblem, now_ms: u64) -> Result<SolveResult, SolverError> {
        self.status = SolverStatus::Solving;

        let num_col = problem.variables.len() as i32;
        let num_row = problem.constraints.num_rows as i32;
        let num_nz = problem.constraints.num_nz as i32;
        let sense_val = match problem.sense {
            ObjectiveSense::Minimize => 1i32,
            ObjectiveSense::Maximize => -1i32,
        };

        // v0.102.0 增量：含非连续变量（Integer/Binary）时走 MILP 路径.
        let has_integer = problem.var_types.iter().any(|t| *t != VarType::Continuous);
        let ret = if has_integer {
            // 整数性数组：0=连续，1=整数（Binary 以 [0,1] 界 + 1 表达）.
            let integrality: Vec<c_int> = problem
                .var_types
                .iter()
                .map(|t| match t {
                    VarType::Continuous => 0,
                    VarType::Integer | VarType::Binary => 1,
                })
                .collect();
            // SAFETY: 所有指针指向有效的 Rust Vec 内存，求解期间不可修改.
            // `integrality` 长度 == num_col（逐变量一一映射），其生命周期覆盖本次 FFI 调用.
            unsafe {
                crate::ffi::Highs_passMip(
                    self.handle.as_ptr(),
                    num_col,
                    num_row,
                    num_nz,
                    3, // a_format = kRowwise
                    sense_val,
                    0.0,
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
            // SAFETY: 所有指针指向有效的 Rust Vec 内存，求解期间不可修改.
            unsafe {
                crate::ffi::Highs_passLp(
                    self.handle.as_ptr(),
                    num_col,
                    num_row,
                    num_nz,
                    3, // a_format = kRowwise
                    sense_val,
                    0.0,
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
        if ret != 0 {
            self.status = SolverStatus::Error;
            return Err(SolverError::PassFailed(ret));
        }

        // SAFETY: handle 有效，调用 Highs_run 进行求解.
        let ret = unsafe { crate::ffi::Highs_run(self.handle.as_ptr()) };
        if ret != 0 {
            self.status = SolverStatus::Error;
            return Err(SolverError::RunFailed(ret));
        }

        // SAFETY: handle 有效，查询求解结果.
        let model_status = unsafe { crate::ffi::Highs_getModelStatus(self.handle.as_ptr()) };
        let objective_value = unsafe { crate::ffi::Highs_getObjectiveValue(self.handle.as_ptr()) };

        let mut solution = vec![0.0f64; problem.variables.len()];
        let mut dual = vec![0.0f64; problem.variables.len()];
        let mut row_value = vec![0.0f64; problem.constraints.num_rows];
        let mut row_dual = vec![0.0f64; problem.constraints.num_rows];

        // SAFETY: 所有指针指向有效缓冲区，长度匹配 num_col/num_row.
        unsafe {
            crate::ffi::Highs_getSolution(
                self.handle.as_ptr(),
                solution.as_mut_ptr(),
                dual.as_mut_ptr(),
                row_value.as_mut_ptr(),
                row_dual.as_mut_ptr(),
            );
        }

        // 简化：使用 now_ms 作为 elapsed_ms（实际应在调用前后取时间差，
        // 但 no_std 无 Instant，由调用方在 solve 前后传入 now_ms 计算 D1）.
        let elapsed_ms = now_ms;

        self.status = SolverStatus::Idle;
        Ok(SolveResult {
            status: Self::map_status(model_status),
            objective_value,
            solution,
            elapsed_ms,
            dual_solution: Some(dual),
        })
    }

    fn name(&self) -> &'static str {
        "HiGHS"
    }

    fn version(&self) -> &'static str {
        "1.7.2"
    }

    fn set_param(&mut self, key: &str, value: &str) -> Result<(), SolverError> {
        let c_key =
            CString::new(key).map_err(|e| SolverError::ParamError(String::from(e.to_string())))?;
        let c_val = CString::new(value)
            .map_err(|e| SolverError::ParamError(String::from(e.to_string())))?;

        // SAFETY: handle 有效，c_key/c_val 为有效 C 字符串.
        let ret = unsafe {
            crate::ffi::Highs_setStringOptionValue(
                self.handle.as_ptr(),
                c_key.as_ptr(),
                c_val.as_ptr(),
            )
        };
        if ret != 0 {
            return Err(SolverError::ParamSetFailed(String::from(key)));
        }
        Ok(())
    }

    fn set_warm_start(&mut self, solution: &[f64]) -> Result<(), SolverError> {
        // SAFETY: handle 有效；solution 指针指向有效 Rust slice 内存，
        // 长度 == num_col（调用方契约，C51），生命周期覆盖本次 FFI 调用.
        let ret = unsafe { crate::ffi::Highs_setSolution(self.handle.as_ptr(), solution.as_ptr()) };
        if ret != 0 {
            return Err(SolverError::ParamSetFailed(String::from("set_warm_start")));
        }
        Ok(())
    }

    fn status(&self) -> SolverStatus {
        self.status.clone()
    }
}

// 引入 c_int 类型（FFI 模块中使用）.
use core::ffi::c_int;
