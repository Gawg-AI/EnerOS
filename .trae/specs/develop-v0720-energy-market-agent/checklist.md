# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.72.0`
- [x] C2 members 列表已添加 `crates/agents/energy-market-agent`（置于 `crates/agents/alarm` 之后）
- [x] C3 `cargo metadata --format-version 1` 成功

## Crate 骨架
- [x] C4 `crates/agents/energy-market-agent/Cargo.toml` 存在，package name = `eneros-energy-market-agent`
- [x] C5 dependencies 包含 6 个 eneros crate + serde + serde_json
- [x] C6 无 `[features]` 段（纯 Rust，无 FFI）
- [x] C7 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C8 `src/lib.rs` 包含 D1~D14 偏差声明表
- [x] C9 模块声明：error / runtime / market / energy_agent / market_agent

## error.rs — AgentRuntimeError
- [x] C10 `AgentRuntimeError` 枚举：`DualBrainError(DualBrainError)` / `ChannelError(String)` / `MarketDataError(String)` / `AgentError(AgentError)` / `NotRunning`
- [x] C11 派生 `Debug`（D12：不派生 Clone）
- [x] C12 实现 `From<DualBrainError>` 转换
- [x] C13 实现 `From<AgentError>` 转换

## runtime.rs — AgentRuntime trait + HeartbeatStatus
- [x] C14 `HeartbeatStatus` 枚举：`Alive` / `Dead`，派生 `Debug + Clone + Copy + PartialEq + Eq`（D8）
- [x] C15 `AgentRuntime` trait：`descriptor()` / `on_start(now_ms)` / `on_tick(now_ms)` / `on_stop(now_ms)` / `on_heartbeat(now_ms)`
- [x] C16 trait 不加 `Send + Sync` bound
- [x] C17 `now_ms: u64` 参数（D2）

## market.rs — MarketData + MarketSignal + MarketChannel + MarketDataSource + MockMarketSource
- [x] C18 `MarketSignal` 枚举：4 变体，派生 `Debug + Clone` + serde
- [x] C19 `MarketData` 结构体：5 字段，派生 `Debug + Clone` + serde（D13）
- [x] C20 `MarketChannel` 结构体：`buffer: Vec<MarketData>` / `capacity: usize`（D4）
- [x] C21 `MarketChannel::new(capacity)` 创建空通道
- [x] C22 `MarketChannel::send(data)` — 满时丢弃最旧数据（蓝图 §8.3）
- [x] C23 `MarketChannel::try_recv()` 返回 `Option<MarketData>`
- [x] C24 `MarketChannel::is_empty()` / `len()`
- [x] C25 `MarketDataSource` trait：`recv_nonblocking()`（D5）
- [x] C26 `MockMarketSource` 结构体 + `new()` / `with_data()` / `push()` / `recv_nonblocking()`

## energy_agent.rs — EnergyAgent
- [x] C27 `EnergyAgent` 结构体：6 字段（descriptor / coordinator / market_channel / current_schedule / state / tick_count）
- [x] C28 `new(name, config, now_ms)` — `AgentDescriptor::new(AgentType::Energy, name, now_ms)`（D7）+ `DualBrainCoordinator::new(config, llm, solver, sink)`（D9）
- [x] C29 `new_default(now_ms)` — 使用 `DualBrainCoordinator::default_with_mock()`
- [x] C30 `market_channel_mut()` 返回 `&mut MarketChannel`
- [x] C31 实现 `AgentRuntime::descriptor()`
- [x] C32 实现 `on_start(now_ms)` — `state = Running`
- [x] C33 实现 `on_tick(now_ms)` — 接收市场数据 + 构建 `RealtimeState`（D11）+ `coordinator.execute(&state, now_ms)`（D10）
- [x] C34 `on_tick` 成功：`current_schedule = Some(result.schedule)`
- [x] C35 `on_tick` 失败：`state = Error`（D14），返回 `Err(AgentRuntimeError::DualBrainError)`
- [x] C36 实现 `on_stop(now_ms)` — `state = Dead`
- [x] C37 实现 `on_heartbeat(now_ms)` — `Running` → `Alive` / 否则 `Dead`（D8）

## market_agent.rs — MarketAgent
- [x] C38 `MarketAgent` 结构体：6 字段（descriptor / source / market_channel / price_cache / state / tick_count）
- [x] C39 `new(name, source, now_ms)` — `AgentDescriptor::new(AgentType::Market, name, now_ms)`（D7）+ `price_cache = vec![0.5; 96]`
- [x] C40 `new_default(now_ms)` — 使用 `MockMarketSource::new()`
- [x] C41 `market_channel_mut()` 返回 `&mut MarketChannel`
- [x] C42 实现 `AgentRuntime::descriptor()`
- [x] C43 实现 `on_start(now_ms)` — `state = Running`
- [x] C44 实现 `on_tick(now_ms)` — `source.recv_nonblocking()` → 有数据：更新 `price_cache` + `market_channel.send(data)` / 无数据：正常返回
- [x] C45 实现 `on_stop(now_ms)` — `state = Dead`
- [x] C46 实现 `on_heartbeat(now_ms)` — `Running` → `Alive` / 否则 `Dead`

## 集成测试（lib.rs）
- [x] C47 T1 MarketData 构造（96 时段）
- [x] C48 T2 MarketSignal 变体构造
- [x] C49 T3 MarketChannel::new 空
- [x] C50 T4 MarketChannel::send + try_recv 成功
- [x] C51 T5 MarketChannel 缓冲满丢弃旧数据
- [x] C52 T6 MarketChannel 空时 try_recv 返回 None
- [x] C53 T7 MockMarketSource::new 空
- [x] C54 T8 MockMarketSource::with_data 预加载
- [x] C55 T9 MockMarketSource::recv_nonblocking 有数据
- [x] C56 T10 MockMarketSource::recv_nonblocking 空返回 None
- [x] C57 T11 HeartbeatStatus::Alive / Dead
- [x] C58 T12 AgentRuntimeError 变体构造
- [x] C59 T13 EnergyAgent::new_default 构造
- [x] C60 T14 EnergyAgent::on_start 状态转 Running
- [x] C61 T15 EnergyAgent::on_tick 执行双脑（慢路径）
- [x] C62 T16 EnergyAgent::on_tick 接收市场数据
- [x] C63 T17 EnergyAgent::on_stop 状态转 Dead
- [x] C64 T18 EnergyAgent::on_heartbeat Running → Alive
- [x] C65 T19 EnergyAgent::on_heartbeat 非 Running → Dead
- [x] C66 T20 MarketAgent::new_default 构造
- [x] C67 T21 MarketAgent::on_tick 接收并转发数据
- [x] C68 T22 MarketAgent::on_tick 无数据正常返回
- [x] C69 T23 双 Agent 协作：MarketAgent send → EnergyAgent 接收
- [x] C70 T24 EnergyAgent 双脑失败降级（state = Error）
- [x] C71 `cargo test -p eneros-energy-market-agent` 全部通过

## 设计文档
- [x] C72 `docs/agents/energy-market-agent-design.md` 存在
- [x] C73 12 章节完整
- [x] C74 2 Mermaid 图（双 Agent 协作流程图 + Energy Agent tick 时序图）
- [x] C75 D1~D14 偏差声明表
- [x] C76 文档在 `docs/agents/` 下

## 版本同步
- [x] C77 `Makefile` 版本号 `0.72.0`（header + VERSION 变量 2 处）
- [x] C78 `.github/workflows/ci.yml` 版本号 `0.72.0`
- [x] C79 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-energy-market-agent`

## 构建校验（§2.4.2 C6~C11）
- [x] C80 `cargo metadata --format-version 1` 成功
- [x] C81 `cargo test -p eneros-energy-market-agent` 全部通过
- [x] C82 `cargo build -p eneros-energy-market-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C83 `cargo fmt -p eneros-energy-market-agent -- --check` 通过
- [x] C84 `cargo clippy -p eneros-energy-market-agent --all-targets -- -D warnings` 无 warning
- [x] C85 `cargo deny check licenses bans sources` 通过

## no_std 合规
- [x] C86 无 `use std::*`（仅 `alloc::*` / `core::*`）
- [x] C87 无 `panic!` / `todo!` / `unimplemented!`
- [x] C88 子模块不重复 `#![cfg_attr(not(test), no_std)]`
- [x] C89 无 `unsafe` 块
- [x] C90 无 `Instant::now()` / `SystemTime::now()` / `uuid::Uuid::new_v4()`（D2/D3）
- [x] C91 无 `log::warn!` / `log::info!` / `log::error!`（D1）
- [x] C92 无 `std::net::TcpStream` / `std::sync::Mutex`（D5）

## 目录规范
- [x] C93 crate 在 `crates/agents/energy-market-agent/`
- [x] C94 跨 crate path 引用均为相对路径
- [x] C95 文档在 `docs/agents/` 下
- [x] C96 无根目录 crate（除 `ci/`）
- [x] C97 无垃圾文件

## 依赖复用
- [x] C98 复用 v0.71.0 `DualBrainCoordinator<MockSolver>` / `DualBrainMockEngine` / `MockCommandSink`
- [x] C99 复用 v0.70.0 `RealtimeState`
- [x] C100 复用 v0.66.0 `ScheduleConfig` / `ScheduleResult`
- [x] C101 复用 v0.64.0 `MockSolver`
- [x] C102 复用 v0.59.0 `LlmEngine` / `InferParams`
- [x] C103 复用 v0.33.0 `AgentDescriptor` / `AgentType` / `AgentState` / `TrustLevel` / `AgentError` / `AgentId`

## 简化设计验证（Karpathy 原则）
- [x] C104 单 crate 含双 Agent（D12：不做两个 crate）
- [x] C105 `AgentRuntime` trait 本地定义（D6：不依赖不存在的 trait）
- [x] C106 `HeartbeatStatus` 本地定义 2 级（D8：不滥用 4 级 `HealthStatus`）
- [x] C107 `MarketChannel` 简单 Vec-backed（D4：无外部通道依赖）
- [x] C108 `MarketDataSource` trait + Mock（D5：无网络栈依赖）
- [x] C109 安全默认仅状态标记（D14：功率归零由 v0.73.0 实现）
- [x] C110 `AgentRuntimeError` 不派生 Clone（D12）
