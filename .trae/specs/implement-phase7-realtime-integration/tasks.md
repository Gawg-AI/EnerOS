# Tasks

## 层级 0: 实时数据闭环（P0 — 最高优先级）

- [x] Task 1: SCADA 数据采集管线 — eneros-scada crate
  - [x] 1.1: 创建 `crates/eneros-scada/` crate，定义 Cargo.toml 依赖
  - [x] 1.2: 定义 `ScadaPoint` 结构体（element_id, parameter, scan_rate_ms, deadband）
  - [x] 1.3: 定义 `ScadaConfig` 结构体（采集点列表、采集周期、超时设置）
  - [x] 1.4: 实现 `ScadaCollector` — 从 DataSource trait 按周期读取数据
  - [x] 1.5: 实现 `DataPipeline` — Collector→TimeSeriesEngine 的数据流管道
  - [x] 1.6: 实现数据质量标记逻辑（越限→Bad，读取失败→Bad+NaN）
  - [x] 1.7: 添加测试（25 项通过）

- [x] Task 2: 实时快照构建器
  - [x] 2.1: 在 eneros-scada 定义 `SnapshotBuilder` 结构体
  - [x] 2.2: 定义 `MeasurementMapping` — 将 ScadaPoint 映射到 PowerSystemState 字段
  - [x] 2.3: 实现 `SnapshotBuilder::build()` — 从 ScadaCollector 最新数据构建 PowerSystemState
  - [x] 2.4: 实现数据完整性检查 — 必填字段缺失时返回错误
  - [x] 2.5: 添加测试

- [x] Task 3: 时序引擎增强
  - [x] 3.1: 新增滑动窗口聚合（WindowedAggregator — 滑动窗口均值/最大/最小）
  - [x] 3.2: 新增数据插值（线性插值填补缺失数据点）
  - [x] 3.3: 新增异常检测（3-sigma、突变检测）
  - [x] 3.4: 新增 `TimeSeriesEngine::query_aggregated()` — 按时间窗口聚合查询
  - [x] 3.5: 添加测试（25 项通过）

- [x] Task 4: 数据驱动 Agent 闭环
  - [x] 4.1: 在 eneros-agent 新增 `DataDrivenAgentLoop` 结构体
  - [x] 4.2: 实现 `DataDrivenAgentLoop::new(pipeline, collector, snapshot_builder, orchestrator, state_machine)`
  - [x] 4.3: 实现数据变化检测 — deadband 变化检测
  - [x] 4.4: 实现触发逻辑 — 收集→紧急检测→变化检测→快照→约束→Agent
  - [x] 4.5: 实现紧急数据触发 — 电压/频率紧急阈值直接触发
  - [x] 4.6: 添加测试（19 项通过）

## 层级 1: 高级电力分析（P1）

- [x] Task 5: 高级分析 crate — eneros-analysis
  - [x] 5.1: 创建 `crates/eneros-analysis/` crate
  - [x] 5.2: 定义 `AnalysisResult<T>` 统一结果类型
  - [x] 5.3: 定义 `AnalysisError` 错误枚举

- [x] Task 6: DC-OPF 最优潮流
  - [x] 6.1: 定义 `DcOpfProblem` 结构体（GeneratorBid, BranchLimit, loads）
  - [x] 6.2: 实现 B' 矩阵和 PTDF 矩阵计算
  - [x] 6.3: 实现经济调度（merit-order dispatch）
  - [x] 6.4: 实现 `DcOpfSolver::solve()` — 返回最优有功分配和 LMP 节点电价
  - [x] 6.5: 添加测试（8 项通过，含 3-bus 和 14-bus）

- [x] Task 7: 状态估计
  - [x] 7.1: 定义 `Measurement` 结构体（MeasType, element_id, value, sigma）
  - [x] 7.2: 定义 `StateEstimator` 结构体
  - [x] 7.3: 实现加权最小二乘法（WLS）
  - [x] 7.4: 实现坏数据检测（最大标准残差法）
  - [x] 7.5: 实现 `StateEstimator::estimate()` — 返回估计状态和残差
  - [x] 7.6: 添加测试（6 项通过）

- [x] Task 8: 短路计算
  - [x] 8.1: 定义 `FaultType` 枚举（ThreePhase, SingleLineGround, LineLine, DoubleLineGround）
  - [x] 8.2: 定义 `FaultResult` 结构体（fault_current, bus_voltages, branch_currents）
  - [x] 8.3: 实现对称分量法（SequenceImpedance）
  - [x] 8.4: 实现三相短路计算（Z_bus 方法）
  - [x] 8.5: 实现不对称故障计算（SLG, LL, DLG）
  - [x] 8.6: 添加测试（8 项通过）

## 层级 2: 更多领域 Agent（P2）

- [x] Task 9: 负荷预测 Agent — LoadForecastAgent
  - [x] 9.1: 创建 `agents/forecast_agent.rs`
  - [x] 9.2: 实现指数平滑算法（单指数/双指数/Holt-Winters）
  - [x] 9.3: 实现 `LoadForecastAgent`，Agent trait（authority: Operator, tick: 15min）
  - [x] 9.4: 实现 `handle_event()` — ConstraintViolation/DataReceived
  - [x] 9.5: 实现 `tick()` — 定期预测，发布 LoadForecastAvailable 事件
  - [x] 9.6: 添加测试（26 项通过）

- [x] Task 10: 配网规划 Agent — PlanningAgent
  - [x] 10.1: 创建 `agents/planning_agent.rs`
  - [x] 10.2: 定义 `ExpansionPlan`、`CandidateLine`、`CandidateTransformer`、`RiskLevel`
  - [x] 10.3: 实现 `evaluate_capacity()` — 容量评估
  - [x] 10.4: 实现 `propose_expansion()` — 三种方案（minimal/moderate/aggressive）
  - [x] 10.5: 实现 `PlanningAgent`，Agent trait（authority: Supervisor, tick: 1h）
  - [x] 10.6: 添加测试（15 项通过）

- [x] Task 11: 交易 Agent — TradingAgent
  - [x] 11.1: 创建 `agents/trading_agent.rs`
  - [x] 11.2: 定义 `MarketPrice`、`BidStrategy`、`TradingBid`、`RiskAssessment`
  - [x] 11.3: 实现 `marginal_cost_pricing()` — dC/dP = 2aP + b
  - [x] 11.4: 实现 `risk_adjusted_bid()` — 风险调整报价
  - [x] 11.5: 实现 `TradingAgent`，Agent trait（authority: Operator, tick: 5min）
  - [x] 11.6: 添加测试（15 项通过）

## 层级 3: 系统集成与演示（P3）

- [x] Task 12: axum HTTP 服务器
  - [x] 12.1: 添加 axum 依赖
  - [x] 12.2: 实现 RESTful API 路由（/api/topology, /api/power-flow, /api/constraints, /api/agents, /api/scada, /api/analysis）
  - [x] 12.3: 实现 `/api/power-flow` — 调用 PowerFlowSolver/PowerNetwork
  - [x] 12.4: 实现 `/api/constraints` — 调用 ConstraintEngine
  - [x] 12.5: 实现 `/api/agents` — Agent 信息查询
  - [x] 12.6: 实现 `/api/scada/latest` — 实时数据查询
  - [x] 12.7: 实现 `/api/analysis` — OPF/状态估计/短路计算 API
  - [x] 12.8: 添加测试（21 项通过）

- [x] Task 13: WebSocket 实时推送
  - [x] 13.1: 添加 WebSocket 支持（axum ws feature）
  - [x] 13.2: 实现 WebSocket 端点 — 连接管理、消息处理
  - [x] 13.3: 实现 `broadcast_event()` — 向所有客户端广播
  - [x] 13.4: 集成到路由
  - [x] 13.5: 添加测试

- [x] Task 14: CLI 增强
  - [x] 14.1: `eneros run` — 一键启动 API + Agent + SCADA
  - [x] 14.2: `eneros status` — 系统状态查询
  - [x] 14.3: `eneros agent list/inspect` — Agent 管理
  - [x] 14.4: `eneros analyze opf/state-estimation/short-circuit` — 分析命令
  - [x] 14.5: `eneros power-flow` — 潮流计算命令
  - [x] 14.6: 添加测试

- [x] Task 15: Web 仪表盘
  - [x] 15.1: 创建 `crates/eneros-dashboard/`
  - [x] 15.2: 实现电网拓扑 SVG 可视化（圆形布局、zone 着色）
  - [x] 15.3: 实现潮流热力图（电压颜色映射、支路负载映射）
  - [x] 15.4: 实现 Agent 状态面板（HTML 表格、状态颜色编码）
  - [x] 15.5: 实现实时数据面板（HTML 表格、质量颜色编码）
  - [x] 15.6: 内嵌 HTML/CSS/JS 静态资源（暗色主题、WebSocket 连接、自动刷新）
  - [x] 15.7: 添加测试（33 项通过）

## 层级 4: 全局验证

- [x] Task 16: 全局验证
  - [x] 16.1: cargo test --workspace 全部通过（680+ 测试）
  - [x] 16.2: cargo clippy --workspace 无错误
  - [x] 16.3: 更新 README.md 路线图 Phase 7
  - [x] 16.4: 各 crate 独立测试已通过

# Task Dependencies
- [Task 2] depends on [Task 1]
- [Task 3] 可与 Task 1/2 并行
- [Task 4] depends on [Task 1, 2, 3]
- [Task 5] 可与 Task 1-4 并行
- [Task 6] depends on [Task 5]
- [Task 7] depends on [Task 5]
- [Task 8] depends on [Task 5]
- [Task 6, 7, 8] 可并行
- [Task 9] depends on [Task 3]
- [Task 10] depends on [Task 6]
- [Task 11] depends on [Task 6]
- [Task 9, 10, 11] 可并行
- [Task 12] depends on [Task 1, 5]
- [Task 13] depends on [Task 12]
- [Task 14] depends on [Task 12]
- [Task 15] depends on [Task 12, 13]
- [Task 16] depends on [Task 4, 8, 11, 15]
