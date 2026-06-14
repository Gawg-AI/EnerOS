use std::collections::{HashMap, HashSet};
use eneros_core::{ZoneId, Jurisdiction, AuthorityLevel};
use eneros_eventbus::event::{Event, EventType};
use serde::{Deserialize, Serialize};
use parking_lot::RwLock;

/// Agent registration info for scheduling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRegistration {
    /// Agent unique ID
    pub agent_id: String,
    /// Agent's jurisdiction
    pub jurisdiction: Jurisdiction,
    /// Agent's authority level
    pub authority_level: AuthorityLevel,
    /// Event types this agent subscribes to
    pub subscribed_event_types: Vec<EventType>,
}

/// Event routing result
#[derive(Debug, Clone)]
pub struct RoutingResult {
    /// Agent IDs that should receive this event
    pub target_agent_ids: Vec<String>,
    /// Whether the event was routed based on topology
    pub topology_routed: bool,
}

/// Topology-aware scheduler — routes events to agents based on jurisdiction
pub struct TopologyAwareScheduler {
    /// Registered agents
    agents: RwLock<HashMap<String, AgentRegistration>>,
    /// Zone to agent mapping (for fast lookup)
    zone_to_agents: RwLock<HashMap<ZoneId, HashSet<String>>>,
}

impl TopologyAwareScheduler {
    /// Create a new scheduler
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
            zone_to_agents: RwLock::new(HashMap::new()),
        }
    }

    /// Register an agent
    pub fn register(&self, registration: AgentRegistration) {
        // Update zone mapping
        for zone_id in &registration.jurisdiction.zone_ids {
            self.zone_to_agents
                .write()
                .entry(*zone_id)
                .or_default()
                .insert(registration.agent_id.clone());
        }

        // Store registration
        self.agents.write().insert(registration.agent_id.clone(), registration);
    }

    /// Unregister an agent
    pub fn unregister(&self, agent_id: &str) {
        if let Some(reg) = self.agents.write().remove(agent_id) {
            for zone_id in &reg.jurisdiction.zone_ids {
                if let Some(agent_set) = self.zone_to_agents.write().get_mut(zone_id) {
                    agent_set.remove(agent_id);
                }
            }
        }
    }

    /// Route an event to relevant agents
    pub fn route_event(&self, event: &Event, event_zone: Option<ZoneId>) -> RoutingResult {
        let agents = self.agents.read();

        let mut target_ids = Vec::new();

        for (agent_id, reg) in agents.iter() {
            // Check event type subscription
            if !reg.subscribed_event_types.is_empty()
                && !reg.subscribed_event_types.contains(&event.event_type)
            {
                continue;
            }

            // Check jurisdiction
            if let Some(zone_id) = event_zone {
                if !reg.jurisdiction.contains_zone(zone_id) {
                    continue;
                }
            }

            target_ids.push(agent_id.clone());
        }

        // Sort by authority level (higher authority first for priority)
        target_ids.sort_by(|a, b| {
            let auth_a = agents.get(a).map(|r| r.authority_level).unwrap_or(AuthorityLevel::Observer);
            let auth_b = agents.get(b).map(|r| r.authority_level).unwrap_or(AuthorityLevel::Observer);
            auth_b.cmp(&auth_a) // Higher authority first
        });

        RoutingResult {
            target_agent_ids: target_ids,
            topology_routed: event_zone.is_some(),
        }
    }

    /// Get agents responsible for a specific zone
    pub fn agents_for_zone(&self, zone_id: ZoneId) -> Vec<String> {
        self.zone_to_agents
            .read()
            .get(&zone_id)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Get the registration for a specific agent
    pub fn get_agent(&self, agent_id: &str) -> Option<AgentRegistration> {
        self.agents.read().get(agent_id).cloned()
    }

    /// Get all registered agent IDs
    pub fn agent_ids(&self) -> Vec<String> {
        self.agents.read().keys().cloned().collect()
    }

    /// Update jurisdiction for an agent (e.g., after topology change)
    pub fn update_jurisdiction(&self, agent_id: &str, new_jurisdiction: Jurisdiction) {
        let mut agents = self.agents.write();
        if let Some(reg) = agents.get_mut(agent_id) {
            // Remove old zone mappings
            for zone_id in &reg.jurisdiction.zone_ids {
                if let Some(agent_set) = self.zone_to_agents.write().get_mut(zone_id) {
                    agent_set.remove(agent_id);
                }
            }

            // Update jurisdiction
            reg.jurisdiction = new_jurisdiction.clone();

            // Add new zone mappings
            for zone_id in &new_jurisdiction.zone_ids {
                self.zone_to_agents
                    .write()
                    .entry(*zone_id)
                    .or_default()
                    .insert(agent_id.to_string());
            }
        }
    }

    /// Check if an agent has jurisdiction over a zone
    pub fn has_jurisdiction(&self, agent_id: &str, zone_id: ZoneId) -> bool {
        self.agents
            .read()
            .get(agent_id)
            .map(|reg| reg.jurisdiction.contains_zone(zone_id))
            .unwrap_or(false)
    }

    /// Number of registered agents
    pub fn len(&self) -> usize {
        self.agents.read().len()
    }

    /// Check if no agents registered
    pub fn is_empty(&self) -> bool {
        self.agents.read().is_empty()
    }
}

impl Default for TopologyAwareScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_eventbus::event::EventPayload;

    fn make_registration(agent_id: &str, zone_ids: Vec<ZoneId>) -> AgentRegistration {
        AgentRegistration {
            agent_id: agent_id.to_string(),
            jurisdiction: Jurisdiction::for_zones(zone_ids),
            authority_level: AuthorityLevel::Operator,
            subscribed_event_types: vec![EventType::ConstraintViolation, EventType::SystemAlarm],
        }
    }

    #[test]
    fn test_register_and_route() {
        let scheduler = TopologyAwareScheduler::new();
        scheduler.register(make_registration("agent-1", vec![1, 2]));
        scheduler.register(make_registration("agent-2", vec![2, 3]));

        let event = Event::new(
            EventType::ConstraintViolation,
            "test",
            EventPayload::Message("test".to_string()),
        );

        // Route to zone 2 — both agents should receive
        let result = scheduler.route_event(&event, Some(2));
        assert_eq!(result.target_agent_ids.len(), 2);
        assert!(result.topology_routed);
    }

    #[test]
    fn test_route_zone_specific() {
        let scheduler = TopologyAwareScheduler::new();
        scheduler.register(make_registration("agent-1", vec![1]));
        scheduler.register(make_registration("agent-2", vec![2]));

        let event = Event::new(
            EventType::ConstraintViolation,
            "test",
            EventPayload::Message("test".to_string()),
        );

        let result = scheduler.route_event(&event, Some(1));
        assert_eq!(result.target_agent_ids.len(), 1);
        assert_eq!(result.target_agent_ids[0], "agent-1");
    }

    #[test]
    fn test_route_no_zone_broadcast() {
        let scheduler = TopologyAwareScheduler::new();
        scheduler.register(make_registration("agent-1", vec![1]));
        scheduler.register(make_registration("agent-2", vec![2]));

        let event = Event::new(
            EventType::ConstraintViolation,
            "test",
            EventPayload::Message("test".to_string()),
        );

        // No zone specified — broadcast to all subscribed agents
        let result = scheduler.route_event(&event, None);
        assert_eq!(result.target_agent_ids.len(), 2);
        assert!(!result.topology_routed);
    }

    #[test]
    fn test_unregistered_agent_not_routed() {
        let scheduler = TopologyAwareScheduler::new();
        scheduler.register(make_registration("agent-1", vec![1]));
        scheduler.unregister("agent-1");

        let event = Event::new(
            EventType::ConstraintViolation,
            "test",
            EventPayload::Message("test".to_string()),
        );

        let result = scheduler.route_event(&event, Some(1));
        assert!(result.target_agent_ids.is_empty());
    }

    #[test]
    fn test_agents_for_zone() {
        let scheduler = TopologyAwareScheduler::new();
        scheduler.register(make_registration("agent-1", vec![1, 2]));
        scheduler.register(make_registration("agent-2", vec![2, 3]));

        let agents = scheduler.agents_for_zone(2);
        assert_eq!(agents.len(), 2);
    }

    #[test]
    fn test_update_jurisdiction() {
        let scheduler = TopologyAwareScheduler::new();
        scheduler.register(make_registration("agent-1", vec![1]));

        // Agent-1 should be in zone 1
        assert!(scheduler.has_jurisdiction("agent-1", 1));
        assert!(!scheduler.has_jurisdiction("agent-1", 2));

        // Update jurisdiction
        scheduler.update_jurisdiction("agent-1", Jurisdiction::for_zones(vec![2, 3]));

        // Agent-1 should now be in zone 2, not zone 1
        assert!(!scheduler.has_jurisdiction("agent-1", 1));
        assert!(scheduler.has_jurisdiction("agent-1", 2));
    }

    #[test]
    fn test_priority_sorting_by_authority() {
        let scheduler = TopologyAwareScheduler::new();

        let mut reg1 = make_registration("operator", vec![1]);
        reg1.authority_level = AuthorityLevel::Operator;

        let mut reg2 = make_registration("supervisor", vec![1]);
        reg2.authority_level = AuthorityLevel::Supervisor;

        scheduler.register(reg1);
        scheduler.register(reg2);

        let event = Event::new(
            EventType::ConstraintViolation,
            "test",
            EventPayload::Message("test".to_string()),
        );

        let result = scheduler.route_event(&event, Some(1));
        // Supervisor should be first (higher authority)
        assert_eq!(result.target_agent_ids[0], "supervisor");
    }

    #[test]
    fn test_unrestricted_jurisdiction() {
        let scheduler = TopologyAwareScheduler::new();
        let reg = AgentRegistration {
            agent_id: "global-agent".to_string(),
            jurisdiction: Jurisdiction::unrestricted(),
            authority_level: AuthorityLevel::Supervisor,
            subscribed_event_types: vec![EventType::SystemAlarm],
        };
        scheduler.register(reg);

        let event = Event::new(
            EventType::SystemAlarm,
            "test",
            EventPayload::Message("test".to_string()),
        );

        // Unrestricted agent should receive events from any zone
        let result = scheduler.route_event(&event, Some(999));
        assert_eq!(result.target_agent_ids.len(), 1);
    }

    #[test]
    fn test_default_scheduler() {
        let scheduler = TopologyAwareScheduler::default();
        assert!(scheduler.is_empty());
    }

    #[test]
    fn test_len() {
        let scheduler = TopologyAwareScheduler::new();
        scheduler.register(make_registration("a1", vec![1]));
        scheduler.register(make_registration("a2", vec![2]));
        assert_eq!(scheduler.len(), 2);
    }
}
