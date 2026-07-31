# 高级监控仪表盘（v0.190.0）

`eneros-dashboard` 提供 EnerOS 监控仪表盘的核心数据模型与计算抽象，
支撑 **3D 拓扑可视化**、**实时功率流** 与 **交互式推送** 三大能力。

## 模块结构

| 模块 | 职责 |
| --- | --- |
| `types` | 3D 拓扑图、节点/边、功率流采样等数据结构 |
| `topology` | 拓扑快照抽象、样例实现与图查询 |
| `powerflow` | 功率流计算抽象与简化实现 |
| `render` | 推送事件、WebSocket 广播抽象与模拟实现 |

## 核心类型

- `NodeKind` / `NodeStatus` / `EdgeKind` / `FlowDirection` — 枚举语义
- `Position3D` — 3D 坐标（`f32`）
- `TopologyNode` / `TopologyEdge` / `TopologyGraph` — 拓扑图模型
- `PowerFlowSample` — 功率流采样（功率/电压/电流，`f64`）
- `DashboardEvent` — 推送事件（拓扑更新 / 功率流更新 / 节点状态变更）
- `ObsError` — 错误类型（连接失败 / 广播失败 / 非法拓扑）

## 数值约定

- 3D 坐标使用 `f32`
- 功率与指标使用 `f64`
- ID 与时间戳使用 `u64`

## 功率流计算

`SimplePowerFlow` 对每条 `EdgeKind::Power` 边：

```text
power_kw = edge.flow
voltage  = 220 V（标称电压）
current  = power_kw / voltage
```

## no_std 合规

`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`，零第三方依赖，
可交叉编译到 `aarch64-unknown-none`。
