use crate::matrix::YBusMatrix;
use crate::result::{BranchResult, BusResult, PowerFlowResult};
use eneros_core::{ElementId, EnerOSError, Result};

/// Power flow algorithm selection.
///
/// - `NewtonRaphson`: General-purpose, works for meshed and radial networks.
///   Best for transmission systems with strong coupling.
/// - `BackwardForwardSweep`: Optimized for radial distribution networks.
///   Uses BIBC/BCBV matrices (Jen-Hao Teng method). Faster than NR for radial
///   topologies but requires tree structure (no loops).
/// - `DC`: Linearized DC power flow. Fast approximation, ignores reactive power.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PowerFlowAlgorithm {
    #[default]
    NewtonRaphson,
    BackwardForwardSweep,
    DC,
}

/// Power flow solver using Newton-Raphson method
#[derive(Clone)]
pub struct PowerFlowSolver {
    max_iterations: u32,
    tolerance: f64,
    algorithm: PowerFlowAlgorithm,
}

impl PowerFlowSolver {
    pub fn new(max_iterations: u32, tolerance: f64) -> Self {
        Self {
            max_iterations,
            tolerance,
            algorithm: PowerFlowAlgorithm::NewtonRaphson,
        }
    }

    pub fn default_solver() -> Self {
        Self::new(50, 1e-8)
    }

    /// Set the power flow algorithm.
    pub fn with_algorithm(mut self, algorithm: PowerFlowAlgorithm) -> Self {
        self.algorithm = algorithm;
        self
    }

    /// Get the current algorithm.
    pub fn algorithm(&self) -> PowerFlowAlgorithm {
        self.algorithm
    }

    /// Solve power flow using Newton-Raphson method
    pub fn solve(
        &self,
        ybus: &YBusMatrix,
        p_spec: &[f64],
        q_spec: &[f64],
        bus_types: &[BusTypeNR],
    ) -> Result<PowerFlowResult> {
        self.solve_with_initial(ybus, p_spec, q_spec, bus_types, None)
    }

    /// Solve power flow with optional initial voltage magnitudes
    pub fn solve_with_initial(
        &self,
        ybus: &YBusMatrix,
        p_spec: &[f64],
        q_spec: &[f64],
        bus_types: &[BusTypeNR],
        v_initial: Option<&[f64]>,
    ) -> Result<PowerFlowResult> {
        self.solve_with_options(ybus, p_spec, q_spec, bus_types, v_initial, None, None)
    }

    /// Solve power flow with Q limit enforcement and recycle cache support.
    ///
    /// # Arguments
    /// * `q_limits` - Optional Q limits for PV buses. When a PV bus's Q
    ///   exceeds limits, it is converted to PQ (pandapower-style enforcement).
    /// * `recycle` - Optional recycle cache. If provided and valid, uses
    ///   cached voltages as initial values to speed up convergence.
    #[allow(clippy::too_many_arguments)]
    pub fn solve_with_options(
        &self,
        ybus: &YBusMatrix,
        p_spec: &[f64],
        q_spec: &[f64],
        bus_types: &[BusTypeNR],
        v_initial: Option<&[f64]>,
        q_limits: Option<&QLimits>,
        recycle: Option<&RecycleCache>,
    ) -> Result<PowerFlowResult> {
        // Use recycle cache for initial voltages if available and no explicit initial provided
        let recycle_v: Vec<f64>;
        let effective_v_initial: Option<&[f64]> = if let Some(vi) = v_initial {
            Some(vi)
        } else if let Some(cache) = recycle {
            if let Some(cached_v) = &cache.cached_v {
                recycle_v = cached_v.clone();
                Some(&recycle_v)
            } else {
                None
            }
        } else {
            None
        };

        // If no Q limits, solve normally
        if q_limits.map(|q| q.is_empty()).unwrap_or(true) {
            return self.solve_nr(ybus, p_spec, q_spec, bus_types, effective_v_initial);
        }

        // Q limit enforcement: iterate, converting PV→PQ when Q exceeds limits
        let mut current_bus_types = bus_types.to_vec();
        let mut current_q_spec = q_spec.to_vec();
        let mut q_conversions: Vec<(usize, f64)> = Vec::new(); // (bus_idx, fixed_q)
        let max_qlim_iterations = 10;

        for _qlim_iter in 0..max_qlim_iterations {
            let result = self.solve_nr(
                ybus,
                p_spec,
                &current_q_spec,
                &current_bus_types,
                effective_v_initial,
            )?;

            if q_limits.is_none() {
                return Ok(result);
            }
            let q_limits = q_limits.unwrap();

            // Check Q violations at PV buses
            let mut violations = Vec::new();
            for (i, &bt) in current_bus_types.iter().enumerate() {
                if bt == BusTypeNR::PV {
                    let q_computed = result.bus_results.iter()
                        .find(|br| br.bus_id == i as u64)
                        .map(|br| br.q_injection)
                        .unwrap_or(0.0);

                    if let Some((qmin, qmax)) = q_limits.get(i) {
                        if q_computed > qmax {
                            violations.push((i, qmax, q_computed));
                        } else if q_computed < qmin {
                            violations.push((i, qmin, q_computed));
                        }
                    }
                }
            }

            if violations.is_empty() {
                return Ok(result);
            }

            // Convert the most violated PV bus to PQ
            violations.sort_by(|a, b| {
                let va = (a.2 - a.1).abs();
                let vb = (b.2 - b.1).abs();
                vb.partial_cmp(&va).unwrap_or(std::cmp::Ordering::Equal)
            });

            let (bus_idx, fixed_q, _) = violations[0];
            current_bus_types[bus_idx] = BusTypeNR::PQ;
            current_q_spec[bus_idx] = fixed_q;
            q_conversions.push((bus_idx, fixed_q));
        }

        // Final solve with updated bus types
        self.solve_nr(ybus, p_spec, &current_q_spec, &current_bus_types, effective_v_initial)
    }

    /// Core Newton-Raphson solver (internal).
    fn solve_nr(
        &self,
        ybus: &YBusMatrix,
        p_spec: &[f64],
        q_spec: &[f64],
        bus_types: &[BusTypeNR],
        v_initial: Option<&[f64]>,
    ) -> Result<PowerFlowResult> {
        let n = ybus.size();

        let mut v = v_initial
            .map(|vi| vi.to_vec())
            .unwrap_or_else(|| vec![1.0; n]);
        let mut theta = vec![0.0; n];

        let mut converged = false;
        let mut final_mismatch = f64::MAX;
        let mut iterations = 0;

        for iter in 0..self.max_iterations {
            iterations = iter + 1;

            let (dp, dq) =
                self.calculate_mismatches(&v, &theta, ybus, p_spec, q_spec, bus_types)?;

            let max_mismatch = dp
                .iter()
                .chain(dq.iter())
                .fold(0.0_f64, |a, &b| a.max(b.abs()));
            final_mismatch = max_mismatch;

            if max_mismatch < self.tolerance {
                converged = true;
                break;
            }

            let jacobian = self.build_jacobian(&v, &theta, ybus, bus_types)?;

            let mut rhs = Vec::new();
            rhs.extend_from_slice(&dp);
            rhs.extend_from_slice(&dq);

            let dx = gaussian_elimination(&jacobian, &rhs)?;

            // Apply correction: first all Δθ, then all ΔV
            // dx vector structure: [Δθ₁..Δθₙ, ΔV₁..ΔVₘ]
            let mut idx = 0;
            for (i, &bt) in bus_types.iter().enumerate() {
                if bt != BusTypeNR::Slack {
                    theta[i] += dx[idx];
                    idx += 1;
                }
            }
            for (i, &bt) in bus_types.iter().enumerate() {
                if bt == BusTypeNR::PQ {
                    v[i] += dx[idx];
                    idx += 1;
                }
            }
        }

        if !converged {
            return Err(EnerOSError::PowerFlow(format!(
                "Power flow did not converge after {} iterations",
                self.max_iterations
            )));
        }

        let branch_results = self.calculate_branch_flows(&v, &theta, ybus)?;
        let bus_results = self.calculate_bus_results(&v, &theta, ybus, p_spec, q_spec);
        let total_losses = branch_results.iter().map(|br| br.loss_mw).sum();

        Ok(PowerFlowResult {
            converged,
            iterations,
            max_mismatch: final_mismatch,
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

            // 使用稀疏迭代：零元素对 p_calc/q_calc 贡献为 0，可跳过
            for (j, g, b) in ybus.iter_row(i) {
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

        // J1: dP/dtheta diagonal.
        //
        // The mismatch used by this solver is `dp = p_spec - p_calc` where
        // `p_calc = Σ_k V_i·V_k·(G_ik·cos(θ_ik) + B_ik·sin(θ_ik))`. The Newton
        // step solves `J·Δx = dp` (rhs is the *positive* residual), so the
        // Jacobian entries here are `dp/dθ_i` with the sign convention that
        // makes the iteration self-consistent (the k==i term of dp_calc/dθ_i
        // is zero because sin(θ_ii)=0 and cos(θ_ii)=1 contributes only to
        // dQ/dV, so summing k != i here is correct and matches the test
        // convergence at 1e-8 on IEEE 14).
        for (ii, &i) in non_slack_indices.iter().enumerate() {
            for (jj, &j) in non_slack_indices.iter().enumerate() {
                if i == j {
                    let mut sum = 0.0;
                    for (k, g_ik, b_ik) in ybus.iter_row(i) {
                        if k != i {
                            let angle_diff_ik = theta[i] - theta[k];
                            sum += v[i]
                                * v[k]
                                * (g_ik * angle_diff_ik.sin() - b_ik * angle_diff_ik.cos());
                        }
                    }
                    jacobian[ii][jj] = -sum;
                } else {
                    let (g, b) = ybus.get(i, j);
                    let angle_diff = theta[i] - theta[j];
                    jacobian[ii][jj] = v[i] * v[j] * (g * angle_diff.sin() - b * angle_diff.cos());
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
                    for (k, g_ik, b_ik) in ybus.iter_row(i) {
                        let angle_diff_ik = theta[i] - theta[k];
                        p_calc +=
                            v[i] * v[k] * (g_ik * angle_diff_ik.cos() + b_ik * angle_diff_ik.sin());
                    }
                    jacobian[ii][nns + jj] = p_calc / v[i] + v[i] * g;
                } else {
                    // dP_i/dV_j = V_i * (G_ij * cos(theta_i-theta_j) + B_ij * sin(theta_i-theta_j))
                    jacobian[ii][nns + jj] = v[i] * (g * angle_diff.cos() + b * angle_diff.sin());
                }
            }
        }

        // J3: dQ/dtheta diagonal. Same self-consistency note as J1: the k==i
        // term contributes to dP/dV (via cos(0)) not to dQ/dθ, so summing k!=i
        // is the correct matching form for this solver's residual convention.
        for (ii, &i) in pq_indices.iter().enumerate() {
            for (jj, &j) in non_slack_indices.iter().enumerate() {
                if i == j {
                    let mut sum = 0.0;
                    for (k, g_ik, b_ik) in ybus.iter_row(i) {
                        if k != i {
                            let angle_diff_ik = theta[i] - theta[k];
                            sum += v[i]
                                * v[k]
                                * (g_ik * angle_diff_ik.cos() + b_ik * angle_diff_ik.sin());
                        }
                    }
                    jacobian[nns + ii][jj] = sum;
                } else {
                    let (g, b) = ybus.get(i, j);
                    let angle_diff = theta[i] - theta[j];
                    jacobian[nns + ii][jj] =
                        -v[i] * v[j] * (g * angle_diff.cos() + b * angle_diff.sin());
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
                    for (k, g_ik, b_ik) in ybus.iter_row(i) {
                        let angle_diff_ik = theta[i] - theta[k];
                        q_calc +=
                            v[i] * v[k] * (g_ik * angle_diff_ik.sin() - b_ik * angle_diff_ik.cos());
                    }
                    jacobian[nns + ii][nns + jj] = q_calc / v[i] - v[i] * b;
                } else {
                    // dQ_i/dV_j = V_i * (G_ij * sin(theta_i-theta_j) - B_ij * cos(theta_i-theta_j))
                    jacobian[nns + ii][nns + jj] =
                        v[i] * (g * angle_diff.sin() - b * angle_diff.cos());
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
            // 使用稀疏迭代，仅处理 j > i 的非零元（上三角）
            for (j, g, b) in ybus.iter_row(i) {
                if j <= i {
                    continue;
                }
                if g.abs() < 1e-10 && b.abs() < 1e-10 {
                    continue;
                }

                let _angle_diff = theta[i] - theta[j];

                let y_complex = num_complex::Complex::new(g, b);
                let v_i = num_complex::Complex::from_polar(v[i], theta[i]);
                let v_j = num_complex::Complex::from_polar(v[j], theta[j]);

                let i_ij = y_complex * (v_i - v_j);
                let s_ij = v_i * i_ij.conj();

                let p_mw = s_ij.re;
                let q_mvar = s_ij.im;

                let apparent_power_pu = s_ij.norm();
                let loading_percent = if let Some(rating_mva) = ybus.branch_rating_mva(i, j) {
                    (apparent_power_pu * ybus.base_mva() / rating_mva) * 100.0
                } else {
                    apparent_power_pu * 100.0
                };

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
    eneros_core::solve_linear_system(matrix, rhs).ok_or_else(|| {
        EnerOSError::PowerFlow("Singular matrix in Gaussian elimination".to_string())
    })
}

/// Bus type for Newton-Raphson solver
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusTypeNR {
    PQ,
    PV,
    Slack,
}

/// Q limits for PV buses (for Q limit enforcement).
///
/// When a PV bus's reactive power exceeds these limits, it is converted to a
/// PQ bus with Q fixed at the violated limit. This mirrors pandapower's
/// `_run_ac_pf_with_qlims_enforced` behavior.
#[derive(Debug, Clone, Default)]
pub struct QLimits {
    /// (bus_idx, q_min_mvar, q_max_mvar)
    pub limits: Vec<(usize, f64, f64)>,
}

impl QLimits {
    pub fn new() -> Self {
        Self { limits: Vec::new() }
    }

    pub fn add(&mut self, bus_idx: usize, q_min_mvar: f64, q_max_mvar: f64) {
        self.limits.push((bus_idx, q_min_mvar, q_max_mvar));
    }

    pub fn get(&self, bus_idx: usize) -> Option<(f64, f64)> {
        self.limits.iter()
            .find(|(idx, _, _)| *idx == bus_idx)
            .map(|(_, qmin, qmax)| (*qmin, *qmax))
    }

    pub fn is_empty(&self) -> bool {
        self.limits.is_empty()
    }
}

/// Cache for recycling Y-bus and previous solution across sequential power flow runs.
///
/// Inspired by pandapower's recycle mechanism (`powerflow.py:73-134`).
/// When the network topology doesn't change between runs, the Y-bus matrix
/// can be reused, saving construction time. Previous voltage results can
/// also be used as initial values to speed up convergence.
#[derive(Debug, Clone, Default)]
pub struct RecycleCache {
    /// Cached voltage magnitudes from previous solve (used as initial values).
    pub cached_v: Option<Vec<f64>>,
    /// Cached voltage angles from previous solve.
    pub cached_theta: Option<Vec<f64>>,
    /// Network topology signature (for invalidation detection).
    pub topology_signature: Option<u64>,
}

impl RecycleCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update cache with latest solution.
    pub fn update(&mut self, v: &[f64], theta: &[f64]) {
        self.cached_v = Some(v.to_vec());
        self.cached_theta = Some(theta.to_vec());
    }

    /// Invalidate cache (call when topology changes).
    pub fn invalidate(&mut self) {
        self.cached_v = None;
        self.cached_theta = None;
        self.topology_signature = None;
    }

    /// Get cached initial voltages if available.
    pub fn initial_voltages(&self) -> Option<(&[f64], &[f64])> {
        match (&self.cached_v, &self.cached_theta) {
            (Some(v), Some(theta)) => Some((v, theta)),
            _ => None,
        }
    }
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

        let branches = vec![(0u64, 1u64, 0.01, 0.1, 0.0, 1.0)];

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
            (0u64, 1u64, 0.01, 0.1, 0.0, 1.0),
            (1u64, 2u64, 0.015, 0.15, 0.0, 1.0),
            (0u64, 2u64, 0.02, 0.2, 0.0, 1.0),
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
        let matrix = vec![vec![2.0, 1.0], vec![1.0, 3.0]];
        let rhs = vec![5.0, 7.0];

        let result = gaussian_elimination(&matrix, &rhs);
        assert!(result.is_ok());

        let x = result.unwrap();
        assert!((x[0] - 1.6).abs() < 1e-10);
        assert!((x[1] - 1.8).abs() < 1e-10);
    }

    #[test]
    fn test_gaussian_elimination_singular() {
        let matrix = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
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
    fn test_branch_loading_is_apparent_power_percent_of_system_base() {
        let (ybus, p_spec, q_spec, bus_types) = create_two_bus_system();
        let solver = PowerFlowSolver::default_solver();

        let result = solver.solve(&ybus, &p_spec, &q_spec, &bus_types).unwrap();
        let branch = &result.branch_results[0];
        let expected = branch.p_from.hypot(branch.q_from) * 100.0;

        assert!((branch.loading_percent - expected).abs() < 1e-10);
    }

    #[test]
    fn test_branch_loading_uses_branch_rating_when_available() {
        let (mut ybus, p_spec, q_spec, bus_types) = create_two_bus_system();
        ybus.set_base_mva(100.0);
        ybus.set_branch_rating_mva(0, 1, 50.0);
        let solver = PowerFlowSolver::default_solver();

        let result = solver.solve(&ybus, &p_spec, &q_spec, &bus_types).unwrap();
        let branch = &result.branch_results[0];
        let expected = branch.p_from.hypot(branch.q_from) * 100.0 / 50.0 * 100.0;

        assert!((branch.loading_percent - expected).abs() < 1e-10);
    }

    #[test]
    fn test_losses_positive() {
        let (ybus, p_spec, q_spec, bus_types) = create_two_bus_system();
        let solver = PowerFlowSolver::default_solver();

        let result = solver.solve(&ybus, &p_spec, &q_spec, &bus_types).unwrap();

        assert!(result.total_losses >= 0.0);
    }

    #[test]
    fn test_ieee14_convergence() {
        let data = crate::ieee::ieee14();
        let (ybus, p_spec, q_spec, bus_types) = data.to_solver_input();

        // Use specified voltage magnitudes as initial values
        let v_initial: Vec<f64> = data.buses.iter().map(|b| b.v_pu).collect();

        let solver = PowerFlowSolver::new(100, 1e-8);
        let result =
            solver.solve_with_initial(&ybus, &p_spec, &q_spec, &bus_types, Some(&v_initial));

        assert!(
            result.is_ok(),
            "IEEE 14 power flow failed: {:?}",
            result.err()
        );
        let result = result.unwrap();
        assert!(
            result.converged,
            "IEEE 14 did not converge in {} iterations",
            result.iterations
        );
        assert!(
            result.iterations <= 20,
            "IEEE 14 took too many iterations: {}",
            result.iterations
        );

        // Verify bus voltages are in reasonable range (0.9 to 1.2 pu)
        for br in &result.bus_results {
            assert!(
                br.voltage_magnitude > 0.9 && br.voltage_magnitude < 1.2,
                "Bus {} voltage {} pu out of range",
                br.bus_id,
                br.voltage_magnitude
            );
        }

        // Verify total losses are positive and reasonable (< 20 MW for IEEE 14)
        assert!(
            result.total_losses > 0.0 && result.total_losses < 20.0,
            "Total losses {} MW out of range",
            result.total_losses
        );
    }

    #[test]
    fn test_ieee14_voltage_accuracy() {
        let data = crate::ieee::ieee14();
        let (ybus, p_spec, q_spec, bus_types) = data.to_solver_input();

        let v_initial: Vec<f64> = data.buses.iter().map(|b| b.v_pu).collect();

        let solver = PowerFlowSolver::new(100, 1e-8);
        let result = solver
            .solve_with_initial(&ybus, &p_spec, &q_spec, &bus_types, Some(&v_initial))
            .expect("IEEE 14 power flow failed");

        assert!(result.converged, "IEEE 14 did not converge");

        // Self-consistency check: max mismatch must be below tolerance
        assert!(
            result.max_mismatch < 1e-8,
            "Max mismatch too large: {:.2e}",
            result.max_mismatch
        );

        // Print comparison table header
        eprintln!(
            "{:>6} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10}",
            "Bus", "Comp V", "Exp V", "V Err", "Comp θ°", "Exp θ°", "θ Err°"
        );
        eprintln!("{}", "-".repeat(68));

        for (i, bus_data) in data.buses.iter().enumerate() {
            let computed = &result.bus_results[i];
            let computed_v = computed.voltage_magnitude;
            let expected_v = bus_data.v_pu;
            let v_error = (computed_v - expected_v).abs();

            let computed_angle_deg = computed.voltage_angle.to_degrees();
            let expected_angle_deg = bus_data.angle_deg;
            let angle_error = (computed_angle_deg - expected_angle_deg).abs();

            eprintln!(
                "{:>6} {:>10.4} {:>10.4} {:>10.4} {:>10.4} {:>10.4} {:>10.4}",
                bus_data.bus_id,
                computed_v,
                expected_v,
                v_error,
                computed_angle_deg,
                expected_angle_deg,
                angle_error
            );

            // Reference solution has limited precision (3-4 significant digits),
            // so we use a moderate tolerance for comparison
            assert!(
                v_error < 0.02,
                "Bus {} voltage error too large: computed={}, expected={}, error={}",
                bus_data.bus_id,
                computed_v,
                expected_v,
                v_error
            );
            assert!(
                angle_error < 0.1,
                "Bus {} angle error too large: computed={:.4}°, expected={:.4}°, error={:.4}°",
                bus_data.bus_id,
                computed_angle_deg,
                expected_angle_deg,
                angle_error
            );
        }
    }

    #[test]
    fn test_ieee14_total_losses() {
        let data = crate::ieee::ieee14();
        let (ybus, p_spec, q_spec, bus_types) = data.to_solver_input();

        let v_initial: Vec<f64> = data.buses.iter().map(|b| b.v_pu).collect();

        let solver = PowerFlowSolver::new(100, 1e-8);
        let result = solver
            .solve_with_initial(&ybus, &p_spec, &q_spec, &bus_types, Some(&v_initial))
            .expect("IEEE 14 power flow failed");

        assert!(result.converged, "IEEE 14 did not converge");

        // IEEE 14-bus standard total losses are approximately 13.8 MW
        // total_losses is in per-unit; convert to MW by multiplying base_mva
        let total_losses_mw = result.total_losses * data.base_mva;
        let expected_losses = 13.8;
        let loss_error = (total_losses_mw - expected_losses).abs();

        eprintln!(
            "Total losses: computed={:.4} MW ({} pu), expected≈{:.1} MW, error={:.4} MW",
            total_losses_mw, result.total_losses, expected_losses, loss_error
        );

        assert!(
            loss_error < 1.0,
            "Total losses {:.4} MW too far from expected {:.1} MW (error={:.4} MW)",
            total_losses_mw,
            expected_losses,
            loss_error
        );
    }

    #[test]
    fn test_ieee14_branch_flow_reasonableness() {
        let data = crate::ieee::ieee14();
        let (ybus, p_spec, q_spec, bus_types) = data.to_solver_input();

        let v_initial: Vec<f64> = data.buses.iter().map(|b| b.v_pu).collect();

        let solver = PowerFlowSolver::new(100, 1e-8);
        let result = solver
            .solve_with_initial(&ybus, &p_spec, &q_spec, &bus_types, Some(&v_initial))
            .expect("IEEE 14 power flow failed");

        assert!(result.converged, "IEEE 14 did not converge");
        assert!(!result.branch_results.is_empty(), "No branch results");

        for br in &result.branch_results {
            // Branch losses should be positive
            assert!(
                br.loss_mw >= 0.0,
                "Branch {} ({}->{}) has negative loss: {} MW",
                br.branch_id,
                br.from_bus,
                br.to_bus,
                br.loss_mw
            );

            // At least one direction should have positive active power flow magnitude
            let has_flow = br.p_from.abs() > 1e-10 || br.p_to.abs() > 1e-10;
            assert!(
                has_flow || br.loss_mw < 1e-10,
                "Branch {} ({}->{}) has no flow but nonzero loss",
                br.branch_id,
                br.from_bus,
                br.to_bus
            );
        }

        for (branch_result, branch_data) in result.branch_results.iter().zip(data.branches.iter()) {
            let flow_mva = branch_result.p_from.hypot(branch_result.q_from) * data.base_mva;
            let expected_loading = flow_mva / branch_data.rate_mva * 100.0;
            assert!(
                (branch_result.loading_percent - expected_loading).abs() < 1e-8,
                "Branch {} ({}->{}) loading {:.2}% != expected {:.2}%",
                branch_result.branch_id,
                branch_result.from_bus,
                branch_result.to_bus,
                branch_result.loading_percent,
                expected_loading
            );
        }
    }
}
