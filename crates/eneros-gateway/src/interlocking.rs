use std::collections::HashMap;
use eneros_core::{ElementId, InterlockingRule};
use serde::{Deserialize, Serialize};

/// Result of an interlocking check
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterlockingResult {
    /// Whether the operation is allowed
    pub allowed: bool,
    /// Rules that blocked the operation (if any)
    pub blocked_by: Vec<String>,
    /// Messages explaining why the operation was blocked
    pub messages: Vec<String>,
}

impl InterlockingResult {
    /// Create an allowed result
    pub fn allowed() -> Self {
        Self {
            allowed: true,
            blocked_by: Vec::new(),
            messages: Vec::new(),
        }
    }

    /// Create a blocked result
    pub fn blocked(rule_id: &str, message: &str) -> Self {
        Self {
            allowed: false,
            blocked_by: vec![rule_id.to_string()],
            messages: vec![message.to_string()],
        }
    }

    /// Merge two results
    pub fn merge(&mut self, other: InterlockingResult) {
        if !other.allowed {
            self.allowed = false;
        }
        self.blocked_by.extend(other.blocked_by);
        self.messages.extend(other.messages);
    }
}

/// Device state for interlocking checks
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceStates {
    /// Breaker states: true = closed, false = open
    pub breaker_states: HashMap<ElementId, bool>,
    /// Disconnector states: true = closed, false = open
    pub disconnector_states: HashMap<ElementId, bool>,
    /// Ground switch states: true = closed (grounded), false = open
    pub ground_switch_states: HashMap<ElementId, bool>,
    /// Bus voltage magnitudes (p.u.) for sync check
    pub bus_voltages: HashMap<ElementId, f64>,
    /// Bus voltage angles (degrees) for sync check
    pub bus_angles: HashMap<ElementId, f64>,
}

/// Operation being checked
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceOperation {
    /// Type of operation
    pub operation_type: OperationType,
    /// Target device ID
    pub target_device_id: ElementId,
    /// Associated bus IDs (for sync check)
    pub associated_buses: Vec<ElementId>,
    /// Associated breaker ID (for disconnector operation)
    pub associated_breaker_id: Option<ElementId>,
}

/// Type of device operation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationType {
    /// Close a breaker
    CloseBreaker,
    /// Open a breaker
    OpenBreaker,
    /// Close a disconnector (isolator)
    CloseDisconnector,
    /// Open a disconnector (isolator)
    OpenDisconnector,
    /// Close a ground switch
    CloseGroundSwitch,
    /// Open a ground switch
    OpenGroundSwitch,
    /// Close a tie switch (sync check required)
    CloseTieSwitch,
}

/// Interlocking rule engine — prevents unsafe equipment operations
pub struct InterlockingRuleEngine {
    /// Registered interlocking rules
    rules: Vec<InterlockingRule>,
}

impl InterlockingRuleEngine {
    /// Create a new engine with built-in rules
    pub fn new() -> Self {
        let mut engine = Self { rules: Vec::new() };
        engine.add_builtin_rules();
        engine
    }

    /// Create an engine without built-in rules
    pub fn empty() -> Self {
        Self { rules: Vec::new() }
    }

    /// Add a custom interlocking rule
    pub fn add_rule(&mut self, rule: InterlockingRule) {
        self.rules.push(rule);
    }

    /// Check if an operation is allowed given current device states
    pub fn check(&self, operation: &DeviceOperation, states: &DeviceStates) -> InterlockingResult {
        let mut result = InterlockingResult::allowed();

        // Built-in rule 1: Cannot open disconnector when breaker is closed
        if operation.operation_type == OperationType::OpenDisconnector {
            if let Some(breaker_id) = operation.associated_breaker_id {
                if states.breaker_states.get(&breaker_id).copied().unwrap_or(false) {
                    result.merge(InterlockingResult::blocked(
                        "BUILTIN_BREAKER_OPEN_BEFORE_DISCONNECTOR",
                        "断路器未断开，禁止拉开隔离开关 (Breaker must be open before opening disconnector)",
                    ));
                }
            }
        }

        // Built-in rule 2: Cannot close breaker/switch when ground is applied
        if matches!(operation.operation_type, OperationType::CloseBreaker | OperationType::CloseDisconnector | OperationType::CloseTieSwitch) {
            // Check if any ground switch on associated buses is closed
            for (gs_id, &closed) in &states.ground_switch_states {
                if closed {
                    result.merge(InterlockingResult::blocked(
                        "BUILTIN_GROUND_REMOVED_BEFORE_CLOSE",
                        &format!("接地线未拆除（接地开关 {} 在合位），禁止合闸 (Ground switch {} must be open before closing)", gs_id, gs_id),
                    ));
                }
            }
        }

        // Built-in rule 3: Sync check for closing tie switch
        if operation.operation_type == OperationType::CloseTieSwitch && operation.associated_buses.len() == 2 {
            let bus1 = operation.associated_buses[0];
            let bus2 = operation.associated_buses[1];

            let v1 = states.bus_voltages.get(&bus1).copied().unwrap_or(1.0);
            let v2 = states.bus_voltages.get(&bus2).copied().unwrap_or(1.0);
            let a1 = states.bus_angles.get(&bus1).copied().unwrap_or(0.0);
            let a2 = states.bus_angles.get(&bus2).copied().unwrap_or(0.0);

            let voltage_diff = (v1 - v2).abs();
            let angle_diff = (a1 - a2).abs();

            // Thresholds: voltage difference < 0.1 p.u., angle difference < 20 degrees
            if voltage_diff > 0.1 || angle_diff > 20.0 {
                result.merge(InterlockingResult::blocked(
                    "BUILTIN_SYNC_CHECK_BEFORE_CLOSE",
                    &format!(
                        "同期检查未通过：电压差={:.3} p.u. (限值0.1), 角度差={:.1}° (限值20°) (Sync check failed: ΔV={:.3} pu, Δθ={:.1}°)",
                        voltage_diff, angle_diff, voltage_diff, angle_diff
                    ),
                ));
            }
        }

        // Check custom rules
        for rule in &self.rules {
            if rule.blocked_action == format!("{:?}", operation.operation_type) {
                // Simple condition evaluation: check if condition string matches a known pattern
                if self.evaluate_condition(&rule.condition, operation, states) {
                    result.merge(InterlockingResult::blocked(&rule.rule_id, &rule.block_message));
                }
            }
        }

        result
    }

    /// Check if an operation can be bypassed in emergency
    pub fn can_bypass_in_emergency(&self, operation: &DeviceOperation, states: &DeviceStates) -> bool {
        // Hard constraints (like opening disconnector under load) cannot be bypassed
        if operation.operation_type == OperationType::OpenDisconnector {
            if let Some(breaker_id) = operation.associated_breaker_id {
                if states.breaker_states.get(&breaker_id).copied().unwrap_or(false) {
                    return false; // This is a hard constraint — arc flash hazard
                }
            }
        }
        true
    }

    /// Simple condition evaluator for custom rules
    fn evaluate_condition(&self, condition: &str, operation: &DeviceOperation, states: &DeviceStates) -> bool {
        let cond_lower = condition.to_lowercase();
        if cond_lower.contains("breaker_closed") {
            if let Some(bid) = operation.associated_breaker_id {
                return states.breaker_states.get(&bid).copied().unwrap_or(false);
            }
        }
        if cond_lower.contains("ground_applied") {
            return states.ground_switch_states.values().any(|&v| v);
        }
        false
    }

    fn add_builtin_rules(&mut self) {
        // Built-in rules are implemented directly in check() for performance
        // Custom rules can be added via add_rule()
    }

    /// Get all registered rules
    pub fn rules(&self) -> &[InterlockingRule] {
        &self.rules
    }
}

impl Default for InterlockingRuleEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interlocking_result_allowed() {
        let result = InterlockingResult::allowed();
        assert!(result.allowed);
        assert!(result.blocked_by.is_empty());
    }

    #[test]
    fn test_interlocking_result_blocked() {
        let result = InterlockingResult::blocked("RULE_1", "blocked");
        assert!(!result.allowed);
        assert_eq!(result.blocked_by, vec!["RULE_1"]);
    }

    #[test]
    fn test_interlocking_result_merge() {
        let mut r1 = InterlockingResult::allowed();
        let r2 = InterlockingResult::blocked("R2", "msg2");
        r1.merge(r2);
        assert!(!r1.allowed);
        assert_eq!(r1.blocked_by.len(), 1);
    }

    #[test]
    fn test_open_disconnector_breaker_closed_blocked() {
        let engine = InterlockingRuleEngine::new();
        let mut states = DeviceStates::default();
        states.breaker_states.insert(1, true); // Breaker closed

        let op = DeviceOperation {
            operation_type: OperationType::OpenDisconnector,
            target_device_id: 2,
            associated_buses: vec![],
            associated_breaker_id: Some(1),
        };

        let result = engine.check(&op, &states);
        assert!(!result.allowed);
        assert!(result.messages[0].contains("断路器未断开"));
    }

    #[test]
    fn test_open_disconnector_breaker_open_allowed() {
        let engine = InterlockingRuleEngine::new();
        let mut states = DeviceStates::default();
        states.breaker_states.insert(1, false); // Breaker open

        let op = DeviceOperation {
            operation_type: OperationType::OpenDisconnector,
            target_device_id: 2,
            associated_buses: vec![],
            associated_breaker_id: Some(1),
        };

        let result = engine.check(&op, &states);
        assert!(result.allowed);
    }

    #[test]
    fn test_close_breaker_ground_applied_blocked() {
        let engine = InterlockingRuleEngine::new();
        let mut states = DeviceStates::default();
        states.ground_switch_states.insert(10, true); // Ground applied

        let op = DeviceOperation {
            operation_type: OperationType::CloseBreaker,
            target_device_id: 1,
            associated_buses: vec![],
            associated_breaker_id: None,
        };

        let result = engine.check(&op, &states);
        assert!(!result.allowed);
        assert!(result.messages[0].contains("接地线未拆除"));
    }

    #[test]
    fn test_sync_check_pass() {
        let engine = InterlockingRuleEngine::new();
        let mut states = DeviceStates::default();
        states.bus_voltages.insert(100, 1.02);
        states.bus_voltages.insert(101, 1.01);
        states.bus_angles.insert(100, 5.0);
        states.bus_angles.insert(101, 3.0);

        let op = DeviceOperation {
            operation_type: OperationType::CloseTieSwitch,
            target_device_id: 50,
            associated_buses: vec![100, 101],
            associated_breaker_id: None,
        };

        let result = engine.check(&op, &states);
        assert!(result.allowed);
    }

    #[test]
    fn test_sync_check_fail_voltage() {
        let engine = InterlockingRuleEngine::new();
        let mut states = DeviceStates::default();
        states.bus_voltages.insert(100, 1.05);
        states.bus_voltages.insert(101, 0.90);
        states.bus_angles.insert(100, 0.0);
        states.bus_angles.insert(101, 0.0);

        let op = DeviceOperation {
            operation_type: OperationType::CloseTieSwitch,
            target_device_id: 50,
            associated_buses: vec![100, 101],
            associated_breaker_id: None,
        };

        let result = engine.check(&op, &states);
        assert!(!result.allowed);
        assert!(result.messages[0].contains("同期检查未通过"));
    }

    #[test]
    fn test_sync_check_fail_angle() {
        let engine = InterlockingRuleEngine::new();
        let mut states = DeviceStates::default();
        states.bus_voltages.insert(100, 1.00);
        states.bus_voltages.insert(101, 1.00);
        states.bus_angles.insert(100, 0.0);
        states.bus_angles.insert(101, 25.0);

        let op = DeviceOperation {
            operation_type: OperationType::CloseTieSwitch,
            target_device_id: 50,
            associated_buses: vec![100, 101],
            associated_breaker_id: None,
        };

        let result = engine.check(&op, &states);
        assert!(!result.allowed);
    }

    #[test]
    fn test_cannot_bypass_disconnector_under_load() {
        let engine = InterlockingRuleEngine::new();
        let mut states = DeviceStates::default();
        states.breaker_states.insert(1, true);

        let op = DeviceOperation {
            operation_type: OperationType::OpenDisconnector,
            target_device_id: 2,
            associated_buses: vec![],
            associated_breaker_id: Some(1),
        };

        assert!(!engine.can_bypass_in_emergency(&op, &states));
    }

    #[test]
    fn test_can_bypass_other_in_emergency() {
        let engine = InterlockingRuleEngine::new();
        let states = DeviceStates::default();

        let op = DeviceOperation {
            operation_type: OperationType::CloseBreaker,
            target_device_id: 1,
            associated_buses: vec![],
            associated_breaker_id: None,
        };

        assert!(engine.can_bypass_in_emergency(&op, &states));
    }

    #[test]
    fn test_custom_rule() {
        let mut engine = InterlockingRuleEngine::new();
        engine.add_rule(InterlockingRule {
            rule_id: "CUSTOM_1".to_string(),
            description: "Custom test rule".to_string(),
            blocked_action: "CloseBreaker".to_string(),
            condition: "ground_applied".to_string(),
            block_message: "Custom: ground applied".to_string(),
            is_hard_constraint: false,
        });

        let mut states = DeviceStates::default();
        states.ground_switch_states.insert(10, true);

        let op = DeviceOperation {
            operation_type: OperationType::CloseBreaker,
            target_device_id: 1,
            associated_buses: vec![],
            associated_breaker_id: None,
        };

        let result = engine.check(&op, &states);
        assert!(!result.allowed);
    }

    #[test]
    fn test_engine_default() {
        let engine = InterlockingRuleEngine::default();
        assert!(engine.rules().is_empty()); // Built-in rules are in check(), not in rules vec
    }
}
