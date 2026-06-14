use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use parking_lot::RwLock;
use eneros_core::{AuthorityLevel, Jurisdiction, SystemOperatingState, AuditEntry};
use eneros_constraint::ConstraintEngine;
use eneros_eventbus::EventBus;
use eneros_gateway::SafetyGateway;
use eneros_tool::ToolEngine;
use eneros_network::PowerNetwork;
use eneros_memory::AgentMemory;
use eneros_reasoning::ReasoningEngine;
use crate::message::AgentMessage;

/// Global sequence counter for message IDs, shared across all AgentContexts
/// that use the same message store.
static MESSAGE_SEQ_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Shared message store that supports cursor-based delivery so that
/// multiple agents can independently read the same messages.
#[derive(Debug)]
pub struct MessageStore {
    /// All messages, ordered by insertion (and thus by seq).
    messages: RwLock<Vec<AgentMessage>>,
}

impl MessageStore {
    /// Create an empty message store.
    pub fn new() -> Self {
        Self {
            messages: RwLock::new(Vec::new()),
        }
    }

    /// Insert a message, assigning it the next global sequence number.
    pub fn push(&self, mut message: AgentMessage) {
        message.seq = MESSAGE_SEQ_COUNTER.fetch_add(1, Ordering::Relaxed);
        self.messages.write().push(message);
    }

    /// Return all messages with seq > `since` that are addressed to `agent_id`.
    /// Does NOT remove messages from the store.
    pub fn messages_since(&self, agent_id: &str, since: u64) -> Vec<AgentMessage> {
        let queue = self.messages.read();
        queue
            .iter()
            .filter(|m| m.seq > since && m.is_for(agent_id))
            .cloned()
            .collect()
    }

    /// Return the highest seq currently in the store (0 if empty).
    pub fn latest_seq(&self) -> u64 {
        let queue = self.messages.read();
        queue.last().map_or(0, |m| m.seq)
    }

    /// Remove messages whose timestamp is older than `max_age` ago.
    /// Returns the number of removed messages.
    pub fn cleanup_old_messages(&self, max_age: Duration) -> usize {
        let cutoff = chrono::Utc::now() - chrono::Duration::from_std(max_age).unwrap_or(chrono::Duration::zero());
        let mut queue = self.messages.write();
        let before = queue.len();
        queue.retain(|m| m.timestamp >= cutoff);
        before - queue.len()
    }

    /// Remove messages that have already been consumed by all known agents.
    /// `min_last_seen` is the minimum last_seen_message_id across all agents;
    /// any message with seq <= that value can be safely removed.
    pub fn cleanup_consumed(&self, min_last_seen: u64) -> usize {
        let mut queue = self.messages.write();
        let before = queue.len();
        queue.retain(|m| m.seq > min_last_seen);
        before - queue.len()
    }
}

impl Default for MessageStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared context available to all agents during execution.
///
/// Provides access to the core subsystems an agent may need:
/// event bus, safety gateway, tool engine, power network, memory, and reasoning.
pub struct AgentContext {
    pub event_bus: Arc<EventBus>,
    pub gateway: Arc<SafetyGateway>,
    pub tool_engine: Arc<RwLock<ToolEngine>>,
    pub network: Arc<RwLock<PowerNetwork>>,
    pub memory: Arc<dyn AgentMemory>,
    pub reasoning: Arc<dyn ReasoningEngine>,
    /// Shared message store (same Arc is cloned across all agents).
    pub message_queue: Arc<MessageStore>,
    /// Constraint engine for pre-action validation
    pub constraint_engine: Option<Arc<ConstraintEngine>>,
    /// Current system operating state
    pub system_state: Arc<RwLock<SystemOperatingState>>,
    /// Agent's authority level
    pub authority: AuthorityLevel,
    /// Agent's jurisdiction
    pub jurisdiction: Jurisdiction,
    /// Audit trail for action recording
    pub audit_trail: Arc<RwLock<Vec<AuditEntry>>>,
    /// Cursor: last message seq this agent has seen
    last_seen_message_id: RwLock<u64>,
}

impl AgentContext {
    /// Create a new AgentContext from the given subsystem handles.
    pub fn new(
        event_bus: Arc<EventBus>,
        gateway: Arc<SafetyGateway>,
        tool_engine: Arc<RwLock<ToolEngine>>,
        network: Arc<RwLock<PowerNetwork>>,
        memory: Arc<dyn AgentMemory>,
        reasoning: Arc<dyn ReasoningEngine>,
    ) -> Self {
        Self {
            event_bus,
            gateway,
            tool_engine,
            network,
            memory,
            reasoning,
            message_queue: Arc::new(MessageStore::new()),
            constraint_engine: None,
            system_state: Arc::new(RwLock::new(SystemOperatingState::Normal)),
            authority: AuthorityLevel::Observer,
            jurisdiction: Jurisdiction::unrestricted(),
            audit_trail: Arc::new(RwLock::new(Vec::new())),
            last_seen_message_id: RwLock::new(0),
        }
    }

    /// Create a new AgentContext with full configuration including authority and jurisdiction.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_authority(
        event_bus: Arc<EventBus>,
        gateway: Arc<SafetyGateway>,
        tool_engine: Arc<RwLock<ToolEngine>>,
        network: Arc<RwLock<PowerNetwork>>,
        memory: Arc<dyn AgentMemory>,
        reasoning: Arc<dyn ReasoningEngine>,
        constraint_engine: Option<Arc<ConstraintEngine>>,
        system_state: Arc<RwLock<SystemOperatingState>>,
        authority: AuthorityLevel,
        jurisdiction: Jurisdiction,
    ) -> Self {
        Self {
            event_bus,
            gateway,
            tool_engine,
            network,
            memory,
            reasoning,
            message_queue: Arc::new(MessageStore::new()),
            constraint_engine,
            system_state,
            authority,
            jurisdiction,
            audit_trail: Arc::new(RwLock::new(Vec::new())),
            last_seen_message_id: RwLock::new(0),
        }
    }

    /// Create a new AgentContext that shares the same message store as an existing context.
    /// This is the key method for multi-agent collaboration: all agents sharing the
    /// same store can independently receive broadcast messages.
    pub fn with_shared_message_store(&self) -> Self {
        Self {
            event_bus: Arc::clone(&self.event_bus),
            gateway: Arc::clone(&self.gateway),
            tool_engine: Arc::clone(&self.tool_engine),
            network: Arc::clone(&self.network),
            memory: self.memory.clone(),
            reasoning: self.reasoning.clone(),
            message_queue: Arc::clone(&self.message_queue),
            constraint_engine: self.constraint_engine.clone(),
            system_state: Arc::clone(&self.system_state),
            authority: self.authority,
            jurisdiction: self.jurisdiction.clone(),
            audit_trail: Arc::clone(&self.audit_trail),
            last_seen_message_id: RwLock::new(0),
        }
    }

    /// Send a message to the shared message store.
    /// The message is assigned a globally unique sequence number.
    pub fn send_message(&self, message: AgentMessage) {
        self.message_queue.push(message);
    }

    /// Receive all new messages addressed to the given agent_id since the last call.
    /// Messages are NOT removed from the store, so other agents can still read them.
    /// The agent's cursor is advanced to the latest message seq.
    pub fn receive_messages(&self, agent_id: &str) -> Vec<AgentMessage> {
        let since = *self.last_seen_message_id.read();
        let messages = self.message_queue.messages_since(agent_id, since);
        if let Some(max_seq) = messages.iter().map(|m| m.seq).max() {
            *self.last_seen_message_id.write() = max_seq;
        }
        messages
    }

    /// Broadcast a message to all agents
    pub fn broadcast_message(&self, sender_id: &str, content: &str) {
        let msg = AgentMessage::broadcast(sender_id, content);
        self.message_queue.push(msg);
    }

    /// Remove messages older than `max_age` from the shared store.
    pub fn cleanup_old_messages(&self, max_age: Duration) -> usize {
        self.message_queue.cleanup_old_messages(max_age)
    }

    /// Remove messages that have been consumed by all agents.
    /// `min_last_seen` should be the minimum last_seen_message_id across all agents.
    pub fn cleanup_consumed(&self, min_last_seen: u64) -> usize {
        self.message_queue.cleanup_consumed(min_last_seen)
    }

    /// Get this agent's last seen message id (useful for coordinated cleanup).
    pub fn last_seen_message_id(&self) -> u64 {
        *self.last_seen_message_id.read()
    }

    /// Check if the system is in emergency state
    pub fn is_emergency(&self) -> bool {
        self.system_state.read().is_emergency()
    }

    /// Get effective authority level considering system state
    pub fn effective_authority(&self) -> AuthorityLevel {
        self.authority.effective_level(self.is_emergency())
    }

    /// Record an audit entry
    pub fn record_audit(&self, entry: AuditEntry) {
        self.audit_trail.write().push(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_memory::InMemoryMemory;
    use eneros_reasoning::RuleBasedEngine;
    use eneros_core::ActionVerdict;

    fn make_ctx() -> AgentContext {
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
    fn test_agent_context_construction() {
        let ctx = make_ctx();

        // Verify the context can be dereferenced
        let _bus = &ctx.event_bus;
        let _gw = &ctx.gateway;
        let _te = &ctx.tool_engine;
        let _net = &ctx.network;
        let _mem = &ctx.memory;
        let _re = &ctx.reasoning;
    }

    #[test]
    fn test_send_and_receive_direct_message() {
        let ctx = make_ctx();

        let msg = AgentMessage::direct("agent_a", "agent_b", "hello");
        ctx.send_message(msg);

        let received = ctx.receive_messages("agent_b");
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].sender_id, "agent_a");
        assert_eq!(received[0].content, "hello");
    }

    #[test]
    fn test_broadcast_message_received_by_all() {
        let ctx = make_ctx();
        let ctx_b = ctx.with_shared_message_store();
        let ctx_c = ctx.with_shared_message_store();

        ctx.broadcast_message("agent_a", "announcement");

        // Both agent_b and agent_c should receive the broadcast
        let for_b = ctx_b.receive_messages("agent_b");
        assert_eq!(for_b.len(), 1);
        assert_eq!(for_b[0].content, "announcement");
        assert!(for_b[0].is_broadcast);

        let for_c = ctx_c.receive_messages("agent_c");
        assert_eq!(for_c.len(), 1);
        assert_eq!(for_c[0].content, "announcement");
        assert!(for_c[0].is_broadcast);
    }

    #[test]
    fn test_receive_messages_returns_new_messages_each_time() {
        let ctx = make_ctx();

        ctx.send_message(AgentMessage::direct("a", "b", "msg1"));
        ctx.send_message(AgentMessage::direct("a", "b", "msg2"));

        let first = ctx.receive_messages("b");
        assert_eq!(first.len(), 2);

        // No new messages since last receive
        let second = ctx.receive_messages("b");
        assert!(second.is_empty());

        // Send a new message, should receive only the new one
        ctx.send_message(AgentMessage::direct("a", "b", "msg3"));
        let third = ctx.receive_messages("b");
        assert_eq!(third.len(), 1);
        assert_eq!(third[0].content, "msg3");
    }

    #[test]
    fn test_two_contexts_sharing_store_both_receive_broadcast() {
        let ctx = make_ctx();
        let ctx_b = ctx.with_shared_message_store();
        let ctx_c = ctx.with_shared_message_store();

        // Broadcast from the original context
        ctx.broadcast_message("agent_a", "fire_drill");

        let for_b = ctx_b.receive_messages("agent_b");
        let for_c = ctx_c.receive_messages("agent_c");

        assert_eq!(for_b.len(), 1);
        assert_eq!(for_b[0].content, "fire_drill");

        assert_eq!(for_c.len(), 1);
        assert_eq!(for_c[0].content, "fire_drill");
    }

    #[test]
    fn test_non_broadcast_messages_only_received_by_target() {
        let ctx = make_ctx();
        let ctx_b = ctx.with_shared_message_store();
        let ctx_c = ctx.with_shared_message_store();

        // Direct message to agent_b only
        ctx.send_message(AgentMessage::direct("agent_a", "agent_b", "secret"));

        let for_b = ctx_b.receive_messages("agent_b");
        assert_eq!(for_b.len(), 1);
        assert_eq!(for_b[0].content, "secret");
        assert!(!for_b[0].is_broadcast);

        let for_c = ctx_c.receive_messages("agent_c");
        assert!(for_c.is_empty());
    }

    #[test]
    fn test_cleanup_old_messages() {
        let ctx = make_ctx();

        ctx.send_message(AgentMessage::direct("a", "b", "old"));
        ctx.send_message(AgentMessage::direct("a", "b", "also_old"));

        // These messages are brand new, so cleanup with 0 max_age should remove them
        let removed = ctx.cleanup_old_messages(Duration::ZERO);
        assert_eq!(removed, 2);

        // Now the store should be empty
        let received = ctx.receive_messages("b");
        assert!(received.is_empty());
    }

    #[test]
    fn test_cleanup_old_messages_preserves_recent() {
        let ctx = make_ctx();

        ctx.send_message(AgentMessage::direct("a", "b", "fresh"));

        // 1 hour max age should preserve just-created messages
        let removed = ctx.cleanup_old_messages(Duration::from_secs(3600));
        assert_eq!(removed, 0);

        let received = ctx.receive_messages("b");
        assert_eq!(received.len(), 1);
    }

    #[test]
    fn test_cleanup_consumed() {
        let ctx = make_ctx();
        let ctx_b = ctx.with_shared_message_store();

        ctx.send_message(AgentMessage::direct("a", "b", "msg1"));
        ctx.send_message(AgentMessage::direct("a", "b", "msg2"));

        // Agent b receives messages, advancing its cursor
        let _ = ctx_b.receive_messages("b");

        // min_last_seen is the cursor of agent_b
        let min_last_seen = ctx_b.last_seen_message_id();
        let removed = ctx.cleanup_consumed(min_last_seen);
        assert_eq!(removed, 2);

        // Store should now be empty
        let remaining = ctx.receive_messages("b");
        assert!(remaining.is_empty());
    }

    #[test]
    fn test_context_default_fields() {
        let ctx = make_ctx();

        assert!(ctx.constraint_engine.is_none());
        assert_eq!(*ctx.system_state.read(), SystemOperatingState::Normal);
        assert_eq!(ctx.authority, AuthorityLevel::Observer);
        assert!(ctx.jurisdiction.contains_zone(1)); // unrestricted
        assert!(ctx.audit_trail.read().is_empty());
    }

    #[test]
    fn test_context_is_emergency_normal() {
        let ctx = make_ctx();

        assert!(!ctx.is_emergency());
    }

    #[test]
    fn test_context_is_emergency_emergency_state() {
        let ctx = AgentContext::new_with_authority(
            Arc::new(EventBus::new(64)),
            Arc::new(SafetyGateway::new(100)),
            Arc::new(RwLock::new(ToolEngine::new())),
            Arc::new(RwLock::new(PowerNetwork::from_ieee14())),
            Arc::new(InMemoryMemory::default()),
            Arc::new(RuleBasedEngine::new()),
            None,
            Arc::new(RwLock::new(SystemOperatingState::Emergency)),
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
        );

        assert!(ctx.is_emergency());
    }

    #[test]
    fn test_effective_authority_normal_state() {
        let ctx = AgentContext::new_with_authority(
            Arc::new(EventBus::new(64)),
            Arc::new(SafetyGateway::new(100)),
            Arc::new(RwLock::new(ToolEngine::new())),
            Arc::new(RwLock::new(PowerNetwork::from_ieee14())),
            Arc::new(InMemoryMemory::default()),
            Arc::new(RuleBasedEngine::new()),
            None,
            Arc::new(RwLock::new(SystemOperatingState::Normal)),
            AuthorityLevel::Emergency,
            Jurisdiction::unrestricted(),
        );

        // Emergency authority is downgraded to Supervisor when not in emergency
        assert_eq!(ctx.effective_authority(), AuthorityLevel::Supervisor);
    }

    #[test]
    fn test_effective_authority_emergency_state() {
        let ctx = AgentContext::new_with_authority(
            Arc::new(EventBus::new(64)),
            Arc::new(SafetyGateway::new(100)),
            Arc::new(RwLock::new(ToolEngine::new())),
            Arc::new(RwLock::new(PowerNetwork::from_ieee14())),
            Arc::new(InMemoryMemory::default()),
            Arc::new(RuleBasedEngine::new()),
            None,
            Arc::new(RwLock::new(SystemOperatingState::Emergency)),
            AuthorityLevel::Emergency,
            Jurisdiction::unrestricted(),
        );

        // Emergency authority stays Emergency when in emergency state
        assert_eq!(ctx.effective_authority(), AuthorityLevel::Emergency);
    }

    #[test]
    fn test_effective_authority_observer_unchanged() {
        let ctx = make_ctx();

        // Observer stays Observer regardless of system state
        assert_eq!(ctx.effective_authority(), AuthorityLevel::Observer);
    }

    #[test]
    fn test_record_audit() {
        let ctx = make_ctx();

        assert!(ctx.audit_trail.read().is_empty());

        let entry = AuditEntry {
            entry_id: 1,
            agent_id: "agent-1".to_string(),
            authority_level: AuthorityLevel::Operator,
            action_description: "Switch capacitor bank".to_string(),
            constraint_check_result: "passed".to_string(),
            approval_chain: vec![],
            timestamp: chrono::Utc::now(),
            reasoning_summary: "Voltage support needed".to_string(),
            system_state: SystemOperatingState::Normal,
            verdict: ActionVerdict::Approved,
        };

        ctx.record_audit(entry);

        let trail = ctx.audit_trail.read();
        assert_eq!(trail.len(), 1);
        assert_eq!(trail[0].agent_id, "agent-1");
        assert_eq!(trail[0].action_description, "Switch capacitor bank");
    }

    #[test]
    fn test_record_multiple_audit_entries() {
        let ctx = make_ctx();

        for i in 0..3 {
            let entry = AuditEntry {
                entry_id: i,
                agent_id: format!("agent-{}", i),
                authority_level: AuthorityLevel::Operator,
                action_description: format!("Action {}", i),
                constraint_check_result: "passed".to_string(),
                approval_chain: vec![],
                timestamp: chrono::Utc::now(),
                reasoning_summary: "test".to_string(),
                system_state: SystemOperatingState::Normal,
                verdict: ActionVerdict::Approved,
            };
            ctx.record_audit(entry);
        }

        let trail = ctx.audit_trail.read();
        assert_eq!(trail.len(), 3);
    }

    #[test]
    fn test_new_with_authority_custom_jurisdiction() {
        let ctx = AgentContext::new_with_authority(
            Arc::new(EventBus::new(64)),
            Arc::new(SafetyGateway::new(100)),
            Arc::new(RwLock::new(ToolEngine::new())),
            Arc::new(RwLock::new(PowerNetwork::from_ieee14())),
            Arc::new(InMemoryMemory::default()),
            Arc::new(RuleBasedEngine::new()),
            Some(Arc::new(ConstraintEngine::new())),
            Arc::new(RwLock::new(SystemOperatingState::Alert)),
            AuthorityLevel::Supervisor,
            Jurisdiction::for_zones(vec![1, 2]),
        );

        assert!(ctx.constraint_engine.is_some());
        assert_eq!(*ctx.system_state.read(), SystemOperatingState::Alert);
        assert_eq!(ctx.authority, AuthorityLevel::Supervisor);
        assert!(ctx.jurisdiction.contains_zone(1));
        assert!(!ctx.jurisdiction.contains_zone(99));
    }
}
