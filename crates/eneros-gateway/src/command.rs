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
    pub device_value: Option<eneros_device::adapter::DataValue>,
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
    pub fn with_device(mut self, device_id: &str, address: &str, value: eneros_device::adapter::DataValue) -> Self {
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
    pub fn to_topology_change(&self) -> Option<eneros_core::TopologyChange> {
        match self.command_type {
            CommandType::SwitchToggle => {
                let closed = self.parameters.get("closed").is_some_and(|&v| v != 0.0);
                Some(eneros_core::TopologyChange::SwitchToggle {
                    switch_id: self.target_id,
                    closed,
                })
            }
            CommandType::BranchToggle => {
                let in_service = self.parameters.get("in_service").is_none_or(|&v| v != 0.0);
                if !in_service {
                    Some(eneros_core::TopologyChange::BranchRemoved {
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

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_core::TopologyChange;

    #[test]
    fn test_command_switch_toggle_to_topology_change() {
        let cmd = Command::new(CommandType::SwitchToggle, 42, CommandPriority::Normal, "test")
            .with_parameter("closed", 1.0);
        let tc = cmd.to_topology_change();
        assert!(tc.is_some());
        assert_eq!(
            tc.unwrap(),
            TopologyChange::SwitchToggle {
                switch_id: 42,
                closed: true
            }
        );

        let cmd_open = Command::new(CommandType::SwitchToggle, 42, CommandPriority::Normal, "test")
            .with_parameter("closed", 0.0);
        let tc_open = cmd_open.to_topology_change();
        assert!(tc_open.is_some());
        assert_eq!(
            tc_open.unwrap(),
            TopologyChange::SwitchToggle {
                switch_id: 42,
                closed: false
            }
        );
    }

    #[test]
    fn test_command_branch_toggle_to_topology_change() {
        let cmd = Command::new(CommandType::BranchToggle, 7, CommandPriority::Normal, "test")
            .with_parameter("in_service", 0.0);
        let tc = cmd.to_topology_change();
        assert!(tc.is_some());
        assert_eq!(
            tc.unwrap(),
            TopologyChange::BranchRemoved { branch_id: 7 }
        );

        let cmd_in = Command::new(CommandType::BranchToggle, 7, CommandPriority::Normal, "test")
            .with_parameter("in_service", 1.0);
        assert!(cmd_in.to_topology_change().is_none());
    }

    #[test]
    fn test_command_other_type_no_topology_change() {
        let cmd = Command::new(CommandType::GeneratorSetpoint, 1, CommandPriority::Normal, "test");
        assert!(cmd.to_topology_change().is_none());

        let cmd2 = Command::new(CommandType::TransformerTap, 2, CommandPriority::Normal, "test");
        assert!(cmd2.to_topology_change().is_none());
    }
}
