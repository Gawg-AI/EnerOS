use eneros_core::ElementId;
use crate::agent::AgentAction;
use serde::{Deserialize, Serialize};

/// Power domain collaboration protocol
/// Defines domain-specific collaboration patterns between power system agents
pub trait PowerCollaborationProtocol {
    /// Check if a device is available for dispatch (ask OperationAgent)
    fn check_device_availability(&self, device_id: ElementId) -> DeviceAvailability;

    /// Coordinate emergency response between self-healing and dispatch
    fn coordinate_emergency(&self, fault_bus: ElementId, load_change_mw: f64) -> Vec<AgentAction>;

    /// Negotiate cross-zone power transfer
    fn negotiate_cross_zone(&self, from_zone: u32, to_zone: u32, amount_mw: f64) -> CrossZoneResult;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceAvailability {
    Available,
    UnderMaintenance,
    Faulty,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossZoneResult {
    pub approved: bool,
    pub from_zone: u32,
    pub to_zone: u32,
    pub amount_mw: f64,
    pub reason: String,
}

/// Default implementation of power collaboration protocol
pub struct DefaultPowerCollaboration {
    /// Devices under maintenance
    pub maintenance_devices: std::collections::HashSet<ElementId>,
    /// Faulty devices
    pub faulty_devices: std::collections::HashSet<ElementId>,
}

impl DefaultPowerCollaboration {
    pub fn new() -> Self {
        Self {
            maintenance_devices: std::collections::HashSet::new(),
            faulty_devices: std::collections::HashSet::new(),
        }
    }

    /// Mark a device as under maintenance
    pub fn set_maintenance(&mut self, device_id: ElementId) {
        self.maintenance_devices.insert(device_id);
    }

    /// Mark a device as faulty
    pub fn set_faulty(&mut self, device_id: ElementId) {
        self.faulty_devices.insert(device_id);
    }

    /// Clear device status
    pub fn clear_device_status(&mut self, device_id: ElementId) {
        self.maintenance_devices.remove(&device_id);
        self.faulty_devices.remove(&device_id);
    }
}

impl Default for DefaultPowerCollaboration {
    fn default() -> Self {
        Self::new()
    }
}

impl PowerCollaborationProtocol for DefaultPowerCollaboration {
    fn check_device_availability(&self, device_id: ElementId) -> DeviceAvailability {
        if self.faulty_devices.contains(&device_id) {
            DeviceAvailability::Faulty
        } else if self.maintenance_devices.contains(&device_id) {
            DeviceAvailability::UnderMaintenance
        } else {
            DeviceAvailability::Available
        }
    }

    fn coordinate_emergency(&self, fault_bus: ElementId, load_change_mw: f64) -> Vec<AgentAction> {
        let mut actions = Vec::new();

        // Notify dispatch to adjust generation
        actions.push(AgentAction::DelegateTask {
            target_agent_id: "dispatch".to_string(),
            task_description: format!(
                "紧急联动：故障母线 {}，负荷变化 {:.1}MW，请调整发电出力",
                fault_bus, load_change_mw
            ),
        });

        // Notify operation to check affected devices
        actions.push(AgentAction::DelegateTask {
            target_agent_id: "operation".to_string(),
            task_description: format!(
                "紧急联动：故障母线 {}，请检查相关设备状态",
                fault_bus
            ),
        });

        actions
    }

    fn negotiate_cross_zone(&self, from_zone: u32, to_zone: u32, amount_mw: f64) -> CrossZoneResult {
        // Simplified: always approve if zones are different and amount is reasonable
        let approved = from_zone != to_zone && amount_mw > 0.0 && amount_mw <= 500.0;
        CrossZoneResult {
            approved,
            from_zone,
            to_zone,
            amount_mw,
            reason: if approved {
                format!("跨区功率交换 {:.1}MW 从区域{}到区域{}已批准", amount_mw, from_zone, to_zone)
            } else {
                "跨区功率交换被拒绝：区域相同或功率超限".to_string()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_availability_available() {
        let protocol = DefaultPowerCollaboration::new();
        assert_eq!(protocol.check_device_availability(1), DeviceAvailability::Available);
    }

    #[test]
    fn test_device_availability_maintenance() {
        let mut protocol = DefaultPowerCollaboration::new();
        protocol.set_maintenance(1);
        assert_eq!(protocol.check_device_availability(1), DeviceAvailability::UnderMaintenance);
    }

    #[test]
    fn test_device_availability_faulty() {
        let mut protocol = DefaultPowerCollaboration::new();
        protocol.set_faulty(1);
        assert_eq!(protocol.check_device_availability(1), DeviceAvailability::Faulty);
    }

    #[test]
    fn test_clear_device_status() {
        let mut protocol = DefaultPowerCollaboration::new();
        protocol.set_maintenance(1);
        protocol.clear_device_status(1);
        assert_eq!(protocol.check_device_availability(1), DeviceAvailability::Available);
    }

    #[test]
    fn test_coordinate_emergency() {
        let protocol = DefaultPowerCollaboration::new();
        let actions = protocol.coordinate_emergency(5, -50.0);
        assert_eq!(actions.len(), 2); // Notify dispatch + operation
    }

    #[test]
    fn test_negotiate_cross_zone_approved() {
        let protocol = DefaultPowerCollaboration::new();
        let result = protocol.negotiate_cross_zone(1, 2, 100.0);
        assert!(result.approved);
    }

    #[test]
    fn test_negotiate_cross_zone_rejected_same_zone() {
        let protocol = DefaultPowerCollaboration::new();
        let result = protocol.negotiate_cross_zone(1, 1, 100.0);
        assert!(!result.approved);
    }

    #[test]
    fn test_negotiate_cross_zone_rejected_over_limit() {
        let protocol = DefaultPowerCollaboration::new();
        let result = protocol.negotiate_cross_zone(1, 2, 600.0);
        assert!(!result.approved);
    }

    #[test]
    fn test_default_collaboration() {
        let protocol = DefaultPowerCollaboration::default();
        assert!(protocol.maintenance_devices.is_empty());
        assert!(protocol.faulty_devices.is_empty());
    }
}
