# v0.82.0 — Grid Agent 电网状态感知 Spec

## Why

EnerOS Phase 2 P2-C 子阶段（Agent 矩阵扩展）起点。在 v0.75.0~v0.78.0 Agent Bus DDS、
v0.79.0 gPTP 时间同步、v0.80.0 TAS 调度、v0.81.0 TSN 时延探针之上，需要 Grid Agent
实时感知电网频率/电压/电流/功率，为后续 v0.83.0 并网点管理、v0.84.0 并离网切换、
v0.92.0 Edge Coordinator、VPP < 30s 响应提供电网状态输入。

本版本交付**纯 Rust 类型与算法骨架**（无真实 IEC 104/Modbus 量测装置、无真实 DDS 发布），
通过 trait 抽象 + Mock 实现验证 `GridState` 数据结构、`GridSampler` 采样抽象、
`GridPublisher` 发布抽象、`GridAgent` 生命周期与异常检测回调的正确性。真实 IEC 104/Modbus
协议接入延后到 v0.83.0+ 与硬件量测装置集成阶段。

## What Changes

- **新建 crate**：`crates/agents/grid_agent/`（子系统 = agents，项目规则 §2.3.1）
- **新增源文件**：
  - `src/lib.rs` — crate 入口、模块导出、`GridError` 错误枚举、T1~T45 单元测试
  - `src/state.rs` — `GridState`（12 字段）、`DataQuality`（3 变体）、`GridAgent` 结构体 + `AgentRuntime` impl
  - `src/sampler.rs` — `GridSampler` trait + `MockGridSampler` + `is_valid_grid` + `default_anomaly_detectors`
  - `src/publisher.rs` — `GridPublisher` trait + `MockGridPublisher` + `publish_state` 辅助函数
- **新增类型**：
  - `GridState`（12 字段：frequency/voltage_a/b/c/current_a/b/c/active_power/reactive_power/power_factor/timestamp/quality）
  - `DataQuality`（3 变体：`Good` / `Invalid` / `Uncertain`，本地定义，blueprint 风格的简化枚举，不引入 `eneros-upa-model::PointQuality` 的 7 标志位复杂度）
  - `GridSampler` trait（`sample(&mut self, now_ms: u64) -> Result<GridState, GridError>`）
  - `MockGridSampler`（测试用，可配置返回值与失败模式）
  - `GridPublisher` trait（`publish_state(&GridState) -> Result<(), GridError>` + `publish_alert(&GridState) -> Result<(), GridError>`）
  - `MockGridPublisher`（测试用，记录已发布 state 与 alert 数量）
  - `GridAgent`（持有 `descriptor` + `Box<dyn GridSampler>` + `Box<dyn GridPublisher>` + `state` + `anomaly_handlers` + `sample_interval_ms` + `state` + `tick_count`）
  - `GridError`（3 变体：`SampleFailed` / `PublishFailed` / `InvalidConfig`，本地定义）
- **新增配置**：`configs/grid_points.toml`（电网量测点表配置模板：frequency/voltage/current/power 各项 point_id）
- **新增文档**：`docs/agents/grid-agent-design.md`（12 章节 + 2 Mermaid 图 + D1~D14 偏差声明表）
- **版本同步**：根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs` 由 `0.81.0` → `0.82.0`
- **workspace members**：根 `Cargo.toml` 新增 `"crates/agents/grid_agent"`

### Karpathy 简化决策（沿用 v0.79.0~v0.81.0 模式）

- **无真实 IEC 104/Modbus 量测**：通过 `GridSampler` trait + `MockGridSampler` 抽象，沿用 v0.51.0 `PointAccess` trait 设计风格但更简化（不依赖 `eneros-protocol-abstract` 与 `eneros-upa-model`，避免协议层间接依赖）
- **无真实 DDS 发布**：通过 `GridPublisher` trait + `MockGridPublisher` 抽象，沿用 v0.75.0 `DdsWriter` 设计风格但更简化（不依赖 `eneros-agent-bus-dds`，避免 Agent Bus 间接依赖）
- **无 async runtime**：`async fn run()` 改为 sync `on_tick(now_ms: u64)` 实现 `AgentRuntime` trait（沿用 v0.73.0 `device-agent` 模式，no_std 无 async runtime）
- **无 `Instant::now()`**：通过 `now_ms: u64` 参数注入时间戳（沿用 v0.51.0~v0.81.0 先例）
- **无 `Send + Sync` bound**：no_std 单线程，沿用 D2 (v0.79.0) / D18 (v0.80.0) / D10 (v0.81.0) 先例
- **无 `Box<dyn Fn + Send + Sync>`**：异常检测回调使用 `Vec<fn(&GridState) -> bool>` 函数指针（`Copy` + 无堆分配），不沿用蓝图 `Box<dyn Fn + Send + Sync>` 复杂签名

## Impact

- **Affected specs**：
  - `develop-v0830-grid-agent-pcc`（未存在，v0.83.0 并网点管理）将依赖本版本的 `GridState` / `GridAgent` / `GridSampler` / `GridPublisher`
  - `develop-v0730-device-agent`（已完成，device-agent）的 `AgentRuntime` 模式被本版本沿用，无破坏性变更
- **Affected code**：
  - 新增 `crates/agents/grid_agent/`（独立 crate，不修改既有 crate 源码）
  - 修改根 `Cargo.toml`（workspace members 新增 + 版本号）
  - 修改 `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`（版本号同步 + 类型列表追加）
- **Affected docs**：新增 `docs/agents/grid-agent-design.md`
- **Affected configs**：新增 `configs/grid_points.toml`
- **回归影响**：v0.51.0 `eneros-protocol-abstract` / v0.73.0 `eneros-device-agent` / v0.75.0~v0.81.0 `eneros-agent-bus-dds` / `eneros-tsn-time` 不被修改（Surgical Changes 原则）

## ADDED Requirements

### Requirement: GridState 数据结构

系统 SHALL 提供 `GridState` 结构体，包含 12 个字段：
`frequency: f32` / `voltage_a: f32` / `voltage_b: f32` / `voltage_c: f32` /
`current_a: f32` / `current_b: f32` / `current_c: f32` / `active_power: f32` /
`reactive_power: f32` / `power_factor: f32` / `timestamp: u64` / `quality: DataQuality`。

结构体 SHALL 派生 `Debug, Clone, Copy, PartialEq, Default`。
`Default` 实现 SHALL 所有 `f32` 字段为 `0.0`，`timestamp` 为 `0`，`quality` 为 `DataQuality::Invalid`。

#### Scenario: GridState::default() 全零 + Invalid
- **WHEN** 调用 `GridState::default()`
- **THEN** `frequency == 0.0` && `voltage_a == 0.0` && `active_power == 0.0` && `timestamp == 0` && `quality == DataQuality::Invalid`

#### Scenario: GridState 字段可读访问
- **WHEN** 构造 `GridState { frequency: 50.0, voltage_a: 220.0, ... }`
- **THEN** `state.frequency == 50.0` && `state.voltage_a == 220.0`

### Requirement: DataQuality 枚举

系统 SHALL 提供 `DataQuality` 枚举，含 3 变体：`Good` / `Invalid` / `Uncertain`。
枚举 SHALL 派生 `Debug, Clone, Copy, PartialEq, Eq, Default`。
`Default` 实现 SHALL 返回 `DataQuality::Invalid`（保守默认）。

#### Scenario: DataQuality::default() 返回 Invalid
- **WHEN** 调用 `DataQuality::default()`
- **THEN** 返回 `DataQuality::Invalid`

### Requirement: GridSampler trait

系统 SHALL 提供 `GridSampler` trait，定义电网状态采样抽象：
```rust
pub trait GridSampler {
    fn sample(&mut self, now_ms: u64) -> Result<GridState, GridError>;
}
```

不要求 `Send + Sync`（D10：no_std 单线程）。

#### Scenario: MockGridSampler 成功采样
- **WHEN** `MockGridSampler::new(state)` 配置返回 `GridState { frequency: 50.0, ... }`，调用 `sample(now_ms)`
- **THEN** 返回 `Ok(state)` 且 `state.timestamp == now_ms`

#### Scenario: MockGridSampler 采样失败
- **WHEN** `MockGridSampler::new_failing()` 调用 `sample(now_ms)`
- **THEN** 返回 `Err(GridError::SampleFailed)`

### Requirement: MockGridSampler 实现

系统 SHALL 提供 `MockGridSampler` 结构体，字段：`next_state: GridState` / `fail: bool`。
- `MockGridSampler::new(state: GridState) -> Self`（`fail = false`）
- `MockGridSampler::new_failing() -> Self`（`fail = true`，`next_state = GridState::default()`）
- 实现 `GridSampler::sample`：`fail == true` 返回 `Err(SampleFailed)`；否则返回 `Ok(next_state with timestamp = now_ms)`

### Requirement: GridPublisher trait

系统 SHALL 提供 `GridPublisher` trait，定义状态发布抽象：
```rust
pub trait GridPublisher {
    fn publish_state(&mut self, state: &GridState) -> Result<(), GridError>;
    fn publish_alert(&mut self, state: &GridState) -> Result<(), GridError>;
}
```

不要求 `Send + Sync`（D10）。

### Requirement: MockGridPublisher 实现

系统 SHALL 提供 `MockGridPublisher` 结构体，字段：
`published_states: Vec<GridState>` / `published_alerts: Vec<GridState>` / `fail_state: bool` / `fail_alert: bool`。
- `MockGridPublisher::new() -> Self`（默认无失败）
- `MockGridPublisher::new_failing_state() -> Self`（`fail_state = true`）
- `MockGridPublisher::new_failing_alert() -> Self`（`fail_alert = true`）
- 实现 `GridPublisher::publish_state`：`fail_state == true` 返回 `Err(PublishFailed)`；否则 `published_states.push(*state)` 返回 `Ok(())`
- 实现 `GridPublisher::publish_alert`：`fail_alert == true` 返回 `Err(PublishFailed)`；否则 `published_alerts.push(*state)` 返回 `Ok(())`

### Requirement: GridAgent 结构体

系统 SHALL 提供 `GridAgent` 结构体，字段：
- `descriptor: AgentDescriptor`（来自 `eneros-agent`）
- `sampler: Box<dyn GridSampler>`（采样器，闭包注入式 trait 对象）
- `publisher: Box<dyn GridPublisher>`（发布器）
- `state: GridState`（最近一次采样状态）
- `anomaly_handlers: Vec<fn(&GridState) -> bool>`（异常检测回调，函数指针，D14）
- `sample_interval_ms: u64`（采样周期，仅作为元数据，不直接驱动定时；定时由外部 `on_tick` 调用方控制）
- `agent_state: AgentState`（生命周期状态，来自 `eneros-agent`）
- `tick_count: u64`

### Requirement: GridAgent 构造与生命周期

`GridAgent::new` SHALL 接受 `name: &str` / `sampler: Box<dyn GridSampler>` / `publisher: Box<dyn GridPublisher>` / `sample_interval_ms: u64` / `now_ms: u64`，初始化：
- `descriptor = AgentDescriptor::new(AgentType::Grid, name, now_ms)`
- `state = GridState::default()`
- `anomaly_handlers = Vec::new()`
- `agent_state = AgentState::Created`

`GridAgent::register_anomaly_detector` SHALL 接受 `fn(&GridState) -> bool` 并追加到 `anomaly_handlers`。

`GridAgent::current_state` SHALL 返回 `&GridState`。

`GridAgent` SHALL 实现 `AgentRuntime` trait（来自 `eneros-energy-market-agent`）：
- `descriptor()` → `&AgentDescriptor`
- `on_start(now_ms)` → `agent_state = Running`，返回 `Ok(())`
- `on_tick(now_ms)` → 调用 `sampler.sample(now_ms)`，成功则更新 `state`，运行 anomaly detectors，若任一返回 true 则调用 `publisher.publish_alert(&state)`，最后调用 `publisher.publish_state(&state)`，`tick_count += 1`，返回 `Ok(())`；采样失败返回 `Err(AgentRuntimeError::DeviceError(...))`，发布失败返回 `Err(AgentRuntimeError::DeviceError(...))`
- `on_stop(now_ms)` → `agent_state = Dead`，返回 `Ok(())`
- `on_heartbeat(now_ms)` → `Running` 返回 `Alive`，否则 `Dead`

#### Scenario: GridAgent 构造与初始状态
- **WHEN** 调用 `GridAgent::new("grid", sampler, publisher, 100, 1000)`
- **THEN** `descriptor.agent_type == AgentType::Grid` && `state == GridState::default()` && `agent_state == AgentState::Created` && `tick_count == 0`

#### Scenario: on_tick 成功采样并发布
- **WHEN** Mock sampler 配置返回有效 state，Mock publisher 默认成功，调用 `on_tick(now_ms)`
- **THEN** `tick_count == 1` && `state.frequency == sampler.next_state.frequency` && `publisher.published_states.len() == 1` && `agent_state == Running`（需先 `on_start`）

#### Scenario: on_tick 采样失败返回错误
- **WHEN** Mock sampler 配置 `fail = true`，调用 `on_tick(now_ms)`
- **THEN** 返回 `Err(AgentRuntimeError::DeviceError(...))` 且 `tick_count` 不变

#### Scenario: on_tick 异常检测触发 alert
- **WHEN** Mock sampler 返回 frequency=49.0（越下限），注册了 `frequency_out_of_range` detector，调用 `on_tick(now_ms)`
- **THEN** `publisher.published_alerts.len() == 1` && `publisher.published_states.len() == 1`

### Requirement: is_valid_grid 辅助函数

系统 SHALL 提供 `is_valid_grid(freq: f32, voltage: f32) -> bool` 公开函数：
- `freq` 在 `[49.5, 50.5]` 且 `voltage` 在 `[200.0, 240.0]` 时返回 `true`
- 否则返回 `false`

用于 `MockGridSampler` 与默认异常检测器的复用。

### Requirement: 默认异常检测器

系统 SHALL 提供 `default_anomaly_detectors() -> Vec<fn(&GridState) -> bool>`，返回 3 个检测器：
1. `frequency_out_of_range`：`state.frequency < 49.5 || state.frequency > 50.5`
2. `voltage_out_of_range`：`state.voltage_a < 200.0 || state.voltage_a > 240.0`
3. `quality_invalid`：`state.quality == DataQuality::Invalid`

### Requirement: GridError 错误枚举

系统 SHALL 提供 `GridError` 枚举（位于 `lib.rs`），含 3 变体：
`SampleFailed` / `PublishFailed` / `InvalidConfig`。
派生 `Debug, Clone, Copy, PartialEq, Eq`。
实现 `From<GridError> for AgentRuntimeError`：所有变体映射到 `AgentRuntimeError::DeviceError(alloc::string::String::from(...))`。

## MODIFIED Requirements

### Requirement: workspace members 列表

根 `Cargo.toml` 的 `[workspace.members]` SHALL 在既有列表中追加 `"crates/agents/grid_agent"`（保持字母序或文件出现顺序，沿用既有风格）。

### Requirement: 版本号同步

根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs` SHALL 同步从 `v0.81.0` 升级到 `v0.82.0`。`ci/src/gate.rs` clippy 段与 test 段注释 SHALL 追加 `eneros-grid-agent v0.82.0` 类型列表：`GridState / DataQuality / GridSampler / MockGridSampler / GridPublisher / MockGridPublisher / GridAgent / GridError / is_valid_grid / default_anomaly_detectors`。

## REMOVED Requirements

无。本版本为纯增量，不删除既有功能。
