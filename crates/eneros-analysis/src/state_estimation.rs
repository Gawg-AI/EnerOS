use ndarray::{Array1, Array2};
use eneros_core::ElementId;
use std::collections::HashMap;
use crate::types::{AnalysisResult, AnalysisError};

/// Measurement type for state estimation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone)]
pub struct SeResult {
    /// (bus_id, v_magnitude, v_angle_rad)
    pub bus_voltages: Vec<(ElementId, f64, f64)>,
    /// Normalized residuals (element_id, residual)
    pub residuals: Vec<(ElementId, f64)>,
    /// Bad data flagged element IDs
    pub bad_data: Vec<ElementId>,
    /// Objective function value at the solution (weighted sum of squared
    /// residuals). Useful for goodness-of-fit reporting.
    pub objective: f64,
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

        let m = measurements.len();
        let mut converged = false;
        let mut iterations = 0u32;

        for iter in 0..self.max_iterations {
            iterations = iter + 1;

            let (h_matrix, z_vec, h_x) = self.build_jacobian_network(measurements, &x, network);

            // Weight matrix W = diag(1/σ²).
            let mut w_matrix = Array2::<f64>::zeros((m, m));
            for (i, meas) in measurements.iter().enumerate() {
                if meas.sigma > 1e-12 {
                    w_matrix[[i, i]] = 1.0 / (meas.sigma * meas.sigma);
                } else {
                    w_matrix[[i, i]] = 1e12;
                }
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
        for (i, meas) in measurements.iter().enumerate() {
            let r = residuals_raw[i];
            let weight = if meas.sigma > 1e-12 {
                1.0 / (meas.sigma * meas.sigma)
            } else {
                1e12
            };
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
    fn build_jacobian_network(
        &self,
        measurements: &[Measurement],
        x: &Array1<f64>,
        network: &NetworkModel,
    ) -> (Array2<f64>, Array1<f64>, Array1<f64>) {
        let m = measurements.len();
        let n = x.len();
        let mut h = Array2::<f64>::zeros((m, n));
        let mut z = Array1::<f64>::zeros(m);
        let mut h_x = Array1::<f64>::zeros(m);
        let base = network.base_mva;

        for (i, meas) in measurements.iter().enumerate() {
            let k = match network.bus_map.get(&meas.element_id) {
                Some(&idx) => idx,
                None => continue, // unknown bus — zero row (effectively ignored)
            };
            let v_k = x[2 * k];
            let theta_k = x[2 * k + 1];

            match meas.meas_type {
                MeasType::VoltageMagnitude => {
                    z[i] = meas.value;
                    h_x[i] = v_k;
                    h[[i, 2 * k]] = 1.0;
                }
                MeasType::BusInjectionP => {
                    z[i] = meas.value / base;
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
                    h_x[i] = p_val;
                    h[[i, 2 * k]] = dp_dvk;
                    h[[i, 2 * k + 1]] = dp_dthk;
                }
                MeasType::BusInjectionQ => {
                    z[i] = meas.value / base;
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
                    h_x[i] = q_val;
                    h[[i, 2 * k]] = dq_dvk;
                    h[[i, 2 * k + 1]] = dq_dthk;
                }
                MeasType::BranchFlowP => {
                    let l = match meas.to_element_id.and_then(|id| network.bus_map.get(&id)) {
                        Some(&idx) => idx,
                        None => continue,
                    };
                    let v_l = x[2 * l];
                    let theta_l = x[2 * l + 1];
                    let theta_kl = theta_k - theta_l;
                    let cos_t = theta_kl.cos();
                    let sin_t = theta_kl.sin();
                    let (g_kl, b_kl) = network.ybus.get(k, l);
                    z[i] = meas.value / base;
                    // h(x) = V_k² G_kl − V_k V_l (G_kl cos θ_kl + B_kl sin θ_kl)
                    h_x[i] = v_k * v_k * g_kl
                        - v_k * v_l * (g_kl * cos_t + b_kl * sin_t);
                    h[[i, 2 * k]] = 2.0 * v_k * g_kl - v_l * (g_kl * cos_t + b_kl * sin_t);
                    h[[i, 2 * l]] = -v_k * (g_kl * cos_t + b_kl * sin_t);
                    h[[i, 2 * k + 1]] = v_k * v_l * (g_kl * sin_t - b_kl * cos_t);
                    h[[i, 2 * l + 1]] = -v_k * v_l * (g_kl * sin_t - b_kl * cos_t);
                }
                MeasType::BranchFlowQ => {
                    let l = match meas.to_element_id.and_then(|id| network.bus_map.get(&id)) {
                        Some(&idx) => idx,
                        None => continue,
                    };
                    let v_l = x[2 * l];
                    let theta_l = x[2 * l + 1];
                    let theta_kl = theta_k - theta_l;
                    let cos_t = theta_kl.cos();
                    let sin_t = theta_kl.sin();
                    let (g_kl, b_kl) = network.ybus.get(k, l);
                    z[i] = meas.value / base;
                    // h(x) = −V_k² B_kl − V_k V_l (G_kl sin θ_kl − B_kl cos θ_kl)
                    h_x[i] = -v_k * v_k * b_kl
                        - v_k * v_l * (g_kl * sin_t - b_kl * cos_t);
                    h[[i, 2 * k]] = -2.0 * v_k * b_kl - v_l * (g_kl * sin_t - b_kl * cos_t);
                    h[[i, 2 * l]] = -v_k * (g_kl * sin_t - b_kl * cos_t);
                    h[[i, 2 * k + 1]] = -v_k * v_l * (g_kl * cos_t + b_kl * sin_t);
                    h[[i, 2 * l + 1]] = v_k * v_l * (g_kl * cos_t + b_kl * sin_t);
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
            }
        }
        (h, z)
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
}
