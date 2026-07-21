# Tasks

- [x] Task 1: Workspace 同步
  - [x] 根 `Cargo.toml` 版本号 `0.71.0` → `0.72.0`
  - [x] members 添加 `crates/agents/energy-market-agent`（置于 `crates/agents/alarm` 之后）
  - [x] 验证：`cargo metadata --format-version 1` 成功

- [x] Task 2: 创建 `eneros-energy-market-agent` crate 骨架
  - [x] 新建 `crates/agents/energy-market-agent/Cargo.toml`，package name = `eneros-energy-market-agent`
  - [x] dependencies：`eneros-agent` / `eneros-dual-brain` / `eneros-fast-path` / `eneros-energy-lp-model` / `eneros-solver-core` / `eneros-llm-engine` + `serde` / `serde_json`
  - [x] 无 `[features]` 段（纯 Rust，无 FFI）
  - [x] no_std 配置：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
  - [x] 新建 `src/lib.rs`，模块声明：error / runtime / market / energy_agent / market_agent
  - [x] lib.rs 包含 D1~D14 偏差声明表

- [x] Task 3: 实现 `error.rs` — AgentRuntimeError
  - [x] `AgentRuntimeError` 枚举：`DualBrainError(DualBrainError)` / `ChannelError(String)` / `MarketDataError(String)` / `AgentError(AgentError)` / `NotRunning`
  - [x] 派生 `Debug`（D12：不派生 Clone）
  - [x] 实现 `From<DualBrainError>` 和 `From<AgentError>` 转换

- [x] Task 4: 实现 `runtime.rs` — AgentRuntime trait + HeartbeatStatus
  - [x] `HeartbeatStatus` 枚举：`Alive` / `Dead`，派生 `Debug + Clone + Copy + PartialEq + Eq`（D8）
  - [x] `AgentRuntime` trait：`descriptor()` / `on_start(now_ms)` / `on_tick(now_ms)` / `on_stop(now_ms)` / `on_heartbeat(now_ms)`（D2/D6）
  - [x] trait 不加 `Send + Sync` bound

- [x] Task 5: 实现 `market.rs` — MarketData + MarketSignal + MarketChannel + MarketDataSource + MockMarketSource
  - [x] `MarketSignal` 枚举：`RealtimePrice` / `DayAheadForecast` / `DemandResponse` / `EmergencyDispatch`，派生 `Debug + Clone` + serde
  - [x] `MarketData` 结构体：`timestamp` / `price_forecast: Vec<f64>` / `current_price` / `load_forecast: Option<Vec<f64>>` / `signal_type`，派生 `Debug + Clone` + serde（D13）
  - [x] `MarketChannel` 结构体：`buffer: Vec<MarketData>` / `capacity: usize`（D4）
    - [x] `new(capacity) -> Self`
    - [x] `send(&mut self, data) -> Result<(), AgentRuntimeError>` — 满时丢弃最旧（蓝图 §8.3）
    - [x] `try_recv(&mut self) -> Option<MarketData>`
    - [x] `is_empty(&self) -> bool` / `len(&self) -> usize`
  - [x] `MarketDataSource` trait：`recv_nonblocking(&mut self) -> Result<Option<MarketData>, AgentRuntimeError>`（D5）
  - [x] `MockMarketSource` 结构体：`data: VecDeque<MarketData>`
    - [x] `new() -> Self` / `with_data(data) -> Self` / `push(&mut self, data)` / `recv_nonblocking()`

- [x] Task 6: 实现 `energy_agent.rs` — EnergyAgent
  - [x] `EnergyAgent` 结构体：`descriptor` / `coordinator: DualBrainCoordinator<MockSolver>` / `market_channel` / `current_schedule: Option<ScheduleResult>` / `state` / `tick_count`（D9/D11）
  - [x] `new(name, config, now_ms) -> Self` — `AgentDescriptor::new(AgentType::Energy, name, now_ms)`（D7）+ `DualBrainCoordinator::new(config, llm_engine, solver, sink)`（D9）
  - [x] `new_default(now_ms) -> Self` — 使用 `DualBrainCoordinator::default_with_mock()`
  - [x] `market_channel_mut(&mut self) -> &mut MarketChannel`
  - [x] 实现 `AgentRuntime` trait：
    - [x] `descriptor()` 返回 `&AgentDescriptor`
    - [x] `on_start(now_ms)` — `state = Running`，`Ok(())`
    - [x] `on_tick(now_ms)` — 接收市场数据 → 构建 `RealtimeState`（D11）→ `coordinator.execute(&state, now_ms)`（D10）→ 成功更新 `current_schedule` / 失败 `state = Error`（D14）
    - [x] `on_stop(now_ms)` — `state = Dead`，`Ok(())`
    - [x] `on_heartbeat(now_ms)` — `Running` → `Alive` / 否则 `Dead`（D8）

- [x] Task 7: 实现 `market_agent.rs` — MarketAgent
  - [x] `MarketAgent` 结构体：`descriptor` / `source: Box<dyn MarketDataSource>` / `market_channel` / `price_cache: Vec<f64>` / `state` / `tick_count`
  - [x] `new(name, source, now_ms) -> Self` — `AgentDescriptor::new(AgentType::Market, name, now_ms)`（D7）+ `price_cache = vec![0.5; 96]`
  - [x] `new_default(now_ms) -> Self` — 使用 `MockMarketSource::new()`
  - [x] `market_channel_mut(&mut self) -> &mut MarketChannel`
  - [x] 实现 `AgentRuntime` trait：
    - [x] `on_start(now_ms)` — `state = Running`
    - [x] `on_tick(now_ms)` — `source.recv_nonblocking()` → 有数据：更新 `price_cache` + `market_channel.send(data)` / 无数据：正常返回
    - [x] `on_stop(now_ms)` — `state = Dead`
    - [x] `on_heartbeat(now_ms)` — `Running` → `Alive` / 否则 `Dead`

- [x] Task 8: 集成测试（lib.rs）— 至少 20 个测试
  - [x] T1 MarketData 构造（96 时段）
  - [x] T2 MarketSignal 变体构造
  - [x] T3 MarketChannel::new 空
  - [x] T4 MarketChannel::send + try_recv 成功
  - [x] T5 MarketChannel 缓冲满丢弃旧数据
  - [x] T6 MarketChannel 空时 try_recv 返回 None
  - [x] T7 MockMarketSource::new 空
  - [x] T8 MockMarketSource::with_data 预加载
  - [x] T9 MockMarketSource::recv_nonblocking 有数据
  - [x] T10 MockMarketSource::recv_nonblocking 空返回 None
  - [x] T11 HeartbeatStatus::Alive / Dead
  - [x] T12 AgentRuntimeError 变体构造
  - [x] T13 EnergyAgent::new_default 构造
  - [x] T14 EnergyAgent::on_start 状态转 Running
  - [x] T15 EnergyAgent::on_tick 执行双脑（慢路径）
  - [x] T16 EnergyAgent::on_tick 接收市场数据
  - [x] T17 EnergyAgent::on_stop 状态转 Dead
  - [x] T18 EnergyAgent::on_heartbeat Running → Alive
  - [x] T19 EnergyAgent::on_heartbeat 非 Running → Dead
  - [x] T20 MarketAgent::new_default 构造
  - [x] T21 MarketAgent::on_tick 接收并转发数据
  - [x] T22 MarketAgent::on_tick 无数据正常返回
  - [x] T23 双 Agent 协作：MarketAgent send → EnergyAgent 接收
  - [x] T24 EnergyAgent 双脑失败降级（state = Error）

- [x] Task 9: 创建设计文档 `docs/agents/energy-market-agent-design.md`
  - [x] 12 章节完整（版本目标 / 前置依赖 / 交付物 / 详细设计 / 技术交底 / 测试计划 / 验收标准 / 风险 / 多角度要求 / ADR / 偏差声明 / 参考）
  - [x] 2 Mermaid 图（双 Agent 协作流程图 + Energy Agent tick 时序图）
  - [x] D1~D14 偏差声明表
  - [x] 文档位于 `docs/agents/` 下（C12）

- [x] Task 10: 版本同步
  - [x] `Makefile` 版本号 `0.72.0`（header + VERSION 变量 2 处）
  - [x] `.github/workflows/ci.yml` 版本号 `0.72.0`
  - [x] `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-energy-market-agent`

- [x] Task 11: 6 项构建校验（§2.4.2 C6~C11）
  - [x] `cargo metadata --format-version 1` 成功
  - [x] `cargo test -p eneros-energy-market-agent` 全部通过
  - [x] `cargo build -p eneros-energy-market-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] `cargo fmt -p eneros-energy-market-agent -- --check` 通过
  - [x] `cargo clippy -p eneros-energy-market-agent --all-targets -- -D warnings` 无 warning
  - [x] `cargo deny check licenses bans sources` 通过
  - [x] 更新 tasks.md / checklist.md 全部 [x]

# Task Dependencies
- Task 2 依赖 Task 1
- Task 3~7 依赖 Task 2（并行实现）
- Task 8 依赖 Task 3~7
- Task 9 可与 Task 3~8 并行
- Task 10 依赖 Task 2
- Task 11 依赖 Task 3~10 全部完成
