use async_trait::async_trait;
use super::event::Event;

/// Trait for event handlers
#[async_trait]
pub trait EventHandler: Send + Sync {
    /// Handle an event
    async fn handle(&self, event: Event) -> Result<(), String>;

    /// Get handler name
    fn name(&self) -> &str;

    /// Check if handler can handle this event type
    fn can_handle(&self, event_type: &super::event::EventType) -> bool;
}

/// Simple callback-based event handler
pub struct CallbackHandler {
    name: String,
    callback: Box<dyn Fn(Event) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>> + Send + Sync>,
}

impl CallbackHandler {
    /// Create a new callback handler
    pub fn new<F, Fut>(name: &str, callback: F) -> Self
    where
        F: Fn(Event) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        Self {
            name: name.to_string(),
            callback: Box::new(move |event| Box::pin(callback(event))),
        }
    }
}

#[async_trait]
impl EventHandler for CallbackHandler {
    async fn handle(&self, event: Event) -> Result<(), String> {
        (self.callback)(event).await
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn can_handle(&self, _event_type: &super::event::EventType) -> bool {
        true // Handle all events
    }
}
