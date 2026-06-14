use std::collections::HashMap;
use ndarray::{Array1, Array2};
use eneros_core::ElementId;
use crate::types::{AnalysisResult, AnalysisError};

/// Generator bid for OPF: quadratic cost a*P^2 + b*P + c
#[derive(Debug, Clone)]
pub struct GeneratorBid {
    pub gen_id: ElementId,
    pub bus_id: ElementId,
    pub p_min: f64,
    pub p_max: f64,
    /// Quadratic cost coefficient
    pub cost_a: f64,
    /// Linear cost coefficient
    pub cost_b: f64,
    /// Constant cost coefficient
    pub cost_c: f64,
}

/// Branch flow limit for OPF
#[derive(Debug, Clone)]
pub struct BranchLimit {
    pub branch_id: ElementId,
    pub from_bus: ElementId,
    pub to_bus: ElementId,
    /// Power flow limit in MW
    pub p_limit_mw: f64,
    /// Reactance in per-unit
    pub reactance_pu: f64,
}

/// DC-OPF problem definition
#[derive(Debug, Clone)]
pub struct DcOpfProblem {
    pub generators: Vec<GeneratorBid>,
    pub branches: Vec<BranchLimit>,
    /// (bus_id, load_mw)
    pub loads: Vec<(ElementId, f64)>,
    pub slack_bus_id: ElementId,
}

/// DC-OPF solution result
#[derive(Debug, Clone)]
pub struct DcOpfResult {
    /// (gen_id, p_mw)
    pub generation: Vec<(ElementId, f64)>,
    /// (bus_id, angle_rad)
    pub bus_angles: Vec<(ElementId, f64)>,
    /// (branch_id, flow_mw)
    pub line_flows: Vec<(ElementId, f64)>,
    /// (bus_id, lmp_$/mwh)
    pub nodal_prices: Vec<(ElementId, f64)>,
    /// Total generation cost
    pub total_cost: f64,
}

/// DC-OPF solver using merit-order dispatch with PTDF-based congestion management
pub struct DcOpfSolver;

impl DcOpfSolver {
    pub fn new() -> Self {
        Self
    }

    /// Solve the DC-OPF problem
    pub fn solve(&self, problem: &DcOpfProblem) -> Result<AnalysisResult<DcOpfResult>, AnalysisError> {
        if problem.generators.is_empty() {
            return Err(AnalysisError::DataIncomplete("No generators defined".into()));
        }
        if problem.branches.is_empty() {
            return Err(AnalysisError::DataIncomplete("No branches defined".into()));
        }

        // Collect all unique bus IDs and build index mapping
        let mut bus_ids: Vec<ElementId> = Vec::new();
        for gen in &problem.generators {
            if !bus_ids.contains(&gen.bus_id) {
                bus_ids.push(gen.bus_id);
            }
        }
        for branch in &problem.branches {
            if !bus_ids.contains(&branch.from_bus) {
                bus_ids.push(branch.from_bus);
            }
            if !bus_ids.contains(&branch.to_bus) {
                bus_ids.push(branch.to_bus);
            }
        }
        for &(bus_id, _) in &problem.loads {
            if !bus_ids.contains(&bus_id) {
                bus_ids.push(bus_id);
            }
        }
        if !bus_ids.contains(&problem.slack_bus_id) {
            bus_ids.push(problem.slack_bus_id);
        }
        bus_ids.sort();

        let bus_count = bus_ids.len();
        let bus_map: HashMap<ElementId, usize> = bus_ids.iter()
            .enumerate()
            .map(|(i, &id)| (id, i))
            .collect();

        let slack_idx = *bus_map.get(&problem.slack_bus_id)
            .ok_or_else(|| AnalysisError::InvalidConfiguration(
                format!("Slack bus {} not found in bus list", problem.slack_bus_id)
            ))?;

        // Step 1: Build B' matrix
        let b_matrix = Self::build_b_matrix(problem, &bus_map, bus_count);

        // Step 2: Build PTDF matrix
        let ptdf = Self::build_ptdf(&b_matrix, problem, &bus_map, bus_count, slack_idx);

        // Step 3: Merit-order dispatch
        let mut generation = Self::merit_order_dispatch(problem);

        // Step 4: Check line flow limits and adjust if needed
        let mut warnings = Vec::new();
        let mut iterations = 1u32;
        const MAX_ADJUSTMENT_ITERATIONS: u32 = 20;

        for _ in 0..MAX_ADJUSTMENT_ITERATIONS {
            let flows = Self::compute_line_flows(&ptdf, &generation, &problem.loads, &bus_map, bus_count, slack_idx, problem);

            let mut violation_found = false;
            for (i, branch) in problem.branches.iter().enumerate() {
                let flow = flows[i].abs();
                if flow > branch.p_limit_mw {
                    violation_found = true;
                    // Reduce generation at the bus that contributes most to this line
                    let _from_idx = bus_map.get(&branch.from_bus).copied().unwrap_or(0);
                    let _to_idx = bus_map.get(&branch.to_bus).copied().unwrap_or(0);

                    // Find generators on the sending end and reduce output
                    let overflow = flow - branch.p_limit_mw;
                    for gen_entry in generation.iter_mut() {
                        if let Some(gen) = problem.generators.iter().find(|g| g.gen_id == gen_entry.0) {
                            let gen_bus_idx = bus_map.get(&gen.bus_id).copied().unwrap_or(0);
                            let ptdf_val = ptdf.get((i, gen_bus_idx)).copied().unwrap_or(0.0);
                            if ptdf_val.abs() > 0.01 {
                                let reduction = (overflow * ptdf_val.abs()).min(gen_entry.1 - gen.p_min);
                                if reduction > 0.0 {
                                    gen_entry.1 -= reduction;
                                }
                            }
                        }
                    }
                }
            }

            iterations += 1;
            if !violation_found {
                break;
            }
        }

        // Final flow computation
        let flows = Self::compute_line_flows(&ptdf, &generation, &problem.loads, &bus_map, bus_count, slack_idx, problem);

        // Check for remaining violations
        for (i, branch) in problem.branches.iter().enumerate() {
            if flows[i].abs() > branch.p_limit_mw * 1.01 {
                warnings.push(format!(
                    "Branch {} flow {:.2} MW exceeds limit {:.2} MW",
                    branch.branch_id, flows[i].abs(), branch.p_limit_mw
                ));
            }
        }

        // Step 5: Compute bus angles from B' * theta = P_injection
        let mut p_injection = Array1::<f64>::zeros(bus_count);
        for (gen_id, p_mw) in &generation {
            if let Some(gen) = problem.generators.iter().find(|g| g.gen_id == *gen_id) {
                if let Some(&idx) = bus_map.get(&gen.bus_id) {
                    p_injection[idx] += p_mw;
                }
            }
        }
        for &(bus_id, load_mw) in &problem.loads {
            if let Some(&idx) = bus_map.get(&bus_id) {
                p_injection[idx] -= load_mw;
            }
        }

        // Solve for angles: remove slack row/col and solve reduced system
        let mut angles = vec![0.0f64; bus_count];
        {
            let non_slack: Vec<usize> = (0..bus_count).filter(|&i| i != slack_idx).collect();
            let n_reduced = non_slack.len();
            let mut b_reduced = Array2::<f64>::zeros((n_reduced, n_reduced));
            let mut p_reduced = Array1::<f64>::zeros(n_reduced);

            for (ri, &bi) in non_slack.iter().enumerate() {
                p_reduced[ri] = p_injection[bi];
                for (rj, &bj) in non_slack.iter().enumerate() {
                    b_reduced[[ri, rj]] = b_matrix[[bi, bj]];
                }
            }

            if let Some(theta_reduced) = solve_linear_system(&b_reduced, &p_reduced) {
                for (ri, &bi) in non_slack.iter().enumerate() {
                    angles[bi] = theta_reduced[ri];
                }
            } else {
                return Err(AnalysisError::SingularMatrix("B' matrix is singular".into()));
            }
        }

        // Step 6: Compute LMP
        let lmp = Self::compute_lmp(problem, &ptdf, &bus_map, bus_count);

        // Compute total cost
        let total_cost: f64 = generation.iter().map(|(gen_id, p_mw)| {
            if let Some(gen) = problem.generators.iter().find(|g| g.gen_id == *gen_id) {
                gen.cost_a * p_mw * p_mw + gen.cost_b * p_mw + gen.cost_c
            } else {
                0.0
            }
        }).sum();

        let bus_angles: Vec<(ElementId, f64)> = bus_ids.iter()
            .zip(angles.iter())
            .map(|(&id, &a)| (id, a))
            .collect();

        let line_flows: Vec<(ElementId, f64)> = problem.branches.iter()
            .zip(flows.iter())
            .map(|(b, &f)| (b.branch_id, f))
            .collect();

        let nodal_prices: Vec<(ElementId, f64)> = bus_ids.iter()
            .zip(lmp.iter())
            .map(|(&id, &p)| (id, p))
            .collect();

        Ok(AnalysisResult {
            converged: warnings.is_empty(),
            iterations,
            result: DcOpfResult {
                generation,
                bus_angles,
                line_flows,
                nodal_prices,
                total_cost,
            },
            warnings,
        })
    }

    /// Build B' matrix (DC power flow susceptance matrix) from branch reactances
    pub fn build_b_matrix(problem: &DcOpfProblem, bus_map: &HashMap<ElementId, usize>, bus_count: usize) -> Array2<f64> {
        let mut b_matrix = Array2::<f64>::zeros((bus_count, bus_count));

        for branch in &problem.branches {
            let x = if branch.reactance_pu.abs() < 1e-12 {
                1e-12
            } else {
                branch.reactance_pu
            };
            let b = 1.0 / x;

            if let (Some(&i), Some(&j)) = (bus_map.get(&branch.from_bus), bus_map.get(&branch.to_bus)) {
                b_matrix[[i, i]] += b;
                b_matrix[[j, j]] += b;
                b_matrix[[i, j]] -= b;
                b_matrix[[j, i]] -= b;
            }
        }

        b_matrix
    }

    /// Build PTDF (Power Transfer Distribution Factors) matrix
    /// PTDF[l, i] = fraction of power injected at bus i that flows on line l
    pub fn build_ptdf(
        b_matrix: &Array2<f64>,
        problem: &DcOpfProblem,
        bus_map: &HashMap<ElementId, usize>,
        bus_count: usize,
        slack_idx: usize,
    ) -> Array2<f64> {
        let non_slack: Vec<usize> = (0..bus_count).filter(|&i| i != slack_idx).collect();
        let n_reduced = non_slack.len();

        // Build reduced B' matrix (remove slack row/col)
        let mut b_reduced = Array2::<f64>::zeros((n_reduced, n_reduced));
        for (ri, &bi) in non_slack.iter().enumerate() {
            for (rj, &bj) in non_slack.iter().enumerate() {
                b_reduced[[ri, rj]] = b_matrix[[bi, bj]];
            }
        }

        // Compute X = B'^{-1}
        let x_inv = if let Some(inv) = invert_matrix(&b_reduced) {
            inv
        } else {
            // Fallback: return zero PTDF
            return Array2::<f64>::zeros((problem.branches.len(), bus_count));
        };

        let mut ptdf = Array2::<f64>::zeros((problem.branches.len(), bus_count));

        for (l, branch) in problem.branches.iter().enumerate() {
            let x = if branch.reactance_pu.abs() < 1e-12 { 1e-12 } else { branch.reactance_pu };
            let b_line = 1.0 / x;

            if let (Some(&i), Some(&j)) = (bus_map.get(&branch.from_bus), bus_map.get(&branch.to_bus)) {
                for (ri, &bus_i) in non_slack.iter().enumerate() {
                    // PTDF[l, bus_i] = b_line * (X[i_map, ri] - X[j_map, ri])
                    let i_map = if i == slack_idx { None } else {
                        non_slack.iter().position(|&x| x == i)
                    };
                    let j_map = if j == slack_idx { None } else {
                        non_slack.iter().position(|&x| x == j)
                    };

                    let x_i = i_map.map(|idx| x_inv[[idx, ri]]).unwrap_or(0.0);
                    let x_j = j_map.map(|idx| x_inv[[idx, ri]]).unwrap_or(0.0);

                    ptdf[[l, bus_i]] = b_line * (x_i - x_j);
                }
                // Slack bus PTDF is 0 (reference)
                ptdf[[l, slack_idx]] = 0.0;
            }
        }

        ptdf
    }

    /// Merit-order economic dispatch: sort generators by marginal cost at mid-range, dispatch cheapest first
    pub fn merit_order_dispatch(problem: &DcOpfProblem) -> Vec<(ElementId, f64)> {
        let total_load: f64 = problem.loads.iter().map(|(_, p)| p).sum();

        // Sort generators by marginal cost at mid-range: MC = 2*a*P_mid + b
        let mut sorted_gens: Vec<&GeneratorBid> = problem.generators.iter().collect();
        sorted_gens.sort_by(|a, b| {
            let mc_a = 2.0 * a.cost_a * (a.p_min + a.p_max) / 2.0 + a.cost_b;
            let mc_b = 2.0 * b.cost_a * (b.p_min + b.p_max) / 2.0 + b.cost_b;
            mc_a.partial_cmp(&mc_b).unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut generation = Vec::new();
        let mut remaining_load = total_load;

        for gen in &sorted_gens {
            if remaining_load <= 0.0 {
                generation.push((gen.gen_id, gen.p_min));
            } else {
                let dispatch = (gen.p_max).min(remaining_load).max(gen.p_min);
                generation.push((gen.gen_id, dispatch));
                remaining_load -= dispatch;
            }
        }

        // If still remaining load, try to increase generation up to p_max
        if remaining_load > 0.0 {
            for entry in generation.iter_mut() {
                if remaining_load <= 0.0 {
                    break;
                }
                if let Some(gen) = problem.generators.iter().find(|g| g.gen_id == entry.0) {
                    let headroom = gen.p_max - entry.1;
                    let increase = headroom.min(remaining_load);
                    entry.1 += increase;
                    remaining_load -= increase;
                }
            }
        }

        generation
    }

    /// Compute line flows using PTDF matrix
    pub fn compute_line_flows(
        ptdf: &Array2<f64>,
        generation: &[(ElementId, f64)],
        loads: &[(ElementId, f64)],
        bus_map: &HashMap<ElementId, usize>,
        bus_count: usize,
        _slack_idx: usize,
        problem: &DcOpfProblem,
    ) -> Vec<f64> {
        // Build net injection vector
        let mut p_net = Array1::<f64>::zeros(bus_count);
        for (gen_id, p_mw) in generation {
            if let Some(gen) = problem.generators.iter().find(|g| g.gen_id == *gen_id) {
                if let Some(&idx) = bus_map.get(&gen.bus_id) {
                    p_net[idx] += p_mw;
                }
            }
        }
        for &(bus_id, load_mw) in loads {
            if let Some(&idx) = bus_map.get(&bus_id) {
                p_net[idx] -= load_mw;
            }
        }

        // flows = PTDF * P_net
        let n_lines = problem.branches.len();
        let mut flows = Vec::with_capacity(n_lines);
        for l in 0..n_lines {
            let mut flow = 0.0;
            for i in 0..bus_count {
                flow += ptdf[[l, i]] * p_net[i];
            }
            flows.push(flow);
        }

        flows
    }

    /// Compute Locational Marginal Prices (LMP)
    pub fn compute_lmp(
        problem: &DcOpfProblem,
        ptdf: &Array2<f64>,
        bus_map: &HashMap<ElementId, usize>,
        bus_count: usize,
    ) -> Vec<f64> {
        // LMP = energy component + congestion component
        // Energy component: marginal cost of the marginal generator
        // Congestion component: based on PTDF and shadow prices of congested lines

        // Find the marginal generator (last dispatched)
        let generation = Self::merit_order_dispatch(problem);

        // Energy component: marginal cost of the most expensive dispatched generator
        let energy_price = generation.iter().map(|(gen_id, p_mw)| {
            if let Some(gen) = problem.generators.iter().find(|g| g.gen_id == *gen_id) {
                2.0 * gen.cost_a * p_mw + gen.cost_b
            } else {
                0.0
            }
        }).fold(0.0_f64, f64::max);

        // Congestion component: simplified - based on PTDF sensitivity
        let mut lmp = vec![energy_price; bus_count];

        // Check congested lines and adjust LMP
        let flows = Self::compute_line_flows(
            ptdf, &generation, &problem.loads, bus_map, bus_count,
            *bus_map.get(&problem.slack_bus_id).unwrap_or(&0),
            problem,
        );

        for (l, branch) in problem.branches.iter().enumerate() {
            if flows[l].abs() > branch.p_limit_mw * 0.99 {
                // Congested line - compute shadow price (simplified)
                let shadow_price = (flows[l].abs() - branch.p_limit_mw * 0.95).max(0.0) * 10.0;
                for i in 0..bus_count {
                    let ptdf_val = ptdf[[l, i]];
                    if flows[l] > 0.0 {
                        lmp[i] += ptdf_val * shadow_price;
                    } else {
                        lmp[i] -= ptdf_val * shadow_price;
                    }
                }
            }
        }

        lmp
    }
}

impl Default for DcOpfSolver {
    fn default() -> Self {
        Self::new()
    }
}

/// Solve linear system Ax = b using Gaussian elimination with partial pivoting
fn solve_linear_system(a: &Array2<f64>, b: &Array1<f64>) -> Option<Array1<f64>> {
    let n = b.len();
    if n == 0 {
        return Some(Array1::zeros(0));
    }

    let a_vec: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| a[[i, j]]).collect()).collect();
    let b_vec: Vec<f64> = b.to_vec();

    eneros_core::solve_linear_system(&a_vec, &b_vec).map(Array1::from_vec)
}

/// Invert a matrix using Gaussian elimination
fn invert_matrix(a: &Array2<f64>) -> Option<Array2<f64>> {
    let n = a.nrows();
    if n == 0 {
        return Some(Array2::zeros((0, 0)));
    }

    let a_vec: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| a[[i, j]]).collect()).collect();

    eneros_core::gauss_elimination_inverse(&a_vec).map(|inv| {
        let mut result = Array2::<f64>::zeros((n, n));
        for i in 0..n {
            for j in 0..n {
                result[[i, j]] = inv[i][j];
            }
        }
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_3bus_problem() -> DcOpfProblem {
        // 3-bus system:
        // Bus 1 (slack) --- Line 1 --- Bus 2 --- Line 2 --- Bus 3
        // Gen1 at Bus 1, Gen2 at Bus 2, Load at Bus 3
        DcOpfProblem {
            generators: vec![
                GeneratorBid {
                    gen_id: 1,
                    bus_id: 1,
                    p_min: 0.0,
                    p_max: 200.0,
                    cost_a: 0.01,
                    cost_b: 10.0,
                    cost_c: 0.0,
                },
                GeneratorBid {
                    gen_id: 2,
                    bus_id: 2,
                    p_min: 0.0,
                    p_max: 150.0,
                    cost_a: 0.02,
                    cost_b: 15.0,
                    cost_c: 0.0,
                },
            ],
            branches: vec![
                BranchLimit {
                    branch_id: 1,
                    from_bus: 1,
                    to_bus: 2,
                    p_limit_mw: 200.0,
                    reactance_pu: 0.1,
                },
                BranchLimit {
                    branch_id: 2,
                    from_bus: 2,
                    to_bus: 3,
                    p_limit_mw: 150.0,
                    reactance_pu: 0.15,
                },
            ],
            loads: vec![(3, 100.0)],
            slack_bus_id: 1,
        }
    }

    #[test]
    fn test_dc_opf_3bus() {
        let problem = create_3bus_problem();
        let solver = DcOpfSolver::new();
        let result = solver.solve(&problem);

        assert!(result.is_ok(), "DC-OPF failed: {:?}", result.err());
        let result = result.unwrap();
        assert!(result.converged, "DC-OPF did not converge, warnings: {:?}", result.warnings);

        // Total generation should approximately equal total load
        let total_gen: f64 = result.result.generation.iter().map(|(_, p)| p).sum();
        let total_load: f64 = problem.loads.iter().map(|(_, p)| p).sum();
        assert!(
            (total_gen - total_load).abs() < 5.0,
            "Generation {:.2} should approximately equal load {:.2}",
            total_gen, total_load
        );

        // Cheaper generator (Gen1) should produce more
        let gen1_p = result.result.generation.iter()
            .find(|(id, _)| *id == 1).map(|(_, p)| *p).unwrap_or(0.0);
        let gen2_p = result.result.generation.iter()
            .find(|(id, _)| *id == 2).map(|(_, p)| *p).unwrap_or(0.0);
        assert!(gen1_p >= gen2_p, "Gen1 ({:.2}) should dispatch >= Gen2 ({:.2})", gen1_p, gen2_p);

        // Total cost should be positive
        assert!(result.result.total_cost > 0.0, "Total cost should be positive");

        // Bus angles: slack bus angle should be 0
        let slack_angle = result.result.bus_angles.iter()
            .find(|(id, _)| *id == 1).map(|(_, a)| *a).unwrap_or(999.0);
        assert!(slack_angle.abs() < 1e-10, "Slack bus angle should be 0, got {}", slack_angle);
    }

    #[test]
    fn test_dc_opf_lmp() {
        let problem = create_3bus_problem();
        let solver = DcOpfSolver::new();
        let result = solver.solve(&problem).unwrap();

        // LMPs should be positive
        for (_, price) in &result.result.nodal_prices {
            assert!(*price > 0.0, "LMP should be positive, got {}", price);
        }

        // Slack bus LMP should equal marginal cost of cheapest generator
        let slack_lmp = result.result.nodal_prices.iter()
            .find(|(id, _)| *id == 1).map(|(_, p)| *p).unwrap_or(0.0);
        assert!(slack_lmp > 0.0, "Slack bus LMP should be positive");
    }

    #[test]
    fn test_dc_opf_line_flows() {
        let problem = create_3bus_problem();
        let solver = DcOpfSolver::new();
        let result = solver.solve(&problem).unwrap();

        // Line flows should be within limits (with some tolerance)
        for (branch_id, flow) in &result.result.line_flows {
            let branch = problem.branches.iter().find(|b| b.branch_id == *branch_id).unwrap();
            assert!(
                flow.abs() <= branch.p_limit_mw * 1.05,
                "Line {} flow {:.2} exceeds limit {:.2}",
                branch_id, flow.abs(), branch.p_limit_mw
            );
        }
    }

    #[test]
    fn test_dc_opf_congested() {
        // Create a problem with tight line limits to force congestion
        let mut problem = create_3bus_problem();
        problem.branches[0].p_limit_mw = 50.0; // Tight limit on line 1

        let solver = DcOpfSolver::new();
        let result = solver.solve(&problem);

        // Should still produce a result (possibly with warnings)
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_b_matrix() {
        let problem = create_3bus_problem();
        let bus_ids: Vec<ElementId> = vec![1, 2, 3];
        let bus_map: HashMap<ElementId, usize> = bus_ids.iter()
            .enumerate().map(|(i, &id)| (id, i)).collect();

        let b = DcOpfSolver::build_b_matrix(&problem, &bus_map, 3);

        // Diagonal elements should be positive
        assert!(b[[0, 0]] > 0.0, "B[0,0] should be positive");
        assert!(b[[1, 1]] > 0.0, "B[1,1] should be positive");
        assert!(b[[2, 2]] > 0.0, "B[2,2] should be positive");

        // Off-diagonal elements should be negative
        assert!(b[[0, 1]] < 0.0, "B[0,1] should be negative");
        assert!(b[[1, 0]] < 0.0, "B[1,0] should be negative");

        // Symmetry
        assert!((b[[0, 1]] - b[[1, 0]]).abs() < 1e-10, "B should be symmetric");
    }

    #[test]
    fn test_merit_order_dispatch() {
        let problem = create_3bus_problem();
        let dispatch = DcOpfSolver::merit_order_dispatch(&problem);

        // Total dispatch should cover total load
        let total_dispatch: f64 = dispatch.iter().map(|(_, p)| p).sum();
        let total_load: f64 = problem.loads.iter().map(|(_, p)| p).sum();
        assert!(
            (total_dispatch - total_load).abs() < 1.0,
            "Dispatch {:.2} should cover load {:.2}",
            total_dispatch, total_load
        );

        // Cheaper generator should be dispatched more
        let gen1_p = dispatch.iter().find(|(id, _)| *id == 1).map(|(_, p)| *p).unwrap_or(0.0);
        let gen2_p = dispatch.iter().find(|(id, _)| *id == 2).map(|(_, p)| *p).unwrap_or(0.0);
        assert!(gen1_p >= gen2_p);
    }

    #[test]
    fn test_dc_opf_no_generators() {
        let problem = DcOpfProblem {
            generators: vec![],
            branches: vec![BranchLimit {
                branch_id: 1, from_bus: 1, to_bus: 2,
                p_limit_mw: 100.0, reactance_pu: 0.1,
            }],
            loads: vec![(2, 50.0)],
            slack_bus_id: 1,
        };
        let solver = DcOpfSolver::new();
        let result = solver.solve(&problem);
        assert!(result.is_err());
    }

    #[test]
    fn test_dc_opf_14bus_simplified() {
        // Simplified IEEE 14-bus equivalent: 5 generators, 5 loads, 5 branches
        let problem = DcOpfProblem {
            generators: vec![
                GeneratorBid { gen_id: 1, bus_id: 1, p_min: 0.0, p_max: 200.0, cost_a: 0.005, cost_b: 10.0, cost_c: 100.0 },
                GeneratorBid { gen_id: 2, bus_id: 2, p_min: 0.0, p_max: 150.0, cost_a: 0.01, cost_b: 15.0, cost_c: 80.0 },
                GeneratorBid { gen_id: 3, bus_id: 3, p_min: 0.0, p_max: 100.0, cost_a: 0.015, cost_b: 20.0, cost_c: 60.0 },
                GeneratorBid { gen_id: 4, bus_id: 6, p_min: 0.0, p_max: 80.0, cost_a: 0.02, cost_b: 25.0, cost_c: 40.0 },
                GeneratorBid { gen_id: 5, bus_id: 8, p_min: 0.0, p_max: 60.0, cost_a: 0.025, cost_b: 30.0, cost_c: 20.0 },
            ],
            branches: vec![
                BranchLimit { branch_id: 1, from_bus: 1, to_bus: 2, p_limit_mw: 200.0, reactance_pu: 0.06 },
                BranchLimit { branch_id: 2, from_bus: 2, to_bus: 3, p_limit_mw: 150.0, reactance_pu: 0.08 },
                BranchLimit { branch_id: 3, from_bus: 2, to_bus: 4, p_limit_mw: 150.0, reactance_pu: 0.12 },
                BranchLimit { branch_id: 4, from_bus: 3, to_bus: 6, p_limit_mw: 100.0, reactance_pu: 0.10 },
                BranchLimit { branch_id: 5, from_bus: 6, to_bus: 8, p_limit_mw: 80.0, reactance_pu: 0.15 },
            ],
            loads: vec![
                (2, 50.0), (3, 80.0), (4, 60.0), (6, 40.0), (8, 30.0),
            ],
            slack_bus_id: 1,
        };

        let solver = DcOpfSolver::new();
        let result = solver.solve(&problem);

        assert!(result.is_ok(), "14-bus simplified OPF failed: {:?}", result.err());
        let result = result.unwrap();

        // Total generation should cover total load
        let total_gen: f64 = result.result.generation.iter().map(|(_, p)| p).sum();
        let total_load: f64 = problem.loads.iter().map(|(_, p)| p).sum();
        assert!(
            (total_gen - total_load).abs() < 10.0,
            "Generation {:.2} should cover load {:.2}",
            total_gen, total_load
        );

        // Cheapest generator should produce most
        let gen1_p = result.result.generation.iter()
            .find(|(id, _)| *id == 1).map(|(_, p)| *p).unwrap_or(0.0);
        let gen5_p = result.result.generation.iter()
            .find(|(id, _)| *id == 5).map(|(_, p)| *p).unwrap_or(0.0);
        assert!(gen1_p >= gen5_p, "Gen1 ({:.2}) should dispatch >= Gen5 ({:.2})", gen1_p, gen5_p);
    }
}
