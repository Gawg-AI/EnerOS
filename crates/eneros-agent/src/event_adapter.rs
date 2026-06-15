use async_trait::async_trait;
use eneros_core::{AuthorityLevel, Jurisdiction, Result};
use eneros_eventbus::{Event, EventHandler};
use parking_lot::Mutex;
use tokio::sync::Mutex as AsyncMutex;

use crate::agent::{Agent, AgentAction, AgentType};
use crate::context::AgentContext;

/// Adapter that wraps an Agent as an EventHandler for EventBus integration
pub struct AgentEventHandler {
    agent: AsyncMutex<Box<dyn Agent>>,
    name: String,
    handled_event_types: Vec<eneros_eventbus::event::EventType>,
    /// Cached agent name (immutable after construction)
    agent_name: String,
    /// Cached agent type (immutable after construction)
    agent_type: AgentType,
    /// Cached agent authority level (immutable after construction)
    agent_authority_level: AuthorityLevel,
    agent_jurisdiction: Jurisdiction,
    /// Cached tick interval (immutable after construction)
    tick_interval: std::time::Duration,
    /// Last actions produced by the agent (for inspection/testing)
    last_actions: Mutex<Vec<AgentAction>>,
}

impl AgentEventHandler {
    /// Create a new AgentEventHandler
    pub fn new(
        agent: Box<dyn Agent>,
        handled_event_types: Vec<eneros_eventbus::event::EventType>,
    ) -> Self {
        let name = format!("agent_handler_{}", agent.id());
        let agent_name = agent.name().to_string();
        let agent_type = agent.agent_type();
        let agent_authority_level = agent.authority_level();
        let agent_jurisdiction = agent.jurisdiction();
        let tick_interval = agent.tick_interval();
        Self {
            agent: AsyncMutex::new(agent),
            name,
            handled_event_types,
            agent_name,
            agent_type,
            agent_authority_level,
            agent_jurisdiction,
            tick_interval,
            last_actions: Mutex::new(Vec::new()),
        }
    }

    /// Create a handler that handles all event types
    pub fn new_all_events(agent: Box<dyn Agent>) -> Self {
        use eneros_eventbus::event::EventType;
        Self::new(
            agent,
            vec![
                EventType::ConstraintViolation,
                EventType::EquipmentStatusChanged,
                EventType::TopologyChanged,
                EventType::PowerFlowConverged,
                EventType::SystemAlarm,
            ],
        )
    }

    /// Get the last actions produced by the agent
    pub fn last_actions(&self) -> Vec<AgentAction> {
        self.last_actions.lock().clone()
    }

    /// Handle an event with a given AgentContext, returning the actions
    pub async fn handle_with_context(
        &self,
        event: Event,
        ctx: &AgentContext,
    ) -> Result<Vec<AgentAction>> {
        let mut agent = self.agent.lock().await;
        let actions = agent.handle_event(&event, ctx).await?;
        drop(agent); // Release async lock before acquiring last_actions
        *self.last_actions.lock() = actions.clone();
        Ok(actions)
    }

    /// Handle an event as an emergency with a given AgentContext, returning the actions
    pub async fn handle_emergency_with_context(
        &self,
        event: Event,
        ctx: &AgentContext,
    ) -> Result<Vec<AgentAction>> {
        let mut agent = self.agent.lock().await;
        let actions = agent.handle_emergency(&event, ctx).await?;
        drop(agent); // Release async lock before acquiring last_actions
        *self.last_actions.lock() = actions.clone();
        Ok(actions)
    }

    /// Tick the agent with a given AgentContext, returning the actions
    pub async fn tick_with_context(&self, ctx: &AgentContext) -> Result<Vec<AgentAction>> {
        let mut agent = self.agent.lock().await;
        let actions = agent.tick(ctx).await?;
        drop(agent); // Release async lock before acquiring last_actions
        *self.last_actions.lock() = actions.clone();
        Ok(actions)
    }

    /// Get the agent's tick interval
    pub fn tick_interval(&self) -> std::time::Duration {
        self.tick_interval
    }

    /// Get the agent's name
    pub fn agent_name(&self) -> String {
        self.agent_name.clone()
    }

    /// Get the agent's type
    pub fn agent_type(&self) -> AgentType {
        self.agent_type.clone()
    }

    /// Get the agent's authority level
    pub fn agent_authority_level(&self) -> AuthorityLevel {
        self.agent_authority_level
    }

    pub fn agent_jurisdiction(&self) -> Jurisdiction {
        self.agent_jurisdiction.clone()
    }
}

#[async_trait]
impl EventHandler for AgentEventHandler {
    async fn handle(&self, event: Event) -> std::result::Result<(), String> {
        // Note: Without AgentContext, we can only log that an event was received.
        // Full handling requires the Orchestrator to provide AgentContext.
        tracing::info!(
            "AgentEventHandler '{}' received event: {:?}",
            self.name,
            event.event_type
        );
        Ok(())
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn can_handle(&self, event_type: &eneros_eventbus::event::EventType) -> bool {
        self.handled_event_types.contains(event_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentType, MockAgent};
    use crate::context::AgentContext;
    use eneros_eventbus::event::{EventPayload, EventType};
    use eneros_eventbus::EventBus;
    use eneros_gateway::SafetyGateway;
    use eneros_memory::InMemoryMemory;
    use eneros_network::PowerNetwork;
    use eneros_reasoning::RuleBasedEngine;
    use eneros_tool::ToolEngine;
    use parking_lot::RwLock;
    use std::sync::Arc;

    fn test_context() -> AgentContext {
        AgentContext::new(
            Arc::new(EventBus::new(64)),
            Arc::new(SafetyGateway::new(100)),
            Arc::new(RwLock::new(ToolEngine::new())),
            Arc::new(RwLock::new(PowerNetwork::from_ieee14())),
            Arc::new(InMemoryMemory::default()),
            Arc::new(RuleBasedEngine::new()),
        )
    }

    #[test]
    fn test_agent_event_handler_creation() {
        let agent = MockAgent::new("mock-1", "Test Agent", AgentType::Operator);
        let handler = AgentEventHandler::new(
            Box::new(agent),
            vec![EventType::ConstraintViolation, EventType::SystemAlarm],
        );

        assert_eq!(handler.name(), "agent_handler_mock-1");
        assert!(handler.last_actions().is_empty());
    }

    #[test]
    fn test_can_handle_subscribed_types() {
        let agent = MockAgent::new("mock-2", "Filter Agent", AgentType::Operator);
        let handler = AgentEventHandler::new(
            Box::new(agent),
            vec![EventType::ConstraintViolation, EventType::SystemAlarm],
        );

        assert!(handler.can_handle(&EventType::ConstraintViolation));
        assert!(handler.can_handle(&EventType::SystemAlarm));
        assert!(!handler.can_handle(&EventType::TopologyChanged));
        assert!(!handler.can_handle(&EventType::PowerFlowConverged));
    }

    #[tokio::test]
    async fn test_event_handler_handle_succeeds() {
        let agent = MockAgent::new("mock-3", "Handle Agent", AgentType::Dispatcher);
        let handler = AgentEventHandler::new(Box::new(agent), vec![EventType::ConstraintViolation]);

        let event = Event::new(
            EventType::ConstraintViolation,
            "test-source",
            EventPayload::Message("test".to_string()),
        );

        // EventHandler::handle() just logs, should succeed
        let result = handler.handle(event).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_with_context_returns_actions() {
        let agent = MockAgent::new("mock-4", "Context Agent", AgentType::Operator);
        let handler = AgentEventHandler::new(Box::new(agent), vec![EventType::ConstraintViolation]);

        let ctx = test_context();
        let event = Event::new(
            EventType::ConstraintViolation,
            "test-source",
            EventPayload::Message("violation detected".to_string()),
        );

        let actions = handler.handle_with_context(event, &ctx).await.unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], AgentAction::LogMessage(_)));

        // Verify last_actions is stored
        let last = handler.last_actions();
        assert_eq!(last.len(), 1);
        assert!(matches!(last[0], AgentAction::LogMessage(_)));
    }

    #[test]
    fn test_new_all_events_handles_key_types() {
        let agent = MockAgent::new("mock-5", "All Events Agent", AgentType::Planner);
        let handler = AgentEventHandler::new_all_events(Box::new(agent));

        assert!(handler.can_handle(&EventType::ConstraintViolation));
        assert!(handler.can_handle(&EventType::EquipmentStatusChanged));
        assert!(handler.can_handle(&EventType::TopologyChanged));
        assert!(handler.can_handle(&EventType::PowerFlowConverged));
        assert!(handler.can_handle(&EventType::SystemAlarm));
    }
}
