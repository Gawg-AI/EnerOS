# Tasks

## P0: 端到端连通

- [x] Task 1: 修复 main.rs 组件注入
  - [x] 1.1: 重写 run_server()，将所有组件注入 AppState
  - [x] 1.2: SCADA 管线注入 AppState 并后台运行
  - [x] 1.3: Agent 编排器注入 AppState，注册 6 个领域 Agent
  - [x] 1.4: DataDrivenAgentLoop 连接 SCADA→Agent 闭环
  - [x] 1.5: API handler 从 AppState 获取真实引擎数据
  - [x] 1.6: 添加 SimulatedDataSource 提供模拟数据
  - [x] 1.7: 添加测试

- [x] Task 2: TOML 配置文件加载
  - [x] 2.1: eneros-core 添加 toml 依赖
  - [x] 2.2: 实现 EnerOSConfig::load_from_str()
  - [x] 2.3: 实现 EnerOSConfig::load_from_file()
  - [x] 2.4: 实现 EnerOSConfig::save_to_file() 和 to_toml_string()
  - [x] 2.5: 创建默认配置文件 eneros.toml
  - [x] 2.6: 添加测试（7 项通过）

## P1: 集成测试与 Dashboard

- [x] Task 3: 跨组件 E2E 集成测试
  - [x] 3.1: 创建 e2e_integration.rs
  - [x] 3.2: 测试场景1 — API 返回真实潮流结果
  - [x] 3.3: 测试场景2 — SCADA 数据采集→Agent 事件触发
  - [x] 3.4: 测试场景3 — Agent 响应 ConstraintViolation 事件
  - [x] 3.5: 测试场景4 — 拓扑/约束/Agent/SCADA 端点
  - [x] 3.6: 8 个集成测试全部通过

- [x] Task 4: Dashboard 集成到 API 服务器
  - [x] 4.1: eneros-api 添加 eneros-dashboard 依赖
  - [x] 4.2: GET / 返回 dashboard HTML 页面
  - [x] 4.3: GET /api/dashboard/topology-svg 返回拓扑 SVG
  - [x] 4.4: GET /api/dashboard/flow-heatmap 返回潮流热力图
  - [x] 4.5: 添加测试

- [x] Task 5: 修复 API handler 返回真实数据
  - [x] 5.1: agents_handler 从 AgentOrchestrator 查询实际 Agent
  - [x] 5.2: scada_handler 从 ScadaCollector 返回真实数据
  - [x] 5.3: constraints_handler 从 ConstraintEngine 返回真实违规
  - [x] 5.4: analysis handler 从 eneros-analysis 返回真实计算结果
  - [x] 5.5: 添加测试

## P2: 生产化增强

- [x] Task 6: ApiClient 真实 HTTP 请求
  - [x] 6.1: 添加 reqwest 依赖
  - [x] 6.2: 实现 ApiClient 6 个异步方法
  - [x] 6.3: CLI status 命令查询运行中的服务器
  - [x] 6.4: CLI agent list/inspect 命令查询运行中的服务器
  - [x] 6.5: 添加 /health 端点
  - [x] 6.6: 添加测试

- [x] Task 7: 持久化存储
  - [x] 7.1: 时序引擎 SQLite 持久化后端（SqliteStorage）
  - [x] 7.2: 记忆系统文件持久化（FileMemory）
  - [x] 7.3: 添加测试（10 项通过，含 round-trip 验证）

## 全局验证

- [x] Task 8: 全局验证
  - [x] 8.1: cargo test --workspace 全部通过（760+ 测试）
  - [x] 8.2: cargo clippy --workspace 无错误
  - [x] 8.3: 更新 README.md 路线图 Phase 8

# Task Dependencies
- [Task 2] 可与 Task 1 并行
- [Task 3] depends on [Task 1]
- [Task 4] depends on [Task 1]
- [Task 5] depends on [Task 1]
- [Task 6] depends on [Task 1]
- [Task 7] 可与 Task 1-6 并行
- [Task 8] depends on [Task 1-7]
