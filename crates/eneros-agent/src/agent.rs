use async_trait::async_trait;
use eneros_core::{AuthorityLevel, Jurisdiction, Result};
use eneros_eventbus::Event;
use eneros_gateway::command::Command;
use serde::{Deserialize, Serialize};

use crate::context::AgentContext;

/// Agent type classification
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentType {
    /// Dispatch agent — generation scheduling and load balancing
    Dispatcher,
    /// Operation & maintenance agent — fault diagnosis and recovery
    Operator,
    /// Planning agent — expansion and reconfiguration
    Planner,
    /// Trading agent — energy market operations
    Trader,
    /// Custom agent type
    Custom(String),
}

/// Actions an agent can produce
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentAction {
    /// Publish an event to the event bus
    PublishEvent(Event),
    /// Execute a control command (must go through SafetyGateway)
    ExecuteCommand(Command),
    /// Log a message
    LogMessage(String),
    /// No operation
    NoOp,
    /// Request approval from a higher authority agent
    RequestApproval { action: Box<AgentAction>, reason: String },
    /// Delegate a task to another agent
    DelegateTask { target_agent_id: String, task_description: String },
    /// Emergency override — bypass non-critical safety checks
    EmergencyOverride { action: Box<AgentAction>, justification: String },
    /// Rollback a previously executed action
    RollbackAction { action_id: String, reason: String },
}

/// Agent trait — unified interface for all agents
#[async_trait]
pub trait Agent: Send + Sync {
    /// Agent unique ID
    fn id(&self) -> &str;

    /// Agent display name
    fn name(&self) -> &str;

    /// Agent type
    fn agent_type(&self) -> AgentType;

    /// Start the agent
    async fn start(&mut self) -> Result<()>;

    /// Stop the agent
    async fn stop(&mut self) -> Result<()>;

    /// Handle an incoming event
    async fn handle_event(&mut self, event: &Event, ctx: &AgentContext) -> Result<Vec<AgentAction>>;

    /// Periodic tick for proactive behavior
    async fn tick(&mut self, ctx: &AgentContext) -> Result<Vec<AgentAction>>;

    /// Agent's tick interval — how often the orchestrator should call tick()
    fn tick_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(1)
    }

    /// Agent's authority level — controls what actions it can perform
    fn authority_level(&self) -> AuthorityLevel { AuthorityLevel::Observer }

    /// Agent's jurisdiction — defines scope of authority
    fn jurisdiction(&self) -> Jurisdiction { Jurisdiction::unrestricted() }

    /// Handle emergency event — called when system is in Emergency state
    /// Default implementation delegates to handle_event
    async fn handle_emergency(&mut self, event: &Event, ctx: &AgentContext) -> Result<Vec<AgentAction>> {
        self.handle_event(event, ctx).await
    }
}

/// A simple mock agent for testing
pub struct MockAgent {
    id: String,
    name: String,
    agent_type: AgentType,
    started: bool,
    event_count: u32,
    tick_count: u32,
    authority_level: AuthorityLevel,
    jurisdiction: Jurisdiction,
    tick_interval: std::time::Duration,
}

impl MockAgent {
    /// Create a new mock agent
    pub fn new(id: &str, name: &str, agent_type: AgentType) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            agent_type,
            started: false,
            event_count: 0,
            tick_count: 0,
            authority_level: AuthorityLevel::Observer,
            jurisdiction: Jurisdiction::unrestricted(),
            tick_interval: std::time::Duration::from_secs(1),
        }
    }

    /// Set the authority level for this mock agent
    pub fn with_authority_level(mut self, level: AuthorityLevel) -> Self {
        self.authority_level = level;
        self
    }

    /// Set the jurisdiction for this mock agent
    pub fn with_jurisdiction(mut self, jurisdiction: Jurisdiction) -> Self {
        self.jurisdiction = jurisdiction;
        self
    }

    /// Set the tick interval for this mock agent
    pub fn with_tick_interval(mut self, interval: std::time::Duration) -> Self {
        self.tick_interval = interval;
        self
    }

    /// Get event count
    pub fn event_count(&self) -> u32 {
        self.event_count
    }

    /// Get tick count
    pub fn tick_count(&self) -> u32 {
        self.tick_count
    }
}

#[async_trait]
impl Agent for MockAgent {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn agent_type(&self) -> AgentType {
        self.agent_type.clone()
    }

    async fn start(&mut self) -> Result<()> {
        self.started = true;
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        self.started = false;
        Ok(())
    }

    async fn handle_event(&mut self, _event: &Event, _ctx: &AgentContext) -> Result<Vec<AgentAction>> {
        self.event_count += 1;
        Ok(vec![AgentAction::LogMessage(format!(
            "MockAgent {} received event #{}",
            self.name, self.event_count
        ))])
    }

    async fn tick(&mut self, _ctx: &AgentContext) -> Result<Vec<AgentAction>> {
        self.tick_count += 1;
        Ok(vec![AgentAction::NoOp])
    }

    fn authority_level(&self) -> AuthorityLevel {
        self.authority_level
    }

    fn jurisdiction(&self) -> Jurisdiction {
        self.jurisdiction.clone()
    }

    fn tick_interval(&self) -> std::time::Duration {
        self.tick_interval
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use parking_lot::RwLock;
    use eneros_eventbus::EventBus;
    use eneros_gateway::SafetyGateway;
    use eneros_tool::ToolEngine;
    use eneros_network::PowerNetwork;
    use eneros_memory::InMemoryMemory;
    use eneros_reasoning::RuleBasedEngine;

    /// Build a minimal AgentContext for testing
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

    #[tokio::test]
    async fn test_mock_agent_lifecycle() {
        let mut agent = MockAgent::new("test-1", "Test Agent", AgentType::Operator);

        assert_eq!(agent.id(), "test-1");
        assert_eq!(agent.name(), "Test Agent");
        assert_eq!(agent.agent_type(), AgentType::Operator);

        agent.start().await.unwrap();
        agent.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_mock_agent_tick() {
        let mut agent = MockAgent::new("test-2", "Ticker", AgentType::Dispatcher);
        let ctx = test_context();

        let actions = agent.tick(&ctx).await.unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], AgentAction::NoOp));
        assert_eq!(agent.tick_count(), 1);
    }

    #[tokio::test]
    async fn test_mock_agent_handle_event() {
        let mut agent = MockAgent::new("test-3", "EventHandler", AgentType::Operator);
        let ctx = test_context();

        let event = Event::new(
            eneros_eventbus::event::EventType::ConstraintViolation,
            "test-source",
            eneros_eventbus::event::EventPayload::Message("test".to_string()),
        );

        let actions = agent.handle_event(&event, &ctx).await.unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], AgentAction::LogMessage(_)));
        assert_eq!(agent.event_count(), 1);
    }

    #[tokio::test]
    async fn test_agent_default_authority_level() {
        let agent = MockAgent::new("auth-1", "Auth Agent", AgentType::Operator);
        assert_eq!(agent.authority_level(), AuthorityLevel::Observer);
    }

    #[tokio::test]
    async fn test_agent_custom_authority_level() {
        let agent = MockAgent::new("auth-2", "Supervisor Agent", AgentType::Operator)
            .with_authority_level(AuthorityLevel::Supervisor);
        assert_eq!(agent.authority_level(), AuthorityLevel::Supervisor);
    }

    #[tokio::test]
    async fn test_agent_default_jurisdiction() {
        let agent = MockAgent::new("jur-1", "Jur Agent", AgentType::Operator);
        assert!(agent.jurisdiction().contains_zone(1));
        assert!(agent.jurisdiction().contains_device(42));
    }

    #[tokio::test]
    async fn test_agent_custom_jurisdiction() {
        let agent = MockAgent::new("jur-2", "Zoned Agent", AgentType::Operator)
            .with_jurisdiction(Jurisdiction::for_zones(vec![1, 2, 3]));
        assert!(agent.jurisdiction().contains_zone(1));
        assert!(!agent.jurisdiction().contains_zone(99));
    }

    #[tokio::test]
    async fn test_agent_handle_emergency_delegates_to_handle_event() {
        let mut agent = MockAgent::new("emg-1", "Emergency Agent", AgentType::Operator);
        let ctx = test_context();

        let event = Event::new(
            eneros_eventbus::event::EventType::SystemAlarm,
            "emergency-source",
            eneros_eventbus::event::EventPayload::Message("emergency".to_string()),
        );

        let actions = agent.handle_emergency(&event, &ctx).await.unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], AgentAction::LogMessage(_)));
        assert_eq!(agent.event_count(), 1);
    }

    #[test]
    fn test_agent_action_request_approval() {
        let action = AgentAction::RequestApproval {
            action: Box::new(AgentAction::NoOp),
            reason: "high risk operation".to_string(),
        };
        assert!(matches!(action, AgentAction::RequestApproval { .. }));
    }

    #[test]
    fn test_agent_action_delegate_task() {
        let action = AgentAction::DelegateTask {
            target_agent_id: "agent-2".to_string(),
            task_description: "Switch capacitor bank".to_string(),
        };
        assert!(matches!(action, AgentAction::DelegateTask { .. }));
    }

    #[test]
    fn test_agent_action_emergency_override() {
        let action = AgentAction::EmergencyOverride {
            action: Box::new(AgentAction::NoOp),
            justification: "system emergency".to_string(),
        };
        assert!(matches!(action, AgentAction::EmergencyOverride { .. }));
    }

    #[test]
    fn test_agent_action_rollback_action() {
        let action = AgentAction::RollbackAction {
            action_id: "action-123".to_string(),
            reason: "unsafe condition detected".to_string(),
        };
        assert!(matches!(action, AgentAction::RollbackAction { .. }));
    }

    #[test]
    fn test_agent_tick_interval_default() {
        let agent = MockAgent::new("tick-1", "Tick Agent", AgentType::Operator);
        assert_eq!(agent.tick_interval(), std::time::Duration::from_secs(1));
    }

    #[test]
    fn test_agent_tick_interval_custom() {
        let agent = MockAgent::new("tick-2", "Fast Agent", AgentType::Dispatcher)
            .with_tick_interval(std::time::Duration::from_millis(500));
        assert_eq!(agent.tick_interval(), std::time::Duration::from_millis(500));
    }

    #[tokio::test]
    async fn test_tick_all_still_works_with_tick_interval() {
        let ctx = test_context();
        let mut orchestrator = crate::orchestrator::AgentOrchestrator::new(ctx);

        let agent = MockAgent::new("tick-3", "Scheduled Agent", AgentType::Dispatcher)
            .with_tick_interval(std::time::Duration::from_secs(5));
        let handler = crate::event_adapter::AgentEventHandler::new_all_events(Box::new(agent));
        orchestrator.register_agent(handler);

        let results = orchestrator.tick_all().await.unwrap();
        assert_eq!(results.len(), 1);
    }
}
