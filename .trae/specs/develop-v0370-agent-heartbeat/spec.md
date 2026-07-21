# v0.37.0 — Agent 心跳与健康检查 Spec

> **蓝图依据**：`蓝图/phase1.md` §v0.37.0（行 5954~6164）
> **开发原则**：Karpathy 四原则 — Think Before Coding / Simplicity First / Surgical Changes / Goal-Driven Execution
> **子版本检查**：蓝图 grep `v0.37.[1-9]` 返回 0 匹配，本任务为单版本开发（无增强子版本）。

## Why

v0.36.0 实现了 Agent 启动器，Agent 可进入 Running 状态，但系统无法检测 Agent 是否存活。v0.37.0 实现 `HeartbeatMonitor` 心跳监控器（1s 周期、3 次超时=故障）与 `HealthCheck` 健康检查 trait，为 v0.38.0 崩溃恢复提供故障检测基础。

## What Changes

- **新增** `crates/agents/agent/src/health.rs` — `HealthStatus` 枚举 + `HealthCheck` trait
- **新增** `crates/agents/agent/src/heartbeat.rs` — `HeartbeatMonitor` / `HeartbeatState`
- **修改** `crates/agents/agent/src/error.rs` — 追加 2 个错误变体（`HeartbeatTimeout` / `AgentUnhealthy`）
- **修改** `crates/agents/agent/src/lib.rs` — 声明 `health` + `heartbeat` 模块 + re-export + VERSION → "0.37.0"
- **新增** `crates/agents/agent/tests/heartbeat_test.rs` — 集成测试
- **新增** `docs/agents/agent-heartbeat-design.md` — 设计文档
- **版本标识同步**：根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`

## Impact

- **Affected specs**：v0.33.0（AgentDescriptor，被引用不修改）/ v0.34.0（AgentRegistry，被引用不修改）/ v0.35.0（LifecycleManager，被引用不修改）/ v0.36.0（AgentSpawner，被引用不修改）/ v0.38.0（崩溃恢复，将使用 HeartbeatMonitor 检测故障）
- **Affected code**：
  - `crates/agents/agent/src/health.rs`（新增）
  - `crates/agents/agent/src/heartbeat.rs`（新增）
  - `crates/agents/agent/src/error.rs`（追加 2 变体）
  - `crates/agents/agent/src/lib.rs`（追加模块声明与 re-export）
  - `crates/agents/agent/tests/heartbeat_test.rs`（新增）
  - `docs/agents/agent-heartbeat-design.md`（新增）
  - 根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`（版本号）
- **回归保护**：v0.31.0~v0.36.0 所有测试必须继续通过

## 设计决策与偏差声明（Think Before Coding）

### 偏差 D1：使用 `BTreeMap` 而非蓝图的 `HashMap`

**蓝图设计**：§3 接口定义使用 `HashMap<AgentId, HeartbeatState>`，§4.5 关键代码使用 `BTreeMap`。

**问题**：蓝图内部不一致。`HashMap` 需要 `std::collections::HashMap` 或外部 crate（如 `hashbrown`），而本项目零外部依赖（v0.34.0 既定约定）。`BTreeMap` 来自 `alloc::collections::BTreeMap`，零依赖。

**决策**：统一使用 `BTreeMap<AgentId, HeartbeatState>`，与 §4.5 关键代码一致，与 v0.34.0 AgentRegistry 一致。

### 偏差 D2：`register()` 追加 `now: u64` 参数

**蓝图设计**：§4.5 `register()` 调用 `crate::time::now_ms()` 获取当前时间戳。

**问题**：v0.37.0 crate 无 `time` 模块，`crate::time::now_ms()` 不存在。no_std 无系统时钟（v0.33.0 既定约定：`now: u64` 由外部提供）。

**决策**：`register(&mut self, id: AgentId, now: u64)` 追加 `now` 参数，用 `now` 初始化 `last_heartbeat`。

**理由**：与 v0.33.0 `AgentDescriptor::new(agent_type, name, now)` + v0.36.0 `spawn(config, now)` 保持一致的 no_std 时间约定。

### 偏差 D3：`HealthStatus` 追加 derives

**蓝图设计**：§3 未声明 `HealthStatus` 的 derives。

**问题**：§4.5 `check()` 返回 `Vec<(AgentId, HealthStatus)>` 并执行 `state.status.clone()`，需要 `Clone`。测试需要 `Debug` + `PartialEq`。枚举简单（4 变体无数据），可 `Copy`。

**决策**：`#[derive(Clone, Copy, Debug, PartialEq, Eq)]`。

### 偏差 D4：`HeartbeatState` / `HeartbeatMonitor` 追加 derives

**蓝图设计**：§3/§4.5 未声明 derives。

**问题**：可观测性需要 `Debug`（§9.6 "健康状态可查"）。`HeartbeatState` 含 `HealthStatus`（已 Copy），可 `Clone`。

**决策**：
- `HeartbeatState`: `#[derive(Clone, Debug)]`
- `HeartbeatMonitor`: `#[derive(Debug)]`（不 Clone，含 BTreeMap 可 Clone 但无必要）

### 偏差 D5：新增 2 个 `AgentError` 变体

**蓝图设计**：§4.4 定义 `HeartbeatTimeout { agent_id, missed }` 和 `AgentUnhealthy { agent_id }`。

**决策**：追加到 `AgentError` 枚举末尾（v0.36.0 的 11 个变体之后）。`AgentId` 是 `Copy`（id.rs:11 确认），变体可 derive `Clone`。

### 偏差 D6：`HeartbeatMonitor` 独立运行（不引用 registry/lifecycle）

**蓝图设计**：§3/§4.5 的 `HeartbeatMonitor` 仅含 `agents: BTreeMap` + 配置字段，无 registry/lifecycle 引用。

**决策**：遵循蓝图设计 — `HeartbeatMonitor` 是独立监控器，维护自己的 `BTreeMap<AgentId, HeartbeatState>`。与 `AgentRegistry` 是两个独立数据源。

**理由**：
1. 心跳监控与注册表管理是不同关注点（单一职责）
2. v0.38.0 崩溃恢复将集成 HeartbeatMonitor + LifecycleManager + AgentSpawner
3. 避免引入 `Rc<RefCell<...>>` 共享引用（Simplicity First）

**代价**：需手动同步 register/unregister（调用方需同时注册到 registry 和 heartbeat monitor）。

### 偏差 D7：`check()` 设置 `Unhealthy` 而非 `Dead`

**蓝图设计**：§4.3 mermaid 写 "Unhealthy/Dead"，§4.5 代码写 `Unhealthy`，§6.5 测试计划写 "3s 后 Dead"。

**问题**：蓝图内部不一致。`HealthStatus::Dead` 何时设置不明确。

**决策**：v0.37.0 `check()` 在 `missed_count >= max_missed` 时设置 `Unhealthy`（遵循 §4.5 代码）。`Dead` 状态由 v0.38.0 崩溃恢复机制设置（`force_state(Dead)`）。

**理由**：
1. §7 验收标准仅要求"检测故障"（=Unhealthy），未要求 Dead
2. Dead 是终态，应由恢复机制（v0.38.0）在重启失败后设置，非心跳监控器直接设置
3. §6.5 "3s 后 Dead" 描述端到端行为（含 v0.38.0），v0.37.0 测试为"3s 后 Unhealthy"

## ADDED Requirements

### Requirement: HealthStatus 健康状态枚举

系统 SHALL 提供 `HealthStatus` 枚举（4 变体）：
- `Healthy` — 健康（最近周期内有心跳）
- `Degraded` — 降级（1+ 次心跳缺失，但未达阈值）
- `Unhealthy` — 不健康（心跳缺失达阈值）
- `Dead` — 已死亡（由 v0.38.0 崩溃恢复设置，v0.37.0 不设置）

derive `Clone, Copy, Debug, PartialEq, Eq`（D3 偏差）。

#### Scenario: Healthy → Degraded → Unhealthy 演进
- **WHEN** Agent 注册后，1 个心跳周期无心跳
- **THEN** `check()` 返回 `Degraded`
- **WHEN** 心跳缺失达 `max_missed` 次
- **THEN** `check()` 返回 `Unhealthy`

### Requirement: HeartbeatState 心跳状态

系统 SHALL 提供 `HeartbeatState` 结构体（4 字段）：
- `last_heartbeat: u64` — 最后心跳时间戳
- `missed_count: u32` — 缺失心跳数
- `status: HealthStatus` — 当前健康状态
- `interval_ms: u64` — 该 Agent 的心跳间隔（毫秒）

derive `Clone, Debug`（D4 偏差）。

### Requirement: HeartbeatMonitor 心跳监控器

系统 SHALL 提供 `HeartbeatMonitor` 结构体：
```rust
pub struct HeartbeatMonitor {
    agents: BTreeMap<AgentId, HeartbeatState>,
    default_interval_ms: u64,
    max_missed: u32,
}
```

derive `Debug`（D4 偏差）。使用 `BTreeMap`（D1 偏差）。

#### Scenario: 构造 HeartbeatMonitor
- **WHEN** 调用 `HeartbeatMonitor::new(1000, 3)`
- **THEN** 返回实例，`default_interval_ms = 1000`，`max_missed = 3`，`agents` 为空

### Requirement: HeartbeatMonitor API

`HeartbeatMonitor` SHALL 提供以下方法：

| 方法 | 签名 | 说明 |
|------|------|------|
| `new` | `(interval_ms: u64, max_missed: u32) -> Self` | 构造监控器 |
| `register` | `(&mut self, id: AgentId, now: u64)` | 注册 Agent（D2：追加 now 参数） |
| `heartbeat` | `(&mut self, id: AgentId, timestamp: u64)` | 记录心跳 |
| `check` | `(&mut self, now: u64) -> Vec<(AgentId, HealthStatus)>` | 检查所有 Agent 健康 |
| `is_healthy` | `(&self, id: AgentId) -> bool` | 查询指定 Agent 是否健康 |
| `set_interval` | `(&mut self, id: AgentId, interval_ms: u64)` | 设置 per-Agent 间隔 |
| `unregister` | `(&mut self, id: AgentId)` | 注销 Agent |

#### Scenario: register 新 Agent
- **WHEN** 调用 `register(id, now=1000)`
- **THEN** `agents` 含该 id，`last_heartbeat=1000`，`missed_count=0`，`status=Healthy`，`interval_ms=default_interval_ms`

#### Scenario: heartbeat 更新状态
- **WHEN** Agent 处于 Degraded，调用 `heartbeat(id, timestamp=2000)`
- **THEN** `last_heartbeat=2000`，`missed_count=0`，`status=Healthy`

#### Scenario: check 检测超时
- **WHEN** Agent `last_heartbeat=1000`，`interval_ms=1000`，调用 `check(now=2500)`
- **THEN** `missed_count=2`，`status=Degraded`，返回 `Vec` 含 `(id, Degraded)`

#### Scenario: check 检测故障
- **WHEN** `missed_count >= max_missed`（如 3）
- **THEN** `status=Unhealthy`（D7：不设置 Dead）

#### Scenario: is_healthy 查询
- **WHEN** Agent 状态为 Healthy
- **THEN** `is_healthy(id) == true`
- **WHEN** Agent 状态为 Degraded/Unhealthy/Dead 或未注册
- **THEN** `is_healthy(id) == false`

#### Scenario: set_interval 覆盖间隔
- **WHEN** 调用 `set_interval(id, 500)`
- **THEN** 该 Agent 的 `interval_ms=500`，后续 `check` 用 500ms 判定

#### Scenario: unregister 注销
- **WHEN** 调用 `unregister(id)`
- **THEN** `agents` 不含该 id，后续 `check` 不返回该 Agent

### Requirement: HealthCheck 健康检查 trait

系统 SHALL 提供 `HealthCheck` trait（object-safe）：
```rust
pub trait HealthCheck {
    fn check_health(&self) -> HealthStatus;
}
```

Agent 可实现此 trait 提供自定义健康检查（蓝图 §9.7）。v0.37.0 仅定义 trait，不主动调用。

#### Scenario: object-safe
- **WHEN** 将实现 `HealthCheck` 的结构装箱为 `Box<dyn HealthCheck>`
- **THEN** 编译通过

### Requirement: AgentError 扩展

系统 SHALL 在 `AgentError` 追加 2 个变体：
- `HeartbeatTimeout { agent_id: AgentId, missed: u32 }` — 心跳超时
- `AgentUnhealthy { agent_id: AgentId }` — Agent 不健康

#### Scenario: Display 输出
- **WHEN** `HeartbeatTimeout { agent_id, missed: 3 }`
- **THEN** Display 输出含 "heartbeat timeout" 和 missed 数
- **WHEN** `AgentUnhealthy { agent_id }`
- **THEN** Display 输出含 "agent unhealthy"

### Requirement: check 算法

`check(now)` SHALL 对每个已注册 Agent 执行：
1. `elapsed = now.saturating_sub(state.last_heartbeat)` — 防溢出（§8.3 时钟回拨）
2. 若 `elapsed > state.interval_ms`：
   - `missed_count = (elapsed / state.interval_ms) as u32`
   - 若 `missed_count >= max_missed` → `status = Unhealthy`
   - 否则若 `missed_count > 0` → `status = Degraded`
3. 返回 `(id, status)` 列表

**注**：若 `elapsed <= interval_ms`，不更新 `missed_count`/`status`（保留上次状态）。`heartbeat()` 会重置为 Healthy。

### Requirement: 默认常量

```rust
const DEFAULT_INTERVAL_MS: u64 = 1000;  // 1 秒
const DEFAULT_MAX_MISSED: u32 = 3;      // 3 次超时 = 故障
```

### Requirement: no_std 合规

`health.rs` 和 `heartbeat.rs` 必须：
- 不使用 `std::*`（仅 `alloc::*` / `core::*`）
- 不在子模块重复 `#![cfg_attr(not(test), no_std)]`
- 不使用 `panic!` / `todo!` / `unimplemented!`（非测试代码）
- 通过 `aarch64-unknown-none` 交叉编译

### Requirement: 零外部依赖

`crates/agents/agent/Cargo.toml` 的 `[dependencies]` 必须保持为空。

### Requirement: 测试覆盖

- register/heartbeat/check 基本流程
- Healthy → Degraded → Unhealthy 状态演进
- is_healthy 查询（注册/未注册）
- set_interval per-Agent 覆盖
- unregister 注销
- 多 Agent 独立监控
- 时钟回拨（saturating_sub 防溢出）
- HealthStatus derives（Clone/Copy/Debug/PartialEq/Eq）
- HealthCheck trait object-safe
- HeartbeatTimeout / AgentUnhealthy 错误变体 Display + Clone + Eq
- 集成测试：完整心跳监控生命周期
