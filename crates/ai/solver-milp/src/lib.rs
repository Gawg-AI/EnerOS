//! EnerOS MILP 求解集成（v0.102.0，P2-F 第 1 版：Solver 从 LP 扩展到 MILP）.
//!
//! 基于 v0.65.0 `OptProblem` DSL 与 v0.64.0 `LpProblem` 矩阵格式，构建机组组合
//! （Unit Commitment, UC）日前调度 MILP 模型：
//! - 决策变量：各机组各时段四元组 P（出力，连续）/ U（运行状态，0-1）/
//!   V（启动动作，0-1）/ W（停机动作，0-1）
//! - 目标函数：最小化总成本 = Σ (price[t]·P[i,t] + start_cost_i·V[i,t])
//! - 约束：功率平衡 / pmax·pmin 联动 / 爬坡 / 启停逻辑 / 最小运行 / 最小停机
//!
//! # 核心类型
//!
//! - [`uc_model::UcUnit`] — 机组参数（出力上下限 / 爬坡率 / 启动成本 /
//!   最小运行停机周期 / 初始状态）
//! - [`uc_model::UnitCommitment`] — UC 模型构建器（`build_model` 完整模型 /
//!   `build_model_relaxed` 松弛模型，编译为 `LpProblem`）
//! - [`day_ahead::UnitSchedule`] — 单机组日前计划（运行状态 + 出力计划）
//! - [`day_ahead::DayAheadPlan`] — 日前计划（全部机组 + 总成本 + 求解状态）
//! - [`day_ahead::DayAheadScheduler`] — 日前调度器（状态驱动三级降级链 D9 /
//!   求解参数注入 D10 / `relax_count`·`lp_fallback_count` 计数器）
//!
//! # 偏差声明（D1~D12）
//!
//! | 偏差 | 蓝图原文 | 本版本处理 | 理由 |
//! |------|---------|-----------|------|
//! | **D1** | `crates/solver_milp/` | `crates/ai/solver-milp/` | 项目规则 §2.3.1：crate 必须按子系统归入 `crates/<subsystem>/` |
//! | **D2** | `docs/phase2/milp_solver.md` | `docs/ai/milp-solver-design.md` | 项目规则 §2.3.3：文档按方向分类 |
//! | **D3** | `tests/milp_day_ahead.rs` | src 内嵌 `#[cfg(test)]` | 项目惯例：测试内嵌 src 文件，禁止 tests/ 目录 |
//! | **D4** | 重定义 `MilpSolver`/`MilpModel`/`MilpSolution`/`SolveStatus` | 复用 v0.64.0 `Solver` trait + `LpProblem`（`var_types` 已含 Binary/Integer）+ `SolveResult` + `SolveStatus` + `SolverError`；蓝图 `Feasible` 由 `Suboptimal` 承载 | 避免类型重定义导致接口割裂 |
//! | **D5** | `highs_ffi` 独立模块 | solver-core ffi/highs 增量 `Highs_passMip` + 分派 | FFI 单一归属（feature-gated），由并行任务 T1 落地 |
//! | **D6** | **蓝图 Bug**：§4.5 `col_cost[base+1] = start_cost`（注释自称 V 启动成本，但 base+1=U） | 启动成本挂 V（base+2），U/W 系数 0 | 蓝图索引错位修正，TU5 断言锁定 |
//! | **D7** | `num_constraints = t + n·t·3` 桩 | 完整标准 UC 约束集（min_up/min_down/ramp/init_status 字段全生效） | 桩公式不满足 UC 语义；约束组详见 `uc_model` 文档 |
//! | **D8** | `build_model` 直接返回 `LpProblem` | 返回 `Result<LpProblem, SolverError>`（长度校验） | no_std 禁 panic |
//! | **D9** | 未明确降级链 | 状态驱动降级链（Infeasible/Unbounded/Error → relaxed → LP 松弛；`relax_count`/`lp_fallback_count` 计数器；Timeout/Suboptimal 视为可接受） | day_ahead.rs 落地（后续任务） |
//! | **D10** | `MilpSolver::set_time_limit` | 复用 `Solver::set_param("time_limit"/"mip_rel_gap")` | day_ahead.rs 落地（后续任务） |
//! | **D11** | 真实 HiGHS 求解 <5s | 测试用 MockSolver；性能基准测模型构建（真实 HiGHS 求解 <5s 留待硬件集成验证） | 与 v0.64.0~v0.66.0 一致，避免 C 库依赖 |
//! | **D12** | `std::string::String` / `std::vec::Vec` / `std::f64::INFINITY` | `String`/`Vec` = `alloc::*`；`f64::INFINITY` = `core::f64`；`interval_min` 参与爬坡约束（ramp MW/min × interval_min = MW/周期） | no_std 合规；量纲正确 |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，零 unsafe，可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod day_ahead;
pub mod uc_model;

pub use day_ahead::{DayAheadPlan, DayAheadScheduler, UnitSchedule};
pub use uc_model::{UcUnit, UnitCommitment};
