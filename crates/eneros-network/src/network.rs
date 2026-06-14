use std::collections::HashMap;
use std::sync::Arc;
use eneros_core::{ElementId, Result};
use eneros_powerflow::{PowerFlowSolver, YBusMatrix, PowerFlowResult, BusTypeNR};
use eneros_constraint::{ConstraintEngine, N1Result, StabilityResult, Violation};

/// Power network model — unified topology-to-powerflow pipeline entry point
///
/// PowerNetwork integrates all Phase 1 kernel crates:
/// - eneros-powerflow: Newton-Raphson solver
/// - eneros-constraint: N-1 / stability / constraint checks
/// - eneros-topology / eneros-equipment: network structure (future)
pub struct PowerNetwork {
    /// Y-Bus admittance matrix
    ybus: YBusMatrix,
    /// Active power specifications (per-unit)
    p_spec: Vec<f64>,
    /// Reactive power specifications (per-unit)
    q_spec: Vec<f64>,
    /// Bus types for power flow
    bus_types: Vec<BusTypeNR>,
    /// Branch data for N-1 analysis: (from, to, r, x, b, tap)
    branches: Vec<(ElementId, ElementId, f64, f64, f64, f64)>,
    /// Bus ID to index mapping
    bus_map: HashMap<ElementId, usize>,
    /// Power flow solver
    solver: PowerFlowSolver,
    /// Constraint engine (shared via Arc for consistency with main ConstraintEngine)
    constraint: Arc<ConstraintEngine>,
    /// Initial voltage magnitudes (optional)
    v_initial: Option<Vec<f64>>,
}

impl PowerNetwork {
    /// Create a new PowerNetwork from raw power flow data
    pub fn new(
        ybus: YBusMatrix,
        p_spec: Vec<f64>,
        q_spec: Vec<f64>,
        bus_types: Vec<BusTypeNR>,
        branches: Vec<(ElementId, ElementId, f64, f64, f64, f64)>,
        bus_map: HashMap<ElementId, usize>,
    ) -> Self {
        Self {
            ybus,
            p_spec,
            q_spec,
            bus_types,
            branches,
            bus_map,
            solver: PowerFlowSolver::default_solver(),
            constraint: Arc::new(ConstraintEngine::new()),
            v_initial: None,
        }
    }

    /// Create from IEEE 14-bus test system
    pub fn from_ieee14() -> Self {
        let data = eneros_powerflow::ieee14();
        let (ybus, p_spec, q_spec, bus_types) = data.to_solver_input();

        let bus_map: HashMap<ElementId, usize> = data
            .buses
            .iter()
            .enumerate()
            .map(|(idx, bus)| (bus.bus_id as ElementId, idx))
            .collect();

        let branches: Vec<(ElementId, ElementId, f64, f64, f64, f64)> = data
            .branches
            .iter()
            .map(|br| {
                (br.from_bus as ElementId, br.to_bus as ElementId,
                 br.r_pu, br.x_pu, br.b_pu, br.tap_ratio)
            })
            .collect();

        let v_initial: Vec<f64> = data.buses.iter().map(|b| b.v_pu).collect();

        Self {
            ybus,
            p_spec,
            q_spec,
            bus_types,
            branches,
            bus_map,
            solver: PowerFlowSolver::new(100, 1e-8),
            constraint: Arc::new(ConstraintEngine::new()),
            v_initial: Some(v_initial),
        }
    }

    /// Create a PowerNetwork from an equipment library and network graph
    pub fn from_equipment(
        _library: &eneros_equipment::EquipmentLibrary,
        graph: &eneros_topology::NetworkGraph,
        base_mva: f64,
    ) -> Result<Self> {
        let (ybus, p_spec, q_spec, bus_types, v_initial) = graph.to_solver_input(base_mva);
        let branches = graph.online_branches();

        let mut bus_ids = graph.bus_ids();
        bus_ids.sort();
        let bus_map: HashMap<ElementId, usize> = bus_ids.iter()
            .enumerate()
            .map(|(i, &id)| (id, i))
            .collect();

        Ok(Self {
            ybus,
            p_spec,
            q_spec,
            bus_types,
            branches,
            bus_map,
            solver: PowerFlowSolver::new(100, 1e-8),
            constraint: Arc::new(ConstraintEngine::new()),
            v_initial: Some(v_initial),
        })
    }

    /// Set custom solver parameters
    pub fn with_solver(mut self, max_iterations: u32, tolerance: f64) -> Self {
        self.solver = PowerFlowSolver::new(max_iterations, tolerance);
        self
    }

    /// Set initial voltage magnitudes
    pub fn with_initial_voltages(mut self, v_initial: Vec<f64>) -> Self {
        self.v_initial = Some(v_initial);
        self
    }

    /// Get reference to constraint engine for registering constraints
    pub fn constraint_engine(&self) -> &ConstraintEngine {
        &self.constraint
    }

    /// Set a shared ConstraintEngine (e.g., one wired with EventBus + Projector)
    pub fn with_constraint_engine(mut self, engine: Arc<ConstraintEngine>) -> Self {
        self.constraint = engine;
        self
    }

    /// Execute power flow calculation
    pub fn solve(&self) -> Result<PowerFlowResult> {
        match &self.v_initial {
            Some(v_init) => self.solver.solve_with_initial(
                &self.ybus, &self.p_spec, &self.q_spec, &self.bus_types, Some(v_init),
            ),
            None => self.solver.solve(
                &self.ybus, &self.p_spec, &self.q_spec, &self.bus_types,
            ),
        }
    }

    /// Perform N-1 contingency analysis
    pub fn check_n1(&self) -> Vec<N1Result> {
        self.constraint.check_n1_analysis(
            &self.ybus,
            &self.p_spec,
            &self.q_spec,
            &self.bus_types,
            &self.branches,
            &self.bus_map,
            &self.solver,
            None, None, None,
        )
    }

    /// Perform N-1 analysis with custom limits
    pub fn check_n1_with_limits(
        &self,
        voltage_min: f64,
        voltage_max: f64,
        thermal_limit: f64,
    ) -> Vec<N1Result> {
        self.constraint.check_n1_analysis(
            &self.ybus,
            &self.p_spec,
            &self.q_spec,
            &self.bus_types,
            &self.branches,
            &self.bus_map,
            &self.solver,
            Some(voltage_min),
            Some(voltage_max),
            Some(thermal_limit),
        )
    }

    /// Check constraints against power flow results
    pub fn check_constraints(&self, result: &PowerFlowResult) -> Vec<Violation> {
        let bus_voltages: Vec<(ElementId, f64)> = result
            .bus_results
            .iter()
            .map(|br| (br.bus_id, br.voltage_magnitude))
            .collect();

        let branch_loadings: Vec<(ElementId, f64)> = result
            .branch_results
            .iter()
            .map(|br| (br.branch_id, br.loading_percent))
            .collect();

        self.constraint.check_all(&bus_voltages, &branch_loadings, 50.0)
    }

    /// Perform voltage stability analysis
    pub fn check_stability(&self, result: &PowerFlowResult) -> StabilityResult {
        self.constraint.check_stability(&result.bus_results)
    }

    /// Get the Y-Bus matrix
    pub fn ybus(&self) -> &YBusMatrix {
        &self.ybus
    }

    /// Get bus types
    pub fn bus_types(&self) -> &[BusTypeNR] {
        &self.bus_types
    }

    /// Get number of buses
    pub fn bus_count(&self) -> usize {
        self.bus_types.len()
    }

    /// Get number of branches
    pub fn branch_count(&self) -> usize {
        self.branches.len()
    }

    /// Get a reference to the active power specifications
    pub fn p_spec(&self) -> &[f64] {
        &self.p_spec
    }

    /// Get a reference to the bus ID to index mapping
    pub fn bus_map(&self) -> &HashMap<ElementId, usize> {
        &self.bus_map
    }

    /// Create a new network with modified active power specifications (for What-If analysis)
    pub fn with_modified_p_spec(&self, p_spec: Vec<f64>) -> Self {
        Self {
            ybus: self.ybus.clone(),
            p_spec,
            q_spec: self.q_spec.clone(),
            bus_types: self.bus_types.clone(),
            branches: self.branches.clone(),
            bus_map: self.bus_map.clone(),
            solver: self.solver.clone(),
            constraint: Arc::new(ConstraintEngine::new()),
            v_initial: self.v_initial.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_from_ieee14() {
        let network = PowerNetwork::from_ieee14();
        assert_eq!(network.bus_count(), 14);
        assert_eq!(network.branch_count(), 20);
    }

    #[test]
    fn test_network_solve_ieee14() {
        let network = PowerNetwork::from_ieee14();
        let result = network.solve();

        assert!(result.is_ok(), "IEEE 14 solve failed: {:?}", result.err());
        let result = result.unwrap();
        assert!(result.converged, "IEEE 14 did not converge");
        assert!(result.iterations <= 20, "Too many iterations: {}", result.iterations);
    }

    #[test]
    fn test_network_n1_ieee14() {
        let network = PowerNetwork::from_ieee14();
        let n1_results = network.check_n1();

        assert_eq!(n1_results.len(), 20);
        // Most contingencies should converge for IEEE 14
        let converged = n1_results.iter().filter(|r| r.converged).count();
        assert!(converged >= 15, "At least 15 N-1 cases should converge, got {}", converged);
    }

    #[test]
    fn test_network_stability_ieee14() {
        let network = PowerNetwork::from_ieee14();
        let result = network.solve().unwrap();
        let stability = network.check_stability(&result);

        assert!(stability.stable, "IEEE 14 should be stable");
        assert_eq!(stability.voltage_margins.len(), 14);
    }

    #[test]
    fn test_from_ieee14_accuracy() {
        let network = PowerNetwork::from_ieee14();
        let result = network.solve().expect("IEEE 14 power flow failed");

        assert!(result.converged, "IEEE 14 did not converge");

        let data = eneros_powerflow::ieee14();

        for (i, bus_data) in data.buses.iter().enumerate() {
            let computed = &result.bus_results[i];
            let v_error = (computed.voltage_magnitude - bus_data.v_pu).abs();
            let angle_error = (computed.voltage_angle.to_degrees() - bus_data.angle_deg).abs();

            assert!(
                v_error < 0.02,
                "Bus {} voltage error too large: computed={}, expected={}, error={}",
                bus_data.bus_id, computed.voltage_magnitude, bus_data.v_pu, v_error
            );
            assert!(
                angle_error < 0.1,
                "Bus {} angle error too large: computed={:.4}°, expected={:.4}°, error={:.4}°",
                bus_data.bus_id, computed.voltage_angle.to_degrees(), bus_data.angle_deg, angle_error
            );
        }
    }

    #[test]
    fn test_network_custom_simple() {
        let mut bus_map = HashMap::new();
        bus_map.insert(0u64, 0);
        bus_map.insert(1u64, 1);

        let branches = vec![(0u64, 1u64, 0.01, 0.1, 0.0, 1.0)];
        let ybus = YBusMatrix::from_branches(&branches, &bus_map);

        let network = PowerNetwork::new(
            ybus,
            vec![0.0, -0.5],
            vec![0.0, -0.2],
            vec![BusTypeNR::Slack, BusTypeNR::PQ],
            branches,
            bus_map,
        );

        let result = network.solve();
        assert!(result.is_ok());
        assert!(result.unwrap().converged);
    }

    #[test]
    fn test_network_from_equipment_ieee14() {
        use eneros_topology::{NetworkGraph, Bus, Branch};
        use eneros_core::{BusType, BranchType};
        use eneros_equipment::EquipmentLibrary;

        let mut graph = NetworkGraph::new();

        // Build a 3-bus network: Slack - PV - PQ
        let buses = vec![
            Bus {
                id: 1, name: "Slack".into(), bus_type: BusType::Slack,
                voltage_kv: 138.0, zone_id: 0,
                bus_type_pf: BusType::Slack,
                p_gen: 0.0, q_gen: 0.0, p_load: 0.0, q_load: 0.0,
                v_pu: 1.06,
            },
            Bus {
                id: 2, name: "PV".into(), bus_type: BusType::PV,
                voltage_kv: 138.0, zone_id: 0,
                bus_type_pf: BusType::PV,
                p_gen: 40.0, q_gen: 0.0, p_load: 0.0, q_load: 0.0,
                v_pu: 1.045,
            },
            Bus {
                id: 3, name: "PQ".into(), bus_type: BusType::PQ,
                voltage_kv: 138.0, zone_id: 0,
                bus_type_pf: BusType::PQ,
                p_gen: 0.0, q_gen: 0.0, p_load: 50.0, q_load: 20.0,
                v_pu: 1.01,
            },
        ];

        let branches = vec![
            Branch {
                id: 1, name: "Line1-2".into(), from_bus: 1, to_bus: 2,
                branch_type: BranchType::Line, status: true,
                r: 0.01, x: 0.1, b: 0.02, tap_ratio: 1.0,
            },
            Branch {
                id: 2, name: "Line2-3".into(), from_bus: 2, to_bus: 3,
                branch_type: BranchType::Line, status: true,
                r: 0.02, x: 0.15, b: 0.03, tap_ratio: 1.0,
            },
            Branch {
                id: 3, name: "Line1-3".into(), from_bus: 1, to_bus: 3,
                branch_type: BranchType::Line, status: true,
                r: 0.03, x: 0.2, b: 0.04, tap_ratio: 1.0,
            },
        ];

        graph.initialize(buses, branches, vec![]).unwrap();

        let library = EquipmentLibrary::new();
        let network = PowerNetwork::from_equipment(&library, &graph, 100.0).unwrap();

        assert_eq!(network.bus_count(), 3);
        assert_eq!(network.branch_count(), 3);

        let result = network.solve();
        assert!(result.is_ok(), "from_equipment solve failed: {:?}", result.err());
        let result = result.unwrap();
        assert!(result.converged, "from_equipment network did not converge");
    }
}
