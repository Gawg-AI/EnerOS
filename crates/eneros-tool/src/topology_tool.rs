use async_trait::async_trait;
use eneros_core::Result;
use parking_lot::RwLock;
use std::sync::Arc;

use crate::tool::{Tool, ToolOutput};

pub struct TopologyQueryTool {
    graph: Arc<RwLock<eneros_topology::NetworkGraph>>,
}

impl TopologyQueryTool {
    pub fn new(graph: Arc<RwLock<eneros_topology::NetworkGraph>>) -> Self {
        Self { graph }
    }
}

#[async_trait]
impl Tool for TopologyQueryTool {
    fn name(&self) -> &str {
        "topology_query"
    }

    fn description(&self) -> &str {
        "Query network topology: is_connected, find_path, zone_count, has_cycle"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "enum": ["is_connected", "find_path", "zone_count", "has_cycle"] },
                "bus1": { "type": "integer", "description": "First bus ID (for is_connected)" },
                "bus2": { "type": "integer", "description": "Second bus ID (for is_connected)" },
                "from": { "type": "integer", "description": "Source bus ID (for find_path)" },
                "to": { "type": "integer", "description": "Target bus ID (for find_path)" }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolOutput> {
        let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let graph = self.graph.read();

        match query {
            "is_connected" => {
                let bus1 = params
                    .get("bus1")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as eneros_core::ElementId;
                let bus2 = params
                    .get("bus2")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as eneros_core::ElementId;
                let connected = graph.is_connected(bus1, bus2);
                Ok(ToolOutput::ok(
                    serde_json::json!({ "connected": connected }),
                    &format!(
                        "Buses {} and {} are {}",
                        bus1,
                        bus2,
                        if connected { "connected" } else { "not connected" }
                    ),
                ))
            }
            "find_path" => {
                let from = params
                    .get("from")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as eneros_core::ElementId;
                let to = params
                    .get("to")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as eneros_core::ElementId;
                match graph.find_path(from, to) {
                    Some(path) => Ok(ToolOutput::ok(
                        serde_json::json!({ "path": path }),
                        &format!("Path found: {:?}", path),
                    )),
                    None => Ok(ToolOutput::ok(
                        serde_json::json!({ "path": null }),
                        &format!("No path from {} to {}", from, to),
                    )),
                }
            }
            "zone_count" => {
                let count = graph.zone_count();
                Ok(ToolOutput::ok(
                    serde_json::json!({ "zone_count": count }),
                    &format!("Network has {} connected zones", count),
                ))
            }
            "has_cycle" => {
                let has_cycle = graph.has_cycle();
                Ok(ToolOutput::ok(
                    serde_json::json!({ "has_cycle": has_cycle }),
                    &format!(
                        "Network {} cycle(s)",
                        if has_cycle { "has" } else { "has no" }
                    ),
                ))
            }
            _ => Ok(ToolOutput::err(&format!("Unknown query: {}", query))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_core::{BranchType, BusType};
    use eneros_topology::{Branch, Bus};

    fn create_bus(id: eneros_core::ElementId, name: &str) -> Bus {
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

    fn create_branch(id: eneros_core::ElementId, name: &str, from: eneros_core::ElementId, to: eneros_core::ElementId) -> Branch {
        Branch {
            id,
            name: name.to_string(),
            from_bus: from,
            to_bus: to,
            branch_type: BranchType::Line,
            status: true,
            r: 0.01,
            x: 0.1,
            b: 0.01,
            tap_ratio: 1.0,
        }
    }

    fn build_simple_graph() -> Arc<RwLock<eneros_topology::NetworkGraph>> {
        let mut graph = eneros_topology::NetworkGraph::new();
        let buses = vec![create_bus(1, "Bus1"), create_bus(2, "Bus2"), create_bus(3, "Bus3")];
        let branches = vec![
            create_branch(1, "Line1-2", 1, 2),
            create_branch(2, "Line2-3", 2, 3),
        ];
        graph.initialize(buses, branches, vec![]).unwrap();
        Arc::new(RwLock::new(graph))
    }

    #[tokio::test]
    async fn test_is_connected_true() {
        let graph = build_simple_graph();
        let tool = TopologyQueryTool::new(graph);
        let result = tool
            .execute(serde_json::json!({ "query": "is_connected", "bus1": 1, "bus2": 3 }))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.data["connected"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_is_connected_same_bus() {
        let graph = build_simple_graph();
        let tool = TopologyQueryTool::new(graph);
        let result = tool
            .execute(serde_json::json!({ "query": "is_connected", "bus1": 1, "bus2": 1 }))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.data["connected"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_find_path() {
        let graph = build_simple_graph();
        let tool = TopologyQueryTool::new(graph);
        let result = tool
            .execute(serde_json::json!({ "query": "find_path", "from": 1, "to": 3 }))
            .await
            .unwrap();
        assert!(result.success);
        let path = result.data["path"].as_array().unwrap();
        let path_ids: Vec<eneros_core::ElementId> = path.iter().map(|v| v.as_u64().unwrap()).collect();
        assert_eq!(path_ids, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn test_zone_count() {
        let graph = build_simple_graph();
        let tool = TopologyQueryTool::new(graph);
        let result = tool
            .execute(serde_json::json!({ "query": "zone_count" }))
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.data["zone_count"].as_u64().unwrap(), 1);
    }

    #[tokio::test]
    async fn test_has_cycle_false() {
        let graph = build_simple_graph();
        let tool = TopologyQueryTool::new(graph);
        let result = tool
            .execute(serde_json::json!({ "query": "has_cycle" }))
            .await
            .unwrap();
        assert!(result.success);
        assert!(!result.data["has_cycle"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_unknown_query() {
        let graph = build_simple_graph();
        let tool = TopologyQueryTool::new(graph);
        let result = tool
            .execute(serde_json::json!({ "query": "invalid_query" }))
            .await
            .unwrap();
        assert!(!result.success);
    }
}
