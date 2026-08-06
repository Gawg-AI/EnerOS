//! Dashboard 真实渲染（v0.240.0，B5-03）。
//!
//! 真实 SVG/HTML 渲染：拓扑图 + 功率流可视化为 SVG 文档（浏览器可
//! 直接显示），功率数值经量纲检查（kW 标注）。
//!
//! 数据面：真实 WS 服务由集成层接 mgmt-api（此处提供数据序列化
//! 与渲染，浏览器可见实时拓扑与功率流）。

use alloc::string::String;
use alloc::vec::Vec;

use crate::types::{NodeKind, PowerFlowSample, TopologyGraph};

/// SVG 渲染结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SvgRender {
    /// SVG 文档（完整）。
    pub svg: String,
    /// 渲染节点数。
    pub node_count: usize,
    /// 渲染边数。
    pub edge_count: usize,
}

/// 真实 SVG 渲染器。
pub struct SvgRenderer {
    /// 画布宽（px）。
    pub width: u32,
    /// 画布高（px）。
    pub height: u32,
}

impl Default for SvgRenderer {
    fn default() -> Self {
        Self {
            width: 800,
            height: 600,
        }
    }
}

impl SvgRenderer {
    /// 创建渲染器。
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// 渲染拓扑图为 SVG。
    ///
    /// 节点以圆圈+标签绘制；边以带箭头的线绘制（flow 标注 kW）。
    pub fn render_topology(&self, graph: &TopologyGraph) -> SvgRender {
        let mut body = String::new();
        // 边先画（在节点下层）。
        for edge in &graph.edges {
            let (x1, y1) = node_xy(graph, edge.from, self.width, self.height);
            let (x2, y2) = node_xy(graph, edge.to, self.width, self.height);
            body.push_str(&alloc::format!(
                "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"#555\" stroke-width=\"2\"/>\n",
                x1, y1, x2, y2
            ));
            // 流量标注（kW，量纲正确）。
            let mx = (x1 + x2) / 2.0;
            let my = (y1 + y2) / 2.0;
            body.push_str(&alloc::format!(
                "<text x=\"{}\" y=\"{}\" font-size=\"10\" fill=\"#222\">{:.1} kW</text>\n",
                mx, my, edge.flow
            ));
        }
        // 节点。
        for node in &graph.nodes {
            let (x, y) = node_xy(graph, node.node_id, self.width, self.height);
            let color = match node.kind {
                NodeKind::Grid => "#2e7d32",
                NodeKind::Asset => "#c62828",
                NodeKind::Market => "#1565c0",
                _ => "#616161",
            };
            body.push_str(&alloc::format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"14\" fill=\"{}\"/>\n",
                x, y, color
            ));
            body.push_str(&alloc::format!(
                "<text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"#000\" text-anchor=\"middle\">{}</text>\n",
                x,
                (y + 26.0),
                node.label
            ));
        }
        let svg = alloc::format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\">{}</svg>",
            self.width,
            self.height,
            body
        );
        SvgRender {
            svg,
            node_count: graph.nodes.len(),
            edge_count: graph.edges.len(),
        }
    }

    /// 渲染功率流数据为 JSON 行（数据面，浏览器 JS 消费）。
    ///
    /// 每条：`{"from":1,"to":2,"kw":150.5,"v":230.0,"a":0.65}`。
    /// 功率值经量纲校验（有限且非负标注）。
    pub fn render_powerflow(&self, samples: &[PowerFlowSample]) -> Vec<String> {
        let mut out = Vec::with_capacity(samples.len());
        for s in samples {
            let kw = if s.power_kw.is_finite() && s.power_kw >= 0.0 {
                s.power_kw
            } else {
                0.0
            };
            out.push(alloc::format!(
                "{{\"from\":{},\"to\":{},\"kw\":{:.3},\"v\":{:.1},\"a\":{:.2}}}",
                s.from, s.to, kw, s.voltage, s.current
            ));
        }
        out
    }
}

/// 节点在画布中的位置（固定 4 列网格布局，no_std 无 sqrt）。
fn node_xy(graph: &TopologyGraph, node_id: u64, w: u32, h: u32) -> (f64, f64) {
    let idx = graph
        .nodes
        .iter()
        .position(|n| n.node_id == node_id)
        .unwrap_or(0);
    const COLS: u32 = 4;
    let n = graph.nodes.len().max(1) as u32;
    let col = idx as u32 % COLS;
    let row = idx as u32 / COLS;
    let rows = n / COLS + if n % COLS == 0 { 0 } else { 1 };
    let x = (col as f64 + 0.5) * (w as f64 / COLS as f64);
    let y = (row as f64 + 0.5) * (h as f64 / rows.max(1) as f64);
    (x, y)
}

// ============================================================================
// Tests（SVG 结构 + 功率量纲）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EdgeKind, FlowDirection, Position3D, TopologyEdge};

    fn graph() -> TopologyGraph {
        TopologyGraph {
            nodes: vec![
                crate::types::TopologyNode {
                    node_id: 1,
                    label: String::from("PV"),
                    kind: NodeKind::Grid,
                    position: Position3D { x: 0.0, y: 0.0, z: 0.0 },
                    status: crate::types::NodeStatus::Healthy,
                },
                crate::types::TopologyNode {
                    node_id: 2,
                    label: String::from("Load"),
                    kind: NodeKind::Asset,
                    position: Position3D { x: 1.0, y: 0.0, z: 0.0 },
                    status: crate::types::NodeStatus::Healthy,
                },
            ],
            edges: vec![TopologyEdge {
                from: 1,
                to: 2,
                kind: EdgeKind::Power,
                flow: 150.5,
                direction: FlowDirection::Forward,
            }],
            timestamp: 1000,
        }
    }

    #[test]
    fn test_render_topology_svg_structure() {
        let renderer = SvgRenderer::default();
        let r = renderer.render_topology(&graph());
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.ends_with("</svg>"));
        assert_eq!(r.node_count, 2);
        assert_eq!(r.edge_count, 1);
        // 节点与边元素存在。
        assert!(r.svg.contains("<circle"));
        assert!(r.svg.contains("<line"));
        // 流量标注（kW 量纲正确）。
        assert!(r.svg.contains("150.5 kW"), "功率标注量纲正确");
        // 标签存在。
        assert!(r.svg.contains("PV"));
        assert!(r.svg.contains("Load"));
    }

    #[test]
    fn test_render_empty_graph() {
        let renderer = SvgRenderer::new(100, 100);
        let r = renderer.render_topology(&TopologyGraph {
            nodes: Vec::new(),
            edges: Vec::new(),
            timestamp: 0,
        });
        assert_eq!(r.node_count, 0);
        assert_eq!(r.edge_count, 0);
    }

    #[test]
    fn test_powerflow_json_lines() {
        let renderer = SvgRenderer::default();
        let samples = vec![
            PowerFlowSample {
                from: 1,
                to: 2,
                power_kw: 150.5,
                voltage: 230.0,
                current: 0.65,
            },
            PowerFlowSample {
                from: 2,
                to: 1,
                power_kw: f64::NAN, // 非法量纲 → 0
                voltage: 0.0,
                current: 0.0,
            },
        ];
        let lines = renderer.render_powerflow(&samples);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"kw\":150.500"));
        assert!(lines[0].contains("\"v\":230.0"));
        assert!(lines[1].contains("\"kw\":0.000"), "NaN 功率归零");
    }

    #[test]
    fn test_powerflow_negative_clamped() {
        let renderer = SvgRenderer::default();
        let samples = vec![PowerFlowSample {
            from: 1,
            to: 2,
            power_kw: -5.0,
            voltage: 0.0,
            current: 0.0,
        }];
        let lines = renderer.render_powerflow(&samples);
        assert!(lines[0].contains("\"kw\":0.000"), "负功率归零");
    }
}
