# Phase 6 领域应用层 Spec

## Why
Phase 1-5 已建立完善的 AgentOS 基础设施（权限/约束/闭锁/紧急/审计/调度），但所有组件都是"通用框架"，没有任何领域 Agent 拥有具体的领域推理逻辑。ReasoningEngine 仅做关键词匹配，紧急预案 actions 是字符串而非可执行动作，AgentOS 层与电力内核的数据流断裂。需要打通从"电力系统分析结果 → Agent 推理 → 可执行动作"的完整闭环，并实现首批领域 Agent。

## What Changes
- 新增 `PowerObservation` 结构化电力系统观测模型，替代 `Vec<String>`
- 新增 `ActionMapping` 推理输出到 AgentAction 的映射层
- 升级 `EmergencyResponsePlan.actions` 从 `Vec<String>` 到 `Vec<EmergencyAction>`
- 新增 `AgentOrchestrator` 定时 tick 调度机制
- 新增 `DispatchAgent` 调度领域 Agent（经济调度、AGC、调度计划）
- 新增 `OperationAgent` 运维领域 Agent（故障诊断、设备健康、巡检）
- 新增 `SelfHealingAgent` 自愈领域 Agent（故障隔离、网络重构、恢复供电）
- 增强 `ReasoningEngine` 支持结构化推理输入和数值判断
- 新增领域专用协作协议（调度-运维协同、紧急联动）

## Impact
- Affected specs: eneros-core (PowerObservation), eneros-agent (领域Agent + 调度增强), eneros-reasoning (结构化推理), eneros-network (数据桥接)
- Affected code: eneros-core/src/agentos_types.rs, eneros-agent/src/{orchestrator,emergency,agent}.rs, eneros-reasoning/src/engine.rs, 新增领域Agent文件

## ADDED Requirements

### Requirement: 结构化电力系统观测模型
系统 SHALL 提供 `PowerObservation` 替代 `ReasoningInput.observations: Vec<String>`，包含结构化的电力系统运行数据。

#### Scenario: 从 PowerNetwork 自动生成观测
- **WHEN** Agent 调用 `PowerObservation::from_network(&network)`
- **THEN** 返回包含所有母线电压、支路潮流、频率、发电机出力、负荷消耗的结构化观测

#### Scenario: 观测数据注入推理引擎
- **WHEN** Agent 调用 `engine.reason(input_with_observation)`
- **THEN** 推理引擎可基于数值型观测做定量判断（如"电压 < 0.95pu"）

### Requirement: 推理输出到 AgentAction 映射
系统 SHALL 将 `ReasoningOutput.actions: Vec<String>` 自动映射为 `Vec<AgentAction>`。

#### Scenario: 推理建议映射为 ExecuteCommand
- **WHEN** ReasoningOutput 包含建议 "调整发电机 G1 出力到 100MW"
- **THEN** ActionMapping 自动生成 `AgentAction::ExecuteCommand(Command { target: "G1", parameter: "P", value: 100.0 })`

#### Scenario: 无法映射的建议降级为 PublishEvent
- **WHEN** ReasoningOutput 包含无法自动映射的建议
- **THEN** 降级为 `AgentAction::PublishEvent`，通知人工处理

### Requirement: 紧急预案执行闭环
系统 SHALL 将紧急预案的动作从字符串升级为可执行的 `EmergencyAction`，实现真正的自动执行。

#### Scenario: 频率崩溃预案自动执行
- **WHEN** 频率低于 49.5Hz 触发频率崩溃预案
- **THEN** 自动执行：1) 切除非关键负荷（ExecuteCommand）2) 启动备用机组（ExecuteCommand）3) 通知调度 Agent（PublishEvent）

#### Scenario: 紧急动作经过约束验证
- **WHEN** 紧急动作执行前
- **THEN** 仍需经过 ConstraintAwareValidator 的硬约束检查（闭锁规则不可绕过），但跳过审批流

### Requirement: Agent 定时 tick 调度
系统 SHALL 在 AgentOrchestrator 中增加定时 tick 机制，支持不同 Agent 不同采样周期。

#### Scenario: 调度 Agent 5秒 tick
- **WHEN** DispatchAgent 注册 tick_interval: Duration::from_secs(5)
- **THEN** Orchestrator 每5秒调用该 Agent 的 tick()

#### Scenario: 运维 Agent 60秒 tick
- **WHEN** OperationAgent 注册 tick_interval: Duration::from_secs(60)
- **THEN** Orchestrator 每60秒调用该 Agent 的 tick()

### Requirement: 调度领域 Agent
系统 SHALL 实现 DispatchAgent，具备经济调度、AGC、调度计划生成能力。

#### Scenario: 经济调度
- **WHEN** DispatchAgent tick 触发且系统处于 Normal 状态
- **THEN** 基于当前负荷和机组成本曲线，计算最优发电出力分配，生成调度指令

#### Scenario: AGC 响应
- **WHEN** 频率偏差超过 ±0.1Hz
- **THEN** DispatchAgent 计算 ACE，自动调整调频机组出力

#### Scenario: 调度指令约束校验
- **WHEN** DispatchAgent 生成调度指令
- **THEN** 指令经过 ConstraintAwareValidator 校验，确保不违反安全约束

### Requirement: 运维领域 Agent
系统 SHALL 实现 OperationAgent，具备故障诊断、设备健康评估、巡检计划能力。

#### Scenario: 故障诊断
- **WHEN** OperationAgent 收到设备告警事件
- **THEN** 基于因果推理链诊断故障原因，生成检修建议

#### Scenario: 设备健康评估
- **WHEN** OperationAgent tick 触发
- **THEN** 评估管辖区域内设备健康状态，对退化设备生成预警

### Requirement: 自愈领域 Agent
系统 SHALL 实现 SelfHealingAgent，具备故障隔离、网络重构、恢复供电能力。

#### Scenario: 故障隔离
- **WHEN** SelfHealingAgent 收到故障事件（如馈线短路）
- **THEN** 自动定位故障区段，生成开关操作序列隔离故障

#### Scenario: 网络重构恢复供电
- **WHEN** 故障隔离完成后存在非故障停电区域
- **THEN** 搜索供电路径，生成联络开关操作序列恢复供电

#### Scenario: 自愈操作闭锁校验
- **WHEN** SelfHealingAgent 生成开关操作序列
- **THEN** 每步操作经过 InterlockingRuleEngine 校验，确保操作安全

### Requirement: 领域专用协作协议
系统 SHALL 实现电力系统特有的多 Agent 协作模式。

#### Scenario: 调度-运维协同
- **WHEN** DispatchAgent 需要调整某设备出力但该设备处于检修状态
- **THEN** 通过协作协议询问 OperationAgent 确认设备可用性

#### Scenario: 紧急联动
- **WHEN** SelfHealingAgent 执行故障隔离
- **THEN** 自动通知 DispatchAgent 调整发电出力以适应新拓扑

## MODIFIED Requirements

### Requirement: ReasoningEngine 增强
ReasoningInput 新增 `power_observation: Option<PowerObservation>` 字段，推理引擎可基于结构化观测做数值判断。RuleBasedEngine 增加数值比较规则（如 voltage < threshold）。

### Requirement: EmergencyResponsePlan 增强
EmergencyResponsePlan.actions 从 `Vec<String>` 升级为 `Vec<EmergencyAction>`，其中 EmergencyAction 可映射为 AgentAction。

### Requirement: Agent trait 增强
Agent trait 新增 `tick_interval() -> Duration` 方法（默认1秒），Orchestrator 据此调度 tick。
