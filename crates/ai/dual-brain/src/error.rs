//! 双脑协调器错误类型（D12：仅 Debug，不 Clone/PartialEq）.

use alloc::string::String;

/// 双脑协调器错误.
#[derive(Debug)]
pub enum DualBrainError {
    /// LLM 推理错误.
    LlmError(String),
    /// 意图解析错误.
    ParseError(String),
    /// 契约校验/转换错误.
    ContractError(String),
    /// LP 求解错误.
    SolveError(String),
    /// 命令下发错误.
    DispatchError(String),
}
