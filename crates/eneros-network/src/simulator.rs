use crate::network::PowerNetwork;
use eneros_constraint::projector::{NetworkSimulator, WhatIfResult};
use eneros_core::StructuredAction;
use eneros_powerflow::BusTypeNR;
use std::sync::Arc;

mod result;

use result::{
    conservative_switching_reject, inapplicable_result, simulate_base_case, solve_and_check,
};

/// System base MVA. Matches `Ieee14BusData.base_mva` (100.0) and the
/// `PowerFlowSolver::new(100, ...)` used by `PowerNetwork`. Used to convert
/// MW/MVar action parameters into the per-unit quantities the solver consumes.
const BASE_MVA: f64 = 100.0;

/// Adapter that wraps PowerNetwork behind Arc<RwLock> to implement NetworkSimulator
/// for the constrained decision pipeline's What-If analysis.
///
/// ## Phase 15: physical accuracy
///
/// The simulation now reads real generator limits and zone membership from the
/// `PowerNetwork` (instead of hardcoded literals and `bus_map` misuse), and
/// applies the correct per-bus net-injection arithmetic for generator and load
/// actions. Switching actions (`ExecuteDevice{open/close}`, `IsolateFault`,
/// `CloseTieSwitch`) are **not** physically modeled (that requires Y-bus
/// reconstruction — deferred to a later phase); instead they return a
/// conservative `all_constraints_satisfied=false` so the projector routes them
/// to rejection / human intervention. This is safer than the previous behavior,
/// which silently treated every switching action as feasible.
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

        match action {
            // ── Generation actions: physically modeled ──
            StructuredAction::StartGenerator { gen_id, target_mw } => {
                self.simulate_generator(&net, *gen_id, *target_mw)
            }
            StructuredAction::ShedLoad { zone_id, amount_mw } => {
                self.simulate_shed_load(&net, *zone_id, *amount_mw)
            }
            StructuredAction::ExecuteDevice {
                operation, value, ..
            } => {
                match operation.as_str() {
                    // Reactive power adjustment: physically modeled (q_spec edit).
                    "adjust_reactive" => self.simulate_reactive(&net, action, *value),
                    // Switching operations: conservative reject (no Y-bus rebuild).
                    "open" | "close" | "合闸" | "分闸" => conservative_switching_reject(action),
                    _ => conservative_switching_reject(action),
                }
            }

            // ── Switching actions: conservative reject (physics deferred) ──
            StructuredAction::IsolateFault { .. } | StructuredAction::CloseTieSwitch { .. } => {
                conservative_switching_reject(action)
            }

            // ── NotifyAgent: no physical effect — solve the unmodified network ──
            StructuredAction::NotifyAgent { .. } => self.simulate_base_case(&net),
        }
    }

    fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
        let net = self.network.read();
        net.generator_table()
            .iter()
            .map(|g| (g.gen_id, g.p_min_mw, g.p_max_mw))
            .collect()
    }

    fn current_voltages(&self) -> Vec<(u64, f64)> {
        let net = self.network.read();
        match net.solve() {
            Ok(result) => result
                .bus_results
                .iter()
                .map(|b| (b.bus_id, b.voltage_magnitude))
                .collect(),
            Err(_) => net.bus_map().iter().map(|(&id, _)| (id, 1.0)).collect(),
        }
    }
}

impl NetworkSimulatorAdapter {
    /// Simulate a generator setpoint change. Looks up the generator by `gen_id`
    /// (NOT by misusing `bus_map`), then recomputes the bus net injection as
    /// `(target_mw - p_load_mw) / base_mva`. If the generator is unknown or sits
    /// on the slack bus (where p_spec has no effect on flow), the action is
    /// reported as inapplicable.
    fn simulate_generator(&self, net: &PowerNetwork, gen_id: u64, target_mw: f64) -> WhatIfResult {
        if !target_mw.is_finite() {
            return inapplicable_result(format!(
                "generator {} target_mw={} is not finite",
                gen_id, target_mw
            ));
        }

        let Some(gen) = net.generator_at(gen_id) else {
            return WhatIfResult {
                applicable: false,
                converged: true,
                voltage_violations: vec![],
                thermal_violations: vec![],
                all_constraints_satisfied: false,
                summary: format!("unknown generator gen_id={}", gen_id),
            };
        };

        let Some(&idx) = net.bus_map().get(&gen.bus_id) else {
            return WhatIfResult {
                applicable: false,
                converged: true,
                voltage_violations: vec![],
                thermal_violations: vec![],
                all_constraints_satisfied: false,
                summary: format!("generator {} bus {} not in bus_map", gen_id, gen.bus_id),
            };
        };

        if matches!(net.bus_types().get(idx), Some(BusTypeNR::Slack)) {
            return inapplicable_result(format!(
                "generator {} is on slack bus {}",
                gen_id, gen.bus_id
            ));
        }

        if target_mw < gen.p_min_mw || target_mw > gen.p_max_mw {
            return WhatIfResult {
                applicable: true,
                converged: true,
                voltage_violations: vec![],
                thermal_violations: vec![],
                all_constraints_satisfied: false,
                summary: format!(
                    "generator {} target {} MW outside limits [{}, {}] MW",
                    gen_id, target_mw, gen.p_min_mw, gen.p_max_mw
                ),
            };
        }

        let mut p_spec = net.p_spec().to_vec();
        if idx < p_spec.len() {
            // Net injection = generation − load, converted to per-unit.
            p_spec[idx] = (target_mw - gen.p_load_mw) / BASE_MVA;
        }

        let modified = net.with_modifications(Some(p_spec), None);
        solve_and_check(&modified)
    }

    /// Simulate load shedding in a zone. The `amount_mw` is distributed across
    /// every load bus in the zone proportionally to that bus's current load
    /// (so no bus sheds more than its load). Shedding load makes the net
    /// injection more positive (`p_spec +=`), which is the correct sign.
    fn simulate_shed_load(&self, net: &PowerNetwork, zone_id: u32, amount_mw: f64) -> WhatIfResult {
        if !amount_mw.is_finite() {
            return inapplicable_result(format!(
                "shed amount_mw={} is not finite for zone {}",
                amount_mw, zone_id
            ));
        }

        let Some(bus_ids) = net.zone_buses(zone_id) else {
            return WhatIfResult {
                applicable: false,
                converged: true,
                voltage_violations: vec![],
                thermal_violations: vec![],
                all_constraints_satisfied: false,
                summary: format!("unknown zone_id={}", zone_id),
            };
        };

        let bus_map = net.bus_map();
        let p_spec_orig = net.p_spec().to_vec();

        // Compute each bus's load magnitude (load = -min(net_p, 0)) so we can
        // distribute the shed proportionally among actual load buses.
        let mut load_buses: Vec<(usize, f64)> = Vec::new();
        let mut total_load = 0.0;
        for &bid in bus_ids {
            if let Some(&idx) = bus_map.get(&bid) {
                if idx < p_spec_orig.len() {
                    let net_p_pu = p_spec_orig[idx];
                    let load_mw = (-net_p_pu).max(0.0) * BASE_MVA;
                    if load_mw > 0.0 {
                        load_buses.push((idx, load_mw));
                        total_load += load_mw;
                    }
                }
            }
        }

        if total_load <= 0.0 || load_buses.is_empty() {
            return WhatIfResult {
                applicable: false,
                converged: true,
                voltage_violations: vec![],
                thermal_violations: vec![],
                all_constraints_satisfied: false,
                summary: format!("zone {} has no load to shed", zone_id),
            };
        }

        // Cap the total shed at the zone's total load.
        let capped_amount = amount_mw.min(total_load);
        let mut p_spec = p_spec_orig;
        for (idx, load_mw) in &load_buses {
            let share = (load_mw / total_load) * capped_amount; // MW
                                                                // Shedding load increases net injection (less negative).
            p_spec[*idx] += share / BASE_MVA;
        }

        let modified = net.with_modifications(Some(p_spec), None);
        solve_and_check(&modified)
    }

    /// Simulate a reactive power adjustment (MVar) on the generator/device bus.
    /// `value` is interpreted as the reactive output in MVar; the bus's q_spec
    /// is set to `value / base_mva`. The target bus is taken from the action's
    /// `device_id`, resolved via the generator table when possible.
    fn simulate_reactive(
        &self,
        net: &PowerNetwork,
        action: &StructuredAction,
        value: f64,
    ) -> WhatIfResult {
        if !value.is_finite() {
            return inapplicable_result(format!(
                "reactive adjustment value={} is not finite",
                value
            ));
        }

        let device_id = match action {
            StructuredAction::ExecuteDevice { device_id, .. } => *device_id,
            _ => return simulate_base_case(net),
        };

        // Resolve device_id to a bus index: prefer the generator table, then
        // fall back to treating device_id as a bus_id (legacy convention).
        let bus_id = net
            .generator_at(device_id)
            .map(|g| g.bus_id)
            .unwrap_or(device_id);

        let Some(&idx) = net.bus_map().get(&bus_id) else {
            return WhatIfResult {
                applicable: false,
                converged: true,
                voltage_violations: vec![],
                thermal_violations: vec![],
                all_constraints_satisfied: false,
                summary: format!("reactive target bus {} not in bus_map", bus_id),
            };
        };

        let mut q_spec = net.q_spec_view().to_vec();
        if idx < q_spec.len() {
            q_spec[idx] = value / BASE_MVA;
        }

        let modified = net.with_modifications(None, Some(q_spec));
        solve_and_check(&modified)
    }

    /// Solve the unmodified network (used for NotifyAgent, which has no
    /// physical effect — feasibility is whatever the current state is).
    fn simulate_base_case(&self, net: &PowerNetwork) -> WhatIfResult {
        simulate_base_case(net)
    }
}
