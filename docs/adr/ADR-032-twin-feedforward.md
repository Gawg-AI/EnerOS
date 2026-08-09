# ADR-032 — twin 预测回灌决策（22 W2.6）

- **状态**：Accepted（2026-08-09）
- **决策者**：主开发代理（按 22 §2.W2.6 强约束）
- **关联**：ADR-031（在线学习）、`蓝图/22` §2.W2.6、§8 G-R2

## 背景

22 §2.W2.6：`twin-agent` 真产预测（`Predictor::forecast`），发布
DDS/MQTT 后无人读回。要求：twin 预测在决策"感知"相被读入，作为
`SystemContext` 的前瞻输入（feedforward），不只发布。

## 决策

1. **energy_agent 感知相接入**：`twin_forecast: Option<Vec<f64>>` +
   `twin_confidence: f32`；`on_tick` 构造 `RealtimeState.load_demand` 时
   用 twin 预测（feedforward）——无预测显式 `None`（不造假值，与
   21-B"无传感器数据源显式 None"纪律一致）。
2. **注入接口**：`set_twin_forecast(points, confidence)` 值注入——
   **不依赖 twin-agent crate**（依赖环：twin→device→energy-market→twin，
   cargo 拒绝环）；上层 runner/设备服务从 twin 预测提取后注入。
3. **可执行证据**：2 测试——feedforward 注入生效 + 有/无 twin 决策对比
   （都决策成功，路径差异可观察）。

## 影响

- twin 预测从"发布即弃"变"感知输入"（闭环 feedforward）；LP 消费
  load_demand 前瞻序列。
- 依赖环规避（ADR 记录）；runner 层接线为 G-R2 闸门验收项。

## 备选方案

- **直接依赖 twin-agent**：cargo 依赖环（twin→device-agent→
  energy-market-agent），否决。
- **twin 预测进 SystemContext 其他字段**：load_demand 语义最匹配
  （前瞻负荷），维持现状。
