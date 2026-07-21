# Tasks

- [x] Task 1: Workspace 同步
  - [x] 根 `Cargo.toml` 版本号 `0.64.0` → `0.65.0`
  - [x] members 添加 `crates/ai/solver-model`（置于 `crates/ai/solver-core` 之后）
  - [x] 验证：`cargo metadata --format-version 1` 成功（待 crate 骨架创建后）

- [x] Task 2: 创建 `eneros-solver-model` crate 骨架
  - [x] 新建 `crates/ai/solver-model/Cargo.toml`，package name = `eneros-solver-model`
  - [x] dependencies 添加 `eneros-solver-core = { path = "../solver-core" }`（D3/D8/D9）
  - [x] 无 `[features]` 段（D11：纯 Rust，无 FFI）
  - [x] no_std 配置：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
  - [x] 新建 `src/lib.rs`，模块声明：variable / expr / constraint / problem
  - [x] lib.rs 包含 D1~D12 偏差声明表
  - [x] 验证：`cargo metadata --format-version 1` 成功

- [x] Task 3: 实现 `variable.rs` — Variable + VarBuilder
  - [x] `Variable` 结构体：name: alloc::string::String / lower_bound: f64 / upper_bound: f64 / var_type: VarType（复用 v0.64.0，D3）/ index: Option<usize>（D4）
  - [x] 派生 `Debug` + `Clone`
  - [x] `VarBuilder` 结构体：name / lower / upper / var_type（链式 Builder）
  - [x] `VarBuilder::new(name: &str) -> Self`（默认 lower=0.0, upper=INFINITY, var_type=Continuous）
  - [x] 链式方法：`lower(v)` / `upper(v)` / `range(lo,hi)` / `non_negative()` / `integer()` / `binary()`
  - [x] `build() -> Variable`（index = None）
  - [x] 验证：编译通过

- [x] Task 4: 实现 `expr.rs` — LinearExpr 线性表达式
  - [x] `LinearExpr` 结构体：terms: BTreeMap<usize, f64>（D2：BTreeMap）/ constant: f64
  - [x] 派生 `Debug` + `Clone` + `Default`（D12）
  - [x] `LinearExpr::new() -> Self`（空表达式）
  - [x] `LinearExpr::from_var(var: &Variable) -> Self`（系数 1.0，需 var.index = Some）
  - [x] `add_term(var_idx: usize, coeff: f64) -> &mut Self`（系数累加，0 系数自动移除）
  - [x] `scale(factor: f64) -> Self`（标量乘法）
  - [x] `add(other: &LinearExpr) -> Self`（加法）
  - [x] `sub(other: &LinearExpr) -> Self`（减法）
  - [x] 运算符重载（D1：`core::ops`）：`impl Add<LinearExpr>` / `impl Sub<LinearExpr>` / `impl Mul<f64>`
  - [x] 验证：编译通过

- [x] Task 5: 实现 `constraint.rs` — Constraint 枚举
  - [x] `Constraint` 枚举：Le(LinearExpr, f64) / Ge(LinearExpr, f64) / Eq(LinearExpr, f64) / Range(LinearExpr, f64, f64)
  - [x] 派生 `Debug` + `Clone`
  - [x] 验证：编译通过

- [x] Task 6: 实现 `problem.rs` — OptProblem 优化问题容器 + 编译器
  - [x] `OptProblem` 结构体：variables: Vec<Variable> / var_map: BTreeMap<String, usize>（D2）/ objective: Option<LinearExpr> / sense: ObjectiveSense（复用 v0.64.0，D3）/ constraints: Vec<Constraint> / constraint_names: Vec<String>
  - [x] `OptProblem::new() -> Self`（空问题）
  - [x] `add_var(var: Variable) -> usize`（分配 index 字段，返回索引）
  - [x] `var(name: &str) -> Option<&Variable>`（按名称查找）
  - [x] `minimize(expr: LinearExpr) -> Self`（链式）
  - [x] `maximize(expr: LinearExpr) -> Self`（链式）
  - [x] `add_constraint(name: &str, constraint: Constraint) -> &mut Self`
  - [x] `compile() -> Result<LpProblem, SolverError>`（D9：复用 v0.64.0 LpProblem/SolverError；编译为 CSR 矩阵格式）
  - [x] 编译逻辑：变量边界/类型 → 目标函数系数 → 约束矩阵（CSR：row_start.len()==num_rows+1）→ rhs_lower/rhs_upper（Le→[-INF,rhs] / Ge→[rhs,INF] / Eq→[rhs,rhs] / Range→[lo,hi]）
  - [x] 稀疏性：系数绝对值 <1e-12 的项自动剔除
  - [x] 验证：编译通过

- [x] Task 7: 集成测试模块（lib.rs `#[cfg(test)] mod tests`）
  - [x] T1 VarBuilder::new().build() 默认值（lower=0, upper=INF, Continuous, index=None）
  - [x] T2 VarBuilder 链式方法：range/integer/binary
  - [x] T3 Variable 字段访问 + Clone
  - [x] T4 LinearExpr::new() 空表达式 + Default
  - [x] T5 LinearExpr::from_var 构造（var.index=Some）
  - [x] T6 LinearExpr::add_term 系数累加 + 0 系数移除
  - [x] T7 LinearExpr::scale 标量乘法
  - [x] T8 LinearExpr::add 加法
  - [x] T9 LinearExpr::sub 减法
  - [x] T10 运算符重载：expr1 + expr2
  - [x] T11 运算符重载：expr1 - expr2
  - [x] T12 运算符重载：expr * 2.0
  - [x] T13 运算符组合：expr1 + expr2 * 2.0 - expr3
  - [x] T14 Constraint::Le/Ge/Eq/Range 构造
  - [x] T15 OptProblem::new() 空问题
  - [x] T16 OptProblem::add_var + var(name) 查找
  - [x] T17 OptProblem::minimize/maximize 链式
  - [x] T18 OptProblem::add_constraint
  - [x] T19 OptProblem::compile() 生成 LpProblem（2 变量 + 1 约束）
  - [x] T20 OptProblem::compile() 空问题（0 变量、0 约束）
  - [x] T21 OptProblem::compile() CSR 格式正确（row_start.len()==num_rows+1）
  - [x] T22 端到端：OptProblem → compile() → MockSolver.solve() 返回 Optimal（D7）
  - [x] 验证：`cargo test -p eneros-solver-model` 全部通过

- [x] Task 8: 设计文档 `docs/ai/solver-model-design.md`
  - [x] 12 章节：版本目标 / 架构定位 / Variable + VarBuilder / LinearExpr 线性表达式 / Constraint 约束 / OptProblem 容器 + 编译器 / 运算符重载 / 编译流程 / 错误处理 / no_std 合规 / 内存预算 / 偏差声明
  - [x] 2 Mermaid 图：OptProblem + 编译流程类图 + compile() 时序图
  - [x] D1~D12 偏差声明表
  - [x] 文档位置在 `docs/ai/` 下（复用 v0.59.0~v0.64.0 创建的目录）

- [x] Task 9: 版本号同步 + gate.rs 注释更新
  - [x] `Makefile` 版本号 `0.64.0` → `0.65.0`
  - [x] `.github/workflows/ci.yml` 版本号 `0.64.0` → `0.65.0`
  - [x] `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-solver-model` 说明
  - [x] 验证：`cargo build -p eneros-solver-model` 通过

- [x] Task 10: 构建校验（§2.4.2 C6~C11）
  - [x] `cargo metadata --format-version 1` 成功
  - [x] `cargo test -p eneros-solver-model` 全部通过（22 tests）
  - [x] `cargo build -p eneros-solver-model --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
  - [x] `cargo fmt -p eneros-solver-model -- --check` 格式通过
  - [x] `cargo clippy -p eneros-solver-model --all-targets -- -D warnings` lint 通过
  - [x] `cargo deny check licenses bans sources` 安全扫描通过

- [x] Task 11: 更新 tasks.md + checklist.md 所有项 → [x]
  - [x] tasks.md 11 任务全部 [x]
  - [x] checklist.md 所有检查点全部 [x]

# Task Dependencies

- Task 2（crate 骨架）→ Task 1（metadata 验证需骨架）
- Task 3（variable）独立（仅依赖 v0.64.0 VarType）
- Task 4（expr）依赖 Task 3（from_var 使用 Variable）
- Task 5（constraint）依赖 Task 4（使用 LinearExpr）
- Task 6（problem）依赖 Task 3 + Task 4 + Task 5（使用 Variable/LinearExpr/Constraint）
- Task 7（集成测试）→ Task 3~6（测试依赖所有模块）
- Task 8（设计文档）可与 Task 6~7 并行（独立工作）
- Task 9（版本同步）→ Task 8（版本同步在功能完成后）
- Task 10（构建校验）→ Task 9
- Task 11（更新文档）→ Task 10（全部校验通过后）

# Parallelizable Work

- Task 3（variable）+ Task 4（expr）+ Task 5（constraint）可部分并行（Task 4 依赖 Task 3 的 Variable，但仅类型引用）
- Task 6（problem）依赖 Task 3 + 4 + 5
- Task 7（集成测试）依赖 Task 3~6
- Task 8（设计文档）可与 Task 6~7 并行（独立工作）
