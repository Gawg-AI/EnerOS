use eneros_core::ElementId;
use crate::graph::NetworkGraph;

/// Topology search results
#[derive(Debug, Clone)]
pub enum SearchResult {
    /// Path found between two buses
    Path(Vec<ElementId>),
    /// Connected zone (list of bus IDs)
    Zone(Vec<ElementId>),
    /// Cycle found
    Cycle(Vec<ElementId>),
    /// No result found
    None,
}

/// Topology searcher for advanced graph operations
pub struct TopologySearcher<'a> {
    graph: &'a NetworkGraph,
}

impl<'a> TopologySearcher<'a> {
    pub fn new(graph: &'a NetworkGraph) -> Self {
        Self { graph }
    }

    /// Find shortest path using BFS
    pub fn shortest_path(&self, from: ElementId, to: ElementId) -> SearchResult {
        match self.graph.find_path(from, to) {
            Some(path) => SearchResult::Path(path),
            None => SearchResult::None,
        }
    }

    /// Find all buses in the connected zone
    pub fn connected_zone(&self, bus_id: ElementId) -> SearchResult {
        let zone = self.graph.get_zone_buses(bus_id);
        SearchResult::Zone(zone)
    }

    /// Get bus IDs
    pub fn bus_ids(&self) -> Vec<ElementId> {
        self.graph.bus_ids()
    }

    /// Get adjacency edges for a bus
    pub fn get_edges(&self, bus_id: ElementId) -> Vec<(ElementId, ElementId)> {
        self.graph.get_edges(bus_id)
    }
}
