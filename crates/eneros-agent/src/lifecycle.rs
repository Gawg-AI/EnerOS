use serde::{Deserialize, Serialize};

/// Agent state in lifecycle
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentState {
    /// Just created, not yet initialized
    Created,
    /// Initializing resources
    Initializing,
    /// Running and processing events
    Running,
    /// Temporarily paused
    Paused,
    /// Gracefully stopping
    Stopping,
    /// Fully stopped
    Stopped,
    /// Failed with error message
    Failed(String),
}

/// Agent lifecycle state machine
pub struct AgentLifecycle {
    state: AgentState,
}

impl AgentLifecycle {
    /// Create a new lifecycle in Created state
    pub fn new() -> Self {
        Self {
            state: AgentState::Created,
        }
    }

    /// Get current state
    pub fn state(&self) -> &AgentState {
        &self.state
    }

    /// Transition to a new state
    pub fn transition(&mut self, new_state: AgentState) -> Result<(), String> {
        let valid = match (&self.state, &new_state) {
            (AgentState::Created, AgentState::Initializing) => true,
            (AgentState::Initializing, AgentState::Running) => true,
            (AgentState::Initializing, AgentState::Failed(_)) => true,
            (AgentState::Running, AgentState::Paused) => true,
            (AgentState::Running, AgentState::Stopping) => true,
            (AgentState::Running, AgentState::Failed(_)) => true,
            (AgentState::Paused, AgentState::Running) => true,
            (AgentState::Paused, AgentState::Stopping) => true,
            (AgentState::Paused, AgentState::Failed(_)) => true,
            (AgentState::Stopping, AgentState::Stopped) => true,
            (AgentState::Stopping, AgentState::Failed(_)) => true,
            (_, AgentState::Failed(_)) => true, // Can always transition to Failed
            _ => false,
        };

        if valid {
            self.state = new_state;
            Ok(())
        } else {
            Err(format!(
                "Invalid transition: {:?} -> {:?}",
                self.state, new_state
            ))
        }
    }

    /// Check if agent is in a runnable state
    pub fn is_runnable(&self) -> bool {
        matches!(self.state, AgentState::Running | AgentState::Paused)
    }
}

impl Default for AgentLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lifecycle_transitions() {
        let mut lifecycle = AgentLifecycle::new();
        assert_eq!(lifecycle.state(), &AgentState::Created);

        lifecycle.transition(AgentState::Initializing).unwrap();
        assert_eq!(lifecycle.state(), &AgentState::Initializing);

        lifecycle.transition(AgentState::Running).unwrap();
        assert_eq!(lifecycle.state(), &AgentState::Running);

        lifecycle.transition(AgentState::Paused).unwrap();
        assert_eq!(lifecycle.state(), &AgentState::Paused);

        lifecycle.transition(AgentState::Running).unwrap();
        lifecycle.transition(AgentState::Stopping).unwrap();
        lifecycle.transition(AgentState::Stopped).unwrap();
        assert_eq!(lifecycle.state(), &AgentState::Stopped);
    }

    #[test]
    fn test_lifecycle_invalid_transition() {
        let mut lifecycle = AgentLifecycle::new();
        assert_eq!(lifecycle.state(), &AgentState::Created);

        // Cannot go directly from Created to Running
        let result = lifecycle.transition(AgentState::Running);
        assert!(result.is_err());
    }

    #[test]
    fn test_lifecycle_always_can_fail() {
        let mut lifecycle = AgentLifecycle::new();
        lifecycle.transition(AgentState::Failed("test error".to_string())).unwrap();
        assert_eq!(lifecycle.state(), &AgentState::Failed("test error".to_string()));
    }

    #[test]
    fn test_lifecycle_is_runnable() {
        let mut lifecycle = AgentLifecycle::new();
        assert!(!lifecycle.is_runnable());

        lifecycle.transition(AgentState::Initializing).unwrap();
        lifecycle.transition(AgentState::Running).unwrap();
        assert!(lifecycle.is_runnable());
    }
}
