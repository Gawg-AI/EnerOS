# v0.42.0 + v0.42.1 — 故障恢复编排 + 本地 HMI Spec

> 覆盖版本：v0.42.0（System Agent 故障恢复编排）+ v0.42.1（本地 HMI / 运维接口，刚性子版本 R3）
> 蓝图依据：`蓝图/phase1.md` §7133-7447
> 前置版本：v0.41.0（System Agent 核心，已完成）
> 解锁版本：v0.43.0（用户态驱动框架）

## Why

v0.41.0 实现了 SystemAgent 核心与单 Agent 崩溃恢复，但当多个 Agent 同时故障时无序恢复可能导致依赖错误（如 Energy Agent 依赖 System Agent，若 System Agent 未恢复则 Energy Agent 恢复后也无法正常工作）。v0.42.0 实现依赖图 + 拓扑排序 + 有序恢复编排。

v0.42.1 作为刚性子版本 R3，实现本地 HMI 运维接口（串口控制台 + Web UI + 手动控制审批），为现场部署提供人工干预通道，满足 Phase 1 出口"autonomous 运行 + 人工干预入口"要求。

## What Changes

### v0.42.0 故障恢复编排
- 新增 `crates/agents/agent/src/system_agent/dependency.rs` — 依赖图（DependencyGraph + 拓扑排序 + 循环检测）
- 新增 `crates/agents/agent/src/system_agent/recovery_orchestrator.rs` — 恢复编排器（RecoveryOrchestrator + 调度队列 + 优先级）
- 修改 `crates/agents/agent/src/error.rs` — 新增 `CircularDependency` + `RecoveryFailed { agent: AgentId, attempts: u32 }` 错误变体
- 修改 `crates/agents/agent/src/system_agent/mod.rs` — 声明新子模块 + re-exports
- 修改 `crates/agents/agent/src/lib.rs` — re-exports + 版本号

### v0.42.1 本地 HMI
- 新增 crate `crates/agents/hmi/`（eneros-hmi）— 本地人机交互接口
- 新增 `crates/agents/hmi/src/lib.rs` — HmiFrame / SystemState / LocalHmi trait / 类型定义
- 新增 `crates/agents/hmi/src/approval.rs` — 手动控制审批状态机
- 新增 `crates/agents/hmi/src/console.rs` — 串口控制台文本渲染
- 新增 `crates/agents/hmi/src/web.rs` — 最小 HTTP/JSON 类型（无实际 TCP 服务器）
- 修改根 `Cargo.toml` — workspace members 新增 `crates/agents/hmi`
- 新增 `docs/runtime/local-hmi-design.md` — HMI 设计文档
- 新增 `configs/hmi.toml` — HMI 配置模板

### 版本同步
- 0.41.0 → 0.42.1（Cargo.toml / Makefile / ci.yml / gate.rs）

## Impact

- **Affected specs**: v0.41.0 System Agent（SystemAgent 不修改，RecoveryOrchestrator 为独立工具）
- **Affected code**:
  - `crates/agents/agent/src/error.rs`（+2 变体）
  - `crates/agents/agent/src/system_agent/`（+2 文件，mod.rs 修改）
  - `crates/agents/agent/src/lib.rs`（re-exports + 版本）
  - `crates/agents/hmi/`（全新 crate）
  - 根 `Cargo.toml`（+1 member）
- **New dependencies**: eneros-hmi 依赖 eneros-agent（SystemState 类型引用）

## ADDED Requirements

### Requirement: 依赖图（DependencyGraph）

系统 SHALL 提供依赖图数据结构，支持添加依赖关系、拓扑排序、循环依赖检测。

#### Scenario: 添加依赖
- **WHEN** 调用 `add_dependency(agent, depends_on)`
- **THEN** 依赖关系被记录到图中

#### Scenario: 拓扑排序
- **WHEN** 调用 `topological_sort()`
- **THEN** 返回依赖有序的 AgentId 列表（被依赖的 Agent 在前）

#### Scenario: 循环依赖检测
- **WHEN** 图中存在循环依赖
- **AND** 调用 `topological_sort()`
- **THEN** 返回 `Err(CircularDependency)`

#### Scenario: 依赖可恢复检查
- **WHEN** 调用 `can_recover(agent)`
- **AND** agent 的所有依赖已恢复或已失败
- **THEN** 返回 `true`

### Requirement: 恢复编排器（RecoveryOrchestrator）

系统 SHALL 提供恢复编排器，按依赖顺序调度多 Agent 恢复。

#### Scenario: 批量调度
- **WHEN** 多个 Agent 同时故障
- **AND** 调用 `schedule_batch(agents)`
- **THEN** 所有 Agent 加入恢复队列

#### Scenario: 有序恢复
- **WHEN** 调用 `process_next()`
- **THEN** 返回下一个可恢复的 Agent（所有依赖已恢复或已失败）
- **AND** 若无可恢复 Agent，返回 `None`

#### Scenario: 恢复成功回调
- **WHEN** Agent 恢复成功
- **AND** 调用 `on_agent_recovered(agent)`
- **THEN** Agent 从 in_progress 移到 recovered 集合
- **AND** 依赖该 Agent 的其他 Agent 变为可恢复

#### Scenario: 恢复失败回调
- **WHEN** Agent 恢复失败
- **AND** 调用 `on_agent_failed(agent)`
- **THEN** Agent 从 in_progress 移到 failed 集合
- **AND** 依赖该 Agent 的其他 Agent 仍可恢复（§8.5：失败依赖不阻塞）

#### Scenario: 恢复完成
- **WHEN** 队列为空且无 in_progress
- **THEN** `is_complete()` 返回 `true`

### Requirement: 恢复优先级

系统 SHALL 定义恢复优先级枚举（Critical/High/Normal/Low），支持按优先级排序恢复队列。

#### Scenario: 优先级映射
- **WHEN** Agent 类型为 System
- **THEN** 优先级为 Critical
- **WHEN** Agent 类型为 Energy
- **THEN** 优先级为 High

### Requirement: HMI 数据模型

系统 SHALL 提供 HMI 帧数据结构，包含系统状态、活动告警、待审批操作、可用手动操作。

#### Scenario: 渲染 HMI 帧
- **WHEN** 调用 `render_hmi_screen(&system_state)`
- **THEN** 返回 HmiFrame，包含 system_state / active_alarms / pending_approvals / manual_actions

### Requirement: 审批状态机

系统 SHALL 提供手动控制审批状态机，支持 提交→待审批→二次确认→执行/拒绝 流程。

#### Scenario: 提交手动操作
- **WHEN** 调用 `submit_manual_action(action)`
- **THEN** 操作进入 Pending 状态，返回 ApprovalId

#### Scenario: 审批通过
- **WHEN** 调用 `approve_action(id)` 进行二次确认
- **THEN** 状态转为 Approved，操作可执行

#### Scenario: 审批拒绝
- **WHEN** 调用 `reject_action(id)`
- **THEN** 状态转为 Rejected

### Requirement: 串口控制台渲染

系统 SHALL 提供串口控制台文本渲染器，输出系统状态菜单。

#### Scenario: 渲染状态菜单
- **WHEN** 调用 `render(&system_state)`
- **THEN** 返回文本字符串，包含 Agent 列表、系统资源、告警信息

### Requirement: Web UI 类型

系统 SHALL 提供最小 HTTP 请求/响应类型，支持 JSON 序列化。

#### Scenario: 状态查询
- **WHEN** 收到 `GET /status` 请求
- **THEN** 返回 SystemState 的 JSON 表示

## MODIFIED Requirements

### Requirement: AgentError

在现有 AgentError 枚举基础上新增 2 个变体：

```rust
/// 循环依赖（依赖图中存在环）
CircularDependency,
/// 恢复失败（超过重试次数）
RecoveryFailed { agent: AgentId, attempts: u32 },
```

Display 实现：
- `CircularDependency` => `"circular dependency detected"`
- `RecoveryFailed { agent, attempts }` => `"recovery failed: agent {:?} after {} attempts"`

## 偏差声明

| 偏差 | 蓝图设计 | 实际实现 | 理由 |
|------|---------|---------|------|
| **D1** | `HashMap<AgentId, Vec<AgentId>>` / `HashSet<AgentId>` | `BTreeMap` / `BTreeSet` | no_std 无 HashMap/HashSet（需 `hashbrown` 依赖）；BTreeMap 零依赖且有序 |
| **D2** | 蓝图 `new()` 用 `HashMap::new()` 但 struct 声明 `BTreeMap` | 统一使用 `BTreeMap::new()` | 蓝图内部不一致，修正为 BTreeMap |
| **D3** | 接口定义有 `schedule_recovery(agent)` + `pending_count()` 但关键代码未实现 | 实现两者 | 接口完整性 |
| **D4** | `RecoveryPriority` 枚举定义但未集成 | 定义枚举 + `priority_of(agent_type)` 辅助方法，process_next 按优先级排序 | 蓝图 §9.1 要求"依赖图/编排/有序恢复"，优先级排序增强有序性 |
| **D5** | 蓝图 `DependencyGraph` 与 `RecoveryOrchestrator` 分离 | 保持分离：dependency.rs + recovery_orchestrator.rs | 蓝图交付物清单明确两个文件；职责分离（图操作 vs 调度） |
| **D6** | 蓝图提到 `CircularDependency` 错误但未实现检测算法 | Kahn 算法拓扑排序，环存在时返回 Err | 蓝图 §8.1 风险"循环依赖导致死锁"需检测 |
| **D7** | 蓝图 `can_recover` 检查 `recovered.contains(d) \|\| failed.contains(d)` | 保持原逻辑（失败依赖不阻塞恢复） | 蓝图 §8.5 决策：依赖 Dead 时继续恢复 |
| **D8** | HMI crate 未声明 no_std | `#![cfg_attr(not(test), no_std)]` | 蓝图 §43.1 全项目 no_std |
| **D9** | 蓝图 HMI 直接操作 UART/TCP | 抽象 I/O 为 trait（`ConsoleOutput` / `HttpHandler`），HMI 仅提供逻辑 | 与 v0.41.0 ResourceSource 同模式；agent/hmi crate 不直接依赖硬件驱动 |
| **D10** | 蓝图 Web UI 基于 smoltcp HTTP | 仅定义 HTTP 请求/响应类型 + JSON 序列化，不实现 TCP 服务器 | smoltcp 集成是调用方职责；保持 crate 零网络依赖 |
| **D11** | 蓝图 `SystemState` 引用未定义类型（NetworkStatus/PowerState/AgentStateSummary） | 在 hmi crate 内定义这些类型 | 类型不存在于其他 crate；HMI 视角的系统状态 |
| **D12** | 蓝图 `configs/hmi.toml` | 创建配置模板文件（无解析器，仅文档化配置项） | 配置解析需 toml crate 依赖；Phase 1 简化为编译时常量 |
| **D13** | 蓝图串口控制台 VT100 转义码 | 文本渲染返回 `String`，VT100 转义码可选（由调用方决定是否启用） | 渲染逻辑与终端解耦 |
| **D14** | 蓝图审批状态需持久化 | 内存状态机（不持久化） | 持久化需文件系统；Phase 1 简化为内存 |
