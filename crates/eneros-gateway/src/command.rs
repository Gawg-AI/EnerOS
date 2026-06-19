//! Command types re-exported from eneros-core.
//!
//! The canonical definitions live in `eneros_core::command` so they can be
//! shared as IPC schema across processes. This module re-exports them for
//! backward compatibility with existing gateway code.

pub use eneros_core::{Command, CommandPriority, CommandType, DeviceValue};

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
