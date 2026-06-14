# Phase 7 — 实时数据闭环与系统集成 Spec

## Why
Phase 1-6 完成了 EnerOS 的内核基座、Agent 运行时、电网感知、多智能体协作、协议适配器和领域 Agent。但当前系统存在三个关键缺口：
1. **数据与决策断裂** — 时序引擎和设备适配器是独立的，SCADA 数据无法自动驱动 Agent 决策
2. **分析能力不足** — 仅有 Newton-Raphson 潮流计算，缺少 OPF、状态估计、短路计算等高级分析
3. **系统不可交互** — API 服务器是空壳，无法实际运行和演示

## What Changes

### 7A: 实时数据闭环（优先）
- 新增 `eneros-scada` crate — SCADA 数据采集管线，统一接入设备数据
- 新增 `DataPipeline` — 设备→时序引擎→Agent 的实时数据流
- 新增 `SnapshotBuilder` — 从实时数据自动构建 PowerSystemState 快照
- 增强 `TimeSeriesEngine` — 支持滑动窗口聚合、数据插值、异常检测
- 新增 `DataDrivenAgentLoop` — 数据变化触发 Agent 推理-决策-执行闭环

### 7B: 高级电力分析
- 新增 `eneros-analysis` crate — 高级电力分析内核
- 实现 DC-OPF（最优潮流）— 线性规划求解最小成本调度
- 实现 State Estimator（状态估计）— 加权最小二乘法
- 实现 Short Circuit Analysis（短路计算）— 对称/不对称故障

### 7C: 更多领域 Agent
- 新增 LoadForecastAgent — 基于 ARIMA/指数平滑的负荷预测
- 新增 PlanningAgent — 配网扩展规划（负荷增长→设备选型→方案评估）
- 新增 TradingAgent — 现货市场报价策略（边际成本定价+风险评估）

### 7D: 系统集成与演示
- 增强 `eneros-api` — axum HTTP 服务器，RESTful API 完整实现
- 新增 WebSocket 实时推送 — Agent 事件、系统状态变更实时通知
- 增强 CLI — `eneros run` 一键启动、`eneros status` 系统状态、`eneros agent` Agent 管理
- 新增 `eneros-dashboard` — Web 前端仪表盘（拓扑图、潮流可视化、Agent 状态）

## Impact
- Affected specs: implement-phase6-domain-agents, build-power-native-agentos
- Affected code:
  - eneros-timeseries (增强)
  - eneros-api (大幅重写)
  - eneros-agent (新增 DataDrivenAgentLoop)
  - eneros-device (与 eneros-scada 协作)
  - Cargo.toml (新增 3 个 crate)

## ADDED Requirements

### Requirement: SCADA 数据采集管线
系统 SHALL 提供 `eneros-scada` crate，统一管理设备数据采集。

#### Scenario: SCADA 数据自动采集
- **WHEN** DeviceManager 连接设备并配置采集点
- **THEN** DataPipeline 自动按采集周期读取数据并写入 TimeSeriesEngine

#### Scenario: 数据质量标记
- **WHEN** 设备数据读取超时或值越限
- **THEN** 数据点标记为 Uncertain 或 Bad 质量

### Requirement: 实时快照构建
系统 SHALL 提供 `SnapshotBuilder`，从实时数据构建 PowerSystemState。

#### Scenario: 自动构建系统快照
- **WHEN** 所有关键采集点有最新数据
- **THEN** SnapshotBuilder 构建完整的 PowerSystemState，可用于潮流计算

### Requirement: 数据驱动 Agent 闭环
系统 SHALL 提供 `DataDrivenAgentLoop`，数据变化自动触发 Agent 决策。

#### Scenario: 电压越限自动响应
- **WHEN** 实时数据检测到母线电压低于 0.95 p.u.
- **THEN** 自动触发 DispatchAgent 重新调度

### Requirement: DC-OPF 最优潮流
系统 SHALL 提供 DC-OPF 求解器，基于线性规划求解最小成本调度方案。

#### Scenario: 经济调度优化
- **WHEN** 给定负荷需求和发电机成本曲线
- **THEN** DC-OPF 返回最小总成本的有功分配方案，满足线路容量约束

### Requirement: 状态估计
系统 SHALL 提供加权最小二乘状态估计器。

#### Scenario: 不完整量测下的状态估计
- **WHEN** 部分量测缺失或存在噪声
- **THEN** 状态估计器利用冗余量测和拓扑信息估计完整系统状态

### Requirement: 短路计算
系统 SHALL 提供对称和不对称短路计算。

#### Scenario: 三相短路计算
- **WHEN** 指定故障母线和故障类型
- **THEN** 计算故障点短路电流和各母线电压

### Requirement: 负荷预测 Agent
系统 SHALL 提供 LoadForecastAgent，基于历史时序数据预测未来负荷。

#### Scenario: 日前负荷预测
- **WHEN** 提供过去 7 天的负荷时序数据
- **THEN** LoadForecastAgent 预测未来 24 小时的负荷曲线

### Requirement: 配网规划 Agent
系统 SHALL 提供 PlanningAgent，评估负荷增长下的网架扩展方案。

#### Scenario: 负荷增长规划
- **WHEN** 预测负荷增长超过现有线路容量
- **THEN** PlanningAgent 提出线路升级或新建方案

### Requirement: 交易 Agent
系统 SHALL 提供 TradingAgent，基于边际成本制定现货市场报价。

#### Scenario: 现货市场报价
- **WHEN** 接收到市场出清价格信号
- **THEN** TradingAgent 基于发电机边际成本和风险评估制定报价

### Requirement: RESTful API 服务器
系统 SHALL 提供完整的 axum HTTP 服务器，暴露所有核心能力。

#### Scenario: 潮流计算 API
- **WHEN** POST /api/power-flow 请求包含网络参数
- **THEN** 返回潮流计算结果 JSON

### Requirement: WebSocket 实时推送
系统 SHALL 通过 WebSocket 推送实时事件。

#### Scenario: Agent 事件推送
- **WHEN** Agent 执行动作或系统状态变更
- **THEN** WebSocket 客户端实时收到事件通知

### Requirement: Web 仪表盘
系统 SHALL 提供 Web 前端仪表盘，可视化系统运行状态。

#### Scenario: 拓扑图可视化
- **WHEN** 用户打开仪表盘
- **THEN** 显示电网拓扑图，节点颜色反映电压水平，边粗细反映潮流大小
