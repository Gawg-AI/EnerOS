# v0.72.0 Energy Agent + Market Agent Spec

## Why

Phase 1 P1-L MVP 集成第一层：实现 Energy Agent（能源调度核心）和 Market Agent（市场数据接收），作为 MVP 场景的两个核心 Agent。Energy Agent 编排 v0.71.0 双脑协调器执行储能调度，Market Agent 接收外部电价/负荷预测并通过 Agent 间通道传递给 Energy Agent。双 Agent 协作构成 MVP 端到端集成的业务核心。

## What Changes

- 新建 crate `eneros-energy-market-agent`（`crates/agents/energy-market-agent/`），含 Energy Agent 与 Market Agent 双 Agent 实现
- 新增 `AgentRuntime` trait — Agent 运行时接口（on_start/on_tick/on_stop/on_heartbeat），本地定义（D6：v0.33.0 `AgentEntry` 语义不同）
- 新增 `HeartbeatStatus` 枚举 — 心跳状态（Alive/Dead），本地定义（D8：`HealthStatus` 语义不同）
- 新增 `AgentRuntimeError` 错误枚举 — Agent 运行时错误
- 新增 `MarketData` + `MarketSignal` — 市场数据结构（电价曲线/负荷预测/信号类型）
- 新增 `MarketChannel` — Agent 间通信通道（Vec-backed 非阻塞发送/接收，D4）
- 新增 `MarketDataSource` trait + `MockMarketSource` — 市场数据源抽象（D5：`TcpConnection` 不存在）
- 新增 `EnergyAgent` — 能源调度 Agent，持有 `DualBrainCoordinator<MockSolver>`，实现 `AgentRuntime`
- 新增 `MarketAgent` — 市场数据 Agent，从 `MarketDataSource` 接收数据并通过 `MarketChannel` 发送给 Energy Agent
- 根 `Cargo.toml` 版本号 `0.71.0` → `0.72.0`，members 添加 `crates/agents/energy-market-agent`

## Impact

- Affected specs: P1-L MVP 集成第一层（v0.72.0~v0.74.0）
- Affected code:
  - 新建 `crates/agents/energy-market-agent/`（6 源文件 + Cargo.toml）
  - 根 `Cargo.toml`（版本 + members）
  - `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`（版本同步）
  - 新建设计文档 `docs/agents/energy-market-agent-design.md`

## ADDED Requirements

### Requirement: AgentRuntime trait

系统 SHALL 提供 `AgentRuntime` trait，定义 Agent 运行时生命周期接口：

```rust
pub trait AgentRuntime {
    fn descriptor(&self) -> &AgentDescriptor;
    fn on_start(&mut self, now_ms: u64) -> Result<(), AgentRuntimeError>;
    fn on_tick(&mut self, now_ms: u64) -> Result<(), AgentRuntimeError>;
    fn on_stop(&mut self, now_ms: u64) -> Result<(), AgentRuntimeError>;
    fn on_heartbeat(&self, now_ms: u64) -> HeartbeatStatus;
}
```

- `now_ms: u64` 参数（D2：no_std 无系统时钟，与 v0.71.0 D1 一致）
- 不派生 `Send + Sync`（与 v0.59/v0.63/v0.71 一致）

#### Scenario: Agent 启动
- **WHEN** 调用 `on_start(now_ms)`
- **THEN** Agent 状态转为 `Running`，返回 `Ok(())`

#### Scenario: Agent tick
- **WHEN** 调用 `on_tick(now_ms)`
- **THEN** Agent 执行周期性业务逻辑，返回 `Ok(())` 或 `Err`

### Requirement: HeartbeatStatus 枚举

```rust
pub enum HeartbeatStatus {
    Alive,
    Dead,
}
```

派生 `Debug + Clone + Copy + PartialEq + Eq`（D8：本地定义，`HealthStatus` 4 级语义不同）。

### Requirement: AgentRuntimeError 错误枚举

```rust
pub enum AgentRuntimeError {
    DualBrainError(DualBrainError),
    ChannelError(String),
    MarketDataError(String),
    AgentError(AgentError),
    NotRunning,
}
```

派生 `Debug`（D12：不派生 Clone，Karpathy 简化原则，与 v0.71.0 一致）。

### Requirement: MarketData 市场数据结构

```rust
pub struct MarketData {
    pub timestamp: u64,
    pub price_forecast: Vec<f64>,      // 96 时段（15min/段，未来 24h）
    pub current_price: f64,
    pub load_forecast: Option<Vec<f64>>,
    pub signal_type: MarketSignal,
}

pub enum MarketSignal {
    RealtimePrice,
    DayAheadForecast,
    DemandResponse,
    EmergencyDispatch,
}
```

派生 `Debug + Clone`（使用 `serde::Serialize + Deserialize` 用于 JSON 解析，D13）。

#### Scenario: 市场数据构造
- **WHEN** 构造 `MarketData` 含 96 时段电价
- **THEN** `price_forecast.len() == 96`，`current_price` 为当前电价

### Requirement: MarketChannel Agent 间通信

系统 SHALL 提供 `MarketChannel`，实现 Market Agent → Energy Agent 的非阻塞数据传递：

```rust
pub struct MarketChannel {
    buffer: Vec<MarketData>,
    capacity: usize,
}
```

- `new(capacity: usize) -> Self` — 创建通道
- `send(&mut self, data: MarketData) -> Result<(), AgentRuntimeError>` — 非阻塞发送；缓冲满时丢弃最旧数据（蓝图 §8.3）
- `try_recv(&mut self) -> Option<MarketData>` — 非阻塞接收；无数据返回 `None`
- `is_empty(&self) -> bool` — 缓冲是否为空
- `len(&self) -> usize` — 缓冲数据量

D4：`ChannelReceiver`/`ChannelSender` 不存在，本地定义。简单 Vec-backed 实现（Karpathy 简化原则）。

#### Scenario: 发送并接收
- **WHEN** `send(data)` 后 `try_recv()`
- **THEN** 返回 `Some(data)`，数据正确传递

#### Scenario: 缓冲满丢弃旧数据
- **WHEN** 缓冲已满（capacity=2，3 条数据）再 `send`
- **THEN** 最旧数据被丢弃，`len() == 2`，最新 2 条保留

#### Scenario: 空通道接收
- **WHEN** 空通道调用 `try_recv()`
- **THEN** 返回 `None`

### Requirement: MarketDataSource 市场数据源抽象

```rust
pub trait MarketDataSource {
    fn recv_nonblocking(&mut self) -> Result<Option<MarketData>, AgentRuntimeError>;
}
```

- `MockMarketSource` — 预加载数据队列的 Mock 实现
  - `new() -> Self` — 创建空 source
  - `with_data(data: Vec<MarketData>) -> Self` — 预加载数据
  - `push(&mut self, data: MarketData)` — 追加数据
  - `recv_nonblocking()` — 弹出队首数据；空时返回 `Ok(None)`

D5：`TcpConnection` 不存在，本地定义 trait + Mock。v0.29.0 socket 抽象复杂，MVP 阶段用 Mock 即可。

### Requirement: EnergyAgent 能源调度 Agent

```rust
pub struct EnergyAgent {
    descriptor: AgentDescriptor,
    coordinator: DualBrainCoordinator<MockSolver>,
    market_channel: MarketChannel,
    current_schedule: Option<ScheduleResult>,
    state: AgentState,
    tick_count: u64,
}
```

- `new(name: &str, config: ScheduleConfig, now_ms: u64) -> Self` — 构造 Energy Agent
  - `descriptor = AgentDescriptor::new(AgentType::Energy, name, now_ms)`（D7）
  - `coordinator = DualBrainCoordinator::new(config, Box::new(DualBrainMockEngine::new()), MockSolver::new(), Box::new(MockCommandSink::new()))`（D9）
- `new_default(now_ms: u64) -> Self` — 使用 `DualBrainCoordinator::default_with_mock()` 构造
- `market_channel_mut(&mut self) -> &mut MarketChannel` — 获取市场通道引用（供测试注入数据）

实现 `AgentRuntime` trait：
- `descriptor()` — 返回 `&AgentDescriptor`
- `on_start(now_ms)` — 状态转 `Running`，返回 `Ok(())`
- `on_tick(now_ms)` — 执行：
  1. 非阻塞接收市场数据（`market_channel.try_recv()`）
  2. 构建 `RealtimeState`（D11：从默认/缓存值构建，`current_price` 取最新市场数据或缓存）
  3. 调用 `coordinator.execute(&state, now_ms)`（D10）
  4. 成功：`current_schedule = Some(result.schedule)`，返回 `Ok(())`
  5. 失败：激活安全默认（D14：记录错误状态，不 panic），返回 `Err(DualBrainError)` 包装错误
- `on_stop(now_ms)` — 状态转 `Dead`，返回 `Ok(())`
- `on_heartbeat(now_ms)` — `Running` 返回 `Alive`，否则 `Dead`

D14：安全默认策略 — 双脑失败时 `state = AgentState::Error`，不执行命令下发（功率保持上次调度或零）。蓝图 `activate_safe_default()` 构造 `ControlCommand` 功率归零，但 v0.22.0 `ControlCommand` 字段差异大（D7）。本版本简化为状态标记，实际功率归零由 v0.73.0 Device Agent 下发。

#### Scenario: Energy Agent 启动
- **WHEN** `on_start(1000)`
- **THEN** `state == Running`，`descriptor.agent_type == Energy`

#### Scenario: Energy Agent tick 执行双脑
- **WHEN** `on_tick(2000)` 且 market_channel 有数据
- **THEN** 调用 `coordinator.execute()`，`current_schedule` 更新为 `Some`

#### Scenario: Energy Agent 双脑失败降级
- **WHEN** `coordinator.execute()` 返回 `Err`
- **THEN** `state == Error`，返回 `Err(AgentRuntimeError::DualBrainError)`

#### Scenario: Energy Agent 心跳
- **WHEN** `state == Running`，调用 `on_heartbeat`
- **THEN** 返回 `HeartbeatStatus::Alive`

### Requirement: MarketAgent 市场数据 Agent

```rust
pub struct MarketAgent {
    descriptor: AgentDescriptor,
    source: Box<dyn MarketDataSource>,
    market_channel: MarketChannel,
    price_cache: Vec<f64>,
    state: AgentState,
    tick_count: u64,
}
```

- `new(name: &str, source: Box<dyn MarketDataSource>, now_ms: u64) -> Self` — 构造 Market Agent
  - `descriptor = AgentDescriptor::new(AgentType::Market, name, now_ms)`（D7）
  - `price_cache = vec![0.5; 96]` 初始化
- `new_default(now_ms: u64) -> Self` — 使用 `MockMarketSource::new()` 构造
- `market_channel_mut(&mut self) -> &mut MarketChannel` — 获取市场通道（供测试读取 Energy Agent 接收的数据）

实现 `AgentRuntime` trait：
- `on_start(now_ms)` — 状态转 `Running`
- `on_tick(now_ms)` — 执行：
  1. `source.recv_nonblocking()` 接收市场数据
  2. 收到数据：更新 `price_cache`，`market_channel.send(data)` 发送给 Energy Agent
  3. 无数据：使用缓存电价，正常返回
- `on_stop(now_ms)` — 状态转 `Dead`
- `on_heartbeat(now_ms)` — `Running` 返回 `Alive`

#### Scenario: Market Agent 接收并转发
- **WHEN** `source` 有数据，`on_tick(2000)`
- **THEN** `price_cache` 更新，`market_channel` 含 1 条数据

#### Scenario: Market Agent 无数据
- **WHEN** `source` 无数据，`on_tick(2000)`
- **THEN** 返回 `Ok(())`，`price_cache` 不变

### Requirement: no_std 合规

- `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 仅使用 `alloc::*` / `core::*`
- 无 `Instant::now()` / `SystemTime::now()` / `uuid::Uuid::new_v4()`（D2/D3）
- 无 `log::warn!` / `log::info!` / `log::error!`（D1）
- 无 `std::net::TcpStream` / `std::sync::Mutex`（D5）
- 子模块不重复 `#![cfg_attr(not(test), no_std)]`

## MODIFIED Requirements

### Requirement: Workspace 版本同步

- 根 `Cargo.toml` 版本号 `0.71.0` → `0.72.0`
- members 列表添加 `"crates/agents/energy-market-agent"`（置于 `"crates/agents/alarm"` 之后）
- `Makefile` 版本号 `0.72.0`（header + VERSION 变量）
- `.github/workflows/ci.yml` 版本号 `0.72.0`
- `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-energy-market-agent`

## 偏差声明（D1~D14，Karpathy "Think Before Coding"）

| 偏差 | 蓝图原文 | 本版本处理 | 理由 |
|------|---------|-----------|------|
| **D1** | `log::info!` / `log::warn!` / `log::error!` | 移除日志；状态/错误通过返回值传递 | no_std 无 `log` crate；与 v0.57/v0.64/v0.70/v0.71 一致 |
| **D2** | `SystemTime::now()` / `UNIX_EPOCH` | `now_ms: u64` 参数 | no_std 合规：`SystemTime` 不可用；与 v0.57/v0.64/v0.70/v0.71 一致 |
| **D3** | `uuid::Uuid::new_v4().to_string()` | `AgentId::generate()` (v0.33.0) | no_std 无 uuid crate；复用 v0.33.0 原子计数器 ID 生成 |
| **D4** | `ChannelReceiver<MarketData>` / `ChannelSender<MarketData>` | 本地 `MarketChannel` (Vec-backed) | `ChannelReceiver`/`ChannelSender` 不存在；本地简单实现保持 crate 自包含可测试（与 v0.71.0 D6 一致） |
| **D5** | `TcpConnection::connect(market_server)` / `recv_nonblocking()` | 本地 `MarketDataSource` trait + `MockMarketSource` | `TcpConnection` 不存在；v0.29.0 socket 抽象复杂，MVP 用 Mock 即可 |
| **D6** | `impl AgentRuntime for EnergyAgent` | 本地定义 `AgentRuntime` trait | v0.33.0 `AgentEntry` trait 语义不同（on_init/on_start/on_stop + AgentContext，无 on_tick/on_heartbeat）；本地 trait 匹配蓝图运行时语义 |
| **D7** | `AgentDescriptor { id, agent_type, priority, capabilities: vec!["control.write"], trust_level, ..Default::default() }` | `AgentDescriptor::new(AgentType::Energy, name, now_ms)` | v0.33.0 `AgentDescriptor` 13 字段 + 构造器 `new(type, name, now)` 自动设置优先级/配额/信任等级；蓝图 `..Default::default()` 与 `capabilities: Vec<&str>` 类型不匹配（实际 `Vec<CapabilityRef>`） |
| **D8** | `HeartbeatStatus::Alive` / `HeartbeatStatus::Dead` | 本地定义 `HeartbeatStatus` 枚举（Alive/Dead） | v0.33.0 `HealthStatus` 4 级（Healthy/Degraded/Unhealthy/Dead）语义不同；蓝图 2 级（Alive/Dead）更简单 |
| **D9** | `DualBrainCoordinator::new(config)` | `DualBrainCoordinator::new(config, llm_engine, solver, sink)` | v0.71.0 实际构造器需 4 参数 |
| **D10** | `self.coordinator.execute(&state)` | `self.coordinator.execute(&state, now_ms)` | v0.71.0 `execute` 需 `now_ms: u64` 参数（D1 一致） |
| **D11** | 蓝图 `SystemState` 含 `soc`/`current_power`/`current_price`/`current_period`/`device_status`/`alarms`/`load_demand` | 构建 `RealtimeState`（v0.70.0）传入 `execute` | v0.67.0 `SystemState` 仅含电气字段；v0.70.0 `RealtimeState` 包装 `SystemState` + `current_price` + `load_demand`；Energy Agent 从缓存/默认值构建 |
| **D12** | 两个 crate：`energy-agent` + `market-agent` | 一个 crate：`eneros-energy-market-agent` | Karpathy 简化原则：两 Agent 共享 `MarketData`/`MarketChannel` 类型，单 crate 避免跨 crate 类型共享；与 v0.71.0 单 crate 多模块一致 |
| **D13** | `serde_json::from_slice(&data)` | `serde_json::from_slice`（alloc 支持） | no_std + alloc 下 `serde_json` 可用；Mock source 直接返回 `MarketData` 无需序列化 |
| **D14** | `activate_safe_default()` 构造 `ControlCommand` 功率归零 | `state = AgentState::Error` 状态标记 | v0.22.0 `ControlCommand` 字段差异大（`cmd_id: [u8;16]`/`DeviceId`/`setpoint: f32`）；功率归零下发由 v0.73.0 Device Agent 实现；本版本仅标记错误状态 |

## 依赖复用清单

| 复用版本 | 复用类型 | 用途 |
|---------|---------|------|
| v0.71.0 | `DualBrainCoordinator<MockSolver>` / `DualBrainResult` / `DualBrainError` / `DualBrainMockEngine` / `MockCommandSink` / `DispatchCommand` | 双脑协调与命令下发 |
| v0.70.0 | `RealtimeState` / `PathType` | 实时状态输入 |
| v0.66.0 | `ScheduleConfig` / `ScheduleResult` | 调度配置与结果 |
| v0.64.0 | `MockSolver` | LP 求解器（双脑默认） |
| v0.59.0 | `LlmEngine` / `InferParams` | LLM 引擎 trait |
| v0.33.0 | `AgentDescriptor` / `AgentType` / `AgentState` / `TrustLevel` / `AgentError` / `AgentId` | Agent 框架类型 |
