//! 契约错误类型（D9）.

use alloc::string::String;

/// 契约错误.
///
/// 仅派生 `Debug`（D9：Karpathy 简化原则，与 v0.68.0 `IntentError` 一致）。
#[derive(Debug)]
pub enum ContractError {
    /// 不支持的契约版本.
    UnsupportedVersion(String),
    /// 必填字段缺失.
    MissingField(String),
    /// 字段值非法（字段名, 原因）.
    InvalidValue(String, String),
    /// 序列化/反序列化或编译错误.
    SerializationError(String),
}
