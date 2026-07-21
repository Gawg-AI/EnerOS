# v0.102.0 MILP 求解器集成 Spec

> 蓝图：`e:\eneros\蓝图\phase2.md` §v0.102.0（P2-F 第 1 版，9 节齐全）。新建 crate `crates/ai/solver-milp/`（eneros-solver-milp）+ solver-core feature-gated FFI 增量。无 v0.102.x 刚性子版本（蓝图检索确认，Phase 2 刚性子版本仅 v0.98.1）。

## Why

日前调度需要机组启停（UC）0-1 离散决策，v0.66.0 纯 LP（连续变量）无法表达"开/停机"，启停成本不可量化。v0.102.0 将 Solver 从 LP 扩展到 MILP：复用 v0.64.0 `LpProblem.var_types`（已含 Binary/Integer）与 HiGHS FFI，补齐整数性传参，实现 UC 建模 + 日前计划生成 + 确定性降级链，为 v0.103.0 热启动 / v0.104.0 Pareto 提供 MILP 基座。

## What Changes

- **新建** `crates/ai/solver-milp/`（`eneros-solver-milp`，no_std + alloc，零第三方依赖）：
  - `src/uc_model.rs`：`UcUnit` / `UnitCommitment` + `build_model` / `build_model_relaxed`（经 v0.65.0 DSL 构建标准 UC MILP → v0.64.0 `LpProblem`）
  - `src/day_ahead.rs`：`UnitSchedule` / `DayAheadPlan` / `DayAheadScheduler`（求解 + 结果解析 + 三级降级链）
  - `src/lib.rs`：模块声明 + 重导出 + crate 文档（含 D1~D12 偏差表）
- **修改** `crates/ai/solver-core/src/ffi.rs`：feature-gated 增量 `Highs_passMip` extern 声明（纯追加，既有声明零改动）
- **修改** `crates/ai/solver-core/src/highs.rs`：`solve()` 检测非连续 `var_types` → 分派 `Highs_passMip`（纯连续问题走原 `Highs_passLp` 路径，LP 行为零变化；全部 `#[cfg(feature = "highs-ffi")]` 门控）
- **修改** `crates/ai/solver-core/src/lib.rs`：crate 文档追加 v0.102.0 一句说明（既有偏差表不动）
- **修改** `crates/ai/solver-core/Cargo.toml`：description 追加 v0.102.0
- **新增** `configs/milp-solver.toml`：`[milp]` time_limit_s / mip_rel_gap + 中文注释 ≥6 点
- **新增** `docs/ai/milp-solver-design.md`：12 章节 + ≥2 Mermaid + D1~D12 偏差表
- **新增 31 个单元测试**（src 内嵌 `#[cfg(test)]`，项目惯例，不新增 tests/ 文件）
- 根 `Cargo.toml`：members 追加 `"crates/ai/solver-milp"` + version 0.101.0 → 0.102.0；`Makefile` / `ci.yml` / `gate.rs` 注释串尾 2 处同步
- **无 BREAKING**：既有全部 crate 公共 API 零改动（solver-core 变更仅 feature-gated 追加）

## Impact

- Affected specs：develop-v10200-milp-solver（新建）；develop-v0640-solver-core（MODIFIED，feature-gated 增量）
- Affected code：`crates/ai/solver-milp/`（新建）、`crates/ai/solver-core/src/{ffi,highs,lib}.rs` + `Cargo.toml`（增量）、`configs/`、`docs/ai/`、根 4 文件版本号
- 上游：v0.64.0 solver-core（Solver/LpProblem/MockSolver）、v0.65.0 solver-model（DSL）、v0.66.0 energy-lp-model（LP 调度先例）、v0.96.0 Coordinator（日前计划消费方）
- 下游：v0.103.0 热启动（MILP 基座 + 历史解）、v0.104.0 Pareto 多目标

## ADDED Requirements

### Requirement: UC MILP 模型构建（uc_model.rs）

The system SHALL provide `UnitCommitment::build_model(load, price)`，经 v0.65.0 DSL 构建标准 UC MILP 并编译为 v0.64.0 `LpProblem`：变量 `P/U/V/W` 四元组 per (机组, 周期)，约束覆盖功率平衡/出力联动/爬坡/启停逻辑/最小启停时间（D7 完整集，非蓝图桩）。

#### Scenario: 变量布局与类型
- **WHEN** n=5 机组 × t=24 周期调用 `build_model`
- **THEN** 变量数 = n·t·4 = 480；索引 `var_index(i,t,k) = (i·periods + t)·4 + k`（k=0 P 连续 / k=1,2,3 U,V,W Binary [0,1]）

#### Scenario: 目标函数系数（蓝图 Bug 修正 D6）
- **WHEN** 编译后读取 `objective`
- **THEN** P[i,t] 系数 = `price[t]`（发电成本，蓝图语义）；**V**[i,t] 系数 = `start_cost_i`（启动成本挂 V=base+2，修正蓝图 §4.5 挂 U=base+1 的 Bug）；U/W 系数 = 0；sense = Minimize

#### Scenario: 标准 UC 约束集（D7）
- **WHEN** 编译后统计约束（min_up/min_down ≥ 1）
- **THEN** 总行数 = t + 5nt + 2n(t−1)（功率平衡 t + pmax/pmin/启停逻辑/最小运行/最小停机 5 个 nt 组 + 爬坡 2n(t−1)）：功率平衡 t 行 Eq（Σ_i P[i,t] == load[t]）+ 出力联动 2nt 行（P ≤ p_max·U / P ≥ p_min·U）+ 爬坡 2n(t−1) 行（|ΔP| ≤ ramp·interval_min）+ 启停逻辑 nt 行（t=0：V−W−U == −init_status；t≥1：V−W−U+U_prev == 0）+ 最小运行 nt 行（窗口 ΣV ≤ U）+ 最小停机 nt 行（窗口 ΣW + U ≤ 1）；CSR `row_start.len() == rows + 1` 且 nnz 三数组等长

#### Scenario: 输入校验（D8）
- **WHEN** `load.len() != periods` 或 `price.len() != periods`
- **THEN** 返回 `Err(SolverError::InvalidProblem)`（no_std 禁 panic）

#### Scenario: 松弛模型
- **WHEN** 调用 `build_model_relaxed(load, price)`
- **THEN** 跳过最小启停 2nt 行，总行数 = t + 3nt + 2n(t−1)，其余约束不变（蓝图 §4.4 松弛最小时间约束）

### Requirement: 日前计划与降级链（day_ahead.rs）

The system SHALL provide `DayAheadScheduler::plan(uc, load, price, solver, now_ms)`：注入 `&mut dyn Solver` seam，先注入 time_limit/mip_rel_gap 参数，MILP 求解；不可行/无界/错误 → 松弛最小时间约束重解（relax_count 可观测）→ 仍失败 → LP 松弛（Binary/Integer → Continuous，上界保持 1.0）重解（lp_fallback_count 可观测）；Optimal/Suboptimal/Timeout 视为可接受结果（蓝图 §4.4 超时返回当前最优可行解）。

#### Scenario: 端到端 5 机组 × 24 周期（蓝图 §6.2）
- **WHEN** MockSolver 返回长度 480 的最优解
- **THEN** `plan.schedule.len() == 5`；每机组 `commitments.len() == generation.len() == 24`；`unit_id` 顺序与 `units` 一致；`total_cost == objective_value`；`solve_status` 透传；两计数器保持 0

#### Scenario: 结果解析
- **WHEN** 解向量中 U[i,t] = 0.8 / 0.2，P[i,t] = 50.0
- **THEN** `commitments[i][t] == true / false`（> 0.5 阈值）；`generation[i][t] == 50.0`

#### Scenario: 不可行触发最小时间松弛（D9）
- **WHEN** MILP 求解返回 Infeasible（或无界/Err）
- **THEN** 自动以 `build_model_relaxed` 重建重解，`relax_count += 1`，最终 plan 来自 relaxed 解

#### Scenario: 松弛仍失败触发 LP 降级（D9）
- **WHEN** relaxed 求解仍 Infeasible/Error
- **THEN** `relax_lp` 将全部 var_types 转 Continuous（Binary 上界保持 1.0）重解，`lp_fallback_count += 1`

#### Scenario: 全链失败显式返回
- **WHEN** MILP / relaxed / LP 三级全部失败
- **THEN** 返回 `Ok(DayAheadPlan { schedule: [], total_cost: 0.0, solve_status: <末级状态> })`（状态字段承载失败，上层 Coordinator 可判定，非静默吞没）

#### Scenario: 求解参数注入（D10）
- **WHEN** `DayAheadScheduler::new(time_limit_s, mip_rel_gap)` 后调用 `plan`
- **THEN** 每次求解前经 `Solver::set_param` 注入 `"time_limit"` 与 `"mip_rel_gap"`（记录型 stub 可验证）

### Requirement: solver-core MILP FFI 增量（feature-gated，D5）

The system SHALL 在 solver-core `ffi.rs` 追加 `Highs_passMip` extern 声明（同 `Highs_passLp` 签名 + 尾部 `integrality: *const c_int`），`highs.rs` `solve()` 在问题含非连续变量时改走 `Highs_passMip`（Continuous→0 / Integer→1 / Binary→1）。

#### Scenario: LP 路径零回归
- **WHEN** `var_types` 全为 Continuous
- **THEN** 仍调用 `Highs_passLp`，行为与 v0.64.0 完全一致

#### Scenario: MILP 分派
- **WHEN** 任一 `var_types[i] != Continuous`（feature 启用）
- **THEN** 构建 integrality 数组并调用 `Highs_passMip`；默认构建（Mock）不编译该路径

## MODIFIED Requirements

### Requirement: solver-core crate 文档与描述

crate 文档与 `Cargo.toml` description 追加 v0.102.0 MILP 增量说明（既有 D1~D12 偏差表与模块结构零改动；`highs-ffi` feature 语义从"LP FFI"扩展为"LP/MILP FFI"）。

## REMOVED Requirements

无。

## 偏差声明（D1~D12，相对蓝图 §3/§4/§5）

| 编号 | 偏差 | 理由 |
|------|------|------|
| **D1** | 蓝图 `crates/solver_milp/` → `crates/ai/solver-milp/` | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；与 solver-core/solver-model/energy-lp-model 同 AI 子系统 |
| **D2** | 蓝图 `docs/phase2/milp_solver.md` → `docs/ai/milp-solver-design.md` | 记忆 §2.3.3 强制：文档按方向分类 |
| **D3** | 蓝图 `tests/milp_day_ahead.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.101.0 项目惯例，不新增 tests/ 文件 |
| **D4** | 不重定义 `MilpSolver`/`MilpModel`/`MilpSolution`/`SolveStatus` | 复用 v0.64.0 `Solver` trait + `LpProblem`（`var_types` 已含 Binary/Integer，即 MILP 模型）+ `SolveResult` + `SolveStatus` + `SolverError`（v0.66.0 D8/D9 复用先例）；蓝图 `Feasible` 变体由既有 `Suboptimal` 承载 |
| **D5** | 蓝图 `highs_ffi.rs` 独立模块 → solver-core `ffi.rs`/`highs.rs` 增量 `Highs_passMip` + 分派 | 避免重复 extern 声明 Highs_create/destroy（Karpathy Simplicity First）；FFI 单一归属；全部 feature-gated，默认构建零 unsafe 零改动 |
| **D6** | **蓝图 Bug 修正**：§4.5 `col_cost[base+1] = start_cost`（注释自称"V 启动成本"，但布局 base+1=U）→ 启动成本挂 V（base+2），U/W 系数 0 | 矩阵布局 [P,U,V,W] 与成本挂载自相矛盾，按注释语义修正（v0.66.0 D3 蓝图 Bug 修正先例） |
| **D7** | 蓝图 `num_constraints = t + n·t·3` 桩 → 完整标准 UC 约束集：t + 5nt + 2n(t−1) | 蓝图 §4.1 `min_up/min_down/ramp_up/ramp_down/init_status` 字段若不进约束则为死重；"骨架可用"要求约束构建真实完整（仅求解器以 Mock 替代） |
| **D8** | `build_model` 返回 `Result<LpProblem, SolverError>`（蓝图为裸返回） | load/price 长度校验，no_std 禁 panic（v0.66.0 D4 安全访问先例） |
| **D9** | 错误处理落地为**状态驱动降级链**：Infeasible/Unbounded/Error → relaxed 重建 → LP 松弛；`relax_count`/`lp_fallback_count` 计数器替代告警日志 | no_std `panic = "abort"` 无 panic 钩子可挂（蓝图 §4.4"panic 钩子捕获"不适用）；no_std 无 log crate，metric 字段化（v0.99.0 D12/v0.101.0 D7 先例）；Timeout/Suboptimal 视为可接受（蓝图 §4.4 超时返回当前最优可行解） |
| **D10** | 蓝图 `MilpSolver::set_time_limit` → 复用 `Solver::set_param("time_limit"/"mip_rel_gap", ...)` seam | 接口归并，不新增 trait；`DayAheadScheduler::new(time_limit_s, mip_rel_gap)` 持参、plan 前注入 |
| **D11** | 测试用 `MockSolver`；性能基准测**模型构建**耗时（10×24 < 1s） | 真实 HiGHS FFI 需编译 C 库，超出单元测试范围（v0.64.0 D7/v0.66.0 D7 先例）；蓝图 §6.3"10 机组 < 5s"真实求解性能留待硬件集成验证，设计文档声明口径 |
| **D12** | `String`/`Vec` = `alloc::*`；`f64::INFINITY` = `core::f64`；`interval_min` 参与爬坡约束（ramp·interval_min = MW/周期） | no_std 合规；爬坡率单位 MW/min 换算每周期 MW |

## 接口契约

```rust
// uc_model.rs
pub struct UcUnit {
    pub id: String, pub p_min: f64, pub p_max: f64,
    pub ramp_up: f64, pub ramp_down: f64,          // MW/min
    pub start_cost: f64,
    pub min_up: usize, pub min_down: usize,         // 周期数；0 按 1 处理（窗口=当期）
    pub init_status: bool,
}  // Debug/Clone
pub struct UnitCommitment {
    pub units: Vec<UcUnit>, pub periods: usize, pub interval_min: u32,
}  // Debug/Clone
impl UnitCommitment {
    pub fn new(units: Vec<UcUnit>, periods: usize, interval_min: u32) -> Self;
    pub fn num_vars(&self) -> usize;                                  // n·t·4
    pub fn var_index(&self, unit: usize, period: usize, kind: usize) -> usize; // (i·t+t)·4+k
    pub fn build_model(&self, load: &[f64], price: &[f64]) -> Result<LpProblem, SolverError>;
    pub fn build_model_relaxed(&self, load: &[f64], price: &[f64]) -> Result<LpProblem, SolverError>; // 跳过最小启停
}

// day_ahead.rs
pub struct UnitSchedule {
    pub unit_id: String, pub commitments: Vec<bool>, pub generation: Vec<f64>,
}  // Debug/Clone
pub struct DayAheadPlan {
    pub schedule: Vec<UnitSchedule>, pub total_cost: f64, pub solve_status: SolveStatus,
}  // Debug/Clone
pub struct DayAheadScheduler {
    pub time_limit_s: f64, pub mip_rel_gap: f64,
    pub relax_count: u64, pub lp_fallback_count: u64,   // 可观测（D9）
}
impl DayAheadScheduler {
    pub fn new(time_limit_s: f64, mip_rel_gap: f64) -> Self;
    pub fn plan(
        &mut self, uc: &UnitCommitment, load: &[f64], price: &[f64],
        solver: &mut dyn Solver, now_ms: u64,
    ) -> Result<DayAheadPlan, SolverError>;
    pub fn relax_lp(model: &LpProblem) -> LpProblem;   // Binary/Integer → Continuous（上界保持 1.0）
}

// solver-core ffi.rs 增量（#[cfg(feature = "highs-ffi")])
extern "C" { pub fn Highs_passMip(highs: HighsPtr, /* 同 Highs_passLp 参数..., */ integrality: *const c_int) -> c_int; }
```

## 测试规划（31 个）

| 文件 | 编号 | 数量 | 覆盖 |
|------|------|------|------|
| uc_model.rs | TU1~TU15 | 15 | UcUnit/new 字段 / 变量数 n·t·4 / var_types 抽查 / 目标系数（D6 修正）/ 变量边界 / 平衡 t 行 / 联动 2nt 行 / 爬坡 2n(t−1) 行 / 启停 nt 行（含 t=0 init_status）/ 最小启停 2nt 行 / 总行数+CSR 一致性 / Minimize / 长度校验 Err / relaxed 行数 |
| day_ahead.rs | TD16~TD31 | 16 | e2e 5×24（蓝图 §6.2）/ commitments 阈值解析 / generation 解析 / total_cost / status 透传 / Infeasible→relax（计数+plan 来源）/ relax 失败→LP / relax_lp 类型转换+上界保持 / Err→降级链 / Optimal 零降级 / unit_id 顺序 / 2×3 手工解映射 / 全链 LP Ok 双计数 / 模型构建性能（Instant，cfg(test) std 允许）/ set_param 参数注入（记录型 stub）/ 三级全失败空 plan+末级状态 |
