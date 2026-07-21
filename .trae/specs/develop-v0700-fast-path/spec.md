# EnerOS v0.70.0 实时路径 - Solver only Spec

## Why

v0.69.0 定义了 LLM ↔ Solver 之间的双向意图契约，但双脑架构仍有两条路径需要实现：(1) 慢路径（LLM 路径，~2s）——LLM 感知→意图→Solver 求解；(2) 快路径（Solver only，<500ms）——实时状态直接→LP 参数→Solver 求解。本版本实现快路径：当电价/负荷/SOC 变化在预定义阈值内时，使用预计算策略表（3×3=9 种电价×SOC 组合）和实时状态直接生成 LP 参数并求解，跳过 LLM 推理。这保证系统对突发状态变化的快速响应，并为 v0.71.0 双脑联调（快/慢路径切换）奠基。

## What Changes

- **ADDED** 新增 `eneros-fast-path` crate（位于 `crates/ai/fast-path/`）
- **ADDED** `PathType` 枚举：`SlowPath` / `FastPath`（派生 `Debug + Clone + PartialEq`，D8）
- **ADDED** `PathSelector` 路径选择器：根据状态变化阈值与最小间隔决定路径
- **ADDED** `StrategyTable` 预计算策略表：3×3=9 种电价×SOC 组合（D7：修复蓝图 bounds bug，使用 2 bounds × 2 bounds → 3×3=9 桶）
- **ADDED** `RealtimePathEngine` 快速路径引擎：查表 → 微调 → 编译 → 求解 → 校验 → 返回
- **ADDED** `RealtimeState` 本地状态结构（D4：包装 v0.67.0 SystemState + current_price + load_demand，蓝图引用但 v0.67.0 SystemState 无此字段）
- **ADDED** `FastPathResult` 快速路径结果：schedule / solve_result / validation / elapsed_ms / path_type
- **ADDED** `FastPathError` 错误类型：`CompileError(String)` / `SolveError(String)`（仅 Debug，D9）
- **MODIFIED** workspace 版本 `0.69.0` → `0.70.0`
- **MODIFIED** 根 `Cargo.toml` members 添加 `crates/ai/fast-path`
- **MODIFIED** `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs` 版本同步

## Impact

- **Affected specs**：v0.69.0 意图契约（不直接依赖，但 v0.71.0 会桥接契约与快路径）；v0.68.0 IntentParser（不依赖，快路径完全跳过 LLM）
- **Affected code**：
  - 新增：`crates/ai/fast-path/`（Cargo.toml + src/lib.rs + src/error.rs + src/state.rs + src/selector.rs + src/strategy.rs + src/engine.rs）
  - 修改：`Cargo.toml`（版本 + members）/ `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`
  - 复用：`eneros-solver-core::{Solver, MockSolver, LpProblem, SolveResult}`、`eneros-energy-lp-model::{ScheduleConfig, EnergyScheduleModel, ScheduleResult}`、`eneros-safety-validator::{SafetyValidator, SystemState, ValidationResult}`
- **后续解锁**：v0.71.0 双脑协同联调（依赖本版本 RealtimePathEngine + PathSelector）

## ADDED Requirements

### Requirement: PathType 路径类型

系统 SHALL 提供 `PathType` 枚举：`SlowPath`（LLM 路径，~2s）/ `FastPath`（Solver only，<500ms）。派生 `Debug + Clone + PartialEq`（D8：测试需要 == 比较）。

### Requirement: RealtimeState 实时状态

系统 SHALL 提供 `RealtimeState` 结构体（D4：蓝图引用 `current_price` / `load_demand` 但 v0.67.0 `SystemState` 无此字段），字段：
- `system: SystemState`（复用 v0.67.0，含 voltage_v/current_a/frequency_hz/soc_pct/timestamp_ms）
- `current_price: f64`（元/kWh）
- `load_demand: Option<Vec<f64>>`（各时段负荷需求，kW）

派生 `Debug + Clone`。实现 `Default`（current_price=0.5, load_demand=None, system=SystemState::default()）。

### Requirement: PathSelector 路径选择器

系统 SHALL 提供 `PathSelector` 结构体，字段：
- `price_change_threshold: f64`（默认 0.1 元/kWh）
- `soc_change_threshold: f64`（默认 5.0%，注意：与 SOC 0.0~1.0 比较，5% = 0.05）
- `load_change_threshold: f64`（默认 20.0 kW）
- `last_slow_path_ms: Option<u64>`（D10：替代 `Option<Instant>`，no_std）
- `min_slow_path_interval: Duration`（D11：`core::time::Duration`，默认 300s）
- `last_state: Option<RealtimeState>`

实现：
- `new() -> Self`：默认阈值
- `select(&mut self, state: &RealtimeState, now_ms: u64) -> PathType`：
  1. 首次运行（`last_slow_path_ms` 为 None）→ 走慢路径，记录时间
  2. 距上次慢路径超过 `min_slow_path_interval` → 走慢路径
  3. `current_price` 变化 > `price_change_threshold` → 走慢路径
  4. `system.soc_pct` 变化 > `soc_change_threshold / 100.0`（百分比转小数）→ 走慢路径
  5. 默认走快路径
- 实现 `Default`

### Requirement: StrategyTable 预计算策略表

系统 SHALL 提供 `StrategyTable` 结构体（D7：修复蓝图 bounds bug），字段：
- `price_levels: Vec<f64>`（默认 `vec![0.3, 0.7]`，2 bounds → 3 桶：谷/平/峰）
- `soc_levels: Vec<f64>`（默认 `vec![0.3, 0.7]`，2 bounds → 3 桶：低/中/高）
- `strategies: Vec<Vec<ScheduleConfig>>`（3×3=9 种策略）

实现：
- `new(default: ScheduleConfig) -> Self`：按电价×SOC 组合预填充 9 种策略
  - 谷时（price < 0.3）：`soc_final = Some(0.8)`（倾向充电）
  - 峰时（price >= 0.7）：`soc_final = Some(0.3)`（倾向放电）
  - 平时（0.3 <= price < 0.7）：`soc_final = None`（自主调度）
- `get_config(&self, state: &RealtimeState) -> ScheduleConfig`：
  - `price_idx = price_levels.iter().position(|&b| state.current_price < b).unwrap_or(price_levels.len())`
  - `soc_idx = soc_levels.iter().position(|&b| state.system.soc_pct < b).unwrap_or(soc_levels.len())`
  - 返回 `strategies[price_idx][soc_idx].clone()`

### Requirement: RealtimePathEngine 快速路径引擎

系统 SHALL 提供 `RealtimePathEngine<S: Solver>` 结构体（D2：泛型 Solver，默认 MockSolver），字段：
- `solver: S`
- `default_config: ScheduleConfig`
- `validator: SafetyValidator`
- `strategy_table: StrategyTable`

实现：
- `new(config: ScheduleConfig, solver: S) -> Self`
- `execute(&mut self, state: &RealtimeState, now_ms: u64) -> Result<FastPathResult, FastPathError>`：
  1. 从策略表获取基础配置（`strategy_table.get_config(state)`）
  2. 微调：`config.soc_init = state.system.soc_pct`；`config.load_demand = state.load_demand.clone()`
  3. 编译：`EnergyScheduleModel::new(config.clone()).compile()`（D7：map_err 为 `FastPathError::CompileError`）
  4. 求解：`solver.solve(&problem, now_ms)`（D5：使用 trait 方法，超时通过 `set_param` 设置；map_err 为 `FastPathError::SolveError`）
  5. 解析：`model.parse_result(&solve_result)`
  6. 校验：`validator.validate(&schedule, &state.system)`（D12：传 `state.system` 给 v0.67.0 SafetyValidator）
  7. 返回 `FastPathResult { schedule: validation.clamped_schedule.unwrap_or(schedule), solve_result, validation, elapsed_ms, path_type: PathType::FastPath }`
  - `elapsed_ms` 由 `now_ms` 参数与起始时间差计算（D6：no_std 无 Instant）

### Requirement: FastPathResult 快速路径结果

系统 SHALL 提供 `FastPathResult` 结构体：`schedule: ScheduleResult` / `solve_result: SolveResult` / `validation: ValidationResult` / `elapsed_ms: u64` / `path_type: PathType`。派生 `Debug + Clone`（D8）。

### Requirement: FastPathError 错误类型

系统 SHALL 提供 `FastPathError` 枚举：`CompileError(String)` / `SolveError(String)`。仅派生 `Debug`（D9：Karpathy 简化原则，与 v0.68.0/v0.69.0 一致）。使用 `alloc::string::String`。

### Requirement: no_std 合规

crate SHALL 遵循 EnerOS §43.1 no_std 规范：
- `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 子模块不重复 `#![cfg_attr(not(test), no_std)]`
- 禁止 `use std::*` / `panic!` / `todo!` / `unimplemented!`
- 使用 `core::time::Duration`（D1：替代 `std::time::Duration`）
- `Instant::now()` 不可用，改用 `now_ms: u64` 参数（D6）

## MODIFIED Requirements

### Requirement: Workspace 版本同步

根 `Cargo.toml` 的 `[workspace.package] version` 从 `0.69.0` 更新为 `0.70.0`；`members` 在 `crates/ai/intent-contract` 之后添加 `crates/ai/fast-path`。`Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs` 同步版本号。

## REMOVED Requirements

（无移除项）
