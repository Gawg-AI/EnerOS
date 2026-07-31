//! 仪表盘数据模型（v0.190.0）.
//!
//! 定义 3D 拓扑图、节点/边、功率流采样等核心数据结构。
//! 3D 坐标使用 `f32`，功率/指标使用 `f64`，ID/时间戳使用 `u64`。

use alloc::string::String;
use alloc::vec::Vec;

/// 节点类型.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// 智能体节点.
    Agent,
    /// 资产节点.
    Asset,
    /// 市场节点.
    Market,
    /// 电网节点.
    Grid,
    /// 变电站节点.
    Substation,
}

/// 节点状态.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus {
    /// 健康.
    Healthy,
    /// 告警.
    Warn,
    /// 故障.
    Fault,
    /// 离线.
    Offline,
}

/// 边类型.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// 功率流.
    Power,
    /// 数据流.
    Data,
    /// 交易流.
    Trade,
}

/// 流向.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowDirection {
    /// 正向.
    Forward,
    /// 反向.
    Reverse,
    /// 双向.
    Bidirectional,
}

/// 3D 坐标（f32）.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position3D {
    /// X 坐标.
    pub x: f32,
    /// Y 坐标.
    pub y: f32,
    /// Z 坐标.
    pub z: f32,
}

impl Position3D {
    /// 构造一个新的 3D 坐标.
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
}

/// 拓扑节点.
#[derive(Debug, Clone, PartialEq)]
pub struct TopologyNode {
    /// 节点 ID.
    pub node_id: u64,
    /// 节点标签.
    pub label: String,
    /// 节点类型.
    pub kind: NodeKind,
    /// 3D 位置.
    pub position: Position3D,
    /// 节点状态.
    pub status: NodeStatus,
}

/// 拓扑边.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TopologyEdge {
    /// 起点节点 ID.
    pub from: u64,
    /// 终点节点 ID.
    pub to: u64,
    /// 边类型.
    pub kind: EdgeKind,
    /// 流量（功率 kW / 数据速率 / 交易额）.
    pub flow: f64,
    /// 流向.
    pub direction: FlowDirection,
}

/// 拓扑图快照.
#[derive(Debug, Clone, PartialEq)]
pub struct TopologyGraph {
    /// 节点集合.
    pub nodes: Vec<TopologyNode>,
    /// 边集合.
    pub edges: Vec<TopologyEdge>,
    /// 快照时间戳（毫秒）.
    pub timestamp: u64,
}

/// 功率流采样点.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PowerFlowSample {
    /// 起点节点 ID.
    pub from: u64,
    /// 终点节点 ID.
    pub to: u64,
    /// 功率（kW）.
    pub power_kw: f64,
    /// 电压（V）.
    pub voltage: f64,
    /// 电流（A）.
    pub current: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position3d_new() {
        let p = Position3D::new(1.0, 2.0, 3.0);
        assert_eq!(p.x, 1.0);
        assert_eq!(p.y, 2.0);
        assert_eq!(p.z, 3.0);
    }

    #[test]
    fn node_kind_eq() {
        assert_eq!(NodeKind::Agent, NodeKind::Agent);
        assert_ne!(NodeKind::Agent, NodeKind::Market);
    }

    #[test]
    fn node_status_variants() {
        let s = NodeStatus::Fault;
        assert_eq!(s, NodeStatus::Fault);
        assert_ne!(s, NodeStatus::Healthy);
    }

    #[test]
    fn edge_kind_and_direction() {
        assert_eq!(EdgeKind::Power, EdgeKind::Power);
        assert_eq!(FlowDirection::Bidirectional, FlowDirection::Bidirectional);
    }

    #[test]
    fn topology_node_clone() {
        let n = TopologyNode {
            node_id: 1,
            label: String::from("node-1"),
            kind: NodeKind::Substation,
            position: Position3D::new(0.0, 0.0, 0.0),
            status: NodeStatus::Healthy,
        };
        let c = n.clone();
        assert_eq!(n, c);
    }

    #[test]
    fn topology_edge_copy() {
        let e = TopologyEdge {
            from: 1,
            to: 2,
            kind: EdgeKind::Power,
            flow: 100.0,
            direction: FlowDirection::Forward,
        };
        let c = e;
        assert_eq!(e, c);
    }

    #[test]
    fn power_flow_sample_fields() {
        let s = PowerFlowSample {
            from: 1,
            to: 2,
            power_kw: 220.0,
            voltage: 220.0,
            current: 1.0,
        };
        assert_eq!(s.power_kw, 220.0);
        assert_eq!(s.current, 1.0);
    }
}
