use std::sync::Arc;

use eneros_core::{
    AuthorityLevel, AuditEntry, EventBusPublisher, GatewayClient, Jurisdiction, Result,
    DecisionContextCore, StructuredAction, SystemOperatingState,
};
use eneros_eventbus::{EventBus, LocalEventBusPublisher};
use eneros_gateway::{LocalGatewayClient, SafetyGateway};
use eneros_gateway::decision_pipeline::ConstrainedDecisionPipeline;
use eneros_tool::ToolEngine;

use crate::agent::AgentAction;
use crate::audit::AuditTrail;
use crate::context::AgentContext;
use crate::message::AgentMessage;

/// Dispatches agent actions to the appropriate subsystems.
///
/// In v0.15.0 the dispatcher uses trait objects (`EventBusPublisher`,
/// `GatewayClient`) instead of concrete types, enabling Agent process
/// migration. The constrained decision pipeline is optionally held for
/// in-process backward compatibility; when present, `dispatch_structured`
/// uses it directly. When absent, `ExecuteStructured` actions are routed
/// through the `GatewayClient::decide()` method.
pub struct ActionDispatcher {
    event_bus: Arc<dyn EventBusPublisher>,
    gateway_client: Arc<dyn GatewayClient>,
    /// Optional constrained decision pipeline for in-process use.
    /// When Some, `dispatch_structured` uses it directly instead of going
    /// through `gateway_client.decide()`.
    decision_pipeline: Option<Arc<ConstrainedDecisionPipeline>>,
    /// Optional tool engine for CallTool actions.
    /// Uses tokio::sync::RwLock (not parking_lot) because ToolEngine::execute
    /// is async and the read guard must be held across an await point.
    tool_engine: Option<Arc<tokio::sync::RwLock<ToolEngine>>>,
    /// Optional shared context for DelegateTask routing (sends messages to
    /// the target agent via the shared MessageStore or event bus).
    context: Option<Arc<AgentContext>>,
}

impl ActionDispatcher {
    /// Create a new ActionDispatcher from trait object handles.
    pub fn new(
        event_bus: Arc<dyn EventBusPublisher>,
        gateway_client: Arc<dyn GatewayClient>,
    ) -> Self {
        Self {
            event_bus,
            gateway_client,
            decision_pipeline: None,
            tool_engine: None,
            context: None,
        }
    }

    /// Create an ActionDispatcher for in-process use, wrapping concrete
    /// `EventBus` and `SafetyGateway` in their local implementations.
    pub fn new_local(
        event_bus: Arc<EventBus>,
        gateway: Arc<SafetyGateway>,
    ) -> Self {
        Self::new(
            Arc::new(LocalEventBusPublisher::new(event_bus)),
            Arc::new(LocalGatewayClient::new(gateway)),
        )
    }

    /// Attach a constrained decision pipeline for in-process use.
    pub fn with_pipeline(mut self, pipeline: Arc<ConstrainedDecisionPipeline>) -> Self {
        self.decision_pipeline = Some(pipeline);
        self
    }

    /// Attach a tool engine to an existing dispatcher
    pub fn with_tool_engine(
        mut self,
        tool_engine: Arc<tokio::sync::RwLock<ToolEngine>>,
    ) -> Self {
        self.tool_engine = Some(tool_engine);
        self
    }

    /// Attach a shared AgentContext for DelegateTask routing.
    /// When set, DelegateTask actions send a message to the target agent
    /// via the shared MessageStore or event bus instead of just logging.
    pub fn with_context(mut self, ctx: Arc<AgentContext>) -> Self {
        self.context = Some(ctx);
        self
    }

    /// Whether a constrained decision pipeline is wired in.
    /// Used by the orchestrator to decide between the pipeline path and the
    /// direct-execution fallback for `ExecuteStructured` actions.
    pub fn has_pipeline(&self) -> bool {
        self.decision_pipeline.is_some()
    }

    /// Dispatch a structured action through the constrained decision pipeline.
    ///
    /// When a local `decision_pipeline` is configured, uses it directly
    /// (in-process backward compatibility). Otherwise, delegates to
    /// `GatewayClient::decide()` which routes to the Gateway's pipeline.
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
            let ctx_core = DecisionContextCore {
                authority,
                jurisdiction: jurisdiction.clone(),
                system_state,
                observation: None,
                agent_id: String::new(),
                reasoning: String::new(),
            };
            match self.gateway_client.decide(action.clone(), ctx_core).await {
                Ok(result) => {
                    if result.executed {
                        Ok(DispatchResult::CommandExecuted)
                    } else {
                        let reason = format_verdict_as_string(&result.verdict);
                        Ok(DispatchResult::ConstraintRejected(reason))
                    }
                }
                Err(e) => {
                    // Gateway client error (e.g. IPC failure, no pipeline
                    // configured on the remote side). Propagate as an error
                    // so the caller knows the action was NOT executed.
                    tracing::warn!("dispatch_structured gateway error: {}", e);
                    Err(eneros_core::EnerOSError::Internal(format!(
                        "gateway decide failed: {}",
                        e
                    )))
                }
            }
        }
    }

    /// Dispatch a single action
    pub async fn dispatch(&self, action: AgentAction) -> Result<DispatchResult> {
        match action {
            AgentAction::PublishEvent(event) => {
                self.event_bus
                    .publish_event(event)
                    .await
                    .map_err(|e| eneros_core::EnerOSError::Internal(e.to_string()))?;
                Ok(DispatchResult::EventPublished)
            }
            AgentAction::ExecuteCommand(cmd) => {
                self.gateway_client
                    .execute_command(cmd)
                    .await
                    .map_err(|e| eneros_core::EnerOSError::Internal(e.to_string()))?;
                Ok(DispatchResult::CommandExecuted)
            }
            AgentAction::ExecuteStructured(sa) => {
                // Direct dispatch of a structured action without explicit
                // authority/jurisdiction context. Convert to a Command and
                // execute so the action still takes effect. The orchestrator
                // normally intercepts ExecuteStructured and routes it through
                // dispatch_structured() with full context.
                let cmd = eneros_gateway::decision_pipeline::structured_action_to_command(&sa);
                self.gateway_client
                    .execute_command(cmd)
                    .await
                    .map_err(|e| eneros_core::EnerOSError::Internal(e.to_string()))?;
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
                if let Some(ref ctx) = self.context {
                    // Route the task to the target agent via the context's
                    // message passing mechanism (MessageStore in local mode,
                    // event bus in remote mode).
                    let msg = AgentMessage::direct(
                        "orchestrator",
                        &target_agent_id,
                        &task_description,
                    );
                    ctx.send_message(msg);
                    tracing::info!(
                        "[Agent] DelegateTask to {}: {} (routed via context)",
                        target_agent_id,
                        task_description
                    );
                } else {
                    tracing::info!(
                        "[Agent] DelegateTask to {}: {} (no context, logged only)",
                        target_agent_id,
                        task_description
                    );
                }
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
            AgentAction::CallTool { tool_name, params } => {
                if let Some(ref tool_engine) = self.tool_engine {
                    let engine = tool_engine.read().await;
                    let output = engine.execute(&tool_name, params.clone()).await?;
                    if output.success {
                        tracing::info!(
                            "[Agent] CallTool '{}' succeeded: {}",
                            tool_name,
                            output.message
                        );
                        Ok(DispatchResult::ToolExecuted(output.message))
                    } else {
                        tracing::warn!(
                            "[Agent] CallTool '{}' failed: {}",
                            tool_name,
                            output.message
                        );
                        Ok(DispatchResult::CommandRejected(format!(
                            "Tool '{}' failed: {}",
                            tool_name, output.message
                        )))
                    }
                } else {
                    tracing::warn!(
                        "[Agent] CallTool '{}' but no ToolEngine configured",
                        tool_name
                    );
                    Ok(DispatchResult::CommandRejected(
                        "No ToolEngine configured".to_string(),
                    ))
                }
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
    /// A tool was called and executed successfully
    ToolExecuted(String),
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
        ActionDispatcher::new_local(
            Arc::new(EventBus::new(64)),
            Arc::new(SafetyGateway::new(100)),
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
        let event_bus = Arc::new(EventBus::new(64));
        // Subscribe so that publish has at least one receiver
        let _receiver = event_bus.subscribe();
        let dispatcher = ActionDispatcher::new_local(
            event_bus,
            Arc::new(SafetyGateway::new(100)),
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
    async fn test_dispatch_call_tool_no_engine() {
        let dispatcher = test_dispatcher();
        let action = AgentAction::CallTool {
            tool_name: "power_flow".to_string(),
            params: serde_json::json!({}),
        };
        let result = dispatcher.dispatch(action).await;
        // No tool engine configured → rejected
        assert!(matches!(result.unwrap(), DispatchResult::CommandRejected(_)));
    }

    #[tokio::test]
    async fn test_dispatch_call_tool_with_engine() {
        use async_trait::async_trait;
        use eneros_tool::{Tool, ToolOutput};

        struct EchoTool;
        #[async_trait]
        impl Tool for EchoTool {
            fn name(&self) -> &str { "echo" }
            fn description(&self) -> &str { "Echoes the input params" }
            fn parameters_schema(&self) -> serde_json::Value {
                serde_json::json!({"type": "object"})
            }
            async fn execute(&self, params: serde_json::Value) -> eneros_core::Result<ToolOutput> {
                Ok(ToolOutput::ok(params, "echoed"))
            }
        }

        let mut engine = ToolEngine::new();
        engine.register(Box::new(EchoTool));
        let tool_engine = Arc::new(tokio::sync::RwLock::new(engine));

        let dispatcher = test_dispatcher().with_tool_engine(tool_engine);
        let action = AgentAction::CallTool {
            tool_name: "echo".to_string(),
            params: serde_json::json!({"msg": "hello"}),
        };
        let result = dispatcher.dispatch(action).await;
        assert!(matches!(result.unwrap(), DispatchResult::ToolExecuted(_)));
    }

    #[tokio::test]
    async fn test_dispatch_call_tool_unknown_tool() {
        let engine = ToolEngine::new();
        let tool_engine = Arc::new(tokio::sync::RwLock::new(engine));

        let dispatcher = test_dispatcher().with_tool_engine(tool_engine);
        let action = AgentAction::CallTool {
            tool_name: "nonexistent".to_string(),
            params: serde_json::json!({}),
        };
        let result = dispatcher.dispatch(action).await;
        // Unknown tool → ToolOutput::err → CommandRejected
        assert!(matches!(result.unwrap(), DispatchResult::CommandRejected(_)));
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
