use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::types::{ElementId, ZoneId, SeverityLevel};

/// Agent authority level — controls what actions an agent can perform
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AuthorityLevel {
    /// Read-only, cannot execute any control commands
    Observer,
    /// Can execute routine operations (switching, parameter adjustment)
    Operator,
    /// Can execute high-risk operations (load shedding, system separation), requires approval
    Supervisor,
    /// Emergency override, can bypass non-critical safety checks, only active in emergency state
    Emergency,
}

impl AuthorityLevel {
    /// Check if this authority level can execute control commands
    pub fn can_execute_commands(&self) -> bool {
        matches!(self, AuthorityLevel::Operator | AuthorityLevel::Supervisor | AuthorityLevel::Emergency)
    }

    /// Check if this authority level can execute high-risk operations
    pub fn can_execute_high_risk(&self) -> bool {
        matches!(self, AuthorityLevel::Supervisor | AuthorityLevel::Emergency)
    }

    /// Check if this authority level can bypass non-critical safety checks
    pub fn can_bypass_checks(&self) -> bool {
        matches!(self, AuthorityLevel::Emergency)
    }

    /// Effective authority level considering system operating state
    /// Emergency authority is only active when system is in Emergency/Blackout state
    pub fn effective_level(&self, system_in_emergency: bool) -> AuthorityLevel {
        if *self == AuthorityLevel::Emergency && !system_in_emergency {
            AuthorityLevel::Supervisor
        } else {
            *self
        }
    }
}

/// System operating state — drives scheduling, safety thresholds, and authority policies
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SystemOperatingState {
    /// Normal operation — all constraints satisfied
    Normal,
    /// Alert — one or more constraints violated, needs attention
    Alert,
    /// Emergency — critical constraints violated, immediate action required
    Emergency,
    /// Blackout — system collapse, restoration needed
    Blackout,
    /// Restoration — recovering from blackout
    Restoration,
}

impl SystemOperatingState {
    /// Check if the system is in an emergency state (Emergency or Blackout)
    pub fn is_emergency(&self) -> bool {
        matches!(self, SystemOperatingState::Emergency | SystemOperatingState::Blackout)
    }

    /// Check if a state transition is valid
    pub fn can_transition_to(&self, target: SystemOperatingState) -> bool {
        match (self, target) {
            // Normal can go to Alert or Emergency (direct escalation)
            (SystemOperatingState::Normal, SystemOperatingState::Alert) => true,
            (SystemOperatingState::Normal, SystemOperatingState::Emergency) => true,
            // Alert can escalate to Emergency or recover to Normal
            (SystemOperatingState::Alert, SystemOperatingState::Emergency) => true,
            (SystemOperatingState::Alert, SystemOperatingState::Normal) => true,
            // Emergency can escalate to Blackout or recover to Alert/Normal
            (SystemOperatingState::Emergency, SystemOperatingState::Blackout) => true,
            (SystemOperatingState::Emergency, SystemOperatingState::Alert) => true,
            (SystemOperatingState::Emergency, SystemOperatingState::Normal) => true,
            // Blackout can only go to Restoration
            (SystemOperatingState::Blackout, SystemOperatingState::Restoration) => true,
            // Restoration can go to Normal (success) or back to Blackout (failure)
            (SystemOperatingState::Restoration, SystemOperatingState::Normal) => true,
            (SystemOperatingState::Restoration, SystemOperatingState::Blackout) => true,
            // Same state is always valid (no-op)
            (a, b) if *a == b => true,
            _ => false,
        }
    }
}

/// Action verdict — result of constraint-aware action validation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionVerdict {
    /// Action approved, can proceed
    Approved,
    /// Action rejected with reason
    Rejected(String),
    /// Action requires approval from higher authority
    PendingApproval { approver_level: AuthorityLevel, reason: String },
    /// Action bypassed non-critical checks due to emergency
    EmergencyBypassed { bypassed_checks: Vec<String>, reason: String },
}

/// Jurisdiction — defines the scope of an agent's authority
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Jurisdiction {
    /// Zone IDs this agent has authority over
    pub zone_ids: Vec<ZoneId>,
    /// Voltage levels this agent can operate on (in kV)
    pub voltage_levels: Vec<f64>,
    /// Specific device IDs this agent can control
    pub device_ids: Vec<ElementId>,
}

impl Jurisdiction {
    /// Create a jurisdiction for specific zones
    pub fn for_zones(zone_ids: Vec<ZoneId>) -> Self {
        Self {
            zone_ids,
            voltage_levels: Vec::new(),
            device_ids: Vec::new(),
        }
    }

    /// Create an unrestricted jurisdiction (all zones, all voltage levels)
    pub fn unrestricted() -> Self {
        Self {
            zone_ids: Vec::new(), // empty means all zones
            voltage_levels: Vec::new(), // empty means all levels
            device_ids: Vec::new(), // empty means all devices
        }
    }

    /// Check if a zone is within this jurisdiction
    pub fn contains_zone(&self, zone_id: ZoneId) -> bool {
        self.zone_ids.is_empty() || self.zone_ids.contains(&zone_id)
    }

    /// Check if a device is within this jurisdiction
    pub fn contains_device(&self, device_id: ElementId) -> bool {
        self.device_ids.is_empty() || self.device_ids.contains(&device_id)
    }

    /// Check if a voltage level is within this jurisdiction
    pub fn contains_voltage_level(&self, voltage_kv: f64) -> bool {
        self.voltage_levels.is_empty() || self.voltage_levels.iter().any(|&v| (v - voltage_kv).abs() < 0.1)
    }
}

/// Interlocking rule — prevents unsafe equipment operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterlockingRule {
    /// Unique rule identifier
    pub rule_id: String,
    /// Human-readable description of the rule
    pub description: String,
    /// The action type this rule blocks
    pub blocked_action: String,
    /// The condition that must be true for the block to apply
    pub condition: String,
    /// Error message when the rule blocks an operation
    pub block_message: String,
    /// Whether this is a hard constraint (cannot be bypassed even in emergency)
    pub is_hard_constraint: bool,
}

/// Audit entry — immutable record of an agent action
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unique entry ID
    pub entry_id: u64,
    /// Agent that performed the action
    pub agent_id: String,
    /// Agent's authority level at the time
    pub authority_level: AuthorityLevel,
    /// Description of the action
    pub action_description: String,
    /// Result of constraint checking
    pub constraint_check_result: String,
    /// Approval chain (who approved, if applicable)
    pub approval_chain: Vec<String>,
    /// Timestamp of the action
    pub timestamp: DateTime<Utc>,
    /// Summary of the reasoning behind the action
    pub reasoning_summary: String,
    /// System operating state at the time
    pub system_state: SystemOperatingState,
    /// Verdict of the action
    pub verdict: ActionVerdict,
}

/// Action feasibility result — predicts the impact of an action
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionFeasibility {
    /// Whether the action is feasible
    pub feasible: bool,
    /// New violations that would be introduced
    pub new_violations: Vec<String>,
    /// Existing violations that would be worsened
    pub worsened_violations: Vec<String>,
    /// Risk level of the action
    pub risk_level: SeverityLevel,
}

/// Structured emergency action — can be mapped to EmergencyAction in eneros-agent
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StructuredAction {
    /// Execute a device operation
    ExecuteDevice { device_id: u64, operation: String, value: f64 },
    /// Shed load
    ShedLoad { zone_id: u32, amount_mw: f64 },
    /// Start/adjust generator
    StartGenerator { gen_id: u64, target_mw: f64 },
    /// Notify an agent
    NotifyAgent { agent_id: String, message: String },
    /// Isolate fault section
    IsolateFault { upstream_switch: u64, downstream_switch: u64 },
    /// Close tie switch for restoration
    CloseTieSwitch { switch_id: u64 },
}

/// Emergency response plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmergencyResponsePlan {
    /// Unique plan identifier
    pub plan_id: String,
    /// Human-readable name
    pub name: String,
    /// Condition that triggers this plan
    pub trigger_condition: EmergencyTriggerCondition,
    /// Actions to execute
    pub actions: Vec<StructuredAction>,
    /// Safety checks to bypass during emergency execution
    pub bypass_checks: Vec<String>,
    /// Whether this plan requires approval
    pub requires_approval: bool,
}

/// Emergency trigger condition
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EmergencyTriggerCondition {
    /// Frequency below threshold (Hz)
    FrequencyBelow { threshold_hz: f64 },
    /// Cascading failure — N+ branches tripped
    CascadingFailure { min_branches_tripped: usize },
    /// Voltage collapse — voltage below threshold
    VoltageCollapse { threshold_pu: f64, min_buses: usize },
    /// Custom condition
    Custom(String),
}

/// Structured power system observation — replaces Vec<String> in ReasoningInput
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerObservation {
    /// Bus voltage observations: bus_id -> (vm_pu, va_degree)
    pub bus_voltages: HashMap<u64, BusVoltageObservation>,
    /// Branch flow observations: branch_id -> (p_mw, q_mvar, loading_percent)
    pub branch_flows: HashMap<u64, BranchFlowObservation>,
    /// System frequency in Hz
    pub frequency_hz: f64,
    /// Generator outputs: gen_id -> (p_mw, q_mvar)
    pub gen_outputs: HashMap<u64, GenOutputObservation>,
    /// Load consumptions: load_id -> (p_mw, q_mvar)
    pub load_consumptions: HashMap<u64, LoadConsumptionObservation>,
    /// Timestamp of observation
    pub timestamp: DateTime<Utc>,
    /// Total system load in MW
    pub total_load_mw: f64,
    /// Total system generation in MW
    pub total_gen_mw: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusVoltageObservation {
    pub vm_pu: f64,
    pub va_degree: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchFlowObservation {
    pub p_mw: f64,
    pub q_mvar: f64,
    pub loading_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenOutputObservation {
    pub p_mw: f64,
    pub q_mvar: f64,
    pub p_max_mw: f64,
    pub p_min_mw: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadConsumptionObservation {
    pub p_mw: f64,
    pub q_mvar: f64,
}

impl PowerObservation {
    /// Create from PowerSystemState
    pub fn from_network_state(state: &crate::types::PowerSystemState) -> Self {
        let mut bus_voltages = HashMap::new();
        for bv in &state.bus_voltages {
            bus_voltages.insert(bv.bus_id, BusVoltageObservation {
                vm_pu: bv.voltage_magnitude,
                va_degree: bv.voltage_angle,
            });
        }

        let mut branch_flows = HashMap::new();
        for bf in &state.branch_flows {
            branch_flows.insert(bf.branch_id, BranchFlowObservation {
                p_mw: bf.active_power_mw,
                q_mvar: bf.reactive_power_mvar,
                loading_percent: bf.loading_percent,
            });
        }

        let mut gen_outputs = HashMap::new();
        for go in &state.generation {
            gen_outputs.insert(go.gen_id, GenOutputObservation {
                p_mw: go.active_power_mw,
                q_mvar: go.reactive_power_mvar,
                p_max_mw: 0.0,
                p_min_mw: 0.0,
            });
        }

        let mut load_consumptions = HashMap::new();
        for lc in &state.loads {
            load_consumptions.insert(lc.load_id, LoadConsumptionObservation {
                p_mw: lc.active_power_mw,
                q_mvar: lc.reactive_power_mvar,
            });
        }

        let total_load_mw = state.loads.iter().map(|l| l.active_power_mw).sum();
        let total_gen_mw = state.generation.iter().map(|g| g.active_power_mw).sum();

        Self {
            bus_voltages,
            branch_flows,
            frequency_hz: state.frequency,
            gen_outputs,
            load_consumptions,
            timestamp: state.timestamp,
            total_load_mw,
            total_gen_mw,
        }
    }

    /// Create an empty observation
    pub fn empty() -> Self {
        Self {
            bus_voltages: HashMap::new(),
            branch_flows: HashMap::new(),
            frequency_hz: 50.0,
            gen_outputs: HashMap::new(),
            load_consumptions: HashMap::new(),
            timestamp: Utc::now(),
            total_load_mw: 0.0,
            total_gen_mw: 0.0,
        }
    }

    /// Get buses with voltage below threshold
    pub fn low_voltage_buses(&self, threshold_pu: f64) -> Vec<(u64, f64)> {
        self.bus_voltages.iter()
            .filter(|(_, v)| v.vm_pu < threshold_pu)
            .map(|(id, v)| (*id, v.vm_pu))
            .collect()
    }

    /// Get buses with voltage above threshold
    pub fn high_voltage_buses(&self, threshold_pu: f64) -> Vec<(u64, f64)> {
        self.bus_voltages.iter()
            .filter(|(_, v)| v.vm_pu > threshold_pu)
            .map(|(id, v)| (*id, v.vm_pu))
            .collect()
    }

    /// Get overloaded branches (loading > threshold%)
    pub fn overloaded_branches(&self, threshold_percent: f64) -> Vec<(u64, f64)> {
        self.branch_flows.iter()
            .filter(|(_, v)| v.loading_percent > threshold_percent)
            .map(|(id, v)| (*id, v.loading_percent))
            .collect()
    }

    /// Check if frequency is within normal range
    pub fn frequency_normal(&self, nominal_hz: f64, tolerance_hz: f64) -> bool {
        (self.frequency_hz - nominal_hz).abs() <= tolerance_hz
    }

    /// Human-readable summary
    pub fn summary(&self) -> String {
        let n_buses = self.bus_voltages.len();
        let n_branches = self.branch_flows.len();
        let n_gens = self.gen_outputs.len();
        let n_loads = self.load_consumptions.len();
        let low_v = self.low_voltage_buses(0.95).len();
        let high_v = self.high_voltage_buses(1.05).len();
        let overloaded = self.overloaded_branches(100.0).len();

        format!(
            "PowerObservation: {} buses ({} low V, {} high V), {} branches ({} overloaded), {} gens, {} loads, f={:.2}Hz, P_load={:.1}MW, P_gen={:.1}MW",
            n_buses, low_v, high_v, n_branches, overloaded, n_gens, n_loads,
            self.frequency_hz, self.total_load_mw, self.total_gen_mw
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === AuthorityLevel tests ===

    #[test]
    fn test_authority_level_ordering() {
        assert!(AuthorityLevel::Observer < AuthorityLevel::Operator);
        assert!(AuthorityLevel::Operator < AuthorityLevel::Supervisor);
        assert!(AuthorityLevel::Supervisor < AuthorityLevel::Emergency);
    }

    #[test]
    fn test_authority_level_can_execute_commands() {
        assert!(!AuthorityLevel::Observer.can_execute_commands());
        assert!(AuthorityLevel::Operator.can_execute_commands());
        assert!(AuthorityLevel::Supervisor.can_execute_commands());
        assert!(AuthorityLevel::Emergency.can_execute_commands());
    }

    #[test]
    fn test_authority_level_can_execute_high_risk() {
        assert!(!AuthorityLevel::Observer.can_execute_high_risk());
        assert!(!AuthorityLevel::Operator.can_execute_high_risk());
        assert!(AuthorityLevel::Supervisor.can_execute_high_risk());
        assert!(AuthorityLevel::Emergency.can_execute_high_risk());
    }

    #[test]
    fn test_authority_level_can_bypass_checks() {
        assert!(!AuthorityLevel::Observer.can_bypass_checks());
        assert!(!AuthorityLevel::Operator.can_bypass_checks());
        assert!(!AuthorityLevel::Supervisor.can_bypass_checks());
        assert!(AuthorityLevel::Emergency.can_bypass_checks());
    }

    #[test]
    fn test_effective_level_emergency_in_emergency() {
        assert_eq!(
            AuthorityLevel::Emergency.effective_level(true),
            AuthorityLevel::Emergency
        );
    }

    #[test]
    fn test_effective_level_emergency_not_in_emergency() {
        assert_eq!(
            AuthorityLevel::Emergency.effective_level(false),
            AuthorityLevel::Supervisor
        );
    }

    #[test]
    fn test_effective_level_non_emergency_unchanged() {
        assert_eq!(AuthorityLevel::Observer.effective_level(false), AuthorityLevel::Observer);
        assert_eq!(AuthorityLevel::Operator.effective_level(true), AuthorityLevel::Operator);
        assert_eq!(AuthorityLevel::Supervisor.effective_level(false), AuthorityLevel::Supervisor);
    }

    // === SystemOperatingState tests ===

    #[test]
    fn test_system_state_is_emergency() {
        assert!(!SystemOperatingState::Normal.is_emergency());
        assert!(!SystemOperatingState::Alert.is_emergency());
        assert!(SystemOperatingState::Emergency.is_emergency());
        assert!(SystemOperatingState::Blackout.is_emergency());
        assert!(!SystemOperatingState::Restoration.is_emergency());
    }

    #[test]
    fn test_valid_transitions_from_normal() {
        assert!(SystemOperatingState::Normal.can_transition_to(SystemOperatingState::Alert));
        assert!(SystemOperatingState::Normal.can_transition_to(SystemOperatingState::Emergency));
        assert!(SystemOperatingState::Normal.can_transition_to(SystemOperatingState::Normal));
    }

    #[test]
    fn test_invalid_transitions_from_normal() {
        assert!(!SystemOperatingState::Normal.can_transition_to(SystemOperatingState::Blackout));
        assert!(!SystemOperatingState::Normal.can_transition_to(SystemOperatingState::Restoration));
    }

    #[test]
    fn test_valid_transitions_from_alert() {
        assert!(SystemOperatingState::Alert.can_transition_to(SystemOperatingState::Emergency));
        assert!(SystemOperatingState::Alert.can_transition_to(SystemOperatingState::Normal));
        assert!(SystemOperatingState::Alert.can_transition_to(SystemOperatingState::Alert));
    }

    #[test]
    fn test_invalid_transitions_from_alert() {
        assert!(!SystemOperatingState::Alert.can_transition_to(SystemOperatingState::Blackout));
        assert!(!SystemOperatingState::Alert.can_transition_to(SystemOperatingState::Restoration));
    }

    #[test]
    fn test_valid_transitions_from_emergency() {
        assert!(SystemOperatingState::Emergency.can_transition_to(SystemOperatingState::Blackout));
        assert!(SystemOperatingState::Emergency.can_transition_to(SystemOperatingState::Alert));
        assert!(SystemOperatingState::Emergency.can_transition_to(SystemOperatingState::Normal));
        assert!(SystemOperatingState::Emergency.can_transition_to(SystemOperatingState::Emergency));
    }

    #[test]
    fn test_invalid_transitions_from_emergency() {
        assert!(!SystemOperatingState::Emergency.can_transition_to(SystemOperatingState::Restoration));
    }

    #[test]
    fn test_valid_transitions_from_blackout() {
        assert!(SystemOperatingState::Blackout.can_transition_to(SystemOperatingState::Restoration));
        assert!(SystemOperatingState::Blackout.can_transition_to(SystemOperatingState::Blackout));
    }

    #[test]
    fn test_invalid_transitions_from_blackout() {
        assert!(!SystemOperatingState::Blackout.can_transition_to(SystemOperatingState::Normal));
        assert!(!SystemOperatingState::Blackout.can_transition_to(SystemOperatingState::Alert));
        assert!(!SystemOperatingState::Blackout.can_transition_to(SystemOperatingState::Emergency));
    }

    #[test]
    fn test_valid_transitions_from_restoration() {
        assert!(SystemOperatingState::Restoration.can_transition_to(SystemOperatingState::Normal));
        assert!(SystemOperatingState::Restoration.can_transition_to(SystemOperatingState::Blackout));
        assert!(SystemOperatingState::Restoration.can_transition_to(SystemOperatingState::Restoration));
    }

    #[test]
    fn test_invalid_transitions_from_restoration() {
        assert!(!SystemOperatingState::Restoration.can_transition_to(SystemOperatingState::Alert));
        assert!(!SystemOperatingState::Restoration.can_transition_to(SystemOperatingState::Emergency));
    }

    // === Jurisdiction tests ===

    #[test]
    fn test_jurisdiction_for_zones() {
        let j = Jurisdiction::for_zones(vec![1, 2, 3]);
        assert_eq!(j.zone_ids, vec![1, 2, 3]);
        assert!(j.voltage_levels.is_empty());
        assert!(j.device_ids.is_empty());
    }

    #[test]
    fn test_jurisdiction_unrestricted() {
        let j = Jurisdiction::unrestricted();
        assert!(j.zone_ids.is_empty());
        assert!(j.voltage_levels.is_empty());
        assert!(j.device_ids.is_empty());
    }

    #[test]
    fn test_jurisdiction_contains_zone() {
        let j = Jurisdiction::for_zones(vec![1, 2, 3]);
        assert!(j.contains_zone(1));
        assert!(j.contains_zone(2));
        assert!(!j.contains_zone(99));
    }

    #[test]
    fn test_jurisdiction_unrestricted_contains_any_zone() {
        let j = Jurisdiction::unrestricted();
        assert!(j.contains_zone(1));
        assert!(j.contains_zone(999));
    }

    #[test]
    fn test_jurisdiction_contains_device() {
        let j = Jurisdiction {
            zone_ids: vec![],
            voltage_levels: vec![],
            device_ids: vec![100, 200, 300],
        };
        assert!(j.contains_device(100));
        assert!(!j.contains_device(999));
    }

    #[test]
    fn test_jurisdiction_unrestricted_contains_any_device() {
        let j = Jurisdiction::unrestricted();
        assert!(j.contains_device(42));
        assert!(j.contains_device(9999));
    }

    #[test]
    fn test_jurisdiction_contains_voltage_level() {
        let j = Jurisdiction {
            zone_ids: vec![],
            voltage_levels: vec![110.0, 220.0, 500.0],
            device_ids: vec![],
        };
        assert!(j.contains_voltage_level(110.0));
        assert!(j.contains_voltage_level(220.05)); // within 0.1 tolerance
        assert!(!j.contains_voltage_level(115.0));
    }

    #[test]
    fn test_jurisdiction_unrestricted_contains_any_voltage() {
        let j = Jurisdiction::unrestricted();
        assert!(j.contains_voltage_level(110.0));
        assert!(j.contains_voltage_level(999.0));
    }

    // === ActionVerdict tests ===

    #[test]
    fn test_action_verdict_approved() {
        let v = ActionVerdict::Approved;
        assert_eq!(v, ActionVerdict::Approved);
    }

    #[test]
    fn test_action_verdict_rejected() {
        let v = ActionVerdict::Rejected("unsafe operation".to_string());
        assert_eq!(v, ActionVerdict::Rejected("unsafe operation".to_string()));
    }

    #[test]
    fn test_action_verdict_pending_approval() {
        let v = ActionVerdict::PendingApproval {
            approver_level: AuthorityLevel::Supervisor,
            reason: "high risk operation".to_string(),
        };
        match v {
            ActionVerdict::PendingApproval { approver_level, reason } => {
                assert_eq!(approver_level, AuthorityLevel::Supervisor);
                assert_eq!(reason, "high risk operation");
            }
            _ => panic!("Expected PendingApproval"),
        }
    }

    #[test]
    fn test_action_verdict_emergency_bypassed() {
        let v = ActionVerdict::EmergencyBypassed {
            bypassed_checks: vec!["voltage_limit".to_string(), "thermal_limit".to_string()],
            reason: "system emergency".to_string(),
        };
        match v {
            ActionVerdict::EmergencyBypassed { bypassed_checks, reason } => {
                assert_eq!(bypassed_checks.len(), 2);
                assert_eq!(reason, "system emergency");
            }
            _ => panic!("Expected EmergencyBypassed"),
        }
    }

    // === ActionFeasibility tests ===

    #[test]
    fn test_action_feasibility_feasible() {
        let f = ActionFeasibility {
            feasible: true,
            new_violations: vec![],
            worsened_violations: vec![],
            risk_level: SeverityLevel::Info,
        };
        assert!(f.feasible);
        assert!(f.new_violations.is_empty());
        assert!(f.worsened_violations.is_empty());
    }

    #[test]
    fn test_action_feasibility_not_feasible() {
        let f = ActionFeasibility {
            feasible: false,
            new_violations: vec!["overcurrent".to_string()],
            worsened_violations: vec!["voltage violation".to_string()],
            risk_level: SeverityLevel::Critical,
        };
        assert!(!f.feasible);
        assert_eq!(f.new_violations.len(), 1);
        assert_eq!(f.worsened_violations.len(), 1);
        assert_eq!(f.risk_level, SeverityLevel::Critical);
    }

    // === EmergencyTriggerCondition tests ===

    #[test]
    fn test_emergency_trigger_frequency_below() {
        let t = EmergencyTriggerCondition::FrequencyBelow { threshold_hz: 49.5 };
        assert_eq!(t, EmergencyTriggerCondition::FrequencyBelow { threshold_hz: 49.5 });
    }

    #[test]
    fn test_emergency_trigger_cascading_failure() {
        let t = EmergencyTriggerCondition::CascadingFailure { min_branches_tripped: 3 };
        assert_eq!(t, EmergencyTriggerCondition::CascadingFailure { min_branches_tripped: 3 });
    }

    #[test]
    fn test_emergency_trigger_voltage_collapse() {
        let t = EmergencyTriggerCondition::VoltageCollapse { threshold_pu: 0.9, min_buses: 5 };
        assert_eq!(t, EmergencyTriggerCondition::VoltageCollapse { threshold_pu: 0.9, min_buses: 5 });
    }

    #[test]
    fn test_emergency_trigger_custom() {
        let t = EmergencyTriggerCondition::Custom("special condition".to_string());
        assert_eq!(t, EmergencyTriggerCondition::Custom("special condition".to_string()));
    }

    // === Serialization round-trip tests ===

    #[test]
    fn test_authority_level_serde_roundtrip() {
        let levels = [
            AuthorityLevel::Observer,
            AuthorityLevel::Operator,
            AuthorityLevel::Supervisor,
            AuthorityLevel::Emergency,
        ];
        for level in &levels {
            let json = serde_json::to_string(level).unwrap();
            let deserialized: AuthorityLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(*level, deserialized);
        }
    }

    #[test]
    fn test_system_operating_state_serde_roundtrip() {
        let states = [
            SystemOperatingState::Normal,
            SystemOperatingState::Alert,
            SystemOperatingState::Emergency,
            SystemOperatingState::Blackout,
            SystemOperatingState::Restoration,
        ];
        for state in &states {
            let json = serde_json::to_string(state).unwrap();
            let deserialized: SystemOperatingState = serde_json::from_str(&json).unwrap();
            assert_eq!(*state, deserialized);
        }
    }

    // === PowerObservation tests ===

    #[test]
    fn test_power_observation_empty() {
        let obs = PowerObservation::empty();
        assert!(obs.bus_voltages.is_empty());
        assert!(obs.branch_flows.is_empty());
        assert!(obs.gen_outputs.is_empty());
        assert!(obs.load_consumptions.is_empty());
        assert_eq!(obs.frequency_hz, 50.0);
        assert_eq!(obs.total_load_mw, 0.0);
        assert_eq!(obs.total_gen_mw, 0.0);
    }

    #[test]
    fn test_power_observation_from_network_state() {
        use crate::types::{BusVoltage, BranchFlow, GenOutput, LoadConsumption, PowerSystemState};

        let state = PowerSystemState {
            timestamp: Utc::now(),
            bus_voltages: vec![
                BusVoltage { bus_id: 1, voltage_magnitude: 1.02, voltage_angle: 0.0, voltage_kv: 220.0 },
                BusVoltage { bus_id: 2, voltage_magnitude: 0.93, voltage_angle: -5.0, voltage_kv: 110.0 },
            ],
            branch_flows: vec![
                BranchFlow {
                    branch_id: 10,
                    from_bus: 1,
                    to_bus: 2,
                    active_power_mw: 50.0,
                    reactive_power_mvar: 10.0,
                    current_ka: 0.15,
                    loading_percent: 80.0,
                },
            ],
            generation: vec![
                GenOutput {
                    gen_id: 100,
                    bus_id: 1,
                    active_power_mw: 200.0,
                    reactive_power_mvar: 30.0,
                    voltage_setpoint: 1.02,
                    status: true,
                },
            ],
            loads: vec![
                LoadConsumption {
                    load_id: 200,
                    bus_id: 2,
                    active_power_mw: 150.0,
                    reactive_power_mvar: 20.0,
                    status: true,
                },
            ],
            frequency: 49.95,
            total_losses: 5.0,
        };

        let obs = PowerObservation::from_network_state(&state);

        // Bus voltages
        assert_eq!(obs.bus_voltages.len(), 2);
        assert_eq!(obs.bus_voltages[&1].vm_pu, 1.02);
        assert_eq!(obs.bus_voltages[&1].va_degree, 0.0);
        assert_eq!(obs.bus_voltages[&2].vm_pu, 0.93);
        assert_eq!(obs.bus_voltages[&2].va_degree, -5.0);

        // Branch flows
        assert_eq!(obs.branch_flows.len(), 1);
        assert_eq!(obs.branch_flows[&10].p_mw, 50.0);
        assert_eq!(obs.branch_flows[&10].q_mvar, 10.0);
        assert_eq!(obs.branch_flows[&10].loading_percent, 80.0);

        // Generator outputs
        assert_eq!(obs.gen_outputs.len(), 1);
        assert_eq!(obs.gen_outputs[&100].p_mw, 200.0);
        assert_eq!(obs.gen_outputs[&100].q_mvar, 30.0);

        // Load consumptions
        assert_eq!(obs.load_consumptions.len(), 1);
        assert_eq!(obs.load_consumptions[&200].p_mw, 150.0);
        assert_eq!(obs.load_consumptions[&200].q_mvar, 20.0);

        // Aggregated values
        assert_eq!(obs.frequency_hz, 49.95);
        assert_eq!(obs.total_load_mw, 150.0);
        assert_eq!(obs.total_gen_mw, 200.0);
    }

    #[test]
    fn test_power_observation_low_voltage_buses() {
        let mut obs = PowerObservation::empty();
        obs.bus_voltages.insert(1, BusVoltageObservation { vm_pu: 0.90, va_degree: 0.0 });
        obs.bus_voltages.insert(2, BusVoltageObservation { vm_pu: 0.94, va_degree: -2.0 });
        obs.bus_voltages.insert(3, BusVoltageObservation { vm_pu: 1.01, va_degree: -1.0 });
        obs.bus_voltages.insert(4, BusVoltageObservation { vm_pu: 0.96, va_degree: 0.5 });

        let low = obs.low_voltage_buses(0.95);
        assert_eq!(low.len(), 2);
        let low_ids: Vec<u64> = low.iter().map(|(id, _)| *id).collect();
        assert!(low_ids.contains(&1));
        assert!(low_ids.contains(&2));
    }

    #[test]
    fn test_power_observation_high_voltage_buses() {
        let mut obs = PowerObservation::empty();
        obs.bus_voltages.insert(1, BusVoltageObservation { vm_pu: 1.06, va_degree: 0.0 });
        obs.bus_voltages.insert(2, BusVoltageObservation { vm_pu: 1.10, va_degree: 1.0 });
        obs.bus_voltages.insert(3, BusVoltageObservation { vm_pu: 1.02, va_degree: -1.0 });
        obs.bus_voltages.insert(4, BusVoltageObservation { vm_pu: 0.98, va_degree: 0.5 });

        let high = obs.high_voltage_buses(1.05);
        assert_eq!(high.len(), 2);
        let high_ids: Vec<u64> = high.iter().map(|(id, _)| *id).collect();
        assert!(high_ids.contains(&1));
        assert!(high_ids.contains(&2));
    }

    #[test]
    fn test_power_observation_overloaded_branches() {
        let mut obs = PowerObservation::empty();
        obs.branch_flows.insert(10, BranchFlowObservation { p_mw: 50.0, q_mvar: 10.0, loading_percent: 80.0 });
        obs.branch_flows.insert(20, BranchFlowObservation { p_mw: 120.0, q_mvar: 30.0, loading_percent: 110.0 });
        obs.branch_flows.insert(30, BranchFlowObservation { p_mw: 130.0, q_mvar: 40.0, loading_percent: 150.0 });

        let overloaded = obs.overloaded_branches(100.0);
        assert_eq!(overloaded.len(), 2);
        let overloaded_ids: Vec<u64> = overloaded.iter().map(|(id, _)| *id).collect();
        assert!(overloaded_ids.contains(&20));
        assert!(overloaded_ids.contains(&30));
    }

    #[test]
    fn test_power_observation_frequency_normal() {
        let mut obs = PowerObservation::empty();

        obs.frequency_hz = 50.0;
        assert!(obs.frequency_normal(50.0, 0.2));

        obs.frequency_hz = 49.85;
        assert!(obs.frequency_normal(50.0, 0.2));

        obs.frequency_hz = 49.79;
        assert!(!obs.frequency_normal(50.0, 0.2));

        obs.frequency_hz = 50.15;
        assert!(obs.frequency_normal(50.0, 0.2));

        obs.frequency_hz = 50.21;
        assert!(!obs.frequency_normal(50.0, 0.2));
    }

    #[test]
    fn test_power_observation_summary() {
        let mut obs = PowerObservation::empty();
        obs.bus_voltages.insert(1, BusVoltageObservation { vm_pu: 0.90, va_degree: 0.0 });
        obs.bus_voltages.insert(2, BusVoltageObservation { vm_pu: 1.06, va_degree: 1.0 });
        obs.bus_voltages.insert(3, BusVoltageObservation { vm_pu: 1.01, va_degree: -1.0 });
        obs.branch_flows.insert(10, BranchFlowObservation { p_mw: 50.0, q_mvar: 10.0, loading_percent: 120.0 });
        obs.gen_outputs.insert(100, GenOutputObservation { p_mw: 200.0, q_mvar: 30.0, p_max_mw: 300.0, p_min_mw: 0.0 });
        obs.load_consumptions.insert(200, LoadConsumptionObservation { p_mw: 150.0, q_mvar: 20.0 });
        obs.frequency_hz = 49.9;
        obs.total_load_mw = 150.0;
        obs.total_gen_mw = 200.0;

        let s = obs.summary();
        assert!(s.contains("3 buses"));
        assert!(s.contains("1 low V"));
        assert!(s.contains("1 high V"));
        assert!(s.contains("1 branches"));
        assert!(s.contains("1 overloaded"));
        assert!(s.contains("1 gens"));
        assert!(s.contains("1 loads"));
        assert!(s.contains("49.90Hz"));
        assert!(s.contains("150.0MW"));
        assert!(s.contains("200.0MW"));
    }

    #[test]
    fn test_power_observation_serde_roundtrip() {
        let mut obs = PowerObservation::empty();
        obs.bus_voltages.insert(1, BusVoltageObservation { vm_pu: 1.02, va_degree: -3.5 });
        obs.branch_flows.insert(10, BranchFlowObservation { p_mw: 50.0, q_mvar: 10.0, loading_percent: 80.0 });
        obs.gen_outputs.insert(100, GenOutputObservation { p_mw: 200.0, q_mvar: 30.0, p_max_mw: 300.0, p_min_mw: 50.0 });
        obs.load_consumptions.insert(200, LoadConsumptionObservation { p_mw: 150.0, q_mvar: 20.0 });
        obs.frequency_hz = 50.0;
        obs.total_load_mw = 150.0;
        obs.total_gen_mw = 200.0;

        let json = serde_json::to_string(&obs).unwrap();
        let deserialized: PowerObservation = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.bus_voltages[&1].vm_pu, 1.02);
        assert_eq!(deserialized.bus_voltages[&1].va_degree, -3.5);
        assert_eq!(deserialized.branch_flows[&10].loading_percent, 80.0);
        assert_eq!(deserialized.gen_outputs[&100].p_max_mw, 300.0);
        assert_eq!(deserialized.load_consumptions[&200].p_mw, 150.0);
        assert_eq!(deserialized.frequency_hz, 50.0);
        assert_eq!(deserialized.total_load_mw, 150.0);
        assert_eq!(deserialized.total_gen_mw, 200.0);
    }

    // === StructuredAction tests ===

    #[test]
    fn test_structured_action_serde_roundtrip() {
        let actions = vec![
            StructuredAction::ExecuteDevice { device_id: 1, operation: "close".to_string(), value: 1.0 },
            StructuredAction::ShedLoad { zone_id: 2, amount_mw: 50.0 },
            StructuredAction::StartGenerator { gen_id: 3, target_mw: 100.0 },
            StructuredAction::NotifyAgent { agent_id: "dispatch".to_string(), message: "紧急切负荷".to_string() },
            StructuredAction::IsolateFault { upstream_switch: 10, downstream_switch: 20 },
            StructuredAction::CloseTieSwitch { switch_id: 30 },
        ];
        for action in &actions {
            let json = serde_json::to_string(action).unwrap();
            let deserialized: StructuredAction = serde_json::from_str(&json).unwrap();
            assert_eq!(action, &deserialized);
        }
    }

    #[test]
    fn test_structured_action_equality() {
        let a1 = StructuredAction::ShedLoad { zone_id: 1, amount_mw: 50.0 };
        let a2 = StructuredAction::ShedLoad { zone_id: 1, amount_mw: 50.0 };
        let a3 = StructuredAction::ShedLoad { zone_id: 2, amount_mw: 50.0 };
        assert_eq!(a1, a2);
        assert_ne!(a1, a3);
    }

    #[test]
    fn test_emergency_response_plan_with_structured_actions() {
        let plan = EmergencyResponsePlan {
            plan_id: "TEST_PLAN".to_string(),
            name: "测试计划".to_string(),
            trigger_condition: EmergencyTriggerCondition::FrequencyBelow { threshold_hz: 49.5 },
            actions: vec![
                StructuredAction::ShedLoad { zone_id: 1, amount_mw: 50.0 },
                StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 },
            ],
            bypass_checks: vec!["approval_flow".to_string()],
            requires_approval: false,
        };
        assert_eq!(plan.actions.len(), 2);
        let json = serde_json::to_string(&plan).unwrap();
        let deserialized: EmergencyResponsePlan = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.plan_id, "TEST_PLAN");
        assert_eq!(deserialized.actions.len(), 2);
    }
}
