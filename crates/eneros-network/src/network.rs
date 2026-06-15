use eneros_constraint::{
    Constraint, ConstraintCategory, ConstraintEngine, ConstraintType, N1Result, ResponseStrategy,
    StabilityResult, Violation,
};
use eneros_core::{ElementId, Result};
use eneros_powerflow::{BusTypeNR, PowerFlowResult, PowerFlowSolver, YBusMatrix};
use std::collections::HashMap;
use std::sync::Arc;

/// Generator specification — the physical model of a single generator.
///
/// Phase 15 addition: `PowerNetwork` previously had no generator table at all,
/// so the What-If simulator hardcoded limits and misused `bus_map` for gen_id
/// lookups. This struct makes the generator data self-describing so the
/// simulator reads real limits and maps gen_id → bus correctly.
#[derive(Debug, Clone)]
pub struct GeneratorSpec {
    /// Generator ID (the `gen_id` used in `StructuredAction::StartGenerator`)
    pub gen_id: ElementId,
    /// Bus ID where this generator connects
    pub bus_id: ElementId,
    /// Minimum active power output (MW)
    pub p_min_mw: f64,
    /// Maximum active power output (MW)
    pub p_max_mw: f64,
    /// Gross active generation (MW) at the snapshot this network was built from
    pub p_gen_mw: f64,
    /// Gross active load (MW) at the same bus at the snapshot
    pub p_load_mw: f64,
}

impl GeneratorSpec {
    /// Net active injection at the generator bus (MW): generation minus load.
    /// Positive = net generation. This is what `p_spec` stores (in per-unit).
    pub fn net_p_mw(&self) -> f64 {
        self.p_gen_mw - self.p_load_mw
    }
}

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
    /// Generator table (Phase 15): gen_id → physical spec.
    /// Empty for networks constructed without generator data; populated by
    /// `from_ieee14`. Drives accurate feasibility projection.
    generators: Vec<GeneratorSpec>,
    /// Zone map (Phase 15): zone_id → bus_ids in that zone.
    /// Used by `ShedLoad` to distribute load reduction across a zone's buses.
    zone_map: HashMap<u32, Vec<ElementId>>,
    /// Branch IDs parallel to `branches` (Phase 15): identifies each branch
    /// for future switching/topology modeling.
    branch_ids: Vec<ElementId>,
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
            generators: Vec::new(),
            zone_map: HashMap::new(),
            branch_ids: Vec::new(),
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
                (
                    br.from_bus as ElementId,
                    br.to_bus as ElementId,
                    br.r_pu,
                    br.x_pu,
                    br.b_pu,
                    br.tap_ratio,
                )
            })
            .collect();

        let v_initial: Vec<f64> = data.buses.iter().map(|b| b.v_pu).collect();

        // Phase 15: real generator table for IEEE-14. The 5 generators sit on
        // buses 1 (Slack), 2, 3, 6, 8 (PV). Limits are the standard IEEE-14
        // values (also referenced by main.rs build_ieee14_opf_problem). Gross
        // gen/load are reconstructed from the documented net-injection values
        // in ieee.rs comments so What-If projections can recompute net injection
        // from a proposed target_mw instead of clobbering it.
        let generators = vec![
            // Bus 1 (Slack): swing bus; p_mw=0 net (balance-determined).
            GeneratorSpec {
                gen_id: 1,
                bus_id: 1,
                p_min_mw: 0.0,
                p_max_mw: 332.4,
                p_gen_mw: 0.0,
                p_load_mw: 0.0,
            },
            // Bus 2: Gen=40MW, Load=21.7MW => net +18.3MW
            GeneratorSpec {
                gen_id: 2,
                bus_id: 2,
                p_min_mw: 0.0,
                p_max_mw: 140.0,
                p_gen_mw: 40.0,
                p_load_mw: 21.7,
            },
            // Bus 3: synchronous condenser (P≈0), Load=94.2MW => net -94.2MW
            GeneratorSpec {
                gen_id: 3,
                bus_id: 3,
                p_min_mw: 0.0,
                p_max_mw: 100.0,
                p_gen_mw: 0.0,
                p_load_mw: 94.2,
            },
            // Bus 6: Gen=0MW, Load=11.2MW => net -11.2MW
            GeneratorSpec {
                gen_id: 6,
                bus_id: 6,
                p_min_mw: 0.0,
                p_max_mw: 80.0,
                p_gen_mw: 0.0,
                p_load_mw: 11.2,
            },
            // Bus 8: Gen=0MW, no load
            GeneratorSpec {
                gen_id: 8,
                bus_id: 8,
                p_min_mw: 0.0,
                p_max_mw: 60.0,
                p_gen_mw: 0.0,
                p_load_mw: 0.0,
            },
        ];

        // Phase 15: zone map. IEEE-14 has no explicit zones in the source data,
        // so we partition buses into a single default zone (zone 0) containing
        // all buses. This lets ShedLoad{zone_id:0} distribute across the whole
        // network rather than silently no-op'ing.
        let all_buses: Vec<ElementId> = data.buses.iter().map(|b| b.bus_id as ElementId).collect();
        let mut zone_map = HashMap::new();
        zone_map.insert(0u32, all_buses);

        // Phase 15: branch IDs (1-based, parallel to `branches`).
        let branch_ids: Vec<ElementId> = (1..=branches.len() as ElementId).collect();

        let network = Self {
            ybus,
            p_spec,
            q_spec,
            bus_types,
            branches,
            bus_map,
            solver: PowerFlowSolver::new(100, 1e-8),
            constraint: Arc::new(ConstraintEngine::new()),
            v_initial: Some(v_initial),
            generators,
            zone_map,
            branch_ids,
        };

        // Phase 15: register standard voltage/thermal constraints so
        // check_constraints actually detects violations instead of always
        // returning an empty Vec.
        network.register_default_constraints();

        network
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
        let bus_map: HashMap<ElementId, usize> =
            bus_ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();

        let branch_ids: Vec<ElementId> = (1..=branches.len() as ElementId).collect();

        let network = Self {
            ybus,
            p_spec,
            q_spec,
            bus_types,
            branches,
            bus_map,
            solver: PowerFlowSolver::new(100, 1e-8),
            constraint: Arc::new(ConstraintEngine::new()),
            v_initial: Some(v_initial),
            generators: Vec::new(),
            zone_map: HashMap::new(),
            branch_ids,
        };

        // Phase 15: register standard constraints.
        network.register_default_constraints();

        Ok(network)
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

    /// Phase 15: register standard voltage and thermal constraints covering
    /// every bus and branch.
    ///
    /// Without this, `check_constraints` always returns an empty `Vec` because
    /// `ConstraintEngine::new()` registers nothing — so every What-If looked
    /// "feasible" even when voltages or loadings were wildly out of bounds.
    /// The IEEE standard limits (0.95–1.05 pu voltage, ≤100% thermal loading)
    /// are sane defaults that callers can override via `constraint_engine()`.
    fn register_default_constraints(&self) {
        // Voltage constraint: all buses within 0.95–1.05 p.u.
        let all_bus_ids: Vec<ElementId> = self.bus_map.keys().copied().collect();
        let voltage = Constraint {
            id: "default_voltage".to_string(),
            name: "Default voltage limits (0.95-1.05 pu)".to_string(),
            constraint_type: ConstraintType::Voltage,
            category: ConstraintCategory::Normal,
            element_type: "bus".to_string(),
            element_ids: all_bus_ids,
            parameter: "voltage_pu".to_string(),
            limit_min: 0.95,
            limit_max: 1.05,
            severity: eneros_core::SeverityLevel::Major,
            response_strategy: ResponseStrategy::Degradation,
            check_interval_ms: 1000,
            enabled: true,
        };
        self.constraint.register(voltage);

        let mut all_branch_ids = Vec::new();
        let n = self.bus_map.len();
        for (from_bus, to_bus, _, _, _, _) in &self.branches {
            if let (Some(&from_idx), Some(&to_idx)) =
                (self.bus_map.get(from_bus), self.bus_map.get(to_bus))
            {
                all_branch_ids.push((from_idx * n + to_idx) as ElementId);
                all_branch_ids.push((to_idx * n + from_idx) as ElementId);
            }
        }
        let thermal = Constraint {
            id: "default_thermal".to_string(),
            name: "Default thermal limits (<=100% loading)".to_string(),
            constraint_type: ConstraintType::Thermal,
            category: ConstraintCategory::Normal,
            element_type: "branch".to_string(),
            element_ids: all_branch_ids,
            parameter: "loading_percent".to_string(),
            limit_min: 0.0,
            limit_max: 100.0,
            severity: eneros_core::SeverityLevel::Major,
            response_strategy: ResponseStrategy::Degradation,
            check_interval_ms: 1000,
            enabled: true,
        };
        self.constraint.register(thermal);
    }

    /// Execute power flow calculation
    pub fn solve(&self) -> Result<PowerFlowResult> {
        match &self.v_initial {
            Some(v_init) => self.solver.solve_with_initial(
                &self.ybus,
                &self.p_spec,
                &self.q_spec,
                &self.bus_types,
                Some(v_init),
            ),
            None => self
                .solver
                .solve(&self.ybus, &self.p_spec, &self.q_spec, &self.bus_types),
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
            None,
            None,
            None,
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
        let mut bus_ids_by_index = vec![0; self.bus_map.len()];
        for (&bus_id, &idx) in &self.bus_map {
            if idx < bus_ids_by_index.len() {
                bus_ids_by_index[idx] = bus_id;
            }
        }

        let bus_voltages: Vec<(ElementId, f64)> = result
            .bus_results
            .iter()
            .map(|br| {
                let bus_id = bus_ids_by_index
                    .get(br.bus_id as usize)
                    .copied()
                    .unwrap_or(br.bus_id);
                (bus_id, br.voltage_magnitude)
            })
            .collect();

        let branch_loadings: Vec<(ElementId, f64)> = result
            .branch_results
            .iter()
            .map(|br| (br.branch_id, br.loading_percent))
            .collect();

        self.constraint
            .check_all(&bus_voltages, &branch_loadings, 50.0)
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

    /// Get a reference to the reactive power specifications (Phase 15).
    /// Needed by the What-If simulator for `ExecuteDevice{adjust_reactive}`.
    pub fn q_spec_view(&self) -> &[f64] {
        &self.q_spec
    }

    /// Get a reference to the bus ID to index mapping
    pub fn bus_map(&self) -> &HashMap<ElementId, usize> {
        &self.bus_map
    }

    /// Create a new network with modified active power specifications (for What-If analysis)
    ///
    /// Phase 15 fix: previously this cloned a fresh empty `ConstraintEngine`,
    /// so every What-If clone silently lost all registered constraints and
    /// `check_constraints` always reported "no violations". We now share the
    /// original `Arc<ConstraintEngine>` so the cloned network enforces the same
    /// limits. The generator/zone/branch-id tables are also carried over so
    /// subsequent What-Ifs remain self-describing.
    pub fn with_modified_p_spec(&self, p_spec: Vec<f64>) -> Self {
        Self {
            ybus: self.ybus.clone(),
            p_spec,
            q_spec: self.q_spec.clone(),
            bus_types: self.bus_types.clone(),
            branches: self.branches.clone(),
            bus_map: self.bus_map.clone(),
            solver: self.solver.clone(),
            constraint: self.constraint.clone(),
            v_initial: self.v_initial.clone(),
            generators: self.generators.clone(),
            zone_map: self.zone_map.clone(),
            branch_ids: self.branch_ids.clone(),
        }
    }

    /// Create a new network with optional P and Q spec modifications.
    ///
    /// Phase 15 generalization of `with_modified_p_spec`: lets the simulator
    /// adjust reactive specs (needed for `ExecuteDevice{adjust_reactive}`)
    /// in one clone instead of two. `None` keeps the original spec.
    pub fn with_modifications(&self, p_spec: Option<Vec<f64>>, q_spec: Option<Vec<f64>>) -> Self {
        Self {
            ybus: self.ybus.clone(),
            p_spec: p_spec.unwrap_or_else(|| self.p_spec.clone()),
            q_spec: q_spec.unwrap_or_else(|| self.q_spec.clone()),
            bus_types: self.bus_types.clone(),
            branches: self.branches.clone(),
            bus_map: self.bus_map.clone(),
            solver: self.solver.clone(),
            constraint: self.constraint.clone(),
            v_initial: self.v_initial.clone(),
            generators: self.generators.clone(),
            zone_map: self.zone_map.clone(),
            branch_ids: self.branch_ids.clone(),
        }
    }

    /// Get the generator table (Phase 15). Empty when the network was built
    /// without generator data (e.g. raw `new()`). Used by the What-If simulator
    /// to read real Pmin/Pmax and to map gen_id → bus_id.
    pub fn generator_table(&self) -> &[GeneratorSpec] {
        &self.generators
    }

    /// Look up a generator by gen_id (Phase 15).
    pub fn generator_at(&self, gen_id: ElementId) -> Option<&GeneratorSpec> {
        self.generators.iter().find(|g| g.gen_id == gen_id)
    }

    /// Get the bus_ids belonging to a zone (Phase 15). Returns `None` if the
    /// zone is unknown. Used by `ShedLoad` to distribute load reduction.
    pub fn zone_buses(&self, zone_id: u32) -> Option<&[ElementId]> {
        self.zone_map.get(&zone_id).map(|v| v.as_slice())
    }

    /// Get branch IDs parallel to `branches` (Phase 15).
    pub fn branch_ids(&self) -> &[ElementId] {
        &self.branch_ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_powerflow::{BranchResult, BusResult};

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
        assert!(
            result.iterations <= 20,
            "Too many iterations: {}",
            result.iterations
        );
    }

    #[test]
    fn test_network_n1_ieee14() {
        let network = PowerNetwork::from_ieee14();
        let n1_results = network.check_n1();

        assert_eq!(n1_results.len(), 20);
        // Most contingencies should converge for IEEE 14
        let converged = n1_results.iter().filter(|r| r.converged).count();
        assert!(
            converged >= 15,
            "At least 15 N-1 cases should converge, got {}",
            converged
        );
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
                bus_data.bus_id,
                computed.voltage_magnitude,
                bus_data.v_pu,
                v_error
            );
            assert!(
                angle_error < 0.1,
                "Bus {} angle error too large: computed={:.4}°, expected={:.4}°, error={:.4}°",
                bus_data.bus_id,
                computed.voltage_angle.to_degrees(),
                bus_data.angle_deg,
                angle_error
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
        use eneros_core::{BranchType, BusType};
        use eneros_equipment::EquipmentLibrary;
        use eneros_topology::{Branch, Bus, NetworkGraph};

        let mut graph = NetworkGraph::new();

        // Build a 3-bus network: Slack - PV - PQ
        let buses = vec![
            Bus {
                id: 1,
                name: "Slack".into(),
                bus_type: BusType::Slack,
                voltage_kv: 138.0,
                zone_id: 0,
                bus_type_pf: BusType::Slack,
                p_gen: 0.0,
                q_gen: 0.0,
                p_load: 0.0,
                q_load: 0.0,
                v_pu: 1.06,
            },
            Bus {
                id: 2,
                name: "PV".into(),
                bus_type: BusType::PV,
                voltage_kv: 138.0,
                zone_id: 0,
                bus_type_pf: BusType::PV,
                p_gen: 40.0,
                q_gen: 0.0,
                p_load: 0.0,
                q_load: 0.0,
                v_pu: 1.045,
            },
            Bus {
                id: 3,
                name: "PQ".into(),
                bus_type: BusType::PQ,
                voltage_kv: 138.0,
                zone_id: 0,
                bus_type_pf: BusType::PQ,
                p_gen: 0.0,
                q_gen: 0.0,
                p_load: 50.0,
                q_load: 20.0,
                v_pu: 1.01,
            },
        ];

        let branches = vec![
            Branch {
                id: 1,
                name: "Line1-2".into(),
                from_bus: 1,
                to_bus: 2,
                branch_type: BranchType::Line,
                status: true,
                r: 0.01,
                x: 0.1,
                b: 0.02,
                tap_ratio: 1.0,
            },
            Branch {
                id: 2,
                name: "Line2-3".into(),
                from_bus: 2,
                to_bus: 3,
                branch_type: BranchType::Line,
                status: true,
                r: 0.02,
                x: 0.15,
                b: 0.03,
                tap_ratio: 1.0,
            },
            Branch {
                id: 3,
                name: "Line1-3".into(),
                from_bus: 1,
                to_bus: 3,
                branch_type: BranchType::Line,
                status: true,
                r: 0.03,
                x: 0.2,
                b: 0.04,
                tap_ratio: 1.0,
            },
        ];

        graph.initialize(buses, branches, vec![]).unwrap();

        let library = EquipmentLibrary::new();
        let network = PowerNetwork::from_equipment(&library, &graph, 100.0).unwrap();

        assert_eq!(network.bus_count(), 3);
        assert_eq!(network.branch_count(), 3);

        let result = network.solve();
        assert!(
            result.is_ok(),
            "from_equipment solve failed: {:?}",
            result.err()
        );
        let result = result.unwrap();
        assert!(result.converged, "from_equipment network did not converge");
    }

    fn non_contiguous_test_network() -> PowerNetwork {
        use eneros_core::{BranchType, BusType};
        use eneros_equipment::EquipmentLibrary;
        use eneros_topology::{Branch, Bus, NetworkGraph};

        let mut graph = NetworkGraph::new();
        let buses = vec![
            Bus {
                id: 10,
                name: "Slack".into(),
                bus_type: BusType::Slack,
                voltage_kv: 138.0,
                zone_id: 0,
                bus_type_pf: BusType::Slack,
                p_gen: 0.0,
                q_gen: 0.0,
                p_load: 0.0,
                q_load: 0.0,
                v_pu: 1.0,
            },
            Bus {
                id: 20,
                name: "Load".into(),
                bus_type: BusType::PQ,
                voltage_kv: 138.0,
                zone_id: 0,
                bus_type_pf: BusType::PQ,
                p_gen: 0.0,
                q_gen: 0.0,
                p_load: 20.0,
                q_load: 5.0,
                v_pu: 1.0,
            },
            Bus {
                id: 30,
                name: "Remote".into(),
                bus_type: BusType::PQ,
                voltage_kv: 138.0,
                zone_id: 0,
                bus_type_pf: BusType::PQ,
                p_gen: 0.0,
                q_gen: 0.0,
                p_load: 10.0,
                q_load: 2.0,
                v_pu: 1.0,
            },
        ];
        let branches = vec![Branch {
            id: 99,
            name: "Line10-30".into(),
            from_bus: 10,
            to_bus: 30,
            branch_type: BranchType::Line,
            status: true,
            r: 0.01,
            x: 0.1,
            b: 0.0,
            tap_ratio: 1.0,
        }];

        graph.initialize(buses, branches, vec![]).unwrap();
        PowerNetwork::from_equipment(&EquipmentLibrary::new(), &graph, 100.0).unwrap()
    }

    #[test]
    fn test_constraints_map_solver_bus_index_to_external_bus_id() {
        let network = non_contiguous_test_network();
        let result = PowerFlowResult {
            converged: true,
            iterations: 1,
            max_mismatch: 0.0,
            bus_results: vec![
                BusResult {
                    bus_id: 0,
                    voltage_magnitude: 1.0,
                    voltage_angle: 0.0,
                    p_injection: 0.0,
                    q_injection: 0.0,
                },
                BusResult {
                    bus_id: 1,
                    voltage_magnitude: 1.2,
                    voltage_angle: 0.0,
                    p_injection: 0.0,
                    q_injection: 0.0,
                },
            ],
            branch_results: vec![],
            total_losses: 0.0,
        };

        let violations = network.check_constraints(&result);
        assert!(violations.iter().any(|v| v.element_id == 20));
    }

    #[test]
    fn test_default_thermal_constraint_uses_directional_solver_branch_id() {
        let network = non_contiguous_test_network();
        let result = PowerFlowResult {
            converged: true,
            iterations: 1,
            max_mismatch: 0.0,
            bus_results: vec![],
            branch_results: vec![BranchResult {
                branch_id: 2,
                from_bus: 10,
                to_bus: 30,
                p_from: 0.0,
                q_from: 0.0,
                p_to: 0.0,
                q_to: 0.0,
                loss_mw: 0.0,
                loss_mvar: 0.0,
                loading_percent: 150.0,
            }],
            total_losses: 0.0,
        };

        let violations = network.check_constraints(&result);
        assert!(violations.iter().any(|v| v.element_id == 2));
    }
}
