use super::PreConditionChecker;
use crate::pipeline_types::{DecisionContext, PreConditionCheck, PreConditionResult};
use eneros_core::StructuredAction;

impl PreConditionChecker {
    pub(super) fn check_jurisdiction(
        &self,
        action: &StructuredAction,
        ctx: &DecisionContext,
        result: &mut PreConditionResult,
    ) {
        let (zone_id, device_id) = extract_targets(action);

        if let Some(zid) = zone_id {
            if !ctx.jurisdiction.contains_zone(zid) {
                result.add_check(PreConditionCheck {
                    name: "jurisdiction_zone".to_string(),
                    passed: false,
                    description: format!("Zone {} check", zid),
                    failure_reason: Some(format!("Zone {} is outside agent's jurisdiction", zid)),
                });
                return;
            }
        }

        if let Some(did) = device_id {
            if !ctx.jurisdiction.contains_device(did) {
                result.add_check(PreConditionCheck {
                    name: "jurisdiction_device".to_string(),
                    passed: false,
                    description: format!("Device {} check", did),
                    failure_reason: Some(format!("Device {} is outside agent's jurisdiction", did)),
                });
                return;
            }
        }

        result.add_check(PreConditionCheck {
            name: "jurisdiction_check".to_string(),
            passed: true,
            description: "Action target within jurisdiction".to_string(),
            failure_reason: None,
        });
    }
}

fn extract_targets(action: &StructuredAction) -> (Option<u32>, Option<u64>) {
    match action {
        StructuredAction::ShedLoad { zone_id, .. } => (Some(*zone_id), None),
        StructuredAction::StartGenerator { gen_id, .. } => (None, Some(*gen_id)),
        StructuredAction::ExecuteDevice { device_id, .. } => (None, Some(*device_id)),
        StructuredAction::IsolateFault {
            upstream_switch, ..
        } => (None, Some(*upstream_switch)),
        StructuredAction::CloseTieSwitch { switch_id } => (None, Some(*switch_id)),
        StructuredAction::NotifyAgent { .. } => (None, None),
    }
}
