use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use crate::agent::{Agent, AgentType};

type AgentEntry = Arc<parking_lot::RwLock<Box<dyn Agent>>>;

/// Agent registry for managing agent instances
pub struct AgentRegistry {
    agents: RwLock<HashMap<String, AgentEntry>>,
}

impl AgentRegistry {
    /// Create a new agent registry
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
        }
    }

    /// Register an agent
    pub fn register(&self, agent: Box<dyn Agent>) {
        let id = agent.id().to_string();
        let mut agents = self.agents.write();
        agents.insert(id, Arc::new(RwLock::new(agent)));
    }

    /// Unregister an agent by ID
    pub fn unregister(&self, id: &str) -> bool {
        let mut agents = self.agents.write();
        agents.remove(id).is_some()
    }

    /// Check if an agent is registered
    pub fn contains(&self, id: &str) -> bool {
        let agents = self.agents.read();
        agents.contains_key(id)
    }

    /// List all agent IDs
    pub fn list(&self) -> Vec<String> {
        let agents = self.agents.read();
        agents.keys().cloned().collect()
    }

    /// List agents by type
    pub fn list_by_type(&self, agent_type: &AgentType) -> Vec<String> {
        let agents = self.agents.read();
        agents
            .values()
            .filter(|agent| {
                let a = agent.read();
                a.agent_type() == *agent_type
            })
            .map(|agent| {
                let a = agent.read();
                a.id().to_string()
            })
            .collect()
    }

    /// Get agent count
    pub fn count(&self) -> usize {
        self.agents.read().len()
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::MockAgent;

    #[test]
    fn test_registry_register() {
        let registry = AgentRegistry::new();
        let agent = Box::new(MockAgent::new("a1", "Agent 1", AgentType::Operator));
        registry.register(agent);

        assert!(registry.contains("a1"));
        assert_eq!(registry.count(), 1);
    }

    #[test]
    fn test_registry_unregister() {
        let registry = AgentRegistry::new();
        let agent = Box::new(MockAgent::new("a2", "Agent 2", AgentType::Dispatcher));
        registry.register(agent);

        assert!(registry.unregister("a2"));
        assert!(!registry.contains("a2"));
        assert_eq!(registry.count(), 0);
    }

    #[test]
    fn test_registry_list_by_type() {
        let registry = AgentRegistry::new();
        registry.register(Box::new(MockAgent::new("op1", "Op 1", AgentType::Operator)));
        registry.register(Box::new(MockAgent::new("op2", "Op 2", AgentType::Operator)));
        registry.register(Box::new(MockAgent::new("dp1", "Disp 1", AgentType::Dispatcher)));

        let operators = registry.list_by_type(&AgentType::Operator);
        assert_eq!(operators.len(), 2);

        let dispatchers = registry.list_by_type(&AgentType::Dispatcher);
        assert_eq!(dispatchers.len(), 1);
    }

    #[test]
    fn test_registry_list() {
        let registry = AgentRegistry::new();
        registry.register(Box::new(MockAgent::new("a1", "A1", AgentType::Operator)));
        registry.register(Box::new(MockAgent::new("a2", "A2", AgentType::Planner)));

        let list = registry.list();
        assert_eq!(list.len(), 2);
        assert!(list.contains(&"a1".to_string()));
        assert!(list.contains(&"a2".to_string()));
    }
}
