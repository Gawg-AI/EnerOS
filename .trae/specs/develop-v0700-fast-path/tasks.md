# Tasks

- [x] Task 1: Workspace 同步
  - [x] 根 `Cargo.toml` 版本号 `0.69.0` → `0.70.0`
  - [x] members 添加 `crates/ai/fast-path`（置于 `crates/ai/intent-contract` 之后）
  - [x] 验证：`cargo metadata --format-version 1` 成功

- [x] Task 2: 创建 `eneros-fast-path` crate 骨架
  - [x] 新建 `crates/ai/fast-path/Cargo.toml`，package name = `eneros-fast-path`
  - [x] dependencies：`eneros-solver-core` / `eneros-energy-lp-model` / `eneros-safety-validator`
  - [x] 无 `[features]` 段（D9：纯 Rust，无 FFI）
  - [x] no_std 配置：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
  - [x] 新建 `src/lib.rs`，模块声明：error / state / selector / strategy / engine
  - [x] lib.rs 包含 D1~D12 偏差声明表

- [x] Task 3: 实现 `error.rs` — FastPathError
  - [x] `FastPathError` 枚举：`CompileError(String)` / `SolveError(String)`
  - [x] 派生 `Debug`（D9：不派生 Clone/PartialEq）
  - [x] 使用 `alloc::string::String`

- [x] Task 4: 实现 `state.rs` — RealtimeState
  - [x] `RealtimeState` 结构体：`system: SystemState`（D4：包装 v0.67.0）/ `current_price: f64` / `load_demand: Option<Vec<f64>>`
  - [x] 派生 `Debug + Clone`（D8）
  - [x] 实现 `Default`（current_price=0.5, load_demand=None, system=SystemState::default()）

- [x] Task 5: 实现 `selector.rs` — PathSelector
  - [x] `PathType` 枚举：`SlowPath` / `FastPath`，派生 `Debug + Clone + PartialEq`（D8）
  - [x] `PathSelector` 结构体：price_change_threshold / soc_change_threshold / load_change_threshold / last_slow_path_ms: Option<u64>（D10）/ min_slow_path_interval: core::time::Duration（D11）/ last_state: Option<RealtimeState>
  - [x] `new()`：默认阈值（0.1 / 5.0 / 20.0 / None / 300s / None）
  - [x] `select(&mut self, state: &RealtimeState, now_ms: u64) -> PathType`：5 步选择逻辑
  - [x] 实现 `Default`

- [x] Task 6: 实现 `strategy.rs` — StrategyTable
  - [x] `StrategyTable` 结构体：price_levels: Vec<f64> / soc_levels: Vec<f64> / strategies: Vec<Vec<ScheduleConfig>>
  - [x] `new(default: ScheduleConfig) -> Self`：price_levels=vec![0.3, 0.7]，soc_levels=vec![0.3, 0.7]，3×3=9 策略（D7：修复蓝图 bounds bug）
    - 谷时（price < 0.3）：soc_final = Some(0.8)
    - 峰时（price >= 0.7）：soc_final = Some(0.3)
    - 平时：soc_final = None
  - [x] `get_config(&self, state: &RealtimeState) -> ScheduleConfig`：position + unwrap_or
  - [x] 实现 `Default`

- [x] Task 7: 实现 `engine.rs` — RealtimePathEngine
  - [x] `RealtimePathEngine<S: Solver>` 结构体（D2：泛型 Solver）：solver / default_config / validator / strategy_table
  - [x] `new(config: ScheduleConfig, solver: S) -> Self`
  - [x] `execute(&mut self, state: &RealtimeState, now_ms: u64) -> Result<FastPathResult, FastPathError>`：7 步执行
    - 查表 → 微调 → 编译（D7：map_err 为 CompileError）→ 求解（D5：solver.solve）→ 解析 → 校验（D12：传 state.system）→ 返回
  - [x] `FastPathResult` 结构体：schedule / solve_result / validation / elapsed_ms / path_type，派生 `Debug + Clone`
  - [x] 实现 `Default` for `RealtimePathEngine<MockSolver>`

- [x] Task 8: 集成测试（lib.rs）— 至少 20 个测试
  - [x] T1 PathType 枚举变体 + PartialEq
  - [x] T2 RealtimeState 构造
  - [x] T3 RealtimeState::default
  - [x] T4 PathSelector::new 默认阈值
  - [x] T5 PathSelector::select 首次走慢路径
  - [x] T6 PathSelector::select 间隔内走快路径
  - [x] T7 PathSelector::select 间隔超时走慢路径
  - [x] T8 PathSelector::select 电价变化超阈值走慢路径
  - [x] T9 PathSelector::select SOC 变化超阈值走慢路径
  - [x] T10 StrategyTable::new 3×3=9 策略
  - [x] T11 StrategyTable::get_config 谷时低 SOC
  - [x] T12 StrategyTable::get_config 峰时高 SOC
  - [x] T13 StrategyTable::get_config 平时中 SOC
  - [x] T14 StrategyTable::get_config 边界（price < 0.3）
  - [x] T15 RealtimePathEngine::new 构造
  - [x] T16 RealtimePathEngine::execute 端到端成功（使用 MockSolver）
  - [x] T17 RealtimePathEngine::execute 返回 FastPath
  - [x] T18 RealtimePathEngine::execute soc_init 来自 state
  - [x] T19 RealtimePathEngine::execute load_demand 传入
  - [x] T20 RealtimePathEngine::default 等价 new(ScheduleConfig::default(), MockSolver::new())
  - [x] T21 端到端：RealtimeState → PathSelector::select(FastPath) → Engine::execute
  - [x] T22 安全校验：FastPathResult.validation 包含 ValidationResult

- [x] Task 9: 创建设计文档 `docs/ai/fast-path-design.md`
  - [x] 12 章节完整（版本目标 / 前置依赖 / 交付物 / 详细设计 / 技术交底 / 测试计划 / 验收标准 / 风险 / 多角度要求 / ADR / 偏差声明 / 参考）
  - [x] 2 Mermaid 图（快/慢路径选择流程图 + 快速路径执行时序图）
  - [x] D1~D12 偏差声明表
  - [x] 文档位于 `docs/ai/` 下（C12）

- [x] Task 10: 版本同步
  - [x] `Makefile` 版本号 `0.70.0`（header + VERSION 变量 2 处）
  - [x] `.github/workflows/ci.yml` 版本号 `0.70.0`
  - [x] `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-fast-path`

- [x] Task 11: 6 项构建校验（§2.4.2 C6~C11）
  - [x] `cargo metadata --format-version 1` 成功
  - [x] `cargo test -p eneros-fast-path` 全部通过
  - [x] `cargo build -p eneros-fast-path --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] `cargo fmt -p eneros-fast-path -- --check` 通过
  - [x] `cargo clippy -p eneros-fast-path --all-targets -- -D warnings` 无 warning
  - [x] `cargo deny check licenses bans sources` 通过
  - [x] 更新 tasks.md / checklist.md 全部 [x]

# Task Dependencies
- Task 2 依赖 Task 1
- Task 3~7 依赖 Task 2（并行实现）
- Task 8 依赖 Task 3~7
- Task 9 可与 Task 3~8 并行
- Task 10 依赖 Task 2
- Task 11 依赖 Task 3~10 全部完成
