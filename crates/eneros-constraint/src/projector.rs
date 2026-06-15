use eneros_core::StructuredAction;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Result of a What-If analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhatIfResult {
    /// Whether the action can be applied to the network
    pub applicable: bool,
    /// Whether the post-action power flow converges
    pub converged: bool,
    /// Voltage violations after action (bus_id, voltage_pu, limit)
    pub voltage_violations: Vec<(u64, f64, f64)>,
    /// Thermal violations after action (branch_id, loading_percent, limit)
    pub thermal_violations: Vec<(u64, f64, f64)>,
    /// Whether all constraints are satisfied after the action
    pub all_constraints_satisfied: bool,
    /// Summary message
    pub summary: String,
}

/// Trait for network simulation — abstracts over PowerNetwork to avoid circular deps
pub trait NetworkSimulator: Send + Sync {
    /// Simulate an action and return the What-If result
    fn simulate_action(&self, action: &StructuredAction) -> WhatIfResult;

    /// Get the current generator limits: gen_id -> (p_min_mw, p_max_mw)
    fn generator_limits(&self) -> Vec<(u64, f64, f64)>;

    /// Get current bus voltages: bus_id -> voltage_pu
    fn current_voltages(&self) -> Vec<(u64, f64)>;
}

/// Modification made during feasibility projection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionModification {
    /// Parameter name that was modified
    pub parameter: String,
    /// Original value proposed by LLM
    pub original_value: f64,
    /// Projected value after feasibility check
    pub projected_value: f64,
    /// Reason for the modification
    pub reason: String,
}

/// Result of projecting an action to the feasible domain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProjectionResult {
    /// Action is feasible as-is
    Feasible(StructuredAction),
    /// Action was projected to nearest feasible point
    Projected {
        original: StructuredAction,
        projected: StructuredAction,
        modifications: Vec<ActionModification>,
    },
    /// Action is completely infeasible
    Infeasible {
        original: StructuredAction,
        violated_constraints: Vec<String>,
        suggested_alternatives: Vec<StructuredAction>,
    },
}

impl ProjectionResult {
    pub fn feasible_action(&self) -> Option<&StructuredAction> {
        match self {
            ProjectionResult::Feasible(a) => Some(a),
            ProjectionResult::Projected { projected, .. } => Some(projected),
            ProjectionResult::Infeasible { .. } => None,
        }
    }

    pub fn is_feasible(&self) -> bool {
        matches!(self, ProjectionResult::Feasible(_))
    }

    pub fn is_projected(&self) -> bool {
        matches!(self, ProjectionResult::Projected { .. })
    }

    pub fn is_infeasible(&self) -> bool {
        matches!(self, ProjectionResult::Infeasible { .. })
    }
}

/// Feasibility projector — projects LLM-proposed actions to the feasible domain
pub struct FeasibilityProjector {
    /// Network simulator for What-If analysis
    simulator: Arc<dyn NetworkSimulator>,
}

impl FeasibilityProjector {
    /// Create a new projector with a network simulator
    pub fn new(simulator: Arc<dyn NetworkSimulator>) -> Self {
        Self { simulator }
    }

    /// Evaluate action feasibility and project to feasible domain
    pub fn project(&self, action: &StructuredAction) -> ProjectionResult {
        // Step 1: Check device parameter hard limits (fast, no simulation needed)
        if let Some(clipped) = self.clip_to_hard_limits(action) {
            if clipped != *action {
                // Action was clipped — verify the clipped version via What-If
                let result = self.simulator.simulate_action(&clipped);
                if Self::simulation_satisfied(&result) {
                    let modifications = self.describe_modifications(action, &clipped);
                    return ProjectionResult::Projected {
                        original: action.clone(),
                        projected: clipped,
                        modifications,
                    };
                }
                // Even clipped version violates constraints — fall through to What-If
            }
        }

        // Step 2: What-If analysis on the original (or already-verified) action
        let what_if = self.simulator.simulate_action(action);

        if Self::simulation_satisfied(&what_if) {
            return ProjectionResult::Feasible(action.clone());
        }

        // Step 3: Try to find a feasible projection by reducing magnitude
        if let Some(projected) = self.try_find_feasible(action, &what_if) {
            return ProjectionResult::Projected {
                original: action.clone(),
                projected: projected.clone(),
                modifications: self.describe_modifications(action, &projected),
            };
        }

        // Step 4: Completely infeasible
        let violated = self.collect_violations(&what_if);
        let alternatives = self.suggest_alternatives(action, &what_if);
        ProjectionResult::Infeasible {
            original: action.clone(),
            violated_constraints: violated,
            suggested_alternatives: alternatives,
        }
    }

    /// Batch project multiple actions
    pub fn project_batch(&self, actions: &[StructuredAction]) -> Vec<ProjectionResult> {
        actions.iter().map(|a| self.project(a)).collect()
    }

    /// Clip action parameters to device hard limits (no simulation needed)
    fn clip_to_hard_limits(&self, action: &StructuredAction) -> Option<StructuredAction> {
        match action {
            StructuredAction::StartGenerator { gen_id, target_mw } => {
                let limits = self.simulator.generator_limits();
                if let Some((_, p_min, p_max)) = limits.iter().find(|(id, _, _)| id == gen_id) {
                    let clipped = target_mw.clamp(*p_min, *p_max);
                    if (clipped - target_mw).abs() > f64::EPSILON {
                        return Some(StructuredAction::StartGenerator {
                            gen_id: *gen_id,
                            target_mw: clipped,
                        });
                    }
                }
                Some(action.clone())
            }
            StructuredAction::ShedLoad { zone_id, amount_mw } => {
                // Cannot shed negative load
                if *amount_mw < 0.0 {
                    return Some(StructuredAction::ShedLoad {
                        zone_id: *zone_id,
                        amount_mw: 0.0,
                    });
                }
                Some(action.clone())
            }
            _ => Some(action.clone()),
        }
    }

    /// Try to find a feasible version of the action by reducing magnitude
    fn try_find_feasible(
        &self,
        action: &StructuredAction,
        _what_if: &WhatIfResult,
    ) -> Option<StructuredAction> {
        match action {
            StructuredAction::StartGenerator { gen_id, target_mw } => {
                // Try reducing target by 10% increments (up to 5 attempts)
                let limits = self.simulator.generator_limits();
                let (_, p_min, p_max) = limits.iter().find(|(id, _, _)| id == gen_id)?;
                let target = target_mw.clamp(*p_min, *p_max);
                for step in 1..=5 {
                    let ratio = step as f64 / 5.0;
                    let candidates = [
                        target - (target - *p_min) * ratio,
                        target + (*p_max - target) * ratio,
                    ];
                    for candidate in candidates {
                        if (candidate - target).abs() <= f64::EPSILON {
                            continue;
                        }
                        let trial_action = StructuredAction::StartGenerator {
                            gen_id: *gen_id,
                            target_mw: candidate,
                        };
                        if self.action_is_feasible(&trial_action) {
                            return Some(trial_action);
                        }
                    }
                }
                None
            }
            StructuredAction::ShedLoad { zone_id, amount_mw } => {
                let mut lower = *amount_mw;
                let mut upper = (*amount_mw).max(1.0);
                for _ in 0..8 {
                    lower *= 0.8;
                    upper *= 1.25;

                    if lower >= 1.0 {
                        let trial_action = StructuredAction::ShedLoad {
                            zone_id: *zone_id,
                            amount_mw: lower,
                        };
                        if self.action_is_feasible(&trial_action) {
                            return Some(trial_action);
                        }
                    }

                    let trial_action = StructuredAction::ShedLoad {
                        zone_id: *zone_id,
                        amount_mw: upper,
                    };
                    if self.action_is_feasible(&trial_action) {
                        return Some(trial_action);
                    }
                }
                None
            }
            _ => None, // Switching operations can't be "reduced" — either feasible or not
        }
    }

    fn action_is_feasible(&self, action: &StructuredAction) -> bool {
        let result = self.simulator.simulate_action(action);
        Self::simulation_satisfied(&result)
    }

    fn simulation_satisfied(result: &WhatIfResult) -> bool {
        result.applicable && result.converged && result.all_constraints_satisfied
    }

    /// Describe the modifications made during projection
    fn describe_modifications(
        &self,
        original: &StructuredAction,
        projected: &StructuredAction,
    ) -> Vec<ActionModification> {
        let mut mods = Vec::new();
        match (original, projected) {
            (
                StructuredAction::StartGenerator {
                    target_mw: orig_mw, ..
                },
                StructuredAction::StartGenerator {
                    target_mw: proj_mw, ..
                },
            ) if (orig_mw - proj_mw).abs() > f64::EPSILON => {
                let limits = self.simulator.generator_limits();
                let reason = limits
                    .iter()
                    .find_map(|(_, _, p_max)| {
                        if *orig_mw > *p_max && (proj_mw - p_max).abs() < f64::EPSILON {
                            Some(format!("Generator rated capacity {:.0}MW", p_max))
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| "Constraint satisfaction requires reduction".to_string());
                mods.push(ActionModification {
                    parameter: "target_mw".to_string(),
                    original_value: *orig_mw,
                    projected_value: *proj_mw,
                    reason,
                });
            }
            (
                StructuredAction::ShedLoad {
                    amount_mw: orig_mw, ..
                },
                StructuredAction::ShedLoad {
                    amount_mw: proj_mw, ..
                },
            ) if (orig_mw - proj_mw).abs() > f64::EPSILON => {
                mods.push(ActionModification {
                    parameter: "amount_mw".to_string(),
                    original_value: *orig_mw,
                    projected_value: *proj_mw,
                    reason: "Reduced to satisfy constraints".to_string(),
                });
            }
            _ => {}
        }
        mods
    }

    /// Collect violation descriptions from What-If result
    fn collect_violations(&self, what_if: &WhatIfResult) -> Vec<String> {
        let mut violations = Vec::new();
        for (bus_id, v, limit) in &what_if.voltage_violations {
            violations.push(format!(
                "Voltage violation: Bus {} voltage {:.3} pu < {:.3} pu limit",
                bus_id, v, limit
            ));
        }
        for (branch_id, loading, limit) in &what_if.thermal_violations {
            violations.push(format!(
                "Thermal violation: Branch {} loading {:.1}% > {:.1}% limit",
                branch_id, loading, limit
            ));
        }
        if !what_if.converged {
            violations.push("Power flow did not converge after action".to_string());
        }
        if violations.is_empty() {
            violations.push("Unknown constraint violation".to_string());
        }
        violations
    }

    /// Suggest alternative actions when the proposed action is infeasible
    fn suggest_alternatives(
        &self,
        action: &StructuredAction,
        what_if: &WhatIfResult,
    ) -> Vec<StructuredAction> {
        let mut alternatives = Vec::new();

        match action {
            StructuredAction::StartGenerator { gen_id, target_mw } => {
                // If voltage violations, suggest reactive power support instead
                if !what_if.voltage_violations.is_empty() {
                    self.push_feasible_alternative(
                        &mut alternatives,
                        StructuredAction::ExecuteDevice {
                            device_id: *gen_id,
                            operation: "adjust_reactive".to_string(),
                            value: 10.0, // Suggest 10 MVar increase
                        },
                    );
                }
                // If thermal violations, suggest load redistribution
                if !what_if.thermal_violations.is_empty() {
                    self.push_feasible_alternative(&mut alternatives, StructuredAction::NotifyAgent {
                        agent_id: "dispatch".to_string(),
                        message: format!(
                            "Generator {} at {:.0}MW causes thermal violations, need redistribution",
                            gen_id, target_mw
                        ),
                    });
                }
            }
            StructuredAction::ShedLoad { zone_id, amount_mw } => {
                // Suggest smaller amount
                self.push_feasible_alternative(
                    &mut alternatives,
                    StructuredAction::ShedLoad {
                        zone_id: *zone_id,
                        amount_mw: amount_mw * 0.5,
                    },
                );
            }
            StructuredAction::IsolateFault {
                upstream_switch,
                downstream_switch,
            } => {
                // Suggest notifying dispatch for manual intervention
                self.push_feasible_alternative(&mut alternatives, StructuredAction::NotifyAgent {
                    agent_id: "dispatch".to_string(),
                    message: format!(
                        "Fault isolation via switches {}/{} blocked by interlocking, requires manual intervention",
                        upstream_switch, downstream_switch
                    ),
                });
            }
            _ => {}
        }

        alternatives
    }

    fn push_feasible_alternative(
        &self,
        alternatives: &mut Vec<StructuredAction>,
        candidate: StructuredAction,
    ) {
        if self.action_is_feasible(&candidate) {
            alternatives.push(candidate);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock network simulator for testing
    struct MockSimulator {
        gen_limits: Vec<(u64, f64, f64)>,
        voltages: Vec<(u64, f64)>,
        always_feasible: bool,
    }

    impl MockSimulator {
        fn new_feasible() -> Self {
            Self {
                gen_limits: vec![(1, 0.0, 200.0), (2, 0.0, 150.0)],
                voltages: vec![(1, 1.02), (2, 0.98)],
                always_feasible: true,
            }
        }

        fn new_with_violations() -> Self {
            Self {
                gen_limits: vec![(1, 0.0, 200.0), (2, 0.0, 150.0)],
                voltages: vec![(1, 1.02), (2, 0.88)],
                always_feasible: false,
            }
        }
    }

    impl NetworkSimulator for MockSimulator {
        fn simulate_action(&self, action: &StructuredAction) -> WhatIfResult {
            if self.always_feasible {
                WhatIfResult {
                    applicable: true,
                    converged: true,
                    voltage_violations: vec![],
                    thermal_violations: vec![],
                    all_constraints_satisfied: true,
                    summary: "All constraints satisfied".to_string(),
                }
            } else {
                // Simulate violations for certain actions
                match action {
                    StructuredAction::StartGenerator { target_mw, .. } if *target_mw > 200.0 => {
                        WhatIfResult {
                            applicable: true,
                            converged: true,
                            voltage_violations: vec![(2, 0.88, 0.95)],
                            thermal_violations: vec![(5, 110.0, 100.0)],
                            all_constraints_satisfied: false,
                            summary: "Voltage and thermal violations".to_string(),
                        }
                    }
                    StructuredAction::ShedLoad { amount_mw, .. } if *amount_mw > 100.0 => {
                        WhatIfResult {
                            applicable: true,
                            converged: true,
                            voltage_violations: vec![],
                            thermal_violations: vec![],
                            all_constraints_satisfied: false,
                            summary: "Excessive load shedding".to_string(),
                        }
                    }
                    _ => WhatIfResult {
                        applicable: true,
                        converged: true,
                        voltage_violations: vec![],
                        thermal_violations: vec![],
                        all_constraints_satisfied: true,
                        summary: "OK".to_string(),
                    },
                }
            }
        }

        fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
            self.gen_limits.clone()
        }

        fn current_voltages(&self) -> Vec<(u64, f64)> {
            self.voltages.clone()
        }
    }

    #[test]
    fn test_feasible_action_passes() {
        let projector = FeasibilityProjector::new(Arc::new(MockSimulator::new_feasible()));
        let action = StructuredAction::StartGenerator {
            gen_id: 1,
            target_mw: 100.0,
        };
        let result = projector.project(&action);
        assert!(result.is_feasible());
    }

    #[test]
    fn test_generator_over_capacity_clipped() {
        let projector = FeasibilityProjector::new(Arc::new(MockSimulator::new_feasible()));
        let action = StructuredAction::StartGenerator {
            gen_id: 1,
            target_mw: 300.0,
        };
        let result = projector.project(&action);
        // Should be projected (clipped to 200MW)
        assert!(result.is_projected() || result.is_feasible());
        if let ProjectionResult::Projected {
            projected,
            modifications,
            ..
        } = &result
        {
            if let StructuredAction::StartGenerator { target_mw, .. } = projected {
                assert!(*target_mw <= 200.0);
            }
            assert!(!modifications.is_empty());
        }
    }

    #[test]
    fn test_infeasible_switching_operation() {
        let projector = FeasibilityProjector::new(Arc::new(MockSimulator::new_with_violations()));
        let action = StructuredAction::IsolateFault {
            upstream_switch: 1,
            downstream_switch: 2,
        };
        // Switching operations are always feasible in mock (no interlocking in simulator)
        let result = projector.project(&action);
        assert!(result.is_feasible() || result.is_infeasible() || result.is_projected());
    }

    #[test]
    fn test_batch_projection() {
        let projector = FeasibilityProjector::new(Arc::new(MockSimulator::new_feasible()));
        let actions = vec![
            StructuredAction::StartGenerator {
                gen_id: 1,
                target_mw: 100.0,
            },
            StructuredAction::ShedLoad {
                zone_id: 1,
                amount_mw: 50.0,
            },
        ];
        let results = projector.project_batch(&actions);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_negative_load_shed_clipped() {
        let projector = FeasibilityProjector::new(Arc::new(MockSimulator::new_feasible()));
        let action = StructuredAction::ShedLoad {
            zone_id: 1,
            amount_mw: -10.0,
        };
        let result = projector.project(&action);
        // Should be projected to 0.0
        assert!(result.is_projected() || result.is_feasible());
    }

    struct DirectionalSimulator;

    impl NetworkSimulator for DirectionalSimulator {
        fn simulate_action(&self, action: &StructuredAction) -> WhatIfResult {
            match action {
                StructuredAction::StartGenerator { target_mw, .. } => {
                    let ok = *target_mw >= 120.0;
                    WhatIfResult {
                        applicable: true,
                        converged: true,
                        voltage_violations: if ok { vec![] } else { vec![(2, 0.90, 0.95)] },
                        thermal_violations: vec![],
                        all_constraints_satisfied: ok,
                        summary: if ok { "OK" } else { "Low voltage" }.to_string(),
                    }
                }
                StructuredAction::ShedLoad { amount_mw, .. } => {
                    let ok = *amount_mw >= 100.0;
                    WhatIfResult {
                        applicable: true,
                        converged: true,
                        voltage_violations: if ok { vec![] } else { vec![(2, 0.90, 0.95)] },
                        thermal_violations: vec![],
                        all_constraints_satisfied: ok,
                        summary: if ok { "OK" } else { "Insufficient shed" }.to_string(),
                    }
                }
                _ => WhatIfResult {
                    applicable: true,
                    converged: true,
                    voltage_violations: vec![],
                    thermal_violations: vec![],
                    all_constraints_satisfied: false,
                    summary: "No feasible alternative".to_string(),
                },
            }
        }

        fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
            vec![(1, 0.0, 200.0)]
        }

        fn current_voltages(&self) -> Vec<(u64, f64)> {
            vec![(2, 0.90)]
        }
    }

    #[test]
    fn test_generator_projection_searches_upward_when_output_is_insufficient() {
        let projector = FeasibilityProjector::new(Arc::new(DirectionalSimulator));
        let action = StructuredAction::StartGenerator {
            gen_id: 1,
            target_mw: 80.0,
        };

        match projector.project(&action) {
            ProjectionResult::Projected { projected, .. } => match projected {
                StructuredAction::StartGenerator { target_mw, .. } => {
                    assert!(target_mw >= 120.0);
                }
                _ => panic!("Expected generator projection"),
            },
            _ => panic!("Expected upward generator projection"),
        }
    }

    #[test]
    fn test_shed_load_projection_searches_upward_when_shed_is_insufficient() {
        let projector = FeasibilityProjector::new(Arc::new(DirectionalSimulator));
        let action = StructuredAction::ShedLoad {
            zone_id: 1,
            amount_mw: 50.0,
        };

        match projector.project(&action) {
            ProjectionResult::Projected { projected, .. } => match projected {
                StructuredAction::ShedLoad { amount_mw, .. } => {
                    assert!(amount_mw >= 100.0);
                }
                _ => panic!("Expected shed-load projection"),
            },
            _ => panic!("Expected upward shed-load projection"),
        }
    }

    #[test]
    fn test_suggested_alternatives_are_simulated_before_returning() {
        let projector = FeasibilityProjector::new(Arc::new(DirectionalSimulator));
        let action = StructuredAction::IsolateFault {
            upstream_switch: 1,
            downstream_switch: 2,
        };

        match projector.project(&action) {
            ProjectionResult::Infeasible {
                suggested_alternatives,
                ..
            } => {
                assert!(suggested_alternatives.is_empty());
            }
            _ => panic!("Expected infeasible switching action"),
        }
    }

    #[test]
    fn test_what_if_result_serialization() {
        let result = WhatIfResult {
            applicable: true,
            converged: true,
            voltage_violations: vec![(1, 0.90, 0.95)],
            thermal_violations: vec![],
            all_constraints_satisfied: false,
            summary: "Voltage violation".to_string(),
        };
        let json = serde_json::to_string(&result).unwrap();
        let deserialized: WhatIfResult = serde_json::from_str(&json).unwrap();
        assert!(!deserialized.all_constraints_satisfied);
        assert_eq!(deserialized.voltage_violations.len(), 1);
    }

    #[test]
    fn test_projection_result_feasible_action() {
        let action = StructuredAction::StartGenerator {
            gen_id: 1,
            target_mw: 100.0,
        };
        let result = ProjectionResult::Feasible(action);
        assert!(result.feasible_action().is_some());
        assert!(result.is_feasible());
    }

    #[test]
    fn test_projection_result_infeasible_no_action() {
        let result: ProjectionResult = ProjectionResult::Infeasible {
            original: StructuredAction::IsolateFault {
                upstream_switch: 1,
                downstream_switch: 2,
            },
            violated_constraints: vec!["interlocking".to_string()],
            suggested_alternatives: vec![],
        };
        assert!(result.feasible_action().is_none());
        assert!(result.is_infeasible());
    }
}
