# Checklist

## Task 1: crate Cargo.toml
- [x] C1: `crates/agents/grid_agent/Cargo.toml` 中 `name = "eneros-grid-agent"`
- [x] C2: `version.workspace = true` / `edition.workspace = true` / `authors.workspace = true` / `license.workspace = true`（沿用 device-agent 风格）
- [x] C3: `description` 含 "v0.82.0 Grid Agent" 与 "电网状态感知" 字样
- [x] C4: `[dependencies]` 含 `eneros-agent = { path = "../agent" }` 与 `eneros-energy-market-agent = { path = "../energy-market-agent" }`
- [x] C5: 不引入 `eneros-protocol-abstract` / `eneros-upa-model` / `eneros-agent-bus-dds` / `eneros-tsn-time`（D7/D8：本地 trait 抽象）

## Task 2: state.rs — GridState + DataQuality + GridAgent
- [x] C6: `DataQuality` 枚举含 3 变体 `Good` / `Invalid` / `Uncertain`
- [x] C7: `DataQuality` 派生 `Debug, Clone, Copy, PartialEq, Eq, Default`
- [x] C8: `DataQuality::default()` 返回 `DataQuality::Invalid`
- [x] C9: `GridState` 结构体含 12 字段（`frequency: f32` / `voltage_a/b/c: f32` / `current_a/b/c: f32` / `active_power: f32` / `reactive_power: f32` / `power_factor: f32` / `timestamp: u64` / `quality: DataQuality`）
- [x] C10: `GridState` 派生 `Debug, Clone, Copy, PartialEq, Default`
- [x] C11: `GridState::default()` 所有 `f32` 字段为 `0.0`，`timestamp = 0`，`quality = DataQuality::Invalid`
- [x] C12: `GridAgent` 结构体含 8 字段（`descriptor` / `sampler: Box<dyn GridSampler>` / `publisher: Box<dyn GridPublisher>` / `state: GridState` / `anomaly_handlers: Vec<fn(&GridState) -> bool>` / `sample_interval_ms: u64` / `agent_state: AgentState` / `tick_count: u64`）
- [x] C13: `GridAgent::new(name, sampler, publisher, sample_interval_ms, now_ms)` 初始化 `descriptor = AgentDescriptor::new(AgentType::Grid, name, now_ms)` / `state = GridState::default()` / `anomaly_handlers = Vec::new()` / `agent_state = AgentState::Created` / `tick_count = 0`
- [x] C14: `GridAgent::register_anomaly_detector(detector: fn(&GridState) -> bool)` 追加到 `anomaly_handlers`
- [x] C15: `GridAgent::current_state(&self) -> &GridState` 返回 `&self.state`
- [x] C16: `impl AgentRuntime for GridAgent` — `descriptor()` 返回 `&self.descriptor`
- [x] C17: `on_start(now_ms)` → `agent_state = Running`，返回 `Ok(())`
- [x] C18: `on_tick(now_ms)` — 调用 `sampler.sample(now_ms)`，失败返回 `Err(AgentRuntimeError::DeviceError("sample failed"))`；成功更新 `state`，迭代 anomaly_handlers，任一返回 true 调用 `publisher.publish_alert(&state)`（失败返回 `Err`），最后 `publisher.publish_state(&state)`（失败返回 `Err`），`tick_count += 1`，返回 `Ok(())`
- [x] C19: `on_stop(now_ms)` → `agent_state = Dead`，返回 `Ok(())`
- [x] C20: `on_heartbeat(now_ms)` → `agent_state == Running` 返回 `HeartbeatStatus::Alive`，否则 `Dead`
- [x] C21: `state.rs` 使用 `use alloc::boxed::Box;` + `use alloc::vec::Vec;` + `use eneros_agent::{AgentDescriptor, AgentState, AgentType};` + `use eneros_energy_market_agent::{AgentRuntime, AgentRuntimeError, HeartbeatStatus};`（no_std 合规）
- [x] C22: `state.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe` / 无 `todo!` / 无 `unimplemented!`

## Task 3: sampler.rs — GridSampler trait + MockGridSampler
- [x] C23: `GridSampler` trait 定义 `fn sample(&mut self, now_ms: u64) -> Result<GridState, GridError>;`
- [x] C24: `GridSampler` 不要求 `Send + Sync`（D10）
- [x] C25: `MockGridSampler` 结构体含字段 `next_state: GridState` / `fail: bool`，派生 `Debug, Clone`
- [x] C26: `MockGridSampler::new(state: GridState) -> Self`（`fail = false`）
- [x] C27: `MockGridSampler::new_failing() -> Self`（`fail = true`，`next_state = GridState::default()`）
- [x] C28: `MockGridSampler::with_state(mut self, state: GridState) -> Self` builder
- [x] C29: `impl GridSampler for MockGridSampler` — `fail == true` 返回 `Err(GridError::SampleFailed)`；否则返回 `Ok(self.next_state with timestamp = now_ms)`
- [x] C30: `is_valid_grid(freq: f32, voltage: f32) -> bool` — `freq ∈ [49.5, 50.5] && voltage ∈ [200.0, 240.0]`
- [x] C31: `default_anomaly_detectors() -> Vec<fn(&GridState) -> bool>` 返回 3 个检测器
- [x] C32: `frequency_out_of_range` 检测器 — `state.frequency < 49.5 || state.frequency > 50.5`
- [x] C33: `voltage_out_of_range` 检测器 — `state.voltage_a < 200.0 || state.voltage_a > 240.0`
- [x] C34: `quality_invalid` 检测器 — `state.quality == DataQuality::Invalid`
- [x] C35: `sampler.rs` 使用 `use alloc::vec::Vec;` + `use crate::error::GridError;` + `use crate::state::{DataQuality, GridState};`（no_std 合规）
- [x] C36: `sampler.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe`

## Task 4: publisher.rs — GridPublisher trait + MockGridPublisher
- [x] C37: `GridPublisher` trait 定义 `fn publish_state(&mut self, state: &GridState) -> Result<(), GridError>;` 与 `fn publish_alert(&mut self, state: &GridState) -> Result<(), GridError>;`
- [x] C38: `GridPublisher` 不要求 `Send + Sync`（D10）
- [x] C39: `MockGridPublisher` 结构体含字段 `published_states: Vec<GridState>` / `published_alerts: Vec<GridState>` / `fail_state: bool` / `fail_alert: bool`，派生 `Debug, Clone`
- [x] C40: `MockGridPublisher::new() -> Self`（默认空 Vec + false）
- [x] C41: `MockGridPublisher::new_failing_state() -> Self`（`fail_state = true`）
- [x] C42: `MockGridPublisher::new_failing_alert() -> Self`（`fail_alert = true`）
- [x] C43: `impl GridPublisher for MockGridPublisher` — `fail_state == true` 返回 `Err(PublishFailed)`；否则 `published_states.push(*state)` 返回 `Ok(())`；`publish_alert` 同理
- [x] C44: `publish_state(publisher: &mut dyn GridPublisher, state: &GridState) -> Result<(), GridError>` 辅助函数存在
- [x] C45: `publisher.rs` 使用 `use alloc::vec::Vec;` + `use crate::error::GridError;` + `use crate::state::GridState;`（no_std 合规）
- [x] C46: `publisher.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe`

## Task 5: lib.rs — crate 入口 + GridError + 测试
- [x] C47: 顶部模块文档注释含 v0.82.0 描述 + D1~D14 偏差表 + no_std 合规声明 + 1 个示例代码块
- [x] C48: `#![cfg_attr(not(test), no_std)]` 存在
- [x] C49: `extern crate alloc;` 存在
- [x] C50: `pub mod publisher;` / `pub mod sampler;` / `pub mod state;` 按字母序声明
- [x] C51: `pub use publisher::{publish_state, GridPublisher, MockGridPublisher};` 导出
- [x] C52: `pub use sampler::{default_anomaly_detectors, is_valid_grid, GridSampler, MockGridSampler};` 导出
- [x] C53: `pub use state::{DataQuality, GridAgent, GridState};` 导出
- [x] C54: `GridError` 枚举含 3 变体 `SampleFailed` / `PublishFailed` / `InvalidConfig`
- [x] C55: `GridError` 派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C56: `impl From<GridError> for AgentRuntimeError` — 所有变体映射到 `AgentRuntimeError::DeviceError(alloc::string::String::from("..."))`
- [x] C57: 新增 T1 测试 — `GridState::default()` 全零 + `DataQuality::Invalid`
- [x] C58: 新增 T2 测试 — `GridState` 字段可读访问
- [x] C59: 新增 T3 测试 — `GridState::default() == GridState::default()`（PartialEq 一致性）
- [x] C60: 新增 T4 测试 — `DataQuality::default()` 返回 `Invalid`
- [x] C61: 新增 T5 测试 — `DataQuality` 3 变体 Display/Debug 输出非空
- [x] C62: 新增 T6 测试 — `MockGridSampler::new(state)` 初始化 `fail = false`
- [x] C63: 新增 T7 测试 — `MockGridSampler::new_failing()` 初始化 `fail = true`
- [x] C64: 新增 T8 测试 — `MockGridSampler::sample(now_ms)` 成功路径返回 `Ok(state)` 且 `state.timestamp == now_ms`
- [x] C65: 新增 T9 测试 — `MockGridSampler::sample(now_ms)` 失败路径返回 `Err(SampleFailed)`
- [x] C66: 新增 T10 测试 — `MockGridSampler::with_state(state)` builder 链式调用
- [x] C67: 新增 T11 测试 — `MockGridPublisher::new()` 初始化空 Vec + false
- [x] C68: 新增 T12 测试 — `MockGridPublisher::new_failing_state()` `fail_state = true`
- [x] C69: 新增 T13 测试 — `MockGridPublisher::new_failing_alert()` `fail_alert = true`
- [x] C70: 新增 T14 测试 — `MockGridPublisher::publish_state(&state)` 成功路径 `published_states.push` + 返回 `Ok(())`
- [x] C71: 新增 T15 测试 — `MockGridPublisher::publish_state(&state)` 失败路径返回 `Err(PublishFailed)`
- [x] C72: 新增 T16 测试 — `MockGridPublisher::publish_alert(&state)` 成功路径 `published_alerts.push` + 返回 `Ok(())`
- [x] C73: 新增 T17 测试 — `MockGridPublisher::publish_alert(&state)` 失败路径返回 `Err(PublishFailed)`
- [x] C74: 新增 T18 测试 — `publish_state(publisher, state)` 辅助函数委托正确
- [x] C75: 新增 T19 测试 — `GridAgent::new(...)` 初始化 `descriptor.agent_type == Grid` / `state == GridState::default()` / `agent_state == Created` / `tick_count == 0`
- [x] C76: 新增 T20 测试 — `GridAgent::register_anomaly_detector(detector)` 追加到 `anomaly_handlers`
- [x] C77: 新增 T21 测试 — `GridAgent::current_state()` 返回 `&self.state`
- [x] C78: 新增 T22 测试 — `impl AgentRuntime::descriptor()` 返回 `&AgentDescriptor`
- [x] C79: 新增 T23 测试 — `on_start(now_ms)` → `agent_state == Running` + 返回 `Ok(())`
- [x] C80: 新增 T24 测试 — `on_tick(now_ms)` 成功采样 → `tick_count == 1` + `state` 更新 + `publisher.published_states.len() == 1`
- [x] C81: 新增 T25 测试 — `on_tick(now_ms)` 采样失败 → 返回 `Err(AgentRuntimeError::DeviceError(...))` + `tick_count` 不变
- [x] C82: 新增 T26 测试 — `on_tick(now_ms)` 发布 state 失败 → 返回 `Err(AgentRuntimeError::DeviceError(...))` + `tick_count` 不变（采样成功后发布失败）
- [x] C83: 新增 T27 测试 — `on_tick(now_ms)` 异常检测触发 alert — frequency=49.0 越下限 + 注册 `frequency_out_of_range` → `publisher.published_alerts.len() == 1` && `publisher.published_states.len() == 1`
- [x] C84: 新增 T28 测试 — `on_tick(now_ms)` 无异常时不调用 `publish_alert` — frequency=50.0 正常 + 注册 detector → `published_alerts.is_empty()` && `published_states.len() == 1`
- [x] C85: 新增 T29 测试 — `on_tick(now_ms)` 多个 anomaly_handlers 中任一返回 true 即触发 alert
- [x] C86: 新增 T30 测试 — `on_tick(now_ms)` alert 发布失败 → 返回 `Err(AgentRuntimeError::DeviceError(...))` + `tick_count` 不变
- [x] C87: 新增 T31 测试 — `on_stop(now_ms)` → `agent_state == Dead` + 返回 `Ok(())`
- [x] C88: 新增 T32 测试 — `on_heartbeat(now_ms)` Running → `Alive`
- [x] C89: 新增 T33 测试 — `on_heartbeat(now_ms)` 非 Running → `Dead`
- [x] C90: 新增 T34 测试 — `is_valid_grid(50.0, 220.0)` 返回 `true`
- [x] C91: 新增 T35 测试 — `is_valid_grid(49.0, 220.0)` 返回 `false`（频率越下限）
- [x] C92: 新增 T36 测试 — `is_valid_grid(51.0, 220.0)` 返回 `false`（频率越上限）
- [x] C93: 新增 T37 测试 — `is_valid_grid(50.0, 199.0)` 返回 `false`（电压越下限）
- [x] C94: 新增 T38 测试 — `is_valid_grid(50.0, 241.0)` 返回 `false`（电压越上限）
- [x] C95: 新增 T39 测试 — `default_anomaly_detectors()` 返回 3 个检测器
- [x] C96: 新增 T40 测试 — `default_anomaly_detectors()[0]` (frequency) 对 frequency=49.0 返回 `true`
- [x] C97: 新增 T41 测试 — `default_anomaly_detectors()[1]` (voltage) 对 voltage=199.0 返回 `true`
- [x] C98: 新增 T42 测试 — `default_anomaly_detectors()[2]` (quality) 对 `DataQuality::Invalid` 返回 `true`
- [x] C99: 新增 T43 测试 — `GridError::SampleFailed` 与 `AgentRuntimeError` 之间 `From` 转换正确
- [x] C100: 新增 T44 测试 — `GridError::PublishFailed` 与 `AgentRuntimeError` 之间 `From` 转换正确
- [x] C101: 新增 T45 测试 — `GridError::InvalidConfig` 与 `AgentRuntimeError` 之间 `From` 转换正确
- [x] C102: `lib.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe`

## Task 6: configs/grid_points.toml
- [x] C103: 文件位于 `configs/grid_points.toml`
- [x] C104: TOML 模板含 `[grid]` 段 + 10 个 `*_point_id` 字段
- [x] C105: TOML 模板含 `[sampling]` 段 + `interval_ms` / `timeout_ms` / `quality_threshold` 字段
- [x] C106: TOML 模板含 `[anomaly]` 段 + `frequency_min` / `frequency_max` / `voltage_min` / `voltage_max` 字段
- [x] C107: 含中文注释说明各字段用途（与 v0.79.0 gptp.toml / v0.81.0 latency_probe.toml 风格一致）

## Task 7: docs/agents/grid-agent-design.md
- [x] C108: 文件位于 `docs/agents/grid-agent-design.md`（非 `docs/phase2/`，符合 D2）
- [x] C109: 12 章节完整（版本目标 / 前置依赖 / 交付物清单 / 数据结构 / 接口设计 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险与坑点 / 偏差声明）
- [x] C110: 至少 1 个 Mermaid 图（采样→检测→发布 sequence diagram，蓝图 §4.3 风格）
- [x] C111: 至少 1 个 Mermaid 图（on_tick 状态机或异常检测决策流程图）
- [x] C112: D1~D14 偏差声明表完整
- [x] C113: 引用 v0.51.0 协议抽象 + v0.75.0 Agent Bus DDS + v0.79.0 gPTP 作为前置依赖
- [x] C114: 包含性能目标说明（采样周期 100ms / 发布延迟 < 50ms，但标注为"硬件集成阶段验收，本版本仅算法骨架"）

## Task 8: 版本同步根目录文件
- [x] C115: 根 `Cargo.toml` 顶层 `[workspace.package] version = "0.82.0"`
- [x] C116: 根 `Cargo.toml` `[workspace.members]` 列表追加 `"crates/agents/grid_agent"`
- [x] C117: `Makefile` 中 `# Version: v0.82.0` 与 `VERSION := 0.82.0`
- [x] C118: `.github/workflows/ci.yml` 中 `# Version: v0.82.0`
- [x] C119: `ci/src/gate.rs` clippy 段注释含 `eneros-grid-agent v0.82.0` 与类型列表
- [x] C120: `ci/src/gate.rs` test 段注释同步追加类型列表

## Task 9: 构建校验（§2.4.2）
- [x] C121: `cargo metadata --format-version 1` 成功（含新 crate）
- [x] C122: `cargo test -p eneros-grid-agent` 全部通过（T1~T45 = 45 tests + 1 doctest，0 failures）
- [x] C123: `cargo build -p eneros-grid-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 退出码 0
- [x] C124: `cargo fmt -p eneros-grid-agent -- --check` 退出码 0
- [x] C125: `cargo clippy -p eneros-grid-agent --all-targets -- -D warnings` 无 warning，退出码 0
- [x] C126: `cargo deny check advisories licenses bans sources` 通过（无新依赖引入）
- [x] C127: 回归 — `cargo test -p eneros-agent-bus-dds` 仍通过 63 tests + 1 doctest（无回归）
- [x] C128: 回归 — `cargo test -p eneros-tsn-time` 仍通过 84 tests + 1 doctest（无回归）
- [x] C129: 回归 — `cargo test -p eneros-device-agent` 仍通过（AgentRuntime trait 未变）

## 总体校验
- [x] C130: 无根目录新 crate（`crates/agents/grid_agent/` 符合 §2.3.1 子系统归属）
- [x] C131: 无 `docs/` 根目录平面化文档（新文档在 `docs/agents/` 下）
- [x] C132: 无 `config/` 目录（新配置在 `configs/grid_points.toml`）
- [x] C133: `.gitignore` 未需更新（无新文件类型）
- [x] C134: `git status` 无 `target/` / `*.elf` / `*.bin` / `*.dtb` / IDE 缓存被追踪
- [x] C135: 提交信息遵循 Conventional Commits（如 `feat(agents/grid_agent): v0.82.0 实现 Grid Agent 电网状态感知`）
- [x] C136: ADR 决策未被违反（未引入研究特性、未自研已有开源替代组件、未超出 v1.0.0 范围）
- [x] C137: no_std 合规性：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` 保留
- [x] C138: 内存预算：Grid Agent ≤ 5MB（蓝图 §8.3 声明，本版本为算法骨架，实际占用远小于此）
- [x] C139: SBOM 未变化（无新第三方依赖，仅引入 workspace 内既有 crate `eneros-agent` / `eneros-energy-market-agent`）
- [x] C140: 文档同步：v0.79.0 / v0.80.0 / v0.81.0 历史偏差声明保留，v0.82.0 新增 D1~D14 段落
