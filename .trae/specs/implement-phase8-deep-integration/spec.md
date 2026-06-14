# Phase 8 — 深度集成与生产化 Spec

## Why
Phase 1-7 完成了所有组件的开发，但系统存在严重的集成断裂：
1. **`eneros run` 不真正工作** — SCADA 管线和 Agent 编排器创建后即被丢弃，AppState 所有引擎为 None
2. **配置系统形同虚设** — EnerOSConfig 定义完善但从未加载任何配置文件
3. **无跨组件 E2E 测试** — 各组件单元测试充分，但无 SCADA→Agent→API 的集成测试
4. **Dashboard 未接入** — eneros-dashboard 生成 HTML 但未集成到 API 服务器
5. **API handler 返回硬编码数据** — agents_handler 返回占位列表，scada_handler 返回空

## What Changes

### 8A: 组件注入与端到端连通（P0）
- 重写 `main.rs` — 将所有组件正确注入 AppState
- SCADA 管线后台运行，数据持续流入 TimeSeriesEngine
- Agent 编排器注册所有领域 Agent，通过 EventBus 连接
- API handler 从 AppState 获取真实引擎数据

### 8B: TOML 配置文件加载（P0）
- eneros-core 添加 toml 依赖，实现 `EnerOSConfig::load_from_file()`
- 创建默认配置文件 `eneros.toml`
- main.rs 从配置文件加载所有参数

### 8C: 跨组件 E2E 集成测试（P1）
- 测试 SCADA→时序→事件→Agent→API 完整链路
- 测试 Agent 决策→安全网关→命令执行链路
- 测试 HTTP API 在有真实引擎时的行为

### 8D: Dashboard 集成与 API 完善（P1）
- eneros-api 集成 eneros-dashboard，提供 Web UI
- agents_handler 从 AgentOrchestrator 查询实际 Agent
- scada_handler 从 ScadaCollector 返回真实数据

### 8E: ApiClient 真实 HTTP 请求（P2）
- 添加 reqwest 依赖
- CLI status/agent 命令查询运行中的服务器

### 8F: 持久化存储（P2）
- 时序引擎 SQLite 持久化
- 记忆系统文件持久化

## Impact
- Affected code: eneros-api (main.rs, app.rs, handlers/), eneros-core (config.rs), eneros-dashboard
- Breaking: main.rs CLI 参数可能变化（增加 --config 选项）

## ADDED Requirements

### Requirement: 端到端组件连通
系统 SHALL 在 `eneros run` 启动时正确连接所有组件。

#### Scenario: eneros run 启动完整系统
- **WHEN** 执行 `eneros run`
- **THEN** API 服务器启动，SCADA 管线后台运行，Agent 编排器注册所有 Agent，API handler 返回真实数据

### Requirement: TOML 配置文件加载
系统 SHALL 支持从 TOML 文件加载配置。

#### Scenario: 加载配置文件
- **WHEN** 执行 `eneros run --config eneros.toml`
- **THEN** 所有组件参数从配置文件读取

### Requirement: 跨组件 E2E 集成测试
系统 SHALL 提供跨组件的端到端集成测试。

#### Scenario: SCADA→Agent→API 链路
- **WHEN** SCADA 采集数据写入时序引擎
- **THEN** 数据变化触发 Agent 决策，API 可查询到最新状态

### Requirement: Dashboard 集成
系统 SHALL 在 API 服务器中提供 Web 仪表盘。

#### Scenario: 访问仪表盘
- **WHEN** 浏览器访问 http://localhost:8080/
- **THEN** 显示 EnerOS 仪表盘页面
