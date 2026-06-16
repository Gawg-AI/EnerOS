use std::sync::atomic::{AtomicU64, Ordering};
use parking_lot::RwLock;
use eneros_core::{ElementId, TopologyChange, Result};

use crate::graph::{Bus, Branch, Switch, NetworkGraph};

/// Topology engine for power grid network analysis
pub struct TopologyEngine {
    /// Network graph data
    graph: RwLock<NetworkGraph>,
    /// Version counter for incremental updates (atomic to avoid nested locking)
    version: AtomicU64,
}

impl TopologyEngine {
    /// Create a new topology engine
    pub fn new() -> Self {
        Self {
            graph: RwLock::new(NetworkGraph::new()),
            version: AtomicU64::new(0),
        }
    }

    /// Initialize the topology engine with network data
    pub fn initialize(
        &self,
        buses: Vec<Bus>,
        branches: Vec<Branch>,
        switches: Vec<Switch>,
    ) -> Result<()> {
        let mut graph = self.graph.write();
        graph.initialize(buses, branches, switches)?;
        drop(graph); // Release graph lock before incrementing version
        self.version.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// Check if two buses are connected
    pub fn is_connected(&self, bus1: ElementId, bus2: ElementId) -> bool {
        let graph = self.graph.read();
        graph.is_connected(bus1, bus2)
    }

    /// Find path between two buses
    pub fn find_path(&self, from: ElementId, to: ElementId) -> Option<Vec<ElementId>> {
        let graph = self.graph.read();
        graph.find_path(from, to)
    }

    /// Get all buses in the same zone
    pub fn get_zone_buses(&self, bus_id: ElementId) -> Vec<ElementId> {
        let graph = self.graph.read();
        graph.get_zone_buses(bus_id)
    }

    /// Apply a topology change
    pub fn apply_change(&self, change: TopologyChange) -> Result<()> {
        let mut graph = self.graph.write();
        graph.apply_change(change)?;
        drop(graph); // Release graph lock before incrementing version
        self.version.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// Apply multiple topology changes in batch
    pub fn apply_batch(&self, changes: Vec<TopologyChange>) -> Result<()> {
        let mut graph = self.graph.write();
        for change in changes {
            graph.apply_change(change)?;
        }
        drop(graph); // Release graph lock before incrementing version
        self.version.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// Get current topology version
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    /// Get network statistics
    pub fn statistics(&self) -> TopologyStatistics {
        let graph = self.graph.read();
        TopologyStatistics {
            bus_count: graph.bus_count(),
            branch_count: graph.branch_count(),
            switch_count: graph.switch_count(),
            zone_count: graph.zone_count(),
            version: self.version.load(Ordering::Acquire),
        }
    }
}

impl Default for TopologyEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Topology engine statistics
#[derive(Debug, Clone)]
pub struct TopologyStatistics {
    pub bus_count: usize,
    pub branch_count: usize,
    pub switch_count: usize,
    pub zone_count: usize,
    pub version: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_core::BusType;

    fn create_bus(id: ElementId, name: &str) -> Bus {
        Bus {
            id,
            name: name.to_string(),
            bus_type: BusType::PQ,
            voltage_kv: 110.0,
            zone_id: 0,
            bus_type_pf: BusType::PQ,
            p_gen: 0.0,
            q_gen: 0.0,
            p_load: 0.0,
            q_load: 0.0,
            v_pu: 1.0,
        }
    }

    fn create_branch(id: ElementId, name: &str, from: ElementId, to: ElementId) -> Branch {
        Branch {
            id,
            name: name.to_string(),
            from_bus: from,
            to_bus: to,
            branch_type: eneros_core::BranchType::Line,
            status: true,
            r: 0.01,
            x: 0.1,
            b: 0.01,
            tap_ratio: 1.0,
        }
    }

    #[test]
    fn test_engine_initialize() {
        let engine = TopologyEngine::new();
        engine.initialize(
            vec![create_bus(1, "B1"), create_bus(2, "B2")],
            vec![create_branch(1, "L1", 1, 2)],
            vec![],
        ).unwrap();

        let stats = engine.statistics();
        assert_eq!(stats.bus_count, 2);
        assert_eq!(stats.branch_count, 1);
        assert_eq!(stats.version, 1);
    }

    #[test]
    fn test_engine_version_increments() {
        let engine = TopologyEngine::new();
        assert_eq!(engine.version(), 0);

        engine.initialize(
            vec![create_bus(1, "B1")],
            vec![],
            vec![],
        ).unwrap();
        assert_eq!(engine.version(), 1);

        engine.apply_change(TopologyChange::BusAdded { bus_id: 2 }).unwrap();
        assert_eq!(engine.version(), 2);
    }

    #[test]
    fn test_engine_batch_change() {
        let engine = TopologyEngine::new();
        engine.initialize(
            vec![create_bus(1, "B1")],
            vec![],
            vec![],
        ).unwrap();

        let changes = vec![
            TopologyChange::BusAdded { bus_id: 2 },
            TopologyChange::BusAdded { bus_id: 3 },
        ];
        engine.apply_batch(changes).unwrap();

        assert_eq!(engine.statistics().bus_count, 3);
        // Batch should increment version only once
        assert_eq!(engine.version(), 2);
    }

    #[test]
    fn test_engine_concurrent_reads() {
        let engine = TopologyEngine::new();
        engine.initialize(
            vec![create_bus(1, "B1"), create_bus(2, "B2")],
            vec![create_branch(1, "L1", 1, 2)],
            vec![],
        ).unwrap();

        // Multiple reads should work concurrently
        assert!(engine.is_connected(1, 2));
        assert!(engine.find_path(1, 2).is_some());
        let zone = engine.get_zone_buses(1);
        assert_eq!(zone.len(), 2);
    }
}
