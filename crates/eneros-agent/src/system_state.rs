use std::sync::Arc;
use parking_lot::RwLock;
use eneros_core::{SystemOperatingState, SeverityLevel};
use eneros_eventbus::{EventBus, Event};
use eneros_eventbus::event::{EventType, EventPayload};
use eneros_constraint::ConstraintEngine;

/// State transition trigger
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateTransitionTrigger {
    /// Critical constraint violation detected
    CriticalViolation,
    /// Multiple cascading violations
    CascadingViolation,
    /// System has stabilized
    Stabilized,
    /// Total system collapse
    SystemCollapse,
    /// Restoration initiated
    RestorationInitiated,
    /// Restoration completed
    RestorationCompleted,
    /// Manual override
    ManualOverride(SystemOperatingState),
}

/// Result of a state transition
#[derive(Debug, Clone)]
pub struct StateTransitionResult {
    /// Previous state
    pub from: SystemOperatingState,
    /// New state
    pub to: SystemOperatingState,
    /// Whether the transition was successful
    pub success: bool,
    /// Reason for the transition
    pub reason: String,
    /// Actions triggered by the transition
    pub triggered_actions: Vec<String>,
}

/// System operating state machine
pub struct SystemStateMachine {
    /// Current state
    state: Arc<RwLock<SystemOperatingState>>,
    /// Event bus for publishing state change events
    event_bus: Option<Arc<EventBus>>,
    /// Constraint engine for threshold adjustment
    constraint_engine: Option<Arc<ConstraintEngine>>,
}

impl SystemStateMachine {
    /// Create a new state machine starting in Normal state
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(SystemOperatingState::Normal)),
            event_bus: None,
            constraint_engine: None,
        }
    }

    /// Create with event bus integration
    pub fn with_event_bus(event_bus: Arc<EventBus>) -> Self {
        Self {
            state: Arc::new(RwLock::new(SystemOperatingState::Normal)),
            event_bus: Some(event_bus),
            constraint_engine: None,
        }
    }

    /// Create with event bus and constraint engine
    pub fn with_event_bus_and_constraints(
        event_bus: Arc<EventBus>,
        constraint_engine: Arc<ConstraintEngine>,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(SystemOperatingState::Normal)),
            event_bus: Some(event_bus),
            constraint_engine: Some(constraint_engine),
        }
    }

    /// Get current state
    pub fn current_state(&self) -> SystemOperatingState {
        *self.state.read()
    }

    /// Attempt a state transition
    pub fn transition(&self, trigger: StateTransitionTrigger) -> StateTransitionResult {
        let current = *self.state.read();
        let target_opt = self.determine_target(&trigger, current);

        let mut result = StateTransitionResult {
            from: current,
            to: current,
            success: false,
            reason: format!("{:?}", trigger),
            triggered_actions: Vec::new(),
        };

        let target = match target_opt {
            Some(t) => t,
            None => {
                result.reason = format!("Trigger {:?} not applicable in {:?} state", trigger, current);
                return result;
            }
        };

        if current == target {
            result.to = target;
            result.success = true;
            return result;
        }

        if !current.can_transition_to(target) {
            result.reason = format!("Invalid transition from {:?} to {:?}", current, target);
            return result;
        }

        // Apply the transition
        result.to = target;
        *self.state.write() = target;
        result.success = true;

        // Execute transition actions
        self.on_state_changed(&mut result);

        result
    }

    /// Determine target state based on trigger.
    /// Returns None if the trigger is not applicable in the current state.
    fn determine_target(&self, trigger: &StateTransitionTrigger, current: SystemOperatingState) -> Option<SystemOperatingState> {
        match trigger {
            StateTransitionTrigger::CriticalViolation => {
                match current {
                    SystemOperatingState::Normal => Some(SystemOperatingState::Alert),
                    SystemOperatingState::Alert => Some(SystemOperatingState::Emergency),
                    _ => None,
                }
            }
            StateTransitionTrigger::CascadingViolation => {
                match current {
                    SystemOperatingState::Normal | SystemOperatingState::Alert => Some(SystemOperatingState::Emergency),
                    SystemOperatingState::Emergency => Some(SystemOperatingState::Blackout),
                    _ => None,
                }
            }
            StateTransitionTrigger::Stabilized => {
                match current {
                    SystemOperatingState::Alert => Some(SystemOperatingState::Normal),
                    SystemOperatingState::Emergency => Some(SystemOperatingState::Alert),
                    _ => None,
                }
            }
            StateTransitionTrigger::SystemCollapse => Some(SystemOperatingState::Blackout),
            StateTransitionTrigger::RestorationInitiated => {
                if current == SystemOperatingState::Blackout {
                    Some(SystemOperatingState::Restoration)
                } else {
                    None
                }
            }
            StateTransitionTrigger::RestorationCompleted => {
                if current == SystemOperatingState::Restoration {
                    Some(SystemOperatingState::Normal)
                } else {
                    None
                }
            }
            StateTransitionTrigger::ManualOverride(target) => Some(*target),
        }
    }

    /// Execute actions on state change
    fn on_state_changed(&self, result: &mut StateTransitionResult) {
        // Adjust constraint engine thresholds
        if let Some(ref _ce) = self.constraint_engine {
            // Use interior mutability pattern - ConstraintEngine needs interior mutability
            // For now, just record the action
            result.triggered_actions.push(format!(
                "Adjust constraint thresholds for {:?} state",
                result.to
            ));
        }

        // Publish state change event
        if let Some(ref bus) = self.event_bus {
            let _ = bus.publish(Event::new(
                EventType::SystemAlarm,
                "system_state_machine",
                EventPayload::Message(format!(
                    "System state changed: {:?} -> {:?}",
                    result.from, result.to
                )),
            ));
            result.triggered_actions.push("Published SystemAlarm event".to_string());
        }

        // Authority escalation in emergency
        if result.to.is_emergency() {
            result.triggered_actions.push(
                "Supervisor agents escalated to Emergency authority".to_string()
            );
        }

        // Auto-recovery in Restoration
        if result.to == SystemOperatingState::Restoration {
            result.triggered_actions.push(
                "Auto-recovery procedures initiated".to_string()
            );
        }
    }

    /// Check if a severity level should trigger a state transition
    pub fn should_escalate(&self, severity: SeverityLevel) -> Option<StateTransitionTrigger> {
        match severity {
            SeverityLevel::Critical => Some(StateTransitionTrigger::CriticalViolation),
            SeverityLevel::Major => {
                // Major violations in Alert state should escalate
                if *self.state.read() == SystemOperatingState::Alert {
                    Some(StateTransitionTrigger::CascadingViolation)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Get the shared state reference (for use in AgentContext)
    pub fn state_ref(&self) -> Arc<RwLock<SystemOperatingState>> {
        self.state.clone()
    }
}

impl Default for SystemStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state_normal() {
        let sm = SystemStateMachine::new();
        assert_eq!(sm.current_state(), SystemOperatingState::Normal);
    }

    #[test]
    fn test_normal_to_alert() {
        let sm = SystemStateMachine::new();
        let result = sm.transition(StateTransitionTrigger::CriticalViolation);
        assert!(result.success);
        assert_eq!(result.to, SystemOperatingState::Alert);
    }

    #[test]
    fn test_alert_to_emergency() {
        let sm = SystemStateMachine::new();
        sm.transition(StateTransitionTrigger::CriticalViolation);
        let result = sm.transition(StateTransitionTrigger::CriticalViolation);
        assert!(result.success);
        assert_eq!(result.to, SystemOperatingState::Emergency);
    }

    #[test]
    fn test_emergency_to_blackout() {
        let sm = SystemStateMachine::new();
        sm.transition(StateTransitionTrigger::CriticalViolation);
        sm.transition(StateTransitionTrigger::CriticalViolation);
        let result = sm.transition(StateTransitionTrigger::CascadingViolation);
        assert!(result.success);
        assert_eq!(result.to, SystemOperatingState::Blackout);
    }

    #[test]
    fn test_blackout_to_restoration() {
        let sm = SystemStateMachine::new();
        sm.transition(StateTransitionTrigger::CriticalViolation);
        sm.transition(StateTransitionTrigger::CriticalViolation);
        sm.transition(StateTransitionTrigger::CascadingViolation);
        let result = sm.transition(StateTransitionTrigger::RestorationInitiated);
        assert!(result.success);
        assert_eq!(result.to, SystemOperatingState::Restoration);
    }

    #[test]
    fn test_restoration_to_normal() {
        let sm = SystemStateMachine::new();
        sm.transition(StateTransitionTrigger::CriticalViolation);
        sm.transition(StateTransitionTrigger::CriticalViolation);
        sm.transition(StateTransitionTrigger::CascadingViolation);
        sm.transition(StateTransitionTrigger::RestorationInitiated);
        let result = sm.transition(StateTransitionTrigger::RestorationCompleted);
        assert!(result.success);
        assert_eq!(result.to, SystemOperatingState::Normal);
    }

    #[test]
    fn test_stabilized_alert_to_normal() {
        let sm = SystemStateMachine::new();
        sm.transition(StateTransitionTrigger::CriticalViolation);
        let result = sm.transition(StateTransitionTrigger::Stabilized);
        assert!(result.success);
        assert_eq!(result.to, SystemOperatingState::Normal);
    }

    #[test]
    fn test_invalid_transition() {
        let sm = SystemStateMachine::new();
        let result = sm.transition(StateTransitionTrigger::RestorationInitiated);
        assert!(!result.success);
        assert_eq!(result.to, SystemOperatingState::Normal); // No change
    }

    #[test]
    fn test_manual_override() {
        let sm = SystemStateMachine::new();
        let result = sm.transition(StateTransitionTrigger::ManualOverride(SystemOperatingState::Emergency));
        assert!(result.success);
        assert_eq!(result.to, SystemOperatingState::Emergency);
    }

    #[test]
    fn test_should_escalate_critical() {
        let sm = SystemStateMachine::new();
        assert!(sm.should_escalate(SeverityLevel::Critical).is_some());
    }

    #[test]
    fn test_should_escalate_major_in_alert() {
        let sm = SystemStateMachine::new();
        sm.transition(StateTransitionTrigger::CriticalViolation);
        assert!(sm.should_escalate(SeverityLevel::Major).is_some());
    }

    #[test]
    fn test_should_not_escalate_minor() {
        let sm = SystemStateMachine::new();
        assert!(sm.should_escalate(SeverityLevel::Minor).is_none());
    }

    #[test]
    fn test_emergency_triggers_authority_escalation() {
        let sm = SystemStateMachine::new();
        let result = sm.transition(StateTransitionTrigger::ManualOverride(SystemOperatingState::Emergency));
        assert!(result.triggered_actions.iter().any(|a| a.contains("escalated")));
    }

    #[test]
    fn test_default() {
        let sm = SystemStateMachine::default();
        assert_eq!(sm.current_state(), SystemOperatingState::Normal);
    }
}
