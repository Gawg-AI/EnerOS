# v0.65.0 优化问题建模框架 Spec

## Why

蓝图 v0.65.0 要求构建优化问题建模 DSL（变量/线性表达式/约束/目标的 Rust DSL + Builder 模式），编译为 v0.64.0 的 `LpProblem` 矩阵格式。但蓝图伪代码使用 `std::ops::Add/Sub/Mul`（no_std 应为 `core::ops`）、`HashMap`（no_std alloc 无 HashMap，应用 `BTreeMap`）、重复定义 `VarType`（v0.64.0 已定义于 `problem.rs`，应复用）。需按 no_std 合规 + 类型复用原则重构。

## What Changes

- 新增 `eneros-solver-model` crate（`crates/ai/solver-model/`）
- 定义 `Variable` + `VarBuilder`（变量 + 链式构建器）
- 定义 `LinearExpr`（线性表达式，BTreeMap 替代 HashMap，D2）
- 定义 `Constraint` 枚举（Le/Ge/Eq/Range）
- 定义 `OptProblem`（优化问题容器 + Builder + `compile()` 编译器）
- 运算符重载 `core::ops::{Add, Sub, Mul<f64>}`（D1：no_std 用 core::ops）
- 复用 v0.64.0 的 `VarType`/`ObjectiveSense`/`LpProblem`/`ConstraintMatrix`/`SolverError`（D3）
- Workspace 同步：Cargo.toml 版本 `0.64.0` → `0.65.0` + 新增 member
- 版本同步：Makefile / ci.yml / gate.rs

## Impact

- Affected specs: v0.64.0（`eneros-solver-core` 提供编译目标类型）；解锁 v0.66.0（能源调度 LP 模型）/ v0.67.0（安全校验器）/ v0.68.0（意图解析）
- Affected code:
  - `crates/ai/solver-model/`（新建 crate）
  - `Cargo.toml`（workspace version + members）
  - `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`（版本同步）
  - `docs/ai/solver-model-design.md`（新建设计文档）

## 偏差声明（D1~D12，Karpathy "Think Before Coding"）

| 偏差 | 蓝图原设计 | 实际实现 | 理由 |
|------|-----------|---------|------|
| **D1** | `std::ops::Add/Sub/Mul` 运算符重载 | `core::ops::Add/Sub/Mul`（no_std 兼容） | 蓝图 §43.1 no_std 硬性要求；`core::ops` 与 `std::ops` trait 路径一致，仅 `use` 路径不同 |
| **D2** | `LinearExpr.terms: HashMap<usize, f64>` + `OptProblem.var_map: HashMap<String, usize>` | `BTreeMap<usize, f64>` + `BTreeMap<String, usize>` | `alloc::collections::BTreeMap` 是 no_std 可用的有序 map；`HashMap` 需 `std::collections::HashMap` 或第三方 no_std crate（如 `hashbrown`），引入依赖过度（Karpathy Simplicity First）；BTreeMap 遍历顺序确定，更利于确定性编译（蓝图 §8.3 提及 HashMap 遍历顺序不确定的坑点，BTreeMap 天然解决） |
| **D3** | 重新定义 `VarType` 枚举（Continuous/Integer/Binary） | 复用 v0.64.0 `eneros_solver_core::problem::VarType` | DRY 原则；v0.64.0 已定义且派生 Debug/Clone/Copy/PartialEq/Eq；重复定义会导致 `compile()` 返回的 `LpProblem.var_types: Vec<VarType>` 类型不匹配 |
| **D4** | `Variable.index: Option<usize>` 字段 | 保留 `Option<usize>`（编译前 None，编译后 Some(idx)） | 与蓝图一致；语义清晰 |
| **D5** | `f64::INFINITY` / `f64::NEG_INFINITY` 作为默认上下界 | 保留 `f64::INFINITY` / `f64::NEG_INFINITY`（`core::f64` 常量，no_std 可用） | `core::f64::INFINITY` 在 no_std 下可用；无需偏差 |
| **D6** | `VarBuilder` 链式方法返回 `Self` | 保留链式 Builder 模式（`lower()`/`upper()`/`range()`/`non_negative()`/`integer()`/`binary()`/`build()`） | 与蓝图一致；链式调用是 DSL 的核心表达力 |
| **D7** | 测试计划包含 "DSL 建模 + HiGHS 求解端到端验证" | 使用 `MockSolver`（v0.64.0）进行端到端验证（编译 → MockSolver.solve → 返回 Optimal）；真实 HiGHS FFI 测试需 `highs-ffi` feature + 编译库，超出 v0.65.0 单元测试范围 | v0.64.0 默认构建无 HiGHS；MockSolver 已实现 `Solver` trait，足以验证 DSL→矩阵→求解的端到端流程 |
| **D8** | 独立 crate `solver-model` | `crates/ai/solver-model/`（AI 子系统；项目规则 §2.3.1）；依赖 `eneros-solver-core`（path = "../solver-core"） | 建模框架是 Solver 子系统第二层；与 v0.64.0 同属 AI 子系统 |
| **D9** | `compile()` 返回 `Result<LpProblem, SolverError>` | 保留 `Result<LpProblem, SolverError>`；复用 v0.64.0 `SolverError`；新增 `InvalidProblem(String)` 错误场景（变量名冲突/空问题）已由 v0.64.0 定义 | 错误类型复用；v0.64.0 `SolverError::InvalidProblem(String)` 正好覆盖编译错误 |
| **D10** | 无 `unsafe` 块 | 保留纯 safe Rust；零 `unsafe`、零外部 C 库 | 建模框架是纯 Rust 逻辑层；与 v0.64.0 默认构建一致（Mock 路径零 unsafe） |
| **D11** | 无 feature-gated 模块 | 保留无 feature-gated；纯 Rust，无 FFI 需求 | 建模框架不涉及 FFI；与 v0.64.0 `highs-ffi` feature 解耦 |
| **D12** | `LinearExpr` 派生 `Default`（`#[derive(Default)]`） | 保留 `#[derive(Default)]`（`BTreeMap` 和 `f64` 都实现 `Default`） | `BTreeMap::default()` 返回空 map；`f64::default()` 返回 0.0；与蓝图一致 |

## ADDED Requirements

### Requirement: Variable + VarBuilder 决策变量

系统 SHALL 提供 `Variable` 结构体表示决策变量：

- `name: alloc::string::String` — 变量名
- `lower_bound: f64` — 下界（默认 0.0）
- `upper_bound: f64` — 上界（默认 `f64::INFINITY`）
- `var_type: VarType` — 变量类型（复用 v0.64.0 `VarType`，D3）
- `index: Option<usize>` — 变量索引（编译前 None，编译后 Some(idx)，D4）

派生 `Debug` + `Clone`。

`VarBuilder` 链式构建器：

- `VarBuilder::new(name: &str) -> Self` — 默认 lower=0.0, upper=INFINITY, var_type=Continuous
- `lower(v: f64) -> Self` / `upper(v: f64) -> Self` / `range(lo: f64, hi: f64) -> Self`
- `non_negative() -> Self` — 等价于 `lower(0.0)`
- `integer() -> Self` — 设置 var_type=Integer
- `binary() -> Self` — 设置 var_type=Binary + range(0.0, 1.0)
- `build() -> Variable` — 构建变量

#### Scenario: VarBuilder 链式构建
- **WHEN** `VarBuilder::new("x").range(0.0, 10.0).integer().build()`
- **THEN** 返回 `Variable { name: "x", lower_bound: 0.0, upper_bound: 10.0, var_type: VarType::Integer, index: None }`

### Requirement: LinearExpr 线性表达式

系统 SHALL 提供 `LinearExpr` 结构体表示线性表达式 `c1*x1 + c2*x2 + ... + constant`：

- `terms: BTreeMap<usize, f64>` — 系数-变量对（变量索引 → 系数，D2：BTreeMap 替代 HashMap）
- `constant: f64` — 常数项

派生 `Debug` + `Clone` + `Default`（D12）。

方法：

- `LinearExpr::new() -> Self` — 空表达式
- `LinearExpr::from_var(var: &Variable) -> Self` — 从变量创建（系数 1.0，需 var.index = Some）
- `add_term(var_idx: usize, coeff: f64) -> &mut Self` — 添加项（系数累加，0 系数自动移除）
- `scale(factor: f64) -> Self` — 标量乘法
- `add(other: &LinearExpr) -> Self` — 加法
- `sub(other: &LinearExpr) -> Self` — 减法

运算符重载（D1：`core::ops`）：

- `impl core::ops::Add<LinearExpr> for LinearExpr` — `expr1 + expr2`
- `impl core::ops::Sub<LinearExpr> for LinearExpr` — `expr1 - expr2`
- `impl core::ops::Mul<f64> for LinearExpr` — `expr * 2.0`

#### Scenario: 运算符组合
- **WHEN** `(expr1 + expr2 * 2.0) - expr3`
- **THEN** 返回 `LinearExpr` 合并所有项，系数正确累加

### Requirement: Constraint 约束类型

系统 SHALL 提供 `Constraint` 枚举表示约束：

- `Le(LinearExpr, f64)` — `expr <= rhs`
- `Ge(LinearExpr, f64)` — `expr >= rhs`
- `Eq(LinearExpr, f64)` — `expr == rhs`
- `Range(LinearExpr, f64, f64)` — `lo <= expr <= hi`

派生 `Debug` + `Clone`。

#### Scenario: 约束构造
- **WHEN** `Constraint::Le(expr, 10.0)`
- **THEN** 表示 `expr <= 10.0`，编译时 rhs_lower=-INFINITY, rhs_upper=10.0

### Requirement: OptProblem 优化问题容器 + 编译器

系统 SHALL 提供 `OptProblem` 结构体作为优化问题容器 + Builder + 编译器：

- `variables: Vec<Variable>` — 变量列表
- `var_map: BTreeMap<String, usize>` — 变量名→索引映射（D2：BTreeMap）
- `objective: Option<LinearExpr>` — 目标函数
- `sense: ObjectiveSense` — 目标方向（复用 v0.64.0 `ObjectiveSense`，D3）
- `constraints: Vec<Constraint>` — 约束列表
- `constraint_names: Vec<String>` — 约束名称列表

方法：

- `OptProblem::new() -> Self` — 空问题
- `add_var(var: Variable) -> usize` — 添加变量并返回索引（分配 index 字段）
- `var(name: &str) -> Option<&Variable>` — 按名称获取变量
- `minimize(expr: LinearExpr) -> Self` — 设置目标（Minimize，链式）
- `maximize(expr: LinearExpr) -> Self` — 设置目标（Maximize，链式）
- `add_constraint(name: &str, constraint: Constraint) -> &mut Self` — 添加约束
- `compile() -> Result<LpProblem, SolverError>` — 编译为 `LpProblem` 矩阵格式（D9：复用 v0.64.0 LpProblem/SolverError）

#### Scenario: 编译为 LpProblem
- **WHEN** 构建含 2 变量 + 1 约束的 OptProblem 并调用 `compile()`
- **THEN** 返回 `Ok(LpProblem)`，其中 `variables.len()==2`，`constraints.num_rows==1`，CSR 格式正确

#### Scenario: 空问题编译
- **WHEN** `OptProblem::new().compile()`
- **THEN** 返回 `Ok(LpProblem)`，所有字段为空（0 变量、0 约束）

#### Scenario: 端到端 MockSolver 求解
- **WHEN** 构建 OptProblem → compile() → MockSolver.solve(&lp, now_ms=0)
- **THEN** 返回 `Ok(SolveResult { status: SolveStatus::Optimal, ... })`（D7：用 MockSolver 验证端到端流程）

## MODIFIED Requirements

### Requirement: Workspace 成员与版本

根 `Cargo.toml` 的 `[workspace.package] version` 从 `0.64.0` 更新为 `0.65.0`；`members` 列表在 `"crates/ai/solver-core"` 之后添加 `"crates/ai/solver-model"`。

## REMOVED Requirements

### Requirement: 重新定义 VarType
**Reason**: 蓝图在 v0.65.0 重新定义 `VarType` 枚举（Continuous/Integer/Binary），但 v0.64.0 `eneros_solver_core::problem::VarType` 已定义且派生完整。重复定义会导致 `compile()` 返回的 `LpProblem.var_types: Vec<VarType>` 类型不匹配（D3）。
**Migration**: `use eneros_solver_core::problem::VarType;` 直接复用。

### Requirement: HashMap 用于 terms 和 var_map
**Reason**: `HashMap` 在 no_std 下不可用（需 `std::collections::HashMap` 或第三方 `hashbrown`）。引入 `hashbrown` 依赖过度（Karpathy Simplicity First）。BTreeMap 遍历顺序确定，更利于确定性编译（解决蓝图 §8.3 坑点）。
**Migration**: `use alloc::collections::BTreeMap;` 替代 `HashMap`，API 基本兼容（`insert`/`get`/`entry`/`iter`）。
