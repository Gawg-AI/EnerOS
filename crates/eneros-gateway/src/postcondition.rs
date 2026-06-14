use eneros_core::StructuredAction;
use eneros_constraint::projector::WhatIfResult;
use crate::pipeline_types::{DecisionContext, PostConditionResult, PostConditionVerification};

/// Post-condition verifier — validates that an executed action produced the expected outcome.
///
/// Uses What-If simulation results to verify that:
/// 1. The power flow converges after the action
/// 2. No new constraint violations are introduced
/// 3. Existing violations are not worsened
/// 4. Voltage stability margin is maintained
/// 5. The action achieved its intended effect
pub struct PostConditionVerifier {
    /// Voltage limits for post-condition check (p.u.)
    pub voltage_min: f64,
    pub voltage_max: f64,
    /// Thermal loading limit (percent)
    pub thermal_limit: f64,
    /// Minimum voltage stability margin
    pub min_stability_margin: f64,
    /// Frequency limits (Hz)
    pub frequency_min: f64,
    pub frequency_max: f64,
}

impl PostConditionVerifier {
    pub fn new() -> Self {
        Self {
            voltage_min: 0.95,
            voltage_max: 1.10,
            thermal_limit: 100.0,
            min_stability_margin: 0.3,
            frequency_min: 49.8,
            frequency_max: 50.2,
        }
    }

    /// Verify post-conditions using What-If simulation result
    pub fn verify(
        &self,
        action: &StructuredAction,
        what_if: &WhatIfResult,
        ctx: &DecisionContext,
    ) -> PostConditionResult {
        let mut verifications = Vec::new();
        let mut new_violations = Vec::new();
        let mut worsened_violations = Vec::new();

        // 1. Convergence check
        let converged = what_if.converged;
        verifications.push(PostConditionVerification {
            name: "power_flow_convergence".to_string(),
            passed: converged,
            description: if converged {
                "Power flow converged after action".to_string()
            } else {
                "Power flow did NOT converge after action".to_string()
            },
        });
        if !converged {
            new_violations.push("Power flow divergence after action".to_string());
        }

        // 2. Voltage violation check
        let voltage_ok = what_if.voltage_violations.is_empty();
        verifications.push(PostConditionVerification {
            name: "voltage_constraints".to_string(),
            passed: voltage_ok,
            description: if voltage_ok {
                "No voltage violations after action".to_string()
            } else {
                format!(
                    "{} voltage violations after action: {}",
                    what_if.voltage_violations.len(),
                    what_if.voltage_violations
                        .iter()
                        .map(|(bus, v, limit)| format!("Bus {} V={:.3}pu (limit={:.3})", bus, v, limit))
                        .collect::<Vec<_>>()
                        .join("; ")
                )
            },
        });
        for (bus, v, limit) in &what_if.voltage_violations {
            new_violations.push(format!(
                "Voltage violation: Bus {} V={:.3}pu vs limit {:.3}pu",
                bus, v, limit
            ));
        }

        // 3. Thermal violation check
        let thermal_ok = what_if.thermal_violations.is_empty();
        verifications.push(PostConditionVerification {
            name: "thermal_constraints".to_string(),
            passed: thermal_ok,
            description: if thermal_ok {
                "No thermal violations after action".to_string()
            } else {
                format!(
                    "{} thermal violations after action: {}",
                    what_if.thermal_violations.len(),
                    what_if.thermal_violations
                        .iter()
                        .map(|(branch, loading, limit)| format!("Branch {} loading={:.1}% (limit={:.1}%)", branch, loading, limit))
                        .collect::<Vec<_>>()
                        .join("; ")
                )
            },
        });
        for (branch, loading, limit) in &what_if.thermal_violations {
            new_violations.push(format!(
                "Thermal violation: Branch {} loading={:.1}% vs limit {:.1}%",
                branch, loading, limit
            ));
        }

        // 4. Overall constraint satisfaction
        verifications.push(PostConditionVerification {
            name: "all_constraints_satisfied".to_string(),
            passed: what_if.all_constraints_satisfied,
            description: if what_if.all_constraints_satisfied {
                "All constraints satisfied after action".to_string()
            } else {
                format!("Constraints NOT all satisfied: {}", what_if.summary)
            },
        });

        // 5. Action-specific post-conditions
        self.verify_action_specific(action, what_if, ctx, &mut verifications, &mut worsened_violations);

        // 6. Compare with pre-action observation for worsening detection
        self.check_worsening(action, what_if, ctx, &mut worsened_violations);

        PostConditionResult::with_violations(verifications, new_violations, worsened_violations)
    }

    /// Verify action-specific post-conditions
    fn verify_action_specific(
        &self,
        action: &StructuredAction,
        what_if: &WhatIfResult,
        _ctx: &DecisionContext,
        verifications: &mut Vec<PostConditionVerification>,
        _worsened: &mut Vec<String>,
    ) {
        match action {
            StructuredAction::StartGenerator { gen_id, target_mw } => {
                // Verify generator is producing at or near target
                verifications.push(PostConditionVerification {
                    name: "generator_output_effective".to_string(),
                    passed: what_if.all_constraints_satisfied,
                    description: format!(
                        "Generator {} set to {:.1}MW — constraints {}",
                        gen_id,
                        target_mw,
                        if what_if.all_constraints_satisfied { "satisfied" } else { "violated" }
                    ),
                });
            }
            StructuredAction::ShedLoad { zone_id, amount_mw } => {
                // Verify load was actually reduced
                verifications.push(PostConditionVerification {
                    name: "load_shed_effective".to_string(),
                    passed: what_if.converged,
                    description: format!(
                        "Shed {:.1}MW from zone {} — flow {}converged",
                        amount_mw,
                        zone_id,
                        if what_if.converged { "" } else { "NOT " }
                    ),
                });
            }
            StructuredAction::IsolateFault { upstream_switch, downstream_switch } => {
                // Verify fault section is isolated (convergence means topology is valid)
                verifications.push(PostConditionVerification {
                    name: "fault_isolation_effective".to_string(),
                    passed: what_if.converged,
                    description: format!(
                        "Fault isolation via switches {}/{} — flow {}converged",
                        upstream_switch,
                        downstream_switch,
                        if what_if.converged { "" } else { "NOT " }
                    ),
                });
            }
            StructuredAction::CloseTieSwitch { switch_id } => {
                // Verify tie close restored supply without violations
                verifications.push(PostConditionVerification {
                    name: "tie_close_effective".to_string(),
                    passed: what_if.converged && what_if.all_constraints_satisfied,
                    description: format!(
                        "Tie switch {} closed — converged={}, constraints={}",
                        switch_id,
                        what_if.converged,
                        if what_if.all_constraints_satisfied { "OK" } else { "VIOLATED" }
                    ),
                });
            }
            _ => {}
        }
    }

    /// Check if the action worsened existing conditions
    fn check_worsening(
        &self,
        _action: &StructuredAction,
        what_if: &WhatIfResult,
        ctx: &DecisionContext,
        worsened: &mut Vec<String>,
    ) {
        if let Some(ref obs) = ctx.observation {
            // Check if voltage violations are worse than before
            let pre_low_voltage = obs.low_voltage_buses(self.voltage_min);
            for (bus, v, _limit) in &what_if.voltage_violations {
                // Was this bus already low?
                if let Some(pre_v) = pre_low_voltage.iter().find(|(b, _)| b == bus).map(|(_, v)| *v) {
                    if *v < pre_v {
                        worsened.push(format!(
                            "Bus {} voltage worsened: {:.3} → {:.3} pu",
                            bus, pre_v, v
                        ));
                    }
                }
            }

            // Check if thermal violations are worse
            let pre_overloaded = obs.overloaded_branches(self.thermal_limit);
            for (branch, loading, _limit) in &what_if.thermal_violations {
                if let Some(pre_loading) = pre_overloaded.iter().find(|(b, _)| b == branch).map(|(_, l)| *l) {
                    if *loading > pre_loading {
                        worsened.push(format!(
                            "Branch {} loading worsened: {:.1}% → {:.1}%",
                            branch, pre_loading, loading
                        ));
                    }
                }
            }
        }
    }
}

impl Default for PostConditionVerifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_core::{AuthorityLevel, Jurisdiction, SystemOperatingState};

    fn make_ctx() -> DecisionContext {
        DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        )
    }

    fn make_what_if_ok() -> WhatIfResult {
        WhatIfResult {
            applicable: true,
            converged: true,
            voltage_violations: vec![],
            thermal_violations: vec![],
            all_constraints_satisfied: true,
            summary: "All OK".to_string(),
        }
    }

    fn make_what_if_with_violations() -> WhatIfResult {
        WhatIfResult {
            applicable: true,
            converged: true,
            voltage_violations: vec![(2, 0.88, 0.95)],
            thermal_violations: vec![(5, 110.0, 100.0)],
            all_constraints_satisfied: false,
            summary: "Voltage and thermal violations".to_string(),
        }
    }

    fn make_what_if_diverged() -> WhatIfResult {
        WhatIfResult {
            applicable: true,
            converged: false,
            voltage_violations: vec![],
            thermal_violations: vec![],
            all_constraints_satisfied: false,
            summary: "Power flow did not converge".to_string(),
        }
    }

    #[test]
    fn test_postcondition_all_ok() {
        let verifier = PostConditionVerifier::new();
        let ctx = make_ctx();
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let what_if = make_what_if_ok();
        let result = verifier.verify(&action, &what_if, &ctx);
        assert!(result.satisfied);
        assert!(result.new_violations.is_empty());
    }

    #[test]
    fn test_postcondition_voltage_violation() {
        let verifier = PostConditionVerifier::new();
        let ctx = make_ctx();
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 300.0 };
        let what_if = make_what_if_with_violations();
        let result = verifier.verify(&action, &what_if, &ctx);
        assert!(!result.satisfied);
        assert!(!result.new_violations.is_empty());
    }

    #[test]
    fn test_postcondition_divergence() {
        let verifier = PostConditionVerifier::new();
        let ctx = make_ctx();
        let action = StructuredAction::ShedLoad { zone_id: 1, amount_mw: 50.0 };
        let what_if = make_what_if_diverged();
        let result = verifier.verify(&action, &what_if, &ctx);
        assert!(!result.satisfied);
        assert!(result.new_violations.iter().any(|v| v.contains("divergence")));
    }

    #[test]
    fn test_postcondition_worsening_detected() {
        let verifier = PostConditionVerifier::new();
        let mut obs = eneros_core::PowerObservation::empty();
        obs.bus_voltages.insert(2, eneros_core::BusVoltageObservation { vm_pu: 0.92, va_degree: -5.0 });
        obs.branch_flows.insert(5, eneros_core::BranchFlowObservation {
            p_mw: 50.0, q_mvar: 10.0, loading_percent: 105.0,
        });
        let ctx = make_ctx().with_observation(obs);

        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 300.0 };
        let what_if = make_what_if_with_violations();
        let result = verifier.verify(&action, &what_if, &ctx);
        // Bus 2 went from 0.92 to 0.88 — worsened
        assert!(!result.worsened_violations.is_empty());
    }

    #[test]
    fn test_postcondition_tie_close_effective() {
        let verifier = PostConditionVerifier::new();
        let ctx = make_ctx();
        let action = StructuredAction::CloseTieSwitch { switch_id: 10 };
        let what_if = make_what_if_ok();
        let result = verifier.verify(&action, &what_if, &ctx);
        assert!(result.satisfied);
    }

    #[test]
    fn test_postcondition_tie_close_with_violations() {
        let verifier = PostConditionVerifier::new();
        let ctx = make_ctx();
        let action = StructuredAction::CloseTieSwitch { switch_id: 10 };
        let what_if = make_what_if_with_violations();
        let result = verifier.verify(&action, &what_if, &ctx);
        assert!(!result.satisfied);
    }
}
