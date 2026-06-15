use axum::extract::State;
use axum::Json;

use eneros_powerflow::ieee14;

use crate::app::AppState;
use crate::types::{
    ApiResponse, BranchFlowResponse, BusVoltageResponse, PowerFlowRequest, PowerFlowResponse,
};

/// POST /api/power-flow
pub async fn power_flow_handler(
    State(state): State<AppState>,
    Json(req): Json<PowerFlowRequest>,
) -> Json<ApiResponse<PowerFlowResponse>> {
    // Try PowerNetwork first (has IEEE 14 default)
    if let Some(network) = &state.network {
        match network.solve() {
            Ok(result) => {
                let response = PowerFlowResponse {
                    converged: result.converged,
                    iterations: result.iterations,
                    total_losses: result.total_losses,
                    bus_voltages: result
                        .bus_results
                        .iter()
                        .map(|b| BusVoltageResponse {
                            bus_id: b.bus_id,
                            voltage_magnitude: b.voltage_magnitude,
                            voltage_angle: b.voltage_angle,
                        })
                        .collect(),
                    branch_flows: result
                        .branch_results
                        .iter()
                        .map(|b| BranchFlowResponse {
                            branch_id: b.branch_id,
                            from_bus: b.from_bus,
                            to_bus: b.to_bus,
                            active_power_mw: b.p_from,
                            reactive_power_mvar: b.q_from,
                            loading_percent: b.loading_percent,
                        })
                        .collect(),
                };
                return Json(ApiResponse::success(response));
            }
            Err(e) => return Json(ApiResponse::error(format!("Power flow failed: {}", e))),
        }
    }

    // Try standalone PowerFlowSolver with IEEE 14 default
    if let Some(_solver) = &state.powerflow_solver {
        let data = ieee14();
        let (ybus, p_spec, q_spec, bus_types) = data.to_solver_input();
        let v_initial: Vec<f64> = data.buses.iter().map(|b| b.v_pu).collect();

        let max_iter = req.max_iterations.unwrap_or(100);
        let tolerance = req.tolerance.unwrap_or(1e-8);

        let solver = eneros_powerflow::PowerFlowSolver::new(max_iter, tolerance);
        match solver.solve_with_initial(&ybus, &p_spec, &q_spec, &bus_types, Some(&v_initial)) {
            Ok(result) => {
                let response = PowerFlowResponse {
                    converged: result.converged,
                    iterations: result.iterations,
                    total_losses: result.total_losses,
                    bus_voltages: result
                        .bus_results
                        .iter()
                        .map(|b| BusVoltageResponse {
                            bus_id: b.bus_id,
                            voltage_magnitude: b.voltage_magnitude,
                            voltage_angle: b.voltage_angle,
                        })
                        .collect(),
                    branch_flows: result
                        .branch_results
                        .iter()
                        .map(|b| BranchFlowResponse {
                            branch_id: b.branch_id,
                            from_bus: b.from_bus,
                            to_bus: b.to_bus,
                            active_power_mw: b.p_from,
                            reactive_power_mvar: b.q_from,
                            loading_percent: b.loading_percent,
                        })
                        .collect(),
                };
                return Json(ApiResponse::success(response));
            }
            Err(e) => return Json(ApiResponse::error(format!("Power flow failed: {}", e))),
        }
    }

    // No solver available — run IEEE 14 inline as default
    let data = ieee14();
    let (ybus, p_spec, q_spec, bus_types) = data.to_solver_input();
    let v_initial: Vec<f64> = data.buses.iter().map(|b| b.v_pu).collect();

    let max_iter = req.max_iterations.unwrap_or(100);
    let tolerance = req.tolerance.unwrap_or(1e-8);

    let solver = eneros_powerflow::PowerFlowSolver::new(max_iter, tolerance);
    match solver.solve_with_initial(&ybus, &p_spec, &q_spec, &bus_types, Some(&v_initial)) {
        Ok(result) => {
            let response = PowerFlowResponse {
                converged: result.converged,
                iterations: result.iterations,
                total_losses: result.total_losses,
                bus_voltages: result
                    .bus_results
                    .iter()
                    .map(|b| BusVoltageResponse {
                        bus_id: b.bus_id,
                        voltage_magnitude: b.voltage_magnitude,
                        voltage_angle: b.voltage_angle,
                    })
                    .collect(),
                branch_flows: result
                    .branch_results
                    .iter()
                    .map(|b| BranchFlowResponse {
                        branch_id: b.branch_id,
                        from_bus: b.from_bus,
                        to_bus: b.to_bus,
                        active_power_mw: b.p_from,
                        reactive_power_mvar: b.q_from,
                        loading_percent: b.loading_percent,
                    })
                    .collect(),
            };
            Json(ApiResponse::success(response))
        }
        Err(e) => Json(ApiResponse::error(format!("Power flow failed: {}", e))),
    }
}
