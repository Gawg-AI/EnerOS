# Checklist

## Task 1: multi_objective.rs — 数据结构 + 自由函数
- [x] C1: `crates/agents/energy-market-agent/src/multi_objective.rs` 文件创建
- [x] C2: `Objective` 枚举 4 变体（Economy / BatteryLife / Safety / Carbon）
- [x] C3: `Objective` 派生 `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default`（`#[default]` on `Economy`）
- [x] C4: `WeightedSum` 结构体字段 `weights: BTreeMap<Objective, f32>`，pub
- [x] C5: `WeightedSum` 派生 `Debug, Clone, Default`
- [x] C6: `WeightedSum::new()` 返回空权重
- [x] C7: `WeightedSum::set(&mut self, obj, w)` 同 obj 覆盖
- [x] C8: `WeightedSum::get(&self, obj) -> f32` 缺失返回 0.0
- [x] C9: `WeightedSum::normalized()` — 正常归一化总和为 1；NaN/负/总和≤0/非有限 → 4 目标各 0.25
- [x] C10: `ParetoSolution` 结构体 2 字段（`objectives: BTreeMap<Objective, f32>` / `plan: DispatchPlan`）
- [x] C11: `ParetoSolution` 派生 `Debug, Clone`
- [x] C12: `ParetoFront` 结构体字段 `solutions: Vec<ParetoSolution>`，pub
- [x] C13: `ParetoFront` 派生 `Debug, Clone, Default`
- [x] C14: `objective_costs(Economy, caps)` 返回 `Vec<f64>`（cost_i = 1.0 - eff_i）
- [x] C15: `objective_costs(BatteryLife, caps)`（cost_i = 1.0/p_max_i，p_max≤0→1.0）
- [x] C16: `objective_costs(Safety, caps)`（cost_i = 1.0/ramp_rate_i，ramp≤0→1.0）
- [x] C17: `objective_costs(Carbon, caps)`（cost_i = 1.0 - eff_i，与 Economy 同）
- [x] C18: `normalize_costs(costs: &mut [f64])` — 除以最大值归一化；max≤0/全非有限 → 全 0.0
- [x] C19: `generate_weight_sample(i: u32, samples: u32) -> WeightedSum` 存在（D11 确定性公式）
- [x] C20: `eval_plan_objectives(plan, pool) -> BTreeMap<Objective, f32>` 存在（D13）
- [x] C21: `multi_objective.rs` 使用 `alloc::collections::BTreeMap`（无 std HashMap）
- [x] C22: `multi_objective.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe` / 无 `todo!` / 无 `unimplemented!`
- [x] C23: `multi_objective.rs` 中文模块文档注释（v0.88.0 + 偏差 D1/D2/D3/D8/D9/D10 引用）

## Task 2: multi_objective.rs — 单元测试 T121~T144
- [x] C24: T121 — `Objective::default() == Economy`；4 变体 Debug 非空
- [x] C25: T122 — 4 变体互不相等（6 对 assert_ne）
- [x] C26: T123 — `Objective` 作 BTreeMap 键：插入 4 目标后 keys 按 Ord 顺序
- [x] C27: T124 — `Objective` Copy 可复制
- [x] C28: T125 — `WeightedSum::new()`：`weights.is_empty()`；`get(Economy) == 0.0`
- [x] C29: T126 — `set(Economy, 2.0)` → `get(Economy)==2.0`；同 obj 覆盖
- [x] C30: T127 — `normalized()` 正常归一化：E=2.0/B=1.0/S=1.0/C 缺失 → [0.5, 0.25, 0.25, 0.0]
- [x] C31: T128 — `normalized()` 含 NaN → 4 目标各 0.25
- [x] C32: T129 — `normalized()` 含负值 → 4 目标各 0.25
- [x] C33: T130 — `normalized()` 全零 / 空 WeightedSum → 4 目标各 0.25；返回值含全部 4 键
- [x] C34: T131 — `WeightedSum` Clone 后 get 一致
- [x] C35: T132 — `ParetoSolution` 构造：objectives 含 4 键 + plan 字段访问
- [x] C36: T133 — `ParetoSolution` Clone 后 objectives 与 plan 一致
- [x] C37: T134 — `ParetoFront::default()` solutions 空；显式构造 solutions len
- [x] C38: T135 — `objective_costs(Economy)`：eff 0.9/0.8 → [0.1, 0.2]
- [x] C39: T136 — `objective_costs(BatteryLife)`：p_max 5.0/10.0 → [0.2, 0.1]；Safety：ramp 1.0/2.0 → [1.0, 0.5]
- [x] C40: T137 — 退化：p_max=0.0 → BatteryLife=1.0；ramp=0.0 → Safety=1.0
- [x] C41: T138 — `objective_costs` 空 caps → 空 Vec
- [x] C42: T139 — `normalize_costs([0.1, 0.2])` → [0.5, 1.0]
- [x] C43: T140 — `normalize_costs([0.0, 0.0])` → 全 0.0；`normalize_costs([NaN, NaN])` → 全 0.0
- [x] C44: T141 — `generate_weight_sample(0, 4)` 重复调用结果一致（确定性）
- [x] C45: T142 — `generate_weight_sample` 不同 i（0 vs 1）产生不同权重组合
- [x] C46: T143 — `generate_weight_sample(0, 1)`：归一化后 4 目标总和 == 1.0
- [x] C47: T144 — `eval_plan_objectives`：2 设备 plan（sp=3.0/2.0，eff=0.9/0.8）→ Economy 值 == 0.1*3.0+0.2*2.0；返回含全部 4 键

## Task 3: MultiObjectiveOptimizer + weighted + pareto + filter_dominated + build_weighted_lp
- [x] C48: `MultiObjectiveOptimizer` 结构体 3 字段（pool / solver / last_setpoints）
- [x] C49: `MultiObjectiveOptimizer::new(pool, solver)`（last_setpoints = BTreeMap::new()）
- [x] C50: `weighted` step 1：`!target.is_finite()` → `Err(InvalidTarget)`
- [x] C51: `weighted` step 2：`last_setpoints.retain(|id, _| pool.devices.contains_key(id))`
- [x] C52: `weighted` step 3：SOC 过滤（socs.get 为 Some(soc) 且 soc <= 0.0 → 跳过）
- [x] C53: `weighted` step 4：`eligible.is_empty()` → `Err(EmptyPool)`
- [x] C54: `weighted` step 5：加权目标构建 — `w.normalized()` + 4 目标 `objective_costs` + `normalize_costs` → `combined_i = Σ w_obj * norm_cost_obj_i`
- [x] C55: `weighted` step 6：LP 构建（变量 `p_{id}`，界 [p_min,p_max]，Continuous，sense Minimize，平衡行 rhs==target，爬坡行 prev±ramp）
- [x] C56: `weighted` step 7：solver.solve Ok+Optimal+solution.len==n → setpoint clamp + objective_value；否则 equal_split + objective_value=0.0
- [x] C57: `weighted` step 8：`last_setpoints` 更新为本次 setpoint
- [x] C58: `weighted` step 9：`Ok(DispatchPlan)` 含 timestamp / assignments / total_power（Σsetpoints）/ objective_value
- [x] C59: 私有 `build_weighted_lp` 函数存在（被 weighted 调用）
- [x] C60: `pareto`：samples==0 → Ok 空 front
- [x] C61: `pareto`：循环 `generate_weight_sample` → `weighted` → `eval_plan_objectives` → 收集
- [x] C62: `pareto`：`weighted` Err(EmptyPool/InvalidTarget) 透传（不吞错误）
- [x] C63: `pareto`：`filter_dominated` 过滤后 → Ok(ParetoFront)
- [x] C64: `filter_dominated`：最小化语义支配（A 全 ≤ B 且至少一 <）
- [x] C65: `filter_dominated`：相同向量保留先出现者；空输入 → 空输出
- [x] C66: `eval_plan_objectives`：每目标 `Σ cost_obj_i * setpoint_i`（原始值）；assignment 设备不在 pool → 跳过
- [x] C67: `multi_objective.rs` use 合规：alloc BTreeMap/Vec/Box/vec!/format! + crate v0.87.0 类型 + eneros_solver_core Solver/LpProblem/SolveStatus/ConstraintMatrix/VarType/ObjectiveSense；主代码无 unwrap/std

## Task 4: multi_objective.rs — 单元测试 T145~T160（含 FixedSolver）
- [x] C68: 测试辅助 `FixedSolver` 存在（impl Solver，含 result/fail 字段）
- [x] C69: `FixedSolver::solve` fail → `Err(SolverError::RunFailed(-1))`；否则返回预设 result
- [x] C70: T145 — `weighted` target NaN / INFINITY → `Err(InvalidTarget)`（solver 未调用）
- [x] C71: T146 — 空 pool → `Err(EmptyPool)`；全部 soc=0.0 → `Err(EmptyPool)`
- [x] C72: T147 — SOC 过滤：设备 1 soc=0.0 跳过 + 设备 2 soc=0.5 → assignments 仅设备 2
- [x] C73: T148 — happy path：2 设备 + FixedSolver Optimal [3.0, 2.0] → assignments / total_power / objective_value / timestamp / mode / ids 有序 / last_setpoints 更新
- [x] C74: T149 — 组合系数：`build_weighted_lp` objective[i] == 0.5*norm_economy[i] + 0.5*norm_battery[i]（容差 1e-6）
- [x] C75: T150 — 平衡行 rhs==target；首次 num_rows==1；有 last_setpoints → num_rows==3 + 爬坡行 rhs（prev-ramp, prev+ramp）
- [x] C76: T151 — solver fail → fallback：equal_split clamp + `objective_value==0.0` + `Ok(plan)`
- [x] C77: T152 — solver Infeasible → fallback；solver Optimal 但 solution 空 → fallback
- [x] C78: T153 — solver 解超 p_max → clamp 5.0；均权路径也正确
- [x] C79: T154 — `filter_dominated`：A(1,2)/B(2,1)/C(3,3) → C 被移除，保留 A、B
- [x] C80: T155 — `filter_dominated`：相同向量保留先者；空输入 → 空输出
- [x] C81: T156 — `pareto` samples=0 → Ok front solutions 空
- [x] C82: T157 — `pareto` happy path：2 设备 + FixedSolver Optimal，samples=4 → solutions.len() <= 4；每解 objectives 含全部 4 键
- [x] C83: T158 — `pareto` 空 pool → `Err(EmptyPool)` 透传
- [x] C84: T159 — 3 目标权衡：同 2 设备，weights {E only} vs {B only} → build_weighted_lp objective 向量不同
- [x] C85: T160 — 权重变更兼容：同一 optimizer 连续两次 weighted 不同权重 → 均 Ok，last_setpoints 滚动，第二次 num_rows==3

## Task 5: lib.rs surgical 修改
- [x] C86: `pub mod multi_objective;` 追加
- [x] C87: `pub use multi_objective::{eval_plan_objectives, filter_dominated, generate_weight_sample, normalize_costs, objective_costs, MultiObjectiveOptimizer, Objective, ParetoFront, ParetoSolution, WeightedSum};` 重导出
- [x] C88: 顶部文档注释追加 `# v0.88.0 多目标优化` 段落（核心类型列表 + D1~D14 偏差表）
- [x] C89: v0.72.0 既有 5 个私有 mod / v0.85.0 3 个 pub mod / v0.86.0 1 个 pub mod / v0.87.0 2 个 pub mod 全部保留
- [x] C90: v0.85.0~v0.87.0 既有 pub use 全部保留
- [x] C91: v0.72.0 既有 24 测试 + v0.85.0 42 + v0.86.0 38 + v0.87.0 40 = 144 测试保留不变
- [x] C92: `lib.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe`

## Task 6: Cargo.toml description 更新
- [x] C93: `description` 字段更新为含 "v0.88.0 多目标优化" 字样
- [x] C94: `[dependencies]` 段不变（无新依赖，复用既有 eneros-solver-core）
- [x] C95: workspace members 列表不变

## Task 7: configs/multi_objective.toml
- [x] C96: 文件位于 `configs/multi_objective.toml`
- [x] C97: `[weights]` 段含 economy / battery_life / safety / carbon 4 个 f32
- [x] C98: `[pareto]` 段含 `samples` 整数
- [x] C99: 中文注释说明权重非法→均权规则（D10）、目标成本系数定义（D8）、归一化说明（D9）、安全权重最高（蓝图 §7.3）

## Task 8: docs/agents/multi-objective-design.md
- [x] C100: 文件位于 `docs/agents/multi-objective-design.md`（非 docs/phase2，D12）
- [x] C101: 12 章节完整
- [x] C102: 至少 1 个 Mermaid 图（蓝图 §4.3：多目标 → 加权单目标 LP / Pareto 多组采样 → 决策者选择）
- [x] C103: 至少 1 个 Mermaid 图（weighted 决策流程）
- [x] C104: D1~D14 偏差声明表完整
- [x] C105: 前置依赖引用 v0.87.0 + v0.66.0 + v0.64.0
- [x] C106: 包含性能目标说明（加权 < 500ms / Pareto(50) < 5s，蓝图 §6.3/§7.2）
- [x] C107: 引用 v0.92.0 仲裁作为下游消费者
- [x] C108: 包含选型对比表（加权和 ⭐ / ε-约束 / NSGA-II，蓝图 §5.1）
- [x] C109: 目标成本系数定义表（D8 四目标公式 + 物理含义）
- [x] C110: 归一化说明章节（D9 + 蓝图 §8.5 坑点）
- [x] C111: 安全权重最高验收（蓝图 §7.3）

## Task 9: 版本同步根目录文件
- [x] C112: 根 `Cargo.toml` `[workspace.package] version = "0.88.0"`
- [x] C113: 根 `Cargo.toml` `[workspace.members]` 列表不变
- [x] C114: `Makefile` 中 `# Version: v0.88.0` 与 `VERSION := 0.88.0`
- [x] C115: `.github/workflows/ci.yml` 中 `# Version: v0.88.0`
- [x] C116: `ci/src/gate.rs` clippy 段注释含 `+ v0.88.0 多目标优化：Objective / WeightedSum / ParetoFront / ParetoSolution / MultiObjectiveOptimizer / objective_costs / normalize_costs / generate_weight_sample / filter_dominated / eval_plan_objectives`
- [x] C117: `ci/src/gate.rs` test 段注释同步追加类型列表

## Task 10: 构建校验（§2.4.2）
- [x] C118: `cargo metadata --format-version 1` 成功
- [x] C119: `cargo test -p eneros-energy-market-agent` 全部通过（144 既有 + T121~T160 40 新增 = 184 tests，0 failures）
- [x] C120: `cargo build -p eneros-energy-market-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 退出码 0
- [x] C121: `cargo fmt -p eneros-energy-market-agent -- --check` 退出码 0
- [x] C122: `cargo clippy -p eneros-energy-market-agent --all-targets -- -D warnings` 无 warning，退出码 0
- [x] C123: `cargo deny check licenses bans sources` 通过（无新依赖引入；advisories 视网络）
- [x] C124: 回归 — `cargo test -p eneros-grid-agent` 仍通过 130 tests + 1 doctest
- [x] C125: 回归 — `cargo test -p eneros-device-agent` 仍通过 24 tests
- [x] C126: 回归 — `cargo test -p eneros-tsn-time` 仍通过 84 tests + `cargo test -p eneros-agent-bus-dds` 仍通过 63 tests

## 总体校验
- [x] C127: 无根目录新 crate（`crates/agents/energy-market-agent/` 既有 crate 追加 1 个新模块，符合 §2.3.1）
- [x] C128: 无 `docs/` 根目录平面化文档（新文档在 `docs/agents/` 下）
- [x] C129: 无 `config/` 目录（新配置在 `configs/multi_objective.toml`）
- [x] C130: `.gitignore` 未需更新（无新文件类型）
- [x] C131: `git status` 无 `target/` / `*.elf` / `*.bin` / `*.dtb` / IDE 缓存被追踪
- [x] C132: ADR 决策未被违反（未引入研究特性、未自研已有开源替代组件、未超出 v1.0.0 范围）
- [x] C133: no_std 合规性：新文件继承 crate 级 `#![cfg_attr(not(test), no_std)]`
- [x] C134: 内存预算：优化模块 ≤ 1MB（算法骨架，实际占用远小于此）
- [x] C135: SBOM 未变化（无新第三方依赖，复用既有 eneros-solver-core）
- [x] C136: Surgical Changes 原则：v0.72.0/v0.85.0/v0.86.0/v0.87.0 既有源文件完全未改动
- [x] C137: v0.87.0 既有命名不冲突（multi_objective 为新命名，无重叠）
- [x] C138: `lib.rs` 仅追加 1 个 `pub mod` + 1 行 `pub use` + 顶部文档注释
- [x] C139: `multi_objective.rs` 内 `eval_plan_objectives` 设备不在 pool → 跳过（非 unwrap/panic）
- [x] C140: `filter_dominated` 避免浮点比较 NaN 陷阱（`is_finite()` 前置检查，D14）
