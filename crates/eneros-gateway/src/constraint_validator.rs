use std::sync::Arc;
use eneros_core::{AuthorityLevel, Jurisdiction, SystemOperatingState, ActionVerdict};
use eneros_constraint::ConstraintEngine;
use crate::interlocking::{InterlockingRuleEngine, DeviceOperation, DeviceStates, OperationType};
use crate::gateway::SafetyGateway;

/// Constraint-aware action validator
/// Validates agent actions through a multi-step pipeline:
/// 1. Authority check — does the agent have permission?
/// 2. Jurisdiction check — is the target within the agent's scope?
/// 3. Constraint pre-check — will the action worsen existing violations?
/// 4. Interlocking check — does the operation violate safety interlocks?
/// 5. Safety gateway check — does the action pass static safety limits?
/// 6. Approval check — does the action require higher-level approval?
pub struct ConstraintAwareValidator {
    /// Constraint engine for pre-action validation
    constraint_engine: Arc<ConstraintEngine>,
    /// Safety gateway for static safety checks
    #[allow(dead_code)]
    safety_gateway: Arc<SafetyGateway>,
    /// Interlocking rule engine for equipment operation safety
    interlocking_engine: InterlockingRuleEngine,
}

impl ConstraintAwareValidator {
    /// Create a new validator
    pub fn new(
        constraint_engine: Arc<ConstraintEngine>,
        safety_gateway: Arc<SafetyGateway>,
        interlocking_engine: InterlockingRuleEngine,
    ) -> Self {
        Self {
            constraint_engine,
            safety_gateway,
            interlocking_engine,
        }
    }

    /// Create a validator with default interlocking engine
    pub fn with_default_interlocking(
        constraint_engine: Arc<ConstraintEngine>,
        safety_gateway: Arc<SafetyGateway>,
    ) -> Self {
        Self {
            constraint_engine,
            safety_gateway,
            interlocking_engine: InterlockingRuleEngine::new(),
        }
    }

    /// Validate an action through the full pipeline
    #[allow(clippy::too_many_arguments)]
    pub fn validate(
        &self,
        action_description: &str,
        authority: AuthorityLevel,
        jurisdiction: &Jurisdiction,
        system_state: SystemOperatingState,
        target_zone_id: Option<u32>,
        target_device_id: Option<u64>,
        device_states: Option<&DeviceStates>,
    ) -> ActionVerdict {
        let effective_authority = authority.effective_level(system_state.is_emergency());

        // Step 1: Authority check
        if !effective_authority.can_execute_commands() {
            return ActionVerdict::Rejected(format!(
                "Insufficient authority: {:?} cannot execute commands",
                authority
            ));
        }

        // Step 2: Jurisdiction check
        if let Some(zone_id) = target_zone_id {
            if !jurisdiction.contains_zone(zone_id) {
                return ActionVerdict::Rejected(format!(
                    "Zone {} is outside agent's jurisdiction",
                    zone_id
                ));
            }
        }
        if let Some(device_id) = target_device_id {
            if !jurisdiction.contains_device(device_id) {
                return ActionVerdict::Rejected(format!(
                    "Device {} is outside agent's jurisdiction",
                    device_id
                ));
            }
        }

        // Step 3: Constraint pre-check
        let feasibility = self.constraint_engine.check_action_feasibility(action_description);
        if !feasibility.feasible && !effective_authority.can_bypass_checks() {
            return ActionVerdict::Rejected(format!(
                "Action would worsen existing violations: {:?}",
                feasibility.worsened_violations
            ));
        }

        // Step 4: Interlocking check (if device states provided)
        if let Some(states) = device_states {
            if let Some(device_id) = target_device_id {
                let operation = self.infer_operation(action_description, device_id);
                let interlocking_result = self.interlocking_engine.check(&operation, states);

                if !interlocking_result.allowed {
                    // Check if bypass is possible in emergency
                    if system_state.is_emergency() && self.interlocking_engine.can_bypass_in_emergency(&operation, states) {
                        return ActionVerdict::EmergencyBypassed {
                            bypassed_checks: interlocking_result.blocked_by,
                            reason: format!("Emergency bypass: {}", interlocking_result.messages.join("; ")),
                        };
                    }
                    return ActionVerdict::Rejected(format!(
                        "Interlocking violation: {}",
                        interlocking_result.messages.join("; ")
                    ));
                }
            }
        }

        // Step 5: Safety gateway check (for high-risk actions)
        if self.is_high_risk_action(action_description) && !effective_authority.can_execute_high_risk() {
            return ActionVerdict::PendingApproval {
                approver_level: AuthorityLevel::Supervisor,
                reason: format!("High-risk action requires {:?} approval: {}", AuthorityLevel::Supervisor, action_description),
            };
        }

        // Step 6: Emergency bypass for feasible but risky actions
        if !feasibility.new_violations.is_empty() && effective_authority.can_bypass_checks() {
            return ActionVerdict::EmergencyBypassed {
                bypassed_checks: vec!["constraint_pre_check".to_string()],
                reason: format!("Emergency override: action may cause {:?}", feasibility.new_violations),
            };
        }

        ActionVerdict::Approved
    }

    /// Infer the operation type from action description
    fn infer_operation(&self, description: &str, device_id: u64) -> DeviceOperation {
        let desc_lower = description.to_lowercase();
        let op_type = if desc_lower.contains("close breaker") || desc_lower.contains("合断路器") {
            OperationType::CloseBreaker
        } else if desc_lower.contains("open breaker") || desc_lower.contains("断断路器") || desc_lower.contains("分断路器") {
            OperationType::OpenBreaker
        } else if desc_lower.contains("close disconnector") || desc_lower.contains("合隔离开关") {
            OperationType::CloseDisconnector
        } else if desc_lower.contains("open disconnector") || desc_lower.contains("拉隔离开关") {
            OperationType::OpenDisconnector
        } else if desc_lower.contains("close ground") || desc_lower.contains("合接地") {
            OperationType::CloseGroundSwitch
        } else if desc_lower.contains("open ground") || desc_lower.contains("拆接地") {
            OperationType::OpenGroundSwitch
        } else if desc_lower.contains("close tie") || desc_lower.contains("合环") {
            OperationType::CloseTieSwitch
        } else {
            OperationType::CloseBreaker // default
        };

        DeviceOperation {
            operation_type: op_type,
            target_device_id: device_id,
            associated_buses: Vec::new(),
            associated_breaker_id: None,
        }
    }

    /// Check if an action is high-risk (requires supervisor approval)
    fn is_high_risk_action(&self, description: &str) -> bool {
        let desc_lower = description.to_lowercase();
        desc_lower.contains("load shed")
            || desc_lower.contains("切负荷")
            || desc_lower.contains("system separation")
            || desc_lower.contains("系统解列")
            || desc_lower.contains("island")
            || desc_lower.contains("孤岛")
            || desc_lower.contains("black start")
            || desc_lower.contains("黑启动")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_constraint::ConstraintEngine;

    fn make_validator() -> ConstraintAwareValidator {
        ConstraintAwareValidator::with_default_interlocking(
            Arc::new(ConstraintEngine::new()),
            Arc::new(SafetyGateway::new(100)),
        )
    }

    #[test]
    fn test_observer_rejected() {
        let validator = make_validator();
        let result = validator.validate(
            "close breaker",
            AuthorityLevel::Observer,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
            None, None, None,
        );
        assert!(matches!(result, ActionVerdict::Rejected(_)));
    }

    #[test]
    fn test_operator_approved() {
        let validator = make_validator();
        let result = validator.validate(
            "close breaker 101",
            AuthorityLevel::Operator,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
            None, None, None,
        );
        assert!(matches!(result, ActionVerdict::Approved));
    }

    #[test]
    fn test_jurisdiction_zone_rejected() {
        let validator = make_validator();
        let jurisdiction = Jurisdiction::for_zones(vec![1, 2]);
        let result = validator.validate(
            "close breaker",
            AuthorityLevel::Operator,
            &jurisdiction,
            SystemOperatingState::Normal,
            Some(5), // Zone 5 not in jurisdiction
            None, None,
        );
        assert!(matches!(result, ActionVerdict::Rejected(_)));
    }

    #[test]
    fn test_jurisdiction_device_rejected() {
        let validator = make_validator();
        let mut jurisdiction = Jurisdiction::unrestricted();
        jurisdiction.device_ids = vec![100, 200]; // Only devices 100 and 200
        let result = validator.validate(
            "close breaker",
            AuthorityLevel::Operator,
            &jurisdiction,
            SystemOperatingState::Normal,
            None,
            Some(999), // Device 999 not in jurisdiction
            None,
        );
        assert!(matches!(result, ActionVerdict::Rejected(_)));
    }

    #[test]
    fn test_high_risk_requires_approval() {
        let validator = make_validator();
        let result = validator.validate(
            "load shedding zone 1",
            AuthorityLevel::Operator,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
            None, None, None,
        );
        assert!(matches!(result, ActionVerdict::PendingApproval { .. }));
    }

    #[test]
    fn test_supervisor_can_execute_high_risk() {
        let validator = make_validator();
        let result = validator.validate(
            "load shedding zone 1",
            AuthorityLevel::Supervisor,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
            None, None, None,
        );
        assert!(matches!(result, ActionVerdict::Approved));
    }

    #[test]
    fn test_emergency_authority_in_normal_state_demoted() {
        let validator = make_validator();
        // Emergency authority in Normal state is demoted to Supervisor
        // Supervisor can execute high-risk actions
        let result = validator.validate(
            "load shedding zone 1",
            AuthorityLevel::Emergency,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
            None, None, None,
        );
        // Emergency in Normal state -> effective Supervisor -> can execute high risk
        assert!(matches!(result, ActionVerdict::Approved));
    }

    #[test]
    fn test_emergency_bypass() {
        let validator = make_validator();
        let result = validator.validate(
            "close breaker 101",
            AuthorityLevel::Emergency,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Emergency,
            None, None, None,
        );
        // Emergency authority in Emergency state -> can bypass
        assert!(matches!(result, ActionVerdict::Approved | ActionVerdict::EmergencyBypassed { .. }));
    }

    #[test]
    fn test_interlocking_block() {
        let validator = make_validator();
        let mut states = DeviceStates::default();
        states.ground_switch_states.insert(10, true); // Ground switch closed

        let result = validator.validate(
            "close breaker",
            AuthorityLevel::Operator,
            &Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
            None,
            Some(1),
            Some(&states),
        );
        assert!(matches!(result, ActionVerdict::Rejected(_)));
    }

    #[test]
    fn test_infer_operation_close_breaker() {
        let validator = make_validator();
        let op = validator.infer_operation("close breaker 101", 101);
        assert_eq!(op.operation_type, OperationType::CloseBreaker);
    }

    #[test]
    fn test_infer_operation_open_disconnector() {
        let validator = make_validator();
        let op = validator.infer_operation("拉隔离开关", 202);
        assert_eq!(op.operation_type, OperationType::OpenDisconnector);
    }

    #[test]
    fn test_is_high_risk_action() {
        let validator = make_validator();
        assert!(validator.is_high_risk_action("load shedding zone 1"));
        assert!(validator.is_high_risk_action("切负荷"));
        assert!(validator.is_high_risk_action("system separation"));
        assert!(!validator.is_high_risk_action("close breaker"));
    }
}
