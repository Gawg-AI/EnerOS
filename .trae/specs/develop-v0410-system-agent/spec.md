# v0.41.0 System Agent 核心 — Spec

## Why

v0.40.0 完成了能力管理器（CapabilityManager），Agent Runtime 已具备描述符、注册表、生命周期、心跳、崩溃恢复、能力令牌等基础能力，但缺少一个**统一管理 Agent 生命周期编排与系统资源监控**的 OS 级管理 Agent。System Agent 是最高权限 Agent，负责全局资源监控（CPU/内存/温度）、Agent 启停管理、故障检测与恢复触发，是 v0.42.0（故障恢复编排）的前置依赖。

## What Changes

### 新增模块

- `crates/agents/agent/src/system_agent/mod.rs` — SystemAgent 结构体 + 构造 + tick 主循环 + 系统统计
- `crates/agents/agent/src/system_agent/monitor.rs` — ResourceSource trait + ResourceMonitor + SystemConfig + SystemStats + SystemEvent + AgentResourceUsage
- `crates/agents/agent/src/system_agent/manager.rs` — impl SystemAgent 的 Agent 管理方法（start/stop/suspend/resume）
- `crates/agents/agent/tests/system_agent_test.rs` — 集成测试
- `docs/agents/system-agent-design.md` — 设计文档

### 修改文件

- `crates/agents/agent/src/error.rs` — 新增 3 个错误变体（SystemOverload / OomRisk / Overheat）
- `crates/agents/agent/src/lib.rs` — 新增 `pub mod system_agent;` + re-exports + 版本号 0.41.0
- 版本同步：`Cargo.toml`、`Makefile`、`.github/workflows/ci.yml`、`ci/src/gate.rs`

## Impact

- Affected specs: v0.40.0 (CapabilityManager — System Agent 可调用 freeze 挂起 Agent 能力), v0.38.0 (CrashRecovery — tick 集成), v0.37.0 (HeartbeatMonitor — tick 集成), v0.36.0 (AgentSpawner — start_agent 委托), v0.35.0 (LifecycleManager — suspend/resume/stop 状态转换)
- Affected code: `crates/agents/agent/src/` 全 crate

## 偏差声明（D1~D11）

| 偏差 | 蓝图设计 | 实际实现 | 理由 |
|------|---------|---------|------|
| **D1** | `run()` 无限循环 + `crate::time::sleep_ms()` | `tick(now: u64) -> Vec<SystemEvent>` 单步执行 | no_std 无 `sleep_ms`；无限循环不可测试；单步 tick 便于调用方控制周期 |
| **D2** | `crate::hal::get_cpu_usage()` 等直接调用 HAL | `ResourceSource` trait 抽象（依赖注入） | agent crate 不依赖 HAL crate；测试提供 mock source |
| **D3** | `HashMap<AgentId, AgentResourceUsage>` | `BTreeMap` / 不在 ResourceMonitor 中维护 agent_stats | no_std 无 HashMap；agent_stats 由 registry 提供，不冗余存储 |
| **D4** | 蓝图引用 `self.lifecycle.transition()` 但未声明 `lifecycle` 字段 | 新增 `lifecycle: Rc<RefCell<LifecycleManager>>` 字段 | suspend/resume/stop 需要状态转换，必须有 lifecycle 引用 |
| **D5** | `start_agent(&self, config: AgentConfig)` | `start_agent(&self, config: AgentConfig, now: u64)` | `spawner.spawn(config, now)` 需要 `now`（no_std 时间外部提供） |
| **D6** | `crate::log::warn!("System overheating")` | `tick()` 返回 `Vec<SystemEvent>` | agent crate 无 log 模块；事件列表便于测试与上层处理 |
| **D7** | 蓝图引用 `config: SystemConfig` 但未定义 | 定义 `SystemConfig` 结构体（3 字段：oom_threshold / overheat_threshold / monitor_interval_ms） | 蓝图未定义但必需 |
| **D8** | `ResourceMonitor::check_oom() -> Option<AgentId>` | `ResourceMonitor::is_oom(threshold) -> bool` + `SystemAgent::find_oom_victim() -> Option<AgentId>` | 监控器只负责阈值判断；victim 选择需访问 registry（优先级排序），职责分离 |
| **D9** | `stop_agent` 注释"状态 → Dead"但未指定方法 | `stop_agent` 使用 `force_state(id, Dead)` | Agent 可能处于 Suspended/Created 等无法直接转换到 Dead 的状态；`force_state` 绕过转换表 |
| **D10** | 蓝图 `system_agent.rs` + `system_agent/` 目录 | `system_agent/mod.rs` + 子模块 | 与现有 `capability/mod.rs` 模式一致 |
| **D11** | `tick()` 中 `handle_crash` 直接调用 | 先 `force_state(id, Error)` 再 `handle_crash` | `handle_crash` D9 要求 Agent 处于 Error 状态；心跳检测到 Unhealthy 时 Agent 可能仍在 Running |

## ADDED Requirements

### Requirement: SystemAgent 核心结构

SystemAgent SHALL 封装 AgentRegistry / AgentSpawner / CrashRecovery / HeartbeatMonitor / LifecycleManager / ResourceMonitor / SystemConfig，作为 OS 级管理 Agent。

#### Scenario: 构造 SystemAgent
- **WHEN** 调用 `SystemAgent::new(registry, spawner, recovery, heartbeat, lifecycle, config)`
- **THEN** 返回 SystemAgent 实例，monitor 初始化为空（cpu=0, mem=0, temp=0）

#### Scenario: 单步 tick 执行
- **WHEN** 调用 `tick(now)` 
- **THEN** 依次执行：资源监控 poll → 心跳 check → 故障恢复 → OOM 检查 → 过热检查
- **AND** 返回本周期产生的 `Vec<SystemEvent>`

### Requirement: ResourceMonitor 资源监控

ResourceMonitor SHALL 提供 CPU/内存/温度监控，通过 `ResourceSource` trait 抽象数据来源。

#### Scenario: 手动设置资源值
- **WHEN** 无 ResourceSource 时
- **THEN** 可通过 `set_values(cpu, mem_used, mem_total, temp)` 手动设置

#### Scenario: 自动轮询资源
- **WHEN** 配置了 ResourceSource 时
- **AND** 调用 `poll()`
- **THEN** 从 ResourceSource 读取最新 CPU/内存/温度值

#### Scenario: OOM 检测
- **WHEN** `mem_used / mem_total > oom_threshold`
- **THEN** `is_oom(threshold)` 返回 `true`

#### Scenario: 过热检测
- **WHEN** `temperature > overheat_threshold`
- **THEN** `is_overheat(threshold)` 返回 `true`

### Requirement: Agent 管理方法

SystemAgent SHALL 提供 start/stop/suspend/resume 四个 Agent 管理方法。

#### Scenario: 启动 Agent
- **WHEN** 调用 `start_agent(config, now)`
- **THEN** 委托 `spawner.spawn(config, now)` 启动 Agent
- **AND** 调用 `heartbeat.register(id, now)` 注册心跳
- **AND** 返回 `Ok(id)`

#### Scenario: 停止 Agent
- **WHEN** 调用 `stop_agent(id)`
- **THEN** `force_state(id, Dead)` 强制转为 Dead 状态
- **AND** `heartbeat.unregister(id)` 注销心跳
- **AND** `registry.unregister(id)` 注销注册
- **AND** 返回 `Ok(())`

#### Scenario: 挂起 Agent
- **WHEN** 调用 `suspend_agent(id)`
- **THEN** `lifecycle.transition(id, Suspended)` 转换状态
- **AND** 返回 `Ok(())`

#### Scenario: 恢复 Agent
- **WHEN** 调用 `resume_agent(id)`
- **THEN** `lifecycle.transition(id, Running)` 转换状态
- **AND** 返回 `Ok(())`

### Requirement: 系统统计

SystemAgent SHALL 提供 `get_system_stats()` 返回系统级统计信息。

#### Scenario: 获取系统统计
- **WHEN** 调用 `get_system_stats()`
- **THEN** 返回 `SystemStats` 包含 cpu_usage / mem_usage / temperature / agent_count / alive_agents / error_agents

### Requirement: OOM Victim 选择

SystemAgent SHALL 在 OOM 时选择最低优先级的存活 Agent 作为 victim。

#### Scenario: 选择 OOM victim
- **WHEN** `monitor.is_oom(threshold)` 为 true
- **AND** 调用 `find_oom_victim()`
- **THEN** 遍历 registry 中所有存活 Agent，返回 `priority` 最低的 AgentId
- **AND** 若无存活 Agent，返回 `None`

## MODIFIED Requirements

### Requirement: AgentError 新增 3 个变体

在现有 `AgentError` 枚举末尾（`NoCapability` 之后）新增：
- `SystemOverload` — 系统过载
- `OomRisk` — OOM 风险
- `Overheat { temp: f32 }` — 系统过热

保留 `#[derive(Debug, Clone, PartialEq)]`（不含 Eq，因 `Overheat` 含 f32）。
