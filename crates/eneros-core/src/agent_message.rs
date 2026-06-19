use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Message priority
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub enum MessagePriority {
    Low,
    #[default]
    Normal,
    High,
    Critical,
}

/// A message sent between agents
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    /// Message ID (UUID)
    pub id: String,
    /// Monotonically increasing sequence number for cursor-based delivery
    pub seq: u64,
    /// Sender agent ID
    pub sender_id: String,
    /// Target agent ID (None for broadcast)
    pub target_id: Option<String>,
    /// Message content
    pub content: String,
    /// Message priority
    pub priority: MessagePriority,
    /// Whether this is a broadcast message
    pub is_broadcast: bool,
    /// Timestamp
    pub timestamp: DateTime<Utc>,
}

impl AgentMessage {
    /// Create a new direct message (seq will be assigned by send_message)
    pub fn direct(sender_id: &str, target_id: &str, content: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            seq: 0,
            sender_id: sender_id.to_string(),
            target_id: Some(target_id.to_string()),
            content: content.to_string(),
            priority: MessagePriority::default(),
            is_broadcast: false,
            timestamp: Utc::now(),
        }
    }

    /// Create a new broadcast message (seq will be assigned by send_message)
    pub fn broadcast(sender_id: &str, content: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            seq: 0,
            sender_id: sender_id.to_string(),
            target_id: None,
            content: content.to_string(),
            priority: MessagePriority::default(),
            is_broadcast: true,
            timestamp: Utc::now(),
        }
    }

    /// Set message priority
    pub fn with_priority(mut self, priority: MessagePriority) -> Self {
        self.priority = priority;
        self
    }

    /// Check if this message is for a specific agent
    pub fn is_for(&self, agent_id: &str) -> bool {
        self.is_broadcast || self.target_id.as_ref().is_some_and(|t| t == agent_id)
    }
}
