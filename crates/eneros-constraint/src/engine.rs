use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::RwLock;
use eneros_core::{ElementId, SeverityLevel, SystemOperatingState, ActionFeasibility};
use eneros_eventbus::EventBus;
use eneros_powerflow::{PowerFlowSolver, YBusMatrix, BusResult};

use crate::rules::{Constraint, ConstraintType, N1Result, N1Violation, N1ViolationType, StabilityResult, VoltageMargin};
use crate::violation::Violation;

/// Default voltage limits for N-1 analysis
const VOLTAGE_MIN_PU: f64 = 0.95;
const VOLTAGE_MAX_PU: f64 = 1.10;
/// Default thermal loading limit for N-1 analysis
const THERMAL_LIMIT_PERCENT: f64 = 100.0;
/// Voltage stability margin threshold
const STABILITY_MARGIN_THRESHOLD: f64 = 0.3;

/// Constraint executor for power system safety
pub struct ConstraintEngine {
    constraints: RwLock<HashMap<String, Constraint>>,
    violations: RwLock<Vec<Violation>>,
    /// Dynamic threshold multiplier (1.0 = normal, higher = more relaxed)
    current_threshold_multiplier: f64,
    /// Optional event bus for violation notifications
    #[allow(dead_code)]
    event_bus: Option<Arc<EventBus>>,
}

impl ConstraintEngine {
    /// Create a new constraint engine
    pub fn new() -> Self {
        Self {
            constraints: RwLock::new(HashMap::new()),
            violations: RwLock::new(Vec::new()),
            current_threshold_multiplier: 1.0,
            event_bus: None,
        }
    }

    /// Register a constraint
    pub fn register(&self, constraint: Constraint) {
        let mut constraints = self.constraints.write();
        constraints.insert(constraint.id.clone(), constraint);
    }

    /// Remove a constraint
    pub fn remove(&self, constraint_id: &str) -> bool {
        let mut constraints = self.constraints.write();
        constraints.remove(constraint_id).is_some()
    }

    /// Check all constraints against current state
    pub fn check_all(
        &self,
        bus_voltages: &[(ElementId, f64)],       // (bus_id, voltage_pu)
        branch_loadings: &[(ElementId, f64)],     // (branch_id, loading_percent)
        frequency: f64,
    ) -> Vec<Violation> {
        let constraints = self.constraints.read();
        let mut violations = Vec::new();

        for constraint in constraints.values() {
            if !constraint.enabled {
                continue;
            }

            let violation = match constraint.constraint_type {
                ConstraintType::Voltage => self.check_voltage_constraint(constraint, bus_voltages),
                ConstraintType::Thermal => self.check_thermal_constraint(constraint, branch_loadings),
                ConstraintType::Frequency => self.check_frequency_constraint(constraint, frequency),
                ConstraintType::N1 => self.check_n1_constraint(constraint, bus_voltages, branch_loadings),
                ConstraintType::Stability => self.check_stability_constraint(constraint, bus_voltages),
            };

            if let Some(v) = violation {
                violations.push(v);
            }
        }

        // Record violations
        let mut recorded_violations = self.violations.write();
        recorded_violations.extend(violations.clone());

        violations
    }

    fn check_voltage_constraint(
        &self,
        constraint: &Constraint,
        bus_voltages: &[(ElementId, f64)],
    ) -> Option<Violation> {
        for &(bus_id, voltage) in bus_voltages {
            if constraint.element_ids.contains(&bus_id)
                && (voltage < constraint.limit_min || voltage > constraint.limit_max)
            {
                return Some(Violation {
                    constraint_id: constraint.id.clone(),
                    element_id: bus_id,
                    constraint_type: ConstraintType::Voltage,
                    actual_value: voltage,
                    limit_min: constraint.limit_min,
                    limit_max: constraint.limit_max,
                    severity: constraint.severity,
                    response_strategy: constraint.response_strategy.clone(),
                    timestamp: chrono::Utc::now(),
                });
            }
        }
        None
    }

    fn check_thermal_constraint(
        &self,
        constraint: &Constraint,
        branch_loadings: &[(ElementId, f64)],
    ) -> Option<Violation> {
        for &(branch_id, loading) in branch_loadings {
            if constraint.element_ids.contains(&branch_id)
                && loading > constraint.limit_max
            {
                return Some(Violation {
                    constraint_id: constraint.id.clone(),
                    element_id: branch_id,
                    constraint_type: ConstraintType::Thermal,
                    actual_value: loading,
                    limit_min: constraint.limit_min,
                    limit_max: constraint.limit_max,
                    severity: constraint.severity,
                    response_strategy: constraint.response_strategy.clone(),
                    timestamp: chrono::Utc::now(),
                });
            }
        }
        None
    }

    fn check_frequency_constraint(
        &self,
        constraint: &Constraint,
        frequency: f64,
    ) -> Option<Violation> {
        if frequency < constraint.limit_min || frequency > constraint.limit_max {
            Some(Violation {
                constraint_id: constraint.id.clone(),
                element_id: 0,
                constraint_type: ConstraintType::Frequency,
                actual_value: frequency,
                limit_min: constraint.limit_min,
                limit_max: constraint.limit_max,
                severity: constraint.severity,
                response_strategy: constraint.response_strategy.clone(),
                timestamp: chrono::Utc::now(),
            })
        } else {
            None
        }
    }

    fn check_n1_constraint(
        &self,
        constraint: &Constraint,
        bus_voltages: &[(ElementId, f64)],
        _branch_loadings: &[(ElementId, f64)],
    ) -> Option<Violation> {
        for &(bus_id, voltage) in bus_voltages {
            if constraint.element_ids.contains(&bus_id)
                && (voltage < constraint.limit_min || voltage > constraint.limit_max)
            {
                return Some(Violation {
                    constraint_id: constraint.id.clone(),
                    element_id: bus_id,
                    constraint_type: ConstraintType::N1,
                    actual_value: voltage,
                    limit_min: constraint.limit_min,
                    limit_max: constraint.limit_max,
                    severity: constraint.severity,
                    response_strategy: constraint.response_strategy.clone(),
                    timestamp: chrono::Utc::now(),
                });
            }
        }
        None
    }

    fn check_stability_constraint(
        &self,
        constraint: &Constraint,
        bus_voltages: &[(ElementId, f64)],
    ) -> Option<Violation> {
        for &(bus_id, voltage) in bus_voltages {
            if constraint.element_ids.contains(&bus_id)
                && voltage < constraint.limit_min
            {
                return Some(Violation {
                    constraint_id: constraint.id.clone(),
                    element_id: bus_id,
                    constraint_type: ConstraintType::Stability,
                    actual_value: voltage,
                    limit_min: constraint.limit_min,
                    limit_max: constraint.limit_max,
                    severity: constraint.severity,
                    response_strategy: constraint.response_strategy.clone(),
                    timestamp: chrono::Utc::now(),
                });
            }
        }
        None
    }

    /// Perform N-1 contingency analysis
    ///
    /// For each branch, remove it from the Y-Bus matrix, re-solve power flow,
    /// and check for voltage/thermal violations.
    #[allow(clippy::too_many_arguments)]
    pub fn check_n1_analysis(
        &self,
        _ybus: &YBusMatrix,
        p_spec: &[f64],
        q_spec: &[f64],
        bus_types: &[eneros_powerflow::solver::BusTypeNR],
        branches: &[(ElementId, ElementId, f64, f64, f64, f64)], // (from, to, r, x, b, tap)
        bus_map: &HashMap<ElementId, usize>,
        solver: &PowerFlowSolver,
        voltage_min: Option<f64>,
        voltage_max: Option<f64>,
        thermal_limit: Option<f64>,
    ) -> Vec<N1Result> {
        let v_min = voltage_min.unwrap_or(VOLTAGE_MIN_PU);
        let v_max = voltage_max.unwrap_or(VOLTAGE_MAX_PU);
        let t_limit = thermal_limit.unwrap_or(THERMAL_LIMIT_PERCENT);

        let mut results = Vec::new();

        for (branch_idx, &(from, to, _r, _x, _b, _tap)) in branches.iter().enumerate() {
            // Build Y-Bus without this branch
            let mut reduced_branches: Vec<(ElementId, ElementId, f64, f64, f64, f64)> = Vec::new();
            for (i, &br) in branches.iter().enumerate() {
                if i != branch_idx {
                    reduced_branches.push(br);
                }
            }

            let ybus_n1 = YBusMatrix::from_branches(&reduced_branches, bus_map);

            // Solve power flow for the contingency case
            let result = solver.solve(&ybus_n1, p_spec, q_spec, bus_types);

            match result {
                Ok(pf_result) if pf_result.converged => {
                    let mut voltage_violations = Vec::new();
                    let mut thermal_violations = Vec::new();

                    // Check voltage violations
                    for bus in &pf_result.bus_results {
                        if bus.voltage_magnitude < v_min {
                            voltage_violations.push(N1Violation {
                                element_id: bus.bus_id,
                                violation_type: N1ViolationType::LowVoltage,
                                actual_value: bus.voltage_magnitude,
                                limit_value: v_min,
                            });
                        } else if bus.voltage_magnitude > v_max {
                            voltage_violations.push(N1Violation {
                                element_id: bus.bus_id,
                                violation_type: N1ViolationType::HighVoltage,
                                actual_value: bus.voltage_magnitude,
                                limit_value: v_max,
                            });
                        }
                    }

                    // Check thermal violations
                    for branch in &pf_result.branch_results {
                        if branch.loading_percent > t_limit {
                            thermal_violations.push(N1Violation {
                                element_id: branch.branch_id,
                                violation_type: N1ViolationType::Overload,
                                actual_value: branch.loading_percent,
                                limit_value: t_limit,
                            });
                        }
                    }

                    let severity = if !voltage_violations.is_empty() || !thermal_violations.is_empty() {
                        let has_low_voltage = voltage_violations.iter().any(|v| v.actual_value < 0.85);
                        let has_overload = thermal_violations.iter().any(|v| v.actual_value > 150.0);
                        if has_low_voltage || has_overload {
                            SeverityLevel::Critical
                        } else {
                            SeverityLevel::Major
                        }
                    } else {
                        SeverityLevel::Info
                    };

                    results.push(N1Result {
                        branch_id: from * 1000 + to, // Generate a unique ID from bus pair
                        converged: true,
                        voltage_violations,
                        thermal_violations,
                        severity,
                    });
                }
                _ => {
                    // Non-convergence is a critical N-1 violation
                    results.push(N1Result {
                        branch_id: from * 1000 + to,
                        converged: false,
                        voltage_violations: Vec::new(),
                        thermal_violations: Vec::new(),
                        severity: SeverityLevel::Critical,
                    });
                }
            }
        }

        results
    }

    /// Perform voltage stability analysis
    ///
    /// Uses a simplified approach: compute voltage stability margin
    /// based on distance from nominal voltage.
    pub fn check_stability(
        &self,
        bus_results: &[BusResult],
    ) -> StabilityResult {
        let mut voltage_margins = Vec::new();
        let mut critical_buses = Vec::new();

        for bus in bus_results {
            // Simplified voltage stability margin:
            // margin = (V - V_critical) / V_nominal
            // V_critical ≈ 0.7 pu (typical voltage collapse threshold)
            let v_critical = 0.7;
            let v_nominal = 1.0;
            let margin = (bus.voltage_magnitude - v_critical) / (v_nominal - v_critical);

            voltage_margins.push(VoltageMargin {
                bus_id: bus.bus_id,
                voltage_pu: bus.voltage_magnitude,
                margin: margin.max(0.0),
            });

            if margin < STABILITY_MARGIN_THRESHOLD {
                critical_buses.push(bus.bus_id);
            }
        }

        let stable = critical_buses.is_empty();

        StabilityResult {
            voltage_margins,
            critical_buses,
            stable,
        }
    }

    /// Get violation history
    pub fn get_violations(&self) -> Vec<Violation> {
        self.violations.read().clone()
    }

    /// Clear violation history
    pub fn clear_violations(&self) {
        self.violations.write().clear();
    }

    /// Check if an action is feasible — predicts impact without executing
    pub fn check_action_feasibility(&self, action_description: &str) -> ActionFeasibility {
        let mut new_violations = Vec::new();
        let mut worsened = Vec::new();

        let action_lower = action_description.to_lowercase();

        // Check against current violations
        let current = self.get_current_violations();

        // Shedding capacitor when voltage is low -> worsen voltage
        if action_lower.contains("shed capacitor") || action_lower.contains("切除电容") {
            let has_voltage_low = current.iter().any(|v| v.constraint_type == ConstraintType::Voltage);
            if has_voltage_low {
                worsened.push("voltage_low".to_string());
            }
        }

        // Tripping line when thermal overload exists -> worsen thermal
        if action_lower.contains("trip line") || action_lower.contains("切除线路") {
            let has_thermal = current.iter().any(|v| v.constraint_type == ConstraintType::Thermal);
            if has_thermal {
                worsened.push("thermal_overload".to_string());
            }
        }

        // Load shedding -> new violation risk (under-frequency if too much)
        if action_lower.contains("load shed") || action_lower.contains("切负荷") {
            new_violations.push("potential_under_frequency".to_string());
        }

        let feasible = worsened.is_empty();
        let risk_level = if !worsened.is_empty() {
            SeverityLevel::Critical
        } else if !new_violations.is_empty() {
            SeverityLevel::Major
        } else {
            SeverityLevel::Info
        };

        ActionFeasibility {
            feasible,
            new_violations,
            worsened_violations: worsened,
            risk_level,
        }
    }

    /// Get current violations
    pub fn get_current_violations(&self) -> Vec<Violation> {
        self.violations.read().clone()
    }

    /// Set emergency thresholds — adjust constraint limits based on system state
    pub fn set_emergency_thresholds(&mut self, state: SystemOperatingState) {
        match state {
            SystemOperatingState::Normal | SystemOperatingState::Restoration => {
                self.current_threshold_multiplier = 1.0;
            }
            SystemOperatingState::Alert => {
                self.current_threshold_multiplier = 1.0; // No relaxation in alert
            }
            SystemOperatingState::Emergency => {
                self.current_threshold_multiplier = 1.5; // Allow 50% more deviation
            }
            SystemOperatingState::Blackout => {
                self.current_threshold_multiplier = 2.0; // Maximum relaxation
            }
        }
    }

    /// Get current threshold multiplier
    pub fn threshold_multiplier(&self) -> f64 {
        self.current_threshold_multiplier
    }

    /// Get constraint count
    pub fn constraint_count(&self) -> usize {
        self.constraints.read().len()
    }
}

impl Default for ConstraintEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_powerflow::solver::BusTypeNR;
    use std::collections::HashMap;

    #[test]
    fn test_constraint_engine_new() {
        let engine = ConstraintEngine::new();
        assert_eq!(engine.constraint_count(), 0);
    }

    #[test]
    fn test_register_and_check_voltage() {
        let engine = ConstraintEngine::new();
        let mut constraint = Constraint::new(
            "v1".to_string(),
            "Voltage check".to_string(),
            ConstraintType::Voltage,
            0.95,
            1.05,
        );
        constraint.element_ids = vec![1, 2];
        engine.register(constraint);

        let bus_voltages: Vec<(ElementId, f64)> = vec![(1, 0.90), (2, 1.02)];
        let branch_loadings: Vec<(ElementId, f64)> = vec![];
        let violations = engine.check_all(&bus_voltages, &branch_loadings, 50.0);

        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].element_id, 1);
    }

    #[test]
    fn test_check_frequency() {
        let engine = ConstraintEngine::new();
        let constraint = Constraint::new(
            "f1".to_string(),
            "Frequency check".to_string(),
            ConstraintType::Frequency,
            49.8,
            50.2,
        );
        engine.register(constraint);

        let bus_voltages: Vec<(ElementId, f64)> = vec![];
        let branch_loadings: Vec<(ElementId, f64)> = vec![];
        let violations = engine.check_all(&bus_voltages, &branch_loadings, 49.5);

        assert_eq!(violations.len(), 1);
        assert!(violations[0].actual_value < 49.8);
    }

    #[test]
    fn test_check_thermal() {
        let engine = ConstraintEngine::new();
        let mut constraint = Constraint::new(
            "t1".to_string(),
            "Thermal check".to_string(),
            ConstraintType::Thermal,
            0.0,
            100.0,
        );
        constraint.element_ids = vec![10];
        engine.register(constraint);

        let bus_voltages: Vec<(ElementId, f64)> = vec![];
        let branch_loadings: Vec<(ElementId, f64)> = vec![(10, 120.0)];
        let violations = engine.check_all(&bus_voltages, &branch_loadings, 50.0);

        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].element_id, 10);
    }

    #[test]
    fn test_n1_analysis_simple() {
        let mut bus_map = HashMap::new();
        bus_map.insert(0u64, 0);
        bus_map.insert(1u64, 1);
        bus_map.insert(2u64, 2);

        let branches: Vec<(ElementId, ElementId, f64, f64, f64, f64)> = vec![
            (0, 1, 0.01, 0.1, 0.0, 1.0),
            (1, 2, 0.015, 0.15, 0.0, 1.0),
            (0, 2, 0.02, 0.2, 0.0, 1.0),
        ];

        let ybus = YBusMatrix::from_branches(&branches, &bus_map);
        let p_spec = vec![0.0, 0.5, -1.0];
        let q_spec = vec![0.0, 0.2, -0.5];
        let bus_types = vec![BusTypeNR::Slack, BusTypeNR::PV, BusTypeNR::PQ];

        let solver = PowerFlowSolver::new(50, 1e-6);
        let engine = ConstraintEngine::new();

        let results = engine.check_n1_analysis(
            &ybus, &p_spec, &q_spec, &bus_types,
            &branches, &bus_map, &solver,
            None, None, None,
        );

        assert_eq!(results.len(), 3);
        // At least some contingencies should converge (mesh network)
        let converged_count = results.iter().filter(|r| r.converged).count();
        assert!(converged_count >= 1, "At least 1 N-1 case should converge");
    }

    #[test]
    fn test_stability_check() {
        let engine = ConstraintEngine::new();

        let bus_results = vec![
            BusResult { bus_id: 0, voltage_magnitude: 1.05, voltage_angle: 0.0, p_injection: 0.0, q_injection: 0.0 },
            BusResult { bus_id: 1, voltage_magnitude: 0.98, voltage_angle: -0.05, p_injection: 0.5, q_injection: 0.2 },
            BusResult { bus_id: 2, voltage_magnitude: 0.75, voltage_angle: -0.15, p_injection: -1.0, q_injection: -0.5 },
        ];

        let result = engine.check_stability(&bus_results);

        assert!(result.voltage_margins.len() == 3);
        // Bus 2 has low voltage, should be critical
        assert!(result.critical_buses.contains(&2));
        assert!(!result.stable);
    }

    #[test]
    fn test_stability_healthy() {
        let engine = ConstraintEngine::new();

        let bus_results = vec![
            BusResult { bus_id: 0, voltage_magnitude: 1.05, voltage_angle: 0.0, p_injection: 0.0, q_injection: 0.0 },
            BusResult { bus_id: 1, voltage_magnitude: 1.02, voltage_angle: -0.02, p_injection: 0.5, q_injection: 0.2 },
        ];

        let result = engine.check_stability(&bus_results);
        assert!(result.stable);
        assert!(result.critical_buses.is_empty());
    }

    // === Action feasibility tests ===

    #[test]
    fn test_check_action_feasibility_safe_action() {
        let engine = ConstraintEngine::new();
        let result = engine.check_action_feasibility("adjust transformer tap");
        assert!(result.feasible);
        assert!(result.new_violations.is_empty());
        assert!(result.worsened_violations.is_empty());
        assert_eq!(result.risk_level, SeverityLevel::Info);
    }

    #[test]
    fn test_check_action_feasibility_shed_capacitor_with_voltage_low() {
        let engine = ConstraintEngine::new();
        let mut constraint = Constraint::new(
            "v1".to_string(),
            "Voltage check".to_string(),
            ConstraintType::Voltage,
            0.95,
            1.05,
        );
        constraint.element_ids = vec![1];
        engine.register(constraint);

        // Trigger a voltage violation
        let bus_voltages: Vec<(ElementId, f64)> = vec![(1, 0.90)];
        let branch_loadings: Vec<(ElementId, f64)> = vec![];
        let violations = engine.check_all(&bus_voltages, &branch_loadings, 50.0);
        assert_eq!(violations.len(), 1);

        // Now check feasibility of shedding capacitor — should worsen voltage
        let result = engine.check_action_feasibility("shed capacitor bank 1");
        assert!(!result.feasible);
        assert!(result.worsened_violations.contains(&"voltage_low".to_string()));
        assert_eq!(result.risk_level, SeverityLevel::Critical);
    }

    #[test]
    fn test_check_action_feasibility_shed_capacitor_no_voltage_issue() {
        let engine = ConstraintEngine::new();
        // No violations registered, shedding capacitor should be feasible
        let result = engine.check_action_feasibility("shed capacitor bank 1");
        assert!(result.feasible);
        assert!(result.worsened_violations.is_empty());
        assert_eq!(result.risk_level, SeverityLevel::Info);
    }

    #[test]
    fn test_check_action_feasibility_trip_line_with_thermal() {
        let engine = ConstraintEngine::new();
        let mut constraint = Constraint::new(
            "t1".to_string(),
            "Thermal check".to_string(),
            ConstraintType::Thermal,
            0.0,
            100.0,
        );
        constraint.element_ids = vec![10];
        engine.register(constraint);

        // Trigger a thermal violation
        let bus_voltages: Vec<(ElementId, f64)> = vec![];
        let branch_loadings: Vec<(ElementId, f64)> = vec![(10, 120.0)];
        let violations = engine.check_all(&bus_voltages, &branch_loadings, 50.0);
        assert_eq!(violations.len(), 1);

        // Now check feasibility of tripping a line — should worsen thermal
        let result = engine.check_action_feasibility("trip line L1");
        assert!(!result.feasible);
        assert!(result.worsened_violations.contains(&"thermal_overload".to_string()));
        assert_eq!(result.risk_level, SeverityLevel::Critical);
    }

    #[test]
    fn test_check_action_feasibility_load_shed() {
        let engine = ConstraintEngine::new();
        // Load shedding introduces potential under-frequency risk
        let result = engine.check_action_feasibility("load shed 50MW");
        assert!(result.feasible); // feasible but with new violation risk
        assert!(result.new_violations.contains(&"potential_under_frequency".to_string()));
        assert!(result.worsened_violations.is_empty());
        assert_eq!(result.risk_level, SeverityLevel::Major);
    }

    #[test]
    fn test_check_action_feasibility_chinese_keywords() {
        let engine = ConstraintEngine::new();
        let mut constraint = Constraint::new(
            "v1".to_string(),
            "Voltage check".to_string(),
            ConstraintType::Voltage,
            0.95,
            1.05,
        );
        constraint.element_ids = vec![1];
        engine.register(constraint);

        // Trigger a voltage violation
        let bus_voltages: Vec<(ElementId, f64)> = vec![(1, 0.90)];
        let branch_loadings: Vec<(ElementId, f64)> = vec![];
        engine.check_all(&bus_voltages, &branch_loadings, 50.0);

        // Test Chinese keyword: 切除电容
        let result = engine.check_action_feasibility("切除电容组1");
        assert!(!result.feasible);
        assert!(result.worsened_violations.contains(&"voltage_low".to_string()));

        // Test Chinese keyword: 切负荷
        let engine2 = ConstraintEngine::new();
        let result2 = engine2.check_action_feasibility("切负荷50MW");
        assert!(result2.feasible);
        assert!(result2.new_violations.contains(&"potential_under_frequency".to_string()));
    }

    // === get_current_violations tests ===

    #[test]
    fn test_get_current_violations_empty() {
        let engine = ConstraintEngine::new();
        let violations = engine.get_current_violations();
        assert!(violations.is_empty());
    }

    #[test]
    fn test_get_current_violations_with_data() {
        let engine = ConstraintEngine::new();
        let mut constraint = Constraint::new(
            "v1".to_string(),
            "Voltage check".to_string(),
            ConstraintType::Voltage,
            0.95,
            1.05,
        );
        constraint.element_ids = vec![1];
        engine.register(constraint);

        let bus_voltages: Vec<(ElementId, f64)> = vec![(1, 0.90)];
        let branch_loadings: Vec<(ElementId, f64)> = vec![];
        engine.check_all(&bus_voltages, &branch_loadings, 50.0);

        let current = engine.get_current_violations();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].constraint_type, ConstraintType::Voltage);
        assert_eq!(current[0].element_id, 1);
    }

    // === Emergency thresholds tests ===

    #[test]
    fn test_set_emergency_thresholds_normal() {
        let mut engine = ConstraintEngine::new();
        engine.set_emergency_thresholds(SystemOperatingState::Normal);
        assert!((engine.threshold_multiplier() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_set_emergency_thresholds_alert() {
        let mut engine = ConstraintEngine::new();
        engine.set_emergency_thresholds(SystemOperatingState::Alert);
        assert!((engine.threshold_multiplier() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_set_emergency_thresholds_emergency() {
        let mut engine = ConstraintEngine::new();
        engine.set_emergency_thresholds(SystemOperatingState::Emergency);
        assert!((engine.threshold_multiplier() - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_set_emergency_thresholds_blackout() {
        let mut engine = ConstraintEngine::new();
        engine.set_emergency_thresholds(SystemOperatingState::Blackout);
        assert!((engine.threshold_multiplier() - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_set_emergency_thresholds_restoration() {
        let mut engine = ConstraintEngine::new();
        // First set to blackout, then restore
        engine.set_emergency_thresholds(SystemOperatingState::Blackout);
        assert!((engine.threshold_multiplier() - 2.0).abs() < f64::EPSILON);
        engine.set_emergency_thresholds(SystemOperatingState::Restoration);
        assert!((engine.threshold_multiplier() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_threshold_multiplier_default() {
        let engine = ConstraintEngine::new();
        assert!((engine.threshold_multiplier() - 1.0).abs() < f64::EPSILON);
    }
}
