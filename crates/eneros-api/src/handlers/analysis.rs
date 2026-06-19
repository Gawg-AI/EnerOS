use axum::extract::State;
use axum::Json;

use eneros_analysis::{
    AcBranch, AcBus, AcGenerator, AcOpfProblem, AcOpfSolver, BadDataDetector, BranchLimit,
    DcOpfProblem, DcOpfSolver, FaultSpec, FaultType, GeneratorBid, GeneratorDynamic,
    GeneratorModel, IntegrationMethod, MeasType, Measurement, ObservabilityAnalyzer,
    ObservabilityMethod, OpfMethod, SequenceImpedance, SequenceNetworks, ShortCircuitAnalyzer,
    SimulationParams, StateEstimator, TransientFault, TransientScenario,
    TransientStabilityAnalyzer,
};
use ndarray::Array2;
use num_complex::Complex64;

use crate::app::AppState;
use crate::types::*;

/// POST /api/analysis/opf
#[utoipa::path(
    post,
    path = "/api/analysis/opf",
    request_body = OpfRequest,
    responses(
        (status = 200, description = "最优潮流计算成功", body = OpfResponse),
        (status = 400, description = "请求参数错误或求解失败"),
    )
)]
#[tracing::instrument(skip(state, req), fields(endpoint = "/api/analysis/opf"))]
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
#[tracing::instrument(skip(state, req), fields(endpoint = "/api/analysis/state-estimation"))]
pub async fn state_estimation_handler(
    State(state): State<AppState>,
    Json(req): Json<SeRequest>,
) -> Json<ApiResponse<SeResponse>> {
    // If network is available, create synthetic measurements from power flow
    if let Some(network) = &state.network {
        match network.solve() {
            Ok(pf_result) => {
                // 构建反向映射：内部索引 → 外部母线 ID
                let idx_to_bus: std::collections::HashMap<usize, u64> = network
                    .bus_map()
                    .iter()
                    .map(|(&ext_id, &idx)| (idx, ext_id))
                    .collect();

                let measurements: Vec<Measurement> = pf_result
                    .bus_results
                    .iter()
                    .flat_map(|b| {
                        let ext_id = idx_to_bus.get(&(b.bus_id as usize)).copied().unwrap_or(b.bus_id);
                        vec![
                            Measurement::bus(MeasType::VoltageMagnitude, ext_id, b.voltage_magnitude, 0.01),
                            Measurement::bus(MeasType::BusInjectionP, ext_id, b.p_injection, 0.5),
                            Measurement::bus(MeasType::BusInjectionQ, ext_id, b.q_injection, 0.5),
                        ]
                    })
                    .collect();

                // Also add branch flow measurements
                let branch_measurements: Vec<Measurement> = pf_result
                    .branch_results
                    .iter()
                    .flat_map(|br| {
                        let from_ext = idx_to_bus.get(&(br.from_bus as usize)).copied().unwrap_or(br.from_bus);
                        let to_ext = idx_to_bus.get(&(br.to_bus as usize)).copied().unwrap_or(br.to_bus);
                        vec![
                            Measurement::branch(MeasType::BranchFlowP, from_ext, to_ext, br.p_from, 0.3),
                            Measurement::branch(MeasType::BranchFlowQ, from_ext, to_ext, br.q_from, 0.3),
                        ]
                    })
                    .collect();
                let all_measurements = [measurements, branch_measurements].concat();

                // 使用 bus_map 中映射到索引 0 的外部母线作为平衡母线
                let slack_bus = network
                    .bus_map()
                    .iter()
                    .find(|(_, &idx)| idx == 0)
                    .map(|(&ext_id, _)| ext_id)
                    .unwrap_or(1);

                // Use network-based estimation if Y-bus is available
                let network_model = eneros_analysis::NetworkModel::new(
                    network.ybus().clone(),
                    network.bus_map().clone(),
                    100.0,
                );
                let estimator = StateEstimator::default_estimator();
                match estimator.estimate_with_network(&all_measurements, &network_model, slack_bus) {
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
            Measurement::bus(meas_type, m.element_id, m.value, m.sigma)
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
#[tracing::instrument(skip(state, req), fields(endpoint = "/api/analysis/short-circuit"))]
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

// ============================================================
// v0.8.0 — AC-OPF, Transient, Observability, Bad Data, Asymmetric SC
// ============================================================

/// 构建合成测量集（从网络潮流结果生成电压 + 支路潮流测量）
///
/// 用于可观测性分析、不良数据检测等端点：当请求未提供测量集时，
/// 使用网络潮流结果合成一组物理一致的测量，使分析能正常运行。
///
/// **注意**：潮流结果的 `bus_id` 是内部索引（0-based），而 `bus_map`
/// 使用外部母线 ID（IEEE-14 为 1-based）。此处通过反向映射将内部索引
/// 转换为外部母线 ID，确保与 `NetworkModel.bus_map` 一致。
fn build_synthetic_measurements(
    network: &eneros_network::PowerNetwork,
) -> Result<(Vec<Measurement>, u64), String> {
    let pf_result = network
        .solve()
        .map_err(|e| format!("Power flow failed: {}", e))?;

    // 构建反向映射：内部索引 → 外部母线 ID（只读，不 remove，避免支路映射丢失）
    let idx_to_bus: std::collections::HashMap<usize, u64> = network
        .bus_map()
        .iter()
        .map(|(&ext_id, &idx)| (idx, ext_id))
        .collect();

    // sigma 选择：电压 0.01 p.u.（权重 1e4），注入/支路 2.0/1.5 MW（权重 0.25/0.44），
    // 权重比 ~4e4，通过 Tikhonov 正则化可保证增益矩阵良态。
    let mut measurements: Vec<Measurement> = pf_result
        .bus_results
        .iter()
        .flat_map(|b| {
            let ext_id = idx_to_bus.get(&(b.bus_id as usize)).copied().unwrap_or(b.bus_id);
            vec![
                Measurement::bus(MeasType::VoltageMagnitude, ext_id, b.voltage_magnitude, 0.01),
                Measurement::bus(MeasType::BusInjectionP, ext_id, b.p_injection, 2.0),
                Measurement::bus(MeasType::BusInjectionQ, ext_id, b.q_injection, 2.0),
            ]
        })
        .collect();

    let branch_measurements: Vec<Measurement> = pf_result
        .branch_results
        .iter()
        .flat_map(|br| {
            // 支路结果中的 from_bus/to_bus 也是内部索引，需要转换
            let from_ext = idx_to_bus.get(&(br.from_bus as usize)).copied().unwrap_or(br.from_bus);
            let to_ext = idx_to_bus.get(&(br.to_bus as usize)).copied().unwrap_or(br.to_bus);
            vec![
                Measurement::branch(MeasType::BranchFlowP, from_ext, to_ext, br.p_from, 1.5),
                Measurement::branch(MeasType::BranchFlowQ, from_ext, to_ext, br.q_from, 1.5),
            ]
        })
        .collect();
    measurements.extend(branch_measurements);

    // 使用 bus_map 中映射到索引 0 的外部母线作为平衡母线
    let slack_bus = network
        .bus_map()
        .iter()
        .find(|(_, &idx)| idx == 0)
        .map(|(&ext_id, _)| ext_id)
        .unwrap_or(1);

    Ok((measurements, slack_bus))
}

/// 将 MeasRequest 转换为 Measurement（用于可观测性 / 不良数据端点）
fn meas_request_to_measurement(m: &MeasRequest) -> Measurement {
    let meas_type = match m.meas_type.to_lowercase().as_str() {
        "voltage" | "voltage_magnitude" => MeasType::VoltageMagnitude,
        "bus_p" | "bus_injection_p" => MeasType::BusInjectionP,
        "bus_q" | "bus_injection_q" => MeasType::BusInjectionQ,
        "branch_p" | "branch_flow_p" => MeasType::BranchFlowP,
        "branch_q" | "branch_flow_q" => MeasType::BranchFlowQ,
        "pmu_voltage" => MeasType::PmuVoltage,
        "pmu_current" => MeasType::PmuCurrent,
        _ => MeasType::VoltageMagnitude,
    };
    Measurement {
        meas_type,
        element_id: m.element_id,
        to_element_id: None,
        value: m.value,
        sigma: m.sigma,
        value_imag: 0.0,
    }
}

/// POST /api/analysis/ac-opf
///
/// 执行交流最优潮流计算。支持牛顿-拉夫逊法和原对偶内点法。
/// 若请求未提供发电机/支路/母线数据，则从已加载的网络模型构建。
#[tracing::instrument(skip(state, req), fields(endpoint = "/api/analysis/ac-opf"))]
pub async fn ac_opf_handler(
    State(state): State<AppState>,
    Json(req): Json<AcOpfRequest>,
) -> Json<ApiResponse<AcOpfResponse>> {
    // 解析求解方法
    let method = match req.method.as_deref().unwrap_or("newton").to_lowercase().as_str() {
        "newton" | "newton_raphson" | "newton-raphson" => OpfMethod::NewtonRaphson,
        "interior_point" | "ipm" | "pdipm" => OpfMethod::InteriorPoint,
        _ => OpfMethod::NewtonRaphson,
    };

    let base_mva = req.base_mva.unwrap_or(100.0);
    let slack_bus = req.slack_bus.unwrap_or(1);

    // 优先使用请求中的自定义数据
    let (buses, generators, branches) = if !req.buses.is_empty() {
        // 使用请求中的完整数据
        let buses: Vec<AcBus> = req
            .buses
            .iter()
            .map(|b| AcBus {
                bus_id: b.bus_id,
                p_load: b.p_load,
                q_load: b.q_load,
                v_min: b.v_min,
                v_max: b.v_max,
                v_init: b.v_init,
                theta_init: b.theta_init,
            })
            .collect();
        let generators: Vec<AcGenerator> = req
            .generators
            .iter()
            .map(|g| AcGenerator {
                gen_id: g.gen_id,
                bus_id: g.bus_id,
                p_min: g.p_min,
                p_max: g.p_max,
                q_min: g.q_min,
                q_max: g.q_max,
                cost_a: g.cost_a,
                cost_b: g.cost_b,
                cost_c: g.cost_c,
            })
            .collect();
        let branches: Vec<AcBranch> = req
            .branches
            .iter()
            .map(|b| AcBranch {
                branch_id: b.branch_id,
                from_bus: b.from_bus,
                to_bus: b.to_bus,
                r_pu: b.r_pu,
                x_pu: b.x_pu,
                b_half: b.b_half,
                tap_ratio: b.tap_ratio,
                s_limit_mva: b.s_limit_mva,
            })
            .collect();
        (buses, generators, branches)
    } else if let Some(network) = &state.network {
        // 从已加载网络构建
        let pf_result = match network.solve() {
            Ok(r) => r,
            Err(e) => {
                return Json(ApiResponse::error(format!(
                    "Power flow for AC-OPF initialization failed: {}",
                    e
                )))
            }
        };

        // 构建反向映射：内部索引 → 外部母线 ID
        let idx_to_bus: std::collections::HashMap<usize, u64> = network
            .bus_map()
            .iter()
            .map(|(&ext_id, &idx)| (idx, ext_id))
            .collect();

        // 从潮流结果构建母线（使用外部母线 ID）
        let buses: Vec<AcBus> = pf_result
            .bus_results
            .iter()
            .map(|b| {
                let ext_id = idx_to_bus.get(&(b.bus_id as usize)).copied().unwrap_or(b.bus_id);
                AcBus {
                    bus_id: ext_id,
                    p_load: if b.p_injection < 0.0 { -b.p_injection } else { 0.0 },
                    q_load: if b.q_injection < 0.0 { -b.q_injection } else { 0.0 },
                    v_min: 0.95,
                    v_max: 1.05,
                    v_init: b.voltage_magnitude,
                    theta_init: b.voltage_angle,
                }
            })
            .collect();

        // 从发电机表构建（若有），否则从正注入构建
        let generators: Vec<AcGenerator> = if !network.generator_table().is_empty() {
            network
                .generator_table()
                .iter()
                .enumerate()
                .map(|(i, g)| AcGenerator {
                    gen_id: g.gen_id,
                    bus_id: g.bus_id,
                    p_min: g.p_min_mw,
                    p_max: g.p_max_mw,
                    q_min: -50.0,
                    q_max: 150.0,
                    cost_a: 0.005,
                    cost_b: 10.0 + i as f64,
                    cost_c: 100.0,
                })
                .collect()
        } else {
            pf_result
                .bus_results
                .iter()
                .enumerate()
                .filter(|(_, b)| b.p_injection > 0.0)
                .map(|(i, b)| {
                    let ext_id = idx_to_bus.get(&(b.bus_id as usize)).copied().unwrap_or(b.bus_id);
                    AcGenerator {
                        gen_id: ext_id,
                        bus_id: ext_id,
                        p_min: 0.0,
                        p_max: b.p_injection * 1.5,
                        q_min: -50.0,
                        q_max: 150.0,
                        cost_a: 0.005,
                        cost_b: 10.0 + i as f64,
                        cost_c: 100.0,
                    }
                })
                .collect()
        };

        // 从网络支路数据构建
        let branches: Vec<AcBranch> = network
            .branches_data()
            .iter()
            .enumerate()
            .map(|(i, (from, to, r, x, b, tap))| AcBranch {
                branch_id: i as u64,
                from_bus: *from,
                to_bus: *to,
                r_pu: *r,
                x_pu: *x,
                b_half: *b,
                tap_ratio: *tap,
                s_limit_mva: 200.0,
            })
            .collect();

        (buses, generators, branches)
    } else {
        return Json(ApiResponse::error(
            "AC-OPF requires either request data (buses/generators/branches) or a loaded network model.".to_string(),
        ));
    };

    if buses.is_empty() {
        return Json(ApiResponse::error("No buses available for AC-OPF".to_string()));
    }
    if generators.is_empty() {
        return Json(ApiResponse::error("No generators available for AC-OPF".to_string()));
    }

    let problem = AcOpfProblem {
        buses,
        generators,
        branches,
        slack_bus_id: slack_bus,
        base_mva,
    };

    let solver = AcOpfSolver::new();
    match solver.solve(&problem, method) {
        Ok(result) => {
            let response = AcOpfResponse {
                generation: result.result.generation,
                bus_voltages: result.result.bus_voltages,
                branch_flows: result.result.branch_flows,
                nodal_prices: result.result.nodal_prices,
                total_cost: result.result.total_cost,
                total_losses: result.result.total_losses,
                converged: result.converged,
                iterations: result.iterations,
                warnings: result.warnings,
            };
            Json(ApiResponse::success(response))
        }
        Err(e) => Json(ApiResponse::error(format!("AC-OPF failed: {}", e))),
    }
}

/// POST /api/analysis/transient
///
/// 执行暂态稳定分析。支持三种模式：
/// - `simulate`: 单次暂态仿真，返回完整时间序列
/// - `cct`: 临界故障清除时间二分搜索
/// - `equal_area`: 等面积法则快速判定（单机无穷大系统）
#[tracing::instrument(skip(state, req), fields(endpoint = "/api/analysis/transient"))]
pub async fn transient_handler(
    State(state): State<AppState>,
    Json(req): Json<TransientRequest>,
) -> Json<ApiResponse<TransientResponse>> {
    let analyzer = TransientStabilityAnalyzer::new();

    // 等面积法则模式（独立路径，不需要完整网络）
    if req.mode == "equal_area" {
        let ea_params = match &req.equal_area {
            Some(p) => p,
            None => {
                return Json(ApiResponse::error(
                    "equal_area mode requires `equal_area` parameters".to_string(),
                ))
            }
        };

        // 构造单台发电机（使用请求中的第一台或默认值）
        let gen = if let Some(g) = req.generators.first() {
            GeneratorDynamic {
                gen_id: g.gen_id,
                bus_id: g.bus_id,
                model: if g.model == "fourth_order" {
                    GeneratorModel::FourthOrder
                } else {
                    GeneratorModel::Classical2nd
                },
                h: g.h,
                d: g.d,
                xd_prime: g.xd_prime,
                xd: if g.xd > 0.0 { g.xd } else { g.xd_prime * 1.5 },
                efd: if g.efd > 0.0 { g.efd } else { 1.0 },
                pm: g.pm,
                ka: g.ka,
                ta: g.ta,
            }
        } else {
            // 默认单机参数
            GeneratorDynamic {
                gen_id: 1,
                bus_id: 1,
                model: GeneratorModel::Classical2nd,
                h: 5.0,
                d: 2.0,
                xd_prime: 0.3,
                xd: 0.4,
                efd: 1.05,
                pm: 0.8,
                ka: 0.0,
                ta: 0.0,
            }
        };

        match analyzer.equal_area_criterion(
            &gen,
            ea_params.v_inf,
            ea_params.x_pre_fault,
            ea_params.x_fault,
            ea_params.x_post_fault,
            ea_params.frequency,
        ) {
            Ok(result) => {
                let ea = &result.result;
                let response = TransientResponse {
                    mode: "equal_area".to_string(),
                    stable: ea.stable,
                    max_angle_spread_deg: (ea.delta_c_critical - ea.delta_0).to_degrees().abs(),
                    time_series: Vec::new(),
                    warnings: result.warnings,
                    cct: None,
                    equal_area: Some(EqualAreaResponse {
                        delta_0: ea.delta_0,
                        delta_c_critical: ea.delta_c_critical,
                        delta_max: ea.delta_max,
                        a_accel: ea.a_accel,
                        a_decel: ea.a_decel,
                        cct: ea.cct,
                        stable: ea.stable,
                        pmax_pre: ea.pmax_pre,
                        pmax_fault: ea.pmax_fault,
                        pmax_post: ea.pmax_post,
                    }),
                };
                Json(ApiResponse::success(response))
            }
            Err(e) => Json(ApiResponse::error(format!(
                "Equal area criterion failed: {}",
                e
            ))),
        }
    } else {
        // simulate / cct 模式：需要构建完整 TransientScenario
        let (buses, branches, base_mva) = if !req.buses.is_empty() {
            (
                req.buses.clone(),
                req.branches.clone(),
                req.base_mva,
            )
        } else if let Some(network) = &state.network {
            let buses: Vec<u64> = (0..network.bus_count() as u64).collect();
            let branches: Vec<(u64, u64, f64, f64, f64, f64)> = network
                .branches_data()
                .iter()
                .map(|(from, to, r, x, b, tap)| (*from, *to, *r, *x, *b, *tap))
                .collect();
            (buses, branches, req.base_mva)
        } else {
            return Json(ApiResponse::error(
                "Transient analysis requires either request data (buses/branches) or a loaded network model.".to_string(),
            ));
        };

        // 构建发电机
        let generators: Vec<GeneratorDynamic> = if !req.generators.is_empty() {
            req.generators
                .iter()
                .map(|g| GeneratorDynamic {
                    gen_id: g.gen_id,
                    bus_id: g.bus_id,
                    model: if g.model == "fourth_order" {
                        GeneratorModel::FourthOrder
                    } else {
                        GeneratorModel::Classical2nd
                    },
                    h: g.h,
                    d: g.d,
                    xd_prime: g.xd_prime,
                    xd: if g.xd > 0.0 { g.xd } else { g.xd_prime * 1.5 },
                    efd: if g.efd > 0.0 { g.efd } else { 1.05 },
                    pm: g.pm,
                    ka: g.ka,
                    ta: g.ta,
                })
                .collect()
        } else if let Some(network) = &state.network {
            // 从网络发电机表构建默认动态参数
            network
                .generator_table()
                .iter()
                .map(|g| GeneratorDynamic {
                    gen_id: g.gen_id,
                    bus_id: g.bus_id,
                    model: GeneratorModel::Classical2nd,
                    h: 5.0,
                    d: 2.0,
                    xd_prime: 0.3,
                    xd: 0.4,
                    efd: 1.05,
                    pm: (g.net_p_mw() / base_mva).max(0.01),
                    ka: 0.0,
                    ta: 0.0,
                })
                .collect()
        } else {
            vec![GeneratorDynamic {
                gen_id: 1,
                bus_id: *buses.first().unwrap_or(&1),
                model: GeneratorModel::Classical2nd,
                h: 5.0,
                d: 2.0,
                xd_prime: 0.3,
                xd: 0.4,
                efd: 1.05,
                pm: 0.8,
                ka: 0.0,
                ta: 0.0,
            }]
        };

        // 构建故障
        let fault = match req.fault_type.to_lowercase().as_str() {
            "three_phase" | "3ph" => {
                let bus_id = req.fault_bus.unwrap_or(*buses.first().unwrap_or(&1));
                TransientFault::ThreePhase {
                    bus_id,
                    fault_impedance: req.fault_impedance.unwrap_or(0.0),
                }
            }
            "line_outage" => {
                let branch_id = req.fault_branch.unwrap_or(0);
                TransientFault::LineOutage { branch_id }
            }
            _ => {
                return Json(ApiResponse::error(format!(
                    "Unknown fault type: {} (supported: three_phase, line_outage)",
                    req.fault_type
                )))
            }
        };

        // 构建仿真参数
        let params = match &req.params {
            Some(p) => SimulationParams {
                t_start: p.t_start,
                t_end: p.t_end,
                dt: p.dt,
                t_fault: p.t_fault,
                t_clear: p.t_clear,
                method: if p.method.to_lowercase() == "implicit_trapezoidal" {
                    IntegrationMethod::ImplicitTrapezoidal
                } else {
                    IntegrationMethod::RK4
                },
                frequency: p.frequency,
            },
            None => SimulationParams::default(),
        };

        let scenario = TransientScenario {
            generators,
            buses,
            branches,
            base_mva,
            fault,
            params: params.clone(),
            loads: req.loads.clone(),
        };

        if req.mode == "cct" {
            // CCT 二分搜索
            match analyzer.compute_cct(
                &scenario,
                req.cct_min,
                req.cct_max,
                req.cct_tolerance,
                30,
            ) {
                Ok(result) => {
                    let cct = &result.result;
                    let response = TransientResponse {
                        mode: "cct".to_string(),
                        stable: cct.max_angle_spread_at_cct_deg < 180.0,
                        max_angle_spread_deg: cct.max_angle_spread_at_cct_deg,
                        time_series: Vec::new(),
                        warnings: result.warnings,
                        cct: Some(CctResponse {
                            cct: cct.cct,
                            tolerance: cct.tolerance,
                            iterations: cct.iterations,
                            max_angle_spread_at_cct_deg: cct.max_angle_spread_at_cct_deg,
                        }),
                        equal_area: None,
                    };
                    Json(ApiResponse::success(response))
                }
                Err(e) => Json(ApiResponse::error(format!("CCT computation failed: {}", e))),
            }
        } else {
            // 单次仿真
            match analyzer.analyze(&scenario) {
                Ok(result) => {
                    let tr = &result.result;
                    let time_series: Vec<TimeStepResponse> = tr
                        .time_series
                        .iter()
                        .map(|step| TimeStepResponse {
                            t: step.t,
                            rotor_angles: step.rotor_angles.clone(),
                            rotor_speeds: step.rotor_speeds.clone(),
                            bus_voltages: step.bus_voltages.clone(),
                        })
                        .collect();
                    let response = TransientResponse {
                        mode: "simulate".to_string(),
                        stable: tr.stable,
                        max_angle_spread_deg: tr.max_angle_spread_deg,
                        time_series,
                        warnings: tr.warnings.clone(),
                        cct: None,
                        equal_area: None,
                    };
                    Json(ApiResponse::success(response))
                }
                Err(e) => Json(ApiResponse::error(format!(
                    "Transient simulation failed: {}",
                    e
                ))),
            }
        }
    }
}

/// POST /api/analysis/observability
///
/// 执行可观测性分析。支持数值法（雅可比矩阵秩）和拓扑法（图论）。
/// 可选计算最小 PMU 配置。
pub async fn observability_handler(
    State(state): State<AppState>,
    Json(req): Json<ObservabilityRequest>,
) -> Json<ApiResponse<ObservabilityResponse>> {
    let network = match &state.network {
        Some(n) => n,
        None => {
            return Json(ApiResponse::error(
                "Observability analysis requires a loaded network model.".to_string(),
            ))
        }
    };

    let network_model = eneros_analysis::NetworkModel::new(
        network.ybus().clone(),
        network.bus_map().clone(),
        100.0,
    );

    // 获取测量集
    let (measurements, default_slack) = if !req.measurements.is_empty() {
        let meas: Vec<Measurement> = req
            .measurements
            .iter()
            .map(meas_request_to_measurement)
            .collect();
        (meas, req.slack_bus.unwrap_or(1))
    } else {
        // 合成测量
        match build_synthetic_measurements(network) {
            Ok((m, slack)) => (m, req.slack_bus.unwrap_or(slack)),
            Err(e) => return Json(ApiResponse::error(e)),
        }
    };

    let analyzer = ObservabilityAnalyzer::new();
    let method = if req.method.to_lowercase() == "topological" {
        ObservabilityMethod::Topological
    } else {
        ObservabilityMethod::Numerical
    };

    let result = if matches!(method, ObservabilityMethod::Numerical) {
        analyzer.analyze_numerical(&measurements, &network_model, default_slack)
    } else {
        analyzer.analyze_topological(&measurements, &network_model, default_slack)
    };

    match result {
        Ok(analysis) => {
            let r = &analysis.result;
            // 可选：计算最小 PMU 配置
            let pmu_placement = if req.compute_pmu_placement {
                let placement = analyzer.optimal_pmu_placement(
                    &network_model,
                    &req.existing_pmu_buses,
                );
                Some(PmuPlacementResponse {
                    pmu_buses: placement.pmu_buses.clone(),
                    coverage: placement.coverage,
                    covered_buses: placement.covered_buses.clone(),
                    pmu_count: placement.pmu_count,
                    total_buses: placement.total_buses,
                })
            } else {
                None
            };

            let response = ObservabilityResponse {
                observable: r.observable,
                observable_buses: r.observable_buses.clone(),
                unobservable_buses: r.unobservable_buses.clone(),
                observable_islands: r.observable_islands.clone(),
                jacobian_rank: r.jacobian_rank,
                state_dimension: r.state_dimension,
                missing_measurements: r
                    .missing_measurements
                    .iter()
                    .map(|m| MissingMeasResponse {
                        bus_id: m.bus_id,
                        suggested_measurement: m.suggested_measurement.clone(),
                        reason: m.reason.clone(),
                    })
                    .collect(),
                method: format!("{:?}", method),
                pmu_placement,
            };
            Json(ApiResponse::success(response))
        }
        Err(e) => Json(ApiResponse::error(format!(
            "Observability analysis failed: {}",
            e
        ))),
    }
}

/// POST /api/analysis/bad-data
///
/// 执行不良数据检测。包括 χ² 检测、最大标准残差法（LNR）、
/// 拓扑错误辨识。可选迭代剔除最严重的不良数据后重新估计。
pub async fn bad_data_handler(
    State(state): State<AppState>,
    Json(req): Json<BadDataRequest>,
) -> Json<ApiResponse<BadDataResponse>> {
    let network = match &state.network {
        Some(n) => n,
        None => {
            return Json(ApiResponse::error(
                "Bad data detection requires a loaded network model.".to_string(),
            ))
        }
    };

    let network_model = eneros_analysis::NetworkModel::new(
        network.ybus().clone(),
        network.bus_map().clone(),
        100.0,
    );

    // 获取测量集
    let (measurements, default_slack) = if !req.measurements.is_empty() {
        let meas: Vec<Measurement> = req
            .measurements
            .iter()
            .map(meas_request_to_measurement)
            .collect();
        (meas, req.slack_bus.unwrap_or(1))
    } else {
        match build_synthetic_measurements(network) {
            Ok((m, slack)) => (m, req.slack_bus.unwrap_or(slack)),
            Err(e) => return Json(ApiResponse::error(e)),
        }
    };

    let estimator = StateEstimator::default_estimator();
    let mut detector = BadDataDetector::new(req.threshold, req.significance_level);
    detector.max_elimination_rounds = req.max_elimination_rounds;

    // 执行迭代剔除（若请求）或单次检测
    if req.eliminate {
        match detector.eliminate(&measurements, &estimator, &network_model, default_slack) {
            Ok((cleaned_measurements, report)) => {
                // 用剔除后的测量集重新估计，获取清洗后的状态
                let cleaned_voltages = estimator
                    .estimate_with_network(&cleaned_measurements, &network_model, default_slack)
                    .ok()
                    .map(|r| r.result.bus_voltages);

                let response = build_bad_data_response(&report, cleaned_voltages, true);
                Json(ApiResponse::success(response))
            }
            Err(e) => Json(ApiResponse::error(format!(
                "Bad data elimination failed: {}",
                e
            ))),
        }
    } else {
        // 单次检测：先估计，再检测
        match estimator.estimate_with_network(&measurements, &network_model, default_slack) {
            Ok(se_result) => {
                if !se_result.converged {
                    return Json(ApiResponse::error(
                        "State estimation did not converge; cannot perform bad data detection."
                            .to_string(),
                    ));
                }
                // 构建雅可比和残差
                let state = eneros_analysis::bad_data::build_state_vector(
                    &se_result.result.bus_voltages,
                    &network_model,
                );
                let (jacobian, z_vec, h_x) =
                    estimator.build_jacobian_network(&measurements, &state, &network_model);
                let residuals = &z_vec - &h_x;

                match detector.detect(&measurements, &residuals, &jacobian, state.len()) {
                    Ok(detection) => {
                        let report = detection.result;
                        let response = build_bad_data_response(&report, None, se_result.converged);
                        Json(ApiResponse::success(response))
                    }
                    Err(e) => Json(ApiResponse::error(format!(
                        "Bad data detection failed: {}",
                        e
                    ))),
                }
            }
            Err(e) => Json(ApiResponse::error(format!(
                "State estimation for bad data detection failed: {}",
                e
            ))),
        }
    }
}

/// 构建不良数据检测响应
fn build_bad_data_response(
    report: &eneros_analysis::BadDataReport,
    cleaned_voltages: Option<Vec<(u64, f64, f64)>>,
    converged: bool,
) -> BadDataResponse {
    BadDataResponse {
        has_bad_data: report.has_bad_data,
        chi_square_test: ChiSquareTestResponse {
            objective: report.chi_square_test.objective,
            degrees_of_freedom: report.chi_square_test.degrees_of_freedom,
            significance_level: report.chi_square_test.significance_level,
            critical_value: report.chi_square_test.critical_value,
            rejected: report.chi_square_test.rejected,
        },
        bad_data_items: report
            .bad_data_items
            .iter()
            .map(|item| BadDataItemResponse {
                meas_type: format!("{:?}", item.meas_type),
                element_id: item.element_id,
                to_element_id: item.to_element_id,
                measured_value: item.measured_value,
                estimated_value: item.estimated_value,
                residual: item.residual,
                normalized_residual: item.normalized_residual,
                sensitivity: item.sensitivity,
            })
            .collect(),
        topology_errors: report
            .topology_errors
            .iter()
            .map(|te| TopologyErrorResponse {
                from_bus: te.from_bus,
                to_bus: te.to_bus,
                error_type: te.error_type.clone(),
                confidence: te.confidence,
                evidence_residuals: te.evidence_residuals.clone(),
            })
            .collect(),
        threshold: report.threshold,
        elimination_rounds: report.elimination_rounds,
        cleaned_bus_voltages: cleaned_voltages,
        converged,
    }
}

/// POST /api/analysis/short-circuit/asymmetric
///
/// 执行不对称短路分析（SLG/LL/DLG）。使用完整的序网络 Z-bus 矩阵，
/// 支持自定义正序、负序、零序网络。若未提供序网络，则从已加载网络
/// 的 Y-bus 构建正序 Z-bus，并假设 z2 = z1，z0 使用典型值。
pub async fn asymmetric_short_circuit_handler(
    State(state): State<AppState>,
    Json(req): Json<AsymmetricScRequest>,
) -> Json<ApiResponse<AsymmetricScResponse>> {
    let fault_type = match req.fault_type.to_lowercase().as_str() {
        "slg" | "single_line_ground" => FaultType::SingleLineGround,
        "ll" | "line_line" => FaultType::LineLine,
        "dlg" | "double_line_ground" => FaultType::DoubleLineGround,
        "3ph" | "three_phase" => FaultType::ThreePhase,
        _ => {
            return Json(ApiResponse::error(format!(
                "Unknown fault type: {} (supported: slg, ll, dlg, 3ph)",
                req.fault_type
            )))
        }
    };

    let network = match &state.network {
        Some(n) => n,
        None => {
            return Json(ApiResponse::error(
                "Asymmetric short circuit analysis requires a loaded network model.".to_string(),
            ))
        }
    };

    // 求解潮流获取故障前电压
    let pf_result = match network.solve() {
        Ok(r) => r,
        Err(e) => {
            return Json(ApiResponse::error(format!(
                "Power flow for prefault voltages failed: {}",
                e
            )))
        }
    };

    let prefault_voltages: Vec<Complex64> = pf_result
        .bus_results
        .iter()
        .map(|b| Complex64::from_polar(b.voltage_magnitude, b.voltage_angle))
        .collect();

    // 构建正序 Z-bus
    let z_bus_positive = match build_z_bus(network.ybus()) {
        Some(z) => z,
        None => {
            return Json(ApiResponse::error(
                "Failed to build positive-sequence Z-bus (singular Y-bus)".to_string(),
            ))
        }
    };

    // 构建负序 Z-bus（默认 = 正序）
    let z_bus_negative = if let Some(zn) = &req.z_negative {
        match complex_matrix_from_pairs(zn) {
            Ok(m) => m,
            Err(e) => return Json(ApiResponse::error(e)),
        }
    } else {
        z_bus_positive.clone()
    };

    // 构建零序 Z-bus（默认使用典型零序阻抗）
    let z_bus_zero = if let Some(z0) = &req.z_zero {
        match complex_matrix_from_pairs(z0) {
            Ok(m) => m,
            Err(e) => return Json(ApiResponse::error(e)),
        }
    } else {
        // 构建典型零序网络：在正序 Z-bus 基础上增加零序阻抗
        // 典型零序阻抗约为正序的 3 倍（含接地阻抗）
        let n = z_bus_positive.nrows();
        let mut z0 = Array2::<Complex64>::zeros((n, n));
        for i in 0..n {
            for j in 0..n {
                let z_pos = z_bus_positive[[i, j]];
                // 零序自阻抗 = 正序自阻抗 + 3·Z_ground（典型 0.03 + j0.3）
                if i == j {
                    z0[[i, j]] = z_pos + Complex64::new(0.09, 0.9);
                } else {
                    z0[[i, j]] = z_pos * 0.3; // 零序互阻抗较小
                }
            }
        }
        z0
    };

    let seq_networks = SequenceNetworks {
        z_bus_positive,
        z_bus_negative,
        z_bus_zero,
    };

    let fault = FaultSpec {
        bus_id: req.bus_id,
        fault_type,
        fault_impedance: Complex64::new(req.fault_impedance_real, req.fault_impedance_imag),
    };

    let analyzer = ShortCircuitAnalyzer::new();
    match analyzer.analyze_with_sequence_networks(&fault, &seq_networks, &prefault_voltages) {
        Ok(result) => {
            let fault_current_mag = result.fault_current_ka.norm();
            let response = AsymmetricScResponse {
                fault_current_real: result.fault_current_ka.re,
                fault_current_imag: result.fault_current_ka.im,
                fault_current_magnitude_ka: fault_current_mag,
                bus_voltages: result
                    .bus_voltages
                    .iter()
                    .map(|(id, v)| (*id, v.re, v.im))
                    .collect(),
                branch_currents: result
                    .branch_currents
                    .iter()
                    .map(|(id, i)| (*id, i.re, i.im))
                    .collect(),
                fault_type: format!("{:?}", fault_type),
                method: "sequence_networks".to_string(),
            };
            Json(ApiResponse::success(response))
        }
        Err(e) => Json(ApiResponse::error(format!(
            "Asymmetric short circuit analysis failed: {}",
            e
        ))),
    }
}

/// 从 (real, imag) 对的二维 Vec 构建 Complex64 矩阵
fn complex_matrix_from_pairs(
    pairs: &[Vec<(f64, f64)>],
) -> Result<Array2<Complex64>, String> {
    let n = pairs.len();
    if n == 0 {
        return Err("Empty matrix".to_string());
    }
    let m = pairs[0].len();
    if m == 0 || pairs.iter().any(|row| row.len() != m) {
        return Err("Non-rectangular matrix".to_string());
    }
    let mut matrix = Array2::<Complex64>::zeros((n, m));
    for (i, row) in pairs.iter().enumerate() {
        for (j, (re, im)) in row.iter().enumerate() {
            matrix[[i, j]] = Complex64::new(*re, *im);
        }
    }
    Ok(matrix)
}
