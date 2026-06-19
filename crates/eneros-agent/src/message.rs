//! Agent message types re-exported from eneros-core.
//!
//! The canonical definitions live in `eneros_core::agent_message` so they can
//! be shared as IPC schema across processes. This module re-exports them for
//! backward compatibility with existing agent code.

pub use eneros_core::agent_message::{AgentMessage, MessagePriority};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_direct_message_fields() {
        let msg = AgentMessage::direct("agent_a", "agent_b", "hello");
        assert_eq!(msg.sender_id, "agent_a");
        assert_eq!(msg.target_id, Some("agent_b".to_string()));
        assert_eq!(msg.content, "hello");
        assert_eq!(msg.priority, MessagePriority::Normal);
        assert!(!msg.id.is_empty());
        assert!(!msg.is_broadcast);
    }

    #[test]
    fn test_broadcast_message_target_is_none() {
        let msg = AgentMessage::broadcast("agent_a", "announcement");
        assert_eq!(msg.sender_id, "agent_a");
        assert_eq!(msg.target_id, None);
        assert_eq!(msg.content, "announcement");
        assert!(msg.is_broadcast);
    }

    #[test]
    fn test_is_for_direct_and_broadcast() {
        let direct = AgentMessage::direct("a", "b", "hi");
        assert!(direct.is_for("b"));
        assert!(!direct.is_for("c"));

        let broadcast = AgentMessage::broadcast("a", "hi");
        assert!(broadcast.is_for("b"));
        assert!(broadcast.is_for("c"));
    }

    #[test]
    fn test_with_priority() {
        let msg = AgentMessage::direct("a", "b", "urgent").with_priority(MessagePriority::Critical);
        assert_eq!(msg.priority, MessagePriority::Critical);
    }
}
