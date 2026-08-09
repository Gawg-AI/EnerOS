//! EnerOS v0.190.0 高级监控仪表盘（3D 拓扑 + 实时功率流 + 交互式）.
//!
//! 本 crate 为 EnerOS 提供监控仪表盘的核心数据模型与计算抽象：
//!
//! - [`types`] — 3D 拓扑图、节点/边、功率流采样等数据结构
//!   （3D 坐标 `f32`，功率/指标 `f64`，ID/时间戳 `u64`）
//! - [`topology`] — [`topology::TopologyProvider`] 拓扑快照抽象、
//!   [`topology::MockTopology`] 样例实现与节点查找 / 邻居查询
//! - [`powerflow`] — [`powerflow::PowerFlowCalculator`] 功率流计算抽象
//!   与 [`powerflow::SimplePowerFlow`] 简化实现
//! - [`render`] — [`render::DashboardEvent`] 推送事件与
//!   [`render::DashboardWsServer`] 广播抽象
//! - [`ws`] — 真 WebSocket 服务端（RFC 6455 握手 + 帧编解码 +
//!   事件 JSON 推送，[`ws::WsServer`]）
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`，
//! 零第三方依赖，非测试代码仅使用 `core::*` / `alloc::*`，可交叉编译到
//! `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod powerflow;
pub mod real_render;
pub mod render;
pub mod topology;
pub mod types;
pub mod ws;

pub use powerflow::{PowerFlowCalculator, SimplePowerFlow, NOMINAL_VOLTAGE};
pub use render::{DashboardEvent, DashboardWsServer};
pub use topology::{find_node, neighbors, MockTopology, TopologyProvider};
pub use types::{
    EdgeKind, FlowDirection, NodeKind, NodeStatus, Position3D, PowerFlowSample, TopologyEdge,
    TopologyGraph, TopologyNode,
};
pub use ws::{WsServer, MAX_FRAME_BYTES, OP_BINARY, OP_CLOSE, OP_PING, OP_PONG, OP_TEXT};

/// 仪表盘错误类型.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObsError {
    /// 连接失败.
    ConnectionFailed,
    /// 广播失败.
    BroadcastFailed,
    /// 非法拓扑.
    InvalidTopology,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obs_error_variants() {
        assert_eq!(ObsError::ConnectionFailed, ObsError::ConnectionFailed);
        assert_ne!(ObsError::BroadcastFailed, ObsError::InvalidTopology);
    }

    #[test]
    fn reexports_available() {
        let g = MockTopology::new().snapshot();
        assert!(!g.nodes.is_empty());
        let samples = SimplePowerFlow::new().calculate(&g);
        assert!(!samples.is_empty());
    }
}
