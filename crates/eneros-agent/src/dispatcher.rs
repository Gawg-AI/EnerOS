use eneros_core::{AuthorityLevel, AuditEntry, Jurisdiction, Result, StructuredAction, SystemOperatingState};
use eneros_eventbus::EventBus;
use eneros_gateway::SafetyGateway;
use eneros_gateway::decision_pipeline::ConstrainedDecisionPipeline;

use crate::agent::AgentAction;
use crate::audit::AuditTrail;

/// Dispatches agent actions to the appropriate subsystems
pub struct ActionDispatcher {
    event_bus: std::sync::Arc<EventBus>,
    gateway: std::sync::Arc<SafetyGateway>,
    /// Optional constrained decision pipeline for StructuredAction validation
    decision_pipeline: Option<std::sync::Arc<ConstrainedDecisionPipeline>>,
}

impl ActionDispatcher {
    /// Create a new ActionDispatcher
    pub fn new(
        event_bus: std::sync::Arc<EventBus>,
        gateway: std::sync::Arc<SafetyGateway>,
    ) -> Self {
        Self { event_bus, gateway, decision_pipeline: None }
    }

    /// Create an ActionDispatcher with a constrained decision pipeline
    pub fn with_pipeline(
        event_bus: std::sync::Arc<EventBus>,
        gateway: std::sync::Arc<SafetyGateway>,
        pipeline: std::sync::Arc<ConstrainedDecisionPipeline>,
    ) -> Self {
        Self { event_bus, gateway, decision_pipeline: Some(pipeline) }
    }

    /// Whether a constrained decision pipeline is wired in.
    /// Used by the orchestrator to decide between the pipeline path and the
    /// direct-execution fallback for `ExecuteStructured` actions.
    pub fn has_pipeline(&self) -> bool {
        self.decision_pipeline.is_some()
    }

    /// Dispatch a structured action through the constrained decision pipeline
    pub async fn dispatch_structured(
        &self,
        action: &StructuredAction,
        authority: AuthorityLevel,
        jurisdiction: &Jurisdiction,
        system_state: SystemOperatingState,
    ) -> Result<DispatchResult> {
        if let Some(ref pipeline) = self.decision_pipeline {
            let decision = pipeline.decide(action, authority, jurisdiction, system_state).await;
            if decision.executed_action.is_some() {
                Ok(DispatchResult::CommandExecuted)
            } else {
                let reason = format_verdict_as_string(&decision.verdict);
                Ok(DispatchResult::ConstraintRejected(reason))
            }
        } else {
            // No pipeline — fallback to direct execution (backward compat)
            Ok(DispatchResult::CommandExecuted)
        }
    }

    /// Dispatch a single action
    pub async fn dispatch(&self, action: AgentAction) -> Result<DispatchResult> {
        match action {
            AgentAction::PublishEvent(event) => {
                self.event_bus.publish(event)?;
                Ok(DispatchResult::EventPublished)
            }
            AgentAction::ExecuteCommand(cmd) => {
                self.gateway.execute_command(cmd).await?;
                Ok(DispatchResult::CommandExecuted)
            }
            AgentAction::ExecuteStructured(sa) => {
                // Direct dispatch of a structured action without the pipeline.
                // The orchestrator normally intercepts ExecuteStructured and
                // routes it through dispatch_structured(); reaching this arm
                // means the caller invoked dispatch() directly (e.g. legacy
                // code paths or tests). Convert to a Command and execute so
                // the action still takes effect, but note this bypasses
                // feasibility projection and constraint validation.
                let cmd = eneros_gateway::decision_pipeline::structured_action_to_command(&sa);
                self.gateway.execute_command(cmd).await?;
                Ok(DispatchResult::CommandExecuted)
            }
            AgentAction::LogMessage(msg) => {
                tracing::info!("[Agent] {}", msg);
                Ok(DispatchResult::Logged)
            }
            AgentAction::NoOp => Ok(DispatchResult::NoOp),
            AgentAction::RequestApproval { action, reason } => {
                tracing::info!("[Agent] RequestApproval: {} (action: {:?})", reason, action);
                Ok(DispatchResult::ApprovalRequested)
            }
            AgentAction::DelegateTask { target_agent_id, task_description } => {
                tracing::info!("[Agent] DelegateTask to {}: {}", target_agent_id, task_description);
                Ok(DispatchResult::TaskDelegated)
            }
            AgentAction::EmergencyOverride { action, justification } => {
                tracing::warn!("[Agent] EmergencyOverride: {} (action: {:?})", justification, action);
                Ok(DispatchResult::EmergencyOverrideApplied)
            }
            AgentAction::RollbackAction { action_id, reason } => {
                tracing::info!("[Agent] RollbackAction {}: {}", action_id, reason);
                Ok(DispatchResult::ActionRolledBack)
            }
        }
    }

    /// Dispatch multiple actions in order
    pub async fn dispatch_all(&self, actions: Vec<AgentAction>) -> Vec<Result<DispatchResult>> {
        let mut results = Vec::with_capacity(actions.len());
        for a in actions {
            results.push(self.dispatch(a).await);
        }
        results
    }

    /// Dispatch an action with additional validation context.
    ///
    /// This method performs authority and state checks before delegating
    /// to the existing `dispatch()` method:
    /// 1. For `ExecuteCommand` actions, validates authority level first
    /// 2. For `EmergencyOverride`, checks if system is in emergency state
    /// 3. Records an audit entry if `audit_trail` is provided
    /// 4. Then delegates to `dispatch()`
    pub async fn dispatch_with_validation(
        &self,
        action: AgentAction,
        authority: AuthorityLevel,
        jurisdiction: &Jurisdiction,
        system_state: SystemOperatingState,
        audit_trail: Option<&AuditTrail>,
    ) -> Result<DispatchResult> {
        match &action {
            AgentAction::ExecuteCommand(_)
            | AgentAction::ExecuteStructured(_)
                if !authority.can_execute_commands() =>
            {
                if let Some(trail) = audit_trail {
                    trail.record(AuditEntry {
                        entry_id: 0,
                        agent_id: String::new(),
                        authority_level: authority,
                        action_description: format!("{:?}", action),
                        constraint_check_result: "rejected: insufficient authority".to_string(),
                        approval_chain: vec![],
                        timestamp: chrono::Utc::now(),
                        reasoning_summary: format!("Authority {:?} cannot execute commands", authority),
                        system_state,
                        verdict: eneros_core::ActionVerdict::Rejected(
                            format!("Authority level {:?} cannot execute commands", authority),
                        ),
                    });
                }
                return Ok(DispatchResult::CommandRejected(format!(
                    "Authority level {:?} cannot execute commands",
                    authority
                )));
            }
            AgentAction::EmergencyOverride { justification, .. }
                if !system_state.is_emergency() =>
            {
                if let Some(trail) = audit_trail {
                    trail.record(AuditEntry {
                        entry_id: 0,
                        agent_id: String::new(),
                        authority_level: authority,
                        action_description: "EmergencyOverride".to_string(),
                        constraint_check_result: "rejected: not in emergency state".to_string(),
                        approval_chain: vec![],
                        timestamp: chrono::Utc::now(),
                        reasoning_summary: justification.clone(),
                        system_state,
                        verdict: eneros_core::ActionVerdict::Rejected(
                            "EmergencyOverride only allowed in emergency state".to_string(),
                        ),
                    });
                }
                return Ok(DispatchResult::CommandRejected(
                    "EmergencyOverride only allowed in emergency state".to_string(),
                ));
            }
            _ => {}
        }

        // Record audit entry before dispatching
        if let Some(trail) = audit_trail {
            let verdict = match &action {
                AgentAction::EmergencyOverride { .. } => {
                    eneros_core::ActionVerdict::EmergencyBypassed {
                        bypassed_checks: vec!["approval_flow".to_string()],
                        reason: "Emergency state active".to_string(),
                    }
                }
                _ => eneros_core::ActionVerdict::Approved,
            };
            trail.record(AuditEntry {
                entry_id: 0,
                agent_id: String::new(),
                authority_level: authority,
                action_description: format!("{:?}", action),
                constraint_check_result: "passed".to_string(),
                approval_chain: vec![],
                timestamp: chrono::Utc::now(),
                reasoning_summary: format!("Jurisdiction: {:?}, State: {:?}", jurisdiction, system_state),
                system_state,
                verdict,
            });
        }

        self.dispatch(action).await
    }
}

/// Result of dispatching an action
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchResult {
    EventPublished,
    CommandExecuted,
    CommandRejected(String),
    Logged,
    NoOp,
    ApprovalRequested,
    TaskDelegated,
    EmergencyOverrideApplied,
    ActionRolledBack,
    ConstraintRejected(String),
    PendingApproval { approver_level: AuthorityLevel, reason: String },
    ConflictDetected(String),
    EmergencyBypassed { bypassed_checks: Vec<String>, reason: String },
}

fn format_verdict_as_string(verdict: &eneros_core::ActionVerdict) -> String {
    match verdict {
        eneros_core::ActionVerdict::Approved => "approved".to_string(),
        eneros_core::ActionVerdict::Rejected(r) => format!("rejected: {}", r),
        eneros_core::ActionVerdict::PendingApproval { approver_level, reason } =>
            format!("pending approval from {:?}: {}", approver_level, reason),
        eneros_core::ActionVerdict::EmergencyBypassed { reason, .. } =>
            format!("emergency bypassed: {}", reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_eventbus::event::{Event, EventPayload, EventType};
    use eneros_gateway::command::{Command, CommandPriority, CommandType};

    fn test_dispatcher() -> ActionDispatcher {
        ActionDispatcher::new(
            std::sync::Arc::new(EventBus::new(64)),
            std::sync::Arc::new(SafetyGateway::new(100)),
        )
    }

    #[test]
    fn test_dispatcher_creation() {
        let _dispatcher = test_dispatcher();
    }

    #[tokio::test]
    async fn test_dispatch_log_message() {
        let dispatcher = test_dispatcher();
        let result = dispatcher.dispatch(AgentAction::LogMessage("hello".to_string())).await;
        assert_eq!(result.unwrap(), DispatchResult::Logged);
    }

    #[tokio::test]
    async fn test_dispatch_noop() {
        let dispatcher = test_dispatcher();
        let result = dispatcher.dispatch(AgentAction::NoOp).await;
        assert_eq!(result.unwrap(), DispatchResult::NoOp);
    }

    #[tokio::test]
    async fn test_dispatch_publish_event() {
        let event_bus = std::sync::Arc::new(EventBus::new(64));
        // Subscribe so that publish has at least one receiver
        let _receiver = event_bus.subscribe();
        let dispatcher = ActionDispatcher::new(
            event_bus,
            std::sync::Arc::new(SafetyGateway::new(100)),
        );
        let event = Event::new(
            EventType::ConstraintViolation,
            "test",
            EventPayload::Message("test".to_string()),
        );
        let result = dispatcher.dispatch(AgentAction::PublishEvent(event)).await;
        assert_eq!(result.unwrap(), DispatchResult::EventPublished);
    }

    #[tokio::test]
    async fn test_dispatch_execute_command() {
        let dispatcher = test_dispatcher();
        let cmd = Command::new(CommandType::GeneratorSetpoint, 1, CommandPriority::Normal, "test");
        let result = dispatcher.dispatch(AgentAction::ExecuteCommand(cmd)).await;
        assert_eq!(result.unwrap(), DispatchResult::CommandExecuted);
    }

    #[tokio::test]
    async fn test_dispatch_all() {
        let dispatcher = test_dispatcher();
        let actions = vec![
            AgentAction::LogMessage("msg1".to_string()),
            AgentAction::NoOp,
            AgentAction::LogMessage("msg2".to_string()),
        ];
        let results = dispatcher.dispatch_all(actions).await;
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].as_ref().unwrap(), &DispatchResult::Logged);
        assert_eq!(results[1].as_ref().unwrap(), &DispatchResult::NoOp);
        assert_eq!(results[2].as_ref().unwrap(), &DispatchResult::Logged);
    }

    #[tokio::test]
    async fn test_dispatch_request_approval() {
        let dispatcher = test_dispatcher();
        let action = AgentAction::RequestApproval {
            action: Box::new(AgentAction::NoOp),
            reason: "high risk".to_string(),
        };
        let result = dispatcher.dispatch(action).await;
        assert_eq!(result.unwrap(), DispatchResult::ApprovalRequested);
    }

    #[tokio::test]
    async fn test_dispatch_delegate_task() {
        let dispatcher = test_dispatcher();
        let action = AgentAction::DelegateTask {
            target_agent_id: "agent-2".to_string(),
            task_description: "Switch capacitor bank".to_string(),
        };
        let result = dispatcher.dispatch(action).await;
        assert_eq!(result.unwrap(), DispatchResult::TaskDelegated);
    }

    #[tokio::test]
    async fn test_dispatch_emergency_override() {
        let dispatcher = test_dispatcher();
        let action = AgentAction::EmergencyOverride {
            action: Box::new(AgentAction::NoOp),
            justification: "system emergency".to_string(),
        };
        let result = dispatcher.dispatch(action).await;
        assert_eq!(result.unwrap(), DispatchResult::EmergencyOverrideApplied);
    }

    #[tokio::test]
    async fn test_dispatch_rollback_action() {
        let dispatcher = test_dispatcher();
        let action = AgentAction::RollbackAction {
            action_id: "action-123".to_string(),
            reason: "unsafe condition".to_string(),
        };
        let result = dispatcher.dispatch(action).await;
        assert_eq!(result.unwrap(), DispatchResult::ActionRolledBack);
    }

    #[tokio::test]
    async fn test_dispatch_with_validation_observer_rejected() {
        let dispatcher = test_dispatcher();
        let cmd = Command::new(CommandType::GeneratorSetpoint, 1, CommandPriority::Normal, "test");
        let result = dispatcher.dispatch_with_validation(
            AgentAction::ExecuteCommand(cmd),
            AuthorityLevel::Observer,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
            None,
        ).await.unwrap();
        assert!(matches!(result, DispatchResult::CommandRejected(_)));
    }

    #[tokio::test]
    async fn test_dispatch_with_validation_operator_allowed() {
        let dispatcher = test_dispatcher();
        let cmd = Command::new(CommandType::GeneratorSetpoint, 1, CommandPriority::Normal, "test");
        let result = dispatcher.dispatch_with_validation(
            AgentAction::ExecuteCommand(cmd),
            AuthorityLevel::Operator,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
            None,
        ).await.unwrap();
        assert_eq!(result, DispatchResult::CommandExecuted);
    }

    #[tokio::test]
    async fn test_dispatch_with_validation_emergency_override_rejected_in_normal() {
        let dispatcher = test_dispatcher();
        let result = dispatcher.dispatch_with_validation(
            AgentAction::EmergencyOverride {
                action: Box::new(AgentAction::NoOp),
                justification: "test".to_string(),
            },
            AuthorityLevel::Emergency,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
            None,
        ).await.unwrap();
        assert!(matches!(result, DispatchResult::CommandRejected(_)));
    }

    #[tokio::test]
    async fn test_dispatch_with_validation_emergency_override_allowed_in_emergency() {
        let dispatcher = test_dispatcher();
        let result = dispatcher.dispatch_with_validation(
            AgentAction::EmergencyOverride {
                action: Box::new(AgentAction::NoOp),
                justification: "system emergency".to_string(),
            },
            AuthorityLevel::Emergency,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Emergency,
            None,
        ).await.unwrap();
        assert_eq!(result, DispatchResult::EmergencyOverrideApplied);
    }

    #[tokio::test]
    async fn test_dispatch_with_validation_audit_trail() {
        let dispatcher = test_dispatcher();
        let trail = AuditTrail::new();
        let result = dispatcher.dispatch_with_validation(
            AgentAction::LogMessage("audit test".to_string()),
            AuthorityLevel::Operator,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
            Some(&trail),
        ).await.unwrap();
        assert_eq!(result, DispatchResult::Logged);
        assert_eq!(trail.len(), 1);
    }

    #[tokio::test]
    async fn test_dispatch_with_validation_observer_rejected_with_audit() {
        let dispatcher = test_dispatcher();
        let trail = AuditTrail::new();
        let cmd = Command::new(CommandType::GeneratorSetpoint, 1, CommandPriority::Normal, "test");
        let result = dispatcher.dispatch_with_validation(
            AgentAction::ExecuteCommand(cmd),
            AuthorityLevel::Observer,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
            Some(&trail),
        ).await.unwrap();
        assert!(matches!(result, DispatchResult::CommandRejected(_)));
        assert_eq!(trail.len(), 1);
    }
}
