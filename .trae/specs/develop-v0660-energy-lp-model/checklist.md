# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.66.0`
- [x] C2 members 列表已添加 `crates/ai/energy-lp-model`（置于 `crates/ai/solver-model` 之后）
- [x] C3 `cargo metadata --format-version 1` 成功

## Crate 骨架
- [x] C4 `crates/ai/energy-lp-model/Cargo.toml` 存在，package name = `eneros-energy-lp-model`
- [x] C5 dependencies 包含 `eneros-solver-model = { path = "../solver-model" }`（D9）+ `eneros-solver-core = { path = "../solver-core" }`（D8）
- [x] C6 **不声明** `[features]`（D11：纯 Rust，无 FFI）
- [x] C7 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C8 `src/lib.rs` 包含 D1~D12 偏差声明表
- [x] C9 模块声明：config / model / result

## config.rs — ScheduleConfig
- [x] C10 `ScheduleConfig` 结构体包含 14 字段：num_periods / period_hours / pcs_power_kw / battery_capacity_kwh / soc_min / soc_max / soc_init / soc_final: Option<f64> / charge_ramp_kw / discharge_ramp_kw / charge_efficiency / discharge_efficiency / price: Vec<f64> / load_demand: Option<Vec<f64>>
- [x] C11 派生 `Debug` + `Clone`（D10）
- [x] C12 `ScheduleConfig::default() -> Self`（96 时段 / 0.25h / 100kW / 200kWh / SOC 0.1~0.9 / init 0.5 / ramp 50kW / eff 0.95 / price=[0.5;96] / load=None）
- [x] C13 单元测试：默认值字段验证

## result.rs — ScheduleEntry + ScheduleResult
- [x] C14 `ScheduleEntry` 结构体：period: usize / charge_power_kw: f64 / discharge_power_kw: f64 / net_power_kw: f64 / soc_pct: f64 / revenue_yuan: f64
- [x] C15 `ScheduleResult` 结构体：schedule: Vec<ScheduleEntry> / total_revenue_yuan: f64 / objective_value: f64 / solve_status: SolveStatus（复用 v0.64.0，D8）
- [x] C16 两者派生 `Debug` + `Clone`（D10）
- [x] C17 单元测试：字段访问 + Clone

## model.rs — EnergyScheduleModel
- [x] C18 `EnergyScheduleModel` 结构体：config / problem: OptProblem / charge_var_idx: Vec<usize> / discharge_var_idx: Vec<usize> / soc_var_idx: Vec<usize>
- [x] C19 `EnergyScheduleModel::new(config: ScheduleConfig) -> Self` — 自动创建 3×n 变量 + 添加约束 + 设置目标
- [x] C20 决策变量：charge[t] ∈ [0, pcs_power] / discharge[t] ∈ [0, pcs_power] / soc[t] ∈ [soc_min·cap, soc_max·cap]
- [x] C21 `add_soc_dynamics_constraints` — SOC 动态：`soc[t] - soc[t-1] - charge[t]·η_c·dt + discharge[t]·(dt/η_d) == 0`（D3 修正：dt/η_d 而非 η_d·dt）
- [x] C22 SOC 动态约束数 = num_periods - 1
- [x] C23 `add_ramp_constraints` — 爬坡：`charge[t] - charge[t-1] <= ramp_c`、`discharge[t] - discharge[t-1] <= ramp_d`
- [x] C24 爬坡约束数 = 2 × (num_periods - 1)
- [x] C25 `add_soc_init_constraint` — `soc[0] == soc_init·capacity`
- [x] C26 SOC 初值约束数 = 1
- [x] C27 `add_soc_final_constraint` — `soc[n-1] == soc_final·capacity`（仅 config.soc_final = Some 时）
- [x] C28 `set_objective` — `max Σ (price[t]·discharge[t] - price[t]·charge[t])·dt`；用 `core::mem::take`（D1）
- [x] C29 目标函数 sense = Maximize
- [x] C30 `compile(&self) -> Result<LpProblem, SolverError>`（复用 v0.65.0 OptProblem::compile，D9）
- [x] C31 `parse_result(&self, result: &SolveResult) -> ScheduleResult` — 用 `result.solution.get(idx).copied().unwrap_or(0.0)`（D4）
- [x] C32 parse_result：soc_pct = soc / capacity、revenue = (discharge - charge)·price·dt、total_revenue = Σ
- [x] C33 单元测试：new 构建后变量数 = 3 × num_periods

## 集成测试（lib.rs）
- [x] C34 T1 ScheduleConfig::default 默认值
- [x] C35 T2 ScheduleConfig 字段访问 + Clone
- [x] C36 T3 EnergyScheduleModel::new 构建 96 时段模型（变量数 288）
- [x] C37 T4 变量索引正确（charge/discharge/soc 各 96 个）
- [x] C38 T5 compile() 返回 Ok(LpProblem)
- [x] C39 T6 LpProblem 变量数 = 288
- [x] C40 T7 CSR row_start.len() == num_constraints + 1
- [x] C41 T8 SOC 动态约束数 = 95
- [x] C42 T9 爬坡约束数 = 190
- [x] C43 T10 SOC 初值约束数 = 1
- [x] C44 T11 SOC 终值约束（soc_final = Some 时存在）
- [x] C45 T12 目标函数 sense = Maximize
- [x] C46 T13 parse_result 提取 ScheduleResult（len == num_periods）
- [x] C47 T14 parse_result soc_pct = soc / capacity
- [x] C48 T15 parse_result revenue_yuan = (discharge - charge)·price·dt
- [x] C49 T16 parse_result total_revenue_yuan = Σ revenue
- [x] C50 T17 端到端：new → compile → MockSolver.solve → parse_result（D7）
- [x] C51 T18 端到端返回 solve_status = Optimal
- [x] C52 T19 小规模模型（4 时段）变量数 = 12
- [x] C53 T20 小规模模型 compile + MockSolver 端到端
- [x] C54 T21 ScheduleEntry 字段访问
- [x] C55 T22 ScheduleResult 字段访问 + Clone
- [x] C56 `cargo test -p eneros-energy-lp-model` 22/22 通过

## 设计文档
- [x] C57 `docs/ai/energy-lp-model-design.md` 存在
- [x] C58 12 章节完整
- [x] C59 2 Mermaid 图（EnergyScheduleModel 类图 + new() 构建流程时序图）
- [x] C60 D1~D12 偏差声明表
- [x] C61 LP 数学公式（决策变量 / 目标 / 约束方程）
- [x] C62 文档在 `docs/ai/` 下（符合目录规范）

## 版本同步
- [x] C63 `Makefile` 版本号 `0.66.0`
- [x] C64 `.github/workflows/ci.yml` 版本号 `0.66.0`
- [x] C65 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-energy-lp-model`

## 构建校验（§2.4.2 C6~C11）
- [x] C66 `cargo metadata --format-version 1` 成功
- [x] C67 `cargo test -p eneros-energy-lp-model` 全部通过（22 tests）
- [x] C68 `cargo build -p eneros-energy-lp-model --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C69 `cargo fmt -p eneros-energy-lp-model -- --check` 通过
- [x] C70 `cargo clippy -p eneros-energy-lp-model --all-targets -- -D warnings` 无 warning
- [x] C71 `cargo deny check licenses bans sources` 通过

## no_std 合规
- [x] C72 无 `use std::*`（仅 `alloc::*` / `core::*`）
- [x] C73 无 `panic!` / `todo!` / `unimplemented!`
- [x] C74 子模块不重复 `#![cfg_attr(not(test), no_std)]`
- [x] C75 无 `unsafe` 块
- [x] C76 用 `core::mem::take` 而非 `std::mem::take`（D1）

## 目录规范
- [x] C77 crate 在 `crates/ai/energy-lp-model/`（D6）
- [x] C78 跨 crate path 引用 `../solver-model` + `../solver-core`（相对路径）
- [x] C79 文档在 `docs/ai/` 下
- [x] C80 无根目录 crate（除 `ci/`）
- [x] C81 无垃圾文件（`target/` / `*.elf` / `*.bin` 被忽略）

## 依赖复用（D8/D9）
- [x] C82 复用 v0.65.0 `OptProblem` / `VarBuilder` / `LinearExpr` / `Constraint`（不重定义）
- [x] C83 复用 v0.64.0 `LpProblem` / `SolverError` / `SolveResult` / `SolveStatus`（不重定义）
- [x] C84 复用 v0.64.0 `MockSolver` 做端到端测试（D7）
- [x] C85 **不依赖** v0.52.0 telemetry-model crate（D5：ScheduleConfig 自带数据）

## 简化设计验证（Karpathy 原则）
- [x] C86 无 `Send + Sync` bounds（v0.66.0 无 trait，D12 不适用）
- [x] C87 无 `PartialEq` 派生（D10：当前测试不需要）
- [x] C88 无 `[features]` 段（D11：纯 Rust）
- [x] C89 无 Python 测试代码（D7：Rust MockSolver）
- [x] C90 修正蓝图 SOC 动态效率 bug（D3：dt/η_d 而非 η_d·dt）

## 蓝图 Bug 修正（D3）
- [x] C91 SOC 动态约束放电项系数 = `dt / discharge_efficiency`（非 `discharge_efficiency * dt`）
- [x] C92 无 `/cap * cap` 无意义操作（SOC 变量已用 kWh 单位）
- [x] C93 注释说明蓝图原式与修正理由

## 安全访问（D4）
- [x] C94 parse_result 用 `result.solution.get(idx).copied().unwrap_or(0.0)`
- [x] C95 不直接索引 `result.solution[idx]`
