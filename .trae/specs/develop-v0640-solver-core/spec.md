# v0.64.0 LP 求解器集成（HiGHS via FFI）Spec

## Why

蓝图 v0.64.0 要求通过 C FFI 集成 HiGHS 线性规划求解器，为能源调度优化（v0.66.0）提供底层求解能力。但蓝图伪代码使用 `String`/`Vec`/`HashMap`/`Instant::now()` 等 std 类型，且隐含通过 `build.rs` 链接 HiGHS 静态库——在 no_std + 无 HiGHS 编译环境下 `cargo test` 会失败。需按 v0.59.0 Mock 默认 + FFI feature-gated 模式重构。

## What Changes

- 新增 `eneros-solver-core` crate（`crates/ai/solver-core/`）
- 定义 `Solver` trait（统一求解器抽象）+ `LpProblem`/`SolveResult`/`SolveStatus`/`SolverStatus`/`SolverError`/`VarType`/`ObjectiveSense`/`ConstraintMatrix` 类型
- 实现 `MockSolver`（默认可用，纯 Rust，无 FFI）
- 实现 `HighsSolver` + FFI 绑定（feature-gated `highs-ffi`，默认关闭）
- Workspace 同步：Cargo.toml 版本 `0.63.0` → `0.64.0` + 新增 member
- 版本同步：Makefile / ci.yml / gate.rs

## Impact

- Affected specs: 无前置依赖（独立基础层）；解锁 v0.65.0（建模 DSL）/ v0.66.0（能源 LP）/ v0.67.0（安全校验）/ v0.68.0（意图解析）
- Affected code:
  - `crates/ai/solver-core/`（新建 crate）
  - `Cargo.toml`（workspace version + members）
  - `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`（版本同步）
  - `docs/ai/solver-core-design.md`（新建设计文档）

## 偏差声明（D1~D12，Karpathy "Think Before Coding"）

| 偏差 | 蓝图原设计 | 实际实现 | 理由 |
|------|-----------|---------|------|
| **D1** | `String`/`Vec`/`HashMap`/`Instant::now()` 等 std 类型 | `alloc::*` 替代；删除 `Instant::now()`，`solve()` 方法签名增加 `now_ms: u64` 参数用于计算 `elapsed_ms`（参考 v0.57.0 `now_ns` 模式） | no_std 合规（蓝图 §43.1 硬性要求）；`Instant` 在 no_std 不可用 |
| **D2** | 隐含通过 `build.rs` 链接 HiGHS 静态库，默认即真实 FFI | `MockSolver` 默认可用；`HighsSolver` + `ffi` 模块通过 `#[cfg(feature = "highs-ffi")]` 门控；`Cargo.toml` 声明 `[features] highs-ffi = []`（默认关闭） | 无 HiGHS 编译环境下 `cargo test` 会失败；参考 v0.59.0 `MockEngine` 默认 + `LlamaCppEngine` feature-gated 模式 |
| **D3** | `params: HashMap<String, String>` 缓存已设参数 | 移除 params 缓存字段 | HiGHS 内部已存储参数；外部缓存重复状态，过度工程化（Karpathy Simplicity First） |
| **D4** | `SolverError::FfiError(String)` / `ParamError(String)` / `ParamSetFailed(String)` 使用 `String` | 保留 `String`（动态错误消息不可静态化）；但默认构建（Mock）下这些变体不可达，标 `#[allow(dead_code)]` | FFI 错误消息为运行时动态内容；feature-gated 路径才触发 |
| **D5** | `HighsSolver` 派生 Drop 析构 | 保留 `impl Drop for HighsSolver` 调用 `Highs_destroy`（feature-gated）；默认构建无 Drop 需求 | RAII 资源管理；仅 feature-gated 路径需要 |
| **D6** | `build.rs` HiGHS 静态库编译链接脚本 | 默认构建无 `build.rs`；`build.rs` 仅在 `highs-ffi` feature 启用时才需要（本版本暂不提供，留待真实集成时补充） | 默认构建保持 `cargo test` 快速且无外部依赖（Karpathy Simplicity First） |
| **D7** | Python `test_solver_ffi()` + valgrind 内存泄漏检测 | Rust `MockSolver` 单元测试 T1~T18；真实 HiGHS FFI 测试需 `highs-ffi` feature + 编译库，超出 v0.64.0 单元测试范围 | 项目为 Rust no_std；蓝图 §4.4 要求非瓶颈版本可用伪代码，但 trait/struct 签名必须可编译 |
| **D8** | `name: String` / `version: &str` 字段 | `name()` / `version()` 方法返回 `&'static str`（MockSolver="MockSolver"/"0.1.0"，HighsSolver="HiGHS"/"1.7.2"） | 避免 alloc；与 v0.59.0 `ModelInfo` 静态字段一致 |
| **D9** | 独立 crate | `crates/ai/solver-core/`（AI 子系统；项目规则 §2.3.1）；不依赖 v0.59.0~v0.63.0 任何 crate | Solver 是基础层；v0.68.0 意图解析将同时消费 Solver + PromptTemplate |
| **D10** | `unsafe` 块遍布 `HighsSolver` 各方法 | 所有 `unsafe` FFI 代码门控在 `highs-ffi` feature 下；默认构建（MockSolver）零 `unsafe`、零外部依赖 | 默认构建可 `cargo test` 无需任何 C 库 |
| **D11** | `ConstraintMatrix` 内联于 `LpProblem` | 独立 `ConstraintMatrix` 结构体（`num_rows`/`num_nz`/`row_start: Vec<i32>`/`col_index: Vec<i32>`/`values: Vec<f64>` CSR 格式） | 清晰的 CSR 格式封装；v0.65.0 DSL 编译目标 |
| **D12** | `SolveStatus` 派生 `PartialEq`（含 `Error(String)` 变体） | `alloc::string::String` 实现 `PartialEq`，可正常派生；保留 `Error(String)` 变体 | `String` 在 no_std 下 `PartialEq` 可用（alloc feature） |

## ADDED Requirements

### Requirement: Solver Trait 统一抽象

系统 SHALL 提供 `Solver` trait 作为所有 LP/MIP 求解器的统一抽象，定义以下方法：

- `solve(&mut self, problem: &LpProblem, now_ms: u64) -> Result<SolveResult, SolverError>` — 求解 LP 问题（D1：注入 `now_ms` 用于 `elapsed_ms` 计算）
- `name(&self) -> &'static str` — 求解器名称（D8：`&'static str` 避免 alloc）
- `version(&self) -> &'static str` — 求解器版本（D8）
- `set_param(&mut self, key: &str, value: &str) -> Result<(), SolverError>` — 设置求解器参数
- `status(&self) -> SolverStatus` — 获取求解器运行时状态

trait 不要求 `Send + Sync`（与 v0.59.0 `LlmEngine` 一致；HiGHS 对象非线程安全）。

#### Scenario: MockSolver 求解成功
- **WHEN** 调用 `MockSolver::new().solve(&problem, now_ms=1000)`
- **THEN** 返回 `Ok(SolveResult { status: SolveStatus::Optimal, objective_value: <preset>, solution: <preset>, elapsed_ms: 0, dual_solution: None })`

#### Scenario: MockSolver 自定义结果
- **WHEN** 调用 `MockSolver::with_result(result).solve(&problem, now_ms=2000)`
- **THEN** 返回 `Ok(result)`（预设的 result）

### Requirement: LpProblem 问题定义

系统 SHALL 提供 `LpProblem` 结构体表示线性规划问题：

- `variables: Vec<String>` — 变量名列表
- `lower_bounds: Vec<f64>` — 变量下界
- `upper_bounds: Vec<f64>` — 变量上界
- `var_types: Vec<VarType>` — 变量类型（Continuous/Integer/Binary）
- `objective: Vec<f64>` — 目标函数系数
- `sense: ObjectiveSense` — 目标方向（Minimize/Maximize）
- `constraints: ConstraintMatrix` — 约束矩阵（CSR 格式，D11）
- `rhs_lower: Vec<f64>` — 约束下界
- `rhs_upper: Vec<f64>` — 约束上界

`VarType` 枚举：`Continuous` / `Integer` / `Binary`（派生 Debug/Clone/Copy/PartialEq/Eq）。
`ObjectiveSense` 枚举：`Minimize` / `Maximize`（派生 Debug/Clone/Copy/PartialEq/Eq）。

#### Scenario: LpProblem 构造
- **WHEN** 构造 `LpProblem { variables: vec!["x".into(), "y".into()], ... }`
- **THEN** 所有字段可访问，长度一致

### Requirement: ConstraintMatrix CSR 格式

系统 SHALL 提供 `ConstraintMatrix` 结构体表示 CSR 格式约束矩阵（D11）：

- `num_rows: usize` — 约束行数
- `num_nz: usize` — 非零元素数
- `row_start: Vec<i32>` — 行起始索引（长度 = `num_rows + 1`）
- `col_index: Vec<i32>` — 列索引（长度 = `num_nz`）
- `values: Vec<f64>` — 非零值（长度 = `num_nz`）

#### Scenario: CSR 构造
- **WHEN** 构造 `ConstraintMatrix::new(num_rows=2, num_nz=4, row_start=vec![0,2,4], col_index=vec![0,1,0,1], values=vec![1.0,1.0,2.0,3.0])`
- **THEN** `matrix.num_rows == 2`，`matrix.row_start.len() == 3`

### Requirement: SolveResult + SolveStatus 求解结果

系统 SHALL 提供 `SolveResult` 结构体：

- `status: SolveStatus` — 求解状态
- `objective_value: f64` — 目标函数值
- `solution: Vec<f64>` — 变量解值
- `elapsed_ms: u64` — 求解耗时（由 `now_ms` 参数计算，D1）
- `dual_solution: Option<Vec<f64>>` — 对偶解（影子价格），MockSolver 返回 `None`

`SolveStatus` 枚举（派生 Debug/Clone/PartialEq，D12）：
- `Optimal` — 最优解
- `Suboptimal` — 次优解
- `Infeasible` — 不可行
- `Unbounded` — 无界
- `Timeout` — 超时
- `Error(String)` — 错误（含错误消息）

### Requirement: SolverStatus + SolverError 错误处理

系统 SHALL 提供 `SolverStatus` 枚举（运行时状态，区别于 `SolveStatus` 求解结果状态）：

- `Idle` — 空闲
- `Solving` — 求解中
- `Error` — 错误

派生 Debug/Clone/PartialEq。

`SolverError` 枚举（派生 Debug/Clone）：

- `FfiError(String)` — FFI 调用失败（D4，feature-gated 路径触发）
- `PassFailed(i32)` — 问题传入失败（返回码）
- `RunFailed(i32)` — 求解运行失败（返回码）
- `ParamError(String)` — 参数设置失败（CString 转换等）
- `ParamSetFailed(String)` — 参数设置失败（参数名）
- `InvalidProblem(String)` — 问题定义非法（变量数不一致等）
- `NotImplemented` — 功能未实现

实现 `core::fmt::Display`。默认构建下 `#[allow(dead_code)]`（Mock 路径不触发 FFI 错误变体）。

### Requirement: MockSolver 默认实现

系统 SHALL 提供 `MockSolver` 作为默认可用的 Solver 实现（D2/D10）：

- `MockSolver::new() -> Self` — 创建默认 Mock，返回 `SolveStatus::Optimal` + `objective_value=0.0` + `solution=vec![]`
- `MockSolver::with_result(result: SolveResult) -> Self` — 创建自定义 Mock，返回预设结果
- 实现 `Solver` trait：`name()="MockSolver"`，`version()="0.1.0"`，`status()=SolverStatus::Idle`，`set_param()` 返回 `Ok(())`
- `solve()` 返回预设结果（`elapsed_ms` 设为 0，忽略 `now_ms` 参数）
- 纯 Rust，零 `unsafe`，零外部依赖

#### Scenario: MockSolver 默认行为
- **WHEN** `MockSolver::new().name()`
- **THEN** 返回 `"MockSolver"`

### Requirement: HighsSolver FFI 实现（feature-gated）

系统 SHALL 在 `highs-ffi` feature 启用时提供 `HighsSolver`（D2/D10）：

- `HighsSolver::new() -> Result<Self, SolverError>` — 调用 `Highs_create()` FFI
- `impl Drop for HighsSolver` — 调用 `Highs_destroy()` FFI（D5）
- 实现 `Solver` trait — 调用 `Highs_passLp` / `Highs_run` / `Highs_getSolution` FFI
- FFI 绑定模块 `ffi`：`Highs_create` / `Highs_destroy` / `Highs_passLp` / `Highs_run` / `Highs_getModelStatus` / `Highs_getObjectiveValue` / `Highs_getSolution` / `Highs_setStringOptionValue` / `Highs_setDoubleOptionValue`
- 所有 `unsafe` 块附 SAFETY 注释（参考 v0.59.0 D10 模式）

**默认关闭**：`Cargo.toml` 声明 `[features] default = [] highs-ffi = []`。

## MODIFIED Requirements

### Requirement: Workspace 成员与版本

根 `Cargo.toml` 的 `[workspace.package] version` 从 `0.63.0` 更新为 `0.64.0`；`members` 列表在 `"crates/ai/prompt-template"` 之后添加 `"crates/ai/solver-core"`。

## REMOVED Requirements

### Requirement: params 缓存字段
**Reason**: 蓝图 `HighsSolver.params: HashMap<String, String>` 重复 HiGHS 内部状态，过度工程化（Karpathy Simplicity First）。
**Migration**: 无需迁移；如未来需要参数查询，HiGHS 提供 `Highs_getStringOptionValue` FFI 可调用。

### Requirement: build.rs HiGHS 静态库链接脚本
**Reason**: 默认构建（Mock-only）无需链接 HiGHS；`build.rs` 仅在 `highs-ffi` feature 启用时才需要，本版本暂不提供（留待真实集成时补充）。
**Migration**: 启用 `highs-ffi` feature 时需用户自行提供 HiGHS 静态库路径（通过环境变量或后续版本补充 `build.rs`）。
