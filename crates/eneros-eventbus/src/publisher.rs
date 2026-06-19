//! EventBusPublisher 实现（v0.15.0）
//!
//! 提供 `EventBusPublisher` trait 的两种实现：
//! - `LocalEventBusPublisher`：进程内使用，包装 `Arc<EventBus>`
//! - `RemoteEventBusPublisher`：通过 IPC 访问独立 EventBusBroker 进程

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use eneros_core::{AgentMessage, EventBusPublisher, Event, EventPayload, EventType};

use crate::bus::EventBus;
use crate::client::EventBusClient;

/// 进程内 EventBusPublisher，包装 `Arc<EventBus>`。
///
/// 用于 Agent 与 EventBus 同进程的场景（如测试、legacy 模式）。
pub struct LocalEventBusPublisher {
    bus: Arc<EventBus>,
}

impl LocalEventBusPublisher {
    pub fn new(bus: Arc<EventBus>) -> Self {
        Self { bus }
    }

    /// 返回内部 EventBus 的引用（供测试或同进程高级用法使用）。
    pub fn bus(&self) -> &Arc<EventBus> {
        &self.bus
    }
}

#[async_trait]
impl EventBusPublisher for LocalEventBusPublisher {
    async fn publish_event(&self, event: Event) -> anyhow::Result<()> {
        self.bus
            .publish(event)
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    async fn send_direct_message(&self, message: AgentMessage) -> anyhow::Result<()> {
        let sender_id = message.sender_id.clone();
        let event = Event::new(
            EventType::AgentMessage,
            &sender_id,
            EventPayload::AgentMessage(message),
        );
        self.bus
            .publish(event)
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }
}

/// 远程 EventBusPublisher，包装 `EventBusClient`。
///
/// 用于 Agent 进程通过 IPC 向独立 EventBusBroker 发布事件。
/// 内部使用 `Arc<Mutex<EventBusClient>>` 因为 `publish()` 需要 `&mut self`。
pub struct RemoteEventBusPublisher {
    client: Arc<Mutex<EventBusClient>>,
}

impl RemoteEventBusPublisher {
    pub fn new(client: EventBusClient) -> Self {
        Self {
            client: Arc::new(Mutex::new(client)),
        }
    }

    pub fn from_arc(client: Arc<Mutex<EventBusClient>>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl EventBusPublisher for RemoteEventBusPublisher {
    async fn publish_event(&self, event: Event) -> anyhow::Result<()> {
        self.client
            .lock()
            .await
            .publish(event)
            .await
            .map_err(|e| anyhow::anyhow!("publish failed: {}", e))
    }

    async fn send_direct_message(&self, message: AgentMessage) -> anyhow::Result<()> {
        let sender_id = message.sender_id.clone();
        let event = Event::new(
            EventType::AgentMessage,
            &sender_id,
            EventPayload::AgentMessage(message),
        );
        self.client
            .lock()
            .await
            .publish(event)
            .await
            .map_err(|e| anyhow::anyhow!("send_direct_message failed: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_local_publisher_publish_event() {
        let bus = Arc::new(EventBus::new(64));
        let _receiver = bus.subscribe();
        let publisher = LocalEventBusPublisher::new(bus);
        let event = Event::new(
            EventType::SystemAlarm,
            "test",
            EventPayload::Message("hello".to_string()),
        );
        let result = publisher.publish_event(event).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_local_publisher_send_direct_message() {
        let bus = Arc::new(EventBus::new(64));
        let _receiver = bus.subscribe();
        let publisher = LocalEventBusPublisher::new(bus);
        let msg = AgentMessage::direct("a", "b", "hello");
        let result = publisher.send_direct_message(msg).await;
        assert!(result.is_ok());
    }
}
