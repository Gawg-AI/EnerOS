# Bridge 深度集成 + Phase 4 多智能体 Spec

## Why
cnpower/pandapower 集成存在严重缺陷：每次调用 spawn 新 Python 进程（性能极差）、pandapower 潮流结果无法回传 Rust、拓扑信息完全缺失、9 类设备无 Rust 端解析。同时 Phase 3 闭环已跑通，需要推进 Phase 4 多智能体协作让系统具备实际调度能力。

## What Changes
- Bridge 架构重构：从"每次 spawn 进程"改为"持久化 HTTP 服务"
- pandapower 结果序列化：电压/功率/损耗/收敛信息回传 Rust
- 拓扑信息回传：build_network 返回完整拓扑（bus 列表 + branch 列表 + bus_types）
- Rust 端新增设备解析：开关柜、无功补偿、新能源等
- Phase 4 多智能体协作：Agent 间消息传递、协作协议、拓扑结构化通信

## Impact
- Affected specs: eneros-bridge (架构重构), eneros-agent (多智能体), eneros-network (从 bridge 构建网络)
- Affected code: eneros-bridge 全部文件, eneros-agent 新增模块, eneros-network 新增方法

## ADDED Requirements

### Requirement: 持久化 Python Bridge 服务
系统 SHALL 提供持久化的 Python HTTP 服务替代当前"每次 spawn 进程"模式，Rust 端通过 HTTP 客户端调用 Python 端 API。

#### Scenario: 启动 Bridge 服务并调用
- **WHEN** Rust 端调用 `BridgeClient::start()`
- **THEN** Python HTTP 服务在后台启动，后续调用通过 HTTP 请求完成，无需重复启动 Python

#### Scenario: 多次调用不重复启动
- **WHEN** 连续调用 `list_transformers()` 和 `list_cables()`
- **THEN** 两次调用通过同一 HTTP 服务完成，总耗时 < 2 秒（vs 当前每次 5+ 秒）

### Requirement: pandapower 潮流结果序列化
系统 SHALL 将 pandapower 潮流计算结果（电压幅值/相角、线路有功/无功、损耗、收敛状态）序列化为 JSON 并回传 Rust 端。

#### Scenario: 从 pandapower 获取潮流结果
- **WHEN** 调用 `run_powerflow()` 命令
- **THEN** 返回包含 bus 电压/相角、line 有功/无功、总损耗、收敛标志的 JSON 结构

### Requirement: 拓扑信息回传
系统 SHALL 从 pandapower/cnpower 构建的网络中提取完整拓扑信息（bus 列表、branch 列表、bus_types、负荷/发电数据）并回传 Rust 端，可直接用于 PowerNetwork 构建。

#### Scenario: 从 cnpower 构建完整网络
- **WHEN** 调用 `build_full_network()` 命令
- **THEN** 返回拓扑数据（buses with type/gen/load、branches with params），Rust 端可直接构造 `PowerNetwork::from_equipment()`

### Requirement: 多智能体消息传递
系统 SHALL 提供智能体间消息传递机制，使 Agent 可向其他 Agent 发送定向消息。

#### Scenario: 调度 Agent 向运维 Agent 发送指令
- **WHEN** Dispatcher Agent 生成 `AgentMessage::Direct(target_id, content)`
- **THEN** 目标 Agent 在下一次 tick 或事件处理中收到该消息

### Requirement: 多智能体协作协议
系统 SHALL 提供基于角色的协作协议：Dispatcher 分配任务、Operator 执行并反馈、Planner 提供建议。

#### Scenario: 约束违规的协作处理
- **WHEN** 约束违规事件进入系统
- **THEN** Orchestrator 分发给 Dispatcher → Dispatcher 分配给 Operator → Operator 执行并反馈 → Dispatcher 确认

## MODIFIED Requirements

### Requirement: CnpowerEquipmentLoader 扩展
CnpowerEquipmentLoader 新增方法：`load_all_switchgear()`, `load_all_reactive_compensation()`, `load_all_new_energy()`，并修复 `load_all_loads()` 使用正确的数据源。

### Requirement: AgentOrchestrator 扩展
AgentOrchestrator 新增 `send_message()` 和 `broadcast_message()` 方法，支持 Agent 间通信。
