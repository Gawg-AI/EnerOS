use super::PreConditionChecker;
use crate::pipeline_types::{DecisionContext, PreConditionCheck, PreConditionResult};

impl PreConditionChecker {
    pub(super) fn check_isolate_fault_preconditions(
        &self,
        upstream_switch: u64,
        downstream_switch: u64,
        ctx: &DecisionContext,
        result: &mut PreConditionResult,
    ) {
        if upstream_switch == downstream_switch {
            result.add_check(PreConditionCheck {
                name: "isolate_switches_different".to_string(),
                passed: false,
                description: "Upstream and downstream switches must be different".to_string(),
                failure_reason: Some(format!(
                    "Upstream switch {} and downstream switch {} are the same",
                    upstream_switch, downstream_switch
                )),
            });
        } else {
            result.add_check(PreConditionCheck {
                name: "isolate_switches_different".to_string(),
                passed: true,
                description: "Upstream and downstream switches are different".to_string(),
                failure_reason: None,
            });
        }

        if ctx.device_states.is_none() {
            result.add_check(PreConditionCheck {
                name: "isolate_device_states_available".to_string(),
                passed: false,
                description: "Device states required for fault isolation interlocking check"
                    .to_string(),
                failure_reason: Some(
                    "Device states not available - cannot verify interlocking conditions"
                        .to_string(),
                ),
            });
        } else {
            result.add_check(PreConditionCheck {
                name: "isolate_device_states_available".to_string(),
                passed: true,
                description: "Device states available for interlocking check".to_string(),
                failure_reason: None,
            });
        }
    }

    pub(super) fn check_close_tie_preconditions(
        &self,
        switch_id: u64,
        ctx: &DecisionContext,
        result: &mut PreConditionResult,
    ) {
        if let Some(ref obs) = ctx.observation {
            let low_voltage_buses = obs.low_voltage_buses(self.min_voltage_for_tie_close);
            if !low_voltage_buses.is_empty() {
                result.add_check(PreConditionCheck {
                    name: "tie_close_voltage_adequate".to_string(),
                    passed: false,
                    description: format!(
                        "{} buses below minimum voltage {:.2}pu for tie close",
                        low_voltage_buses.len(),
                        self.min_voltage_for_tie_close
                    ),
                    failure_reason: Some(format!(
                        "Cannot close tie switch {} - {} buses have voltage below {:.2}pu",
                        switch_id,
                        low_voltage_buses.len(),
                        self.min_voltage_for_tie_close
                    )),
                });
            } else {
                result.add_check(PreConditionCheck {
                    name: "tie_close_voltage_adequate".to_string(),
                    passed: true,
                    description: "All bus voltages adequate for tie close".to_string(),
                    failure_reason: None,
                });
            }
        }
    }

    pub(super) fn check_device_preconditions(
        &self,
        device_id: u64,
        operation: &str,
        value: f64,
        ctx: &DecisionContext,
        result: &mut PreConditionResult,
    ) {
        if !value.is_finite() {
            result.add_check(PreConditionCheck {
                name: "device_value_finite".to_string(),
                passed: false,
                description: format!(
                    "Device {} operation '{}' value must be finite",
                    device_id, operation
                ),
                failure_reason: Some(format!(
                    "Device {} operation '{}' value={} is not finite",
                    device_id, operation, value
                )),
            });
            return;
        }

        if let Some(ref states) = ctx.device_states {
            let is_close = operation.contains("close") || operation.contains('\u{5408}');
            if is_close {
                let ground_applied = states.ground_switch_states.values().any(|&v| v);
                if ground_applied {
                    result.add_check(PreConditionCheck {
                        name: "device_ground_check".to_string(),
                        passed: false,
                        description: "Ground switch is applied, cannot close device".to_string(),
                        failure_reason: Some(format!(
                            "Cannot close device {} - ground switch is applied",
                            device_id
                        )),
                    });
                    return;
                }
            }
        }

        result.add_check(PreConditionCheck {
            name: "device_precondition".to_string(),
            passed: true,
            description: format!(
                "Device {} operation '{}' pre-conditions satisfied",
                device_id, operation
            ),
            failure_reason: None,
        });
    }
}
