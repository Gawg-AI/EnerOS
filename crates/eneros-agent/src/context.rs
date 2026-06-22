use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use parking_lot::RwLock;
use tokio::sync::Mutex as TokioMutex;
use eneros_core::{
    AgentMessage, AuthorityLevel, EventBusPublisher, Event, EventPayload,
    GatewayClient, Jurisdiction, SystemOperatingState, AuditEntry,
};
use eneros_constraint::ConstraintEngine;
use eneros_eventbus::{EventBus, LocalEventBusPublisher};
use eneros_gateway::{LocalGatewayClient, SafetyGateway};
use eneros_tool::ToolEngine;
use eneros_network::PowerNetwork;
use eneros_memory::AgentMemory;
use eneros_reasoning::ReasoningEngine;

/// Global sequence counter for message IDs, shared across all AgentContexts
/// that use the same message store.
static MESSAGE_SEQ_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Shared message store that supports cursor-based delivery so that
/// multiple agents can independently read the same messages.
///
/// Kept for backward compatibility with in-process (local) mode tests.
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

/// Local state for an agent (no shared `Arc<T>` references to remote services).
///
/// This struct is `Clone` so it can be cheaply copied when spawning agent
/// processes or creating derived contexts.
#[derive(Clone)]
pub struct LocalContext {
    /// Agent's unique identifier.
    pub agent_id: String,
    /// Agent's authority level
    pub authority: AuthorityLevel,
    /// Agent's jurisdiction
    pub jurisdiction: Jurisdiction,
    /// Tick interval for the agent's perceive-act loop
    pub tick_interval: Duration,
    /// Cursor: last message seq this agent has seen
    pub last_seen_message_id: Arc<RwLock<u64>>,
    /// 分布式追踪 ID（T029-06）。
    ///
    /// 贯穿 API 请求 → Agent 调度 → 插件执行 → 任务完成的全链路。
    /// 默认生成 UUID v4；API handler 会从请求扩展中取出 trace_id 注入此处，
    /// 后续 AgentOrchestrator / ActionDispatcher / SpawnedAgent 都会读取该字段，
    /// 并通过 `tracing::Span` 携带，使所有日志自动包含 trace_id。
    pub trace_id: String,
}

/// Remote service handles (replaces direct `Arc<T>` with trait objects).
///
/// In local (in-process) mode, the handles wrap in-process implementations
/// (`LocalEventBusPublisher`, `LocalGatewayClient`). In remote (process) mode,
/// they wrap IPC clients (`RemoteEventBusPublisher`, `RemoteGatewayClient`).
pub struct RemoteHandles {
    /// Event bus publisher (LocalEventBusPublisher or RemoteEventBusPublisher)
    pub event_bus: Arc<dyn EventBusPublisher>,

    /// Gateway client (LocalGatewayClient or RemoteGatewayClient)
    pub gateway_client: Arc<dyn GatewayClient>,

    /// Event receiver for subscribed events (None if not subscribed).
    /// In local mode this is typically None (messages go through MessageStore).
    pub event_receiver: Arc<TokioMutex<Option<tokio::sync::mpsc::Receiver<Event>>>>,

    /// In-process message store for local mode (None in remote mode).
    /// When Some, `send_message`/`receive_messages` use cursor-based delivery.
    pub message_store: Option<Arc<MessageStore>>,

    /// Tool engine (local to agent process)
    pub tool_engine: Option<Arc<tokio::sync::RwLock<ToolEngine>>>,

    /// Network snapshot (read-only copy)
    pub network: Arc<RwLock<PowerNetwork>>,

    /// Agent memory (local to agent process)
    pub memory: Option<Arc<dyn AgentMemory>>,

    /// Reasoning engine (local to agent process)
    pub reasoning: Option<Arc<dyn ReasoningEngine>>,

    /// Constraint engine (local to agent process)
    pub constraint_engine: Option<Arc<ConstraintEngine>>,

    /// System operating state (local copy, updated by events)
    pub system_state: Arc<RwLock<SystemOperatingState>>,

    /// Audit trail (local to agent process)
    pub audit_trail: Arc<RwLock<Vec<AuditEntry>>>,
}

/// Agent context: local state + remote handles.
///
/// Replaces the old `AgentContext` with 13 `Arc<T>` fields. The context is
/// split into `local` (cheaply cloneable state) and `remote` (service handles).
pub struct AgentContext {
    pub local: LocalContext,
    pub remote: RemoteHandles,
}

impl AgentContext {
    /// Create a new AgentContext for in-process use (tests, legacy mode).
    ///
    /// Wraps `EventBus` and `SafetyGateway` in their local publisher/client
    /// implementations. A shared `MessageStore` is created for cursor-based
    /// inter-agent messaging.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        event_bus: Arc<EventBus>,
        gateway: Arc<SafetyGateway>,
        _tool_engine: Arc<RwLock<ToolEngine>>,
        network: Arc<RwLock<PowerNetwork>>,
        memory: Arc<dyn AgentMemory>,
        reasoning: Arc<dyn ReasoningEngine>,
    ) -> Self {
        Self::new_local(
            "default-agent",
            event_bus,
            gateway,
            Some(Arc::new(tokio::sync::RwLock::new(ToolEngine::new()))),
            network,
            Some(memory),
            Some(reasoning),
            None,
            Arc::new(RwLock::new(SystemOperatingState::Normal)),
            AuthorityLevel::Observer,
            Jurisdiction::unrestricted(),
        )
    }

    /// Create a new AgentContext with full configuration including authority and jurisdiction.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_authority(
        event_bus: Arc<EventBus>,
        gateway: Arc<SafetyGateway>,
        _tool_engine: Arc<RwLock<ToolEngine>>,
        network: Arc<RwLock<PowerNetwork>>,
        memory: Arc<dyn AgentMemory>,
        reasoning: Arc<dyn ReasoningEngine>,
        constraint_engine: Option<Arc<ConstraintEngine>>,
        system_state: Arc<RwLock<SystemOperatingState>>,
        authority: AuthorityLevel,
        jurisdiction: Jurisdiction,
    ) -> Self {
        Self::new_local(
            "default-agent",
            event_bus,
            gateway,
            Some(Arc::new(tokio::sync::RwLock::new(ToolEngine::new()))),
            network,
            Some(memory),
            Some(reasoning),
            constraint_engine,
            system_state,
            authority,
            jurisdiction,
        )
    }

    /// Build an AgentContext for in-process use.
    ///
    /// All service handles are wrapped in their local implementations.
    /// A shared `MessageStore` is created for cursor-based messaging.
    #[allow(clippy::too_many_arguments)]
    pub fn new_local(
        agent_id: &str,
        event_bus: Arc<EventBus>,
        gateway: Arc<SafetyGateway>,
        tool_engine: Option<Arc<tokio::sync::RwLock<ToolEngine>>>,
        network: Arc<RwLock<PowerNetwork>>,
        memory: Option<Arc<dyn AgentMemory>>,
        reasoning: Option<Arc<dyn ReasoningEngine>>,
        constraint_engine: Option<Arc<ConstraintEngine>>,
        system_state: Arc<RwLock<SystemOperatingState>>,
        authority: AuthorityLevel,
        jurisdiction: Jurisdiction,
    ) -> Self {
        let publisher: Arc<dyn EventBusPublisher> =
            Arc::new(LocalEventBusPublisher::new(event_bus));
        let gateway_client: Arc<dyn GatewayClient> =
            Arc::new(LocalGatewayClient::new(gateway));

        Self {
            local: LocalContext {
                agent_id: agent_id.to_string(),
                authority,
                jurisdiction,
                tick_interval: Duration::from_secs(1),
                last_seen_message_id: Arc::new(RwLock::new(0)),
                trace_id: uuid::Uuid::new_v4().to_string(),
            },
            remote: RemoteHandles {
                event_bus: publisher,
                gateway_client,
                event_receiver: Arc::new(TokioMutex::new(None)),
                message_store: Some(Arc::new(MessageStore::new())),
                tool_engine,
                network,
                memory,
                reasoning,
                constraint_engine,
                system_state,
                audit_trail: Arc::new(RwLock::new(Vec::new())),
            },
        }
    }

    /// Create a new AgentContext that shares the same message store as an existing context.
    /// This is the key method for multi-agent collaboration: all agents sharing the
    /// same store can independently receive broadcast messages.
    ///
    /// trace_id 默认继承自父上下文（T029-06），保证同一请求链路下的所有
    /// 衍生 Agent 上下文都携带相同的 trace_id。
    pub fn with_shared_message_store(&self) -> Self {
        Self {
            local: LocalContext {
                agent_id: self.local.agent_id.clone(),
                authority: self.local.authority,
                jurisdiction: self.local.jurisdiction.clone(),
                tick_interval: self.local.tick_interval,
                last_seen_message_id: Arc::new(RwLock::new(0)),
                trace_id: self.local.trace_id.clone(),
            },
            remote: RemoteHandles {
                event_bus: Arc::clone(&self.remote.event_bus),
                gateway_client: Arc::clone(&self.remote.gateway_client),
                event_receiver: Arc::new(TokioMutex::new(None)),
                message_store: self.remote.message_store.clone(),
                tool_engine: self.remote.tool_engine.clone(),
                network: Arc::clone(&self.remote.network),
                memory: self.remote.memory.clone(),
                reasoning: self.remote.reasoning.clone(),
                constraint_engine: self.remote.constraint_engine.clone(),
                system_state: Arc::clone(&self.remote.system_state),
                audit_trail: Arc::new(RwLock::new(Vec::new())),
            },
        }
    }

    /// 返回当前上下文携带的 trace_id（T029-06）。
    ///
    /// trace_id 在 `LocalContext::new_local` 中默认生成 UUID v4，
    /// 也可通过 [`with_trace_id`](Self::with_trace_id) 覆盖。
    /// AgentOrchestrator / ActionDispatcher / SpawnedAgent 都会读取该字段，
    /// 并通过 `tracing::Span` 携带，使所有日志自动包含 trace_id。
    pub fn trace_id(&self) -> &str {
        &self.local.trace_id
    }

    /// 返回一个新的 `AgentContext`，其 trace_id 被替换为 `trace_id`（T029-06）。
    ///
    /// 用于 API handler 从请求扩展中取出 trace_id 后注入到 Agent 执行上下文。
    /// 其它字段（agent_id、authority、jurisdiction、remote handles 等）保持不变。
    /// 调用方通常会在每次 API 请求中调用一次本方法，得到一个请求作用域的上下文。
    pub fn with_trace_id(&self, trace_id: impl Into<String>) -> Self {
        let mut new_ctx = self.with_shared_message_store();
        new_ctx.local.trace_id = trace_id.into();
        new_ctx
    }

    /// Send a message to the shared message store (local mode) or via the event
    /// bus publisher (remote mode).
    ///
    /// In local mode the message is assigned a globally unique sequence number
    /// and stored for cursor-based delivery. In remote mode the message is
    /// converted to an `Event` and published to the EventBusBroker.
    pub fn send_message(&self, message: AgentMessage) {
        if let Some(ref store) = self.remote.message_store {
            store.push(message);
        } else {
            // Remote mode: fire-and-forget via the publisher.
            // Errors are logged but not propagated (send_message is sync).
            let publisher = Arc::clone(&self.remote.event_bus);
            tokio::spawn(async move {
                if let Err(e) = publisher.send_direct_message(message).await {
                    tracing::warn!("send_direct_message failed: {}", e);
                }
            });
        }
    }

    /// Receive all new messages addressed to the given agent_id since the last call.
    ///
    /// In local mode, messages are read from the shared `MessageStore` using
    /// cursor-based delivery. In remote mode, this drains pending events from
    /// the event receiver channel and filters for `AgentMessage` payloads.
    pub fn receive_messages(&self, agent_id: &str) -> Vec<AgentMessage> {
        if let Some(ref store) = self.remote.message_store {
            let since = *self.local.last_seen_message_id.read();
            let messages = store.messages_since(agent_id, since);
            if let Some(max_seq) = messages.iter().map(|m| m.seq).max() {
                *self.local.last_seen_message_id.write() = max_seq;
            }
            messages
        } else {
            // Remote mode: drain pending events from the receiver.
            // Uses try_recv in a loop (non-blocking).
            let mut result = Vec::new();
            if let Ok(mut guard) = self.remote.event_receiver.try_lock() {
                if let Some(ref mut rx) = *guard {
                    while let Ok(event) = rx.try_recv() {
                        if let EventPayload::AgentMessage(msg) = event.payload {
                            if msg.is_for(agent_id) {
                                result.push(msg);
                            }
                        }
                    }
                }
            }
            result
        }
    }

    /// Broadcast a message to all agents
    pub fn broadcast_message(&self, sender_id: &str, content: &str) {
        let msg = AgentMessage::broadcast(sender_id, content);
        self.send_message(msg);
    }

    /// Get this agent's last seen message id (useful for coordinated cleanup).
    pub fn last_seen_message_id(&self) -> u64 {
        *self.local.last_seen_message_id.read()
    }

    /// Check if the system is in emergency state
    pub fn is_emergency(&self) -> bool {
        self.remote.system_state.read().is_emergency()
    }

    /// Get effective authority level considering system state
    pub fn effective_authority(&self) -> AuthorityLevel {
        self.local.authority.effective_level(self.is_emergency())
    }

    /// Record an audit entry
    pub fn record_audit(&self, entry: AuditEntry) {
        self.remote.audit_trail.write().push(entry);
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

        // Verify the context can be dereferenced via remote handles
        let _bus = &ctx.remote.event_bus;
        let _gw = &ctx.remote.gateway_client;
        let _te = &ctx.remote.tool_engine;
        let _net = &ctx.remote.network;
        let _mem = &ctx.remote.memory;
        let _re = &ctx.remote.reasoning;
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
    fn test_context_default_fields() {
        let ctx = make_ctx();

        assert!(ctx.remote.constraint_engine.is_none());
        assert_eq!(*ctx.remote.system_state.read(), SystemOperatingState::Normal);
        assert_eq!(ctx.local.authority, AuthorityLevel::Observer);
        assert!(ctx.local.jurisdiction.contains_zone(1)); // unrestricted
        assert!(ctx.remote.audit_trail.read().is_empty());
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

        assert!(ctx.remote.audit_trail.read().is_empty());

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

        let trail = ctx.remote.audit_trail.read();
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

        let trail = ctx.remote.audit_trail.read();
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

        assert!(ctx.remote.constraint_engine.is_some());
        assert_eq!(*ctx.remote.system_state.read(), SystemOperatingState::Alert);
        assert_eq!(ctx.local.authority, AuthorityLevel::Supervisor);
        assert!(ctx.local.jurisdiction.contains_zone(1));
        assert!(!ctx.local.jurisdiction.contains_zone(99));
    }

    // === T029-06: 分布式追踪 trace_id 贯穿 Agent 管线 ===

    /// 验证 `LocalContext` 在构造时默认生成一个非空的 UUID v4 trace_id。
    #[test]
    fn test_trace_id_default_is_nonempty_uuid() {
        let ctx = make_ctx();

        // trace_id 应为非空字符串
        let trace_id = ctx.trace_id();
        assert!(!trace_id.is_empty());

        // 应为合法的 UUID v4 格式（36 字符，含 4 个连字符）
        assert_eq!(trace_id.len(), 36);
        assert_eq!(trace_id.matches('-').count(), 4);
        // UUID v4 的版本位应为 '4'
        let bytes: Vec<char> = trace_id.chars().collect();
        assert_eq!(bytes[14], '4');
    }

    /// 验证 `trace_id()` 访问器返回 `LocalContext.trace_id` 字段。
    #[test]
    fn test_trace_id_accessor_returns_local_field() {
        let ctx = make_ctx();
        assert_eq!(ctx.trace_id(), ctx.local.trace_id);
    }

    /// 验证 `with_trace_id()` 返回一个新的 `AgentContext`，其 trace_id 被替换为
    /// 调用方提供的值，其余字段（agent_id、authority、jurisdiction 等）保持不变。
    #[test]
    fn test_with_trace_id_replaces_trace_id() {
        let ctx = make_ctx();
        let original_trace_id = ctx.trace_id().to_string();

        let new_trace_id = "test-trace-id-12345678-abcd-ef01-2345-678901234567";
        let new_ctx = ctx.with_trace_id(new_trace_id);

        // 新上下文的 trace_id 应为调用方提供的值
        assert_eq!(new_ctx.trace_id(), new_trace_id);
        // 原上下文的 trace_id 应保持不变（不可变借用）
        assert_eq!(ctx.trace_id(), original_trace_id);
        assert_ne!(new_ctx.trace_id(), ctx.trace_id());
    }

    /// 验证 `with_trace_id()` 保留其它字段（agent_id、authority、jurisdiction、
    /// tick_interval、共享消息存储等）。
    #[test]
    fn test_with_trace_id_preserves_other_fields() {
        let ctx = AgentContext::new_with_authority(
            Arc::new(EventBus::new(64)),
            Arc::new(SafetyGateway::new(100)),
            Arc::new(RwLock::new(ToolEngine::new())),
            Arc::new(RwLock::new(PowerNetwork::from_ieee14())),
            Arc::new(InMemoryMemory::default()),
            Arc::new(RuleBasedEngine::new()),
            Some(Arc::new(ConstraintEngine::new())),
            Arc::new(RwLock::new(SystemOperatingState::Normal)),
            AuthorityLevel::Operator,
            Jurisdiction::for_zones(vec![1, 2, 3]),
        );

        let new_ctx = ctx.with_trace_id("custom-trace-id");

        // agent_id、authority、jurisdiction 应保持不变
        assert_eq!(new_ctx.local.agent_id, ctx.local.agent_id);
        assert_eq!(new_ctx.local.authority, ctx.local.authority);
        assert_eq!(new_ctx.local.jurisdiction, ctx.local.jurisdiction);
        assert_eq!(new_ctx.local.tick_interval, ctx.local.tick_interval);

        // 共享消息存储应保持不变（同一 Arc 指针）
        assert!(new_ctx.remote.message_store.is_some());
        // 通过发送消息验证两个上下文共享同一个消息存储
        ctx.send_message(AgentMessage::direct("a", "b", "shared"));
        let received = new_ctx.receive_messages("b");
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].content, "shared");
    }

    /// 验证 `with_shared_message_store()` 继承父上下文的 trace_id（T029-06）。
    /// 这是 trace_id 在衍生上下文中自动传播的关键机制。
    #[test]
    fn test_with_shared_message_store_inherits_trace_id() {
        let ctx = make_ctx();
        let custom_trace_id = "inherited-trace-id-abc";
        let ctx_with_trace = ctx.with_trace_id(custom_trace_id);

        // 通过 with_shared_message_store 派生的上下文应继承 trace_id
        let derived_ctx = ctx_with_trace.with_shared_message_store();
        assert_eq!(derived_ctx.trace_id(), custom_trace_id);

        // 原始 ctx 的 trace_id 应保持默认值（未被修改）
        assert_ne!(ctx.trace_id(), custom_trace_id);
    }

    /// 验证两个独立构造的 `AgentContext` 拥有不同的 trace_id（UUID v4 唯一性）。
    #[test]
    fn test_trace_id_uniqueness_across_contexts() {
        let ctx1 = make_ctx();
        let ctx2 = make_ctx();

        // 两次构造应生成不同的 trace_id
        assert_ne!(ctx1.trace_id(), ctx2.trace_id());
    }

    /// 验证 `with_trace_id()` 接受 `&str`、`String`、`&String` 等多种参数类型
    /// （`impl Into<String>` 泛型约束）。
    #[test]
    fn test_with_trace_id_accepts_multiple_string_types() {
        let ctx = make_ctx();

        // &str
        let ctx1 = ctx.with_trace_id("str-literal");
        assert_eq!(ctx1.trace_id(), "str-literal");

        // String
        let ctx2 = ctx.with_trace_id(String::from("owned-string"));
        assert_eq!(ctx2.trace_id(), "owned-string");
    }
}
