//! 意图解析错误类型（D7）.

use alloc::string::String;

/// 意图解析错误.
///
/// 仅派生 `Debug`（D7：Simplicity First，不派生 `Clone`/`PartialEq`）。
#[derive(Debug)]
pub enum IntentError {
    /// JSON 反序列化失败.
    ParseError(String),
    /// 调度配置非法（时段数为 0 / 功率为负 / SOC 范围不合理 / 价格曲线长度不匹配）.
    InvalidConfig(String),
    /// 意图间约束冲突（预留，本版本未触发）.
    ConstraintConflict(String),
    /// LP 问题编译失败（`SolverError` 不实现 `From`，需显式 `map_err`，D4）.
    CompileError(String),
}
