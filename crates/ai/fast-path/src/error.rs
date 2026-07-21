//! 快速路径错误类型（D9）.

use alloc::string::String;

/// 快速路径错误.
///
/// 仅派生 `Debug`（D9：Karpathy 简化原则，与 v0.68.0/v0.69.0 一致）。
#[derive(Debug)]
pub enum FastPathError {
    /// LP 编译错误.
    CompileError(String),
    /// 求解错误.
    SolveError(String),
}
