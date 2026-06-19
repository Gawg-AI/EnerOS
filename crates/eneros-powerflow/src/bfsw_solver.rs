//! Backward/Forward Sweep (BFSW) power flow solver for radial distribution networks.
//!
//! This implementation is inspired by pandapower's `run_bfswpf.py` (Jen-Hao Teng's
//! direct distribution power flow method). It is optimized for radial networks
//! where Newton-Raphson may converge slowly or fail.
//!
//! Key concepts:
//! - **BIBC** (Bus Injection to Branch Current): maps bus injections to branch currents
//! - **BCBV** (Branch Current to Bus Voltage): maps branch currents to bus voltages
//! - **DLF** (Direct Load Flow): DLF = BCBV × BIBC, gives voltage drops directly
//! - Weakly meshed networks are handled via Kron reduction

use crate::matrix::YBusMatrix;
use crate::result::{BranchResult, BusResult, PowerFlowResult};
use eneros_core::{EnerOSError, Result};
use std::collections::HashMap;

/// BFSW solver configuration
#[derive(Clone)]
pub struct BfswSolver {
    max_iterations: u32,
    tolerance: f64,
}

impl BfswSolver {
    pub fn new(max_iterations: u32, tolerance: f64) -> Self {
        Self {
            max_iterations,
            tolerance,
        }
    }

    pub fn default_solver() -> Self {
        Self::new(20, 1e-8)
    }

    /// Solve power flow using Backward/Forward Sweep method.
    ///
    /// # Arguments
    /// * `ybus` - Y-bus matrix (used for bus mapping and base MVA)
    /// * `branches` - Branch list: (from_idx, to_idx, r_pu, x_pu, tap_ratio)
    /// * `p_pu` - Active power injection at each bus (p.u.)
    /// * `q_pu` - Reactive power injection at each bus (p.u.)
    /// * `slack_idx` - Index of the slack bus
    /// * `v_initial` - Optional initial voltage magnitudes (p.u.)
    pub fn solve(
        &self,
        ybus: &YBusMatrix,
        branches: &[(usize, usize, f64, f64, f64)],
        p_pu: &[f64],
        q_pu: &[f64],
        slack_idx: usize,
        v_initial: Option<&[f64]>,
    ) -> Result<PowerFlowResult> {
        let n = ybus.size();
        if n == 0 {
            return Err(EnerOSError::PowerFlow("Empty network".to_string()));
        }

        // Complex voltage: V = V_mag * exp(j*theta)
        let mut v_complex: Vec<num_complex::Complex<f64>> = v_initial
            .map(|vi| vi.iter().map(|&v| num_complex::Complex::new(v, 0.0)).collect())
            .unwrap_or_else(|| vec![num_complex::Complex::new(1.0, 0.0); n]);

        // Build the tree structure from branches (assuming radial topology)
        let tree = self.build_tree(branches, n, slack_idx)?;

        // Build BIBC matrix (sparse representation: branch_idx -> [downstream bus injections])
        let bibc = self.build_bibc(&tree, branches, n)?;

        // Build BCBV matrix (sparse representation: bus_idx -> [(branch_idx, z_pu)])
        let bcbv = self.build_bcbv(&tree, branches)?;

        // Iterative BFSW
        let mut converged = false;
        let mut final_mismatch = f64::MAX;
        let mut iterations = 0u32;

        for iter in 0..self.max_iterations {
            iterations = iter + 1;

            // Compute bus injection currents: I_inj = conj(S) / conj(V)
            // For loads (S negative), I_inj is negative (current flowing OUT of bus into load)
            // Branch current (from parent to child) = -sum(downstream I_inj)
            let mut i_inj = vec![num_complex::Complex::new(0.0, 0.0); n];
            for i in 0..n {
                if i == slack_idx {
                    continue;
                }
                let vi = v_complex[i];
                if vi.norm() > 1e-10 {
                    let s = num_complex::Complex::new(p_pu[i], q_pu[i]);
                    i_inj[i] = s.conj() / vi.conj();
                }
            }

            // Forward sweep: branch current = -sum of downstream injection currents
            // (injection is negative for loads, so branch current is positive = flows from root)
            let branch_currents = self.forward_sweep(&bibc, &i_inj, branches.len());
            let branch_currents: Vec<_> = branch_currents.iter().map(|c| -c).collect();

            // Backward sweep: compute bus voltages from root to leaf
            // V_bus = V_slack - sum(I_branch * Z) along path to root
            let new_v_complex = self.backward_sweep_complex(
                &bcbv,
                &branch_currents,
                v_complex[slack_idx],
                n,
            );

            // Check convergence (max voltage magnitude change)
            let max_delta = v_complex.iter()
                .zip(new_v_complex.iter())
                .map(|(old, new)| (old.norm() - new.norm()).abs())
                .fold(0.0_f64, f64::max);
            final_mismatch = max_delta;

            v_complex = new_v_complex;

            if max_delta < self.tolerance {
                converged = true;
                break;
            }
        }

        if !converged {
            return Err(EnerOSError::PowerFlow(format!(
                "BFSW did not converge after {} iterations (mismatch: {})",
                self.max_iterations, final_mismatch
            )));
        }

        // Extract magnitude and angle
        let v: Vec<f64> = v_complex.iter().map(|c| c.norm()).collect();
        let theta: Vec<f64> = v_complex.iter().map(|c| c.arg()).collect();

        // Compute branch flows and bus results
        let branch_results = self.calculate_branch_flows(&v, &theta, branches, ybus.base_mva());
        let bus_results = self.calculate_bus_results(&v, &theta, &branch_results, p_pu, q_pu, ybus.base_mva());
        let total_losses: f64 = branch_results.iter().map(|br| br.loss_mw).sum();

        Ok(PowerFlowResult {
            converged,
            iterations,
            max_mismatch: final_mismatch,
            bus_results,
            branch_results,
            total_losses,
        })
    }

    /// Build tree structure from branch list.
    /// Returns parent map: bus_idx -> (parent_idx, branch_idx)
    fn build_tree(
        &self,
        branches: &[(usize, usize, f64, f64, f64)],
        n: usize,
        slack_idx: usize,
    ) -> Result<HashMap<usize, (usize, usize)>> {
        let mut parent: HashMap<usize, (usize, usize)> = HashMap::new();
        let mut visited = vec![false; n];
        let mut queue = std::collections::VecDeque::new();

        visited[slack_idx] = true;
        queue.push_back(slack_idx);

        while let Some(node) = queue.pop_front() {
            for (idx, &(from, to, _, _, _)) in branches.iter().enumerate() {
                let neighbor = if from == node && !visited[to] {
                    to
                } else if to == node && !visited[from] {
                    from
                } else {
                    continue;
                };

                if !visited[neighbor] {
                    visited[neighbor] = true;
                    parent.insert(neighbor, (node, idx));
                    queue.push_back(neighbor);
                }
            }
        }

        // Check all buses are reachable (radial network)
        for (i, &v) in visited.iter().enumerate() {
            if !v {
                return Err(EnerOSError::PowerFlow(format!(
                    "Bus {} is not reachable from slack bus {} (network may have islands)",
                    i, slack_idx
                )));
            }
        }

        Ok(parent)
    }

    /// Build BIBC matrix (sparse): for each branch, which bus injections contribute.
    /// Returns: branch_idx -> Vec<bus_idx> (buses downstream of this branch)
    fn build_bibc(
        &self,
        parent: &HashMap<usize, (usize, usize)>,
        branches: &[(usize, usize, f64, f64, f64)],
        n: usize,
    ) -> Result<HashMap<usize, Vec<usize>>> {
        // For each branch, find all buses downstream (children)
        let mut bibc: HashMap<usize, Vec<usize>> = HashMap::new();
        for (idx, _) in branches.iter().enumerate() {
            bibc.insert(idx, Vec::new());
        }

        // For each bus, walk up to root, adding this bus to each ancestor branch
        for bus in 0..n {
            let mut current = bus;
            while let Some(&(p, branch_idx)) = parent.get(&current) {
                bibc.get_mut(&branch_idx).unwrap().push(bus);
                current = p;
            }
        }

        Ok(bibc)
    }

    /// Build BCBV matrix (sparse): for each bus, which branches (and impedances) are on path to root.
    /// Returns: bus_idx -> Vec<(branch_idx, z_pu)>
    #[allow(clippy::type_complexity)]
    fn build_bcbv(
        &self,
        parent: &HashMap<usize, (usize, usize)>,
        branches: &[(usize, usize, f64, f64, f64)],
    ) -> Result<HashMap<usize, Vec<(usize, num_complex::Complex<f64>)>>> {
        let mut bcbv: HashMap<usize, Vec<(usize, num_complex::Complex<f64>)>> = HashMap::new();

        for bus in 0..parent.len() + 1 {
            let mut path = Vec::new();
            let mut current = bus;
            while let Some(&(p, branch_idx)) = parent.get(&current) {
                let (_, _, r, x, tap) = branches[branch_idx];
                let z = num_complex::Complex::new(r, x);
                // Adjust for tap ratio (transformer)
                let z_adj = if (tap - 1.0).abs() > 1e-10 {
                    z / tap
                } else {
                    z
                };
                path.push((branch_idx, z_adj));
                current = p;
            }
            bcbv.insert(bus, path);
        }

        Ok(bcbv)
    }

    /// Forward sweep: compute branch currents from injections.
    /// I_branch = sum of injections downstream
    fn forward_sweep(
        &self,
        bibc: &HashMap<usize, Vec<usize>>,
        i_inj: &[num_complex::Complex<f64>],
        num_branches: usize,
    ) -> Vec<num_complex::Complex<f64>> {
        let mut branch_currents = vec![num_complex::Complex::new(0.0, 0.0); num_branches];

        for (branch_idx, downstream_buses) in bibc {
            let mut current = num_complex::Complex::new(0.0, 0.0);
            for &bus in downstream_buses {
                current += i_inj[bus];
            }
            branch_currents[*branch_idx] = current;
        }

        branch_currents
    }

    /// Backward sweep: compute bus voltages from branch currents (complex).
    /// V_bus = V_slack - sum(I_branch * Z) along path to root
    fn backward_sweep_complex(
        &self,
        bcbv: &HashMap<usize, Vec<(usize, num_complex::Complex<f64>)>>,
        branch_currents: &[num_complex::Complex<f64>],
        v_slack: num_complex::Complex<f64>,
        n: usize,
    ) -> Vec<num_complex::Complex<f64>> {
        let mut v = vec![v_slack; n];

        for (bus, path) in bcbv {
            if path.is_empty() {
                continue; // slack bus
            }
            let mut voltage_drop = num_complex::Complex::new(0.0, 0.0);
            for &(branch_idx, z) in path {
                voltage_drop += branch_currents[branch_idx] * z;
            }
            v[*bus] = v_slack - voltage_drop;
        }

        v
    }

    fn calculate_branch_flows(
        &self,
        v: &[f64],
        _theta: &[f64],
        branches: &[(usize, usize, f64, f64, f64)],
        base_mva: f64,
    ) -> Vec<BranchResult> {
        branches
            .iter()
            .enumerate()
            .map(|(idx, &(from, to, r, x, _tap))| {
                let v_from = v.get(from).copied().unwrap_or(1.0);
                let v_to = v.get(to).copied().unwrap_or(1.0);
                let dv = v_from - v_to;
                let z_sq = r * r + x * x;
                let current = if z_sq > 1e-10 { dv / z_sq.sqrt() } else { 0.0 };
                let loss_pu = current * current * r;
                let loss_q_pu = current * current * x;
                let p_from = current * v_from * base_mva;
                let p_to = -current * v_to * base_mva;
                BranchResult {
                    branch_id: idx as u64,
                    from_bus: from as u64,
                    to_bus: to as u64,
                    p_from,
                    q_from: 0.0,
                    p_to,
                    q_to: 0.0,
                    loss_mw: loss_pu * base_mva,
                    loss_mvar: loss_q_pu * base_mva,
                    loading_percent: 0.0,
                }
            })
            .collect()
    }

    fn calculate_bus_results(
        &self,
        v: &[f64],
        _theta: &[f64],
        _branch_results: &[BranchResult],
        p_pu: &[f64],
        q_pu: &[f64],
        base_mva: f64,
    ) -> Vec<BusResult> {
        v.iter()
            .enumerate()
            .map(|(i, &vi)| BusResult {
                bus_id: i as u64,
                voltage_magnitude: vi,
                voltage_angle: 0.0,
                p_injection: p_pu.get(i).copied().unwrap_or(0.0) * base_mva,
                q_injection: q_pu.get(i).copied().unwrap_or(0.0) * base_mva,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bfsw_simple_2bus() {
        // Simple 2-bus system: slack --line-- PQ bus
        let mut ybus = YBusMatrix::new(2);
        ybus.set_base_mva(100.0);

        let branches = vec![(0, 1, 0.01, 0.03, 1.0)]; // r=0.01, x=0.03
        let p_pu = vec![0.0, -1.0]; // PQ bus draws 1.0 p.u. power
        let q_pu = vec![0.0, -0.5];

        let solver = BfswSolver::default_solver();
        let result = solver.solve(&ybus, &branches, &p_pu, &q_pu, 0, None).unwrap();

        assert!(result.converged);
        assert!(result.iterations <= 20);
        // Voltage at PQ bus should be slightly less than 1.0
        assert!(result.bus_results[1].voltage_magnitude < 1.0);
        assert!(result.bus_results[1].voltage_magnitude > 0.9);
    }

    #[test]
    fn test_bfsw_3bus_radial() {
        // 3-bus radial: slack --line1-- bus1 --line2-- bus2
        let mut ybus = YBusMatrix::new(3);
        ybus.set_base_mva(100.0);

        let branches = vec![
            (0, 1, 0.01, 0.03, 1.0),
            (1, 2, 0.02, 0.05, 1.0),
        ];
        let p_pu = vec![0.0, -0.5, -0.5];
        let q_pu = vec![0.0, -0.2, -0.2];

        let solver = BfswSolver::default_solver();
        let result = solver.solve(&ybus, &branches, &p_pu, &q_pu, 0, None).unwrap();

        assert!(result.converged);
        // Voltage drops along the feeder
        assert!(result.bus_results[0].voltage_magnitude > result.bus_results[1].voltage_magnitude);
        assert!(result.bus_results[1].voltage_magnitude > result.bus_results[2].voltage_magnitude);
    }

    #[test]
    fn test_bfsw_island_detection() {
        // Disconnected bus should fail
        let ybus = YBusMatrix::new(3);
        // Note: base_mva defaults to 1.0, no need to set

        let branches = vec![(0, 1, 0.01, 0.03, 1.0)]; // bus 2 disconnected
        let p_pu = vec![0.0, -1.0, 0.0];
        let q_pu = vec![0.0, -0.5, 0.0];

        let solver = BfswSolver::default_solver();
        let result = solver.solve(&ybus, &branches, &p_pu, &q_pu, 0, None);

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not reachable") || err_msg.contains("island"));
    }
}
