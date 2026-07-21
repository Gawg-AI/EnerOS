# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.64.0`
- [x] C2 members 列表已添加 `crates/ai/solver-core`（置于 `crates/ai/prompt-template` 之后）
- [x] C3 `cargo metadata --format-version 1` 成功

## Crate 骨架
- [x] C4 `crates/ai/solver-core/Cargo.toml` 存在，package name = `eneros-solver-core`
- [x] C5 dependencies 为空（无外部依赖，纯 no_std，D9）
- [x] C6 声明 `[features] default = [] highs-ffi = []`（D2/D10）
- [x] C7 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C8 `src/lib.rs` 包含 D1~D12 偏差声明表
- [x] C9 默认模块声明：error / problem / result / solver / mock
- [x] C10 feature-gated 模块声明：`#[cfg(feature = "highs-ffi")] pub mod ffi;` + `#[cfg(feature = "highs-ffi")] pub mod highs;`

## error.rs — SolverError
- [x] C11 `SolverError` 枚举包含 7 变体（FfiError(String) / PassFailed(i32) / RunFailed(i32) / ParamError(String) / ParamSetFailed(String) / InvalidProblem(String) / NotImplemented）
- [x] C12 派生 `Debug` + `Clone`
- [x] C13 实现 `core::fmt::Display`
- [x] C14 默认构建下 `#[allow(dead_code)]`（Mock 路径不触发 FFI 错误变体，D4）

## problem.rs — LpProblem + VarType + ObjectiveSense + ConstraintMatrix
- [x] C15 `VarType` 枚举：Continuous / Integer / Binary（派生 Debug/Clone/Copy/PartialEq/Eq）
- [x] C16 `ObjectiveSense` 枚举：Minimize / Maximize（派生 Debug/Clone/Copy/PartialEq/Eq）
- [x] C17 `ConstraintMatrix` 结构体：num_rows / num_nz / row_start / col_index / values（D11，CSR 格式）
- [x] C18 `ConstraintMatrix::new(num_rows, num_nz, row_start, col_index, values) -> Self`
- [x] C19 `LpProblem` 结构体：variables / lower_bounds / upper_bounds / var_types / objective / sense / constraints / rhs_lower / rhs_upper（D1，alloc::vec::Vec + alloc::string::String）
- [x] C20 派生 `Debug` + `Clone`

## result.rs — SolveResult + SolveStatus
- [x] C21 `SolveStatus` 枚举：Optimal / Suboptimal / Infeasible / Unbounded / Timeout / Error(alloc::string::String)（D12，派生 Debug/Clone/PartialEq）
- [x] C22 `SolveResult` 结构体：status / objective_value / solution / elapsed_ms / dual_solution
- [x] C23 派生 `Debug` + `Clone`
- [x] C24 `SolveResult::optimal(objective_value, solution) -> Self` 便捷构造

## solver.rs — Solver trait + SolverStatus
- [x] C25 `SolverStatus` 枚举：Idle / Solving / Error（派生 Debug/Clone/PartialEq）
- [x] C26 `Solver` trait 无 Send + Sync（与 v0.59.0 LlmEngine 一致）
- [x] C27 `solve(&mut self, problem: &LpProblem, now_ms: u64) -> Result<SolveResult, SolverError>`（D1：注入 now_ms）
- [x] C28 `name(&self) -> &'static str` + `version(&self) -> &'static str`（D8：&'static str）
- [x] C29 `set_param(&mut self, key: &str, value: &str) -> Result<(), SolverError>`
- [x] C30 `status(&self) -> SolverStatus`
- [x] C31 trait 编译通过

## mock.rs — MockSolver
- [x] C32 `MockSolver` 结构体（无 params 缓存，D3）
- [x] C33 `MockSolver::new() -> Self`（默认返回 Optimal + objective=0.0 + solution=vec![]）
- [x] C34 `MockSolver::with_result(result: SolveResult) -> Self`
- [x] C35 `impl Solver for MockSolver`：name()="MockSolver" / version()="0.1.0" / status()=Idle / set_param()=Ok(()) / solve()返回 preset_result
- [x] C36 纯 Rust，零 `unsafe`，零外部依赖（D10）

## ffi.rs — HiGHS FFI 绑定（feature-gated）
- [x] C37 `HighsPtr = *mut core::ffi::c_void` 类型别名
- [x] C38 `extern "C"` 声明 9 个函数（Highs_create / Highs_destroy / Highs_passLp / Highs_run / Highs_getModelStatus / Highs_getObjectiveValue / Highs_getSolution / Highs_setStringOptionValue / Highs_setDoubleOptionValue）
- [x] C39 模块整体 `#[cfg(feature = "highs-ffi")]`（D2/D10）
- [x] C40 默认构建（`cargo build`）不编译 ffi 模块

## highs.rs — HighsSolver（feature-gated）
- [x] C41 `HighsSolver` 结构体：handle: core::ptr::NonNull<core::ffi::c_void> / status: SolverStatus
- [x] C42 `HighsSolver::new() -> Result<Self, SolverError>`（调用 ffi::Highs_create）
- [x] C43 `impl Drop for HighsSolver`（调用 ffi::Highs_destroy，D5）
- [x] C44 `impl Solver for HighsSolver`（调用 ffi::Highs_passLp / Highs_run / Highs_getSolution；elapsed_ms 通过 now_ms 计算）
- [x] C45 `HighsSolver::set_time_limit(&mut self, seconds: f64) -> Result<(), SolverError>`
- [x] C46 `HighsSolver::set_method(&mut self, method: &str) -> Result<(), SolverError>`
- [x] C47 `HighsSolver::map_status(status: i32) -> SolveStatus` 私有方法
- [x] C48 所有 `unsafe` 块附 SAFETY 注释（参考 v0.59.0 D10 模式）
- [x] C49 CString 使用 `alloc::ffi::CString`（Rust 1.64+ 稳定）
- [x] C50 默认构建（`cargo build`）不编译 highs 模块

## 集成测试（lib.rs）
- [x] C51 T1 LpProblem 构造 + 字段访问
- [x] C52 T2 VarType 枚举变体
- [x] C53 T3 ObjectiveSense 枚举变体
- [x] C54 T4 ConstraintMatrix CSR 构造
- [x] C55 T5 SolveStatus Optimal/Infeasible 变体
- [x] C56 T6 SolveStatus Error(String) 变体 + PartialEq
- [x] C57 T7 SolveResult::optimal 便捷构造
- [x] C58 T8 SolverStatus Idle/Solving 变体
- [x] C59 T9 SolverError PassFailed(i32) 变体
- [x] C60 T10 SolverError InvalidProblem(String) 变体 + Display
- [x] C61 T11 MockSolver::new() + name()="MockSolver" + version()="0.1.0"
- [x] C62 T12 MockSolver::new().status()=Idle
- [x] C63 T13 MockSolver::new().set_param("key","val")=Ok(())
- [x] C64 T14 MockSolver::new().solve(&problem, now_ms=1000) 返回 Optimal
- [x] C65 T15 MockSolver::with_result(custom).solve(&problem, now_ms=2000) 返回自定义结果
- [x] C66 T16 dyn Solver trait object 使用（`&dyn Solver`）
- [x] C67 T17 MockSolver 多次调用 solve() 返回一致结果
- [x] C68 T18 LpProblem 全字段构造 + ConstraintMatrix 嵌入
- [x] C69 `cargo test -p eneros-solver-core` 18/18 通过

## 设计文档
- [x] C70 `docs/ai/solver-core-design.md` 存在
- [x] C71 12 章节完整
- [x] C72 2 Mermaid 图（Solver trait 类图 + solve() 时序图）
- [x] C73 D1~D12 偏差声明表
- [x] C74 文档在 `docs/ai/` 下（符合目录规范）

## 版本同步
- [x] C75 `Makefile` 版本号 `0.64.0`
- [x] C76 `.github/workflows/ci.yml` 版本号 `0.64.0`
- [x] C77 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-solver-core`

## 构建校验（§2.4.2 C6~C11）
- [x] C78 `cargo metadata --format-version 1` 成功
- [x] C79 `cargo test -p eneros-solver-core` 全部通过（18 tests）
- [x] C80 `cargo build -p eneros-solver-core --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C81 `cargo fmt -p eneros-solver-core -- --check` 通过
- [x] C82 `cargo clippy -p eneros-solver-core --all-targets -- -D warnings` 无 warning
- [x] C83 `cargo deny check licenses bans sources` 通过

## no_std 合规
- [x] C84 无 `use std::*`（仅 `alloc::*` / `core::*`）
- [x] C85 无 `panic!` / `todo!` / `unimplemented!`
- [x] C86 子模块不重复 `#![cfg_attr(not(test), no_std)]`
- [x] C87 默认构建无 `unsafe` 块（D10：所有 unsafe 在 highs-ffi feature 下）
- [x] C88 无 `HashMap`（D3：移除 params 缓存）
- [x] C89 无 `Instant::now()`（D1：now_ms 注入）

## 目录规范
- [x] C90 crate 在 `crates/ai/solver-core/`（D9）
- [x] C91 跨 crate path 引用：无（D9：独立 crate，无外部依赖）
- [x] C92 文档在 `docs/ai/` 下
- [x] C93 无根目录 crate（除 `ci/`）
- [x] C94 无垃圾文件（`target/` / `*.elf` / `*.bin` 被忽略）

## 依赖与解耦（D9）
- [x] C95 不依赖 v0.59.0（llm-engine）/ v0.60.0（gguf-loader）/ v0.61.0（model-deploy）/ v0.62.0（infer-scheduler）/ v0.63.0（prompt-template）
- [x] C96 Solver 子系统独立基础层，为 v0.65.0~v0.68.0 提供求解能力

## 简化设计验证（Karpathy 原则）
- [x] C97 无 `params: HashMap<String, String>` 缓存（D3：HiGHS 内部已存储，不重复）
- [x] C98 无 `build.rs`（D6：默认构建无需链接 HiGHS，留待真实集成时补充）
- [x] C99 无 Python 测试代码（D7：Rust MockSolver 单元测试）
- [x] C100 无 `Send + Sync` bounds（与 v0.59.0 LlmEngine 一致；HiGHS 对象非线程安全）
