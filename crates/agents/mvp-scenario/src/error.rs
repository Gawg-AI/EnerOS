//! MVP 编排错误类型.

use eneros_energy_market_agent::AgentRuntimeError;

/// MVP 编排错误.
#[derive(Debug)]
pub enum MvpError {
    /// Agent 运行时错误.
    AgentError(AgentRuntimeError),
    /// 编排器未运行（`start` 未调用或已 `stop`）.
    NotRunning,
}

impl core::fmt::Display for MvpError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            // AgentRuntimeError 仅派生 Debug，未实现 Display（v0.72.0），用 Debug 兜底.
            MvpError::AgentError(e) => write!(f, "agent error: {:?}", e),
            MvpError::NotRunning => write!(f, "orchestrator not running"),
        }
    }
}

impl From<AgentRuntimeError> for MvpError {
    fn from(e: AgentRuntimeError) -> Self {
        Self::AgentError(e)
    }
}
