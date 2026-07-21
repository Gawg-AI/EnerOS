//! Solver 错误类型（D4）.

use alloc::string::String;
use core::fmt;

/// 求解器错误.
///
/// 覆盖 FFI 调用失败、问题传入失败、求解运行失败、参数设置失败、问题定义非法等场景。
/// 默认构建（Mock）下 FFI 错误变体不可达，标 `#[allow(dead_code)]`（D4）。
#[derive(Debug, Clone)]
pub enum SolverError {
    /// FFI 调用失败（动态错误消息）.
    FfiError(String),
    /// 问题传入失败（HiGHS 返回码）.
    PassFailed(i32),
    /// 求解运行失败（HiGHS 返回码）.
    RunFailed(i32),
    /// 参数设置失败（CString 转换等）.
    ParamError(String),
    /// 参数设置失败（参数名）.
    ParamSetFailed(String),
    /// 问题定义非法（变量数不一致等）.
    InvalidProblem(String),
    /// 功能未实现.
    NotImplemented,
}

impl fmt::Display for SolverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SolverError::FfiError(msg) => write!(f, "ffi error: {}", msg),
            SolverError::PassFailed(code) => write!(f, "pass failed with code {}", code),
            SolverError::RunFailed(code) => write!(f, "run failed with code {}", code),
            SolverError::ParamError(msg) => write!(f, "param error: {}", msg),
            SolverError::ParamSetFailed(name) => write!(f, "param set failed: {}", name),
            SolverError::InvalidProblem(msg) => write!(f, "invalid problem: {}", msg),
            SolverError::NotImplemented => f.write_str("not implemented"),
        }
    }
}
