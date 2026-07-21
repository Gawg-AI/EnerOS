# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.65.0`
- [x] C2 members 列表已添加 `crates/ai/solver-model`（置于 `crates/ai/solver-core` 之后）
- [x] C3 `cargo metadata --format-version 1` 成功

## Crate 骨架
- [x] C4 `crates/ai/solver-model/Cargo.toml` 存在，package name = `eneros-solver-model`
- [x] C5 dependencies 包含 `eneros-solver-core = { path = "../solver-core" }`（D3/D8/D9）
- [x] C6 **不声明** `[features]` 段（D11：纯 Rust，无 FFI）
- [x] C7 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C8 `src/lib.rs` 包含 D1~D12 偏差声明表
- [x] C9 模块声明：variable / expr / constraint / problem

## variable.rs — Variable + VarBuilder
- [x] C10 `Variable` 结构体：name / lower_bound / upper_bound / var_type（复用 v0.64.0 VarType，D3）/ index: Option<usize>（D4）
- [x] C11 派生 `Debug` + `Clone`
- [x] C12 `VarBuilder` 结构体 + 链式方法：new / lower / upper / range / non_negative / integer / binary / build
- [x] C13 `VarBuilder::new(name)` 默认 lower=0.0, upper=INFINITY, var_type=Continuous
- [x] C14 `binary()` 同时设置 var_type=Binary + range(0.0, 1.0)
- [x] C15 `build()` 返回 Variable（index = None）

## expr.rs — LinearExpr 线性表达式
- [x] C16 `LinearExpr` 结构体：terms: BTreeMap<usize, f64>（D2）/ constant: f64
- [x] C17 派生 `Debug` + `Clone` + `Default`（D12）
- [x] C18 `LinearExpr::new() -> Self`（空表达式）
- [x] C19 `LinearExpr::from_var(var: &Variable) -> Self`（系数 1.0，需 var.index = Some）
- [x] C20 `add_term(var_idx, coeff) -> &mut Self`（系数累加，0 系数自动移除）
- [x] C21 `scale(factor: f64) -> Self`（标量乘法）
- [x] C22 `add(other: &LinearExpr) -> Self`（加法）
- [x] C23 `sub(other: &LinearExpr) -> Self`（减法）
- [x] C24 运算符重载 `impl core::ops::Add<LinearExpr>`（D1：core::ops）
- [x] C25 运算符重载 `impl core::ops::Sub<LinearExpr>`（D1）
- [x] C26 运算符重载 `impl core::ops::Mul<f64>`（D1）

## constraint.rs — Constraint 枚举
- [x] C27 `Constraint` 枚举：Le(LinearExpr, f64) / Ge(LinearExpr, f64) / Eq(LinearExpr, f64) / Range(LinearExpr, f64, f64)
- [x] C28 派生 `Debug` + `Clone`

## problem.rs — OptProblem 容器 + 编译器
- [x] C29 `OptProblem` 结构体：variables / var_map: BTreeMap<String, usize>（D2）/ objective / sense（复用 v0.64.0 ObjectiveSense，D3）/ constraints / constraint_names
- [x] C30 `OptProblem::new() -> Self`（空问题）
- [x] C31 `add_var(var: Variable) -> usize`（分配 index 字段，返回索引）
- [x] C32 `var(name: &str) -> Option<&Variable>`（按名称查找）
- [x] C33 `minimize(expr: LinearExpr) -> Self`（链式）
- [x] C34 `maximize(expr: LinearExpr) -> Self`（链式）
- [x] C35 `add_constraint(name: &str, constraint: Constraint) -> &mut Self`
- [x] C36 `compile() -> Result<LpProblem, SolverError>`（D9：复用 v0.64.0 LpProblem/SolverError）
- [x] C37 编译逻辑：Le→rhs_lower=-INF, rhs_upper=rhs / Ge→rhs_lower=rhs, rhs_upper=INF / Eq→rhs_lower=rhs, rhs_upper=rhs / Range→rhs_lower=lo, rhs_upper=hi
- [x] C38 CSR 格式：row_start.len() == num_rows + 1
- [x] C39 稀疏性：系数绝对值 <1e-12 的项自动剔除

## 集成测试（lib.rs）
- [x] C40 T1 VarBuilder::new().build() 默认值
- [x] C41 T2 VarBuilder 链式方法：range/integer/binary
- [x] C42 T3 Variable 字段访问 + Clone
- [x] C43 T4 LinearExpr::new() 空表达式 + Default
- [x] C44 T5 LinearExpr::from_var 构造
- [x] C45 T6 LinearExpr::add_term 系数累加 + 0 系数移除
- [x] C46 T7 LinearExpr::scale 标量乘法
- [x] C47 T8 LinearExpr::add 加法
- [x] C48 T9 LinearExpr::sub 减法
- [x] C49 T10 运算符重载：expr1 + expr2
- [x] C50 T11 运算符重载：expr1 - expr2
- [x] C51 T12 运算符重载：expr * 2.0
- [x] C52 T13 运算符组合：expr1 + expr2 * 2.0 - expr3
- [x] C53 T14 Constraint::Le/Ge/Eq/Range 构造
- [x] C54 T15 OptProblem::new() 空问题
- [x] C55 T16 OptProblem::add_var + var(name) 查找
- [x] C56 T17 OptProblem::minimize/maximize 链式
- [x] C57 T18 OptProblem::add_constraint
- [x] C58 T19 OptProblem::compile() 生成 LpProblem（2 变量 + 1 约束）
- [x] C59 T20 OptProblem::compile() 空问题（0 变量、0 约束）
- [x] C60 T21 OptProblem::compile() CSR 格式正确（row_start.len()==num_rows+1）
- [x] C61 T22 端到端：OptProblem → compile() → MockSolver.solve() 返回 Optimal（D7）
- [x] C62 `cargo test -p eneros-solver-model` 22/22 通过

## 设计文档
- [x] C63 `docs/ai/solver-model-design.md` 存在
- [x] C64 12 章节完整
- [x] C65 2 Mermaid 图（OptProblem + 编译流程类图 + compile() 时序图）
- [x] C66 D1~D12 偏差声明表
- [x] C67 文档在 `docs/ai/` 下（符合目录规范）

## 版本同步
- [x] C68 `Makefile` 版本号 `0.65.0`
- [x] C69 `.github/workflows/ci.yml` 版本号 `0.65.0`
- [x] C70 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-solver-model`

## 构建校验（§2.4.2 C6~C11）
- [x] C71 `cargo metadata --format-version 1` 成功
- [x] C72 `cargo test -p eneros-solver-model` 全部通过（22 tests）
- [x] C73 `cargo build -p eneros-solver-model --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C74 `cargo fmt -p eneros-solver-model -- --check` 通过
- [x] C75 `cargo clippy -p eneros-solver-model --all-targets -- -D warnings` 无 warning
- [x] C76 `cargo deny check licenses bans sources` 通过

## no_std 合规
- [x] C77 无 `use std::*`（仅 `alloc::*` / `core::*`）
- [x] C78 无 `panic!` / `todo!` / `unimplemented!`
- [x] C79 子模块不重复 `#![cfg_attr(not(test), no_std)]`
- [x] C80 无 `unsafe` 块（D10：纯 safe Rust）
- [x] C81 无 `HashMap`（D2：使用 `BTreeMap`）
- [x] C82 无 `std::ops`（D1：使用 `core::ops`）
- [x] C83 `f64::INFINITY` / `f64::NEG_INFINITY` 使用 `core::f64` 常量（D5）

## 目录规范
- [x] C84 crate 在 `crates/ai/solver-model/`（D8）
- [x] C85 跨 crate path 引用 `../solver-core`（相对路径，D8）
- [x] C86 文档在 `docs/ai/` 下
- [x] C87 无根目录 crate（除 `ci/`）
- [x] C88 无垃圾文件（`target/` / `*.elf` / `*.bin` 被忽略）

## 类型复用与解耦（D3）
- [x] C89 复用 v0.64.0 `VarType`（不重定义）
- [x] C90 复用 v0.64.0 `ObjectiveSense`（不重定义）
- [x] C91 复用 v0.64.0 `LpProblem` / `ConstraintMatrix`（编译目标）
- [x] C92 复用 v0.64.0 `SolverError`（错误类型）
- [x] C93 复用 v0.64.0 `MockSolver`（端到端测试，D7）
- [x] C94 **不依赖** v0.59.0~v0.63.0（LLM 子系统）— Solver 建模层与 LLM 解耦

## 简化设计验证（Karpathy 原则）
- [x] C95 无 `hashbrown` 外部依赖（D2：使用 `BTreeMap`，Simplicity First）
- [x] C96 无重新定义 `VarType`（D3：复用 v0.64.0）
- [x] C97 无 `std::ops`（D1：`core::ops`）
- [x] C98 无 Python 测试代码（D7：Rust 单元测试 + MockSolver 端到端）
- [x] C99 无 feature-gated 模块（D11：纯 Rust，无 FFI）
- [x] C100 无 `Send + Sync` bounds（与 v0.64.0 Solver trait 一致）
