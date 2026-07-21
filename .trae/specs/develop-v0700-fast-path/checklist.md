# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.70.0`
- [x] C2 members 列表已添加 `crates/ai/fast-path`（置于 `crates/ai/intent-contract` 之后）
- [x] C3 `cargo metadata --format-version 1` 成功

## Crate 骨架
- [x] C4 `crates/ai/fast-path/Cargo.toml` 存在，package name = `eneros-fast-path`
- [x] C5 dependencies 包含 `eneros-solver-core` / `eneros-energy-lp-model` / `eneros-safety-validator`
- [x] C6 无 `[features]` 段（纯 Rust，无 FFI）
- [x] C7 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`（D9）
- [x] C8 `src/lib.rs` 包含 D1~D12 偏差声明表
- [x] C9 模块声明：error / state / selector / strategy / engine

## error.rs — FastPathError
- [x] C10 `FastPathError` 枚举：`CompileError(String)` / `SolveError(String)`
- [x] C11 派生 `Debug`（D9：不派生 Clone/PartialEq）
- [x] C12 使用 `alloc::string::String`

## state.rs — RealtimeState
- [x] C13 `RealtimeState` 结构体：`system: SystemState`（D4：包装 v0.67.0）/ `current_price: f64` / `load_demand: Option<Vec<f64>>`
- [x] C14 派生 `Debug + Clone`（D8）
- [x] C15 实现 `Default`（current_price=0.5, load_demand=None, system=SystemState::default()）

## selector.rs — PathType + PathSelector
- [x] C16 `PathType` 枚举：`SlowPath` / `FastPath`
- [x] C17 `PathType` 派生 `Debug + Clone + PartialEq`（D8）
- [x] C18 `PathSelector` 结构体：price_change_threshold / soc_change_threshold / load_change_threshold / last_slow_path_ms: Option<u64>（D10）/ min_slow_path_interval: core::time::Duration（D11）/ last_state: Option<RealtimeState>
- [x] C19 `new()`：默认阈值（0.1 / 5.0 / 20.0 / None / 300s / None）
- [x] C20 `select(&mut self, state: &RealtimeState, now_ms: u64) -> PathType`：首次走慢路径
- [x] C21 `select`：间隔超时走慢路径
- [x] C22 `select`：电价变化超阈值走慢路径
- [x] C23 `select`：SOC 变化超阈值走慢路径
- [x] C24 `select`：默认走快路径
- [x] C25 实现 `Default` for `PathSelector`

## strategy.rs — StrategyTable
- [x] C26 `StrategyTable` 结构体：price_levels / soc_levels / strategies: Vec<Vec<ScheduleConfig>>
- [x] C27 `new(default: ScheduleConfig)`：price_levels=vec![0.3, 0.7]，soc_levels=vec![0.3, 0.7]（D7：修复蓝图 bounds bug）
- [x] C28 `new`：3×3=9 策略矩阵
- [x] C29 谷时（price < 0.3）：soc_final = Some(0.8)
- [x] C30 峰时（price >= 0.7）：soc_final = Some(0.3)
- [x] C31 平时：soc_final = None
- [x] C32 `get_config(&self, state: &RealtimeState) -> ScheduleConfig`：position + unwrap_or
- [x] C33 实现 `Default` for `StrategyTable`

## engine.rs — RealtimePathEngine
- [x] C34 `RealtimePathEngine<S: Solver>` 结构体（D2：泛型 Solver）
- [x] C35 字段：solver: S / default_config: ScheduleConfig / validator: SafetyValidator / strategy_table: StrategyTable
- [x] C36 `new(config: ScheduleConfig, solver: S) -> Self`
- [x] C37 `execute(&mut self, state: &RealtimeState, now_ms: u64) -> Result<FastPathResult, FastPathError>`
- [x] C38 `execute`：从 strategy_table.get_config 获取基础配置
- [x] C39 `execute`：微调 soc_init = state.system.soc_pct（D3：使用 soc_pct 而非 soc）
- [x] C40 `execute`：微调 load_demand = state.load_demand.clone()
- [x] C41 `execute`：编译 LP（D7：map_err 为 CompileError）
- [x] C42 `execute`：求解 LP（D5：solver.solve(&problem, now_ms)）
- [x] C43 `execute`：解析结果 model.parse_result(&solve_result)
- [x] C44 `execute`：安全校验 validator.validate(&schedule, &state.system)（D12：传 state.system）
- [x] C45 `execute`：返回 FastPathResult，schedule = validation.clamped_schedule.unwrap_or(schedule)
- [x] C46 `FastPathResult` 结构体：schedule / solve_result / validation / elapsed_ms / path_type，派生 `Debug + Clone`
- [x] C47 实现 `Default` for `RealtimePathEngine<MockSolver>`

## 集成测试（lib.rs）
- [x] C48 T1 PathType 枚举变体 + PartialEq
- [x] C49 T2 RealtimeState 构造
- [x] C50 T3 RealtimeState::default
- [x] C51 T4 PathSelector::new 默认阈值
- [x] C52 T5 PathSelector::select 首次走慢路径
- [x] C53 T6 PathSelector::select 间隔内走快路径
- [x] C54 T7 PathSelector::select 间隔超时走慢路径
- [x] C55 T8 PathSelector::select 电价变化超阈值走慢路径
- [x] C56 T9 PathSelector::select SOC 变化超阈值走慢路径
- [x] C57 T10 StrategyTable::new 3×3=9 策略
- [x] C58 T11 StrategyTable::get_config 谷时低 SOC
- [x] C59 T12 StrategyTable::get_config 峰时高 SOC
- [x] C60 T13 StrategyTable::get_config 平时中 SOC
- [x] C61 T14 StrategyTable::get_config 边界（price < 0.3）
- [x] C62 T15 RealtimePathEngine::new 构造
- [x] C63 T16 RealtimePathEngine::execute 端到端成功
- [x] C64 T17 RealtimePathEngine::execute 返回 FastPath
- [x] C65 T18 RealtimePathEngine::execute soc_init 来自 state
- [x] C66 T19 RealtimePathEngine::execute load_demand 传入
- [x] C67 T20 RealtimePathEngine::default 等价 new(default, MockSolver::new())
- [x] C68 T21 端到端：RealtimeState → PathSelector::select(FastPath) → Engine::execute
- [x] C69 T22 安全校验：FastPathResult.validation 包含 ValidationResult
- [x] C70 `cargo test -p eneros-fast-path` 全部通过

## 设计文档
- [x] C71 `docs/ai/fast-path-design.md` 存在
- [x] C72 12 章节完整
- [x] C73 2 Mermaid 图（快/慢路径选择流程图 + 快速路径执行时序图）
- [x] C74 D1~D12 偏差声明表
- [x] C75 文档在 `docs/ai/` 下

## 版本同步
- [x] C76 `Makefile` 版本号 `0.70.0`（header + VERSION 变量 2 处）
- [x] C77 `.github/workflows/ci.yml` 版本号 `0.70.0`
- [x] C78 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-fast-path`

## 构建校验（§2.4.2 C6~C11）
- [x] C79 `cargo metadata --format-version 1` 成功
- [x] C80 `cargo test -p eneros-fast-path` 全部通过
- [x] C81 `cargo build -p eneros-fast-path --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C82 `cargo fmt -p eneros-fast-path -- --check` 通过
- [x] C83 `cargo clippy -p eneros-fast-path --all-targets -- -D warnings` 无 warning
- [x] C84 `cargo deny check licenses bans sources` 通过

## no_std 合规
- [x] C85 无 `use std::*`（仅 `alloc::*` / `core::*`）
- [x] C86 无 `panic!` / `todo!` / `unimplemented!`
- [x] C87 子模块不重复 `#![cfg_attr(not(test), no_std)]`
- [x] C88 无 `unsafe` 块
- [x] C89 使用 `core::time::Duration`（D1：非 `std::time::Duration`）
- [x] C90 无 `Instant::now()`（D6：使用 `now_ms: u64` 参数）

## 目录规范
- [x] C91 crate 在 `crates/ai/fast-path/`
- [x] C92 跨 crate path 引用均为相对路径（`../solver-core` / `../energy-lp-model` / `../safety-validator`）
- [x] C93 文档在 `docs/ai/` 下
- [x] C94 无根目录 crate（除 `ci/`）
- [x] C95 无垃圾文件

## 依赖复用
- [x] C96 复用 v0.64.0 `Solver` trait / `MockSolver` / `LpProblem` / `SolveResult`（D2：泛型 `<S: Solver>`）
- [x] C97 复用 v0.66.0 `ScheduleConfig` / `EnergyScheduleModel` / `ScheduleResult`
- [x] C98 复用 v0.67.0 `SafetyValidator` / `SystemState` / `ValidationResult`（D4：RealtimeState 包装 SystemState）

## 简化设计验证（Karpathy 原则）
- [x] C99 `FastPathError` 不派生 Clone/PartialEq（D9：Simplicity First）
- [x] C100 无 `[features]` 段（纯 Rust）
