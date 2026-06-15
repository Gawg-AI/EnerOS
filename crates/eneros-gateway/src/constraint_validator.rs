use std::sync::Arc;
use eneros_core::{AuthorityLevel, Jurisdiction, SystemOperatingState, ActionVerdict, StructuredAction};
use eneros_constraint::ConstraintEngine;
use eneros_constraint::projector::{FeasibilityProjector, ProjectionResult};
use crate::interlocking::{InterlockingRuleEngine, DeviceOperation, DeviceStates, OperationType};
use crate::gateway::SafetyGateway;
use crate::command::{Command, CommandType, CommandPriority};

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
    safety_gateway: Arc<SafetyGateway>,
    /// Interlocking rule engine for equipment operation safety
    interlocking_engine: InterlockingRuleEngine,
    /// Optional projector for What-If based feasibility checks
    projector: Option<Arc<FeasibilityProjector>>,
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
            projector: None,
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
            projector: None,
        }
    }

    /// Create a validator with a FeasibilityProjector for What-If based pre-checks
    pub fn with_projector(
        constraint_engine: Arc<ConstraintEngine>,
        safety_gateway: Arc<SafetyGateway>,
        interlocking_engine: InterlockingRuleEngine,
        projector: Arc<FeasibilityProjector>,
    ) -> Self {
        Self {
            constraint_engine,
            safety_gateway,
            interlocking_engine,
            projector: Some(projector),
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
        // When a FeasibilityProjector is available, use What-If analysis for richer checks;
        // otherwise fall back to keyword-based constraint engine check.
        let (feasible, worsened, new_violations) = if let Some(ref projector) = self.projector {
            // Try to infer a StructuredAction from the description for projector-based check
            if let Some(action) = self.infer_structured_action(action_description, target_device_id) {
                let projection = projector.project(&action);
                match &projection {
                    ProjectionResult::Feasible(_) => (true, vec![], vec![]),
                    ProjectionResult::Projected { modifications, .. } => {
                        // Projected means the action was adjusted but is now feasible
                        let worsened: Vec<String> = modifications.iter()
                            .map(|m| format!("{} adjusted: {}", m.parameter, m.reason))
                            .collect();
                        (true, worsened, vec![])
                    }
                    ProjectionResult::Infeasible { violated_constraints, .. } => {
                        (false, violated_constraints.clone(), violated_constraints.clone())
                    }
                }
            } else {
                // Cannot infer StructuredAction — fall back to keyword check
                let feasibility = self.constraint_engine.check_action_feasibility(action_description);
                (feasibility.feasible, feasibility.worsened_violations, feasibility.new_violations)
            }
        } else {
            let feasibility = self.constraint_engine.check_action_feasibility(action_description);
            (feasibility.feasible, feasibility.worsened_violations, feasibility.new_violations)
        };

        if !feasible && !effective_authority.can_bypass_checks() {
            return ActionVerdict::Rejected(format!(
                "Action would worsen existing violations: {:?}",
                worsened
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

        // Step 5: Safety gateway check — validate action through registered safety rules
        // Convert action to Command and run through SafetyGateway's safety checks
        if let Some(cmd) = self.action_to_command(action_description, target_device_id) {
            if let Err(e) = self.safety_gateway.validate_command(&cmd) {
                if !effective_authority.can_execute_high_risk() {
                    return ActionVerdict::Rejected(format!(
                        "Safety gateway violation: {}", e
                    ));
                }
                // High-risk authority can proceed but needs approval
                return ActionVerdict::PendingApproval {
                    approver_level: AuthorityLevel::Supervisor,
                    reason: format!("Safety gateway override required: {}", e),
                };
            }
        }

        // Step 5b: High-risk action approval check (keyword-based supplement)
        if self.is_high_risk_action(action_description) && !effective_authority.can_execute_high_risk() {
            return ActionVerdict::PendingApproval {
                approver_level: AuthorityLevel::Supervisor,
                reason: format!("High-risk action requires {:?} approval: {}", AuthorityLevel::Supervisor, action_description),
            };
        }

        // Step 6: Emergency bypass for feasible but risky actions
        if !new_violations.is_empty() && effective_authority.can_bypass_checks() {
            return ActionVerdict::EmergencyBypassed {
                bypassed_checks: vec!["constraint_pre_check".to_string()],
                reason: format!("Emergency override: action may cause {:?}", new_violations),
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

    /// Infer a StructuredAction from the action description for projector-based checks.
    /// Returns None if the description cannot be mapped to a StructuredAction.
    fn infer_structured_action(&self, description: &str, target_device_id: Option<u64>) -> Option<StructuredAction> {
        let desc_lower = description.to_lowercase();
        let device_id = target_device_id.unwrap_or(0);

        // Generator setpoint
        if desc_lower.contains("generator") || desc_lower.contains("发电机") {
            // Try to extract target_mw from description (e.g., "100.0 MW")
            let target_mw = extract_mw_value(description).unwrap_or(0.0);
            return Some(StructuredAction::StartGenerator {
                gen_id: device_id,
                target_mw,
            });
        }

        // Load shedding
        if desc_lower.contains("load shed") || desc_lower.contains("切负荷") {
            let amount_mw = extract_mw_value(description).unwrap_or(0.0);
            return Some(StructuredAction::ShedLoad {
                zone_id: device_id as u32,
                amount_mw,
            });
        }

        // Fault isolation
        if desc_lower.contains("isolate fault") || desc_lower.contains("隔离故障") {
            return Some(StructuredAction::IsolateFault {
                upstream_switch: device_id,
                downstream_switch: device_id + 1,
            });
        }

        // Tie switch
        if desc_lower.contains("close tie") || desc_lower.contains("合环") {
            return Some(StructuredAction::CloseTieSwitch {
                switch_id: device_id,
            });
        }

        // Device operation (breaker, disconnector, etc.)
        if desc_lower.contains("breaker") || desc_lower.contains("断路器")
            || desc_lower.contains("disconnector") || desc_lower.contains("隔离开关")
            || desc_lower.contains("switch") || desc_lower.contains("开关")
        {
            let operation = if desc_lower.contains("close") || desc_lower.contains("合") {
                "close".to_string()
            } else {
                "open".to_string()
            };
            return Some(StructuredAction::ExecuteDevice {
                device_id,
                operation,
                value: 1.0,
            });
        }

        // Cannot infer
        None
    }

    /// Check if an action is high-risk (requires supervisor approval)
    fn is_high_risk_action(&self, description: &str) -> bool {
        let desc_lower = description.to_lowercase();
        desc_lower.contains("load shed")
            || desc_lower.contains("切负荷")
            || desc_lower.contains("shed ") && desc_lower.contains("mw")
            || desc_lower.contains("system separation")
            || desc_lower.contains("系统解列")
            || desc_lower.contains("island")
            || desc_lower.contains("孤岛")
            || desc_lower.contains("black start")
            || desc_lower.contains("黑启动")
    }

    /// Convert an action description to a Command for SafetyGateway validation.
    /// Returns None if the description cannot be mapped to a Command.
    fn action_to_command(&self, description: &str, target_device_id: Option<u64>) -> Option<Command> {
        let desc_lower = description.to_lowercase();
        let device_id = target_device_id.unwrap_or(0);

        let (cmd_type, priority) = if desc_lower.contains("generator") || desc_lower.contains("发电机") {
            let target_mw = extract_mw_value(description).unwrap_or(0.0);
            let mut cmd = Command::new(CommandType::GeneratorSetpoint, device_id, CommandPriority::Normal, "validator");
            cmd.parameters.insert("target_mw".to_string(), target_mw);
            return Some(cmd);
        } else if desc_lower.contains("load shed") || desc_lower.contains("切负荷") {
            let amount_mw = extract_mw_value(description).unwrap_or(0.0);
            let mut cmd = Command::new(CommandType::LoadShedding, device_id, CommandPriority::High, "validator");
            cmd.parameters.insert("amount_mw".to_string(), amount_mw);
            return Some(cmd);
        } else if desc_lower.contains("close breaker") || desc_lower.contains("合断路器")
            || desc_lower.contains("open breaker") || desc_lower.contains("断断路器") || desc_lower.contains("分断路器") {
            (CommandType::SwitchOperation, CommandPriority::High)
        } else if desc_lower.contains("close disconnector") || desc_lower.contains("合隔离开关")
            || desc_lower.contains("open disconnector") || desc_lower.contains("拉隔离开关") {
            (CommandType::SwitchOperation, CommandPriority::Normal)
        } else if desc_lower.contains("capacitor") || desc_lower.contains("电容") {
            (CommandType::CapacitorSwitch, CommandPriority::Normal)
        } else if desc_lower.contains("transformer") || desc_lower.contains("变压器") {
            (CommandType::TransformerTap, CommandPriority::Normal)
        } else {
            return None;
        };

        Some(Command::new(cmd_type, device_id, priority, "validator"))
    }
}

/// Extract a MW value from a description string (e.g., "100.0 MW" or "50.0MW")
fn extract_mw_value(description: &str) -> Option<f64> {
    // Find "MW" or "mw" in the string and parse the number before it
    let upper = description.to_uppercase();
    let idx = upper.find("MW")?;
    let prefix = &description[..idx];
    // Find the last number in the prefix
    let num_start = prefix.rfind(|c: char| !c.is_ascii_digit() && c != '.')
        .map(|i| i + 1)
        .unwrap_or(0);
    let num_str = prefix[num_start..].trim();
    num_str.parse().ok()
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
