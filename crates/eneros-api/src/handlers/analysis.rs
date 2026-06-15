use axum::extract::State;
use axum::Json;

use eneros_analysis::{
    BranchLimit, DcOpfProblem, DcOpfSolver, FaultSpec, FaultType, GeneratorBid, MeasType,
    Measurement, SequenceImpedance, ShortCircuitAnalyzer, StateEstimator,
};
use ndarray::Array2;
use num_complex::Complex64;

use crate::app::AppState;
use crate::types::*;

/// POST /api/analysis/opf
pub async fn opf_handler(
    State(state): State<AppState>,
    Json(req): Json<OpfRequest>,
) -> Json<ApiResponse<OpfResponse>> {
    // If network is available, build OPF problem from network data
    if let Some(network) = &state.network {
        let pf_result = match network.solve() {
            Ok(r) => r,
            Err(e) => {
                return Json(ApiResponse::error(format!(
                    "Power flow for OPF failed: {}",
                    e
                )))
            }
        };

        // Build generators from power flow results (use existing generation as bids)
        let generators: Vec<GeneratorBid> = pf_result
            .bus_results
            .iter()
            .enumerate()
            .map(|(i, bus)| GeneratorBid {
                gen_id: bus.bus_id,
                bus_id: bus.bus_id,
                p_min: 0.0,
                p_max: if bus.p_injection > 0.0 {
                    bus.p_injection * 1.5
                } else {
                    100.0
                },
                cost_a: 0.001,
                cost_b: 10.0 + i as f64,
                cost_c: 100.0,
            })
            .collect();

        // Build branches from network
        let branches: Vec<BranchLimit> = pf_result
            .branch_results
            .iter()
            .map(|b| BranchLimit {
                branch_id: b.branch_id,
                from_bus: b.from_bus,
                to_bus: b.to_bus,
                p_limit_mw: 200.0,
                reactance_pu: 0.1,
            })
            .collect();

        // Build loads from negative injections
        let loads: Vec<(u64, f64)> = pf_result
            .bus_results
            .iter()
            .filter(|b| b.p_injection < 0.0)
            .map(|b| (b.bus_id, -b.p_injection))
            .collect();

        let slack_bus = pf_result.bus_results.first().map(|b| b.bus_id).unwrap_or(0);

        let problem = DcOpfProblem {
            generators,
            branches,
            loads,
            slack_bus_id: slack_bus,
        };

        let solver = DcOpfSolver::new();
        match solver.solve(&problem) {
            Ok(result) => {
                let response = OpfResponse {
                    generation: result.result.generation,
                    total_cost: result.result.total_cost,
                    nodal_prices: result.result.nodal_prices,
                    converged: result.converged,
                };
                return Json(ApiResponse::success(response));
            }
            Err(e) => return Json(ApiResponse::error(format!("OPF failed: {}", e))),
        }
    }

    // Fallback: use request data directly
    let generators: Vec<GeneratorBid> = req
        .generators
        .iter()
        .map(|g| GeneratorBid {
            gen_id: g.gen_id,
            bus_id: g.bus_id,
            p_min: g.p_min,
            p_max: g.p_max,
            cost_a: g.cost_a,
            cost_b: g.cost_b,
            cost_c: g.cost_c,
        })
        .collect();

    let branches: Vec<BranchLimit> = req
        .branches
        .iter()
        .map(|b| BranchLimit {
            branch_id: b.branch_id,
            from_bus: b.from_bus,
            to_bus: b.to_bus,
            p_limit_mw: b.p_limit,
            reactance_pu: b.reactance,
        })
        .collect();

    let problem = DcOpfProblem {
        generators,
        branches,
        loads: req.loads,
        slack_bus_id: req.slack_bus,
    };

    let solver = DcOpfSolver::new();
    match solver.solve(&problem) {
        Ok(result) => {
            let response = OpfResponse {
                generation: result.result.generation,
                total_cost: result.result.total_cost,
                nodal_prices: result.result.nodal_prices,
                converged: result.converged,
            };
            Json(ApiResponse::success(response))
        }
        Err(e) => Json(ApiResponse::error(format!("OPF failed: {}", e))),
    }
}

/// POST /api/analysis/state-estimation
pub async fn state_estimation_handler(
    State(state): State<AppState>,
    Json(req): Json<SeRequest>,
) -> Json<ApiResponse<SeResponse>> {
    // If network is available, create synthetic measurements from power flow
    if let Some(network) = &state.network {
        match network.solve() {
            Ok(pf_result) => {
                let measurements: Vec<Measurement> = pf_result
                    .bus_results
                    .iter()
                    .map(|b| Measurement {
                        meas_type: MeasType::VoltageMagnitude,
                        element_id: b.bus_id,
                        value: b.voltage_magnitude,
                        sigma: 0.01,
                    })
                    .collect();

                let bus_count = pf_result.bus_results.len();
                let slack_bus = pf_result.bus_results.first().map(|b| b.bus_id).unwrap_or(0);

                let estimator = StateEstimator::default_estimator();
                match estimator.estimate(&measurements, bus_count, slack_bus) {
                    Ok(result) => {
                        let response = SeResponse {
                            bus_voltages: result.result.bus_voltages,
                            bad_data: result.result.bad_data,
                            converged: result.converged,
                        };
                        return Json(ApiResponse::success(response));
                    }
                    Err(e) => {
                        return Json(ApiResponse::error(format!(
                            "State estimation failed: {}",
                            e
                        )))
                    }
                }
            }
            Err(e) => {
                return Json(ApiResponse::error(format!(
                    "Power flow for SE failed: {}",
                    e
                )))
            }
        }
    }

    // Fallback: use request data
    let measurements: Vec<Measurement> = req
        .measurements
        .iter()
        .map(|m| {
            let meas_type = match m.meas_type.to_lowercase().as_str() {
                "voltage" | "voltage_magnitude" => MeasType::VoltageMagnitude,
                "bus_p" | "bus_injection_p" => MeasType::BusInjectionP,
                "bus_q" | "bus_injection_q" => MeasType::BusInjectionQ,
                "branch_p" | "branch_flow_p" => MeasType::BranchFlowP,
                "branch_q" | "branch_flow_q" => MeasType::BranchFlowQ,
                _ => MeasType::VoltageMagnitude,
            };
            Measurement {
                meas_type,
                element_id: m.element_id,
                value: m.value,
                sigma: m.sigma,
            }
        })
        .collect();

    let estimator = StateEstimator::default_estimator();
    match estimator.estimate(&measurements, req.bus_count, req.slack_bus) {
        Ok(result) => {
            let response = SeResponse {
                bus_voltages: result.result.bus_voltages,
                bad_data: result.result.bad_data,
                converged: result.converged,
            };
            Json(ApiResponse::success(response))
        }
        Err(e) => Json(ApiResponse::error(format!(
            "State estimation failed: {}",
            e
        ))),
    }
}

/// Build Z-bus matrix by inverting Y-bus
fn build_z_bus(ybus: &eneros_powerflow::YBusMatrix) -> Option<Array2<Complex64>> {
    let n = ybus.size();
    if n == 0 {
        return Some(Array2::zeros((0, 0)));
    }

    // Build complex Y-bus matrix as Vec<Vec<Complex64>>
    let mut y_matrix = vec![vec![Complex64::new(0.0, 0.0); n]; n];
    #[allow(clippy::needless_range_loop)]
    for i in 0..n {
        for j in 0..n {
            let (g, b) = ybus.get(i, j);
            y_matrix[i][j] = Complex64::new(g, b);
        }
    }

    // Invert using shared linalg utility
    eneros_core::invert_complex_matrix(&y_matrix).map(|inv| {
        let mut z_bus = Array2::<Complex64>::zeros((n, n));
        for i in 0..n {
            for j in 0..n {
                z_bus[[i, j]] = inv[i][j];
            }
        }
        z_bus
    })
}

/// POST /api/analysis/short-circuit
pub async fn short_circuit_handler(
    State(state): State<AppState>,
    Json(req): Json<ScRequest>,
) -> Json<ApiResponse<ScResponse>> {
    let fault_type = match req.fault_type.to_lowercase().as_str() {
        "three_phase" | "3ph" => FaultType::ThreePhase,
        "slg" | "single_line_ground" => FaultType::SingleLineGround,
        "ll" | "line_line" => FaultType::LineLine,
        "dlg" | "double_line_ground" => FaultType::DoubleLineGround,
        _ => FaultType::ThreePhase,
    };

    let fault = FaultSpec {
        bus_id: req.bus_id,
        fault_type,
        fault_impedance: Complex64::new(req.fault_impedance_real, req.fault_impedance_imag),
    };

    // If network is available, build Z-bus from network data and analyze
    if let Some(network) = &state.network {
        let pf_result = match network.solve() {
            Ok(r) => r,
            Err(e) => {
                return Json(ApiResponse::error(format!(
                    "Power flow for SC failed: {}",
                    e
                )))
            }
        };

        let z_bus = match build_z_bus(network.ybus()) {
            Some(z) => z,
            None => {
                return Json(ApiResponse::error(
                    "Failed to build Z-bus matrix (singular Y-bus)".to_string(),
                ))
            }
        };

        let prefault_voltages: Vec<Complex64> = pf_result
            .bus_results
            .iter()
            .map(|b| Complex64::from_polar(b.voltage_magnitude, b.voltage_angle))
            .collect();

        // For asymmetric faults, provide sequence impedances
        let seq_z = if fault_type != FaultType::ThreePhase {
            let z_ff = z_bus[[req.bus_id as usize, req.bus_id as usize]];
            Some(SequenceImpedance {
                z1: z_ff,
                z2: z_ff,                      // Assume z2 = z1 for simplicity
                z0: Complex64::new(0.03, 0.3), // Typical zero-sequence impedance
            })
        } else {
            None
        };

        let analyzer = ShortCircuitAnalyzer::new();
        match analyzer.analyze(&fault, &z_bus, &prefault_voltages, seq_z.as_ref()) {
            Ok(result) => {
                let response = ScResponse {
                    fault_current_real: result.fault_current_ka.re,
                    fault_current_imag: result.fault_current_ka.im,
                    bus_voltages: result
                        .bus_voltages
                        .iter()
                        .map(|(id, v)| (*id, v.re, v.im))
                        .collect(),
                };
                return Json(ApiResponse::success(response));
            }
            Err(e) => {
                return Json(ApiResponse::error(format!(
                    "Short circuit analysis failed: {}",
                    e
                )))
            }
        }
    }

    // No network model available
    Json(ApiResponse::error(
        "Short circuit analysis requires a loaded network model. Configure a PowerNetwork first."
            .to_string(),
    ))
}
