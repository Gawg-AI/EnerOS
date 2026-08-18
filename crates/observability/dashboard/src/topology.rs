//! 拓扑提供与查询（v0.190.0）.
//!
//! 定义 [`TopologyProvider`] 抽象与 [`MockTopology`] 样例实现，
//! 并提供节点查找 / 邻居查询等图算法辅助函数。

use alloc::string::String;
use alloc::vec::Vec;

#[cfg(test)]
use crate::types::{EdgeKind, FlowDirection, TopologyEdge};
use crate::types::{NodeKind, NodeStatus, Position3D, TopologyGraph, TopologyNode};

/// 拓扑提供者抽象.
pub trait TopologyProvider {
    /// 获取当前拓扑快照.
    fn snapshot(&self) -> TopologyGraph;
}

/// 注册表拓扑提供者（蓝图 41 WP-41.13）：从 Device/Service/Capability
/// registry 获取数据，timestamp 来自可信时间——不再固定 3 点/0 时间戳
/// 伪造健康。未知/降级状态如实呈现（NodeStatus::Unknown/Degraded）。
pub trait RegistrySource {
    /// 设备快照（registry 数据；空 = 无设备，图如实为空）。
    fn snapshot_devices(&self) -> alloc::vec::Vec<DeviceEntry>;
}

/// registry 设备条目（提供者输入）。
#[derive(Debug, Clone)]
pub struct DeviceEntry {
    pub device_id: u16,
    pub name: String,
    pub kind: NodeKind,
    pub healthy: bool,
    pub degraded: bool,
}

/// 基于注册表的正式拓扑提供者。
#[derive(Debug, Clone)]
pub struct RegistryTopologyProvider<R: RegistrySource> {
    pub registry: R,
    /// 可信时间源（now_ms）。
    pub now_ms: fn() -> u64,
}

impl<R: RegistrySource> RegistryTopologyProvider<R> {
    pub fn new(registry: R, now_ms: fn() -> u64) -> Self {
        RegistryTopologyProvider { registry, now_ms }
    }
}

impl<R: RegistrySource> TopologyProvider for RegistryTopologyProvider<R> {
    fn snapshot(&self) -> TopologyGraph {
        let devices = self.registry.snapshot_devices();
        let nodes = devices
            .iter()
            .map(|d| TopologyNode {
                node_id: d.device_id as u64,
                label: d.name.clone(),
                kind: d.kind,
                position: Position3D::new(0.0, 0.0, 0.0),
                // 如实：healthy → Healthy；degraded → Degraded；
                // 其余 → Unknown（不伪造 Healthy）。
                status: if d.healthy {
                    NodeStatus::Healthy
                } else if d.degraded {
                    NodeStatus::Degraded
                } else {
                    NodeStatus::Unknown
                },
            })
            .collect();
        TopologyGraph {
            nodes,
            edges: alloc::vec![],
            timestamp: (self.now_ms)(),
        }
    }
}

/// 测试/仿真专用模拟拓扑（蓝图 41：移入 test-support，release 不导出）。
#[cfg(any(test, feature = "mock"))]
#[derive(Debug, Clone, Default)]
pub struct MockTopology;

#[cfg(any(test, feature = "mock"))]
impl MockTopology {
    /// 构造一个空的模拟拓扑提供者.
    pub const fn new() -> Self {
        Self
    }
}

#[cfg(any(test, feature = "mock"))]
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
