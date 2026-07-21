# v0.83.0 PCC Management Spec — Grid Agent 并网点管理

## Why

v0.82.0 完成 Grid Agent 电网状态感知（频率/电压/电流/功率采样与异常检测），但尚未管理**并网点（PCC, Point of Common Coupling）**状态。PCC 是微电网与主网的物理接口，其开关位置 + 功率方向是 v0.84.0 并离网切换决策的前提输入。本版本扩展 `eneros-grid-agent` crate 增加 PCC 管理能力：监测开关状态、计算功率方向/功率因数、判定并网/离网/过渡态，并提供最小防抖逻辑。

## What Changes

- **ADDED**：`crates/agents/grid_agent/src/pcc.rs` — PCC 管理模块（新增源文件）
  - 数据结构：`PccState` / `BreakerStatus` / `PowerDirection` / `PccStatus` / `PccReading`
  - 采样器抽象：`PccReader` trait + `MockPccReader`（沿用 v0.82.0 `GridSampler` trait 模式）
  - 管理器：`PccManager`（持有 reader，周期 `update(now_ms)` 更新状态，最小防抖逻辑）
  - 辅助函数：`compute_power_direction(active_power)` / `compute_power_factor(active_power, reactive_power)`
- **MODIFIED**：`crates/agents/grid_agent/src/lib.rs` — 新增 `pub mod pcc;` 模块声明 + `pub use pcc::{...}` 重导出（surgical：仅追加，不修改 v0.82.0 既有代码）
- **MODIFIED**：`crates/agents/grid_agent/Cargo.toml` — `description` 字段追加 "+ v0.83.0 PCC 管理"（无新依赖，沿用 eneros-agent + eneros-energy-market-agent）
- **ADDED**：`configs/pcc.toml` — PCC 配置模板
- **ADDED**：`docs/agents/pcc-management-design.md` — 设计文档（12 章 + Mermaid 图 + D1~D14 偏差表）
- **MODIFIED**：根 `Cargo.toml` workspace 版本 `0.82.0` → `0.83.0`
- **MODIFIED**：`Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs` 版本同步
- **未新增 crate**：PCC 模块追加到 v0.82.0 已存在的 `eneros-grid-agent` crate，无需新增 workspace member

无 **BREAKING** 变更：v0.82.0 既有 `GridState` / `DataQuality` / `GridAgent` / `GridSampler` / `GridPublisher` / `GridError` 公共 API 完全保留；新增类型与函数仅追加，不修改既有签名。

## Impact

- **Affected specs**：v0.82.0 Grid Agent 状态感知（追加 PCC 子模块，不破坏既有 API）；为 v0.84.0 并离网切换提供决策输入
- **Affected code**：
  - `crates/agents/grid_agent/src/pcc.rs`（新建）
  - `crates/agents/grid_agent/src/lib.rs`（追加 `pub mod pcc;` + 重导出，约 3 行新增）
  - `crates/agents/grid_agent/Cargo.toml`（description 字段更新）
  - 根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`（版本同步）
- **依赖不变**：无新第三方依赖；无新 workspace crate 依赖（仅复用 v0.82.0 既有 `eneros-agent` + `eneros-energy-market-agent`）；SBOM 不变
- **回归面**：v0.82.0 的 46 tests 必须仍全部通过；v0.79.0/v0.80.0/v0.81.0/v0.72.0/v0.73.0 等既有 crate 必须无回归

## ADDED Requirements

### Requirement: PCC State Data Structures

系统 SHALL 提供以下 PCC 相关数据结构，全部派生 `Debug, Clone, Copy, PartialEq`（`Default` 视情况）：

- `BreakerStatus` 枚举（4 变体：`Closed` / `Open` / `Tripped` / `Unknown`，`Default → Unknown`）
- `PowerDirection` 枚举（3 变体：`Import` / `Export` / `Idle`，`Default → Idle`；约定 P>0 = Import）
- `PccStatus` 枚举（3 变体：`GridConnected` / `Islanded` / `Transitioning`，`Default → Transitioning`）
- `PccReading` 结构体（3 字段：`breaker_status: BreakerStatus` / `active_power: f32` / `reactive_power: f32`，派生 `Debug, Clone, Copy, PartialEq, Default`）
- `PccState` 结构体（7 字段：`pcc_id: u32` / `breaker_status: BreakerStatus` / `power_direction: PowerDirection` / `power_factor: f32` / `active_power: f32` / `reactive_power: f32` / `status: PccStatus`，派生 `Debug, Clone, Copy, PartialEq, Default`）

#### Scenario: Default values
- **WHEN** 调用 `PccState::default()`
- **THEN** `pcc_id == 0`，`breaker_status == BreakerStatus::Unknown`，`power_direction == PowerDirection::Idle`，`power_factor == 0.0`，`active_power == 0.0`，`reactive_power == 0.0`，`status == PccStatus::Transitioning`

### Requirement: PccReader Trait + MockPccReader

系统 SHALL 提供 `PccReader` trait 抽象 PCC 数据采集源（RTU/IED/PMU/保护装置），不要求 `Send + Sync`（no_std 单线程）：

```rust
pub trait PccReader {
    fn read(&mut self, pcc_id: u32, now_ms: u64) -> Result<PccReading, GridError>;
}
```

系统 SHALL 提供 `MockPccReader` 用于测试：
- 字段 `next_reading: PccReading` / `fail: bool`，派生 `Debug, Clone`
- `MockPccReader::new(reading: PccReading) -> Self`（`fail = false`）
- `MockPccReader::new_failing() -> Self`（`fail = true`，`next_reading = PccReading::default()`）
- `MockPccReader::with_reading(mut self, reading: PccReading) -> Self` builder
- `impl PccReader for MockPccReader` — `fail == true` 返回 `Err(GridError::SampleFailed)`；否则返回 `Ok(self.next_reading)`

#### Scenario: Read success
- **WHEN** `MockPccReader::new(normal_reading).read(1, 1000)`
- **THEN** 返回 `Ok(reading)`，其中 `reading == normal_reading`

#### Scenario: Read failure
- **WHEN** `MockPccReader::new_failing().read(1, 1000)`
- **THEN** 返回 `Err(GridError::SampleFailed)`

### Requirement: PccManager

系统 SHALL 提供 `PccManager` 管理单个 PCC 点的周期性状态更新：

- 字段（6 个）：`pcc_id: u32` / `reader: Box<dyn PccReader>` / `state: PccState` / `debounce_ms: u64` / `last_breaker_status: BreakerStatus` / `last_change_ms: u64`
- `PccManager::new(pcc_id: u32, reader: Box<dyn PccReader>, debounce_ms: u64) -> Self`
  - 初始化：`state = PccState::default()`（含 `status = Transitioning`）/ `last_breaker_status = BreakerStatus::Unknown` / `last_change_ms = 0`
- `PccManager::current(&self) -> &PccState`
- `PccManager::is_islanded(&self) -> bool`（`self.state.status == PccStatus::Islanded`）
- `PccManager::update(&mut self, now_ms: u64) -> Result<PccState, GridError>` — 核心更新逻辑：
  1. 调用 `reader.read(pcc_id, now_ms)`，失败返回 `Err(GridError::SampleFailed)`
  2. 取 `new_breaker = reading.breaker_status`
  3. **防抖逻辑**：
     - 若 `new_breaker != self.last_breaker_status`：更新 `last_breaker_status = new_breaker`，`last_change_ms = now_ms`，`state.status = PccStatus::Transitioning`
     - 否则若 `now_ms - last_change_ms >= debounce_ms`：`state.status = compute_stable_status(new_breaker)`
     - 否则（防抖期内）：`state.status` 保持 `Transitioning`
  4. 更新 `state.breaker_status = new_breaker`
  5. 更新 `state.active_power = reading.active_power`，`state.reactive_power = reading.reactive_power`
  6. 更新 `state.power_direction = compute_power_direction(reading.active_power)`
  7. 更新 `state.power_factor = compute_power_factor(reading.active_power, reading.reactive_power)`
  8. 返回 `Ok(self.state)`

`compute_stable_status(breaker: BreakerStatus) -> PccStatus`：
- `Closed → GridConnected`
- `Open → Islanded`
- `Tripped → Islanded`
- `Unknown → Transitioning`（不确定，保持过渡态）

#### Scenario: Initial state
- **WHEN** `PccManager::new(1, Box::new(MockPccReader::new(reading)), 100)`
- **THEN** `current().status == Transitioning`，`is_islanded() == false`

#### Scenario: First update returns Transitioning
- **WHEN** 首次调用 `update(1000)` 且 `debounce_ms = 100`，reader 返回 `breaker_status = Closed`
- **THEN** 返回 `Ok(state)`，`state.status == Transitioning`（防抖期未过）

#### Scenario: Stable after debounce
- **WHEN** 第一次 `update(1000)` 返回 `Closed` → 第二次 `update(1100)`（`now_ms - last_change_ms = 100 >= debounce_ms`）返回 `Closed`
- **THEN** 第二次返回 `state.status == GridConnected`

#### Scenario: Breaker open → Islanded
- **WHEN** reader 返回 `breaker_status = Open`，且防抖期已过
- **THEN** `state.status == Islanded`，`is_islanded() == true`

#### Scenario: Breaker change resets debounce
- **WHEN** 稳态 `Closed` 后 reader 返回 `Open`
- **THEN** `state.status == Transitioning`（防抖重置）

#### Scenario: Read failure
- **WHEN** `reader.read()` 返回 `Err`
- **THEN** `update()` 返回 `Err(GridError::SampleFailed)`，`state` 不变

### Requirement: Power Calculation Helpers

系统 SHALL 提供两个公开辅助函数：

- `compute_power_direction(active_power: f32) -> PowerDirection`：
  - `active_power > 1.0 → Import`（导入，P 为正）
  - `active_power < -1.0 → Export`（导出，P 为负）
  - 否则 `→ Idle`（|P| ≤ 1.0 视为空载）
- `compute_power_factor(active_power: f32, reactive_power: f32) -> f32`：
  - 若 `|active_power| < 0.1` → 返回 `1.0`（避免除零与微小值噪声）
  - 否则 → 返回 `active_power / (active_power² + reactive_power²).sqrt()`（使用 `core::f32::sqrt`）

#### Scenario: Import direction
- **WHEN** `compute_power_direction(10.0)`
- **THEN** 返回 `PowerDirection::Import`

#### Scenario: Export direction
- **WHEN** `compute_power_direction(-10.0)`
- **THEN** 返回 `PowerDirection::Export`

#### Scenario: Idle direction
- **WHEN** `compute_power_direction(0.5)`
- **THEN** 返回 `PowerDirection::Idle`

#### Scenario: Power factor 3-4-5 triangle
- **WHEN** `compute_power_factor(3.0, 4.0)`
- **THEN** 返回值约等于 `0.6`（误差 < 1e-6）

#### Scenario: Power factor zero power
- **WHEN** `compute_power_factor(0.0, 0.0)`
- **THEN** 返回 `1.0`（避免除零）

### Requirement: no_std Compliance

所有新增代码 MUST 满足 no_std 合规：
- `pcc.rs` 不添加 `#![cfg_attr(not(test), no_std)]`（继承 lib.rs 的 crate 级属性）
- `pcc.rs` 仅 `use alloc::boxed::Box;`（如需要）+ `use crate::GridError;`（复用 v0.82.0 错误类型）
- 禁止 `use std::*` / `async` / `panic!` / `unsafe` / `todo!` / `unimplemented!` / `Instant::now()`
- `core::f32::sqrt` 用于 PF 计算（aarch64 原生 `fsqrts` 指令，无需 libm）

## MODIFIED Requirements

### Requirement: eneros-grid-agent crate 公共 API

v0.82.0 既有公共 API（`GridAgent` / `GridState` / `DataQuality` / `GridSampler` / `MockGridSampler` / `GridPublisher` / `MockGridPublisher` / `GridError` / `is_valid_grid` / `default_anomaly_detectors` / `publish_state`）全部保留不变。

本版本追加以下公共 API（仅追加，不修改既有签名）：
- 模块：`pub mod pcc;`
- 重导出：`pub use pcc::{compute_power_direction, compute_power_factor, MockPccReader, PccManager, PccReader, PccReading, PccState, PccStatus, BreakerStatus, PowerDirection};`
- crate `description` 字段更新为 `"EnerOS v0.82.0 Grid Agent — 电网状态感知 + v0.83.0 PCC 并网点管理 (采样/异常检测/DDS 发布/PCC 状态抽象, no_std)"`

### Requirement: 版本同步

- 根 `Cargo.toml` `[workspace.package] version = "0.83.0"`
- `Makefile` VERSION 变量 + header 注释 → `0.83.0`
- `.github/workflows/ci.yml` header 注释 → `0.83.0`
- `ci/src/gate.rs` clippy 段 + test 段注释追加：`+ v0.83.0 PCC 管理：PccState / PccReading / BreakerStatus / PowerDirection / PccStatus / PccReader / MockPccReader / PccManager / compute_power_direction / compute_power_factor`
- `crates/agents/grid_agent/Cargo.toml` workspace members 列表**不变**（pcc.rs 是既有 crate 的新模块，非新 crate）

## REMOVED Requirements

无。本版本仅追加，不删除任何既有功能。

## 偏差声明（D1~D14，Karpathy "Think Before Coding"）

| 偏差 | 蓝图原文 | 本版本处理 | 理由 |
|------|---------|-----------|------|
| **D1** | `async fn update()` | sync `fn update(&mut self, now_ms: u64) -> Result<PccState, GridError>` | no_std 无 async runtime；沿用 v0.82.0 D4 sync 模式 |
| **D2** | `pcc_id: String` | `pcc_id: u32` | no_std 无堆 String 分配；Copy 语义使 PccState 可 derive Copy；支持多 PCC（§8.4） |
| **D3** | `PointTable` trait + `points.read(format!("pcc_{}_breaker", id)).await` | `PccReader` trait + `MockPccReader`，返回 `PccReading` 结构体 | 避免依赖 eneros-protocol-abstract / eneros-upa-model；沿用 v0.82.0 D5/D11 trait 抽象模式；避免 `format!()` 堆分配 |
| **D4** | `AgentError` | 复用 v0.82.0 `GridError`（`SampleFailed` 语义复用为读取失败） | 不引入新错误类型；surgical — 不修改 lib.rs 的 `GridError` 定义与 `impl From` |
| **D5** | "扩展 GridAgent 持有 PccManager"（§5.3） | `PccManager` 独立组件，不嵌入 `GridAgent` | Surgical Changes：不破坏 v0.82.0 GridAgent 8 字段与构造器签名；用户可在自己的 Agent 中组合 `GridAgent` + `PccManager`；§7.5 出口判定仅要求"并网点状态实时报告可用"，未要求 GridAgent 持有 |
| **D6** | "状态防抖"（§5.4 难点 + §9 多角度要求）未明确实现 | `PccStatus::Transitioning` + `last_breaker_status` + `last_change_ms` + `debounce_ms` 字段实现最小防抖 | 约 5 行核心逻辑：开关状态变化后 `debounce_ms` 内报告 `Transitioning`，过期后稳定为 `GridConnected`/`Islanded`；满足 §9 "可靠：状态防抖" |
| **D7** | `(p*p+q*q).sqrt()`（隐含 std libm） | `core::f32::sqrt` | aarch64 原生 `fsqrts` 指令；no_std 无需 libm 依赖；与 build-std=core,alloc + compiler-builtins-mem 兼容 |
| **D8** | `docs/phase2/pcc_management.md` + `config/pcc.toml` | `docs/agents/pcc-management-design.md` + `configs/pcc.toml` | 工作区规则 §2.3.3 禁止 `docs/phase2/` 平面化；工作区使用 `configs/` 而非 `config/`；沿用 v0.82.0 命名约定 |
| **D9** | `tests/pcc_status.rs` 集成测试 | `pcc.rs` 内 `#[cfg(test)] mod tests` 单元测试 T47+ | 沿用 v0.82.0 测试模式（lib.rs 内嵌 test 模块）；无需独立集成测试文件 |
| **D10** | `BreakerStatus` 4 变体 | 保持 4 变体（`Closed`/`Open`/`Tripped`/`Unknown`） | `Tripped` 不同于 `Open`（保护跳闸 vs 手动分闸）；`Unknown` 用于 §6.5 故障注入（开关状态丢失告警） |
| **D11** | `PowerDirection` 符号约定（导入为正，§8.5） | 保持：`P > 1.0 → Import`，`P < -1.0 → Export`，`|P| ≤ 1.0 → Idle` | §8.5 符号约定；阈值 1.0 避免零漂误判 |
| **D12** | `PccStatus` 3 变体 | 保持（`GridConnected`/`Islanded`/`Transitioning`） | `Transitioning` 在 D6 防抖逻辑中使用 |
| **D13** | `PccState` 7 字段（含 `String`） | 7 字段（`pcc_id: u32`，全 Copy） | D2 派生；`PccState` 派生 `Copy`，`update()` 返回 `Ok(self.state)` 无需 `clone()` 堆分配 |
| **D14** | 蓝图未定义 `PccReading` | 新增 `PccReading { breaker_status, active_power, reactive_power }` 一次性读取结构体 | D3 抽象：避免多次 `read()` 调用 + `format!()`；单次读取返回所有 PCC 量，原子性强 |

## no_std 合规声明

本版本所有新增代码：
- 继承 crate 级 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc;`（已在 v0.82.0 lib.rs 设置）
- 仅使用 `alloc::boxed::Box` + `core::f32::sqrt` + `core::*`
- 禁止 `use std::*` / `async` / `panic!` / `unsafe` / `todo!` / `unimplemented!` / `Instant::now()`
- 可交叉编译到 `aarch64-unknown-none`（`-Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`）

## Surgical Changes 声明

- v0.82.0 既有源文件 `state.rs` / `sampler.rs` / `publisher.rs` **完全未改动**
- `lib.rs` 仅追加 `pub mod pcc;` + `pub use pcc::{...}` + 顶部文档注释更新（不修改任何既有代码行）
- `Cargo.toml` 仅更新 `description` 字段（依赖列表不变）
- v0.82.0 既有 46 个测试（T1~T45 + `_ensure_imports_used`）必须仍全部通过
