//! Agent 运行时错误类型（D12：仅 Debug，不 Clone）.

use alloc::string::String;

use eneros_agent::AgentError;
use eneros_dual_brain::DualBrainError;

/// Agent 运行时错误.
#[derive(Debug)]
pub enum AgentRuntimeError {
    /// 双脑协调器错误.
    DualBrainError(DualBrainError),
    /// 通道错误.
    ChannelError(String),
    /// 市场数据错误.
    MarketDataError(String),
    /// Agent 框架错误.
    AgentError(AgentError),
    /// Agent 未运行.
    NotRunning,
    /// 设备错误.
    DeviceError(String),
}

impl From<DualBrainError> for AgentRuntimeError {
    fn from(e: DualBrainError) -> Self {
        Self::DualBrainError(e)
    }
}

impl From<AgentError> for AgentRuntimeError {
    fn from(e: AgentError) -> Self {
        Self::AgentError(e)
    }
}
