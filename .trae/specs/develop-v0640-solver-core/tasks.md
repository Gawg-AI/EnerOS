# Tasks

- [x] Task 1: Workspace 同步
  - [x] 根 `Cargo.toml` 版本号 `0.63.0` → `0.64.0`
  - [x] members 添加 `crates/ai/solver-core`（置于 `crates/ai/prompt-template` 之后）
  - [x] 验证：`cargo metadata --format-version 1` 成功（待 crate 骨架创建后）

- [x] Task 2: 创建 `eneros-solver-core` crate 骨架
  - [x] 新建 `crates/ai/solver-core/Cargo.toml`，package name = `eneros-solver-core`
  - [x] dependencies：空（无外部依赖，纯 no_std）
  - [x] 声明 `[features] default = []` + `highs-ffi = []`（D2/D10）
  - [x] no_std 配置：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
  - [x] 新建 `src/lib.rs`，模块声明：error / problem / result / solver / mock
  - [x] feature-gated 模块声明：`#[cfg(feature = "highs-ffi")] pub mod ffi;` + `#[cfg(feature = "highs-ffi")] pub mod highs;`
  - [x] lib.rs 包含 D1~D12 偏差声明表
  - [x] 验证：`cargo metadata --format-version 1` 成功

- [x] Task 3: 实现 `error.rs` — SolverError 错误类型
  - [x] `SolverError` 枚举：FfiError(String) / PassFailed(i32) / RunFailed(i32) / ParamError(String) / ParamSetFailed(String) / InvalidProblem(String) / NotImplemented
  - [x] 派生 `Debug` + `Clone`
  - [x] 实现 `core::fmt::Display`
  - [x] 默认构建下 `#[allow(dead_code)]`（Mock 路径不触发 FFI 错误变体，D4）
  - [x] 验证：`cargo build -p eneros-solver-core` 通过

- [x] Task 4: 实现 `problem.rs` — LpProblem + VarType + ObjectiveSense + ConstraintMatrix
  - [x] `VarType` 枚举：Continuous / Integer / Binary（派生 Debug/Clone/Copy/PartialEq/Eq）
  - [x] `ObjectiveSense` 枚举：Minimize / Maximize（派生 Debug/Clone/Copy/PartialEq/Eq）
  - [x] `ConstraintMatrix` 结构体：num_rows: usize / num_nz: usize / row_start: Vec<i32> / col_index: Vec<i32> / values: Vec<f64>（D11，CSR 格式）
  - [x] `ConstraintMatrix::new(num_rows, num_nz, row_start, col_index, values) -> Self`
  - [x] `LpProblem` 结构体：variables / lower_bounds / upper_bounds / var_types / objective / sense / constraints / rhs_lower / rhs_upper（全 Vec 用 alloc::vec::Vec，String 用 alloc::string::String，D1）
  - [x] 派生 `Debug` + `Clone`
  - [x] 验证：编译通过

- [x] Task 5: 实现 `result.rs` — SolveResult + SolveStatus
  - [x] `SolveStatus` 枚举：Optimal / Suboptimal / Infeasible / Unbounded / Timeout / Error(alloc::string::String)（D12，派生 Debug/Clone/PartialEq）
  - [x] `SolveResult` 结构体：status: SolveStatus / objective_value: f64 / solution: Vec<f64> / elapsed_ms: u64 / dual_solution: Option<Vec<f64>>
  - [x] 派生 `Debug` + `Clone`
  - [x] `SolveResult::optimal(objective_value, solution) -> Self`（便捷构造）
  - [x] 验证：编译通过

- [x] Task 6: 实现 `solver.rs` — Solver trait + SolverStatus
  - [x] `SolverStatus` 枚举：Idle / Solving / Error（派生 Debug/Clone/PartialEq）
  - [x] `Solver` trait（无 Send + Sync，与 v0.59.0 LlmEngine 一致）：`solve(&mut self, problem: &LpProblem, now_ms: u64) -> Result<SolveResult, SolverError>`（D1：注入 now_ms）/ `name(&self) -> &'static str`（D8）/ `version(&self) -> &'static str`（D8）/ `set_param(&mut self, key: &str, value: &str) -> Result<(), SolverError>` / `status(&self) -> SolverStatus`
  - [x] 验证：编译通过

- [x] Task 7: 实现 `mock.rs` — MockSolver（默认可用，D2/D10）
  - [x] `MockSolver` 结构体：preset_result: SolveResult（无 params 缓存，D3）
  - [x] `MockSolver::new() -> Self`（默认返回 Optimal + objective=0.0 + solution=vec![]）
  - [x] `MockSolver::with_result(result: SolveResult) -> Self`
  - [x] `impl Solver for MockSolver`：name()="MockSolver"，version()="0.1.0"，status()=Idle，set_param()=Ok(())，solve()返回 preset_result（elapsed_ms=0，忽略 now_ms）
  - [x] 纯 Rust，零 `unsafe`，零外部依赖
  - [x] 验证：编译通过

- [x] Task 8: 实现 `ffi.rs` — HiGHS C API FFI 绑定（feature-gated `highs-ffi`，D2/D10）
  - [x] `HighsPtr = *mut core::ffi::c_void` 类型别名
  - [x] `extern "C"` 声明：Highs_create / Highs_destroy / Highs_passLp / Highs_run / Highs_getModelStatus / Highs_getObjectiveValue / Highs_getSolution / Highs_setStringOptionValue / Highs_setDoubleOptionValue
  - [x] 模块整体 `#[cfg(feature = "highs-ffi")]`
  - [x] 验证：`cargo build -p eneros-solver-core`（默认）不编译 ffi 模块

- [x] Task 9: 实现 `highs.rs` — HighsSolver（feature-gated `highs-ffi`，D2/D5/D10）
  - [x] `HighsSolver` 结构体：handle: core::ptr::NonNull<core::ffi::c_void> / status: SolverStatus
  - [x] `HighsSolver::new() -> Result<Self, SolverError>`（调用 ffi::Highs_create）
  - [x] `impl Drop for HighsSolver`（调用 ffi::Highs_destroy，D5）
  - [x] `impl Solver for HighsSolver`（调用 ffi::Highs_passLp / Highs_run / Highs_getSolution；elapsed_ms 通过 now_ms 参数计算）
  - [x] `HighsSolver::set_time_limit(&mut self, seconds: f64) -> Result<(), SolverError>`
  - [x] `HighsSolver::set_method(&mut self, method: &str) -> Result<(), SolverError>`
  - [x] `HighsSolver::map_status(status: i32) -> SolveStatus`（私有方法，HiGHS 状态码映射）
  - [x] 所有 `unsafe` 块附 SAFETY 注释（参考 v0.59.0 D10 模式）
  - [x] CString 使用 `alloc::ffi::CString`（Rust 1.64+ 稳定）
  - [x] 验证：`cargo build -p eneros-solver-core`（默认）不编译 highs 模块

- [x] Task 10: 集成测试模块（lib.rs `#[cfg(test)] mod tests`）
  - [x] T1 LpProblem 构造 + 字段访问
  - [x] T2 VarType 枚举变体
  - [x] T3 ObjectiveSense 枚举变体
  - [x] T4 ConstraintMatrix CSR 构造
  - [x] T5 SolveStatus Optimal/Infeasible 变体
  - [x] T6 SolveStatus Error(String) 变体 + PartialEq
  - [x] T7 SolveResult::optimal 便捷构造
  - [x] T8 SolverStatus Idle/Solving 变体
  - [x] T9 SolverError PassFailed(i32) 变体
  - [x] T10 SolverError InvalidProblem(String) 变体 + Display
  - [x] T11 MockSolver::new() + name()="MockSolver" + version()="0.1.0"
  - [x] T12 MockSolver::new().status()=Idle
  - [x] T13 MockSolver::new().set_param("key","val")=Ok(())
  - [x] T14 MockSolver::new().solve(&problem, now_ms=1000) 返回 Optimal
  - [x] T15 MockSolver::with_result(custom).solve(&problem, now_ms=2000) 返回自定义结果
  - [x] T16 dyn Solver trait object 使用（`&dyn Solver`）
  - [x] T17 MockSolver 多次调用 solve() 返回一致结果
  - [x] T18 LpProblem 全字段构造 + ConstraintMatrix 嵌入
  - [x] 验证：`cargo test -p eneros-solver-core` 全部通过（18/18）

- [x] Task 11: 设计文档 `docs/ai/solver-core-design.md`
  - [x] 12 章节：版本目标 / 架构定位 / Solver trait / LpProblem + ConstraintMatrix / SolveResult + SolveStatus / MockSolver 默认实现 / HighsSolver FFI 实现（feature-gated）/ 错误处理 / no_std 合规 / GPU 策略（Solver 不涉及 GPU，CPU 求解）/ 内存预算 / 偏差声明
  - [x] 2 Mermaid 图：Solver trait 类图 + solve() 时序图
  - [x] D1~D12 偏差声明表
  - [x] 文档位置在 `docs/ai/` 下（复用 v0.59.0~v0.63.0 创建的目录）
  - [x] 文档总行数：1663 行

- [x] Task 12: 版本号同步 + gate.rs 注释更新
  - [x] `Makefile` 版本号 `0.63.0` → `0.64.0`（header + VERSION 变量）
  - [x] `.github/workflows/ci.yml` 版本号 `0.63.0` → `0.64.0`
  - [x] `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-solver-core` 说明（standalone no_std crate，无 feature-gated 需求）
  - [x] 验证：`cargo build -p eneros-ci` 通过

- [x] Task 13: 构建校验（§2.4.2 C6~C11）
  - [x] `cargo metadata --format-version 1` 成功
  - [x] `cargo test -p eneros-solver-core` 全部通过（18 tests）
  - [x] `cargo build -p eneros-solver-core --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
  - [x] `cargo fmt -p eneros-solver-core -- --check` 格式通过
  - [x] `cargo clippy -p eneros-solver-core --all-targets -- -D warnings` lint 通过
  - [x] `cargo deny check licenses bans sources` 安全扫描通过

- [x] Task 14: 更新 tasks.md + checklist.md 所有项 → [x]
  - [x] tasks.md 14 任务全部 [x]
  - [x] checklist.md 所有检查点全部 [x]

# Task Dependencies

- Task 2（crate 骨架）→ Task 1（metadata 验证需骨架）
- Task 3（error）→ Task 4~9（各模块使用 SolverError）
- Task 4（problem）→ Task 6（solver trait 使用 LpProblem）
- Task 5（result）→ Task 6（solver trait 使用 SolveResult）
- Task 6（solver trait）→ Task 7（MockSolver 实现 trait）+ Task 9（HighsSolver 实现 trait）
- Task 7（mock）→ Task 10（测试使用 MockSolver）
- Task 8（ffi）→ Task 9（highs 使用 ffi）
- Task 9（highs）→ 独立验证（feature-gated，默认不编译）
- Task 10（集成测试）→ Task 3~7（测试依赖所有默认模块）
- Task 11（设计文档）可与 Task 7~9 并行（独立工作）
- Task 12（版本同步）→ Task 11（版本同步在功能完成后）
- Task 13（构建校验）→ Task 12
- Task 14（更新文档）→ Task 13（全部校验通过后）

# Parallelizable Work

- Task 3（error）+ Task 4（problem）+ Task 5（result）可并行（无相互依赖）
- Task 6（solver trait）依赖 Task 3 + Task 4 + Task 5
- Task 7（mock）依赖 Task 6
- Task 8（ffi）+ Task 9（highs）可并行（但均 feature-gated，默认不编译）
- Task 10（集成测试）依赖 Task 3~7
- Task 11（设计文档）可与 Task 7~10 并行（独立工作）
