use super::PreConditionChecker;
use crate::pipeline_types::{DecisionContext, PreConditionCheck, PreConditionResult};
use eneros_core::{StructuredAction, SystemOperatingState};

impl PreConditionChecker {
    pub(super) fn check_system_state_compatibility(
        &self,
        action: &StructuredAction,
        ctx: &DecisionContext,
        result: &mut PreConditionResult,
    ) {
        if ctx.system_state == SystemOperatingState::Blackout {
            match action {
                StructuredAction::StartGenerator { .. } => {
                    result.add_check(PreConditionCheck {
                        name: "blackout_compatibility".to_string(),
                        passed: true,
                        description: "Generator start allowed during blackout (black start)"
                            .to_string(),
                        failure_reason: None,
                    });
                }
                StructuredAction::IsolateFault { .. } => {
                    result.add_check(PreConditionCheck {
                        name: "blackout_compatibility".to_string(),
                        passed: true,
                        description: "Fault isolation allowed during blackout".to_string(),
                        failure_reason: None,
                    });
                }
                StructuredAction::NotifyAgent { .. } => {
                    result.add_check(PreConditionCheck {
                        name: "blackout_compatibility".to_string(),
                        passed: true,
                        description: "NotifyAgent allowed during blackout".to_string(),
                        failure_reason: None,
                    });
                }
                _ => {
                    result.add_check(PreConditionCheck {
                        name: "blackout_compatibility".to_string(),
                        passed: false,
                        description: "Only black start and fault isolation allowed during blackout"
                            .to_string(),
                        failure_reason: Some(
                            "System is in BLACKOUT - only generator start and fault isolation are allowed".to_string(),
                        ),
                    });
                }
            }
        }
    }
}
