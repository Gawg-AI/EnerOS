use crate::network::PowerNetwork;
use eneros_constraint::projector::{NetworkSimulator, WhatIfResult};
use eneros_core::{ElementId, StructuredAction};
use eneros_powerflow::BusTypeNR;
use std::sync::Arc;

mod result;

use result::{inapplicable_result, simulate_base_case, solve_and_check};

/// System base MVA. Matches `Ieee14BusData.base_mva` (100.0) and the
/// `PowerFlowSolver::new(100, ...)` used by `PowerNetwork`. Used to convert
/// MW/MVar action parameters into the per-unit quantities the solver consumes.
const BASE_MVA: f64 = 100.0;

/// Adapter that wraps PowerNetwork behind `Arc<RwLock>` to implement NetworkSimulator
/// for the constrained decision pipeline's What-If analysis.
///
/// ## Phase 15: physical accuracy
///
/// The simulation now reads real generator limits and zone membership from the
/// `PowerNetwork` (instead of hardcoded literals and `bus_map` misuse), and
/// applies the correct per-bus net-injection arithmetic for generator and load
/// actions.
///
/// ## v0.8.0 T9: switch-action physics
///
/// Switching actions are now physically modeled by rebuilding the Y-Bus with
/// the targeted branches removed (`with_opened_branches`):
/// - `ExecuteDevice{open/分闸}` opens the branch identified by `device_id`.
/// - `IsolateFault` opens both the upstream and downstream branches.
/// - `CloseTieSwitch` is simulated as the base case (re-closing not yet modeled).
/// - `ExecuteDevice{close/合闸}` is inapplicable (re-closing not yet modeled).
///
/// The old `conservative_switching_reject` path is deprecated but retained for
/// backward compatibility.
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
                device_id,
                operation,
                value,
            } => {
                match operation.as_str() {
                    // Reactive power adjustment: physically modeled (q_spec edit).
                    "adjust_reactive" => self.simulate_reactive(&net, action, *value),
                    // v0.8.0 T9.3：开关分闸物理建模——将 device_id 解释为支路 ID，
                    // 断开对应支路并重建 Y-Bus 求解。
                    "open" | "分闸" => match self.find_branch_index(&net, *device_id) {
                        Some(idx) => {
                            self.simulate_with_opened_branches(&net, &[idx], action)
                        }
                        None => inapplicable_result(format!(
                            "open: device_id {} not found in branch_ids",
                            device_id
                        )),
                    },
                    // v0.8.0 T9.3：合闸操作需要恢复已断开的支路，当前
                    // `with_opened_branches` 只能断开，不支持恢复，故报不可应用。
                    "close" | "合闸" => inapplicable_result(format!(
                        "close: re-closing not yet modeled (device_id={})",
                        device_id
                    )),
                    _ => inapplicable_result(format!(
                        "unknown ExecuteDevice operation: {}",
                        operation
                    )),
                }
            }

            // v0.8.0 T9.4：故障隔离物理建模——断开故障段两侧开关（解释为支路 ID）。
            StructuredAction::IsolateFault {
                upstream_switch,
                downstream_switch,
            } => {
                let mut indices: Vec<usize> = Vec::new();
                match self.find_branch_index(&net, *upstream_switch) {
                    Some(idx) => indices.push(idx),
                    None => {
                        return inapplicable_result(format!(
                            "IsolateFault: upstream_switch {} not found in branch_ids",
                            upstream_switch
                        ))
                    }
                }
                match self.find_branch_index(&net, *downstream_switch) {
                    Some(idx) => indices.push(idx),
                    None => {
                        return inapplicable_result(format!(
                            "IsolateFault: downstream_switch {} not found in branch_ids",
                            downstream_switch
                        ))
                    }
                }
                self.simulate_with_opened_branches(&net, &indices, action)
            }

            // v0.8.0 T9.5：合联络开关——当前网络中该支路可能已断开，合上意味着
            // 恢复。由于 `with_opened_branches` 只能断开，这里按原始网络（不解列）
            // 求解，并在 summary 中注明尚未建模恢复。
            StructuredAction::CloseTieSwitch { switch_id } => {
                let mut result = self.simulate_base_case(&net);
                result.summary = format!(
                    "tie switch close (switch_id={}): simulated as base case (re-closing not yet modeled); {}",
                    switch_id, result.summary
                );
                result
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

    /// v0.8.0 T9.2：根据开关状态重建 Y-Bus 并求解。
    ///
    /// 调用 `net.with_opened_branches(open_indices)` 创建断开指定支路后的网络副本，
    /// 再用 `solve_and_check` 求解并检查约束。若潮流不收敛，返回 `converged=false`
    /// 但 `applicable=true`（动作本身可应用，只是物理上无解）。
    fn simulate_with_opened_branches(
        &self,
        net: &PowerNetwork,
        open_indices: &[usize],
        action: &StructuredAction,
    ) -> WhatIfResult {
        let modified = net.with_opened_branches(open_indices);
        let mut result = solve_and_check(&modified);
        // 在 summary 前缀标注动作与断开支路数，便于追溯
        result.summary = format!(
            "{:?}: opened {} branch(es); {}",
            action,
            open_indices.len(),
            result.summary
        );
        result
    }

    /// v0.8.0 T9.3：将支路 ID 映射到 `branches` 中的索引。
    ///
    /// 开关动作中的 `device_id` / `upstream_switch` / `downstream_switch` /
    /// `switch_id` 均解释为支路 ID，通过 `branch_ids()` 查找对应索引。
    fn find_branch_index(&self, net: &PowerNetwork, branch_id: ElementId) -> Option<usize> {
        net.branch_ids().iter().position(|&id| id == branch_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_constraint::projector::NetworkSimulator;
    use std::sync::Arc;

    fn make_adapter() -> NetworkSimulatorAdapter {
        let network = Arc::new(parking_lot::RwLock::new(PowerNetwork::from_ieee14()));
        NetworkSimulatorAdapter::new(network)
    }

    // v0.8.0 T9.7：开关分闸物理建模——断开支路 1（1→2），网络仍连通（经 1→5→2），
    // 应返回 applicable=true、converged=true，且不再包含保守拒绝的 summary。
    #[test]
    fn test_switch_open_physical() {
        let adapter = make_adapter();
        let action = StructuredAction::ExecuteDevice {
            device_id: 1,
            operation: "open".to_string(),
            value: 0.0,
        };
        let result = adapter.simulate_action(&action);

        assert!(
            result.applicable,
            "open branch 1 must be applicable: {}",
            result.summary
        );
        // 不再返回保守拒绝的 summary
        assert!(
            !result.summary.contains("conservative reject"),
            "summary must not be conservative reject, got: {}",
            result.summary
        );
        // 支路 1（1→2）断开后网络仍连通（1→5→2），潮流应收敛
        assert!(
            result.converged,
            "network stays connected after opening branch 1, expected converged=true: {}",
            result.summary
        );
        // summary 应标注断开了 1 条支路
        assert!(
            result.summary.contains("opened 1 branch(es)"),
            "summary should mention opened 1 branch: {}",
            result.summary
        );
    }

    // v0.8.0 T9.7：未知 device_id 应返回 inapplicable
    #[test]
    fn test_switch_open_unknown_branch_inapplicable() {
        let adapter = make_adapter();
        let action = StructuredAction::ExecuteDevice {
            device_id: 999,
            operation: "open".to_string(),
            value: 0.0,
        };
        let result = adapter.simulate_action(&action);
        assert!(
            !result.applicable,
            "unknown branch id must be inapplicable: {}",
            result.summary
        );
    }

    // v0.8.0 T9.7：close 操作当前不支持（只能断开，不能恢复），应返回 inapplicable
    #[test]
    fn test_switch_close_not_modeled() {
        let adapter = make_adapter();
        let action = StructuredAction::ExecuteDevice {
            device_id: 1,
            operation: "close".to_string(),
            value: 0.0,
        };
        let result = adapter.simulate_action(&action);
        assert!(
            !result.applicable,
            "close must be inapplicable until re-closing is modeled: {}",
            result.summary
        );
    }

    // v0.8.0 T9.8：故障隔离物理建模——断开支路 1（1→2）与支路 2（1→5），
    // 这会孤立 slack 母线 1，潮流可能不收敛，但动作本身 applicable=true，
    // 且 summary 应标注断开了 2 条支路。
    #[test]
    fn test_isolate_fault_physical() {
        let adapter = make_adapter();
        let action = StructuredAction::IsolateFault {
            upstream_switch: 1,
            downstream_switch: 2,
        };
        let result = adapter.simulate_action(&action);

        assert!(
            result.applicable,
            "IsolateFault must be applicable: {}",
            result.summary
        );
        assert!(
            !result.summary.contains("conservative reject"),
            "summary must not be conservative reject: {}",
            result.summary
        );
        // 两条支路被断开
        assert!(
            result.summary.contains("opened 2 branch(es)"),
            "summary should mention opened 2 branches: {}",
            result.summary
        );
    }

    // v0.8.0 T9.8：故障隔离——未知 upstream_switch 应返回 inapplicable
    #[test]
    fn test_isolate_fault_unknown_upstream_inapplicable() {
        let adapter = make_adapter();
        let action = StructuredAction::IsolateFault {
            upstream_switch: 999,
            downstream_switch: 2,
        };
        let result = adapter.simulate_action(&action);
        assert!(
            !result.applicable,
            "unknown upstream_switch must be inapplicable: {}",
            result.summary
        );
    }

    // v0.8.0 T9.8：合联络开关——按原始网络（base case）求解，applicable=true，
    // summary 应注明按 base case 模拟。
    #[test]
    fn test_close_tie_switch() {
        let adapter = make_adapter();
        let action = StructuredAction::CloseTieSwitch { switch_id: 1 };
        let result = adapter.simulate_action(&action);

        assert!(
            result.applicable,
            "CloseTieSwitch must be applicable: {}",
            result.summary
        );
        assert!(
            !result.summary.contains("conservative reject"),
            "summary must not be conservative reject: {}",
            result.summary
        );
        // 返回 base case 结果
        assert!(
            result.summary.contains("simulated as base case"),
            "summary should mention base case: {}",
            result.summary
        );
        // base case 应收敛
        assert!(
            result.converged,
            "base case should converge: {}",
            result.summary
        );
    }
}
