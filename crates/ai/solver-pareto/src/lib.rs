//! EnerOS v0.104.0 多目标 Pareto 优化（P2-F 第 3 版：Solver 扩展收尾）.
//!
//! 日前/联邦调度需兼顾经济性、碳排放、设备寿命三目标，单目标 LP/MILP 无法表达
//! 目标间权衡。本版基于 v0.64.0 `SolverError`（D11 复用）与 v0.103.0 热启动加速
//! 底座，实现 NSGA-II 多目标 Pareto 前沿生成 + 决策者加权选择，为联邦多目标协调
//! 奠基（P2-F 闭环）。
//!
//! # 核心类型
//!
//! - [`pareto_front::MultiObjectiveProblem`] — 多目标问题（目标列表 + 变量界，D6 无约束字段）
//! - [`pareto_front::Objective`] — 优化目标（名称 + 方向 + 权重）
//! - [`pareto_front::OptDirection`] — 优化方向（Minimize / Maximize）
//! - [`pareto_front::VariableSpec`] — 决策变量上下界
//! - [`pareto_front::ParetoSolution`] — Pareto 解（变量值 + 目标值 + rank + 拥挤度）
//! - [`pareto_front::ParetoFront`] — Pareto 前沿（`non_dominated` / `select_by_weight`）
//! - [`pareto_front::ParetoSolver`] — 多目标求解器 trait（D5：无 Send + Sync bound）
//! - [`pareto_front::dominates`] — 支配判定（统一最小化口径，D7）
//! - `nsga2::Nsga2Solver` — NSGA-II 求解器（T2 实现，D4 内置确定性 PRNG）
//! - [`decision::DecisionMaker`] — 决策者（偏好权重归一化 → 前沿加权选择）
//!
//! # 偏差声明（D1~D12）
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | **D1** | 蓝图 `crates/solver_pareto/` → `crates/ai/solver-pareto/` | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；与 solver-core/solver-milp/solver-warm 同 AI 子系统 |
//! | **D2** | 蓝图 `docs/phase2/pareto.md` → `docs/ai/pareto-design.md` | 记忆 §2.3.3 强制：文档按方向分类 |
//! | **D3** | 蓝图 `tests/pareto_front.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.103.0 项目惯例，不新增 tests/ 文件 |
//! | **D4** | 蓝图 `use rand::Rng` + `thread_rng()` → 内置确定性 xorshift64* PRNG（~20 行），seed 构造注入（`Nsga2Solver::with_seed(seed)`，`new()` 默认固定 seed） | rand crate 依赖 std，违反全项目 no_std（记忆 §4.3，蓝图 §43.1）；确定性 seed 使测试可复现（Karpathy Goal-Driven） |
//! | **D5** | 蓝图 `ParetoSolver: Send + Sync` → 去除 bound | 与 v0.64.0 `Solver`/v0.103.0 `WarmStartProvider` 惯例一致；NSGA-II 单线程种群算法无跨线程需求 |
//! | **D6** | 蓝图 §4.1 `constraints: Vec<Constraint>` 删除（`Constraint` 类型蓝图未定义且算法全程未消费） | 界约束已由 `VariableSpec.{lower,upper}` 表达；功能约束属目标评估层（Karpathy Simplicity First，不引入死字段） |
//! | **D7** | Maximize 目标在评估出口统一取负（蓝图 `"lifespan" => -sum` 硬编码的一般化），dominates/crowding/select 统一最小化口径 | 蓝图支配判定隐含全最小化假设但未声明；归一化后算法与方向解耦，目标可扩展（蓝图 §8.4/§9 可扩展要求） |
//! | **D8** | 蓝图 `partial_cmp(...).unwrap()` → `f64::total_cmp` | NaN 输入时 unwrap 会 panic，违反 no_std 禁 `panic!`（项目规则）；total_cmp 全序确定性（core 可用，≥1.62） |
//! | **D9** | 蓝图 solve 每代 `population = front1.take(pop_size)`（种群随 front 萎缩、无真实交叉变异，注释自承"简化"）→ 实现锦标赛选择（rank 优先、平手比拥挤度）+ 均匀交叉 + 均匀变异补满 pop_size | 对齐蓝图 §4.3 Mermaid（选择/交叉/变异为流程必经节点）与 §5.1"NSGA-II 采用"承诺；骨架可用标准（记忆 §4.4） |
//! | **D10** | 蓝图 §4.4"前沿为空 → 返回 LP 单目标解" → 本 crate 不内联 LP 兜底：`solve` 前沿为空时返回空 front（`is_empty()` 可判），由编排层回退 v0.66.0 单目标 LP | crate 无 LP 问题输入，内联 LP 造成依赖反转（Simplicity First）；`select_by_weight`/`choose` 空 front 返回 None |
//! | **D11** | `SolverError` 复用 eneros-solver-core（`InvalidProblem` 变体），不新建 ParetoError | 蓝图 §4.2 签名即 `SolverError`；v0.103.0 复用先例；避免平行错误体系 |
//! | **D12** | 性能 50 代 × 100 种群 < 10s 落地为 `#[cfg(test)]` 断言（std `Instant` 仅测试可用）；算法复杂度 O(gen × pop² × obj) 声明于文档 | no_std 无计时器（v0.64.0 D1 `now_ms` 注入先例；测试外不注入计时，保持 solve 签名与蓝图一致） |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，默认构建零 unsafe、零 C 依赖、零第三方依赖
//! （仅 eneros-solver-core），不调用 `panic!` / `todo!` / `unimplemented!`，
//! 可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod decision;
pub mod nsga2;
pub mod pareto_front;

pub use decision::DecisionMaker;
pub use pareto_front::{
    dominates, MultiObjectiveProblem, Objective, OptDirection, ParetoFront, ParetoSolution,
    ParetoSolver, VariableSpec,
};
