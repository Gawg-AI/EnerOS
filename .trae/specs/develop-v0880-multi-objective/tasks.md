# Tasks

- [x] Task 1: 创建 `crates/agents/energy-market-agent/src/multi_objective.rs` — 数据结构（Objective / WeightedSum / ParetoSolution / ParetoFront）+ 自由函数
  - [x] SubTask 1.1: `Objective` 枚举（4 变体 `Economy` / `BatteryLife` / `Safety` / `Carbon`），派生 `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default`（`#[default]` on `Economy`），每变体中文 doc
  - [x] SubTask 1.2: `WeightedSum` 结构体（字段 `weights: BTreeMap<Objective, f32>`，pub），派生 `Debug, Clone, Default`
  - [x] SubTask 1.3: `WeightedSum::new()` / `set(&mut self, obj, w)` / `get(&self, obj) -> f32`（缺失 0.0）/ `normalized(&self) -> BTreeMap<Objective, f32>`（D10：任一 NaN/负值/总和≤0/非有限 → 4 目标各 0.25；否则归一化总和 1）
  - [x] SubTask 1.4: `ParetoSolution` 结构体（2 字段：`objectives: BTreeMap<Objective, f32>` / `plan: DispatchPlan`），派生 `Debug, Clone`
  - [x] SubTask 1.5: `ParetoFront` 结构体（字段 `solutions: Vec<ParetoSolution>`，pub），派生 `Debug, Clone, Default`
  - [x] SubTask 1.6: `objective_costs(obj: Objective, caps: &[(u64, DeviceCapability)]) -> Vec<f64>` — Economy/Carbon=`1.0-eff`；BatteryLife=`1.0/p_max`（p_max≤0→1.0）；Safety=`1.0/ramp_rate`（ramp≤0→1.0）（D8）
  - [x] SubTask 1.7: `normalize_costs(costs: &mut [f64])` — 除以最大值；max≤0 或全非有限 → 全置 0.0（D9）
  - [x] SubTask 1.8: 中文模块文档注释（v0.88.0 多目标优化 + 偏差 D1/D2/D3/D8/D9/D10 引用）；`use alloc::collections::BTreeMap;` 等；无 std/async/panic!/unsafe/todo!/unimplemented!/HashMap

- [x] Task 2: 在 `multi_objective.rs` 添加 `#[cfg(test)] mod tests` 单元测试 T121~T144
  - [x] SubTask 2.1: T121 — `Objective::default() == Economy`；4 变体 Debug 非空
  - [x] SubTask 2.2: T122 — 4 变体互不相等（6 对 assert_ne）
  - [x] SubTask 2.3: T123 — `Objective` 作 BTreeMap 键：插入 4 目标后 keys 顺序为 derive Ord 顺序（Economy < BatteryLife < Safety < Carbon）
  - [x] SubTask 2.4: T124 — `Objective` Copy 可复制
  - [x] SubTask 2.5: T125 — `WeightedSum::new()`：`weights.is_empty()`；`get(Economy) == 0.0`
  - [x] SubTask 2.6: T126 — `set(Economy, 2.0)` → `get(Economy)==2.0`；同 obj 重复 set 覆盖
  - [x] SubTask 2.7: T127 — `normalized()` 正常归一化：E=2.0/B=1.0/S=1.0/C 缺失 → [0.5, 0.25, 0.25, 0.0]（f32 EPSILON 容差）
  - [x] SubTask 2.8: T128 — `normalized()` 含 NaN → 4 目标各 0.25
  - [x] SubTask 2.9: T129 — `normalized()` 含负值（如 E=-1.0, B=2.0）→ 4 目标各 0.25
  - [x] SubTask 2.10: T130 — `normalized()` 全零 / 空 WeightedSum → 4 目标各 0.25；返回值含全部 4 键
  - [x] SubTask 2.11: T131 — `WeightedSum` Clone 后 get 一致
  - [x] SubTask 2.12: T132 — `ParetoSolution` 构造：objectives 含 4 键 + plan 字段访问
  - [x] SubTask 2.13: T133 — `ParetoSolution` Clone 后 objectives 与 plan 一致
  - [x] SubTask 2.14: T134 — `ParetoFront::default()` solutions 空；显式构造 solutions len
  - [x] SubTask 2.15: T135 — `objective_costs(Economy)`：eff 0.9/0.8 → [0.1, 0.2]（容差 1e-6，f32→f64）
  - [x] SubTask 2.16: T136 — `objective_costs(BatteryLife)`：p_max 5.0/10.0 → [0.2, 0.1]；`objective_costs(Safety)`：ramp 1.0/2.0 → [1.0, 0.5]
  - [x] SubTask 2.17: T137 — 退化值：p_max=0.0 → BatteryLife cost=1.0；ramp_rate=0.0 → Safety cost=1.0
  - [x] SubTask 2.18: T138 — `objective_costs` 空 caps → 空 Vec
  - [x] SubTask 2.19: T139 — `normalize_costs([0.1, 0.2])` → [0.5, 1.0]（max 归一化）
  - [x] SubTask 2.20: T140 — `normalize_costs([0.0, 0.0])` → 全 0.0（max≤0 兜底）；`normalize_costs([NaN, NaN])` → 全 0.0
  - [x] SubTask 2.21: T141 — `generate_weight_sample(0, 4)` 与重复调用结果一致（确定性）
  - [x] SubTask 2.22: T142 — `generate_weight_sample` 不同 i（0 vs 1，samples=4）产生不同权重组合
  - [x] SubTask 2.23: T143 — `generate_weight_sample(0, 1)`：归一化后 4 目标总和 == 1.0
  - [x] SubTask 2.24: T144 — `eval_plan_objectives`：2 设备 plan（sp=3.0/2.0，eff=0.9/0.8）→ Economy 值 == 0.1*3.0+0.2*2.0（容差 1e-4）；返回含全部 4 键

- [x] Task 3: 在 `multi_objective.rs` 追加 `MultiObjectiveOptimizer` + `weighted` + `pareto` + `filter_dominated` + 私有 `build_weighted_lp`
  - [x] SubTask 3.1: `MultiObjectiveOptimizer` 结构体（3 字段全 pub：`pool: DevicePool` / `solver: Box<dyn Solver>` / `last_setpoints: BTreeMap<u64, f32>`）+ `new(pool, solver)`（last_setpoints 空）
  - [x] SubTask 3.2: `weighted(&mut self, target, socs, w, now_ms) -> Result<DispatchPlan, DispatchError>` 严格按序 9 步（目标校验 → 陈旧清理 → SOC 过滤 → 空池校验 → 加权目标构建 → LP 构建 → 求解/兜底 → 更新 last_setpoints → 返回 DispatchPlan{timestamp, assignments, total_power=Σsetpoints, objective_value}）
  - [x] SubTask 3.3: 加权目标构建：`w.normalized()` + 4 目标 `objective_costs` + `normalize_costs` → `combined_i = Σ_obj w_obj * norm_cost_obj_i`
  - [x] SubTask 3.4: 私有 `build_weighted_lp(eligible, target, last_setpoints, objective: Vec<f64>) -> LpProblem`（D7 自包含：变量 `p_{id}` / 界 [p_min,p_max] / Continuous / sense Minimize / 平衡行 rhs==target / 爬坡行 prev±ramp，CSR 结构与 v0.87.0 一致）
  - [x] SubTask 3.5: 求解分支：`Ok` 且 `Optimal` 且 `solution.len()==n` → setpoint clamp [p_min,p_max] + `objective_value = result.objective_value as f32`；否则 `equal_split(target, &eligible)` + `objective_value = 0.0`
  - [x] SubTask 3.6: `pareto(&mut self, target, socs, samples, now_ms) -> Result<ParetoFront, DispatchError>`：samples==0 → Ok 空 front；循环 `generate_weight_sample` → `weighted`（Err 透传）→ `eval_plan_objectives` → 收集 → `filter_dominated` → Ok
  - [x] SubTask 3.7: `filter_dominated(solutions: Vec<ParetoSolution>) -> Vec<ParetoSolution>`（D14：O(n²) 两两比较；A 支配 B ⟺ A 全目标 ≤ B 且至少一 <；相同向量保留先者；保持原顺序）
  - [x] SubTask 3.8: `eval_plan_objectives(plan: &DispatchPlan, pool: &DevicePool) -> BTreeMap<Objective, f32>` 公开自由函数（D13：4 目标 `Σ cost_obj_i * setpoint_i` 原始值；assignment 设备不在 pool → 跳过该条）
  - [x] SubTask 3.9: use 仅 `alloc::boxed::Box` / `alloc::collections::BTreeMap` / `alloc::vec::Vec` / `alloc::vec!` / `alloc::format!` + `crate::device_pool::{DeviceCapability, DevicePool}` + `crate::multi_dispatch::{equal_split, DeviceAssignment, DispatchError, DispatchPlan}` + `eneros_solver_core::{problem::{ConstraintMatrix, LpProblem, ObjectiveSense, VarType}, result::SolveStatus, solver::Solver}`；主代码无 unwrap/unsafe/std

- [x] Task 4: 在 `multi_objective.rs` tests 追加 T145~T160（含 `FixedSolver` 测试辅助）
  - [x] SubTask 4.1: 测试辅助 `FixedSolver`（impl `Solver`：字段 `result: Option<SolveResult>` / `fail: bool`；fail → `Err(SolverError::RunFailed(-1))`；否则返回预设或 `SolveResult::optimal(0.0, vec![])`；name/version/set_param/status 简单实现）—— LP 结构验证经 `FixedSolver` 记录不可行（Box 移动后不可读），故 LP 系数验证直接调用同模块私有 `build_weighted_lp`
  - [x] SubTask 4.2: T145 — `weighted` target NaN / INFINITY → `Err(InvalidTarget)`（solver 未调用）
  - [x] SubTask 4.3: T146 — 空 pool → `Err(EmptyPool)`；全部 soc=0.0 → `Err(EmptyPool)`
  - [x] SubTask 4.4: T147 — SOC 过滤：设备 1 soc=0.0 跳过 + 设备 2 soc=0.5 → assignments 仅设备 2（solution 长度 1）
  - [x] SubTask 4.5: T148 — happy path：2 设备 + FixedSolver Optimal [3.0, 2.0] objective 0.4 → assignments [id=1 sp=3.0, id=2 sp=2.0] / total_power==5.0 / objective_value==0.4 / timestamp==now_ms / mode Auto / ids 有序 / last_setpoints {1:3.0, 2:2.0}
  - [x] SubTask 4.6: T149 — 组合系数：`build_weighted_lp` objective[i] == 0.5*norm_economy[i] + 0.5*norm_battery[i]（E=1.0/B=1.0 权重，容差 1e-6）；`variables` 名 `p_1`/`p_2`，`sense==Minimize`
  - [x] SubTask 4.7: T150 — 平衡行：`rhs_lower[0]==rhs_upper[0]==target`；首次 num_rows==1；有 last_setpoints（2 设备）→ num_rows==3 + 爬坡行 rhs（prev=3.0/ramp=1.0 → [2.0, 4.0]）
  - [x] SubTask 4.8: T151 — solver `fail=true` → fallback：equal_split clamp + `objective_value==0.0` + `Ok(plan)` + total_power==Σsetpoints
  - [x] SubTask 4.9: T152 — solver 返回 `Infeasible` → fallback；solver 返回 Optimal 但 solution 空 → fallback
  - [x] SubTask 4.10: T153 — solver 解超 p_max（9.0 > 5.0）→ clamp 5.0；均权（空 WeightedSum）路径也正确（默认 0.25 各）
  - [x] SubTask 4.11: T154 — `filter_dominated`：A(1,2)/B(2,1)/C(3,3)（仅 2 目标键）→ C 被移除，保留 A、B（顺序保持）
  - [x] SubTask 4.12: T155 — `filter_dominated`：完全相同向量（A、B 同值）→ 保留先出现者；空输入 → 空输出
  - [x] SubTask 4.13: T156 — `pareto` samples=0 → `Ok(front)` solutions 空
  - [x] SubTask 4.14: T157 — `pareto` happy path：2 设备 + FixedSolver Optimal，samples=4 → `solutions.len() <= 4`；每解 objectives 含全部 4 键
  - [x] SubTask 4.15: T158 — `pareto` 空 pool → `Err(EmptyPool)` 透传
  - [x] SubTask 4.16: T159 — 3 目标权衡（§6.2）：同 2 设备，weights 分别 {E only} vs {B only} → `build_weighted_lp` objective 向量不同（证明权重影响目标）
  - [x] SubTask 4.17: T160 — 权重变更兼容（§6.4）：同一 optimizer 连续两次 weighted 用不同权重 → 均 Ok，last_setpoints 正确滚动（第二次 LP num_rows==3）

- [x] Task 5: 修改 `crates/agents/energy-market-agent/src/lib.rs` — 追加 1 个 `pub mod` + 重导出（surgical）
  - [x] SubTask 5.1: 追加 `pub mod multi_objective;`（既有 5 私有 mod + v0.85.0 3 pub mod + v0.86.0 1 pub mod + v0.87.0 2 pub mod 全部保留）
  - [x] SubTask 5.2: 追加 `pub use multi_objective::{eval_plan_objectives, filter_dominated, generate_weight_sample, normalize_costs, objective_costs, MultiObjectiveOptimizer, Objective, ParetoFront, ParetoSolution, WeightedSum};`
  - [x] SubTask 5.3: 顶部文档注释追加 `# v0.88.0 多目标优化` 段落（核心类型列表 + D1~D14 偏差表，从 spec.md 复制）
  - [x] SubTask 5.4: 不修改任何 v0.72.0/v0.85.0/v0.86.0/v0.87.0 既有代码行；既有 144 tests 保留
  - [x] SubTask 5.5: `lib.rs` 无 std/async/panic!/unsafe

- [x] Task 6: 修改 `crates/agents/energy-market-agent/Cargo.toml` — 更新 description（surgical）
  - [x] SubTask 6.1: `description` 末尾追加 ` + v0.88.0 多目标优化 (经济/寿命/安全/碳排加权和 + Pareto 前沿, no_std)`
  - [x] SubTask 6.2: `[dependencies]` 段不变（复用既有 eneros-solver-core，无新依赖，D3/D6）
  - [x] SubTask 6.3: workspace members 列表不变

- [x] Task 7: 创建配置文件 `configs/multi_objective.toml`
  - [x] SubTask 7.1: `[weights]` 段：economy / battery_life / safety / carbon 4 个 f32 默认权重（安全权重最高，蓝图 §7.3，如 economy=0.3 / battery_life=0.2 / safety=0.4 / carbon=0.1）
  - [x] SubTask 7.2: `[pareto]` 段：`samples = 50`（蓝图 §6.3 Pareto(50) < 5s）
  - [x] SubTask 7.3: 中文注释：权重非法→均权规则（D10）/ 目标成本系数定义（D8）/ 归一化说明（D9）/ 安全权重最高验收（蓝图 §7.3）

- [x] Task 8: 创建设计文档 `docs/agents/multi-objective-design.md`
  - [x] SubTask 8.1: 12 章节完整（版本目标 / 前置依赖 / 交付物清单 / 数据结构 / 接口设计 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险与坑点 / 偏差声明）
  - [x] SubTask 8.2: Mermaid 图 1：蓝图 §4.3 核心算法（多目标 → 加权→单目标 LP / Pareto→多组加权采样→支配过滤 → 输出方案/决策者选择）
  - [x] SubTask 8.3: Mermaid 图 2：weighted 决策流程（目标校验 → 陈旧清理 → SOC 过滤 → 加权目标构建 → LP → solve 成功 ?/fallback → clamp → 更新 last_setpoints → Ok）
  - [x] SubTask 8.4: D1~D14 偏差声明表完整（从 spec.md 复制）
  - [x] SubTask 8.5: 前置依赖引用 v0.87.0 多设备调度 + v0.66.0 LP + v0.64.0 Solver trait
  - [x] SubTask 8.6: 性能目标（加权 < 500ms / Pareto(50) < 5s，蓝图 §6.3/§7.2，标注"集成阶段验收，本版本仅算法骨架"）
  - [x] SubTask 8.7: 下游引用 v0.92.0 仲裁（多目标基础）
  - [x] SubTask 8.8: 选型对比表（加权和 ⭐ 实时 / ε-约束 备选 / NSGA-II 离线，蓝图 §5.1）
  - [x] SubTask 8.9: 目标成本系数定义表（D8 四目标公式 + 物理含义）+ 归一化说明（D9 + 蓝图 §8.5 坑点）+ 安全权重最高验收（蓝图 §7.3）

- [x] Task 9: 版本同步根目录文件
  - [x] SubTask 9.1: 根 `Cargo.toml` `[workspace.package] version = "0.87.0"` → `"0.88.0"`（members 不变）
  - [x] SubTask 9.2: `Makefile` `# Version: v0.88.0` + `VERSION := 0.88.0`
  - [x] SubTask 9.3: `.github/workflows/ci.yml` `# Version: v0.88.0`
  - [x] SubTask 9.4: `ci/src/gate.rs` clippy 段 + test 段注释追加 `+ v0.88.0 多目标优化：Objective / WeightedSum / ParetoFront / ParetoSolution / MultiObjectiveOptimizer / objective_costs / normalize_costs / generate_weight_sample / filter_dominated / eval_plan_objectives`

- [x] Task 10: 构建校验（§2.4.2 C6~C11）
  - [x] SubTask 10.1: `cargo metadata --format-version 1` 成功
  - [x] SubTask 10.2: `cargo test -p eneros-energy-market-agent` 全部通过（144 既有 + T121~T160 40 新增 = 184 tests，0 failures）
  - [x] SubTask 10.3: `cargo build -p eneros-energy-market-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] SubTask 10.4: `cargo fmt -p eneros-energy-market-agent -- --check` 通过
  - [x] SubTask 10.5: `cargo clippy -p eneros-energy-market-agent --all-targets -- -D warnings` 无 warning
  - [x] SubTask 10.6: `cargo deny check licenses bans sources` 通过（无新依赖；advisories 视网络可用性）
  - [x] SubTask 10.7: 回归 — `cargo test -p eneros-grid-agent`（130 + 1 doctest）/ `cargo test -p eneros-device-agent`（24）
  - [x] SubTask 10.8: 回归 — `cargo test -p eneros-tsn-time`（84）/ `cargo test -p eneros-agent-bus-dds`（63）

- [x] Task 11: 修复 C140 — `filter_dominated` 增加 NaN/非有限前置防御
  - [x] SubTask 11.1: `filter_dominated`（或其目标向量提取处）对非有限目标值做确定性防御（D14 语义：最小化下非有限视为 +∞ 最差值），禁止静默误判支配
  - [x] SubTask 11.2: 追加 1 个回归测试（含 NaN 注入 objectives 后支配判定确定、无 panic、语义正确）
  - [x] SubTask 11.3: 回归 `cargo test -p eneros-energy-market-agent`（185 tests 全过）+ clippy + fmt + aarch64 build

# Task Dependencies

- [Task 2] depends on [Task 1]
- [Task 3] depends on [Task 1]
- [Task 4] depends on [Task 3]
- [Task 5] depends on [Task 3]
- [Task 6] 独立（可与 1~4 并行）
- [Task 7, Task 8] 独立（可与 1~6 并行）
- [Task 9] depends on [Task 5, Task 6]
- [Task 10] depends on [Task 5, Task 9]

# 并行执行计划

- **Sub-Agent A**：Task 1 + Task 2 + Task 3 + Task 4（同 crate 源文件，串行单 agent 保证一致性）
- **Sub-Agent B**：Task 7 + Task 8（configs + docs，与 A 并行）
- **Sub-Agent C**：Task 5 + Task 6 + Task 9（lib.rs + Cargo.toml + 版本同步，待 A 完成后启动）
- **主 agent**：Task 10（全部完成后统一构建校验）
