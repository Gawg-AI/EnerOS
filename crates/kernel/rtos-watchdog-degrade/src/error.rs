//! FlowError — 端到端降级流程错误类型.
//!
//! 覆盖点写入失败、心跳未注册、恢复未进行中三类错误场景。

use core::fmt;

/// 端到端降级流程错误.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowError {
    /// 点写入失败（协议层返回错误）。
    PointWriteFailed,
    /// 心跳未注册（未调用 `on_heartbeat` 即开始监控）。
    HeartbeatNotRegistered,
    /// 恢复未进行中（调用 `transition_step` 时未先 `start_transition`）。
    RecoveryNotInProgress,
}

impl fmt::Display for FlowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlowError::PointWriteFailed => write!(f, "point write failed"),
            FlowError::HeartbeatNotRegistered => write!(f, "heartbeat not registered"),
            FlowError::RecoveryNotInProgress => write!(f, "recovery not in progress"),
        }
    }
}
