# 端到端闭环 + Phase 3 电网感知 Spec

## Why
Phase 1/2 建立了独立的内核基座和 Agent 运行时组件，但数据流在两处断裂：(1) 设备定义→潮流计算无法自动衔接，(2) Agent→命令执行→拓扑反馈没有闭环。需要补全关键缺口让架构跑通端到端，再推进 Phase 3 电网感知上下文。

## What Changes
- NetworkGraph 添加潮流相关字段（bus_type_pf, p_gen, q_gen, p_load, q_load, v_pu, tap_ratio）
- EquipmentLibrary 添加批量导出接口（collect_admittances, get_injections_at_bus, bus_ids）
- PowerNetwork::from_equipment() — 从设备库+拓扑构建潮流网络
- AgentContext — Agent 运行时上下文，持有所有子系统引用
- AgentEventHandler — 将 Agent 适配为 EventHandler，接入 EventBus
- ActionDispatcher — 路由 AgentAction（PublishEvent→EventBus, ExecuteCommand→SafetyGateway, LogMessage→tracing）
- Command 添加 TopologyChange 关联
- TopologyQueryTool — Agent 拓扑查询工具
- AgentOrchestrator — 系统主循环（事件分发→Agent推理→Action路由→tick调度）

## Impact
- Affected specs: eneros-topology (Bus/Branch 结构变更), eneros-equipment (EquipmentLibrary 新方法), eneros-network (from_equipment), eneros-agent (AgentContext + AgentOrchestrator), eneros-tool (TopologyQueryTool), eneros-gateway (Command 扩展)
- Affected code: 6 个 crate 需修改，2 个 crate 需新增模块

## ADDED Requirements

### Requirement: PowerNetwork::from_equipment
系统 SHALL 提供 `PowerNetwork::from_equipment(library, graph, base_mva)` 方法，从 EquipmentLibrary 和 NetworkGraph 自动构建潮流计算所需全部输入。

#### Scenario: 从设备库构建潮流网络
- **WHEN** 调用 `PowerNetwork::from_equipment(&library, &graph, 100.0)`
- **THEN** 返回的 PowerNetwork 包含正确的 YBusMatrix、p_spec、q_spec、bus_types，且 `solve()` 可收敛

### Requirement: AgentContext
系统 SHALL 提供 `AgentContext` 结构体，持有 EventBus、SafetyGateway、ToolEngine、PowerNetwork、Memory、ReasoningEngine 的共享引用，供 Agent 在 handle_event/tick 中访问。

#### Scenario: Agent 通过上下文访问服务
- **WHEN** Agent 的 handle_event 方法接收 AgentContext
- **THEN** Agent 可通过 ctx 调用 tool_engine.execute()、memory.store()、reasoning_engine.reason() 等服务

### Requirement: AgentEventHandler 适配器
系统 SHALL 提供 `AgentEventHandler` 将 Agent 适配为 EventHandler，使 Agent 可通过 EventBus 接收事件。

#### Scenario: Agent 通过 EventBus 接收约束违规事件
- **WHEN** EventBus 发布 ConstraintViolation 事件
- **THEN** 已注册的 AgentEventHandler 调用对应 Agent 的 handle_event，返回 AgentAction 列表

### Requirement: ActionDispatcher
系统 SHALL 提供 `ActionDispatcher` 将 AgentAction 路由到正确的子系统。

#### Scenario: Agent 发布事件
- **WHEN** AgentAction::PublishEvent(event) 被 dispatch
- **THEN** event 被发送到 EventBus::publish()

#### Scenario: Agent 执行命令
- **WHEN** AgentAction::ExecuteCommand(cmd) 被 dispatch
- **THEN** cmd 先经过 SafetyGateway::execute_command() 校验，通过后执行

### Requirement: AgentOrchestrator
系统 SHALL 提供 `AgentOrchestrator` 作为系统主循环，协调事件分发、Agent 调度、Action 路由。

#### Scenario: 端到端闭环
- **WHEN** 约束违规事件进入 EventBus
- **THEN** Orchestrator 将事件分发给注册的 Agent → Agent 推理决策 → ActionDispatcher 路由动作 → 安全网关校验命令 → 执行并反馈

### Requirement: TopologyQueryTool
系统 SHALL 提供 TopologyQueryTool，支持 Agent 查询网络拓扑（连通性、路径、区域、环网）。

#### Scenario: Agent 查询两母线连通性
- **WHEN** 调用 TopologyQueryTool，参数 `{ query: "is_connected", bus1: 1, bus2: 5 }`
- **THEN** 返回两母线是否连通的结果

### Requirement: Command 拓扑变更关联
Command 结构 SHALL 支持 TopologyChange 类型，使命令执行后可自动触发拓扑更新。

#### Scenario: 开关操作命令触发拓扑变更
- **WHEN** CommandType::SwitchToggle 命令通过 SafetyGateway 执行
- **THEN** 对应的 TopologyChange::SwitchToggle 被应用到 NetworkGraph

## MODIFIED Requirements

### Requirement: NetworkGraph Bus/Branch 结构
Bus 结构新增字段：bus_type_pf (BusType), p_gen, q_gen, p_load, q_load, v_pu。Branch 结构新增字段：tap_ratio。

### Requirement: EquipmentLibrary 批量接口
EquipmentLibrary 新增方法：collect_admittances(), get_injections_at_bus(), bus_ids()。

### Requirement: Agent trait 签名
Agent trait 的 handle_event 和 tick 方法签名新增 `ctx: &AgentContext` 参数。
