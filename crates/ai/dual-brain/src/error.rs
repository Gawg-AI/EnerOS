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
    /// 安全校验 Fatal（v0.206.0 P0-17：Fatal 违规阻断命令下发）.
    SafetyFatal(String),
    /// 决策闸拒绝（22 §2.W5.2：速率超限/能量超限；调用方告警，fail-closed）.
    GateRejected(crate::gate::GateDecision),
}
