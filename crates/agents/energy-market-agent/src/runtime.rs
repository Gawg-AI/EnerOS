//! Agent 运行时 trait + 心跳状态（D6/D8：本地定义）.

use eneros_agent::AgentDescriptor;

use crate::error::AgentRuntimeError;

/// 心跳状态（D8：2 级，本地定义；v0.33.0 `HealthStatus` 4 级语义不同）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatStatus {
    /// 存活.
    Alive,
    /// 已死亡.
    Dead,
}

/// Agent 运行时生命周期接口（D6：本地定义；v0.33.0 `AgentEntry` 语义不同）.
///
/// 定义 Agent 的启动/tick/停止/心跳回调。`now_ms` 由外部提供（D2：no_std 无系统时钟）。
pub trait AgentRuntime {
    /// 返回 Agent 描述符.
    fn descriptor(&self) -> &AgentDescriptor;
    /// 启动 Agent（状态转 Running）.
    fn on_start(&mut self, now_ms: u64) -> Result<(), AgentRuntimeError>;
    /// 周期性 tick（执行业务逻辑）.
    fn on_tick(&mut self, now_ms: u64) -> Result<(), AgentRuntimeError>;
    /// 停止 Agent（状态转 Dead）.
    fn on_stop(&mut self, now_ms: u64) -> Result<(), AgentRuntimeError>;
    /// 心跳检查（Running → Alive，否则 Dead）.
    fn on_heartbeat(&self, now_ms: u64) -> HeartbeatStatus;
}
