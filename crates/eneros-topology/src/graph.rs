use std::collections::{HashMap, VecDeque};
use eneros_core::{ElementId, BusType, BranchType, TopologyChange, Result, EnerOSError};
use eneros_core::{YBusMatrix, BusTypeNR};

/// Bus node in the power grid
#[derive(Debug, Clone)]
pub struct Bus {
    pub id: ElementId,
    pub name: String,
    pub bus_type: BusType,
    pub voltage_kv: f64,
    pub zone_id: u32,
    /// Bus type for power flow calculation (default: PQ)
    pub bus_type_pf: BusType,
    /// Generation active power in MW (default: 0.0)
    pub p_gen: f64,
    /// Generation reactive power in MVar (default: 0.0)
    pub q_gen: f64,
    /// Load active power in MW (default: 0.0)
    pub p_load: f64,
    /// Load reactive power in MVar (default: 0.0)
    pub q_load: f64,
    /// Voltage magnitude in per-unit (default: 1.0)
    pub v_pu: f64,
}

/// Branch connecting two buses
#[derive(Debug, Clone)]
pub struct Branch {
    pub id: ElementId,
    pub name: String,
    pub from_bus: ElementId,
    pub to_bus: ElementId,
    pub branch_type: BranchType,
    pub status: bool,
    pub r: f64,
    pub x: f64,
    pub b: f64,
    /// Transformer tap ratio (default: 1.0 for lines)
    pub tap_ratio: f64,
}

/// Switch device controlling branch connection
#[derive(Debug, Clone)]
pub struct Switch {
    pub id: ElementId,
    pub name: String,
    pub branch_id: ElementId,
    pub closed: bool,
}

/// Edge in the adjacency list
#[derive(Debug, Clone)]
struct Edge {
    to_bus: ElementId,
    branch_id: ElementId,
}

/// Network graph for topology analysis
pub struct NetworkGraph {
    buses: HashMap<ElementId, Bus>,
    branches: HashMap<ElementId, Branch>,
    switches: HashMap<ElementId, Switch>,
    adjacency: HashMap<ElementId, Vec<Edge>>,
}

impl NetworkGraph {
    pub fn new() -> Self {
        Self {
            buses: HashMap::new(),
            branches: HashMap::new(),
            switches: HashMap::new(),
            adjacency: HashMap::new(),
        }
    }

    /// Initialize the graph with network data
    pub fn initialize(
        &mut self,
        buses: Vec<Bus>,
        branches: Vec<Branch>,
        switches: Vec<Switch>,
    ) -> Result<()> {
        // Add buses
        for bus in buses {
            self.adjacency.entry(bus.id).or_default();
            self.buses.insert(bus.id, bus);
        }

        // Add branches
        for branch in branches {
            self.branches.insert(branch.id, branch.clone());
            if branch.status {
                self.add_branch_edges(&branch)?;
            }
        }

        // Add switches
        for switch in switches {
            self.switches.insert(switch.id, switch);
        }

        Ok(())
    }

    /// Add edges for a branch
    fn add_branch_edges(&mut self, branch: &Branch) -> Result<()> {
        self.adjacency
            .entry(branch.from_bus)
            .or_default()
            .push(Edge {
                to_bus: branch.to_bus,
                branch_id: branch.id,
            });
        self.adjacency
            .entry(branch.to_bus)
            .or_default()
            .push(Edge {
                to_bus: branch.from_bus,
                branch_id: branch.id,
            });
        Ok(())
    }

    /// Remove edges for a branch
    fn remove_branch_edges(&mut self, branch: &Branch) {
        if let Some(edges) = self.adjacency.get_mut(&branch.from_bus) {
            edges.retain(|e| e.branch_id != branch.id);
        }
        if let Some(edges) = self.adjacency.get_mut(&branch.to_bus) {
            edges.retain(|e| e.branch_id != branch.id);
        }
    }

    /// Check if two buses are connected
    pub fn is_connected(&self, bus1: ElementId, bus2: ElementId) -> bool {
        if bus1 == bus2 {
            return true;
        }

        let mut visited = std::collections::HashSet::new();
        let mut queue = VecDeque::new();

        visited.insert(bus1);
        queue.push_back(bus1);

        while let Some(current) = queue.pop_front() {
            if let Some(edges) = self.adjacency.get(&current) {
                for edge in edges {
                    if edge.to_bus == bus2 {
                        return true;
                    }
                    if !visited.contains(&edge.to_bus) {
                        visited.insert(edge.to_bus);
                        queue.push_back(edge.to_bus);
                    }
                }
            }
        }

        false
    }

    /// Find path between two buses using BFS
    pub fn find_path(&self, from: ElementId, to: ElementId) -> Option<Vec<ElementId>> {
        if from == to {
            return Some(vec![from]);
        }

        let mut visited = std::collections::HashSet::new();
        let mut queue = VecDeque::new();
        let mut parent: HashMap<ElementId, ElementId> = HashMap::new();

        visited.insert(from);
        queue.push_back(from);

        while let Some(current) = queue.pop_front() {
            if let Some(edges) = self.adjacency.get(&current) {
                for edge in edges {
                    if edge.to_bus == to {
                        // Reconstruct path
                        let mut path = vec![to, current];
                        let mut node = current;
                        while let Some(&p) = parent.get(&node) {
                            path.push(p);
                            node = p;
                        }
                        path.reverse();
                        return Some(path);
                    }
                    if !visited.contains(&edge.to_bus) {
                        visited.insert(edge.to_bus);
                        parent.insert(edge.to_bus, current);
                        queue.push_back(edge.to_bus);
                    }
                }
            }
        }

        None
    }

    /// Get all buses in the same zone (connected component)
    pub fn get_zone_buses(&self, bus_id: ElementId) -> Vec<ElementId> {
        let mut visited = std::collections::HashSet::new();
        let mut queue = VecDeque::new();
        let mut zone_buses = Vec::new();

        visited.insert(bus_id);
        queue.push_back(bus_id);

        while let Some(current) = queue.pop_front() {
            zone_buses.push(current);
            if let Some(edges) = self.adjacency.get(&current) {
                for edge in edges {
                    if !visited.contains(&edge.to_bus) {
                        visited.insert(edge.to_bus);
                        queue.push_back(edge.to_bus);
                    }
                }
            }
        }

        zone_buses
    }

    /// Apply a topology change
    pub fn apply_change(&mut self, change: TopologyChange) -> Result<()> {
        match change {
            TopologyChange::SwitchToggle { switch_id, closed } => {
                if let Some(switch) = self.switches.get_mut(&switch_id) {
                    switch.closed = closed;
                    if let Some(branch) = self.branches.get(&switch.branch_id) {
                        let branch = branch.clone();
                        if closed {
                            self.add_branch_edges(&branch)?;
                        } else {
                            self.remove_branch_edges(&branch);
                        }
                    }
                } else {
                    return Err(EnerOSError::Topology(format!(
                        "Switch {} not found",
                        switch_id
                    )));
                }
            }
            TopologyChange::BranchAdded { branch_id, params } => {
                let branch = Branch {
                    id: branch_id,
                    name: params.name.unwrap_or_else(|| format!("Branch{}", branch_id)),
                    from_bus: params.from_bus,
                    to_bus: params.to_bus,
                    branch_type: params.branch_type,
                    status: true,
                    r: params.r,
                    x: params.x,
                    b: params.b,
                    tap_ratio: 1.0,
                };
                self.branches.insert(branch_id, branch.clone());
                self.add_branch_edges(&branch)?;
            }
            TopologyChange::BranchRemoved { branch_id } => {
                if let Some(branch) = self.branches.remove(&branch_id) {
                    self.remove_branch_edges(&branch);
                }
            }
            TopologyChange::BusAdded { bus_id } => {
                self.buses.entry(bus_id).or_insert(Bus {
                    id: bus_id,
                    name: format!("Bus{}", bus_id),
                    bus_type: BusType::PQ,
                    voltage_kv: 110.0,
                    zone_id: 0,
                    bus_type_pf: BusType::PQ,
                    p_gen: 0.0,
                    q_gen: 0.0,
                    p_load: 0.0,
                    q_load: 0.0,
                    v_pu: 1.0,
                });
                self.adjacency.entry(bus_id).or_default();
            }
            TopologyChange::BusRemoved { bus_id } => {
                self.buses.remove(&bus_id);
                self.adjacency.remove(&bus_id);
                // Remove all edges to this bus
                for edges in self.adjacency.values_mut() {
                    edges.retain(|e| e.to_bus != bus_id);
                }
            }
        }
        Ok(())
    }

    /// Get bus count
    pub fn bus_count(&self) -> usize {
        self.buses.len()
    }

    /// Get branch count
    pub fn branch_count(&self) -> usize {
        self.branches.len()
    }

    /// Get switch count
    pub fn switch_count(&self) -> usize {
        self.switches.len()
    }

    /// Get zone count (connected components)
    pub fn zone_count(&self) -> usize {
        let mut visited = std::collections::HashSet::new();
        let mut count = 0;

        for &bus_id in self.buses.keys() {
            if !visited.contains(&bus_id) {
                count += 1;
                let zone = self.get_zone_buses(bus_id);
                for id in zone {
                    visited.insert(id);
                }
            }
        }

        count
    }

    /// Get all bus IDs
    pub fn bus_ids(&self) -> Vec<ElementId> {
        self.buses.keys().copied().collect()
    }

    /// Export network data for power flow solver
    /// Returns (YBusMatrix, p_spec, q_spec, bus_types, v_initial)
    pub fn to_solver_input(&self, base_mva: f64) -> (YBusMatrix, Vec<f64>, Vec<f64>, Vec<BusTypeNR>, Vec<f64>) {
        // Build sorted bus list and bus_map
        let mut bus_ids: Vec<ElementId> = self.buses.keys().copied().collect();
        bus_ids.sort();
        let bus_map: HashMap<ElementId, usize> = bus_ids.iter()
            .enumerate()
            .map(|(i, &id)| (id, i))
            .collect();

        // Build branch data: (from, to, r, x, b, tap_ratio)
        let branch_data: Vec<(ElementId, ElementId, f64, f64, f64, f64)> = self.branches.values()
            .filter(|b| b.status)
            .map(|b| (b.from_bus, b.to_bus, b.r, b.x, b.b, b.tap_ratio))
            .collect();

        // Build Y-Bus matrix
        let ybus = YBusMatrix::from_branches(&branch_data, &bus_map);

        // Build per-bus vectors
        let n = bus_ids.len();
        let mut p_spec = vec![0.0; n];
        let mut q_spec = vec![0.0; n];
        let mut bus_types = vec![BusTypeNR::PQ; n];
        let mut v_initial = vec![1.0; n];

        for (&bus_id, &idx) in &bus_map {
            if let Some(bus) = self.buses.get(&bus_id) {
                p_spec[idx] = (bus.p_gen - bus.p_load) / base_mva;
                q_spec[idx] = (bus.q_gen - bus.q_load) / base_mva;
                bus_types[idx] = match bus.bus_type_pf {
                    BusType::PQ => BusTypeNR::PQ,
                    BusType::PV => BusTypeNR::PV,
                    BusType::Slack => BusTypeNR::Slack,
                };
                v_initial[idx] = bus.v_pu;
            }
        }

        (ybus, p_spec, q_spec, bus_types, v_initial)
    }

    /// Get online branches as (from_bus, to_bus, r, x, b, tap_ratio) tuples
    pub fn online_branches(&self) -> Vec<(ElementId, ElementId, f64, f64, f64, f64)> {
        self.branches.values()
            .filter(|b| b.status)
            .map(|b| (b.from_bus, b.to_bus, b.r, b.x, b.b, b.tap_ratio))
            .collect()
    }

    /// Get edges for a bus
    pub fn get_edges(&self, bus_id: ElementId) -> Vec<(ElementId, ElementId)> {
        self.adjacency
            .get(&bus_id)
            .map(|edges| edges.iter().map(|e| (bus_id, e.to_bus)).collect())
            .unwrap_or_default()
    }

    /// Detect if the graph contains any cycle using DFS back-edge detection.
    /// For undirected graphs, we skip the parent node to avoid false positives
    /// from bidirectional edges.
    pub fn has_cycle(&self) -> bool {
        let mut visited = std::collections::HashSet::new();

        for &bus_id in self.buses.keys() {
            if !visited.contains(&bus_id)
                && self.dfs_cycle_undirected(bus_id, None, &mut visited)
            {
                return true;
            }
        }
        false
    }

    /// DFS helper for cycle detection in undirected graph
    fn dfs_cycle_undirected(
        &self,
        bus_id: ElementId,
        parent: Option<ElementId>,
        visited: &mut std::collections::HashSet<ElementId>,
    ) -> bool {
        visited.insert(bus_id);

        if let Some(edges) = self.adjacency.get(&bus_id) {
            for edge in edges {
                if !visited.contains(&edge.to_bus) {
                    if self.dfs_cycle_undirected(edge.to_bus, Some(bus_id), visited) {
                        return true;
                    }
                } else if parent != Some(edge.to_bus) {
                    // Visited node that is not the parent = cycle
                    return true;
                }
            }
        }

        false
    }

    /// Find a cycle in the graph, returning the bus IDs forming the cycle.
    /// For undirected graphs, we skip the parent node to avoid false positives.
    pub fn find_cycle(&self) -> Option<Vec<ElementId>> {
        let mut visited = std::collections::HashSet::new();
        let mut path = Vec::new();

        for &bus_id in self.buses.keys() {
            if !visited.contains(&bus_id) {
                if let Some(cycle) = self.dfs_find_cycle_undirected(bus_id, None, &mut visited, &mut path) {
                    return Some(cycle);
                }
            }
        }
        None
    }

    fn dfs_find_cycle_undirected(
        &self,
        bus_id: ElementId,
        parent: Option<ElementId>,
        visited: &mut std::collections::HashSet<ElementId>,
        path: &mut Vec<ElementId>,
    ) -> Option<Vec<ElementId>> {
        visited.insert(bus_id);
        path.push(bus_id);

        if let Some(edges) = self.adjacency.get(&bus_id) {
            for edge in edges {
                if !visited.contains(&edge.to_bus) {
                    if let Some(cycle) = self.dfs_find_cycle_undirected(edge.to_bus, Some(bus_id), visited, path) {
                        return Some(cycle);
                    }
                } else if parent != Some(edge.to_bus) {
                    // Found a cycle - extract it from the path
                    let start_idx = path.iter().position(|&id| id == edge.to_bus).unwrap_or(0);
                    let cycle: Vec<ElementId> = path[start_idx..].to_vec();
                    return Some(cycle);
                }
            }
        }

        path.pop();
        None
    }
}

impl Default for NetworkGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper function to create a test bus
    fn create_bus(id: ElementId, name: &str, voltage_kv: f64) -> Bus {
        Bus {
            id,
            name: name.to_string(),
            bus_type: BusType::PQ,
            voltage_kv,
            zone_id: 0,
            bus_type_pf: BusType::PQ,
            p_gen: 0.0,
            q_gen: 0.0,
            p_load: 0.0,
            q_load: 0.0,
            v_pu: 1.0,
        }
    }

    /// Helper function to create a test branch
    fn create_branch(id: ElementId, name: &str, from_bus: ElementId, to_bus: ElementId) -> Branch {
        Branch {
            id,
            name: name.to_string(),
            from_bus,
            to_bus,
            branch_type: BranchType::Line,
            status: true,
            r: 0.01,
            x: 0.1,
            b: 0.01,
            tap_ratio: 1.0,
        }
    }

    /// Helper function to create a test switch
    fn create_switch(id: ElementId, name: &str, branch_id: ElementId, closed: bool) -> Switch {
        Switch {
            id,
            name: name.to_string(),
            branch_id,
            closed,
        }
    }

    /// Build a simple 4-bus test network:
    ///   1 --- 2
    ///   |     |
    ///   3 --- 4
    fn build_test_network() -> NetworkGraph {
        let mut graph = NetworkGraph::new();

        let buses = vec![
            create_bus(1, "Bus1", 110.0),
            create_bus(2, "Bus2", 110.0),
            create_bus(3, "Bus3", 110.0),
            create_bus(4, "Bus4", 110.0),
        ];

        let branches = vec![
            create_branch(1, "Line1-2", 1, 2),
            create_branch(2, "Line1-3", 1, 3),
            create_branch(3, "Line2-4", 2, 4),
            create_branch(4, "Line3-4", 3, 4),
        ];

        graph.initialize(buses, branches, vec![]).unwrap();
        graph
    }

    #[test]
    fn test_new_graph() {
        let graph = NetworkGraph::new();
        assert_eq!(graph.bus_count(), 0);
        assert_eq!(graph.branch_count(), 0);
        assert_eq!(graph.switch_count(), 0);
    }

    #[test]
    fn test_initialize_graph() {
        let graph = build_test_network();
        assert_eq!(graph.bus_count(), 4);
        assert_eq!(graph.branch_count(), 4);
    }

    #[test]
    fn test_is_connected_same_bus() {
        let graph = build_test_network();
        assert!(graph.is_connected(1, 1));
    }

    #[test]
    fn test_is_connected_adjacent_buses() {
        let graph = build_test_network();
        assert!(graph.is_connected(1, 2));
        assert!(graph.is_connected(1, 3));
        assert!(graph.is_connected(2, 4));
        assert!(graph.is_connected(3, 4));
    }

    #[test]
    fn test_is_connected_non_adjacent_buses() {
        let graph = build_test_network();
        assert!(graph.is_connected(1, 4));
        assert!(graph.is_connected(2, 3));
    }

    #[test]
    fn test_find_path_direct() {
        let graph = build_test_network();
        let path = graph.find_path(1, 2).unwrap();
        assert_eq!(path, vec![1, 2]);
    }

    #[test]
    fn test_find_path_indirect() {
        let graph = build_test_network();
        let path = graph.find_path(1, 4).unwrap();
        assert!(path.contains(&1));
        assert!(path.contains(&4));
        assert!(path.len() >= 3);
    }

    #[test]
    fn test_find_path_same_bus() {
        let graph = build_test_network();
        let path = graph.find_path(1, 1).unwrap();
        assert_eq!(path, vec![1]);
    }

    #[test]
    fn test_get_zone_buses() {
        let graph = build_test_network();
        let zone = graph.get_zone_buses(1);
        assert_eq!(zone.len(), 4);
        assert!(zone.contains(&1));
        assert!(zone.contains(&2));
        assert!(zone.contains(&3));
        assert!(zone.contains(&4));
    }

    #[test]
    fn test_zone_count_single_zone() {
        let graph = build_test_network();
        assert_eq!(graph.zone_count(), 1);
    }

    #[test]
    fn test_zone_count_multiple_zones() {
        let mut graph = NetworkGraph::new();

        // Zone 1: Bus 1, 2
        let buses = vec![
            create_bus(1, "Bus1", 110.0),
            create_bus(2, "Bus2", 110.0),
            create_bus(3, "Bus3", 110.0), // Isolated
        ];

        let branches = vec![create_branch(1, "Line1-2", 1, 2)];

        graph.initialize(buses, branches, vec![]).unwrap();
        assert_eq!(graph.zone_count(), 2);
    }

    #[test]
    fn test_switch_toggle_open() {
        let mut graph = NetworkGraph::new();

        let buses = vec![
            create_bus(1, "Bus1", 110.0),
            create_bus(2, "Bus2", 110.0),
        ];

        let branches = vec![create_branch(1, "Line1-2", 1, 2)];

        let switches = vec![create_switch(1, "SW1", 1, true)];

        graph.initialize(buses, branches, switches).unwrap();
        assert!(graph.is_connected(1, 2));

        // Open the switch
        graph
            .apply_change(TopologyChange::SwitchToggle {
                switch_id: 1,
                closed: false,
            })
            .unwrap();

        assert!(!graph.is_connected(1, 2));
    }

    #[test]
    fn test_switch_toggle_close() {
        let mut graph = NetworkGraph::new();

        let buses = vec![
            create_bus(1, "Bus1", 110.0),
            create_bus(2, "Bus2", 110.0),
        ];

        // Create branch with status false (controlled by switch)
        let mut branch = create_branch(1, "Line1-2", 1, 2);
        branch.status = false; // Not connected initially

        let switches = vec![create_switch(1, "SW1", 1, false)];

        graph.initialize(buses, vec![branch], switches).unwrap();
        assert!(!graph.is_connected(1, 2));

        // Close the switch - should add edges
        graph
            .apply_change(TopologyChange::SwitchToggle {
                switch_id: 1,
                closed: true,
            })
            .unwrap();

        assert!(graph.is_connected(1, 2));
    }

    #[test]
    fn test_switch_not_found() {
        let mut graph = NetworkGraph::new();

        let buses = vec![
            create_bus(1, "Bus1", 110.0),
            create_bus(2, "Bus2", 110.0),
        ];

        graph.initialize(buses, vec![], vec![]).unwrap();

        let result = graph.apply_change(TopologyChange::SwitchToggle {
            switch_id: 999,
            closed: true,
        });

        assert!(result.is_err());
    }

    #[test]
    fn test_bus_removed() {
        let mut graph = build_test_network();
        assert_eq!(graph.bus_count(), 4);

        graph
            .apply_change(TopologyChange::BusRemoved { bus_id: 4 })
            .unwrap();

        assert_eq!(graph.bus_count(), 3);
        assert!(!graph.is_connected(1, 4));
    }

    #[test]
    fn test_bus_added() {
        let mut graph = build_test_network();
        assert_eq!(graph.bus_count(), 4);

        graph
            .apply_change(TopologyChange::BusAdded { bus_id: 5 })
            .unwrap();

        assert_eq!(graph.bus_count(), 5);
    }

    #[test]
    fn test_get_edges() {
        let graph = build_test_network();
        let edges = graph.get_edges(1);
        assert_eq!(edges.len(), 2);

        let edge_targets: Vec<ElementId> = edges.iter().map(|(_, to)| *to).collect();
        assert!(edge_targets.contains(&2));
        assert!(edge_targets.contains(&3));
    }

    #[test]
    fn test_bus_ids() {
        let graph = build_test_network();
        let ids = graph.bus_ids();
        assert_eq!(ids.len(), 4);
        assert!(ids.contains(&1));
        assert!(ids.contains(&2));
        assert!(ids.contains(&3));
        assert!(ids.contains(&4));
    }

    #[test]
    fn test_disconnected_network() {
        let mut graph = NetworkGraph::new();

        let buses = vec![
            create_bus(1, "Bus1", 110.0),
            create_bus(2, "Bus2", 110.0),
            create_bus(3, "Bus3", 220.0),
            create_bus(4, "Bus4", 220.0),
        ];

        // Two separate networks: 1-2 and 3-4
        let branches = vec![
            create_branch(1, "Line1-2", 1, 2),
            create_branch(2, "Line3-4", 3, 4),
        ];

        graph.initialize(buses, branches, vec![]).unwrap();

        assert!(graph.is_connected(1, 2));
        assert!(graph.is_connected(3, 4));
        assert!(!graph.is_connected(1, 3));
        assert!(!graph.is_connected(2, 4));
        assert_eq!(graph.zone_count(), 2);
    }

    #[test]
    fn test_path_not_found() {
        let mut graph = NetworkGraph::new();

        let buses = vec![
            create_bus(1, "Bus1", 110.0),
            create_bus(2, "Bus2", 110.0),
        ];

        graph.initialize(buses, vec![], vec![]).unwrap();

        let path = graph.find_path(1, 2);
        assert!(path.is_none());
    }

    #[test]
    fn test_has_cycle_mesh_network() {
        // Mesh network (1-2-4-3-1) has a cycle
        let graph = build_test_network();
        assert!(graph.has_cycle());
    }

    #[test]
    fn test_has_cycle_radial_network() {
        // Radial (tree) network has no cycle: 1-2, 2-3
        let mut graph = NetworkGraph::new();
        let buses = vec![
            create_bus(1, "Bus1", 110.0),
            create_bus(2, "Bus2", 110.0),
            create_bus(3, "Bus3", 110.0),
        ];
        let branches = vec![
            create_branch(1, "Line1-2", 1, 2),
            create_branch(2, "Line2-3", 2, 3),
        ];
        graph.initialize(buses, branches, vec![]).unwrap();
        assert!(!graph.has_cycle());
    }

    #[test]
    fn test_find_cycle_mesh() {
        let graph = build_test_network();
        let cycle = graph.find_cycle();
        assert!(cycle.is_some());
        let cycle = cycle.unwrap();
        assert!(cycle.len() >= 3);
    }

    #[test]
    fn test_find_cycle_tree() {
        let mut graph = NetworkGraph::new();
        let buses = vec![
            create_bus(1, "Bus1", 110.0),
            create_bus(2, "Bus2", 110.0),
        ];
        let branches = vec![create_branch(1, "Line1-2", 1, 2)];
        graph.initialize(buses, branches, vec![]).unwrap();
        assert!(graph.find_cycle().is_none());
    }

    #[test]
    fn test_branch_added_with_params() {
        let mut graph = NetworkGraph::new();
        let buses = vec![
            create_bus(1, "Bus1", 110.0),
            create_bus(2, "Bus2", 110.0),
        ];
        graph.initialize(buses, vec![], vec![]).unwrap();
        assert_eq!(graph.branch_count(), 0);
        assert!(!graph.is_connected(1, 2));

        graph.apply_change(TopologyChange::BranchAdded {
            branch_id: 10,
            params: eneros_core::BranchParams {
                r: 0.01,
                x: 0.1,
                b: 0.01,
                rate_mva: 100.0,
                name: Some("NewLine".into()),
                branch_type: BranchType::Line,
                from_bus: 1,
                to_bus: 2,
            },
        }).unwrap();

        assert_eq!(graph.branch_count(), 1);
        assert!(graph.is_connected(1, 2));
    }
}
