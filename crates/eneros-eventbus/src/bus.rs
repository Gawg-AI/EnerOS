use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::RwLock;
use tokio::sync::broadcast;
use eneros_core::{Result, EnerOSError};

use super::event::Event;
use super::handler::EventHandler;

/// Event bus for inter-component communication
pub struct EventBus {
    sender: broadcast::Sender<Event>,
    handlers: Arc<RwLock<HashMap<String, Vec<Arc<dyn EventHandler>>>>>,
    running: Arc<RwLock<bool>>,
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

        *running.write() = true;

        tokio::spawn(async move {
            loop {
                if !*running.read() {
                    break;
                }
                match receiver.recv().await {
                    Ok(event) => {
                        let handlers_snapshot: Vec<(String, Vec<Arc<dyn EventHandler>>)> = {
                            let handlers = handlers.read();
                            handlers.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                        };

                        for (_, handler_list) in handlers_snapshot {
                            for handler in handler_list.iter() {
                                if handler.can_handle(&event.event_type) {
                                    if let Err(e) = handler.handle(event.clone()).await {
                                        tracing::error!("Handler '{}' failed: {}", handler.name(), e);
                                    }
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Event bus lagged by {} messages", n);
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
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(10_000)
    }
}
