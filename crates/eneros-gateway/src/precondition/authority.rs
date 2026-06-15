use super::PreConditionChecker;
use crate::pipeline_types::{DecisionContext, PreConditionCheck, PreConditionResult};
use eneros_core::StructuredAction;

impl PreConditionChecker {
    pub(super) fn check_authority(
        &self,
        action: &StructuredAction,
        ctx: &DecisionContext,
        result: &mut PreConditionResult,
    ) {
        let effective = ctx.effective_authority();
        let can_execute = effective.can_execute_commands();
        let is_high_risk = self.is_high_risk_action(action);
        let can_high_risk = effective.can_execute_high_risk();

        if !can_execute {
            result.add_check(PreConditionCheck {
                name: "authority_check".to_string(),
                passed: false,
                description: format!("Authority {:?} cannot execute commands", ctx.authority),
                failure_reason: Some(format!(
                    "Insufficient authority: {:?} cannot execute any commands",
                    ctx.authority
                )),
            });
        } else if is_high_risk && !can_high_risk {
            result.add_check(PreConditionCheck {
                name: "authority_high_risk".to_string(),
                passed: false,
                description: "Action is high-risk, requires Supervisor or Emergency authority"
                    .to_string(),
                failure_reason: Some(format!(
                    "High-risk action requires Supervisor+ authority, got {:?}",
                    ctx.authority
                )),
            });
        } else {
            result.add_check(PreConditionCheck {
                name: "authority_check".to_string(),
                passed: true,
                description: format!("Authority {:?} sufficient for this action", effective),
                failure_reason: None,
            });
        }
    }

    fn is_high_risk_action(&self, action: &StructuredAction) -> bool {
        matches!(
            action,
            StructuredAction::ShedLoad { .. } | StructuredAction::IsolateFault { .. }
        )
    }
}
