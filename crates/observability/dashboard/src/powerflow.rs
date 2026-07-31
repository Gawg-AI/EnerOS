//! 实时功率流计算（v0.190.0）.
//!
//! 定义 [`PowerFlowCalculator`] 抽象与 [`SimplePowerFlow`] 简化实现：
//! 对每条 [`EdgeKind::Power`] 边，以 `flow` 作为有功功率、固定电压 220 V，
//! 由 `I = P / U` 推导电流。

use alloc::vec::Vec;

use crate::types::{EdgeKind, PowerFlowSample, TopologyGraph};

/// 标称电压（V）.
pub const NOMINAL_VOLTAGE: f64 = 220.0;

/// 功率流计算器抽象.
pub trait PowerFlowCalculator {
    /// 基于拓扑图计算功率流采样集合.
    fn calculate(&self, graph: &TopologyGraph) -> Vec<PowerFlowSample>;
}

/// 简化功率流计算器.
#[derive(Debug, Clone, Default)]
pub struct SimplePowerFlow;

impl SimplePowerFlow {
    /// 构造一个简化功率流计算器.
    pub const fn new() -> Self {
        Self
    }
}

impl PowerFlowCalculator for SimplePowerFlow {
    fn calculate(&self, graph: &TopologyGraph) -> Vec<PowerFlowSample> {
        let mut samples = Vec::new();
        for e in &graph.edges {
            if e.kind != EdgeKind::Power {
                continue;
            }
            let power_kw = e.flow;
            let voltage = NOMINAL_VOLTAGE;
            let current = if voltage != 0.0 {
                power_kw / voltage
            } else {
                0.0
            };
            samples.push(PowerFlowSample {
                from: e.from,
                to: e.to,
                power_kw,
                voltage,
                current,
            });
        }
        samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::{MockTopology, TopologyProvider};

    #[test]
    fn only_power_edges() {
        let g = MockTopology::new().snapshot();
        let samples = SimplePowerFlow::new().calculate(&g);
        // 样例图仅 1 条 Power 边（1 -> 2）.
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].from, 1);
        assert_eq!(samples[0].to, 2);
    }

    #[test]
    fn power_and_current() {
        let g = MockTopology::new().snapshot();
        let samples = SimplePowerFlow::new().calculate(&g);
        let s = samples[0];
        assert_eq!(s.power_kw, 500.0);
        assert_eq!(s.voltage, NOMINAL_VOLTAGE);
        assert!((s.current - 500.0 / 220.0).abs() < 1e-9);
    }

    #[test]
    fn empty_graph() {
        let g = TopologyGraph {
            nodes: Vec::new(),
            edges: Vec::new(),
            timestamp: 0,
        };
        assert!(SimplePowerFlow::new().calculate(&g).is_empty());
    }
}
