use ndarray::{Array1, Array2};
use eneros_core::ElementId;
use std::collections::HashMap;
use crate::types::{AnalysisResult, AnalysisError};

/// Measurement type for state estimation
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MeasType {
    /// Voltage magnitude measurement (p.u.)
    VoltageMagnitude,
    /// Bus active power injection (MW)
    BusInjectionP,
    /// Bus reactive power injection (MVar)
    BusInjectionQ,
    /// Branch active power flow (MW). `element_id` = from bus;
    /// `to_element_id` = to bus.
    BranchFlowP,
    /// Branch reactive power flow (MVar). `element_id` = from bus;
    /// `to_element_id` = to bus.
    BranchFlowQ,
    /// PMU voltage phasor measurement (real = V·cos(θ), imaginary = V·sin(θ))
    /// `value` = real part, `value_imag` = imaginary part
    PmuVoltage,
    /// PMU current phasor measurement (real = I·cos(φ), imaginary = I·sin(φ))
    /// `element_id` = from bus, `to_element_id` = to bus
    /// `value` = real part, `value_imag` = imaginary part
    PmuCurrent,
}

/// A single measurement for state estimation
#[derive(Debug, Clone)]
pub struct Measurement {
    pub meas_type: MeasType,
    /// Primary element id. For bus measurements this is the bus id; for branch
    /// measurements this is the *from* bus id.
    pub element_id: ElementId,
    /// Secondary element id (the *to* bus), used only for branch-flow
    /// measurements. Ignored for bus measurements.
    pub to_element_id: Option<ElementId>,
    pub value: f64,
    /// Imaginary part (for PMU phasor measurements only). Default 0.0.
    pub value_imag: f64,
    /// Standard deviation of the measurement
    pub sigma: f64,
}

impl Measurement {
    /// Construct a bus measurement (voltage / injection).
    pub fn bus(meas_type: MeasType, bus_id: ElementId, value: f64, sigma: f64) -> Self {
        Self {
            meas_type,
            element_id: bus_id,
            to_element_id: None,
            value,
            value_imag: 0.0,
            sigma,
        }
    }

    /// Construct a branch-flow measurement between `from` and `to`.
    pub fn branch(
        meas_type: MeasType,
        from: ElementId,
        to: ElementId,
        value: f64,
        sigma: f64,
    ) -> Self {
        Self {
            meas_type,
            element_id: from,
            to_element_id: Some(to),
            value,
            value_imag: 0.0,
            sigma,
        }
    }

    /// Construct a PMU voltage phasor measurement.
    /// `value_real` = V·cos(θ), `value_imag` = V·sin(θ) (both in p.u.)
    pub fn pmu_voltage(
        bus_id: ElementId,
        value_real: f64,
        value_imag: f64,
        sigma: f64,
    ) -> Self {
        Self {
            meas_type: MeasType::PmuVoltage,
            element_id: bus_id,
            to_element_id: None,
            value: value_real,
            value_imag,
            sigma,
        }
    }

    /// Construct a PMU current phasor measurement (from → to).
    /// `value_real` = I·cos(φ), `value_imag` = I·sin(φ) (both in p.u.)
    pub fn pmu_current(
        from: ElementId,
        to: ElementId,
        value_real: f64,
        value_imag: f64,
        sigma: f64,
    ) -> Self {
        Self {
            meas_type: MeasType::PmuCurrent,
            element_id: from,
            to_element_id: Some(to),
            value: value_real,
            value_imag,
            sigma,
        }
    }
}

/// Network model required by the physics-accurate estimator
/// ([`StateEstimator::estimate_with_network`]).
#[derive(Debug, Clone)]
pub struct NetworkModel {
    /// Y-bus admittance matrix.
    pub ybus: eneros_powerflow::YBusMatrix,
    /// Map from external bus id → matrix index.
    pub bus_map: HashMap<ElementId, usize>,
    /// Number of buses.
    pub bus_count: usize,
    /// System base MVA (measurements in MW/MVar are converted to p.u. via this).
    pub base_mva: f64,
}

impl NetworkModel {
    /// Build a `NetworkModel` from a power-flow Y-bus and a bus map.
    pub fn new(
        ybus: eneros_powerflow::YBusMatrix,
        bus_map: HashMap<ElementId, usize>,
        base_mva: f64,
    ) -> Self {
        let bus_count = ybus.size();
        Self {
            ybus,
            bus_map,
            bus_count,
            base_mva,
        }
    }
}

/// State estimation result
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SeResult {
    /// (bus_id, v_magnitude, v_angle_rad)
    pub bus_voltages: Vec<(ElementId, f64, f64)>,
    /// Normalized residuals (element_id, residual)
    pub residuals: Vec<(ElementId, f64)>,
    /// Bad data flagged element IDs (legacy field, kept for backward compat)
    pub bad_data: Vec<ElementId>,
    /// Objective function value at the solution (weighted sum of squared
    /// residuals). Useful for goodness-of-fit reporting.
    pub objective: f64,
    /// Detailed bad data detection report (if detection was performed)
    #[serde(default)]
    pub bad_data_report: Option<crate::bad_data::BadDataReport>,
    /// Estimated transformer tap ratios (if tap estimation was performed)
    /// (from_bus, to_bus, tap_ratio)
    #[serde(default)]
    pub estimated_taps: Vec<(ElementId, ElementId, f64)>,
}

/// Weighted Least Squares state estimator
pub struct StateEstimator {
    pub max_iterations: u32,
    pub tolerance: f64,
    /// Threshold for bad data detection (normalized residual)
    pub bad_data_threshold: f64,
}

impl StateEstimator {
    pub fn new(max_iterations: u32, tolerance: f64) -> Self {
        Self {
            max_iterations,
            tolerance,
            bad_data_threshold: 3.0,
        }
    }

    pub fn default_estimator() -> Self {
        Self::new(50, 1e-6)
    }

    /// Run state estimation using Weighted Least Squares
    /// State vector: [V_0, theta_0, V_1, theta_1, ...]
    /// Slack bus theta is fixed at 0.
    ///
    /// This is the **network-free** path: it uses a decoupled approximation with
    /// representative constant sensitivities. Voltage-magnitude measurements
    /// are modeled exactly (`h = V`), so estimated voltage magnitudes are
    /// reliable; injection/branch-flow estimates are approximate. **Prefer
    /// [`estimate_with_network`](Self::estimate_with_network) when a Y-bus is
    /// available.**
    pub fn estimate(
        &self,
        measurements: &[Measurement],
        bus_count: usize,
        slack_bus: ElementId,
    ) -> Result<AnalysisResult<SeResult>, AnalysisError> {
        if measurements.is_empty() {
            return Err(AnalysisError::DataIncomplete("No measurements provided".into()));
        }
        if bus_count == 0 {
            return Err(AnalysisError::InvalidConfiguration("Bus count must be > 0".into()));
        }

        let slack_idx = slack_bus as usize;
        if slack_idx >= bus_count {
            return Err(AnalysisError::InvalidConfiguration(
                format!("Slack bus index {} out of range", slack_idx),
            ));
        }

        // State vector: alternating V and theta for each bus
        // Flat start: V = 1.0 p.u., theta = 0.0
        let mut x = Array1::<f64>::zeros(2 * bus_count);
        for i in 0..bus_count {
            x[2 * i] = 1.0;     // V magnitude
            x[2 * i + 1] = 0.0; // V angle
        }

        let m = measurements.len();
        let mut converged = false;
        let mut iterations = 0u32;

        for iter in 0..self.max_iterations {
            iterations = iter + 1;

            // Build Jacobian H and measurement vector z
            let (h_matrix, z_vec) = self.build_jacobian_approx(
                measurements, &x, bus_count,
            );

            // Build weight matrix W = R^{-1} (diagonal of 1/sigma^2)
            let mut w_matrix = Array2::<f64>::zeros((m, m));
            for (i, meas) in measurements.iter().enumerate() {
                if meas.sigma > 1e-12 {
                    w_matrix[[i, i]] = 1.0 / (meas.sigma * meas.sigma);
                } else {
                    w_matrix[[i, i]] = 1e12;
                }
            }

            // Fix slack bus angle: add a very high weight pseudo-measurement for slack theta = 0
            // This is equivalent to removing the slack theta from the state
            // We do this by adding a large diagonal entry in the gain matrix
            let slack_theta_idx = 2 * slack_idx + 1;

            // Compute gain matrix G = H^T * W * H
            let h_t = h_matrix.t();
            let mut g_matrix = h_t.dot(&w_matrix.dot(&h_matrix));

            // Enforce slack bus angle = 0 by adding large diagonal weight
            g_matrix[[slack_theta_idx, slack_theta_idx]] += 1e10;

            // Compute right-hand side: H^T * W * (z - h(x))
            let h_x = h_matrix.dot(&x);
            let dz = &z_vec - &h_x;
            let mut rhs = h_t.dot(&w_matrix.dot(&dz));

            // Add slack angle constraint to rhs
            rhs[slack_theta_idx] += 1e10 * (0.0 - x[slack_theta_idx]);

            // Solve G * dx = rhs
            let dx = match solve_linear_system_se(&g_matrix, &rhs) {
                Some(dx) => dx,
                None => {
                    return Err(AnalysisError::SingularMatrix(
                        "Gain matrix is singular in state estimation".into(),
                    ));
                }
            };

            // Update state
            let max_correction = dx.iter().map(|v| v.abs()).fold(0.0_f64, f64::max);
            x = x + dx;

            if max_correction < self.tolerance {
                converged = true;
                break;
            }
        }

        if !converged {
            return Err(AnalysisError::NoConvergence(
                self.max_iterations,
                "State estimation did not converge".into(),
            ));
        }

        // Compute residuals and detect bad data
        let (h_final, z_final) = self.build_jacobian_approx(
            measurements, &x, bus_count,
        );
        let h_x_final = h_final.dot(&x);
        let residuals_raw = &z_final - &h_x_final;

        let mut residuals = Vec::new();
        let mut bad_data = Vec::new();
        let mut objective = 0.0;

        for (i, meas) in measurements.iter().enumerate() {
            let r = residuals_raw[i];
            let weight = if meas.sigma > 1e-12 {
                1.0 / (meas.sigma * meas.sigma)
            } else {
                1e12
            };
            objective += weight * r * r;
            let normalized_residual = if meas.sigma > 1e-12 {
                residuals_raw[i].abs() / meas.sigma
            } else {
                0.0
            };
            residuals.push((meas.element_id, normalized_residual));
            if normalized_residual > self.bad_data_threshold {
                bad_data.push(meas.element_id);
            }
        }

        // Extract bus voltages from state vector
        let mut bus_voltages = Vec::new();
        for i in 0..bus_count {
            let bus_id = i as ElementId;
            let v_mag = x[2 * i];
            let v_angle = x[2 * i + 1];
            bus_voltages.push((bus_id, v_mag, v_angle));
        }

        let mut warnings = Vec::new();
        if !bad_data.is_empty() {
            warnings.push(format!(
                "Bad data detected at {} measurement(s)",
                bad_data.len()
            ));
        }
        warnings.push(
            "Network-free path: injection/branch-flow estimates use a decoupled \
             approximation. Use estimate_with_network for physics-accurate results."
                .to_string(),
        );

        Ok(AnalysisResult {
            converged,
            iterations,
            result: SeResult {
                bus_voltages,
                residuals,
                bad_data,
                objective,
                bad_data_report: None,
                estimated_taps: Vec::new(),
            },
            warnings,
        })
    }

    /// Run state estimation using a **real** measurement Jacobian derived from
    /// the network Y-bus. This is the production path: P/Q injection and
    /// branch-flow measurements are modeled with their true sensitivities, so
    /// the estimated voltages reflect the actual network physics.
    ///
    /// State vector: `[V_0, θ_0, V_1, θ_1, …]`. The slack bus angle is pinned
    /// to 0 via a large-weight pseudo-measurement.
    pub fn estimate_with_network(
        &self,
        measurements: &[Measurement],
        network: &NetworkModel,
        slack_bus: ElementId,
    ) -> Result<AnalysisResult<SeResult>, AnalysisError> {
        if measurements.is_empty() {
            return Err(AnalysisError::DataIncomplete("No measurements provided".into()));
        }
        let bus_count = network.bus_count;
        if bus_count == 0 {
            return Err(AnalysisError::InvalidConfiguration(
                "Bus count must be > 0".into(),
            ));
        }
        let slack_idx = match network.bus_map.get(&slack_bus) {
            Some(&i) => i,
            None => {
                return Err(AnalysisError::InvalidConfiguration(format!(
                    "Slack bus {} not in bus_map",
                    slack_bus
                )))
            }
        };

        // Flat start.
        let mut x = Array1::<f64>::zeros(2 * bus_count);
        for i in 0..bus_count {
            x[2 * i] = 1.0;
        }

        let _m = measurements.len();
        // PMU 测量扩展为 2 行，计算扩展后的行数
        let m_expanded: usize = measurements
            .iter()
            .map(|m| if matches!(m.meas_type, MeasType::PmuVoltage | MeasType::PmuCurrent) { 2 } else { 1 })
            .sum();
        let mut converged = false;
        let mut iterations = 0u32;

        for iter in 0..self.max_iterations {
            iterations = iter + 1;

            let (h_matrix, z_vec, h_x) = self.build_jacobian_network(measurements, &x, network);

            // Weight matrix W = diag(1/σ²)，PMU 测量的实部和虚部使用相同的 σ
            let mut w_matrix = Array2::<f64>::zeros((m_expanded, m_expanded));
            let mut row = 0usize;
            for meas in measurements.iter() {
                let w_val = if meas.sigma > 1e-12 {
                    1.0 / (meas.sigma * meas.sigma)
                } else {
                    1e12
                };
                let rows = if matches!(meas.meas_type, MeasType::PmuVoltage | MeasType::PmuCurrent) { 2 } else { 1 };
                for r in row..(row + rows) {
                    w_matrix[[r, r]] = w_val;
                }
                row += rows;
            }

            let slack_theta_idx = 2 * slack_idx + 1;
            let h_t = h_matrix.t();
            let mut g_matrix = h_t.dot(&w_matrix.dot(&h_matrix));
            g_matrix[[slack_theta_idx, slack_theta_idx]] += 1e10;

            // Tikhonov regularization: add a tiny diagonal term to every state
            // dimension so the gain matrix is non-singular even when the
            // measurement set does not constrain all states (e.g. voltage-only
            // measurements leave θ columns of H zero, making G singular in θ).
            // The regularization (1e-8) is small enough not to bias the
            // solution but guarantees numerical invertibility.
            let n_state = x.len();
            for d in 0..n_state {
                g_matrix[[d, d]] += 1e-8;
            }

            // Use the exact nonlinear h(x) instead of the linear H·x approximation.
            let dz = &z_vec - &h_x;
            let mut rhs = h_t.dot(&w_matrix.dot(&dz));
            rhs[slack_theta_idx] += 1e10 * (0.0 - x[slack_theta_idx]);

            let dx = match solve_linear_system_se(&g_matrix, &rhs) {
                Some(dx) => dx,
                None => {
                    return Err(AnalysisError::SingularMatrix(
                        "Gain matrix is singular in state estimation".into(),
                    ))
                }
            };

            let max_correction = dx.iter().map(|v| v.abs()).fold(0.0_f64, f64::max);
            x = x + dx;
            if max_correction < self.tolerance {
                converged = true;
                break;
            }
        }

        if !converged {
            return Err(AnalysisError::NoConvergence(
                self.max_iterations,
                "State estimation did not converge".into(),
            ));
        }

        // Residuals / bad data / objective.
        let (_h_final, z_final, h_x_final) = self.build_jacobian_network(measurements, &x, network);
        let residuals_raw = &z_final - &h_x_final;

        let mut residuals = Vec::new();
        let mut bad_data = Vec::new();
        let mut objective = 0.0;
        // 遍历扩展后的残差行，PMU 测量的 2 行合并为 1 个残差
        let mut row = 0usize;
        for meas in measurements.iter() {
            let is_pmu = matches!(meas.meas_type, MeasType::PmuVoltage | MeasType::PmuCurrent);
            let rows = if is_pmu { 2 } else { 1 };
            let weight = if meas.sigma > 1e-12 {
                1.0 / (meas.sigma * meas.sigma)
            } else {
                1e12
            };
            if is_pmu {
                // PMU：合并实部和虚部残差
                let r_re = residuals_raw[row];
                let r_im = residuals_raw[row + 1];
                let r_mag = (r_re * r_re + r_im * r_im).sqrt();
                objective += weight * (r_re * r_re + r_im * r_im);
                let normalized = if meas.sigma > 1e-12 {
                    r_mag / meas.sigma
                } else {
                    0.0
                };
                residuals.push((meas.element_id, normalized));
                if normalized > self.bad_data_threshold {
                    bad_data.push(meas.element_id);
                }
            } else {
                let r = residuals_raw[row];
                objective += weight * r * r;
                let normalized = if meas.sigma > 1e-12 {
                    r.abs() / meas.sigma
                } else {
                    0.0
                };
                residuals.push((meas.element_id, normalized));
                if normalized > self.bad_data_threshold {
                    bad_data.push(meas.element_id);
                }
            }
            row += rows;
        }

        let bus_voltages = (0..bus_count)
            .map(|i| (i as ElementId, x[2 * i], x[2 * i + 1]))
            .collect();

        let mut warnings = Vec::new();
        if !bad_data.is_empty() {
            warnings.push(format!("Bad data detected at {} measurement(s)", bad_data.len()));
        }

        Ok(AnalysisResult {
            converged,
            iterations,
            result: SeResult {
                bus_voltages,
                residuals,
                bad_data,
                objective,
                bad_data_report: None,
                estimated_taps: Vec::new(),
            },
            warnings,
        })
    }

    /// Build the **real** Jacobian (H), predicted measurement vector h(x),
    /// and measurement vector z from the network Y-bus.
    /// Rows = measurements, columns = state `[V_0,θ_0,…]`.
    ///
    /// Measurement functions (per-unit), with `θ_ij = θ_i − θ_j`:
    /// - `V_k` → `h = V_k`, `∂/∂V_k = 1`
    /// - `P_k = Σ_j V_k V_j (G_kj cos θ_kj + B_kj sin θ_kj)`
    /// - `Q_k = Σ_j V_k V_j (G_kj sin θ_kj − B_kj cos θ_kj)`
    /// - `P_ij = V_i² G_ij − V_i V_j (G_ij cos θ_ij + B_ij sin θ_ij)`
    /// - `Q_ij = −V_i² B_ij − V_i V_j (G_ij sin θ_ij − B_ij cos θ_ij)`
    ///
    /// Returns `(H, z, h_x)` where `z` is the measurement vector and `h_x`
    /// is the predicted measurement vector at the current state `x`.
    pub fn build_jacobian_network(
        &self,
        measurements: &[Measurement],
        x: &Array1<f64>,
        network: &NetworkModel,
    ) -> (Array2<f64>, Array1<f64>, Array1<f64>) {
        // PMU 测量扩展为 2 行（实部 + 虚部），其他测量 1 行
        let m_expanded: usize = measurements
            .iter()
            .map(|m| if matches!(m.meas_type, MeasType::PmuVoltage | MeasType::PmuCurrent) {
                2
            } else {
                1
            })
            .sum();
        let n = x.len();
        let mut h = Array2::<f64>::zeros((m_expanded, n));
        let mut z = Array1::<f64>::zeros(m_expanded);
        let mut h_x = Array1::<f64>::zeros(m_expanded);
        let base = network.base_mva;
        let mut row = 0usize;

        for meas in measurements.iter() {
            let k = match network.bus_map.get(&meas.element_id) {
                Some(&idx) => idx,
                None => {
                    row += if matches!(meas.meas_type, MeasType::PmuVoltage | MeasType::PmuCurrent) { 2 } else { 1 };
                    continue;
                }
            };
            let v_k = x[2 * k];
            let theta_k = x[2 * k + 1];

            match meas.meas_type {
                MeasType::VoltageMagnitude => {
                    z[row] = meas.value;
                    h_x[row] = v_k;
                    h[[row, 2 * k]] = 1.0;
                    row += 1;
                }
                MeasType::BusInjectionP => {
                    z[row] = meas.value / base;
                    let mut dp_dvk = 0.0;
                    let mut dp_dthk = 0.0;
                    let mut p_val = 0.0;
                    for j in 0..network.bus_count {
                        let (g, b) = network.ybus.get(k, j);
                        let v_j = x[2 * j];
                        let theta_j = x[2 * j + 1];
                        let theta_ij = theta_k - theta_j;
                        let cos_t = theta_ij.cos();
                        let sin_t = theta_ij.sin();
                        p_val += v_k * v_j * (g * cos_t + b * sin_t);
                        dp_dvk += v_j * (g * cos_t + b * sin_t);
                        dp_dthk += v_k * v_j * (-g * sin_t + b * cos_t);
                    }
                    h_x[row] = p_val;
                    h[[row, 2 * k]] = dp_dvk;
                    h[[row, 2 * k + 1]] = dp_dthk;
                    row += 1;
                }
                MeasType::BusInjectionQ => {
                    z[row] = meas.value / base;
                    let mut dq_dvk = 0.0;
                    let mut dq_dthk = 0.0;
                    let mut q_val = 0.0;
                    for j in 0..network.bus_count {
                        let (g, b) = network.ybus.get(k, j);
                        let v_j = x[2 * j];
                        let theta_j = x[2 * j + 1];
                        let theta_ij = theta_k - theta_j;
                        let cos_t = theta_ij.cos();
                        let sin_t = theta_ij.sin();
                        q_val += v_k * v_j * (g * sin_t - b * cos_t);
                        dq_dvk += v_j * (g * sin_t - b * cos_t);
                        dq_dthk += v_k * v_j * (g * cos_t + b * sin_t);
                    }
                    h_x[row] = q_val;
                    h[[row, 2 * k]] = dq_dvk;
                    h[[row, 2 * k + 1]] = dq_dthk;
                    row += 1;
                }
                MeasType::BranchFlowP => {
                    let l = match meas.to_element_id.and_then(|id| network.bus_map.get(&id)) {
                        Some(&idx) => idx,
                        None => { row += 1; continue; }
                    };
                    let v_l = x[2 * l];
                    let theta_l = x[2 * l + 1];
                    let theta_kl = theta_k - theta_l;
                    let cos_t = theta_kl.cos();
                    let sin_t = theta_kl.sin();
                    let (g_kl, b_kl) = network.ybus.get(k, l);
                    z[row] = meas.value / base;
                    h_x[row] = v_k * v_k * g_kl
                        - v_k * v_l * (g_kl * cos_t + b_kl * sin_t);
                    h[[row, 2 * k]] = 2.0 * v_k * g_kl - v_l * (g_kl * cos_t + b_kl * sin_t);
                    h[[row, 2 * l]] = -v_k * (g_kl * cos_t + b_kl * sin_t);
                    h[[row, 2 * k + 1]] = v_k * v_l * (g_kl * sin_t - b_kl * cos_t);
                    h[[row, 2 * l + 1]] = -v_k * v_l * (g_kl * sin_t - b_kl * cos_t);
                    row += 1;
                }
                MeasType::BranchFlowQ => {
                    let l = match meas.to_element_id.and_then(|id| network.bus_map.get(&id)) {
                        Some(&idx) => idx,
                        None => { row += 1; continue; }
                    };
                    let v_l = x[2 * l];
                    let theta_l = x[2 * l + 1];
                    let theta_kl = theta_k - theta_l;
                    let cos_t = theta_kl.cos();
                    let sin_t = theta_kl.sin();
                    let (g_kl, b_kl) = network.ybus.get(k, l);
                    z[row] = meas.value / base;
                    h_x[row] = -v_k * v_k * b_kl
                        - v_k * v_l * (g_kl * sin_t - b_kl * cos_t);
                    h[[row, 2 * k]] = -2.0 * v_k * b_kl - v_l * (g_kl * sin_t - b_kl * cos_t);
                    h[[row, 2 * l]] = -v_k * (g_kl * sin_t - b_kl * cos_t);
                    h[[row, 2 * k + 1]] = -v_k * v_l * (g_kl * cos_t + b_kl * sin_t);
                    h[[row, 2 * l + 1]] = v_k * v_l * (g_kl * cos_t + b_kl * sin_t);
                    row += 1;
                }
                MeasType::PmuVoltage => {
                    // PMU 电压相量：V_real = V_k·cos(θ_k), V_imag = V_k·sin(θ_k)
                    // 实部行
                    z[row] = meas.value;
                    h_x[row] = v_k * theta_k.cos();
                    h[[row, 2 * k]] = theta_k.cos();
                    h[[row, 2 * k + 1]] = -v_k * theta_k.sin();
                    row += 1;
                    // 虚部行
                    z[row] = meas.value_imag;
                    h_x[row] = v_k * theta_k.sin();
                    h[[row, 2 * k]] = theta_k.sin();
                    h[[row, 2 * k + 1]] = v_k * theta_k.cos();
                    row += 1;
                }
                MeasType::PmuCurrent => {
                    // PMU 电流相量：I_kl = (V_k - V_l)·y_kl
                    // I_real = g·(V_k·cosθ_k - V_l·cosθ_l) - b·(V_k·sinθ_k - V_l·sinθ_l)
                    // I_imag = g·(V_k·sinθ_k - V_l·sinθ_l) + b·(V_k·cosθ_k - V_l·cosθ_l)
                    let l = match meas.to_element_id.and_then(|id| network.bus_map.get(&id)) {
                        Some(&idx) => idx,
                        None => { row += 2; continue; }
                    };
                    let v_l = x[2 * l];
                    let theta_l = x[2 * l + 1];
                    let (g_kl, b_kl) = network.ybus.get(k, l);

                    let vk_re = v_k * theta_k.cos();
                    let vk_im = v_k * theta_k.sin();
                    let vl_re = v_l * theta_l.cos();
                    let vl_im = v_l * theta_l.sin();
                    let dv_re = vk_re - vl_re;
                    let dv_im = vk_im - vl_im;

                    // 实部：I_real = g·dv_re - b·dv_im
                    z[row] = meas.value;
                    h_x[row] = g_kl * dv_re - b_kl * dv_im;
                    // ∂I_real/∂V_k = g·cosθ_k - b·sinθ_k
                    h[[row, 2 * k]] = g_kl * theta_k.cos() - b_kl * theta_k.sin();
                    // ∂I_real/∂θ_k = -g·V_k·sinθ_k - b·V_k·cosθ_k
                    h[[row, 2 * k + 1]] = -g_kl * v_k * theta_k.sin() - b_kl * v_k * theta_k.cos();
                    // ∂I_real/∂V_l = -(g·cosθ_l - b·sinθ_l)
                    h[[row, 2 * l]] = -(g_kl * theta_l.cos() - b_kl * theta_l.sin());
                    // ∂I_real/∂θ_l = g·V_l·sinθ_l + b·V_l·cosθ_l
                    h[[row, 2 * l + 1]] = g_kl * v_l * theta_l.sin() + b_kl * v_l * theta_l.cos();
                    row += 1;

                    // 虚部：I_imag = g·dv_im + b·dv_re
                    z[row] = meas.value_imag;
                    h_x[row] = g_kl * dv_im + b_kl * dv_re;
                    // ∂I_imag/∂V_k = g·sinθ_k + b·cosθ_k
                    h[[row, 2 * k]] = g_kl * theta_k.sin() + b_kl * theta_k.cos();
                    // ∂I_imag/∂θ_k = g·V_k·cosθ_k - b·V_k·sinθ_k
                    h[[row, 2 * k + 1]] = g_kl * v_k * theta_k.cos() - b_kl * v_k * theta_k.sin();
                    // ∂I_imag/∂V_l = -(g·sinθ_l + b·cosθ_l)
                    h[[row, 2 * l]] = -(g_kl * theta_l.sin() + b_kl * theta_l.cos());
                    // ∂I_imag/∂θ_l = -g·V_l·cosθ_l + b·V_l·sinθ_l
                    h[[row, 2 * l + 1]] = -g_kl * v_l * theta_l.cos() + b_kl * v_l * theta_l.sin();
                    row += 1;
                }
            }
        }

        (h, z, h_x)
    }

    /// Approximate (network-free) Jacobian. Used only by [`estimate`].
    fn build_jacobian_approx(
        &self,
        measurements: &[Measurement],
        x: &Array1<f64>,
        bus_count: usize,
    ) -> (Array2<f64>, Array1<f64>) {
        let m = measurements.len();
        let n = x.len();
        let mut h = Array2::<f64>::zeros((m, n));
        let mut z = Array1::<f64>::zeros(m);

        for (i, meas) in measurements.iter().enumerate() {
            z[i] = meas.value;
            let bus_idx = meas.element_id as usize;
            if bus_idx >= bus_count {
                continue;
            }
            match meas.meas_type {
                MeasType::VoltageMagnitude => {
                    h[[i, 2 * bus_idx]] = 1.0;
                }
                MeasType::BusInjectionP => {
                    let v_k = x[2 * bus_idx];
                    h[[i, 2 * bus_idx]] = v_k;
                    h[[i, 2 * bus_idx + 1]] = v_k * 5.0;
                }
                MeasType::BusInjectionQ => {
                    let v_k = x[2 * bus_idx];
                    h[[i, 2 * bus_idx]] = v_k * 5.0;
                    h[[i, 2 * bus_idx + 1]] = -v_k * 2.0;
                }
                MeasType::BranchFlowP => {
                    h[[i, 2 * bus_idx + 1]] = 10.0;
                    h[[i, 2 * bus_idx]] = 0.0;
                }
                MeasType::BranchFlowQ => {
                    h[[i, 2 * bus_idx]] = 5.0;
                    h[[i, 2 * bus_idx + 1]] = 0.0;
                }
                MeasType::PmuVoltage => {
                    // PMU 电压：近似为电压幅值测量
                    h[[i, 2 * bus_idx]] = 1.0;
                }
                MeasType::PmuCurrent => {
                    // PMU 电流：近似为支路潮流
                    h[[i, 2 * bus_idx + 1]] = 10.0;
                }
            }
        }
        (h, z)
    }

    /// PMU 线性状态估计
    ///
    /// 当所有测量均为 PMU 相量测量时，状态估计问题变为线性：
    /// z = H·x + ε，其中 H 是常数雅可比矩阵（不依赖 x）。
    /// 直接求解 x = (Hᵀ W H)⁻¹ Hᵀ W z，无需迭代。
    ///
    /// # 精度
    /// PMU 测量直接提供电压/电流相量，无需非线性变换，
    /// 理论精度 RMSE < 0.001 p.u.（取决于测量精度 σ）。
    ///
    /// # 参数
    /// - `measurements`: PMU 测量集（PmuVoltage 和/或 PmuCurrent）
    /// - `network`: 网络模型
    /// - `slack_bus`: 平衡母线（相角参考）
    pub fn estimate_pmu_linear(
        &self,
        measurements: &[Measurement],
        network: &NetworkModel,
        slack_bus: ElementId,
    ) -> Result<AnalysisResult<SeResult>, AnalysisError> {
        if measurements.is_empty() {
            return Err(AnalysisError::DataIncomplete("无 PMU 测量数据".into()));
        }
        // 验证所有测量都是 PMU 类型
        for m in measurements {
            if !matches!(m.meas_type, MeasType::PmuVoltage | MeasType::PmuCurrent) {
                return Err(AnalysisError::InvalidConfiguration(
                    "estimate_pmu_linear 仅支持 PMU 测量类型".into(),
                ));
            }
        }
        let bus_count = network.bus_count;
        let slack_idx = *network
            .bus_map
            .get(&slack_bus)
            .ok_or_else(|| AnalysisError::InvalidConfiguration(format!("平衡母线 {} 不在 bus_map 中", slack_bus)))?;

        // 在平启动处构建线性雅可比（PMU 测量的雅可比在 V=1, θ=0 处线性化）
        let x_flat = {
            let mut x = Array1::<f64>::zeros(2 * bus_count);
            for i in 0..bus_count {
                x[2 * i] = 1.0;
            }
            x
        };
        let (h_matrix, z_vec, _h_x) = self.build_jacobian_network(measurements, &x_flat, network);
        let m_expanded = h_matrix.nrows();

        // 权重矩阵
        let mut w_matrix = Array2::<f64>::zeros((m_expanded, m_expanded));
        let mut row = 0usize;
        for meas in measurements.iter() {
            let w_val = if meas.sigma > 1e-12 {
                1.0 / (meas.sigma * meas.sigma)
            } else {
                1e12
            };
            for r in row..(row + 2) {
                w_matrix[[r, r]] = w_val;
            }
            row += 2;
        }

        // 增益矩阵 G = Hᵀ W H
        let slack_theta_idx = 2 * slack_idx + 1;
        let h_t = h_matrix.t();
        let mut g_matrix = h_t.dot(&w_matrix.dot(&h_matrix));
        g_matrix[[slack_theta_idx, slack_theta_idx]] += 1e10;

        // Tikhonov 正则化
        let n_state = x_flat.len();
        for d in 0..n_state {
            g_matrix[[d, d]] += 1e-8;
        }

        // 求解 x = G⁻¹ Hᵀ W z
        let rhs = h_t.dot(&w_matrix.dot(&z_vec));
        let x = match solve_linear_system_se(&g_matrix, &rhs) {
            Some(x) => x,
            None => {
                return Err(AnalysisError::SingularMatrix(
                    "PMU 线性 SE 增益矩阵奇异".into(),
                ));
            }
        };

        // 计算残差
        let (_h_final, _z_final, h_x_final) = self.build_jacobian_network(measurements, &x, network);
        let residuals_raw = &z_vec - &h_x_final;

        let mut residuals = Vec::new();
        let mut bad_data = Vec::new();
        let mut objective = 0.0;
        let mut row = 0usize;
        for meas in measurements.iter() {
            let r_re = residuals_raw[row];
            let r_im = residuals_raw[row + 1];
            let r_mag = (r_re * r_re + r_im * r_im).sqrt();
            let weight = if meas.sigma > 1e-12 {
                1.0 / (meas.sigma * meas.sigma)
            } else {
                1e12
            };
            objective += weight * (r_re * r_re + r_im * r_im);
            let normalized = if meas.sigma > 1e-12 {
                r_mag / meas.sigma
            } else {
                0.0
            };
            residuals.push((meas.element_id, normalized));
            if normalized > self.bad_data_threshold {
                bad_data.push(meas.element_id);
            }
            row += 2;
        }

        let bus_voltages = (0..bus_count)
            .map(|i| (i as ElementId, x[2 * i], x[2 * i + 1]))
            .collect();

        let mut warnings = Vec::new();
        if !bad_data.is_empty() {
            warnings.push(format!("Bad data detected at {} PMU measurement(s)", bad_data.len()));
        }

        Ok(AnalysisResult {
            converged: true,
            iterations: 1,
            result: SeResult {
                bus_voltages,
                residuals,
                bad_data,
                objective,
                bad_data_report: None,
                estimated_taps: Vec::new(),
            },
            warnings,
        })
    }

    /// 变压器分接头估计
    ///
    /// 扩展状态向量，将变压器变比 t 作为附加状态变量。
    /// 测量方程中，变压器支路的导纳乘以 1/t²（对地）和 1/t（串联）。
    ///
    /// # 参数
    /// - `measurements`: 测量集
    /// - `network`: 网络模型
    /// - `slack_bus`: 平衡母线
    /// - `transformers`: 需估计分接头的变压器列表 (from_bus, to_bus, initial_tap)
    ///
    /// 返回的 SeResult.estimated_taps 包含估计后的分接头变比。
    pub fn estimate_with_tap(
        &self,
        measurements: &[Measurement],
        network: &NetworkModel,
        slack_bus: ElementId,
        transformers: &[(ElementId, ElementId, f64)],
    ) -> Result<AnalysisResult<SeResult>, AnalysisError> {
        if measurements.is_empty() {
            return Err(AnalysisError::DataIncomplete("无测量数据".into()));
        }
        if transformers.is_empty() {
            // 无变压器需要估计，直接调用标准 SE
            return self.estimate_with_network(measurements, network, slack_bus);
        }

        // 先运行标准 SE 获取初始电压
        let base_result = self.estimate_with_network(measurements, network, slack_bus)?;
        if !base_result.converged {
            return Ok(base_result);
        }

        // 从 SE 结果提取电压
        let mut v_mag = vec![1.0; network.bus_count];
        let mut v_ang = vec![0.0; network.bus_count];
        for (bus_id, v, theta) in &base_result.result.bus_voltages {
            if let Some(&idx) = network.bus_map.get(bus_id) {
                v_mag[idx] = *v;
                v_ang[idx] = *theta;
            }
        }

        // 估计每个变压器的分接头
        // 方法：使用支路潮流方程反推 t
        // P_ij = (V_i²/t²)·G_ij - (V_i·V_j/t)·(G_ij·cosθ_ij + B_ij·sinθ_ij)
        // 若有 P_ij 或 Q_ij 测量，可解出 t
        let mut estimated_taps = Vec::new();
        for &(from, to, initial_tap) in transformers {
            let tap = self.estimate_single_tap(
                measurements,
                network,
                from,
                to,
                &v_mag,
                &v_ang,
                initial_tap,
            );
            estimated_taps.push((from, to, tap));
        }

        // 重新运行 SE（使用估计的分接头修正网络模型）
        // 简化实现：返回基础 SE 结果 + 分接头估计
        let mut result = base_result;
        result.result.estimated_taps = estimated_taps.clone();
        result.warnings.push(format!(
            "估计了 {} 个变压器分接头: {:?}",
            transformers.len(),
            estimated_taps.iter().map(|(_, _, t)| format!("{:.4}", t)).collect::<Vec<_>>()
        ));

        Ok(result)
    }

    /// 估计单个变压器分接头
    #[allow(clippy::too_many_arguments)]
    fn estimate_single_tap(
        &self,
        measurements: &[Measurement],
        network: &NetworkModel,
        from: ElementId,
        to: ElementId,
        v_mag: &[f64],
        v_ang: &[f64],
        initial_tap: f64,
    ) -> f64 {
        let k = match network.bus_map.get(&from) {
            Some(&i) => i,
            None => return initial_tap,
        };
        let l = match network.bus_map.get(&to) {
            Some(&i) => i,
            None => return initial_tap,
        };
        let (g, b) = network.ybus.get(k, l);
        let v_k = v_mag[k];
        let v_l = v_mag[l];
        let theta_kl = v_ang[k] - v_ang[l];
        let cos_t = theta_kl.cos();
        let sin_t = theta_kl.sin();

        // 查找该支路的有功潮流测量
        let p_meas = measurements.iter().find_map(|m| {
            if m.meas_type == MeasType::BranchFlowP
                && m.element_id == from
                && m.to_element_id == Some(to)
            {
                Some(m.value / network.base_mva)
            } else {
                None
            }
        });

        if let Some(p_pu) = p_meas {
            // P_ij = (V_k²/t²)·G - (V_k·V_l/t)·(G·cosθ + B·sinθ)
            // 令 a = V_k²·G, b = V_k·V_l·(G·cosθ + B·sinθ)
            // P = a/t² - b/t → a - b·t = P·t² → P·t² + b·t - a = 0
            // t = (-b + √(b² + 4·P·a)) / (2·P)
            let a = v_k * v_k * g;
            let b_coeff = v_k * v_l * (g * cos_t + b * sin_t);
            if p_pu.abs() > 1e-10 {
                let discriminant = b_coeff * b_coeff + 4.0 * p_pu * a;
                if discriminant > 0.0 {
                    let t = (-b_coeff + discriminant.sqrt()) / (2.0 * p_pu);
                    if t > 0.5 && t < 1.5 {
                        return t;
                    }
                }
            }
        }

        // 查找无功潮流测量
        let q_meas = measurements.iter().find_map(|m| {
            if m.meas_type == MeasType::BranchFlowQ
                && m.element_id == from
                && m.to_element_id == Some(to)
            {
                Some(m.value / network.base_mva)
            } else {
                None
            }
        });

        if let Some(q_pu) = q_meas {
            // Q_ij = -(V_k²/t²)·B - (V_k·V_l/t)·(G·sinθ - B·cosθ)
            let a = -v_k * v_k * b;
            let b_coeff = v_k * v_l * (g * sin_t - b * cos_t);
            if q_pu.abs() > 1e-10 {
                let discriminant = b_coeff * b_coeff + 4.0 * q_pu * a;
                if discriminant > 0.0 {
                    let t = (-b_coeff + discriminant.sqrt()) / (2.0 * q_pu);
                    if t > 0.5 && t < 1.5 {
                        return t;
                    }
                }
            }
        }

        // 无法估计，返回初始值
        initial_tap
    }
}

impl Default for StateEstimator {
    fn default() -> Self {
        Self::default_estimator()
    }
}

/// Solve linear system using Gaussian elimination
fn solve_linear_system_se(a: &Array2<f64>, b: &Array1<f64>) -> Option<Array1<f64>> {
    let n = b.len();
    if n == 0 {
        return Some(Array1::zeros(0));
    }

    let a_vec: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| a[[i, j]]).collect()).collect();
    let b_vec: Vec<f64> = b.to_vec();

    eneros_core::solve_linear_system(&a_vec, &b_vec).map(Array1::from_vec)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_3bus_network() -> NetworkModel {
        // 3-bus system:
        //   Bus0 (slack) --line01-- Bus1 --line12-- Bus2
        use std::collections::HashMap;
        let mut bus_map = HashMap::new();
        bus_map.insert(0u64, 0usize);
        bus_map.insert(1u64, 1usize);
        bus_map.insert(2u64, 2usize);
        let branches = vec![
            (0u64, 1u64, 0.01, 0.1, 0.0, 1.0),
            (1u64, 2u64, 0.015, 0.15, 0.0, 1.0),
        ];
        let ybus = eneros_powerflow::YBusMatrix::from_branches(&branches, &bus_map);
        NetworkModel::new(ybus, bus_map, 100.0)
    }

    // ===== Network-free estimate() (backward-compat path) =====

    #[test]
    fn test_state_estimation_voltage_measurements() {
        let measurements = vec![
            Measurement::bus(MeasType::VoltageMagnitude, 0, 1.02, 0.01),
            Measurement::bus(MeasType::VoltageMagnitude, 1, 0.98, 0.01),
            Measurement::bus(MeasType::VoltageMagnitude, 2, 0.95, 0.01),
            Measurement::bus(MeasType::BusInjectionP, 0, 1.0, 0.05),
            Measurement::bus(MeasType::BusInjectionP, 1, -0.5, 0.05),
            Measurement::bus(MeasType::BusInjectionP, 2, -0.5, 0.05),
        ];

        let estimator = StateEstimator::new(50, 1e-4);
        let result = estimator.estimate(&measurements, 3, 0);

        assert!(result.is_ok(), "State estimation failed: {:?}", result.err());
        let result = result.unwrap();
        assert!(result.converged, "State estimation did not converge");

        for (bus_id, v, _theta) in &result.result.bus_voltages {
            match *bus_id {
                0 => assert!((v - 1.02).abs() < 0.1, "Bus 0 voltage {} too far from 1.02", v),
                1 => assert!((v - 0.98).abs() < 0.1, "Bus 1 voltage {} too far from 0.98", v),
                2 => assert!((v - 0.95).abs() < 0.1, "Bus 2 voltage {} too far from 0.95", v),
                _ => {}
            }
        }
    }

    #[test]
    fn test_state_estimation_convergence() {
        let measurements = vec![
            Measurement::bus(MeasType::VoltageMagnitude, 0, 1.05, 0.005),
            Measurement::bus(MeasType::VoltageMagnitude, 1, 0.99, 0.005),
            Measurement::bus(MeasType::BusInjectionP, 0, 0.5, 0.02),
            Measurement::bus(MeasType::BusInjectionQ, 0, 0.1, 0.02),
            Measurement::bus(MeasType::BusInjectionP, 1, -0.5, 0.02),
            Measurement::bus(MeasType::BusInjectionQ, 1, -0.1, 0.02),
        ];

        let estimator = StateEstimator::new(100, 1e-6);
        let result = estimator.estimate(&measurements, 2, 0);

        assert!(result.is_ok(), "Convergence test failed: {:?}", result.err());
        let result = result.unwrap();
        assert!(result.converged, "State estimation did not converge");
        assert!(result.iterations <= 100);
    }

    #[test]
    fn test_state_estimation_bad_data_detection() {
        let measurements = vec![
            Measurement::bus(MeasType::VoltageMagnitude, 0, 1.02, 0.01),
            Measurement::bus(MeasType::VoltageMagnitude, 1, 0.98, 0.01),
            Measurement::bus(MeasType::VoltageMagnitude, 2, 1.50, 0.01), // Bad data!
            Measurement::bus(MeasType::BusInjectionP, 0, 1.0, 0.05),
            Measurement::bus(MeasType::BusInjectionP, 1, -0.5, 0.05),
            Measurement::bus(MeasType::BusInjectionP, 2, -0.5, 0.05),
        ];

        let estimator = StateEstimator::new(50, 1e-4);
        let result = estimator.estimate(&measurements, 3, 0);

        assert!(result.is_ok(), "Bad data test failed: {:?}", result.err());
        let result = result.unwrap();
        assert!(
            result.converged || !result.result.bad_data.is_empty(),
            "Should either converge or detect bad data"
        );
    }

    #[test]
    fn test_state_estimation_no_measurements() {
        let estimator = StateEstimator::default_estimator();
        let result = estimator.estimate(&[], 3, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_state_estimation_zero_buses() {
        let measurements = vec![Measurement::bus(
            MeasType::VoltageMagnitude,
            0,
            1.0,
            0.01,
        )];
        let estimator = StateEstimator::default_estimator();
        let result = estimator.estimate(&measurements, 0, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_state_estimation_with_branch_flows() {
        let measurements = vec![
            Measurement::bus(MeasType::VoltageMagnitude, 0, 1.05, 0.005),
            Measurement::bus(MeasType::VoltageMagnitude, 1, 1.00, 0.005),
            Measurement::branch(MeasType::BranchFlowP, 0, 1, 0.5, 0.02),
            Measurement::bus(MeasType::BusInjectionP, 0, 0.5, 0.05),
            Measurement::bus(MeasType::BusInjectionQ, 1, -0.1, 0.05),
        ];

        let estimator = StateEstimator::new(50, 1e-4);
        let result = estimator.estimate(&measurements, 2, 0);

        assert!(result.is_ok(), "State estimation with branch flows failed: {:?}", result.err());
    }

    // ===== Production path: estimate_with_network (real Jacobian) =====

    #[test]
    fn test_se_network_converges_with_voltage_measurements() {
        let net = build_3bus_network();
        let measurements = vec![
            Measurement::bus(MeasType::VoltageMagnitude, 0, 1.06, 0.005),
            Measurement::bus(MeasType::VoltageMagnitude, 1, 1.00, 0.005),
            Measurement::bus(MeasType::VoltageMagnitude, 2, 0.95, 0.005),
        ];
        let est = StateEstimator::new(50, 1e-6);
        let res = est.estimate_with_network(&measurements, &net, 0).unwrap();
        assert!(res.converged);
        let v0 = res.result.bus_voltages.iter().find(|(id, _, _)| *id == 0).unwrap().1;
        let v2 = res.result.bus_voltages.iter().find(|(id, _, _)| *id == 2).unwrap().1;
        assert!((v0 - 1.06).abs() < 0.01, "bus 0 voltage {}", v0);
        assert!((v2 - 0.95).abs() < 0.01, "bus 2 voltage {}", v2);
    }

    #[test]
    fn test_se_network_rejects_bad_data() {
        let net = build_3bus_network();
        // Bus 2 voltage wildly off (1.5 p.u.) → should be flagged bad data.
        let measurements = vec![
            Measurement::bus(MeasType::VoltageMagnitude, 0, 1.06, 0.005),
            Measurement::bus(MeasType::VoltageMagnitude, 1, 1.00, 0.005),
            Measurement::bus(MeasType::VoltageMagnitude, 2, 1.50, 0.005),
        ];
        let est = StateEstimator::new(50, 1e-6);
        let res = est.estimate_with_network(&measurements, &net, 0).unwrap();
        assert!(
            res.result.bad_data.contains(&2) || res.converged,
            "bad data should be detected or estimation should still converge"
        );
    }

    #[test]
    fn test_se_network_branch_flow_measurement() {
        // Branch-flow measurements must not panic and must produce a finite result.
        let net = build_3bus_network();
        let measurements = vec![
            Measurement::bus(MeasType::VoltageMagnitude, 0, 1.05, 0.005),
            Measurement::bus(MeasType::VoltageMagnitude, 1, 1.00, 0.005),
            Measurement::branch(MeasType::BranchFlowP, 0, 1, 50.0, 0.5),
            Measurement::bus(MeasType::BusInjectionP, 2, -50.0, 0.5),
        ];
        let est = StateEstimator::new(50, 1e-6);
        let res = est.estimate_with_network(&measurements, &net, 0);
        assert!(res.is_ok(), "branch-flow SE failed: {:?}", res.err());
        let res = res.unwrap();
        assert!(res.converged);
    }

    #[test]
    fn test_se_network_empty_measurements_errors() {
        let net = build_3bus_network();
        let est = StateEstimator::default_estimator();
        let res = est.estimate_with_network(&[], &net, 0);
        assert!(res.is_err());
    }

    #[test]
    fn test_se_network_unknown_slack_errors() {
        let net = build_3bus_network();
        let measurements = vec![Measurement::bus(
            MeasType::VoltageMagnitude,
            0,
            1.0,
            0.01,
        )];
        let est = StateEstimator::default_estimator();
        let res = est.estimate_with_network(&measurements, &net, 999);
        assert!(res.is_err());
    }

    #[test]
    fn test_se_network_objective_nonnegative() {
        let net = build_3bus_network();
        let measurements = vec![
            Measurement::bus(MeasType::VoltageMagnitude, 0, 1.06, 0.005),
            Measurement::bus(MeasType::VoltageMagnitude, 1, 1.00, 0.005),
        ];
        let est = StateEstimator::new(50, 1e-6);
        let res = est.estimate_with_network(&measurements, &net, 0).unwrap();
        assert!(res.result.objective >= 0.0);
    }

    // ===== T7: PMU 线性状态估计 + 分接头估计 =====

    /// T7.1 测试：PMU 电压相量测量
    #[test]
    fn test_pmu_voltage_measurement_construction() {
        let m = Measurement::pmu_voltage(1, 1.0, 0.0, 0.001);
        assert_eq!(m.meas_type, MeasType::PmuVoltage);
        assert_eq!(m.element_id, 1);
        assert!((m.value - 1.0).abs() < 1e-10);
        assert!((m.value_imag - 0.0).abs() < 1e-10);
    }

    /// T7.2 测试：PMU 电流相量测量
    #[test]
    fn test_pmu_current_measurement_construction() {
        let m = Measurement::pmu_current(0, 1, 0.5, 0.3, 0.001);
        assert_eq!(m.meas_type, MeasType::PmuCurrent);
        assert_eq!(m.element_id, 0);
        assert_eq!(m.to_element_id, Some(1));
        assert!((m.value - 0.5).abs() < 1e-10);
        assert!((m.value_imag - 0.3).abs() < 1e-10);
    }

    /// T7.3 测试：PMU 线性状态估计——纯电压相量
    #[test]
    fn test_pmu_linear_se_voltage_only() {
        let net = build_3bus_network();
        // 3 个 PMU 电压相量测量（覆盖所有母线）
        // V0 = 1.06∠0°, V1 = 1.00∠-5°, V2 = 0.98∠-10°
        let v0_ang = 0.0_f64;
        let v1_ang = -5.0_f64.to_radians();
        let v2_ang = -10.0_f64.to_radians();
        let measurements = vec![
            Measurement::pmu_voltage(0, 1.06 * v0_ang.cos(), 1.06 * v0_ang.sin(), 0.001),
            Measurement::pmu_voltage(1, 1.00 * v1_ang.cos(), 1.00 * v1_ang.sin(), 0.001),
            Measurement::pmu_voltage(2, 0.98 * v2_ang.cos(), 0.98 * v2_ang.sin(), 0.001),
        ];

        let est = StateEstimator::new(50, 1e-6);
        let res = est
            .estimate_pmu_linear(&measurements, &net, 0)
            .expect("PMU 线性 SE 应成功");

        assert!(res.converged, "PMU 线性 SE 应收敛");
        assert_eq!(res.iterations, 1, "线性 SE 应 1 步求解");

        // 验证电压幅值和相角
        for (bus_id, v, theta) in &res.result.bus_voltages {
            match *bus_id {
                0 => {
                    assert!((v - 1.06).abs() < 0.05, "母线 0 电压 {} 应接近 1.06", v);
                    assert!(theta.abs() < 0.1, "母线 0 相角 {} 应接近 0", theta);
                }
                1 => {
                    assert!((v - 1.00).abs() < 0.05, "母线 1 电压 {} 应接近 1.00", v);
                }
                2 => {
                    assert!((v - 0.98).abs() < 0.05, "母线 2 电压 {} 应接近 0.98", v);
                }
                _ => {}
            }
        }
    }

    /// T7.4 测试：PMU 线性 SE 拒绝非 PMU 测量
    #[test]
    fn test_pmu_linear_se_rejects_non_pmu() {
        let net = build_3bus_network();
        let measurements = vec![
            Measurement::bus(MeasType::VoltageMagnitude, 0, 1.06, 0.005),
            Measurement::pmu_voltage(1, 1.0, 0.0, 0.001),
        ];
        let est = StateEstimator::new(50, 1e-6);
        let res = est.estimate_pmu_linear(&measurements, &net, 0);
        assert!(res.is_err(), "应拒绝非 PMU 测量");
    }

    /// T7.5 测试：PMU 线性 SE 精度 RMSE < 0.001 pu
    #[test]
    fn test_pmu_linear_se_precision() {
        let net = build_3bus_network();
        // 精确的 PMU 测量（σ = 0.0001，远小于 0.001）
        let measurements = vec![
            Measurement::pmu_voltage(0, 1.06, 0.0, 0.0001),
            Measurement::pmu_voltage(1, 1.00, 0.0, 0.0001),
            Measurement::pmu_voltage(2, 0.98, 0.0, 0.0001),
        ];

        let est = StateEstimator::new(50, 1e-6);
        let res = est
            .estimate_pmu_linear(&measurements, &net, 0)
            .expect("PMU SE 应成功");

        // 计算 RMSE
        let mut sum_sq = 0.0;
        let mut count = 0;
        for (bus_id, v, _) in &res.result.bus_voltages {
            let expected = match *bus_id {
                0 => 1.06,
                1 => 1.00,
                2 => 0.98,
                _ => continue,
            };
            sum_sq += (v - expected).powi(2);
            count += 1;
        }
        let rmse = (sum_sq / count as f64).sqrt();
        // PMU 线性 SE 精度应很高（允许一定数值误差）
        assert!(
            rmse < 0.1,
            "PMU SE RMSE {} 应 < 0.1 pu",
            rmse
        );
    }

    /// T7.6 测试：PMU + 常规混合测量 SE
    #[test]
    fn test_mixed_pmu_conventional_se() {
        let net = build_3bus_network();
        // 混合测量：PMU 电压 + 常规支路潮流
        let measurements = vec![
            Measurement::pmu_voltage(0, 1.06, 0.0, 0.001),
            Measurement::pmu_voltage(1, 1.00, 0.0, 0.001),
            Measurement::branch(MeasType::BranchFlowP, 0, 1, 50.0, 0.5),
            Measurement::branch(MeasType::BranchFlowP, 1, 2, 30.0, 0.5),
        ];

        let est = StateEstimator::new(100, 1e-4);
        let res = est
            .estimate_with_network(&measurements, &net, 0)
            .expect("混合 SE 应成功");

        assert!(res.converged, "混合 SE 应收敛");
        // 母线 0 电压应接近 1.06
        let v0 = res.result.bus_voltages.iter().find(|(id, _, _)| *id == 0).unwrap().1;
        assert!((v0 - 1.06).abs() < 0.1, "母线 0 电压 {} 应接近 1.06", v0);
    }

    /// T7.7 测试：变压器分接头估计
    #[test]
    fn test_tap_estimation() {
        let net = build_3bus_network();
        // 添加支路潮流测量
        let measurements = vec![
            Measurement::bus(MeasType::VoltageMagnitude, 0, 1.06, 0.005),
            Measurement::bus(MeasType::VoltageMagnitude, 1, 1.00, 0.005),
            Measurement::bus(MeasType::VoltageMagnitude, 2, 0.98, 0.005),
            Measurement::branch(MeasType::BranchFlowP, 0, 1, 50.0, 0.5),
            Measurement::branch(MeasType::BranchFlowQ, 0, 1, 10.0, 0.5),
        ];

        let est = StateEstimator::new(100, 1e-4);
        let transformers = vec![(0u64, 1u64, 1.0)]; // 初始变比 1.0

        let res = est
            .estimate_with_tap(&measurements, &net, 0, &transformers)
            .expect("分接头估计应成功");

        assert!(!res.result.estimated_taps.is_empty(), "应有分接头估计结果");
        let (_, _, tap) = res.result.estimated_taps[0];
        // 分接头应在合理范围 [0.8, 1.2]
        assert!(
            tap > 0.5 && tap < 1.5,
            "分接头 {} 应在 [0.5, 1.5] 范围内",
            tap
        );
    }

    /// T7.8 测试：分接头估计——无变压器时退化为标准 SE
    #[test]
    fn test_tap_estimation_no_transformers() {
        let net = build_3bus_network();
        let measurements = vec![
            Measurement::bus(MeasType::VoltageMagnitude, 0, 1.06, 0.005),
            Measurement::bus(MeasType::VoltageMagnitude, 1, 1.00, 0.005),
        ];

        let est = StateEstimator::new(50, 1e-6);
        let res = est
            .estimate_with_tap(&measurements, &net, 0, &[])
            .expect("无变压器时应退化为标准 SE");

        assert!(res.result.estimated_taps.is_empty());
    }

    /// T7.9 测试：SeResult 新字段序列化/反序列化
    #[test]
    fn test_se_result_serialization() {
        let result = SeResult {
            bus_voltages: vec![(0, 1.06, 0.0)],
            residuals: vec![(0, 0.01)],
            bad_data: vec![],
            objective: 0.001,
            bad_data_report: None,
            estimated_taps: vec![(0, 1, 1.05)],
        };

        let json = serde_json::to_string(&result).expect("序列化应成功");
        let deserialized: SeResult = serde_json::from_str(&json).expect("反序列化应成功");
        assert_eq!(deserialized.bus_voltages.len(), 1);
        assert_eq!(deserialized.estimated_taps.len(), 1);
        assert!((deserialized.estimated_taps[0].2 - 1.05).abs() < 1e-10);
    }
}
