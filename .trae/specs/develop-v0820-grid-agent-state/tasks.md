# Tasks

- [x] Task 1: 新建 crate `crates/agents/grid_agent/Cargo.toml`
  - [x] SubTask 1.1: `[package]` 段 `name = "eneros-grid-agent"` / `version.workspace = true` / `edition.workspace = true` / `authors.workspace = true` / `license.workspace = true` / `description = "EnerOS v0.82.0 Grid Agent — 电网状态感知 (采样/异常检测/DDS 发布抽象, no_std)"`
  - [x] SubTask 1.2: `[dependencies]` 含 `eneros-agent = { path = "../agent" }` 与 `eneros-energy-market-agent = { path = "../energy-market-agent" }`（沿用 device-agent 依赖风格）
  - [x] SubTask 1.3: 不引入 `eneros-protocol-abstract` / `eneros-upa-model` / `eneros-agent-bus-dds` / `eneros-tsn-time`（D7/D8：通过本地 trait 抽象，避免协议层与 Agent Bus 间接依赖）

- [x] Task 2: 实现 `crates/agents/grid_agent/src/state.rs` — GridState + DataQuality + GridAgent
  - [x] SubTask 2.1: `DataQuality` 枚举（3 变体 `Good` / `Invalid` / `Uncertain`），派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（Default → `Invalid`）
  - [x] SubTask 2.2: `GridState` 结构体（12 字段：`frequency: f32` / `voltage_a/b/c: f32` / `current_a/b/c: f32` / `active_power: f32` / `reactive_power: f32` / `power_factor: f32` / `timestamp: u64` / `quality: DataQuality`），派生 `Debug, Clone, Copy, PartialEq, Default`（Default → `quality: Invalid`，其余 0.0/0）
  - [x] SubTask 2.3: `GridAgent` 结构体（8 字段：`descriptor: AgentDescriptor` / `sampler: Box<dyn GridSampler>` / `publisher: Box<dyn GridPublisher>` / `state: GridState` / `anomaly_handlers: Vec<fn(&GridState) -> bool>` / `sample_interval_ms: u64` / `agent_state: AgentState` / `tick_count: u64`）
  - [x] SubTask 2.4: `GridAgent::new(name: &str, sampler: Box<dyn GridSampler>, publisher: Box<dyn GridPublisher>, sample_interval_ms: u64, now_ms: u64) -> Self`
  - [x] SubTask 2.5: `GridAgent::register_anomaly_detector(&mut self, detector: fn(&GridState) -> bool)` 追加到 `anomaly_handlers`
  - [x] SubTask 2.6: `GridAgent::current_state(&self) -> &GridState`
  - [x] SubTask 2.7: `impl AgentRuntime for GridAgent` — `descriptor()` / `on_start(now_ms)` / `on_tick(now_ms)` / `on_stop(now_ms)` / `on_heartbeat(now_ms)`
  - [x] SubTask 2.8: `on_tick` 逻辑：`sampler.sample(now_ms)` → 失败返回 `Err(AgentRuntimeError::DeviceError("sample failed"))`；成功则更新 `state`，迭代 `anomaly_handlers`，任一返回 true 则 `publisher.publish_alert(&state)`（失败返回 `Err`），最后 `publisher.publish_state(&state)`（失败返回 `Err`），`tick_count += 1`，返回 `Ok(())`
  - [x] SubTask 2.9: `state.rs` 使用 `use alloc::boxed::Box;` + `use alloc::vec::Vec;` + `use eneros_agent::{AgentDescriptor, AgentState, AgentType};` + `use eneros_energy_market_agent::{AgentRuntime, AgentRuntimeError, HeartbeatStatus};` + `use crate::publisher::GridPublisher;` + `use crate::sampler::GridSampler;`（no_std 合规）
  - [x] SubTask 2.10: `state.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe` / 无 `todo!` / 无 `unimplemented!`

- [x] Task 3: 实现 `crates/agents/grid_agent/src/sampler.rs` — GridSampler trait + MockGridSampler
  - [x] SubTask 3.1: `GridSampler` trait（`fn sample(&mut self, now_ms: u64) -> Result<GridState, GridError>;`），不要求 `Send + Sync`（D10）
  - [x] SubTask 3.2: `MockGridSampler` 结构体（字段 `next_state: GridState` / `fail: bool`），派生 `Debug, Clone`
  - [x] SubTask 3.3: `MockGridSampler::new(state: GridState) -> Self`（`fail = false`）
  - [x] SubTask 3.4: `MockGridSampler::new_failing() -> Self`（`fail = true`，`next_state = GridState::default()`）
  - [x] SubTask 3.5: `MockGridSampler::with_state(mut self, state: GridState) -> Self` builder 方法
  - [x] SubTask 3.6: `impl GridSampler for MockGridSampler` — `fail == true` 返回 `Err(GridError::SampleFailed)`；否则返回 `Ok(self.next_state with timestamp = now_ms)`
  - [x] SubTask 3.7: `is_valid_grid(freq: f32, voltage: f32) -> bool` 公开函数（freq ∈ [49.5, 50.5] && voltage ∈ [200.0, 240.0]）
  - [x] SubTask 3.8: `default_anomaly_detectors() -> Vec<fn(&GridState) -> bool>` 返回 3 个检测器（`frequency_out_of_range` / `voltage_out_of_range` / `quality_invalid`）
  - [x] SubTask 3.9: `sampler.rs` 使用 `use alloc::vec::Vec;` + `use crate::error::GridError;` + `use crate::state::{DataQuality, GridState};`（no_std 合规）
  - [x] SubTask 3.10: `sampler.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe`

- [x] Task 4: 实现 `crates/agents/grid_agent/src/publisher.rs` — GridPublisher trait + MockGridPublisher
  - [x] SubTask 4.1: `GridPublisher` trait（`fn publish_state(&mut self, state: &GridState) -> Result<(), GridError>;` + `fn publish_alert(&mut self, state: &GridState) -> Result<(), GridError>;`），不要求 `Send + Sync`（D10）
  - [x] SubTask 4.2: `MockGridPublisher` 结构体（字段 `published_states: Vec<GridState>` / `published_alerts: Vec<GridState>` / `fail_state: bool` / `fail_alert: bool`），派生 `Debug, Clone`
  - [x] SubTask 4.3: `MockGridPublisher::new() -> Self`（全默认：空 Vec + false）
  - [x] SubTask 4.4: `MockGridPublisher::new_failing_state() -> Self`（`fail_state = true`）
  - [x] SubTask 4.5: `MockGridPublisher::new_failing_alert() -> Self`（`fail_alert = true`）
  - [x] SubTask 4.6: `impl GridPublisher for MockGridPublisher` — `fail_state == true` 返回 `Err(PublishFailed)`；否则 `published_states.push(*state)` 返回 `Ok(())`；`publish_alert` 同理
  - [x] SubTask 4.7: `publish_state(publisher: &mut dyn GridPublisher, state: &GridState) -> Result<(), GridError>` 辅助函数（蓝图 §4.5 引用，简单委托给 `publisher.publish_state(state)`）
  - [x] SubTask 4.8: `publisher.rs` 使用 `use alloc::vec::Vec;` + `use crate::error::GridError;` + `use crate::state::GridState;`（no_std 合规）
  - [x] SubTask 4.9: `publisher.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe`

- [x] Task 5: 实现 `crates/agents/grid_agent/src/lib.rs` — crate 入口 + GridError + 测试
  - [x] SubTask 5.1: 顶部模块文档注释（描述 Grid Agent 电网状态感知 + D1~D14 偏差表 + no_std 合规声明 + 示例）
  - [x] SubTask 5.2: `#![cfg_attr(not(test), no_std)]` + `extern crate alloc;`
  - [x] SubTask 5.3: `pub mod publisher;` + `pub mod sampler;` + `pub mod state;`（按字母序）
  - [x] SubTask 5.4: `pub use publisher::{publish_state, GridPublisher, MockGridPublisher};` + `pub use sampler::{default_anomaly_detectors, is_valid_grid, GridSampler, MockGridSampler};` + `pub use state::{DataQuality, GridAgent, GridState};`
  - [x] SubTask 5.5: `GridError` 枚举（3 变体 `SampleFailed` / `PublishFailed` / `InvalidConfig`），派生 `Debug, Clone, Copy, PartialEq, Eq`
  - [x] SubTask 5.6: `impl From<GridError> for AgentRuntimeError` — 所有变体映射到 `AgentRuntimeError::DeviceError(alloc::string::String::from("..."))`
  - [x] SubTask 5.7: 新增 T1~T45 测试（45 个测试，覆盖 GridState / DataQuality / GridSampler / MockGridSampler / GridPublisher / MockGridPublisher / GridAgent 构造与生命周期 / on_tick 成功与失败 / 异常检测触发 / is_valid_grid / default_anomaly_detectors / GridError From 转换）
  - [x] SubTask 5.8: `lib.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe`（除测试模块内 `static` 计数器，如有）

- [x] Task 6: 创建配置文件 `configs/grid_points.toml`
  - [x] SubTask 6.1: TOML 模板含 `[grid]` 段 + `frequency_point_id` / `voltage_a_point_id` / `voltage_b_point_id` / `voltage_c_point_id` / `current_a_point_id` / `current_b_point_id` / `current_c_point_id` / `active_power_point_id` / `reactive_power_point_id` / `power_factor_point_id` 字段（u32）
  - [x] SubTask 6.2: 含 `[sampling]` 段 + `interval_ms` / `timeout_ms` / `quality_threshold` 字段
  - [x] SubTask 6.3: 含 `[anomaly]` 段 + `frequency_min` / `frequency_max` / `voltage_min` / `voltage_max` 字段
  - [x] SubTask 6.4: 附中文注释说明各字段用途（与 v0.79.0 gptp.toml / v0.81.0 latency_probe.toml 风格一致）

- [x] Task 7: 创建设计文档 `docs/agents/grid-agent-design.md`
  - [x] SubTask 7.1: 12 章节完整（版本目标 / 前置依赖 / 交付物清单 / 数据结构 / 接口设计 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险与坑点 / 偏差声明）
  - [x] SubTask 7.2: 至少 1 个 Mermaid 图（采样→检测→发布 sequence diagram，蓝图 §4.3 风格）
  - [x] SubTask 7.3: 至少 1 个 Mermaid 图（on_tick 状态机或异常检测决策流程图）
  - [x] SubTask 7.4: D1~D14 偏差声明表完整
  - [x] SubTask 7.5: 引用 v0.51.0 协议抽象 + v0.75.0 Agent Bus DDS + v0.79.0 gPTP 作为前置依赖
  - [x] SubTask 7.6: 包含性能目标说明（采样周期 100ms / 发布延迟 < 50ms，但标注为"硬件集成阶段验收，本版本仅算法骨架"）

- [x] Task 8: 版本同步根目录文件
  - [x] SubTask 8.1: 根 `Cargo.toml` 顶层 `[workspace.package] version = "0.81.0"` → `"0.82.0"`
  - [x] SubTask 8.2: 根 `Cargo.toml` `[workspace.members]` 列表追加 `"crates/agents/grid_agent"`
  - [x] SubTask 8.3: `Makefile` 版本号 `0.81.0` → `0.82.0`（header 注释 + VERSION 变量）
  - [x] SubTask 8.4: `.github/workflows/ci.yml` 版本号 `0.81.0` → `0.82.0`
  - [x] SubTask 8.5: `ci/src/gate.rs` clippy 段注释追加 `eneros-grid-agent v0.82.0` 与类型列表（`GridState / DataQuality / GridSampler / MockGridSampler / GridPublisher / MockGridPublisher / GridAgent / GridError / is_valid_grid / default_anomaly_detectors`）
  - [x] SubTask 8.6: `ci/src/gate.rs` test 段注释同步追加类型列表

- [x] Task 9: 构建校验（§2.4.2 C6~C11）
  - [x] SubTask 9.1: `cargo metadata --format-version 1` 成功（含新 crate）
  - [x] SubTask 9.2: `cargo test -p eneros-grid-agent` 全部通过（T1~T45 = 45 tests + 1 doctest）
  - [x] SubTask 9.3: `cargo build -p eneros-grid-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] SubTask 9.4: `cargo fmt -p eneros-grid-agent -- --check` 通过
  - [x] SubTask 9.5: `cargo clippy -p eneros-grid-agent --all-targets -- -D warnings` 无 warning
  - [x] SubTask 9.6: `cargo deny check advisories licenses bans sources` 通过（无新依赖引入）
  - [x] SubTask 9.7: 回归 — `cargo test -p eneros-agent-bus-dds` 仍通过 63 tests + 1 doctest（无回归）
  - [x] SubTask 9.8: 回归 — `cargo test -p eneros-tsn-time` 仍通过 84 tests + 1 doctest（无回归）
  - [x] SubTask 9.9: 回归 — `cargo test -p eneros-device-agent` 仍通过（无回归，AgentRuntime trait 未变）

# Task Dependencies

- Task 1（Cargo.toml）必须先完成 — 后续所有 Task 依赖 crate 已创建
- Task 2（state.rs）依赖 Task 1，且被 Task 3/4 引用（`GridSampler` 在 sampler.rs，`GridPublisher` 在 publisher.rs，但 `GridAgent` 持有它们的 `Box<dyn>`）
- Task 3（sampler.rs）依赖 Task 2（`GridState` / `DataQuality` 来自 state.rs）+ Task 5（`GridError` 来自 lib.rs）
- Task 4（publisher.rs）依赖 Task 2（`GridState` 来自 state.rs）+ Task 5（`GridError` 来自 lib.rs）
- Task 5（lib.rs）依赖 Task 2/3/4 完成（需导出三个模块的类型）
- Task 5 与 Task 2/3/4 可并行起草，但最终编译依赖三者齐备
- Task 6/7（配置 + 文档）可与 Task 2~5 并行
- Task 8（版本同步）依赖 Task 1~7 完成
- Task 9（构建校验）依赖所有前置任务完成
