use std::collections::HashMap;
use parking_lot::RwLock;
use eneros_core::ElementId;

use crate::rules::{Constraint, ConstraintType};
use crate::violation::Violation;

/// Constraint executor for power system safety
pub struct ConstraintEngine {
    constraints: RwLock<HashMap<String, Constraint>>,
    violations: RwLock<Vec<Violation>>,
}

impl ConstraintEngine {
    /// Create a new constraint engine
    pub fn new() -> Self {
        Self {
            constraints: RwLock::new(HashMap::new()),
            violations: RwLock::new(Vec::new()),
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
                _ => None,
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
            if constraint.element_ids.contains(&bus_id) {
                if voltage < constraint.limit_min || voltage > constraint.limit_max {
                    return Some(Violation {
                        constraint_id: constraint.id.clone(),
                        element_id: bus_id,
                        actual_value: voltage,
                        limit_min: constraint.limit_min,
                        limit_max: constraint.limit_max,
                        severity: constraint.severity.clone(),
                        response_strategy: constraint.response_strategy.clone(),
                        timestamp: chrono::Utc::now(),
                    });
                }
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
            if constraint.element_ids.contains(&branch_id) {
                if loading > constraint.limit_max {
                    return Some(Violation {
                        constraint_id: constraint.id.clone(),
                        element_id: branch_id,
                        actual_value: loading,
                        limit_min: constraint.limit_min,
                        limit_max: constraint.limit_max,
                        severity: constraint.severity.clone(),
                        response_strategy: constraint.response_strategy.clone(),
                        timestamp: chrono::Utc::now(),
                    });
                }
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
            return Some(Violation {
                constraint_id: constraint.id.clone(),
                element_id: 0, // System-wide constraint
                actual_value: frequency,
                limit_min: constraint.limit_min,
                limit_max: constraint.limit_max,
                severity: constraint.severity.clone(),
                response_strategy: constraint.response_strategy.clone(),
                timestamp: chrono::Utc::now(),
            })
        } else {
            None
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
