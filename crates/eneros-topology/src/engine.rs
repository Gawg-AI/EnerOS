use parking_lot::RwLock;
use eneros_core::{ElementId, TopologyChange, Result};

use crate::graph::{Bus, Branch, Switch, NetworkGraph};

/// Topology engine for power grid network analysis
pub struct TopologyEngine {
    /// Network graph data
    graph: RwLock<NetworkGraph>,
    /// Version counter for incremental updates
    version: RwLock<u64>,
}

impl TopologyEngine {
    /// Create a new topology engine
    pub fn new() -> Self {
        Self {
            graph: RwLock::new(NetworkGraph::new()),
            version: RwLock::new(0),
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
        *self.version.write() += 1;
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
        *self.version.write() += 1;
        Ok(())
    }

    /// Apply multiple topology changes in batch
    pub fn apply_batch(&self, changes: Vec<TopologyChange>) -> Result<()> {
        let mut graph = self.graph.write();
        for change in changes {
            graph.apply_change(change)?;
        }
        *self.version.write() += 1;
        Ok(())
    }

    /// Get current topology version
    pub fn version(&self) -> u64 {
        *self.version.read()
    }

    /// Get network statistics
    pub fn statistics(&self) -> TopologyStatistics {
        let graph = self.graph.read();
        TopologyStatistics {
            bus_count: graph.bus_count(),
            branch_count: graph.branch_count(),
            switch_count: graph.switch_count(),
            zone_count: graph.zone_count(),
            version: *self.version.read(),
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
