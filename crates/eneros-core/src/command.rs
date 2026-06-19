use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::{ElementId, TopologyChange};

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
    /// Toggle a switch open/close
    SwitchToggle,
    /// Toggle a branch in/out of service
    BranchToggle,
}

/// Command priority
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CommandPriority {
    Low,
    Normal,
    High,
    Critical,
}

/// Device value mirror of `eneros_device::adapter::DataValue`.
///
/// Defined in eneros-core so that `Command` can carry a device value without
/// depending on eneros-device (which would create a circular dependency).
/// `eneros_device::adapter::DataValue` implements `From<DeviceValue>` for
/// lossless conversion at the gateway/device boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceValue {
    Bool(bool),
    Int16(i16),
    Int32(i32),
    Int64(i64),
    Float32(f32),
    Float64(f64),
    String(String),
    Bytes(Vec<u8>),
}

/// Command structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    /// Unique command ID
    pub id: String,
    /// Command type
    pub command_type: CommandType,
    /// Target element ID (e.g., switch_id, gen_id, zone_id)
    pub target_id: ElementId,
    /// Command parameters
    pub parameters: std::collections::HashMap<String, f64>,
    /// Command priority
    pub priority: CommandPriority,
    /// Command timestamp
    pub timestamp: DateTime<Utc>,
    /// Command source
    pub source: String,
    /// Target device ID for real execution (e.g., "rtu-1", "ied-bay3")
    /// When set, the command will be dispatched to this device via ProtocolAdapter::write()
    #[serde(default)]
    pub device_id: Option<String>,
    /// Device address for write operation (e.g., "holding:40001", "LD0/GGIO1.Pos.stVal")
    /// Interpreted by the protocol adapter of the target device
    #[serde(default)]
    pub device_address: Option<String>,
    /// Value to write to the device, derived from command_type and parameters
    #[serde(skip)]
    pub device_value: Option<DeviceValue>,
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
            device_id: None,
            device_address: None,
            device_value: None,
        }
    }

    /// Add a parameter to the command
    pub fn with_parameter(mut self, key: &str, value: f64) -> Self {
        self.parameters.insert(key.to_string(), value);
        self
    }

    /// Set the target device for real execution
    pub fn with_device(mut self, device_id: &str, address: &str, value: DeviceValue) -> Self {
        self.device_id = Some(device_id.to_string());
        self.device_address = Some(address.to_string());
        self.device_value = Some(value);
        self
    }

    /// Whether this command has device routing information
    pub fn has_device_target(&self) -> bool {
        self.device_id.is_some() && self.device_address.is_some() && self.device_value.is_some()
    }

    /// Convert command to a topology change, if applicable
    pub fn to_topology_change(&self) -> Option<TopologyChange> {
        match self.command_type {
            CommandType::SwitchToggle => {
                let closed = self.parameters.get("closed").is_some_and(|&v| v != 0.0);
                Some(TopologyChange::SwitchToggle {
                    switch_id: self.target_id,
                    closed,
                })
            }
            CommandType::BranchToggle => {
                let in_service = self.parameters.get("in_service").is_none_or(|&v| v != 0.0);
                if !in_service {
                    Some(TopologyChange::BranchRemoved {
                        branch_id: self.target_id,
                    })
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}
