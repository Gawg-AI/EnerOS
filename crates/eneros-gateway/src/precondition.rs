#[path = "precondition/authority.rs"]
mod authority;
#[path = "precondition/device.rs"]
mod device;
#[path = "precondition/generator.rs"]
mod generator;
#[path = "precondition/jurisdiction.rs"]
mod jurisdiction;
#[path = "precondition/load.rs"]
mod load;
#[path = "precondition/system_state.rs"]
mod system_state;

#[cfg(test)]
#[path = "precondition/tests.rs"]
mod tests;

use crate::pipeline_types::{DecisionContext, PreConditionCheck, PreConditionResult};
use eneros_core::StructuredAction;

/// Pre-condition checker 鈥?validates that an action can be attempted.
///
/// Checks are deterministic and based solely on the current system state.
/// They do NOT involve What-If simulation (that's the projector's job).
/// Pre-conditions answer: "Is it physically/legal possible to even try this action?"
pub struct PreConditionChecker {
    /// Minimum frequency threshold for load shedding (Hz)
    pub min_frequency_for_shedding: f64,
    /// Maximum load shedding amount as fraction of total load
    pub max_shed_fraction: f64,
    /// Minimum voltage threshold for closing tie switch (p.u.)
    pub min_voltage_for_tie_close: f64,
    /// Maximum voltage difference for closing tie switch (p.u.)
    pub max_voltage_diff_for_tie_close: f64,
}

impl PreConditionChecker {
    pub fn new() -> Self {
        Self {
            min_frequency_for_shedding: 49.0,
            max_shed_fraction: 0.5,
            min_voltage_for_tie_close: 0.85,
            max_voltage_diff_for_tie_close: 0.15,
        }
    }

    /// Check all pre-conditions for an action
    pub fn check(&self, action: &StructuredAction, ctx: &DecisionContext) -> PreConditionResult {
        let mut result = PreConditionResult::passed();

        self.check_authority(action, ctx, &mut result);
        self.check_jurisdiction(action, ctx, &mut result);

        match action {
            StructuredAction::StartGenerator { gen_id, target_mw } => {
                self.check_generator_preconditions(*gen_id, *target_mw, ctx, &mut result);
            }
            StructuredAction::ShedLoad { zone_id, amount_mw } => {
                self.check_shed_load_preconditions(*zone_id, *amount_mw, ctx, &mut result);
            }
            StructuredAction::IsolateFault {
                upstream_switch,
                downstream_switch,
            } => {
                self.check_isolate_fault_preconditions(
                    *upstream_switch,
                    *downstream_switch,
                    ctx,
                    &mut result,
                );
            }
            StructuredAction::CloseTieSwitch { switch_id } => {
                self.check_close_tie_preconditions(*switch_id, ctx, &mut result);
            }
            StructuredAction::ExecuteDevice {
                device_id,
                operation,
                value,
            } => {
                self.check_device_preconditions(*device_id, operation, *value, ctx, &mut result);
            }
            StructuredAction::NotifyAgent { .. } => {
                result.add_check(PreConditionCheck {
                    name: "notify_precondition".to_string(),
                    passed: true,
                    description: "NotifyAgent has no pre-conditions".to_string(),
                    failure_reason: None,
                });
            }
        }

        self.check_system_state_compatibility(action, ctx, &mut result);

        result
    }
}

impl Default for PreConditionChecker {
    fn default() -> Self {
        Self::new()
    }
}
