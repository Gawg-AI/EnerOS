use eneros_core::{EnerOSError, Result};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;

use super::event::Event;
use super::handler::EventHandler;

type HandlerList = Vec<Arc<dyn EventHandler>>;
type HandlerMap = HashMap<String, HandlerList>;

/// Event bus for inter-component communication
pub struct EventBus {
    sender: broadcast::Sender<Event>,
    handlers: Arc<RwLock<HandlerMap>>,
    running: Arc<RwLock<bool>>,
    lagged_messages: Arc<AtomicU64>,
    dispatch_lagged: Arc<AtomicBool>,
    channel_capacity: usize,
}

impl EventBus {
    /// Create a new event bus
    pub fn new(channel_capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(channel_capacity);
        Self {
            sender,
            handlers: Arc::new(RwLock::new(HashMap::new())),
            running: Arc::new(RwLock::new(false)),
            lagged_messages: Arc::new(AtomicU64::new(0)),
            dispatch_lagged: Arc::new(AtomicBool::new(false)),
            channel_capacity,
        }
    }

    /// Publish an event (sends to channel, handlers called in dispatch loop)
    pub fn publish(&self, event: Event) -> Result<()> {
        self.sender
            .send(event)
            .map_err(|e| EnerOSError::EventBus(e.to_string()))?;
        Ok(())
    }

    /// Start background dispatch loop
    pub fn start_dispatch_loop(&self) {
        let mut receiver = self.sender.subscribe();
        let handlers = self.handlers.clone();
        let running = self.running.clone();
        let lagged_messages = self.lagged_messages.clone();
        let dispatch_lagged = self.dispatch_lagged.clone();

        *running.write() = true;
        self.dispatch_lagged.store(false, Ordering::Relaxed);

        tokio::spawn(async move {
            loop {
                if !*running.read() {
                    break;
                }
                match receiver.recv().await {
                    Ok(event) => {
                        let handlers_snapshot: Vec<(String, Vec<Arc<dyn EventHandler>>)> = {
                            let handlers = handlers.read();
                            handlers
                                .iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect()
                        };

                        for (_, handler_list) in handlers_snapshot {
                            for handler in handler_list.iter() {
                                if handler.can_handle(&event.event_type) {
                                    if let Err(e) = handler.handle(event.clone()).await {
                                        tracing::error!(
                                            "Handler '{}' failed: {}",
                                            handler.name(),
                                            e
                                        );
                                    }
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        lagged_messages.fetch_add(n, Ordering::Relaxed);
                        dispatch_lagged.store(true, Ordering::Relaxed);
                        tracing::error!(
                            "Event bus dispatch loop lagged by {} messages; retained events will continue dispatching",
                            n
                        );
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::info!("Event bus channel closed");
                        break;
                    }
                }
            }
        });
    }

    /// Stop the dispatch loop
    pub fn stop_dispatch_loop(&self) {
        *self.running.write() = false;
    }

    /// Subscribe to events
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }

    /// Register an event handler
    pub fn register_handler(&self, handler: Box<dyn EventHandler>) {
        let mut handlers = self.handlers.write();
        handlers
            .entry(handler.name().to_string())
            .or_default()
            .push(Arc::from(handler));
    }

    /// Get handler count
    pub fn handler_count(&self) -> usize {
        let handlers = self.handlers.read();
        handlers.values().map(|v| v.len()).sum()
    }

    /// Get channel capacity
    pub fn capacity(&self) -> usize {
        self.channel_capacity
    }

    /// Number of messages skipped by the internal dispatch receiver.
    pub fn lagged_message_count(&self) -> u64 {
        self.lagged_messages.load(Ordering::Relaxed)
    }

    /// Whether the background dispatch loop believes it is still running.
    pub fn is_dispatch_loop_running(&self) -> bool {
        *self.running.read()
    }

    /// Whether the internal dispatch loop has avoided receiver lag since start.
    pub fn is_dispatch_loop_healthy(&self) -> bool {
        self.is_dispatch_loop_running() && !self.dispatch_lagged.load(Ordering::Relaxed)
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(10_000)
    }
}

#[cfg(test)]
mod tests {
    use super::super::event::{EventPayload, EventType};
    use super::super::handler::CallbackHandler;
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn test_dispatch_loop_records_lag_and_keeps_running() {
        let bus = Arc::new(EventBus::new(1));
        bus.register_handler(Box::new(CallbackHandler::new(
            "slow",
            |_event| async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(())
            },
        )));

        bus.start_dispatch_loop();

        for index in 0..20 {
            bus.publish(Event::new(
                EventType::ConstraintViolation,
                "test",
                EventPayload::Message(format!("event-{index}")),
            ))
            .unwrap();
        }

        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(bus.lagged_message_count() > 0);
        assert!(bus.is_dispatch_loop_running());
        assert!(!bus.is_dispatch_loop_healthy());
        bus.stop_dispatch_loop();
    }
}
