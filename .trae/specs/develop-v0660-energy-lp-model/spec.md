# v0.66.0 能源调度 LP 模型 Spec

## Why

v0.65.0 完成了优化问题建模框架（`OptProblem` DSL + Builder + `compile()` → CSR 矩阵），但缺少**领域模型层**：储能调度的标准 LP 模型（功率平衡 / SOC 动态 / 爬坡 / 容量约束）。v0.66.0 基于上一层 DSL 构建能源调度 LP 模型，为 v0.67.0 安全校验器与 v0.68.0 调度执行器提供可求解的领域模型。

## What Changes

- **ADDED** 新 crate `eneros-energy-lp-model`（`crates/ai/energy-lp-model/`）
- **ADDED** `ScheduleConfig` 调度参数配置（时段数 / PCS 功率 / 电池容量 / SOC 上下限 / 爬坡率 / 效率 / 电价曲线 / 负荷曲线）
- **ADDED** `EnergyScheduleModel` 调度模型构建器（自动创建决策变量 + 添加约束 + 设置目标函数 + 编译 + 解析结果）
- **ADDED** `ScheduleEntry` / `ScheduleResult` 调度结果类型
- **ADDED** 3 类约束：SOC 动态约束 / 爬坡约束 / SOC 初终值约束
- **ADDED** 目标函数：最大化收益 = Σ (price·discharge - price·charge)·dt
- **MODIFIED** workspace `members` 列表新增 `crates/ai/energy-lp-model`
- **MODIFIED** workspace 版本号 `0.65.0` → `0.66.0`

## Impact

- Affected specs: v0.65.0 (OptProblem DSL 复用)、v0.64.0 (Solver/LpProblem/SolveResult 复用)
- Affected code: 根 `Cargo.toml`、`Makefile`、`.github/workflows/ci.yml`、`ci/src/gate.rs`
- 新增 crate 位置：`crates/ai/energy-lp-model/`（AI 子系统，项目规则 §2.3.1）

## 偏差声明（D1~D12，Karpathy "Think Before Coding"）

| 偏差 | 蓝图原文 | 本版本处理 | 理由 |
|------|---------|-----------|------|
| **D1** | `self.problem = std::mem::take(&mut self.problem).maximize(obj);`（蓝图 line 13851） | 改用 `core::mem::take` | no_std 合规：`std::mem` 不可用，`core::mem::take` 提供相同功能 |
| **D2** | `format!("charge_{}", t)` 等（蓝图 line 13720 等） | 依赖 `extern crate alloc` 的 `alloc::format!` 宏（prelude 自动可用） | no_std 合规：`alloc::format!` 在 `extern crate alloc` 后通过 prelude 可用 |
| **D3** | **蓝图 Bug**：`expr.add_term(self.discharge_var_idx[t], eff_d * dt / cap * cap);`（蓝图 line 13782） | 改为 `expr.add_term(self.discharge_var_idx[t], dt / eff_d);` | **数学错误**：根据 §5 公式 `soc[t] = soc[t-1] + (charge[t]·η_c - discharge[t]/η_d)·dt`，放电项系数应为 `+1/η_d·dt = dt/η_d`，而非 `η_d·dt`。蓝图原式 `η_d·dt` 方向反了（乘以效率而非除以），且 `/cap * cap` 相互抵消是无意义操作。SOC 变量已用 kWh 单位（bounds 为 `soc_min*cap` ~ `soc_max*cap`），无需 cap 归一化 |
| **D4** | `result.solution[self.charge_var_idx[t]]` 直接索引（蓝图 line 13864） | 改用 `result.solution.get(idx).copied().unwrap_or(0.0)` | no_std 环境 panic 不可恢复，安全访问避免越界 panic |
| **D5** | 前置依赖列出 v0.52.0 四遥数据模型（蓝图 §2） | **不引入 v0.52.0 crate 依赖** | `ScheduleConfig` 自带 `price: Vec<f64>` 与 `load_demand: Option<Vec<f64>>`，数据由调用方填充，与 telemetry-model 解耦。Karpathy "Simplicity First"：不为未使用的依赖引入耦合 |
| **D6** | 蓝图未明确 crate 位置 | `crates/ai/energy-lp-model/` | 项目规则 §2.3.1：AI 子系统 crate 归入 `crates/ai/` |
| **D7** | 蓝图 §6.2 场景测试"谷充峰放求解" | 用 `MockSolver`（v0.64.0 默认实现）做端到端验证 | 与 v0.64.0/v0.65.0 一致：真实 HiGHS 需 `highs-ffi` feature + C 库链接，MockSolver 已实现 `Solver` trait 足以验证 DSL → compile → solve → parse 管道 |
| **D8** | 蓝图重定义 `LpProblem`/`SolverError`/`SolveResult`/`SolveStatus` | 复用 v0.64.0 `eneros-solver-core` 的类型（`use eneros_solver_core::*`） | 避免类型重定义导致 `compile()` 返回值不匹配。v0.64.0 已定义全部所需类型 |
| **D9** | 蓝图重定义 `OptProblem`/`VarBuilder`/`LinearExpr`/`Constraint` | 复用 v0.65.0 `eneros-solver-model` 的类型 | 同 D8 理由 |
| **D10** | 蓝图 `ScheduleConfig`/`ScheduleEntry`/`ScheduleResult` 派生 `Debug` + `Clone` | 保持一致，不额外派生 `PartialEq` | Karpathy "Simplicity First"：当前测试不需要 `PartialEq`，避免过早添加 |
| **D11** | 蓝图未声明 `[features]` | 不声明 `[features]` | 纯 Rust，无 FFI，无 feature gate |
| **D12** | 蓝图 `SafetyRule: Send + Sync`（蓝图 §4.1 line 13987） | **不适用**（该 trait 属于 v0.67.0 安全校验器，非 v0.66.0） | v0.66.0 不实现 `SafetyRule`，仅为 `EnergyScheduleModel` 领域模型 |

## ADDED Requirements

### Requirement: ScheduleConfig 调度参数配置

系统 SHALL 提供 `ScheduleConfig` 结构体，包含调度时段数、时段时长、PCS 功率、电池容量、SOC 上下限、初始/终值 SOC、充放电爬坡率、充放电效率、电价曲线、负荷曲线。

#### Scenario: 默认配置
- **WHEN** 调用 `ScheduleConfig::default()`
- **THEN** 返回 96 时段 / 0.25h / 100kW PCS / 200kWh 电池 / SOC 0.1~0.9 / 初始 0.5 / 爬坡 50kW / 效率 0.95 / 平价 0.5 元/kWh

### Requirement: EnergyScheduleModel 调度模型构建器

系统 SHALL 提供 `EnergyScheduleModel`，在 `new(config)` 时自动创建 3×n 决策变量（charge/discharge/soc）、添加 SOC 动态约束、爬坡约束、SOC 初值约束（可选终值）、设置最大化收益目标函数。

#### Scenario: 构建 96 时段模型
- **WHEN** `EnergyScheduleModel::new(ScheduleConfig::default())`
- **THEN** 模型包含 288 变量（96 charge + 96 discharge + 96 soc）、SOC 动态约束 95 条、爬坡约束 190 条（充放电各 95）、SOC 初值约束 1 条

#### Scenario: 编译为 LpProblem
- **WHEN** 调用 `model.compile()`
- **THEN** 返回 `Ok(LpProblem)`，CSR 矩阵 `row_start.len() == num_constraints + 1`

### Requirement: ScheduleResult 调度结果解析

系统 SHALL 提供 `parse_result(&SolveResult) -> ScheduleResult`，从求解结果提取各时段充放电功率、SOC 百分比、收益。

#### Scenario: 解析最优解
- **WHEN** MockSolver 返回 `SolveResult { status: Optimal, solution: vec![...], .. }`
- **THEN** `parse_result` 返回 `ScheduleResult`，`solve_status == Optimal`，`schedule.len() == num_periods`

## MODIFIED Requirements

### Requirement: Workspace members

根 `Cargo.toml` 的 `members` 列表 SHALL 在 `crates/ai/solver-model` 之后添加 `crates/ai/energy-lp-model`。

### Requirement: Workspace 版本号

根 `Cargo.toml` 的 `[workspace.package].version` SHALL 从 `0.65.0` 更新为 `0.66.0`。

## REMOVED Requirements

### Requirement: v0.52.0 crate 依赖
**Reason**: `ScheduleConfig` 自带数据字段，与 telemetry-model 解耦（D5）
**Migration**: 调用方负责从四遥数据填充 `ScheduleConfig.price` / `load_demand`
