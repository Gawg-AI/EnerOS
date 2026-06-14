use std::pin::Pin;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use eneros_core::{EnerOSError, Result};

use crate::event::Event;
use crate::bus::EventBus;
use crate::handler::EventHandler;

/// Event priority level
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub enum EventPriority {
    Low,
    #[default]
    Normal,
    High,
    Critical,
}

/// Dual-channel event bus with priority support.
///
/// Urgent events (High/Critical) are published to a dedicated channel
/// that is always drained first by subscribers. Normal events (Low/Normal)
/// go through the standard EventBus channel.
pub struct PriorityEventBus {
    /// Standard channel for Low/Normal priority events
    normal_bus: EventBus,
    /// Dedicated channel for High/Critical priority events
    urgent_sender: broadcast::Sender<Event>,
    /// Channel capacity for the urgent channel
    urgent_capacity: usize,
}

impl PriorityEventBus {
    /// Create a new priority event bus.
    /// `normal_capacity`: capacity for the normal event channel
    /// `urgent_capacity`: capacity for the urgent event channel
    pub fn new(normal_capacity: usize, urgent_capacity: usize) -> Self {
        let (urgent_sender, _) = broadcast::channel(urgent_capacity);
        Self {
            normal_bus: EventBus::new(normal_capacity),
            urgent_sender,
            urgent_capacity,
        }
    }

    /// Publish an event with the given priority.
    pub fn publish(&self, event: Event, priority: EventPriority) -> Result<()> {
        match priority {
            EventPriority::Low | EventPriority::Normal => {
                // It's OK if no one is listening on the normal bus
                let _ = self.normal_bus.publish(event);
                Ok(())
            }
            EventPriority::High | EventPriority::Critical => {
                // It's OK if no one is listening on the urgent channel
                let _ = self.urgent_sender.send(event);
                Ok(())
            }
        }
    }

    /// Subscribe to both urgent and normal channels.
    /// Returns a `PriorityEventReceiver` that always checks urgent first.
    pub fn subscribe(&self) -> PriorityEventReceiver {
        PriorityEventReceiver {
            urgent_rx: self.urgent_sender.subscribe(),
            normal_rx: self.normal_bus.subscribe(),
        }
    }

    /// Subscribe to urgent events only.
    pub fn subscribe_urgent_only(&self) -> broadcast::Receiver<Event> {
        self.urgent_sender.subscribe()
    }

    /// Subscribe to normal events only.
    pub fn subscribe_normal_only(&self) -> broadcast::Receiver<Event> {
        self.normal_bus.subscribe()
    }

    /// Register a handler on the normal bus.
    pub fn register_handler(&self, handler: Box<dyn EventHandler>) {
        self.normal_bus.register_handler(handler);
    }

    /// Get the underlying normal EventBus reference.
    pub fn normal_bus(&self) -> &EventBus {
        &self.normal_bus
    }

    /// Get urgent channel capacity.
    pub fn urgent_capacity(&self) -> usize {
        self.urgent_capacity
    }
}

/// Receiver that prioritizes urgent events over normal events.
pub struct PriorityEventReceiver {
    urgent_rx: broadcast::Receiver<Event>,
    normal_rx: broadcast::Receiver<Event>,
}

impl PriorityEventReceiver {
    /// Receive the next event, checking urgent channel first.
    /// If an urgent event is available, it is returned immediately.
    /// Otherwise, waits for an event from either channel.
    pub fn recv(&mut self) -> Pin<Box<dyn std::future::Future<Output = Result<Event>> + Send + '_>> {
        Box::pin(async move {
            // Always check urgent first (non-blocking)
            match self.urgent_rx.try_recv() {
                Ok(event) => return Ok(event),
                Err(broadcast::error::TryRecvError::Empty) => {}
                Err(broadcast::error::TryRecvError::Lagged(n)) => {
                    tracing::warn!("Urgent channel lagged by {} messages", n);
                }
                Err(broadcast::error::TryRecvError::Closed) => {
                    // Urgent channel closed, only normal remains
                }
            }

            // No urgent event available; wait on both channels using tokio::select!
            tokio::select! {
                result = self.urgent_rx.recv() => {
                    match result {
                        Ok(event) => Ok(event),
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("Urgent channel lagged by {}", n);
                            self.recv().await
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            self.recv_normal().await
                        }
                    }
                }
                result = self.normal_rx.recv() => {
                    match result {
                        Ok(event) => Ok(event),
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("Normal channel lagged by {}", n);
                            self.recv().await
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            Err(EnerOSError::EventBus("Both channels closed".to_string()))
                        }
                    }
                }
            }
        })
    }

    /// Try to receive without blocking. Checks urgent first.
    pub fn try_recv(&mut self) -> Option<Event> {
        // Check urgent first
        match self.urgent_rx.try_recv() {
            Ok(event) => return Some(event),
            Err(broadcast::error::TryRecvError::Lagged(n)) => {
                tracing::warn!("Urgent channel lagged by {}", n);
            }
            _ => {}
        }
        // Then normal
        match self.normal_rx.try_recv() {
            Ok(event) => Some(event),
            Err(broadcast::error::TryRecvError::Lagged(n)) => {
                tracing::warn!("Normal channel lagged by {}", n);
                None
            }
            _ => None,
        }
    }

    async fn recv_normal(&mut self) -> Result<Event> {
        loop {
            match self.normal_rx.recv().await {
                Ok(event) => return Ok(event),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("Normal channel lagged by {}", n);
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(EnerOSError::EventBus("Normal channel closed".to_string()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, EventType, EventPayload};

    fn make_event(source: &str) -> Event {
        Event::new(EventType::SystemAlarm, source, EventPayload::Message("test".to_string()))
    }

    #[tokio::test]
    async fn test_urgent_events_received_first() {
        let bus = PriorityEventBus::new(100, 100);
        let mut rx = bus.subscribe();

        // Publish a normal event first, then an urgent event
        let normal_event = make_event("normal");
        let urgent_event = make_event("urgent");

        bus.publish(normal_event, EventPriority::Normal).unwrap();
        bus.publish(urgent_event, EventPriority::High).unwrap();

        // The urgent event should be received first
        let received = rx.recv().await.unwrap();
        assert_eq!(received.source, "urgent");

        let received = rx.recv().await.unwrap();
        assert_eq!(received.source, "normal");
    }

    #[tokio::test]
    async fn test_normal_events_work() {
        let bus = PriorityEventBus::new(100, 100);
        let mut rx = bus.subscribe();

        let event1 = make_event("normal1");
        let event2 = make_event("normal2");

        bus.publish(event1, EventPriority::Normal).unwrap();
        bus.publish(event2, EventPriority::Low).unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.source, "normal1");

        let received = rx.recv().await.unwrap();
        assert_eq!(received.source, "normal2");
    }

    #[tokio::test]
    async fn test_subscribe_urgent_only() {
        let bus = PriorityEventBus::new(100, 100);
        let mut urgent_rx = bus.subscribe_urgent_only();

        bus.publish(make_event("normal"), EventPriority::Normal).unwrap();
        bus.publish(make_event("urgent"), EventPriority::Critical).unwrap();

        // Urgent-only subscriber should only receive the urgent event
        let received = urgent_rx.recv().await.unwrap();
        assert_eq!(received.source, "urgent");
    }

    #[tokio::test]
    async fn test_try_recv() {
        let bus = PriorityEventBus::new(100, 100);
        let mut rx = bus.subscribe();

        // No events yet
        assert!(rx.try_recv().is_none());

        // Publish urgent event
        bus.publish(make_event("urgent"), EventPriority::High).unwrap();

        // try_recv should return the urgent event
        let received = rx.try_recv().unwrap();
        assert_eq!(received.source, "urgent");

        // No more events
        assert!(rx.try_recv().is_none());

        // Publish normal event
        bus.publish(make_event("normal"), EventPriority::Normal).unwrap();

        let received = rx.try_recv().unwrap();
        assert_eq!(received.source, "normal");
    }

    #[test]
    fn test_event_priority_default() {
        assert_eq!(EventPriority::default(), EventPriority::Normal);
    }
}
