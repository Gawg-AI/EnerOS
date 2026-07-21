# v0.74.0 MVP 端到端集成 — 储能自治场景 Spec

> **Skill**: andrej-karpathy-skills-main
> **版本**: v0.74.0（Phase 1 P1-L MVP 集成第三层 / Phase 1 出口验证 / ★瓶颈版本）
> **蓝图依据**: `蓝图/phase1.md` §v0.74.0（行 16168~16594）
> **change-id**: `develop-v0740-mvp-scenario`

---

## Why

Phase 1 单机 MVP 的最终里程碑版本。需要将 v0.72.0 Energy/Market Agent 与 v0.73.0 Device Agent 统一编排，完成储能自治端到端场景（电价→双脑决策→设备执行），并提供收益对比基准（vs 传统规则 EMS），为 Phase 1 三项出口标准（autonomous 24h / 双脑链路 < 2s / 收益提升 ≥ 10%）提供可验证的代码骨架。

本版本不实现 24h 真实运行（那是集成测试层的事），而是提供可单元测试的 `MvpOrchestrator` 编排逻辑 + `RevenueComparator` 收益对比器 + `TraditionalEms` 基准策略，使三项出口标准可在 Rust 单元测试层验证逻辑正确性。

---

## What Changes

- **新增 crate** `eneros-mvp-scenario`（位于 `crates/agents/mvp-scenario/`）
- **新增 `MvpOrchestrator`** — 编排 Energy/Market/Device 三个 Agent 协同完成储能自治场景
- **新增 `RevenueComparator`** — 收益对比器，追踪双脑 EMS vs 传统 EMS 收益
- **新增 `TraditionalEms`** — 传统 EMS 基准策略（规则：谷充峰放）
- **新增 `MvpError`** — MVP 编排错误类型
- **版本号 0.73.0 → 0.74.0**（Cargo.toml / Makefile / ci.yml / ci/src/gate.rs）
- **无外科手术式变更**（不修改 v0.72.0/v0.73.0 代码，仅依赖其 pub API）

---

## Impact

- **Affected specs**: v0.72.0（Energy/Market Agent）、v0.73.0（Device Agent）、v0.66.0（ScheduleConfig/ScheduleResult）、v0.71.0（DualBrainCoordinator，间接）
- **Affected code**: 新增 crate，无现有 crate 修改
- **依赖**: `eneros-energy-market-agent` / `eneros-device-agent` / `eneros-energy-lp-model` / `eneros-agent`
- **解锁**: v0.74.0 完成 → Phase 1 出口验证完成 → 进入 Phase 2（v0.75.0 多机联邦）

---

## ADDED Requirements

### Requirement: MVP Orchestrator

系统 SHALL 提供 `MvpOrchestrator` 结构体，统一调度 `EnergyAgent` / `MarketAgent` / `DeviceAgent` 三个 Agent 协同完成储能自治场景的一个 tick 周期。

#### Scenario: 构造与默认初始化

- **WHEN** 调用 `MvpOrchestrator::new_default(now_ms)`
- **THEN** 返回包含 3 个 `new_default` 构造的 Agent + 空 `RevenueComparator` + `running = false` + `tick_count = 0` 的实例

#### Scenario: 启动生命周期

- **WHEN** 调用 `start(now_ms)`
- **THEN** 3 个 Agent 的 `on_start(now_ms)` 依次调用，全部转 `Running` 状态，`running = true`

#### Scenario: 单 tick 执行

- **WHEN** 调用 `tick(now_ms)` 且 `running == true`
- **THEN** 执行顺序：market.on_tick → 转发市场数据 → energy.on_tick → 记录双脑收益 + 传统 EMS 收益 → device.on_tick → `tick_count += 1`，返回 `MvpTickReport`

#### Scenario: 停止生命周期

- **WHEN** 调用 `stop(now_ms)`
- **THEN** 3 个 Agent 的 `on_stop(now_ms)` 依次调用，全部转 `Dead` 状态，`running = false`

#### Scenario: 未运行时 tick 报错

- **WHEN** 调用 `tick(now_ms)` 且 `running == false`
- **THEN** 返回 `Err(MvpError::NotRunning)`

#### Scenario: 市场数据流

- **WHEN** `market_agent` 的 `market_channel` 含数据且调用 `tick`
- **THEN** 数据从 `market_agent.market_channel` 转发到 `energy_agent.market_channel`，`energy_agent.current_price` 更新

#### Scenario: 收益追踪

- **WHEN** `energy_agent.current_schedule` 为 `Some(schedule)` 且调用 `tick`
- **THEN** `revenue_comparator.record_dual_brain(schedule.total_revenue_yuan)` 被调用，同时 `TraditionalEms::schedule()` 计算传统收益并 `record_traditional()`

---

### Requirement: Revenue Comparator

系统 SHALL 提供 `RevenueComparator` 结构体，追踪双脑 EMS 与传统 EMS 的收益记录，计算提升百分比。

#### Scenario: 空构造

- **WHEN** 调用 `RevenueComparator::new()`
- **THEN** 返回空实例（dual_brain_revenue 和 traditional_revenue 均为空 Vec）

#### Scenario: 记录收益

- **WHEN** 调用 `record_dual_brain(100.0)` 和 `record_traditional(80.0)`
- **THEN** 两个 Vec 各有 1 条记录

#### Scenario: 提升百分比计算

- **WHEN** dual_brain 总收益 = 100，traditional 总收益 = 80
- **THEN** `improvement_pct()` 返回 `25.0`（即 `(100-80)/80 * 100`）

#### Scenario: 传统收益为零

- **WHEN** `traditional_revenue` 总和为 0
- **THEN** `improvement_pct()` 返回 `f64::INFINITY`

#### Scenario: 生成报告

- **WHEN** 调用 `report()`
- **THEN** 返回包含双脑总收益、传统总收益、提升百分比、是否达标（≥ 10%）的结构化字符串

---

### Requirement: Traditional EMS Baseline

系统 SHALL 提供 `TraditionalEms` 结构体，作为传统规则策略 EMS 的对比基准。

#### Scenario: 谷时充电

- **WHEN** 某时段电价 < 0.3 元/kWh
- **THEN** 调度条目 `charge_power_kw = config.pcs_power_kw`，`discharge_power_kw = 0.0`

#### Scenario: 峰时放电

- **WHEN** 某时段电价 > 0.8 元/kWh
- **THEN** 调度条目 `charge_power_kw = 0.0`，`discharge_power_kw = config.pcs_power_kw`

#### Scenario: 平时保持

- **WHEN** 某时段电价 ∈ [0.3, 0.8]
- **THEN** 调度条目 `charge_power_kw = 0.0`，`discharge_power_kw = 0.0`

#### Scenario: 全周期调度

- **WHEN** 调用 `schedule(current_price, soc)` 且 config 有 96 时段
- **THEN** 返回 `ScheduleResult`，包含 96 个 `ScheduleEntry`，`total_revenue_yuan` 为各时段收益之和

---

### Requirement: MVP Error Type

系统 SHALL 提供 `MvpError` 枚举，覆盖 Agent 运行时错误和编排错误。

#### Scenario: Agent 错误转换

- **WHEN** Energy/Market/Device Agent 的 `on_tick` 返回 `AgentRuntimeError`
- **THEN** 可通过 `From<AgentRuntimeError> for MvpError` 转换为 `MvpError::AgentError`

#### Scenario: 未运行错误

- **WHEN** `running == false` 时调用 `tick`
- **THEN** 返回 `MvpError::NotRunning`

---

## MODIFIED Requirements

无。本版本不对 v0.72.0/v0.73.0 代码做任何外科手术式变更，仅依赖其 `pub` API。

---

## REMOVED Requirements

无。

---

## 偏差声明（D1~D15，Karpathy "Think Before Coding"）

| 偏差 | 蓝图原文 | 本版本处理 | 理由 |
|------|---------|-----------|------|
| **D1** | `log::info!("=== EnerOS MVP 储能自治场景启动 ===")` 等 | 移除日志；状态/错误通过返回值传递 | no_std 无 `log` crate；与 v0.57~v0.73 一致 |
| **D2** | `SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()` | `now_ms: u64` 参数 | no_std 合规：`SystemTime` 不可用；与 v0.57~v0.73 一致 |
| **D3** | `std::thread::sleep(Duration::from_secs(15))` 无限循环 | `tick(now_ms)` 单步方法；循环由集成测试驱动 | no_std 无 `std::thread`；24h 测试是集成层关注，非单元测试；Karpathy "Simplicity First" |
| **D4** | `AgentRegistry::new()` + `Box::new(self.energy_agent.clone())` | Orchestrator 直接持有 3 个 Agent（非 clone） | Agent 未实现 `Clone`；v0.33.0 `AgentRegistry` 管理 `AgentDescriptor` 而非运行时 Agent 实例；直接持有更简单（Karpathy 简化原则） |
| **D5** | `system_agent: SystemAgent` + `self.system_agent.on_tick()` | 跳过 SystemAgent | SystemAgent（v0.41.0）为监控用途，对三项出口标准（autonomous/延迟/收益）非必需；Karpathy "don't add what wasn't asked" |
| **D6** | `watchdog: WatchdogDegradeFlow` + `self.watchdog.start()` + `self.watchdog.feed()` | 跳过 Watchdog 集成 | Watchdog（v0.58.0）为运行时基础设施；MVP 编排器聚焦 3-Agent 协作 + 收益对比逻辑；Watchdog 喂狗属集成层关注 |
| **D7** | `agent.on_start()?`（无 `now_ms` 参数） | `agent.on_start(now_ms)?` | v0.72.0 `AgentRuntime` trait 签名要求 `now_ms: u64`；与 v0.72.0/v0.73.0 一致 |
| **D8** | `MarketAgent::new("market-server:8080")`（字符串服务器地址） | `MarketAgent::new_default(now_ms)` | v0.72.0 实际 API `MarketAgent::new(name, source: Box<dyn MarketDataSource>, now_ms)` 接收数据源 trait 对象，非服务器地址字符串；MVP 用 Mock 即可 |
| **D9** | `self.energy_agent.coordinator.execute(&energy_state)?` 直接访问 coordinator | `self.energy_agent.on_tick(now_ms)?` | 通过 `AgentRuntime` trait 调用，封装更清晰；Energy Agent 内部构建 `RealtimeState` 并调用 coordinator（v0.72.0 D11 已实现） |
| **D10** | `fn collect_energy_state(&self) -> SystemState { ... }` | 跳过（Energy Agent 内部自建状态） | v0.72.0 `EnergyAgent::on_tick` 已从 `current_price` 构建 `RealtimeState`（v0.72.0 D11），Orchestrator 无需重复构建 |
| **D11** | `self.market_agent.price_cache[0]` | `self.market_agent.price_cache` 字段直接访问 | v0.72.0 `MarketAgent.price_cache` 为 `pub Vec<f64>`，可直接访问 |
| **D12** | 分离的 `revenue_tracker: RevenueTracker` + `RevenueComparator` | 合并为单一 `RevenueComparator` | 两者职责重叠（追踪收益）；Karpathy 简化原则——单一结构体即可 |
| **D13** | `TraditionalEms::schedule(&self, state: &SystemState)` | `TraditionalEms::schedule(&self, current_price: f64, soc: f64)` | 蓝图仅用 `state.soc`；传基本类型避免 `SystemState` 依赖耦合（v0.67.0 `SystemState` 字段不同） |
| **D14** | Python 24h 端到端测试 + GPU 优先（llama.cpp） | Rust 集成测试（Mock 双脑 + Mock 设备） | no_std Rust crate 无 Python；MockSolver/DualBrainMockEngine 为 CPU-only；24h 真实运行 + GPU 推理属集成测试层（蓝图 §6.1 Python 测试在硬件/QEMU 环境运行） |
| **D15** | `revenue_yuan: (discharge - charge) * price * self.config.period_hours` | 保持蓝图公式 | 与蓝图一致； TraditionalEms 用 `ScheduleConfig` 的 `period_hours` 计算收益 |

---

## no_std 合规

本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
仅使用 `alloc::*` / `core::*`，可交叉编译到 `aarch64-unknown-none`。

- 无 `use std::*`
- 无 `panic!` / `todo!` / `unimplemented!`
- 无 `SystemTime::now()` / `thread::sleep`
- 无 `log::*` 宏
- 子模块不重复 `#![cfg_attr(not(test), no_std)]`（继承 lib.rs）

---

## Karpathy 四原则应用

1. **Think Before Coding**：15 项偏差显式声明，说明蓝图与实际 API 的差异
2. **Simplicity First**：跳过 SystemAgent/Watchdog（非出口标准必需）；合并 RevenueTracker/Comparator；不引入 v0.33.0 AgentRegistry
3. **Surgical Changes**：不对 v0.72.0/v0.73.0 做任何修改，仅依赖 pub API
4. **Goal-Driven Execution**：24 个集成测试验证 start/tick/stop/收益追踪/市场数据流/传统 EMS 策略
