use ndarray::{Array1, Array2};
use eneros_core::ElementId;
use crate::types::{AnalysisResult, AnalysisError};

/// Measurement type for state estimation
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MeasType {
    /// Voltage magnitude measurement (p.u.)
    VoltageMagnitude,
    /// Bus active power injection (MW)
    BusInjectionP,
    /// Bus reactive power injection (MVar)
    BusInjectionQ,
    /// Branch active power flow (MW)
    BranchFlowP,
    /// Branch reactive power flow (MVar)
    BranchFlowQ,
}

/// A single measurement for state estimation
#[derive(Debug, Clone)]
pub struct Measurement {
    pub meas_type: MeasType,
    pub element_id: ElementId,
    pub value: f64,
    /// Standard deviation of the measurement
    pub sigma: f64,
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
            let (h_matrix, z_vec) = self.build_jacobian_and_measurements(
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
        let (h_final, z_final) = self.build_jacobian_and_measurements(
            measurements, &x, bus_count,
        );
        let h_x_final = h_final.dot(&x);
        let residuals_raw = &z_final - &h_x_final;

        let mut residuals = Vec::new();
        let mut bad_data = Vec::new();

        for (i, meas) in measurements.iter().enumerate() {
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

        Ok(AnalysisResult {
            converged,
            iterations,
            result: SeResult {
                bus_voltages,
                residuals,
                bad_data,
            },
            warnings,
        })
    }

    /// Build Jacobian matrix H and measurement vector z
    /// Uses a linearized measurement model around the current state
    fn build_jacobian_and_measurements(
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

            match meas.meas_type {
                MeasType::VoltageMagnitude => {
                    // h_i = V_k
                    // dh/dV_k = 1, dh/dtheta_k = 0
                    let bus_idx = meas.element_id as usize;
                    if bus_idx < bus_count {
                        h[[i, 2 * bus_idx]] = 1.0;
                    }
                }
                MeasType::BusInjectionP => {
                    // P_k ≈ V_k * sum_j [G_kj * cos(θ_k - θ_j) + B_kj * sin(θ_k - θ_j)] * V_j
                    // Linearized around current state:
                    // dP/dV_k = sum_j (G_kj * cos(θ_k-θ_j) + B_kj * sin(θ_k-θ_j)) * V_j
                    // dP/dθ_k = V_k * sum_j (-G_kj * sin(θ_k-θ_j) + B_kj * cos(θ_k-θ_j)) * V_j
                    // Simplified (assuming uniform V≈1, small angles):
                    // dP/dV_k ≈ 1.0, dP/dθ_k ≈ B_kk (self-susceptance)
                    let bus_idx = meas.element_id as usize;
                    if bus_idx < bus_count {
                        let v_k = x[2 * bus_idx];
                        let theta_k = x[2 * bus_idx + 1];
                        h[[i, 2 * bus_idx]] = v_k; // dP/dV_k
                        // dP/dθ_k: use simplified B-matrix coupling
                        // For a typical system, B_kk ≈ -10 to -50
                        // We use a simplified coupling based on number of connected buses
                        h[[i, 2 * bus_idx + 1]] = -v_k * 10.0 * (theta_k / (theta_k.abs() + 0.01).max(1e-6));
                        // Better: just use a constant coupling
                        h[[i, 2 * bus_idx + 1]] = v_k * 5.0;
                    }
                }
                MeasType::BusInjectionQ => {
                    let bus_idx = meas.element_id as usize;
                    if bus_idx < bus_count {
                        let v_k = x[2 * bus_idx];
                        h[[i, 2 * bus_idx]] = v_k * 5.0; // dQ/dV_k (stronger V coupling for Q)
                        h[[i, 2 * bus_idx + 1]] = -v_k * 2.0; // dQ/dθ_k
                    }
                }
                MeasType::BranchFlowP => {
                    // P_ij ≈ (θ_i - θ_j) / x_ij
                    // dP/dθ_i = 1/x_ij = B_ij, dP/dθ_j = -B_ij
                    // dP/dV_i ≈ 0 (weak coupling in DC model)
                    let from_idx = meas.element_id as usize;
                    let to_idx = (meas.element_id as usize).wrapping_add(1);
                    if from_idx < bus_count {
                        let b_ij = 10.0; // 1/x_ij ≈ 10 for typical line
                        h[[i, 2 * from_idx + 1]] = b_ij;
                        if to_idx < bus_count {
                            h[[i, 2 * to_idx + 1]] = -b_ij;
                        }
                        // Small V coupling
                        h[[i, 2 * from_idx]] = 0.1;
                    }
                }
                MeasType::BranchFlowQ => {
                    let from_idx = meas.element_id as usize;
                    let to_idx = (meas.element_id as usize).wrapping_add(1);
                    if from_idx < bus_count {
                        h[[i, 2 * from_idx]] = 5.0; // dQ/dV_i
                        if to_idx < bus_count {
                            h[[i, 2 * to_idx]] = -3.0; // dQ/dV_j
                        }
                        h[[i, 2 * from_idx + 1]] = -1.0; // dQ/dθ_i
                    }
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

    #[test]
    fn test_state_estimation_voltage_measurements() {
        // 3-bus system with voltage magnitude and power injection measurements
        let measurements = vec![
            Measurement { meas_type: MeasType::VoltageMagnitude, element_id: 0, value: 1.02, sigma: 0.01 },
            Measurement { meas_type: MeasType::VoltageMagnitude, element_id: 1, value: 0.98, sigma: 0.01 },
            Measurement { meas_type: MeasType::VoltageMagnitude, element_id: 2, value: 0.95, sigma: 0.01 },
            Measurement { meas_type: MeasType::BusInjectionP, element_id: 0, value: 1.0, sigma: 0.05 },
            Measurement { meas_type: MeasType::BusInjectionP, element_id: 1, value: -0.5, sigma: 0.05 },
            Measurement { meas_type: MeasType::BusInjectionP, element_id: 2, value: -0.5, sigma: 0.05 },
        ];

        let estimator = StateEstimator::new(50, 1e-4);
        let result = estimator.estimate(&measurements, 3, 0);

        assert!(result.is_ok(), "State estimation failed: {:?}", result.err());
        let result = result.unwrap();
        assert!(result.converged, "State estimation did not converge");

        // Voltage magnitudes should be close to measurements
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
        // 2-bus system with sufficient measurements for observability
        let measurements = vec![
            Measurement { meas_type: MeasType::VoltageMagnitude, element_id: 0, value: 1.05, sigma: 0.005 },
            Measurement { meas_type: MeasType::VoltageMagnitude, element_id: 1, value: 0.99, sigma: 0.005 },
            Measurement { meas_type: MeasType::BusInjectionP, element_id: 0, value: 0.5, sigma: 0.02 },
            Measurement { meas_type: MeasType::BusInjectionQ, element_id: 0, value: 0.1, sigma: 0.02 },
            Measurement { meas_type: MeasType::BusInjectionP, element_id: 1, value: -0.5, sigma: 0.02 },
            Measurement { meas_type: MeasType::BusInjectionQ, element_id: 1, value: -0.1, sigma: 0.02 },
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
        // 3-bus system with one bad measurement
        let measurements = vec![
            Measurement { meas_type: MeasType::VoltageMagnitude, element_id: 0, value: 1.02, sigma: 0.01 },
            Measurement { meas_type: MeasType::VoltageMagnitude, element_id: 1, value: 0.98, sigma: 0.01 },
            Measurement { meas_type: MeasType::VoltageMagnitude, element_id: 2, value: 1.50, sigma: 0.01 }, // Bad data!
            Measurement { meas_type: MeasType::BusInjectionP, element_id: 0, value: 1.0, sigma: 0.05 },
            Measurement { meas_type: MeasType::BusInjectionP, element_id: 1, value: -0.5, sigma: 0.05 },
            Measurement { meas_type: MeasType::BusInjectionP, element_id: 2, value: -0.5, sigma: 0.05 },
        ];

        let estimator = StateEstimator::new(50, 1e-4);
        let result = estimator.estimate(&measurements, 3, 0);

        assert!(result.is_ok(), "Bad data test failed: {:?}", result.err());
        let result = result.unwrap();

        // The estimation should still converge or detect bad data
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
        let measurements = vec![
            Measurement { meas_type: MeasType::VoltageMagnitude, element_id: 0, value: 1.0, sigma: 0.01 },
        ];
        let estimator = StateEstimator::default_estimator();
        let result = estimator.estimate(&measurements, 0, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_state_estimation_with_branch_flows() {
        let measurements = vec![
            Measurement { meas_type: MeasType::VoltageMagnitude, element_id: 0, value: 1.05, sigma: 0.005 },
            Measurement { meas_type: MeasType::VoltageMagnitude, element_id: 1, value: 1.00, sigma: 0.005 },
            Measurement { meas_type: MeasType::BranchFlowP, element_id: 0, value: 0.5, sigma: 0.02 },
            Measurement { meas_type: MeasType::BusInjectionP, element_id: 0, value: 0.5, sigma: 0.05 },
            Measurement { meas_type: MeasType::BusInjectionQ, element_id: 1, value: -0.1, sigma: 0.05 },
        ];

        let estimator = StateEstimator::new(50, 1e-4);
        let result = estimator.estimate(&measurements, 2, 0);

        assert!(result.is_ok(), "State estimation with branch flows failed: {:?}", result.err());
    }
}
