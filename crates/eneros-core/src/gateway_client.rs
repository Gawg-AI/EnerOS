//! GatewayClient — SafetyGateway 的客户端接口（v0.15.0）
//!
//! 定义了访问 SafetyGateway 服务的统一 trait，由两种实现：
//! - `LocalGatewayClient`（eneros-gateway）：进程内使用，包装 `Arc<SafetyGateway>`
//! - `RemoteGatewayClient`（eneros-gateway）：通过 TCP IPC 访问独立 Gateway 进程
//!
//! 该 trait 定义在 eneros-core 中，使得 eneros-agent 等 crate 可以仅依赖
//! eneros-core 即可对 Gateway 进行抽象访问，避免循环依赖。

use async_trait::async_trait;

use crate::agentos_types::StructuredAction;
use crate::command::Command;
use crate::execution::ExecutionResult;
use crate::pipeline_types::{DecisionContextCore, DecisionResultCore};

/// SafetyGateway 服务的客户端接口。
///
/// 由 `LocalGatewayClient`（进程内）和 `RemoteGatewayClient`（IPC）实现。
/// Agent 进程通过该 trait 访问 Gateway 服务，无需关心 Gateway 是
/// 库级集成还是独立进程。
#[async_trait]
pub trait GatewayClient: Send + Sync {
    /// 立即执行命令（同步路径）。
    ///
    /// 返回命令的执行结果。如果 Gateway 内部执行失败（例如设备 NACK），
    /// 仍然返回 `Ok(ExecutionResult)`，调用方需检查 `result.success`。
    /// 仅当 IPC 通信或 Gateway 内部基础设施出错时返回 `Err`。
    async fn execute_command(&self, cmd: Command) -> anyhow::Result<ExecutionResult>;

    /// 仅校验命令，不执行。
    ///
    /// 通过所有已注册的 SafetyCheck，但不写入设备、不入历史。
    async fn validate_command(&self, cmd: &Command) -> anyhow::Result<()>;

    /// 将命令提交到优先级队列（异步路径）。
    ///
    /// 仅当 Gateway 配置了优先级队列时可用；否则返回错误。
    async fn submit_command(&self, cmd: Command) -> anyhow::Result<()>;

    /// 对结构化动作运行完整的决策管线。
    ///
    /// 输入使用 IPC 友好的 `DecisionContextCore`，返回 `DecisionResultCore`。
    /// 需要 Gateway 端配置了 `ConstrainedDecisionPipeline`。
    async fn decide(
        &self,
        action: StructuredAction,
        ctx: DecisionContextCore,
    ) -> anyhow::Result<DecisionResultCore>;
}
