//! Agent 运行时生命周期 trait（蓝图 54 WP-54.3 自 energy-market-agent 下移）.
//!
//! 迁移是移动不是重写：`AgentRuntime`/`HeartbeatStatus`/`AgentRuntimeError`
//! 三符号定义体与 energy-market-agent 原文逐字一致（仅 use 路径随 crate 归属
//! 调整）。原位保留 `pub use` re-export 保兼容；五个消费者的依赖树不再被迫
//! 经过 ~7k 行市场业务域 crate。
//!
//! 本模块 feature 门控（`runtime`，可选依赖 eneros-dual-brain 仅为此引入）：
//! 不消费运行时生命周期的 eneros-agent 用户不支付 dual-brain 子树闭包。

use alloc::string::String;

use eneros_dual_brain::DualBrainError;

use crate::descriptor::AgentDescriptor;
use crate::error::AgentError;

/// 心跳状态（D8：2 级，本地定义；v0.33.0 `HealthStatus` 4 级语义不同）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatStatus {
    /// 存活.
    Alive,
    /// 已死亡.
    Dead,
}

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
