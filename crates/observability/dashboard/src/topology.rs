//! 拓扑提供与查询（v0.190.0）.
//!
//! 定义 [`TopologyProvider`] 抽象与 [`MockTopology`] 样例实现，
//! 并提供节点查找 / 邻居查询等图算法辅助函数。

use alloc::string::String;
use alloc::vec::Vec;

use crate::types::{
    EdgeKind, FlowDirection, NodeKind, NodeStatus, Position3D, TopologyEdge, TopologyGraph,
    TopologyNode,
};

/// 拓扑提供者抽象.
pub trait TopologyProvider {
    /// 获取当前拓扑快照.
    fn snapshot(&self) -> TopologyGraph;
}

/// 模拟拓扑提供者：固定生成 3 节点 + 2 边的样例图.
#[derive(Debug, Clone, Default)]
pub struct MockTopology;

impl MockTopology {
    /// 构造一个空的模拟拓扑提供者.
    pub const fn new() -> Self {
        Self
    }
}

impl TopologyProvider for MockTopology {
    fn snapshot(&self) -> TopologyGraph {
        let nodes = alloc::vec![
            TopologyNode {
                node_id: 1,
                label: String::from("substation-A"),
                kind: NodeKind::Substation,
                position: Position3D::new(0.0, 0.0, 0.0),
                status: NodeStatus::Healthy,
            },
            TopologyNode {
                node_id: 2,
                label: String::from("grid-B"),
                kind: NodeKind::Grid,
                position: Position3D::new(10.0, 0.0, 0.0),
                status: NodeStatus::Warn,
            },
            TopologyNode {
                node_id: 3,
                label: String::from("market-C"),
                kind: NodeKind::Market,
                position: Position3D::new(5.0, 8.0, 0.0),
                status: NodeStatus::Healthy,
            },
        ];
        let edges = alloc::vec![
            TopologyEdge {
                from: 1,
                to: 2,
                kind: EdgeKind::Power,
                flow: 500.0,
                direction: FlowDirection::Forward,
            },
            TopologyEdge {
                from: 2,
                to: 3,
                kind: EdgeKind::Trade,
                flow: 120.0,
                direction: FlowDirection::Bidirectional,
            },
        ];
        TopologyGraph {
            nodes,
            edges,
            timestamp: 0,
        }
    }
}

/// 在图中查找指定 ID 的节点.
pub fn find_node(graph: &TopologyGraph, id: u64) -> Option<&TopologyNode> {
    graph.nodes.iter().find(|n| n.node_id == id)
}

/// 查询与指定节点直接相连的所有邻居节点 ID（去重）.
pub fn neighbors(graph: &TopologyGraph, id: u64) -> Vec<u64> {
    let mut result: Vec<u64> = Vec::new();
    for e in &graph.edges {
        let other = if e.from == id {
            e.to
        } else if e.to == id {
            e.from
        } else {
            continue;
        };
        if !result.contains(&other) {
            result.push(other);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_graph() -> TopologyGraph {
        MockTopology::new().snapshot()
    }

    #[test]
    fn mock_snapshot_shape() {
        let g = sample_graph();
        assert_eq!(g.nodes.len(), 3);
        assert_eq!(g.edges.len(), 2);
    }

    #[test]
    fn find_node_hit() {
        let g = sample_graph();
        let n = find_node(&g, 2).expect("node 2 exists");
        assert_eq!(n.kind, NodeKind::Grid);
        assert_eq!(n.status, NodeStatus::Warn);
    }

    #[test]
    fn find_node_miss() {
        let g = sample_graph();
        assert!(find_node(&g, 999).is_none());
    }

    #[test]
    fn neighbors_middle_node() {
        let g = sample_graph();
        let mut nb = neighbors(&g, 2);
        nb.sort_unstable();
        assert_eq!(nb, alloc::vec![1, 3]);
    }

    #[test]
    fn neighbors_leaf_node() {
        let g = sample_graph();
        assert_eq!(neighbors(&g, 1), alloc::vec![2]);
    }

    #[test]
    fn neighbors_isolated() {
        let g = sample_graph();
        assert!(neighbors(&g, 42).is_empty());
    }
}
