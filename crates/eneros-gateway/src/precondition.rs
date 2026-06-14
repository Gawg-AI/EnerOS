use eneros_core::{StructuredAction, SystemOperatingState};
use crate::pipeline_types::{DecisionContext, PreConditionCheck, PreConditionResult};

/// Pre-condition checker — validates that an action can be attempted.
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

        // 1. Authority check
        self.check_authority(action, ctx, &mut result);

        // 2. Jurisdiction check
        self.check_jurisdiction(action, ctx, &mut result);

        // 3. Action-specific pre-conditions
        match action {
            StructuredAction::StartGenerator { gen_id, target_mw } => {
                self.check_generator_preconditions(*gen_id, *target_mw, ctx, &mut result);
            }
            StructuredAction::ShedLoad { zone_id, amount_mw } => {
                self.check_shed_load_preconditions(*zone_id, *amount_mw, ctx, &mut result);
            }
            StructuredAction::IsolateFault { upstream_switch, downstream_switch } => {
                self.check_isolate_fault_preconditions(*upstream_switch, *downstream_switch, ctx, &mut result);
            }
            StructuredAction::CloseTieSwitch { switch_id } => {
                self.check_close_tie_preconditions(*switch_id, ctx, &mut result);
            }
            StructuredAction::ExecuteDevice { device_id, operation, value } => {
                self.check_device_preconditions(*device_id, operation, *value, ctx, &mut result);
            }
            StructuredAction::NotifyAgent { .. } => {
                // NotifyAgent always passes pre-conditions
                result.add_check(PreConditionCheck {
                    name: "notify_precondition".to_string(),
                    passed: true,
                    description: "NotifyAgent has no pre-conditions".to_string(),
                    failure_reason: None,
                });
            }
        }

        // 4. System state compatibility check
        self.check_system_state_compatibility(action, ctx, &mut result);

        result
    }

    fn check_authority(&self, action: &StructuredAction, ctx: &DecisionContext, result: &mut PreConditionResult) {
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
                description: "Action is high-risk, requires Supervisor or Emergency authority".to_string(),
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

    fn check_jurisdiction(&self, action: &StructuredAction, ctx: &DecisionContext, result: &mut PreConditionResult) {
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

    fn check_generator_preconditions(
        &self,
        gen_id: u64,
        target_mw: f64,
        ctx: &DecisionContext,
        result: &mut PreConditionResult,
    ) {
        // Check: target_mw must be non-negative
        if target_mw < 0.0 {
            result.add_check(PreConditionCheck {
                name: "generator_target_positive".to_string(),
                passed: false,
                description: format!("Generator {} target MW must be non-negative", gen_id),
                failure_reason: Some(format!("Generator {} target_mw={:.1} is negative", gen_id, target_mw)),
            });
        } else {
            result.add_check(PreConditionCheck {
                name: "generator_target_positive".to_string(),
                passed: true,
                description: format!("Generator {} target MW={:.1} is non-negative", gen_id, target_mw),
                failure_reason: None,
            });
        }

        // Check: frequency must be within operable range
        if let Some(ref obs) = ctx.observation {
            if obs.frequency_hz < 47.5 || obs.frequency_hz > 51.5 {
                result.add_check(PreConditionCheck {
                    name: "generator_frequency_operable".to_string(),
                    passed: false,
                    description: format!("Frequency {:.2} Hz outside operable range [47.5, 51.5]", obs.frequency_hz),
                    failure_reason: Some(format!(
                        "System frequency {:.2} Hz is outside generator operable range",
                        obs.frequency_hz
                    )),
                });
            } else {
                result.add_check(PreConditionCheck {
                    name: "generator_frequency_operable".to_string(),
                    passed: true,
                    description: format!("Frequency {:.2} Hz within operable range", obs.frequency_hz),
                    failure_reason: None,
                });
            }
        }
    }

    fn check_shed_load_preconditions(
        &self,
        _zone_id: u32,
        amount_mw: f64,
        ctx: &DecisionContext,
        result: &mut PreConditionResult,
    ) {
        // Check: amount must be positive
        if amount_mw <= 0.0 {
            result.add_check(PreConditionCheck {
                name: "shed_amount_positive".to_string(),
                passed: false,
                description: format!("Shed amount must be positive, got {:.1} MW", amount_mw),
                failure_reason: Some(format!("Load shedding amount {:.1} MW is not positive", amount_mw)),
            });
        } else {
            result.add_check(PreConditionCheck {
                name: "shed_amount_positive".to_string(),
                passed: true,
                description: format!("Shed amount {:.1} MW is positive", amount_mw),
                failure_reason: None,
            });
        }

        // Check: shedding amount must not exceed max fraction of total load
        if let Some(ref obs) = ctx.observation {
            if obs.total_load_mw > 0.0 {
                let fraction = amount_mw / obs.total_load_mw;
                if fraction > self.max_shed_fraction {
                    result.add_check(PreConditionCheck {
                        name: "shed_amount_fraction".to_string(),
                        passed: false,
                        description: format!(
                            "Shed {:.1}MW is {:.0}% of total load {:.1}MW (max {:.0}%)",
                            amount_mw, fraction * 100.0, obs.total_load_mw, self.max_shed_fraction * 100.0
                        ),
                        failure_reason: Some(format!(
                            "Load shedding {:.1}MW ({:.0}%) exceeds maximum {:.0}% of total load",
                            amount_mw, fraction * 100.0, self.max_shed_fraction * 100.0
                        )),
                    });
                } else {
                    result.add_check(PreConditionCheck {
                        name: "shed_amount_fraction".to_string(),
                        passed: true,
                        description: format!(
                            "Shed {:.1}MW is {:.0}% of total load (within limit)",
                            amount_mw, fraction * 100.0
                        ),
                        failure_reason: None,
                    });
                }
            }
        }

        // Check: frequency must not be critically low before shedding
        if let Some(ref obs) = ctx.observation {
            if obs.frequency_hz < self.min_frequency_for_shedding {
                result.add_check(PreConditionCheck {
                    name: "shed_frequency_critical".to_string(),
                    passed: false,
                    description: format!(
                        "Frequency {:.2}Hz below minimum {:.2}Hz for load shedding",
                        obs.frequency_hz, self.min_frequency_for_shedding
                    ),
                    failure_reason: Some(format!(
                        "Cannot shed load when frequency {:.2}Hz is below critical threshold {:.2}Hz",
                        obs.frequency_hz, self.min_frequency_for_shedding
                    )),
                });
            }
        }
    }

    fn check_isolate_fault_preconditions(
        &self,
        upstream_switch: u64,
        downstream_switch: u64,
        ctx: &DecisionContext,
        result: &mut PreConditionResult,
    ) {
        // Check: switches must be different
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

        // Check: device states must be available for interlocking
        if ctx.device_states.is_none() {
            result.add_check(PreConditionCheck {
                name: "isolate_device_states_available".to_string(),
                passed: false,
                description: "Device states required for fault isolation interlocking check".to_string(),
                failure_reason: Some("Device states not available — cannot verify interlocking conditions".to_string()),
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

    fn check_close_tie_preconditions(
        &self,
        switch_id: u64,
        ctx: &DecisionContext,
        result: &mut PreConditionResult,
    ) {
        // Check: voltage levels must be adequate on both sides
        if let Some(ref obs) = ctx.observation {
            let low_voltage_buses = obs.low_voltage_buses(self.min_voltage_for_tie_close);
            if !low_voltage_buses.is_empty() {
                result.add_check(PreConditionCheck {
                    name: "tie_close_voltage_adequate".to_string(),
                    passed: false,
                    description: format!(
                        "{} buses below minimum voltage {:.2}pu for tie close",
                        low_voltage_buses.len(), self.min_voltage_for_tie_close
                    ),
                    failure_reason: Some(format!(
                        "Cannot close tie switch {} — {} buses have voltage below {:.2}pu",
                        switch_id, low_voltage_buses.len(), self.min_voltage_for_tie_close
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

    fn check_device_preconditions(
        &self,
        device_id: u64,
        operation: &str,
        _value: f64,
        ctx: &DecisionContext,
        result: &mut PreConditionResult,
    ) {
        // Check: device must be in a state that allows the operation
        if let Some(ref states) = ctx.device_states {
            // Check ground switch status for closing operations
            let is_close = operation.contains("close") || operation.contains("合");
            if is_close {
                let ground_applied = states.ground_switch_states.values().any(|&v| v);
                if ground_applied {
                    result.add_check(PreConditionCheck {
                        name: "device_ground_check".to_string(),
                        passed: false,
                        description: "Ground switch is applied, cannot close device".to_string(),
                        failure_reason: Some(format!(
                            "Cannot close device {} — ground switch is applied",
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
            description: format!("Device {} operation '{}' pre-conditions satisfied", device_id, operation),
            failure_reason: None,
        });
    }

    fn check_system_state_compatibility(
        &self,
        action: &StructuredAction,
        ctx: &DecisionContext,
        result: &mut PreConditionResult,
    ) {
        // Blackout state: only specific actions are allowed
        if ctx.system_state == SystemOperatingState::Blackout {
            match action {
                StructuredAction::StartGenerator { .. } => {
                    // Black start is allowed
                    result.add_check(PreConditionCheck {
                        name: "blackout_compatibility".to_string(),
                        passed: true,
                        description: "Generator start allowed during blackout (black start)".to_string(),
                        failure_reason: None,
                    });
                }
                StructuredAction::IsolateFault { .. } => {
                    // Fault isolation allowed during blackout
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
                        description: "Only black start and fault isolation allowed during blackout".to_string(),
                        failure_reason: Some(
                            "System is in BLACKOUT — only generator start and fault isolation are allowed".to_string()
                        ),
                    });
                }
            }
        }
    }

    fn is_high_risk_action(&self, action: &StructuredAction) -> bool {
        matches!(
            action,
            StructuredAction::ShedLoad { .. } | StructuredAction::IsolateFault { .. }
        )
    }
}

impl Default for PreConditionChecker {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract zone and device targets from a StructuredAction
fn extract_targets(action: &StructuredAction) -> (Option<u32>, Option<u64>) {
    match action {
        StructuredAction::ShedLoad { zone_id, .. } => (Some(*zone_id), None),
        StructuredAction::StartGenerator { gen_id, .. } => (None, Some(*gen_id)),
        StructuredAction::ExecuteDevice { device_id, .. } => (None, Some(*device_id)),
        StructuredAction::IsolateFault { upstream_switch, .. } => (None, Some(*upstream_switch)),
        StructuredAction::CloseTieSwitch { switch_id } => (None, Some(*switch_id)),
        StructuredAction::NotifyAgent { .. } => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_core::{AuthorityLevel, Jurisdiction, PowerObservation};
    use crate::interlocking::DeviceStates;

    fn make_ctx() -> DecisionContext {
        DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        )
    }

    #[test]
    fn test_precondition_generator_passes() {
        let checker = PreConditionChecker::new();
        let ctx = make_ctx();
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let result = checker.check(&action, &ctx);
        assert!(result.satisfied, "Pre-conditions should pass: {:?}", result.failure_summary);
    }

    #[test]
    fn test_precondition_observer_rejected() {
        let checker = PreConditionChecker::new();
        let ctx = DecisionContext::new(
            AuthorityLevel::Observer,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let result = checker.check(&action, &ctx);
        assert!(!result.satisfied);
        assert!(result.failure_summary.iter().any(|r| r.contains("Insufficient authority")));
    }

    #[test]
    fn test_precondition_negative_generator_target() {
        let checker = PreConditionChecker::new();
        let ctx = make_ctx();
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: -50.0 };
        let result = checker.check(&action, &ctx);
        assert!(!result.satisfied);
    }

    #[test]
    fn test_precondition_shed_load_fraction_exceeded() {
        let checker = PreConditionChecker::new();
        let mut obs = PowerObservation::empty();
        obs.total_load_mw = 100.0;
        let ctx = make_ctx().with_observation(obs);
        let action = StructuredAction::ShedLoad { zone_id: 1, amount_mw: 60.0 };
        let result = checker.check(&action, &ctx);
        assert!(!result.satisfied);
        assert!(result.failure_summary.iter().any(|r| r.contains("exceeds maximum")));
    }

    #[test]
    fn test_precondition_shed_load_within_fraction() {
        let checker = PreConditionChecker::new();
        let mut obs = PowerObservation::empty();
        obs.total_load_mw = 100.0;
        let ctx = make_ctx().with_observation(obs);
        let action = StructuredAction::ShedLoad { zone_id: 1, amount_mw: 30.0 };
        let result = checker.check(&action, &ctx);
        assert!(result.satisfied, "Should pass: {:?}", result.failure_summary);
    }

    #[test]
    fn test_precondition_isolate_same_switch_rejected() {
        let checker = PreConditionChecker::new();
        let ctx = make_ctx().with_device_states(DeviceStates::default());
        let action = StructuredAction::IsolateFault {
            upstream_switch: 5,
            downstream_switch: 5,
        };
        let result = checker.check(&action, &ctx);
        assert!(!result.satisfied);
    }

    #[test]
    fn test_precondition_blackout_restricts_actions() {
        let checker = PreConditionChecker::new();
        let ctx = DecisionContext::new(
            AuthorityLevel::Emergency,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Blackout,
        );
        // ShedLoad is NOT allowed during blackout
        let action = StructuredAction::ShedLoad { zone_id: 1, amount_mw: 10.0 };
        let result = checker.check(&action, &ctx);
        assert!(!result.satisfied);
    }

    #[test]
    fn test_precondition_blackout_allows_generator_start() {
        let checker = PreConditionChecker::new();
        let ctx = DecisionContext::new(
            AuthorityLevel::Emergency,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Blackout,
        );
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let result = checker.check(&action, &ctx);
        assert!(result.satisfied, "Black start should be allowed: {:?}", result.failure_summary);
    }

    #[test]
    fn test_precondition_notify_always_passes() {
        let checker = PreConditionChecker::new();
        let ctx = make_ctx();
        let action = StructuredAction::NotifyAgent {
            agent_id: "dispatch".to_string(),
            message: "test".to_string(),
        };
        let result = checker.check(&action, &ctx);
        assert!(result.satisfied);
    }

    #[test]
    fn test_precondition_jurisdiction_zone_rejected() {
        let checker = PreConditionChecker::new();
        let ctx = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::for_zones(vec![1, 2]),
            SystemOperatingState::Normal,
        );
        let action = StructuredAction::ShedLoad { zone_id: 5, amount_mw: 10.0 };
        let result = checker.check(&action, &ctx);
        assert!(!result.satisfied);
    }

    #[test]
    fn test_precondition_ground_switch_blocks_close() {
        let checker = PreConditionChecker::new();
        let mut states = DeviceStates::default();
        states.ground_switch_states.insert(10, true);
        let ctx = make_ctx().with_device_states(states);
        let action = StructuredAction::ExecuteDevice {
            device_id: 1,
            operation: "close".to_string(),
            value: 1.0,
        };
        let result = checker.check(&action, &ctx);
        assert!(!result.satisfied);
    }
}
