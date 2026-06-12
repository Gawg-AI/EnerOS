use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use eneros_core::ElementId;

/// Command type classification
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandType {
    /// Switch operation
    SwitchOperation,
    /// Generator setpoint change
    GeneratorSetpoint,
    /// Transformer tap change
    TransformerTap,
    /// Capacitor switching
    CapacitorSwitch,
    /// Load shedding
    LoadShedding,
    /// System separation
    SystemSeparation,
}

/// Command priority
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CommandPriority {
    Low,
    Normal,
    High,
    Critical,
}

/// Command structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    /// Unique command ID
    pub id: String,
    /// Command type
    pub command_type: CommandType,
    /// Target element ID
    pub target_id: ElementId,
    /// Command parameters
    pub parameters: std::collections::HashMap<String, f64>,
    /// Command priority
    pub priority: CommandPriority,
    /// Command timestamp
    pub timestamp: DateTime<Utc>,
    /// Command source
    pub source: String,
}

impl Command {
    /// Create a new command
    pub fn new(
        command_type: CommandType,
        target_id: ElementId,
        priority: CommandPriority,
        source: &str,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            command_type,
            target_id,
            parameters: std::collections::HashMap::new(),
            priority,
            timestamp: Utc::now(),
            source: source.to_string(),
        }
    }

    /// Add a parameter to the command
    pub fn with_parameter(mut self, key: &str, value: f64) -> Self {
        self.parameters.insert(key.to_string(), value);
        self
    }
}
