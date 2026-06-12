use eneros_core::{ElementId, Result, EnerOSError};
use crate::matrix::YBusMatrix;
use crate::result::{PowerFlowResult, BusResult, BranchResult};

/// Power flow solver using Newton-Raphson method
pub struct PowerFlowSolver {
    max_iterations: u32,
    tolerance: f64,
}

impl PowerFlowSolver {
    pub fn new(max_iterations: u32, tolerance: f64) -> Self {
        Self {
            max_iterations,
            tolerance,
        }
    }

    pub fn default_solver() -> Self {
        Self::new(50, 1e-6)
    }

    /// Solve power flow using Newton-Raphson method
    pub fn solve(
        &self,
        ybus: &YBusMatrix,
        p_spec: &[f64],
        q_spec: &[f64],
        bus_types: &[BusTypeNR],
    ) -> Result<PowerFlowResult> {
        let n = ybus.size();

        let mut v = vec![1.0; n];
        let mut theta = vec![0.0; n];

        let mut converged = false;
        let mut iterations = 0;

        for iter in 0..self.max_iterations {
            iterations = iter + 1;

            let (dp, dq) = self.calculate_mismatches(&v, &theta, ybus, p_spec, q_spec, bus_types)?;

            let max_mismatch = dp.iter().chain(dq.iter()).fold(0.0_f64, |a, &b| a.max(b.abs()));

            if max_mismatch < self.tolerance {
                converged = true;
                break;
            }

            let jacobian = self.build_jacobian(&v, &theta, ybus, bus_types)?;

            let mut rhs = Vec::new();
            rhs.extend_from_slice(&dp);
            rhs.extend_from_slice(&dq);

            let dx = gaussian_elimination(&jacobian, &rhs)?;

            // Damping / line search
            let mut alpha = 1.0;
            let v_orig = v.clone();
            let theta_orig = theta.clone();

            for _ in 0..20 {
                let mut idx = 0;
                for (i, &bt) in bus_types.iter().enumerate() {
                    if bt != BusTypeNR::Slack {
                        theta[i] = theta_orig[i] + alpha * dx[idx];
                        idx += 1;
                    }
                    if bt == BusTypeNR::PQ {
                        v[i] = v_orig[i] + alpha * dx[idx];
                        idx += 1;
                    }
                }

                let (new_dp, new_dq) = self.calculate_mismatches(&v, &theta, ybus, p_spec, q_spec, bus_types)?;
                let new_max = new_dp.iter().chain(new_dq.iter()).fold(0.0_f64, |a, &b| a.max(b.abs()));

                if new_max < max_mismatch {
                    break;
                }
                alpha *= 0.5;
            }
        }

        if !converged {
            return Err(EnerOSError::PowerFlow(format!(
                "Power flow did not converge after {} iterations",
                self.max_iterations
            )));
        }

        let branch_results = self.calculate_branch_flows(&v, &theta, ybus)?;
        let bus_results = self.calculate_bus_results(&v, &theta, ybus, p_spec, q_spec, bus_types);
        let total_losses = branch_results.iter().map(|br| br.loss_mw).sum();

        Ok(PowerFlowResult {
            converged,
            iterations,
            bus_results,
            branch_results,
            total_losses,
        })
    }

    fn calculate_mismatches(
        &self,
        v: &[f64],
        theta: &[f64],
        ybus: &YBusMatrix,
        p_spec: &[f64],
        q_spec: &[f64],
        bus_types: &[BusTypeNR],
    ) -> Result<(Vec<f64>, Vec<f64>)> {
        let n = v.len();
        let mut dp = Vec::new();
        let mut dq = Vec::new();

        for i in 0..n {
            if bus_types[i] == BusTypeNR::Slack {
                continue;
            }

            let mut p_calc = 0.0;
            let mut q_calc = 0.0;

            for j in 0..n {
                let (g, b) = ybus.get(i, j);
                let angle_diff = theta[i] - theta[j];
                p_calc += v[i] * v[j] * (g * angle_diff.cos() + b * angle_diff.sin());
                q_calc += v[i] * v[j] * (g * angle_diff.sin() - b * angle_diff.cos());
            }

            dp.push(p_spec[i] - p_calc);
            if bus_types[i] == BusTypeNR::PQ {
                dq.push(q_spec[i] - q_calc);
            }
        }

        Ok((dp, dq))
    }

    fn build_jacobian(
        &self,
        v: &[f64],
        theta: &[f64],
        ybus: &YBusMatrix,
        bus_types: &[BusTypeNR],
    ) -> Result<Vec<Vec<f64>>> {
        let n = v.len();

        let pq_indices: Vec<usize> = bus_types
            .iter()
            .enumerate()
            .filter(|(_, &bt)| bt == BusTypeNR::PQ)
            .map(|(i, _)| i)
            .collect();

        let non_slack_indices: Vec<usize> = bus_types
            .iter()
            .enumerate()
            .filter(|(_, &bt)| bt != BusTypeNR::Slack)
            .map(|(i, _)| i)
            .collect();

        let nns = non_slack_indices.len();
        let npq = pq_indices.len();
        let size = nns + npq;
        let mut jacobian = vec![vec![0.0; size]; size];

        // J1: dP/dtheta (standard: dP_i/dtheta_i = -Q_i - B_ii*V_i^2)
        for (ii, &i) in non_slack_indices.iter().enumerate() {
            for (jj, &j) in non_slack_indices.iter().enumerate() {
                if i == j {
                    let mut sum = 0.0;
                    for k in 0..n {
                        if k != i {
                            let (g_ik, b_ik) = ybus.get(i, k);
                            let angle_diff_ik = theta[i] - theta[k];
                            sum += v[i] * v[k] * (g_ik * angle_diff_ik.sin() - b_ik * angle_diff_ik.cos());
                        }
                    }
                    jacobian[ii][jj] = -sum;
                } else {
                    let (g, b) = ybus.get(i, j);
                    let angle_diff = theta[i] - theta[j];
                    jacobian[ii][jj] = -v[i] * v[j] * (g * angle_diff.sin() - b * angle_diff.cos());
                }
            }
        }

        // J2: dP/dV
        for (ii, &i) in non_slack_indices.iter().enumerate() {
            for (jj, &j) in pq_indices.iter().enumerate() {
                let (g, b) = ybus.get(i, j);
                let angle_diff = theta[i] - theta[j];

                if i == j {
                    // dP_i/dV_i = (1/V_i) * P_i + V_i * G_ii
                    let mut p_calc = 0.0;
                    for k in 0..n {
                        let (g_ik, b_ik) = ybus.get(i, k);
                        let angle_diff_ik = theta[i] - theta[k];
                        p_calc += v[i] * v[k] * (g_ik * angle_diff_ik.cos() + b_ik * angle_diff_ik.sin());
                    }
                    jacobian[ii][nns + jj] = p_calc / v[i] + v[i] * g;
                } else {
                    // dP_i/dV_j = V_i * (G_ij * cos(theta_i-theta_j) + B_ij * sin(theta_i-theta_j))
                    jacobian[ii][nns + jj] = v[i] * (g * angle_diff.cos() + b * angle_diff.sin());
                }
            }
        }

        // J3: dQ/dtheta (standard: dQ_i/dtheta_i = P_i - G_ii*V_i^2)
        for (ii, &i) in pq_indices.iter().enumerate() {
            for (jj, &j) in non_slack_indices.iter().enumerate() {
                if i == j {
                    let mut sum = 0.0;
                    for k in 0..n {
                        if k != i {
                            let (g_ik, b_ik) = ybus.get(i, k);
                            let angle_diff_ik = theta[i] - theta[k];
                            sum += v[i] * v[k] * (g_ik * angle_diff_ik.cos() + b_ik * angle_diff_ik.sin());
                        }
                    }
                    jacobian[nns + ii][jj] = sum;
                } else {
                    let (g, b) = ybus.get(i, j);
                    let angle_diff = theta[i] - theta[j];
                    jacobian[nns + ii][jj] = v[i] * v[j] * (g * angle_diff.cos() + b * angle_diff.sin());
                }
            }
        }

        // J4: dQ/dV
        for (ii, &i) in pq_indices.iter().enumerate() {
            for (jj, &j) in pq_indices.iter().enumerate() {
                let (g, b) = ybus.get(i, j);
                let angle_diff = theta[i] - theta[j];

                if i == j {
                    // dQ_i/dV_i = (1/V_i) * Q_i - V_i * B_ii
                    let mut q_calc = 0.0;
                    for k in 0..n {
                        let (g_ik, b_ik) = ybus.get(i, k);
                        let angle_diff_ik = theta[i] - theta[k];
                        q_calc += v[i] * v[k] * (g_ik * angle_diff_ik.sin() - b_ik * angle_diff_ik.cos());
                    }
                    jacobian[nns + ii][nns + jj] = q_calc / v[i] - v[i] * b;
                } else {
                    // dQ_i/dV_j = V_i * (G_ij * sin(theta_i-theta_j) - B_ij * cos(theta_i-theta_j))
                    jacobian[nns + ii][nns + jj] = v[i] * (g * angle_diff.sin() - b * angle_diff.cos());
                }
            }
        }

        Ok(jacobian)
    }

    fn calculate_branch_flows(
        &self,
        v: &[f64],
        theta: &[f64],
        ybus: &YBusMatrix,
    ) -> Result<Vec<BranchResult>> {
        let n = v.len();
        let mut branch_results = Vec::new();

        for i in 0..n {
            for j in (i + 1)..n {
                let (g, b) = ybus.get(i, j);
                if g.abs() < 1e-10 && b.abs() < 1e-10 {
                    continue;
                }

                let angle_diff = theta[i] - theta[j];
                let y_complex = num_complex::Complex::new(g, b);
                let v_i = num_complex::Complex::from_polar(v[i], theta[i]);
                let v_j = num_complex::Complex::from_polar(v[j], theta[j]);

                let i_ij = y_complex * (v_i - v_j);
                let s_ij = v_i * i_ij.conj();

                let p_mw = s_ij.re;
                let q_mvar = s_ij.im;

                let current_ka = i_ij.norm() / (v[i] * 1000.0);
                let rated_current_ka = 1.0;
                let loading_percent = (current_ka / rated_current_ka) * 100.0;

                let s_ji = v_j * (y_complex * (v_j - v_i)).conj();
                let loss_mw = (p_mw + s_ji.re).abs();

                let loss_mvar = (q_mvar + s_ji.im).abs();

                branch_results.push(BranchResult {
                    branch_id: (i * n + j) as ElementId,
                    from_bus: i as ElementId,
                    to_bus: j as ElementId,
                    p_from: p_mw,
                    q_from: q_mvar,
                    p_to: -s_ji.re,
                    q_to: -s_ji.im,
                    loss_mw,
                    loss_mvar,
                    loading_percent,
                });
            }
        }

        Ok(branch_results)
    }

    fn calculate_bus_results(
        &self,
        v: &[f64],
        theta: &[f64],
        _ybus: &YBusMatrix,
        p_spec: &[f64],
        q_spec: &[f64],
        _bus_types: &[BusTypeNR],
    ) -> Vec<BusResult> {
        v.iter()
            .enumerate()
            .map(|(i, &vi)| BusResult {
                bus_id: i as ElementId,
                voltage_magnitude: vi,
                voltage_angle: theta[i],
                p_injection: p_spec.get(i).copied().unwrap_or(0.0),
                q_injection: q_spec.get(i).copied().unwrap_or(0.0),
            })
            .collect()
    }
}

/// Gaussian elimination with partial pivoting
fn gaussian_elimination(matrix: &[Vec<f64>], rhs: &[f64]) -> Result<Vec<f64>> {
    let n = matrix.len();
    if n == 0 {
        return Ok(Vec::new());
    }

    let mut a: Vec<Vec<f64>> = matrix.to_vec();
    let mut b: Vec<f64> = rhs.to_vec();

    for col in 0..n {
        let mut max_val = a[col][col].abs();
        let mut max_row = col;

        for row in (col + 1)..n {
            if a[row][col].abs() > max_val {
                max_val = a[row][col].abs();
                max_row = row;
            }
        }

        if max_val < 1e-12 {
            return Err(EnerOSError::PowerFlow(format!(
                "Singular matrix at column {}",
                col
            )));
        }

        if max_row != col {
            a.swap(col, max_row);
            b.swap(col, max_row);
        }

        for row in (col + 1)..n {
            let factor = a[row][col] / a[col][col];
            for k in col..n {
                a[row][k] -= factor * a[col][k];
            }
            b[row] -= factor * b[col];
        }
    }

    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        x[i] = b[i];
        for j in (i + 1)..n {
            x[i] -= a[i][j] * x[j];
        }
        x[i] /= a[i][i];
    }

    Ok(x)
}

/// Bus type for Newton-Raphson solver
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusTypeNR {
    PQ,
    PV,
    Slack,
}

impl Default for PowerFlowSolver {
    fn default() -> Self {
        Self::default_solver()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matrix::YBusMatrix;
    use std::collections::HashMap;

    fn create_two_bus_system() -> (YBusMatrix, Vec<f64>, Vec<f64>, Vec<BusTypeNR>) {
        let mut bus_map = HashMap::new();
        bus_map.insert(0, 0);
        bus_map.insert(1, 1);

        let branches = vec![
            (0u64, 1u64, 0.01, 0.1, 0.0),
        ];

        let ybus = YBusMatrix::from_branches(&branches, &bus_map);

        let p_spec = vec![0.0, -0.5];
        let q_spec = vec![0.0, -0.2];
        let bus_types = vec![BusTypeNR::Slack, BusTypeNR::PQ];

        (ybus, p_spec, q_spec, bus_types)
    }

    fn create_three_bus_system() -> (YBusMatrix, Vec<f64>, Vec<f64>, Vec<BusTypeNR>) {
        let mut bus_map = HashMap::new();
        bus_map.insert(0, 0);
        bus_map.insert(1, 1);
        bus_map.insert(2, 2);

        let branches = vec![
            (0u64, 1u64, 0.01, 0.1, 0.0),
            (1u64, 2u64, 0.015, 0.15, 0.0),
            (0u64, 2u64, 0.02, 0.2, 0.0),
        ];

        let ybus = YBusMatrix::from_branches(&branches, &bus_map);

        let p_spec = vec![0.0, 0.5, -1.0];
        let q_spec = vec![0.0, 0.2, -0.5];
        let bus_types = vec![BusTypeNR::Slack, BusTypeNR::PV, BusTypeNR::PQ];

        (ybus, p_spec, q_spec, bus_types)
    }

    #[test]
    fn test_two_bus_convergence() {
        let (ybus, p_spec, q_spec, bus_types) = create_two_bus_system();
        let solver = PowerFlowSolver::default_solver();

        let result = solver.solve(&ybus, &p_spec, &q_spec, &bus_types);

        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(result.converged);
        assert!(result.iterations > 0);
        assert!(result.iterations <= 50);

        assert!((result.bus_results[0].voltage_magnitude - 1.0).abs() < 0.01);
        assert!(result.bus_results[1].voltage_magnitude > 0.9);
        assert!(result.bus_results[1].voltage_magnitude < 1.1);
    }

    #[test]
    fn test_three_bus_convergence() {
        let (ybus, p_spec, q_spec, bus_types) = create_three_bus_system();
        let solver = PowerFlowSolver::default_solver();

        let result = solver.solve(&ybus, &p_spec, &q_spec, &bus_types);

        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(result.converged);

        assert!((result.bus_results[0].voltage_magnitude - 1.0).abs() < 0.01);
        assert!(result.bus_results[1].voltage_magnitude > 0.9);
        assert!(result.bus_results[1].voltage_magnitude < 1.1);
        assert!(result.bus_results[2].voltage_magnitude > 0.9);
        assert!(result.bus_results[2].voltage_magnitude < 1.1);
    }

    #[test]
    fn test_gaussian_elimination() {
        let matrix = vec![
            vec![2.0, 1.0],
            vec![1.0, 3.0],
        ];
        let rhs = vec![5.0, 7.0];

        let result = gaussian_elimination(&matrix, &rhs);
        assert!(result.is_ok());

        let x = result.unwrap();
        assert!((x[0] - 1.6).abs() < 1e-10);
        assert!((x[1] - 1.8).abs() < 1e-10);
    }

    #[test]
    fn test_gaussian_elimination_singular() {
        let matrix = vec![
            vec![1.0, 2.0],
            vec![2.0, 4.0],
        ];
        let rhs = vec![5.0, 10.0];

        let result = gaussian_elimination(&matrix, &rhs);
        assert!(result.is_err());
    }

    #[test]
    fn test_branch_flows() {
        let (ybus, p_spec, q_spec, bus_types) = create_two_bus_system();
        let solver = PowerFlowSolver::default_solver();

        let result = solver.solve(&ybus, &p_spec, &q_spec, &bus_types).unwrap();

        assert!(!result.branch_results.is_empty());
        assert!(result.branch_results[0].from_bus == 0);
        assert!(result.branch_results[0].to_bus == 1);
    }

    #[test]
    fn test_losses_positive() {
        let (ybus, p_spec, q_spec, bus_types) = create_two_bus_system();
        let solver = PowerFlowSolver::default_solver();

        let result = solver.solve(&ybus, &p_spec, &q_spec, &bus_types).unwrap();

        assert!(result.total_losses >= 0.0);
    }
}
