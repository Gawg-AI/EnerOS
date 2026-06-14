use eneros_memory::MemoryEntry;
use eneros_tool::ToolInfo;

use crate::engine::ReasoningInput;

/// Builder for constructing ReasoningInput from various sources
pub struct ReasoningContextBuilder {
    goal: String,
    observations: Vec<String>,
    constraints: Vec<String>,
    memory_entries: Vec<MemoryEntry>,
    available_tools: Vec<ToolInfo>,
}

impl ReasoningContextBuilder {
    /// Create a new builder with a goal
    pub fn new(goal: &str) -> Self {
        Self {
            goal: goal.to_string(),
            observations: Vec::new(),
            constraints: Vec::new(),
            memory_entries: Vec::new(),
            available_tools: Vec::new(),
        }
    }

    /// Create from an event type string
    pub fn from_event(event_type: &str, event_source: &str) -> Self {
        Self::new(&format!("Handle {} from {}", event_type, event_source))
            .with_observation(&format!("Event: {} from {}", event_type, event_source))
    }

    /// Add an observation
    pub fn with_observation(mut self, obs: &str) -> Self {
        self.observations.push(obs.to_string());
        self
    }

    /// Add a constraint
    pub fn with_constraint(mut self, constraint: &str) -> Self {
        self.constraints.push(constraint.to_string());
        self
    }

    /// Add memory entries
    pub fn with_memory(mut self, entries: Vec<MemoryEntry>) -> Self {
        self.memory_entries = entries;
        self
    }

    /// Add available tools
    pub fn with_tools(mut self, tools: Vec<ToolInfo>) -> Self {
        self.available_tools = tools;
        self
    }

    /// Add network state observations from power flow results
    pub fn with_network_state(mut self, converged: bool, total_losses: f64, bus_count: usize) -> Self {
        self.observations.push(format!(
            "Power flow: converged={}, losses={:.2}MW, buses={}",
            converged, total_losses, bus_count
        ));
        if !converged {
            self.constraints.push("Power flow did not converge - system may be unstable".to_string());
        }
        self
    }

    /// Build the ReasoningInput
    pub fn build(self) -> ReasoningInput {
        ReasoningInput {
            goal: self.goal,
            observations: self.observations,
            constraints: self.constraints,
            memory_entries: self.memory_entries,
            available_tools: self.available_tools,
            power_observation: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_builder_basic() {
        let input = ReasoningContextBuilder::new("Handle voltage violation")
            .with_observation("Bus 3 voltage low")
            .with_constraint("Voltage must be 0.95-1.05 pu")
            .build();

        assert_eq!(input.goal, "Handle voltage violation");
        assert_eq!(input.observations.len(), 1);
        assert_eq!(input.constraints.len(), 1);
    }

    #[test]
    fn test_context_builder_from_event() {
        let input = ReasoningContextBuilder::from_event("ConstraintViolation", "bus-3-monitor")
            .build();

        assert!(input.goal.contains("ConstraintViolation"));
        assert!(!input.observations.is_empty());
    }

    #[test]
    fn test_context_builder_with_network_state() {
        let input = ReasoningContextBuilder::new("Analyze grid")
            .with_network_state(true, 13.4, 14)
            .build();

        assert!(input.observations.iter().any(|o| o.contains("converged=true")));
    }

    #[test]
    fn test_context_builder_non_converged() {
        let input = ReasoningContextBuilder::new("Analyze grid")
            .with_network_state(false, 0.0, 14)
            .build();

        assert!(input.constraints.iter().any(|c| c.contains("did not converge")));
    }
}
