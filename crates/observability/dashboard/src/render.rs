//! 仪表盘事件与 WebSocket 推送（v0.190.0）.
//!
//! 定义 [`DashboardEvent`] 推送事件、[`DashboardWsServer`] 广播抽象。
//! 真 WebSocket 服务端在 [`crate::ws::WsServer`]（RFC 6455 实现）；
//! `MockWsServer` 仅 `#[cfg(test)]` 测试替身（验收：非生产代码）。

use alloc::vec::Vec;

use crate::types::{NodeStatus, PowerFlowSample, TopologyGraph};
use crate::ObsError;

/// 仪表盘推送事件.
#[derive(Debug, Clone, PartialEq)]
pub enum DashboardEvent {
    /// 拓扑图全量更新.
    TopologyUpdate(TopologyGraph),
    /// 功率流采样更新.
    PowerFlowUpdate(Vec<PowerFlowSample>),
    /// 单节点状态变更（节点 ID + 新状态）.
    NodeStatusChange(u64, NodeStatus),
}

/// 仪表盘 WebSocket 服务端抽象.
pub trait DashboardWsServer {
    /// 向所有订阅客户端广播事件.
    fn broadcast(&self, event: &DashboardEvent) -> Result<(), ObsError>;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]
    use alloc::string::String;
    use core::cell::RefCell;

    use super::*;
    use crate::types::{EdgeKind, FlowDirection, NodeKind, Position3D, TopologyEdge, TopologyNode};

    /// 模拟 WebSocket 服务端（仅测试替身）：记录最近一次广播事件与计数.
    #[derive(Debug, Default)]
    struct MockWsServer {
        last: RefCell<Option<DashboardEvent>>,
        count: RefCell<u64>,
    }

    impl MockWsServer {
        fn new() -> Self {
            Self::default()
        }

        fn last_event(&self) -> Option<DashboardEvent> {
            self.last.borrow().clone()
        }

        fn broadcast_count(&self) -> u64 {
            *self.count.borrow()
        }
    }

    impl DashboardWsServer for MockWsServer {
        fn broadcast(&self, event: &DashboardEvent) -> Result<(), ObsError> {
            *self.last.borrow_mut() = Some(event.clone());
            *self.count.borrow_mut() += 1;
            Ok(())
        }
    }

    fn sample_graph() -> TopologyGraph {
        TopologyGraph {
            nodes: alloc::vec![TopologyNode {
                node_id: 1,
                label: String::from("n1"),
                kind: NodeKind::Grid,
                position: Position3D::new(0.0, 0.0, 0.0),
                status: NodeStatus::Healthy,
            }],
            edges: alloc::vec![TopologyEdge {
                from: 1,
                to: 2,
                kind: EdgeKind::Power,
                flow: 10.0,
                direction: FlowDirection::Forward,
            }],
            timestamp: 1,
        }
    }

    #[test]
    fn broadcast_topology_update() {
        let srv = MockWsServer::new();
        let ev = DashboardEvent::TopologyUpdate(sample_graph());
        assert!(srv.broadcast(&ev).is_ok());
        assert_eq!(srv.broadcast_count(), 1);
        match srv.last_event() {
            Some(DashboardEvent::TopologyUpdate(g)) => assert_eq!(g.nodes.len(), 1),
            _ => panic!("expected topology update"),
        }
    }

    #[test]
    fn broadcast_power_flow_update() {
        let srv = MockWsServer::new();
        let samples = alloc::vec![PowerFlowSample {
            from: 1,
            to: 2,
            power_kw: 100.0,
            voltage: 220.0,
            current: 100.0 / 220.0,
        }];
        let ev = DashboardEvent::PowerFlowUpdate(samples);
        assert!(srv.broadcast(&ev).is_ok());
        match srv.last_event() {
            Some(DashboardEvent::PowerFlowUpdate(s)) => assert_eq!(s.len(), 1),
            _ => panic!("expected power flow update"),
        }
    }

    #[test]
    fn broadcast_node_status_change() {
        let srv = MockWsServer::new();
        let ev = DashboardEvent::NodeStatusChange(7, NodeStatus::Fault);
        assert!(srv.broadcast(&ev).is_ok());
        assert_eq!(srv.broadcast_count(), 1);
        match srv.last_event() {
            Some(DashboardEvent::NodeStatusChange(id, st)) => {
                assert_eq!(id, 7);
                assert_eq!(st, NodeStatus::Fault);
            }
            _ => panic!("expected status change"),
        }
    }

    #[test]
    fn broadcast_count_accumulates() {
        let srv = MockWsServer::new();
        let ev = DashboardEvent::NodeStatusChange(1, NodeStatus::Offline);
        let _ = srv.broadcast(&ev);
        let _ = srv.broadcast(&ev);
        let _ = srv.broadcast(&ev);
        assert_eq!(srv.broadcast_count(), 3);
    }
}
