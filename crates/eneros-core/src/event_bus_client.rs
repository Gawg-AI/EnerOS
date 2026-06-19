//! EventBusPublisher — 事件总线发布者接口（v0.15.0）
//!
//! 定义了发布事件和直接消息的统一 trait，由两种实现：
//! - `LocalEventBusPublisher`（eneros-eventbus）：进程内使用，包装 `Arc<EventBus>`
//! - `RemoteEventBusPublisher`（eneros-eventbus）：通过 IPC 访问独立 EventBusBroker 进程
//!
//! 该 trait 定义在 eneros-core 中，使得 eneros-agent 等 crate 可以仅依赖
//! eneros-core 即可对事件发布进行抽象访问，避免循环依赖。

use async_trait::async_trait;

use crate::agent_message::AgentMessage;
use crate::event::Event;

/// 事件总线发布者接口。
///
/// 由 `LocalEventBusPublisher`（进程内）和 `RemoteEventBusPublisher`（IPC）实现。
/// Agent 进程通过该 trait 发布事件和直接消息，无需关心 EventBus 是
/// 库级集成还是独立进程。
#[async_trait]
pub trait EventBusPublisher: Send + Sync {
    /// 发布事件到所有订阅者。
    async fn publish_event(&self, event: Event) -> anyhow::Result<()>;

    /// 发送直接消息给特定 Agent（通过事件总线或 IPC）。
    ///
    /// 在进程内模式下，消息被转换为 Event 并发布到 EventBus；
    /// 在远程模式下，消息通过 IPC 发送到 EventBusBroker。
    async fn send_direct_message(&self, message: AgentMessage) -> anyhow::Result<()>;
}
