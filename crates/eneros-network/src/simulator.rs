use std::sync::Arc;
use eneros_constraint::projector::{NetworkSimulator, WhatIfResult};
use eneros_constraint::rules::ConstraintType;
use eneros_core::StructuredAction;
use crate::network::PowerNetwork;

/// Adapter that wraps PowerNetwork behind Arc<RwLock> to implement NetworkSimulator
/// for the constrained decision pipeline's What-If analysis
pub struct NetworkSimulatorAdapter {
    network: Arc<parking_lot::RwLock<PowerNetwork>>,
}

impl NetworkSimulatorAdapter {
    pub fn new(network: Arc<parking_lot::RwLock<PowerNetwork>>) -> Self {
        Self { network }
    }
}

impl NetworkSimulator for NetworkSimulatorAdapter {
    fn simulate_action(&self, action: &StructuredAction) -> WhatIfResult {
        let net = self.network.read();

        // Build a modified network by cloning the spec vectors
        let mut p_spec = net.p_spec().to_vec();
        let bus_map = net.bus_map();

        match action {
            StructuredAction::StartGenerator { gen_id, target_mw } => {
                if let Some(&idx) = bus_map.get(gen_id) {
                    if idx < p_spec.len() {
                        p_spec[idx] = target_mw / 100.0; // MW to per-unit (Sbase=100)
                    }
                }
            }
            StructuredAction::ShedLoad { zone_id, amount_mw } => {
                if let Some(&idx) = bus_map.get(&(*zone_id as u64)) {
                    if idx < p_spec.len() {
                        p_spec[idx] += amount_mw / 100.0; // reduce load
                    }
                }
            }
            _ => {} // Other actions don't directly modify power flow inputs
        }

        // Create a modified network for What-If simulation
        let modified = net.with_modified_p_spec(p_spec);
        match modified.solve() {
            Ok(result) => {
                let violations = modified.check_constraints(&result);
                let voltage_violations: Vec<(u64, f64, f64)> = violations.iter()
                    .filter(|v| v.constraint_type == ConstraintType::Voltage)
                    .map(|v| (v.element_id, v.actual_value, v.limit_max))
                    .collect();
                let thermal_violations: Vec<(u64, f64, f64)> = violations.iter()
                    .filter(|v| v.constraint_type == ConstraintType::Thermal)
                    .map(|v| (v.element_id, v.actual_value, v.limit_max))
                    .collect();

                WhatIfResult {
                    applicable: true,
                    converged: result.converged,
                    voltage_violations,
                    thermal_violations,
                    all_constraints_satisfied: violations.is_empty(),
                    summary: if violations.is_empty() { "OK".to_string() } else {
                        format!("{} violations", violations.len())
                    },
                }
            }
            Err(e) => WhatIfResult {
                applicable: false,
                converged: false,
                voltage_violations: vec![],
                thermal_violations: vec![],
                all_constraints_satisfied: false,
                summary: format!("Power flow failed: {}", e),
            },
        }
    }

    fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
        vec![
            (1, 0.0, 200.0),
            (2, 0.0, 150.0),
            (3, 0.0, 100.0),
            (6, 0.0, 80.0),
            (8, 0.0, 60.0),
        ]
    }

    fn current_voltages(&self) -> Vec<(u64, f64)> {
        let net = self.network.read();
        match net.solve() {
            Ok(result) => result.bus_results.iter()
                .map(|b| (b.bus_id, b.voltage_magnitude))
                .collect(),
            Err(_) => net.bus_map().iter()
                .map(|(&id, _)| (id, 1.0))
                .collect(),
        }
    }
}
