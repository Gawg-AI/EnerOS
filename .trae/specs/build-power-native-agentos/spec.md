# 电力原生 AgentOS 层 Spec

## Why
当前 EnerOS 的 Agent 层缺乏电力系统核心安全机制：无 Agent 权限等级、无紧急响应路径、约束引擎与安全网关脱节、调度不感知拓扑、动作执行前无约束预校核。这些缺失使得当前 Agent 层无法满足电力系统"安全是硬法律"的根本要求。需要构建一个电力原生的 AgentOS 层，将安全约束、权限控制、紧急响应、拓扑感知作为操作系统级的一等公民。

## What Changes
- 新增 `AuthorityLevel` 权限等级体系（Observer / Operator / Supervisor / Emergency）
- 新增 `SystemOperatingState` 系统运行状态机（Normal / Alert / Emergency / Blackout / Restoration）
- 新增 `ConstraintAwareValidator` 约束感知动作验证器，联动 ConstraintEngine + SafetyGateway
- 新增 `EmergencyResponsePipeline` 紧急响应管线
- 新增 `TopologyAwareScheduler` 拓扑感知调度器
- 增强 `Agent` trait：权限声明、拓扑管辖、紧急处理
- 增强 `AgentContext`：ConstraintEngine 引用、权限凭证、系统状态
- 增强 `ActionDispatcher`：约束预校核、动作冲突检测、审批流
- 增强 `SafetyGateway`：动态约束验证、操作闭锁、权限校验
- 新增 `InterlockingRule` 操作闭锁规则引擎
- 新增 `ActionConflictResolver` 动作冲突仲裁器
- 新增 `AuditTrail` 完整审计追踪

## Impact
- Affected specs: eneros-core (新增类型), eneros-agent (核心重构), eneros-gateway (增强), eneros-constraint (联动接口)
- Affected code: eneros-core/src/types.rs, eneros-agent/src/{agent,context,orchestrator,dispatcher,collaboration}.rs, eneros-gateway/src/{gateway,safety}.rs, eneros-constraint/src/engine.rs

## ADDED Requirements

### Requirement: Agent 权限等级体系
系统 SHALL 为每个 Agent 定义权限等级 `AuthorityLevel`，约束其可执行的动作范围。

```
AuthorityLevel::Observer   — 只读，不可执行任何控制命令
AuthorityLevel::Operator   — 可执行常规操作（开关操作、参数调整）
AuthorityLevel::Supervisor — 可执行高危操作（负荷切除、系统解列），需审批
AuthorityLevel::Emergency  — 紧急越权，可绕过部分安全检查，仅紧急状态可用
```

#### Scenario: Observer Agent 尝试执行控制命令
- **WHEN** AuthorityLevel::Observer 的 Agent 产生 ExecuteCommand 动作
- **THEN** ActionDispatcher 拒绝该动作，返回 DispatchResult::Rejected("insufficient authority")

#### Scenario: Emergency 权限仅在紧急状态激活
- **WHEN** 系统处于 Normal 状态且 Agent 声明 AuthorityLevel::Emergency
- **THEN** 该 Agent 的 Emergency 权限不生效，降级为 Supervisor

### Requirement: 系统运行状态机
系统 SHALL 维护 `SystemOperatingState` 状态机，驱动调度策略、安全阈值、权限策略的动态调整。

```
Normal → Alert → Emergency → Blackout → Restoration → Normal
  ↑                                    ↓
  └────────────────────────────────────┘
```

#### Scenario: 约束违规触发状态升级
- **WHEN** ConstraintEngine 检测到 Critical 级别违规
- **THEN** SystemOperatingState 从 Normal 升级到 Alert，触发告警事件

#### Scenario: 紧急状态下的权限提升
- **WHEN** SystemOperatingState 进入 Emergency
- **THEN** Supervisor 级别的 Agent 自动获得临时 Emergency 权限

#### Scenario: 紧急状态下的安全阈值调整
- **WHEN** SystemOperatingState 进入 Emergency
- **THEN** SafetyGateway 的电压限值从 ±5% 临时放宽到 ±10%，频率限值从 ±0.2Hz 放宽到 ±0.5Hz

### Requirement: 约束感知动作验证器
系统 SHALL 在动作执行前进行约束感知验证，确保动作不会加剧现有约束违规或引入新的违规。

验证管线：Agent → Action → ConstraintAwareValidator → SafetyGateway → Device

#### Scenario: 动作预校核通过
- **WHEN** Agent 请求执行"合闸线路 L1"操作
- **AND** ConstraintEngine 当前无违规，SafetyGateway 校验通过
- **THEN** 动作被批准执行

#### Scenario: 动作预校核拒绝——加剧现有违规
- **WHEN** Agent 请求执行"切除电容器 C1"操作
- **AND** ConstraintEngine 当前存在电压偏低违规
- **AND** 预测切除 C1 将使电压进一步下降
- **THEN** 动作被拒绝，返回 DispatchResult::ConstraintRejected

#### Scenario: 动作预校核——需审批
- **WHEN** Agent 请求执行"负荷切除"操作
- **AND** 该操作标记为 require_approval
- **THEN** 动作进入 PendingApproval 状态，等待 Supervisor 级别 Agent 审批

### Requirement: 紧急响应管线
系统 SHALL 提供紧急响应管线，在紧急状态下自动执行预定义的紧急响应方案。

#### Scenario: 频率崩溃紧急响应
- **WHEN** 系统频率低于 49.5Hz（Emergency 状态）
- **THEN** 自动执行紧急响应方案：1) 切除非关键负荷 2) 启动备用机组 3) 通知调度 Agent

#### Scenario: 级联故障紧急响应
- **WHEN** 连续 3 条以上支路跳闸
- **THEN** 自动执行系统解列方案，隔离故障区域

#### Scenario: 紧急响应动作绕过部分检查
- **WHEN** EmergencyResponsePipeline 执行紧急动作
- **THEN** 跳过审批流和非关键安全检查，但保留硬约束检查（如带负荷拉刀闸仍然禁止）

### Requirement: 拓扑感知调度
系统 SHALL 基于电网拓扑结构调度 Agent，确保 Agent 只操作其管辖区域内的设备。

#### Scenario: Agent 管辖区域定义
- **WHEN** 注册 Agent 时指定 jurisdiction: ZoneId(1)
- **THEN** 该 Agent 只能对 ZoneId(1) 内的设备执行操作

#### Scenario: 跨区域操作需协调
- **WHEN** Agent A（管辖 Zone1）请求操作 Zone2 的设备
- **THEN** 操作被拒绝，建议通过 CollaborationProtocol 委托 Zone2 的 Agent 执行

#### Scenario: 拓扑变化后管辖区域更新
- **WHEN** 网络拓扑发生变化（如开关操作导致区域合并）
- **THEN** TopologyAwareScheduler 自动更新受影响 Agent 的管辖区域

### Requirement: 操作闭锁规则引擎
系统 SHALL 实现操作闭锁规则引擎，防止违反电力系统操作规程的设备操作。

#### Scenario: 带负荷拉刀闸禁止
- **WHEN** 断路器处于合闸状态时尝试拉开隔离开关
- **THEN** InterlockingRuleEngine 拒绝操作，返回"断路器未断开，禁止拉隔离开关"

#### Scenario: 接地线未拆除禁止合闸
- **WHEN** 设备上挂有接地线时尝试合闸
- **THEN** InterlockingRuleEngine 拒绝操作，返回"接地线未拆除，禁止合闸"

#### Scenario: 同期检查
- **WHEN** 尝试合环操作（合上联络开关）
- **THEN** InterlockingRuleEngine 检查两侧电压差和相角差，超限时拒绝操作

### Requirement: 动作冲突检测与仲裁
系统 SHALL 检测多个 Agent 在同一时间步内产生的冲突动作，并通过仲裁机制解决。

#### Scenario: 冲突检测
- **WHEN** Agent A 请求"升压变压器 T1"，Agent B 请求"降压变压器 T1"
- **THEN** ActionConflictResolver 检测到冲突，触发仲裁

#### Scenario: 优先级仲裁
- **WHEN** 冲突动作来自不同权限等级的 Agent
- **THEN** 高权限 Agent 的动作优先执行

#### Scenario: 同级仲裁
- **WHEN** 冲突动作来自相同权限等级的 Agent
- **THEN** 基于拓扑优先级（更近的 Agent 优先）或时间优先仲裁

### Requirement: 完整审计追踪
系统 SHALL 为每个 Agent 动作记录完整审计日志，包括决策依据、约束检查结果、审批链。

#### Scenario: 审计日志记录
- **WHEN** Agent 执行任何动作
- **THEN** AuditTrail 记录：agent_id, authority_level, action, constraint_check_result, approval_chain, timestamp, reasoning_summary

#### Scenario: 审计日志不可篡改
- **WHEN** 审计日志已写入
- **THEN** 日志只能追加，不可修改或删除

### Requirement: Agent 增强接口
系统 SHALL 增强 Agent trait，增加权限声明、拓扑管辖、紧急处理能力。

#### Scenario: Agent 声明权限等级
- **WHEN** 实现 Agent trait 时声明 authority_level() -> AuthorityLevel
- **THEN** ActionDispatcher 根据该等级约束 Agent 的动作范围

#### Scenario: Agent 声明管辖区域
- **WHEN** 实现 Agent trait 时声明 jurisdiction() -> Vec<ZoneId>
- **THEN** TopologyAwareScheduler 根据该区域约束 Agent 的操作范围

#### Scenario: Agent 紧急处理
- **WHEN** 系统进入 Emergency 状态且 Agent 实现 handle_emergency()
- **THEN** Orchestrator 优先调用 handle_emergency() 而非 handle_event()

## MODIFIED Requirements

### Requirement: AgentContext 增强
AgentContext 新增 ConstraintEngine 引用、权限凭证、系统运行状态、紧急模式标志。

### Requirement: SafetyGateway 增强
SafetyGateway 新增动态约束验证（与 ConstraintEngine 联动）、权限校验、操作闭锁检查。

### Requirement: ConstraintEngine 增强
ConstraintEngine 新增动作预校核方法 `check_action_feasibility()`、约束违规主动通知、动态阈值调整。

### Requirement: ActionDispatcher 增强
ActionDispatcher 新增约束预校核步骤、动作冲突检测、审批流、紧急旁路通道。

### Requirement: CollaborationProtocol 增强
CollaborationProtocol 新增权限与角色绑定、拓扑感知任务分配、任务依赖 DAG、冲突解决协议。
