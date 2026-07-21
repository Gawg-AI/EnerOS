# Tasks

- [x] Task 1: Workspace 同步
  - [x] 根 `Cargo.toml` 版本号 `0.65.0` → `0.66.0`
  - [x] members 添加 `crates/ai/energy-lp-model`（置于 `crates/ai/solver-model` 之后）
  - [x] 验证：`cargo metadata --format-version 1` 成功（待 crate 骨架创建后）

- [x] Task 2: 创建 `eneros-energy-lp-model` crate 骨架
  - [x] 新建 `crates/ai/energy-lp-model/Cargo.toml`，package name = `eneros-energy-lp-model`
  - [x] dependencies 添加 `eneros-solver-model = { path = "../solver-model" }`（D9）+ `eneros-solver-core = { path = "../solver-core" }`（D8）
  - [x] 无 `[features]` 段（D11：纯 Rust，无 FFI）
  - [x] no_std 配置：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
  - [x] 新建 `src/lib.rs`，模块声明：config / model / result
  - [x] lib.rs 包含 D1~D12 偏差声明表
  - [x] 验证：`cargo metadata --format-version 1` 成功

- [x] Task 3: 实现 `config.rs` — ScheduleConfig 调度参数配置
  - [x] `ScheduleConfig` 结构体：num_periods / period_hours / pcs_power_kw / battery_capacity_kwh / soc_min / soc_max / soc_init / soc_final: Option<f64> / charge_ramp_kw / discharge_ramp_kw / charge_efficiency / discharge_efficiency / price: Vec<f64> / load_demand: Option<Vec<f64>>
  - [x] 派生 `Debug` + `Clone`（D10）
  - [x] `ScheduleConfig::default() -> Self`（96 时段 / 0.25h / 100kW / 200kWh / SOC 0.1~0.9 / init 0.5 / ramp 50kW / eff 0.95 / price=0.5×96）
  - [x] 验证：编译通过

- [x] Task 4: 实现 `result.rs` — ScheduleEntry + ScheduleResult 调度结果类型
  - [x] `ScheduleEntry` 结构体：period: usize / charge_power_kw: f64 / discharge_power_kw: f64 / net_power_kw: f64 / soc_pct: f64 / revenue_yuan: f64
  - [x] `ScheduleResult` 结构体：schedule: Vec<ScheduleEntry> / total_revenue_yuan: f64 / objective_value: f64 / solve_status: SolveStatus（复用 v0.64.0，D8）
  - [x] 两者派生 `Debug` + `Clone`（D10）
  - [x] 验证：编译通过

- [x] Task 5: 实现 `model.rs` — EnergyScheduleModel 调度模型构建器
  - [x] `EnergyScheduleModel` 结构体：config: ScheduleConfig / problem: OptProblem / charge_var_idx: Vec<usize> / discharge_var_idx: Vec<usize> / soc_var_idx: Vec<usize>
  - [x] `EnergyScheduleModel::new(config: ScheduleConfig) -> Self` — 自动创建 3×n 决策变量 + 添加约束 + 设置目标函数
  - [x] `add_soc_dynamics_constraints(&mut self)` — SOC 动态约束：`soc[t] - soc[t-1] - charge[t]·η_c·dt + discharge[t]·(dt/η_d) == 0`（D3 修正：放电系数为 `dt/η_d` 而非 `η_d·dt`）
  - [x] `add_ramp_constraints(&mut self)` — 爬坡约束：`charge[t] - charge[t-1] <= ramp_c`、`discharge[t] - discharge[t-1] <= ramp_d`
  - [x] `add_soc_init_constraint(&mut self)` — 初始 SOC 约束：`soc[0] == soc_init·capacity`
  - [x] `add_soc_final_constraint(&mut self, soc_final: f64)` — 终值 SOC 约束：`soc[n-1] == soc_final·capacity`
  - [x] `set_objective(&mut self)` — 目标函数：`max Σ (price[t]·discharge[t] - price[t]·charge[t])·dt`；用 `core::mem::take`（D1）绕过借用检查
  - [x] `compile(&self) -> Result<LpProblem, SolverError>` — 编译为 LP 问题（复用 v0.65.0 `OptProblem::compile`，D9）
  - [x] `parse_result(&self, result: &SolveResult) -> ScheduleResult` — 用 `result.solution.get(idx).copied().unwrap_or(0.0)` 安全访问（D4）
  - [x] 验证：编译通过

- [x] Task 6: 集成测试模块（lib.rs `#[cfg(test)] mod tests`）
  - [x] T1 ScheduleConfig::default 默认值（96 时段 / 100kW / 200kWh / SOC 0.1~0.9 / init 0.5）
  - [x] T2 ScheduleConfig 字段访问 + Clone
  - [x] T3 EnergyScheduleModel::new 构建 96 时段模型（变量数 288）
  - [x] T4 EnergyScheduleModel 变量索引正确（charge/discharge/soc 各 96 个）
  - [x] T5 EnergyScheduleModel::compile() 返回 Ok(LpProblem)
  - [x] T6 compile() 后 LpProblem 变量数 = 288
  - [x] T7 compile() 后 CSR row_start.len() == num_constraints + 1
  - [x] T8 SOC 动态约束数 = num_periods - 1（95 条）
  - [x] T9 爬坡约束数 = 2 × (num_periods - 1)（190 条）
  - [x] T10 SOC 初值约束数 = 1
  - [x] T11 SOC 终值约束（config.soc_final = Some 时存在）
  - [x] T12 目标函数 sense = Maximize
  - [x] T13 parse_result 从 SolveResult 提取 ScheduleResult（schedule.len() == num_periods）
  - [x] T14 parse_result soc_pct = soc / capacity
  - [x] T15 parse_result revenue_yuan = (discharge - charge)·price·dt
  - [x] T16 parse_result total_revenue_yuan = Σ revenue
  - [x] T17 端到端：Model::new → compile() → MockSolver.solve() → parse_result（D7）
  - [x] T18 端到端返回 ScheduleResult.solve_status = Optimal（MockSolver 默认）
  - [x] T19 小规模模型（4 时段）变量数 = 12、约束数正确
  - [x] T20 小规模模型 compile() + MockSolver 端到端
  - [x] T21 ScheduleEntry 字段访问
  - [x] T22 ScheduleResult 字段访问 + Clone
  - [x] 验证：`cargo test -p eneros-energy-lp-model` 全部通过

- [x] Task 7: 设计文档 `docs/ai/energy-lp-model-design.md`
  - [x] 12 章节：版本目标 / 架构定位 / ScheduleConfig 配置 / EnergyScheduleModel 构建器 / SOC 动态约束（D3 修正）/ 爬坡约束 / SOC 初终值约束 / 目标函数 / compile + parse_result / 错误处理 / no_std 合规 / 偏差声明
  - [x] 2 Mermaid 图：EnergyScheduleModel 类图 + new() 构建流程时序图
  - [x] D1~D12 偏差声明表
  - [x] LP 数学公式（决策变量 / 目标 / 约束方程）
  - [x] 文档位置在 `docs/ai/` 下（复用 v0.59.0~v0.65.0 创建的目录）

- [x] Task 8: 版本号同步 + gate.rs 注释更新
  - [x] `Makefile` 版本号 `0.65.0` → `0.66.0`
  - [x] `.github/workflows/ci.yml` 版本号 `0.65.0` → `0.66.0`
  - [x] `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-energy-lp-model` 说明
  - [x] 验证：`cargo build -p eneros-energy-lp-model` 通过

- [x] Task 9: 构建校验（§2.4.2 C6~C11）
  - [x] `cargo metadata --format-version 1` 成功
  - [x] `cargo test -p eneros-energy-lp-model` 全部通过（22 tests）
  - [x] `cargo build -p eneros-energy-lp-model --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
  - [x] `cargo fmt -p eneros-energy-lp-model -- --check` 格式通过
  - [x] `cargo clippy -p eneros-energy-lp-model --all-targets -- -D warnings` lint 通过
  - [x] `cargo deny check licenses bans sources` 安全扫描通过

- [x] Task 10: 更新 tasks.md + checklist.md 所有项 → [x]
  - [x] tasks.md 10 任务全部 [x]
  - [x] checklist.md 所有检查点全部 [x]

# Task Dependencies

- Task 2（crate 骨架）→ Task 1（metadata 验证需骨架）
- Task 3（config）独立（无外部依赖）
- Task 4（result）依赖 v0.64.0 SolveStatus（D8）
- Task 5（model）依赖 Task 3 + Task 4 + v0.65.0 OptProblem/VarBuilder/LinearExpr/Constraint（D9）+ v0.64.0 LpProblem/SolverError/SolveResult（D8）
- Task 6（集成测试）→ Task 3~5（测试依赖所有模块）+ v0.64.0 MockSolver（D7）
- Task 7（设计文档）可与 Task 5~6 并行（独立工作）
- Task 8（版本同步）→ Task 7（版本同步在功能完成后）
- Task 9（构建校验）→ Task 8
- Task 10（更新文档）→ Task 9（全部校验通过后）

# Parallelizable Work

- Task 3（config）+ Task 4（result）可并行（无相互依赖）
- Task 5（model）依赖 Task 3 + Task 4
- Task 6（集成测试）依赖 Task 5
- Task 7（设计文档）可与 Task 5~6 并行（独立工作）
