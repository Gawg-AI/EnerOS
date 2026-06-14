use eneros_core::{AuthorityLevel, Result, SystemOperatingState};
use eneros_eventbus::{Event, EventHandler};

use crate::agent::AgentType;
use crate::collaboration::CollaborationProtocol;
use crate::context::AgentContext;
use crate::dispatcher::{ActionDispatcher, DispatchResult};
use crate::emergency::EmergencyResponsePipeline;
use crate::event_adapter::AgentEventHandler;
use crate::system_state::{StateTransitionTrigger, StateTransitionResult, SystemStateMachine};
use crate::topology_scheduler::{AgentRegistration, TopologyAwareScheduler};

/// Orchestrates agent execution: event dispatch → agent reasoning → action routing
pub struct AgentOrchestrator {
    ctx: AgentContext,
    agents: Vec<AgentEventHandler>,
    dispatcher: ActionDispatcher,
    protocol: CollaborationProtocol,
    state_machine: SystemStateMachine,
    emergency_pipeline: EmergencyResponsePipeline,
    topology_scheduler: TopologyAwareScheduler,
}

impl AgentOrchestrator {
    /// Create a new orchestrator with the given context
    pub fn new(ctx: AgentContext) -> Self {
        let event_bus = ctx.event_bus.clone();
        let gateway = ctx.gateway.clone();
        Self {
            ctx,
            agents: Vec::new(),
            dispatcher: ActionDispatcher::new(event_bus, gateway),
            protocol: CollaborationProtocol::new(),
            state_machine: SystemStateMachine::new(),
            emergency_pipeline: EmergencyResponsePipeline::new(),
            topology_scheduler: TopologyAwareScheduler::new(),
        }
    }

    /// Register an agent with the orchestrator
    pub fn register_agent(&mut self, handler: AgentEventHandler) {
        self.agents.push(handler);
    }

    /// Process a single event through all registered agents that can handle it
    ///
    /// Enhanced pipeline:
    /// 1. If system is in emergency state, call `handle_emergency()` instead of `handle_event()`
    /// 2. Use `topology_scheduler.route_event()` to determine which agents should receive the event
    pub async fn process_event(&self, event: Event) -> Result<Vec<DispatchResult>> {
        let mut results = Vec::new();

        let is_emergency = self.state_machine.current_state().is_emergency();

        // Use topology scheduler to determine which agents should receive the event
        let routing = self.topology_scheduler.route_event(&event, None);
        let target_ids: std::collections::HashSet<String> = routing.target_agent_ids.into_iter().collect();

        for handler in &self.agents {
            if !handler.can_handle(&event.event_type) {
                continue;
            }

            // If topology scheduler has registered agents, filter by routing result
            if !self.topology_scheduler.is_empty() && !target_ids.contains(handler.name().strip_prefix("agent_handler_").unwrap_or(handler.name())) {
                continue;
            }

            let actions = if is_emergency {
                handler.handle_emergency_with_context(event.clone(), &self.ctx).await?
            } else {
                handler.handle_with_context(event.clone(), &self.ctx).await?
            };

            for action in actions {
                let dispatch_result = self.dispatcher.dispatch(action)?;
                results.push(dispatch_result);
            }
        }

        Ok(results)
    }

    /// Tick all registered agents (for proactive behavior)
    pub async fn tick_all(&self) -> Result<Vec<DispatchResult>> {
        let mut results = Vec::new();

        for handler in &self.agents {
            let actions = handler.tick_with_context(&self.ctx).await?;
            for action in actions {
                let dispatch_result = self.dispatcher.dispatch(action)?;
                results.push(dispatch_result);
            }
        }

        Ok(results)
    }

    /// Get agent count
    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }

    /// Get information about all registered agents
    pub fn registered_agents(&self) -> Vec<(String, AgentType, AuthorityLevel)> {
        self.agents.iter().map(|h| {
            (h.agent_name(), h.agent_type(), h.agent_authority_level())
        }).collect()
    }

    /// Access the collaboration protocol
    pub fn protocol(&self) -> &CollaborationProtocol {
        &self.protocol
    }

    /// Access the collaboration protocol mutably
    pub fn protocol_mut(&mut self) -> &mut CollaborationProtocol {
        &mut self.protocol
    }

    /// Get current system operating state
    pub fn system_state(&self) -> SystemOperatingState {
        self.state_machine.current_state()
    }

    /// Trigger a state transition
    pub fn transition_state(&self, trigger: StateTransitionTrigger) -> StateTransitionResult {
        self.state_machine.transition(trigger)
    }

    /// Check for emergency conditions and auto-respond
    pub fn check_emergency(&self, frequency_hz: f64, branches_tripped: usize, min_voltage_pu: f64, buses_below: usize) -> Vec<crate::emergency::EmergencyResponseResult> {
        let state = self.state_machine.current_state();
        self.emergency_pipeline.auto_respond(frequency_hz, branches_tripped, min_voltage_pu, buses_below, state)
    }

    /// Register an agent with topology scheduling
    pub fn register_with_topology(&self, registration: AgentRegistration) {
        self.topology_scheduler.register(registration);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentType, MockAgent};
    use eneros_eventbus::event::{EventType, EventPayload};
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

    #[tokio::test]
    async fn test_orchestrator_process_event() {
        let ctx = test_context();
        let mut orchestrator = AgentOrchestrator::new(ctx);

        let agent = MockAgent::new("operator-1", "Operator Agent", AgentType::Operator);
        let handler = AgentEventHandler::new(
            Box::new(agent),
            vec![EventType::ConstraintViolation],
        );
        orchestrator.register_agent(handler);

        assert_eq!(orchestrator.agent_count(), 1);

        let event = Event::new(
            EventType::ConstraintViolation,
            "constraint-check",
            EventPayload::Message("Bus 3 voltage low".to_string()),
        );

        let results = orchestrator.process_event(event).await.unwrap();
        // MockAgent returns LogMessage, which dispatches to Logged
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], DispatchResult::Logged);
    }

    #[tokio::test]
    async fn test_orchestrator_tick_all() {
        let ctx = test_context();
        let mut orchestrator = AgentOrchestrator::new(ctx);

        let agent = MockAgent::new("dispatcher-1", "Dispatcher Agent", AgentType::Dispatcher);
        let handler = AgentEventHandler::new_all_events(Box::new(agent));
        orchestrator.register_agent(handler);

        let results = orchestrator.tick_all().await.unwrap();
        // MockAgent.tick() returns NoOp
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], DispatchResult::NoOp);
    }

    #[tokio::test]
    async fn test_orchestrator_filters_by_event_type() {
        let ctx = test_context();
        let mut orchestrator = AgentOrchestrator::new(ctx);

        let agent = MockAgent::new("filtered-1", "Filtered Agent", AgentType::Operator);
        let handler = AgentEventHandler::new(
            Box::new(agent),
            vec![EventType::ConstraintViolation], // Only handles ConstraintViolation
        );
        orchestrator.register_agent(handler);

        // Send an event the agent doesn't handle
        let event = Event::new(
            EventType::PowerFlowConverged,
            "pf-solver",
            EventPayload::Message("Converged".to_string()),
        );

        let results = orchestrator.process_event(event).await.unwrap();
        assert!(results.is_empty()); // Agent doesn't handle this event type
    }

    #[tokio::test]
    async fn test_orchestrator_multiple_agents() {
        let ctx = test_context();
        let mut orchestrator = AgentOrchestrator::new(ctx);

        let agent1 = MockAgent::new("op-1", "Operator 1", AgentType::Operator);
        let agent2 = MockAgent::new("op-2", "Operator 2", AgentType::Operator);

        orchestrator.register_agent(AgentEventHandler::new_all_events(Box::new(agent1)));
        orchestrator.register_agent(AgentEventHandler::new_all_events(Box::new(agent2)));

        assert_eq!(orchestrator.agent_count(), 2);

        let event = Event::new(
            EventType::SystemAlarm,
            "alarm-source",
            EventPayload::Message("System alarm".to_string()),
        );

        let results = orchestrator.process_event(event).await.unwrap();
        // Both agents handle the event, each returns 1 LogMessage
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| *r == DispatchResult::Logged));
    }

    #[test]
    fn test_orchestrator_collaboration_protocol() {
        let ctx = test_context();
        let mut orchestrator = AgentOrchestrator::new(ctx);

        // Assign roles via protocol
        orchestrator.protocol_mut().assign_role("coordinator-1", crate::collaboration::CollaborationRole::Coordinator);
        orchestrator.protocol_mut().assign_role("executor-1", crate::collaboration::CollaborationRole::Executor);

        assert_eq!(orchestrator.protocol().agent_count(), 2);

        // Assign a task
        let task = orchestrator.protocol_mut().assign_task(
            "executor-1",
            "Switch capacitor bank",
            crate::collaboration::CollaborationRole::Executor,
        );
        let task_id = task.id.clone();

        assert_eq!(orchestrator.protocol().all_tasks().len(), 1);
        assert_eq!(orchestrator.protocol().tasks_for_agent("executor-1").len(), 1);

        // Update task through protocol
        let t = orchestrator.protocol_mut().get_task_mut(&task_id).unwrap();
        t.start();
        t.complete("Capacitor bank switched");

        assert_eq!(orchestrator.protocol().pending_tasks().len(), 0);
    }

    #[test]
    fn test_orchestrator_system_state_initial() {
        let ctx = test_context();
        let orchestrator = AgentOrchestrator::new(ctx);
        assert_eq!(orchestrator.system_state(), SystemOperatingState::Normal);
    }

    #[test]
    fn test_orchestrator_transition_state() {
        let ctx = test_context();
        let orchestrator = AgentOrchestrator::new(ctx);

        let result = orchestrator.transition_state(StateTransitionTrigger::CriticalViolation);
        assert!(result.success);
        assert_eq!(orchestrator.system_state(), SystemOperatingState::Alert);
    }

    #[test]
    fn test_orchestrator_check_emergency_no_trigger() {
        let ctx = test_context();
        let orchestrator = AgentOrchestrator::new(ctx);

        let results = orchestrator.check_emergency(50.0, 0, 1.0, 0);
        assert!(results.is_empty());
    }

    #[test]
    fn test_orchestrator_check_emergency_frequency_trigger() {
        let ctx = test_context();
        let orchestrator = AgentOrchestrator::new(ctx);

        let results = orchestrator.check_emergency(49.0, 0, 1.0, 0);
        assert_eq!(results.len(), 1);
        assert!(results[0].success);
    }

    #[tokio::test]
    async fn test_orchestrator_emergency_state_uses_handle_emergency() {
        let ctx = test_context();
        let mut orchestrator = AgentOrchestrator::new(ctx);

        // Transition to emergency state
        orchestrator.transition_state(StateTransitionTrigger::ManualOverride(SystemOperatingState::Emergency));
        assert_eq!(orchestrator.system_state(), SystemOperatingState::Emergency);

        let agent = MockAgent::new("emg-1", "Emergency Agent", AgentType::Operator);
        let handler = AgentEventHandler::new_all_events(Box::new(agent));
        orchestrator.register_agent(handler);

        let event = Event::new(
            EventType::SystemAlarm,
            "emergency-source",
            EventPayload::Message("Emergency!".to_string()),
        );

        let results = orchestrator.process_event(event).await.unwrap();
        // MockAgent.handle_emergency() delegates to handle_event(), returns LogMessage
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], DispatchResult::Logged);
    }

    #[test]
    fn test_orchestrator_register_with_topology() {
        let ctx = test_context();
        let orchestrator = AgentOrchestrator::new(ctx);

        let registration = AgentRegistration {
            agent_id: "topo-agent-1".to_string(),
            jurisdiction: eneros_core::Jurisdiction::for_zones(vec![1, 2]),
            authority_level: eneros_core::AuthorityLevel::Operator,
            subscribed_event_types: vec![EventType::ConstraintViolation],
        };
        orchestrator.register_with_topology(registration);

        // Verify the topology scheduler has the agent registered
        assert!(orchestrator.topology_scheduler.get_agent("topo-agent-1").is_some());
    }

    // === Integration tests for the full Power-Native AgentOS pipeline ===

    /// Full pipeline test: event → state escalation → emergency handling → validated dispatch
    #[tokio::test]
    async fn test_full_pipeline_state_escalation_emergency_dispatch() {
        let ctx = test_context();
        let mut orchestrator = AgentOrchestrator::new(ctx);

        // 1. Register an agent
        let agent = MockAgent::new("pipeline-1", "Pipeline Agent", AgentType::Operator)
            .with_authority_level(eneros_core::AuthorityLevel::Operator);
        let handler = AgentEventHandler::new_all_events(Box::new(agent));
        orchestrator.register_agent(handler);

        // 2. Initially in Normal state
        assert_eq!(orchestrator.system_state(), SystemOperatingState::Normal);

        // 3. Process an event in Normal state — uses handle_event
        let event = Event::new(
            EventType::ConstraintViolation,
            "test-source",
            EventPayload::Message("Voltage low".to_string()),
        );
        let results = orchestrator.process_event(event).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], DispatchResult::Logged);

        // 4. Escalate to Emergency via state machine
        let result = orchestrator.transition_state(StateTransitionTrigger::ManualOverride(SystemOperatingState::Emergency));
        assert!(result.success);
        assert_eq!(orchestrator.system_state(), SystemOperatingState::Emergency);

        // 5. Process an event in Emergency state — uses handle_emergency
        let event2 = Event::new(
            EventType::SystemAlarm,
            "emergency-source",
            EventPayload::Message("Emergency!".to_string()),
        );
        let results2 = orchestrator.process_event(event2).await.unwrap();
        assert_eq!(results2.len(), 1);
        assert_eq!(results2[0], DispatchResult::Logged);

        // 6. Check emergency pipeline triggers
        let emergency_results = orchestrator.check_emergency(49.0, 0, 1.0, 0);
        assert_eq!(emergency_results.len(), 1);
        assert!(emergency_results[0].success);

        // 7. Recover to Normal
        let recovery = orchestrator.transition_state(StateTransitionTrigger::Stabilized);
        assert!(recovery.success);
        assert_eq!(orchestrator.system_state(), SystemOperatingState::Alert);
    }

    /// Observer agent cannot execute commands via dispatch_with_validation
    #[test]
    fn test_observer_cannot_execute_commands() {
        let dispatcher = crate::dispatcher::ActionDispatcher::new(
            Arc::new(EventBus::new(64)),
            Arc::new(SafetyGateway::new(100)),
        );

        let cmd = eneros_gateway::command::Command::new(
            eneros_gateway::command::CommandType::GeneratorSetpoint,
            1,
            eneros_gateway::command::CommandPriority::Normal,
            "observer test",
        );

        // Observer authority should be rejected
        let result = dispatcher.dispatch_with_validation(
            crate::agent::AgentAction::ExecuteCommand(cmd),
            eneros_core::AuthorityLevel::Observer,
            &eneros_core::Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
            None,
        ).unwrap();
        assert!(matches!(result, DispatchResult::CommandRejected(_)));

        // Operator authority should be allowed
        let cmd2 = eneros_gateway::command::Command::new(
            eneros_gateway::command::CommandType::GeneratorSetpoint,
            1,
            eneros_gateway::command::CommandPriority::Normal,
            "operator test",
        );
        let result2 = dispatcher.dispatch_with_validation(
            crate::agent::AgentAction::ExecuteCommand(cmd2),
            eneros_core::AuthorityLevel::Operator,
            &eneros_core::Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
            None,
        ).unwrap();
        assert_eq!(result2, DispatchResult::CommandExecuted);
    }

    /// Emergency state triggers handle_emergency instead of handle_event
    #[tokio::test]
    async fn test_emergency_state_triggers_handle_emergency() {
        let ctx = test_context();
        let mut orchestrator = AgentOrchestrator::new(ctx);

        // Register an agent
        let agent = MockAgent::new("emergency-test-1", "Emergency Test Agent", AgentType::Operator);
        let handler = AgentEventHandler::new_all_events(Box::new(agent));
        orchestrator.register_agent(handler);

        // In Normal state, process_event uses handle_event
        assert_eq!(orchestrator.system_state(), SystemOperatingState::Normal);

        // Transition to Emergency
        orchestrator.transition_state(StateTransitionTrigger::ManualOverride(SystemOperatingState::Emergency));
        assert!(orchestrator.system_state().is_emergency());

        // In Emergency state, process_event uses handle_emergency
        let event = Event::new(
            EventType::SystemAlarm,
            "emergency",
            EventPayload::Message("Emergency event".to_string()),
        );
        let results = orchestrator.process_event(event).await.unwrap();
        // MockAgent.handle_emergency() delegates to handle_event(), returns LogMessage
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], DispatchResult::Logged);
    }

    /// Audit trail records actions through dispatch_with_validation
    #[test]
    fn test_audit_trail_records_actions() {
        use crate::audit::AuditTrail;

        let dispatcher = crate::dispatcher::ActionDispatcher::new(
            Arc::new(EventBus::new(64)),
            Arc::new(SafetyGateway::new(100)),
        );

        let trail = AuditTrail::new();

        // Dispatch a LogMessage with audit trail
        let result = dispatcher.dispatch_with_validation(
            crate::agent::AgentAction::LogMessage("audit test".to_string()),
            eneros_core::AuthorityLevel::Operator,
            &eneros_core::Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
            Some(&trail),
        ).unwrap();
        assert_eq!(result, DispatchResult::Logged);
        assert_eq!(trail.len(), 1);

        // Dispatch an ExecuteCommand with Observer authority — should be rejected and audited
        let cmd = eneros_gateway::command::Command::new(
            eneros_gateway::command::CommandType::GeneratorSetpoint,
            1,
            eneros_gateway::command::CommandPriority::Normal,
            "audit rejection test",
        );
        let result2 = dispatcher.dispatch_with_validation(
            crate::agent::AgentAction::ExecuteCommand(cmd),
            eneros_core::AuthorityLevel::Observer,
            &eneros_core::Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
            Some(&trail),
        ).unwrap();
        assert!(matches!(result2, DispatchResult::CommandRejected(_)));
        assert_eq!(trail.len(), 2);

        // Verify audit entries
        let entries = trail.all_entries();
        assert_eq!(entries[0].authority_level, eneros_core::AuthorityLevel::Operator);
        assert_eq!(entries[1].authority_level, eneros_core::AuthorityLevel::Observer);

        // Verify integrity
        assert!(trail.verify_integrity());
    }
}
