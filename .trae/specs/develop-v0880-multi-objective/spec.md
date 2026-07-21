# v0.88.0 Multi-Objective Optimization Spec — Energy Agent 多目标优化

## Why

v0.87.0 完成多设备 LP 调度（单目标：损耗最小），但单一目标会过度优化（如纯经济损害电池寿命）。本版本扩展 `eneros-energy-market-agent` crate 增加 `multi_objective.rs` 模块，实现**多目标优化**（经济 vs 寿命 vs 安全 vs 碳排，加权和 + Pareto 前沿），支持决策者在冲突目标间权衡，为 v0.92.0 仲裁提供多目标基础（蓝图 §1 出口关联）。

## What Changes

- **ADDED**：`crates/agents/energy-market-agent/src/multi_objective.rs` — 多目标优化器
  - `Objective` 枚举（4 变体：`Economy` / `BatteryLife` / `Safety` / `Carbon`，默认 `Economy`，派生 `Ord` 作 BTreeMap 键）
  - `WeightedSum` 结构体（`weights: BTreeMap<Objective, f32>`）+ `new` / `set` / `get` / `normalized`
  - `ParetoSolution` 结构体（`objectives: BTreeMap<Objective, f32>` + `plan: DispatchPlan`）
  - `ParetoFront` 结构体（`solutions: Vec<ParetoSolution>`）
  - `MultiObjectiveOptimizer` 结构体（3 字段：`pool: DevicePool` / `solver: Box<dyn Solver>` / `last_setpoints: BTreeMap<u64, f32>`）
  - `weighted(target, socs, w, now_ms)` — 加权聚合单目标 LP（容量/爬坡/SOC 约束，复用 v0.87.0 语义）→ Solver 求解 → 失败回退 `equal_split`（蓝图 §4.4）
  - `pareto(target, socs, samples, now_ms)` — 多组确定性权重采样 → 逐点 weighted → 支配过滤 → ParetoFront
  - 公开自由函数 `objective_costs(obj, caps)` / `normalize_costs` / `generate_weight_sample` / `filter_dominated`（可测试）
- **MODIFIED**：`crates/agents/energy-market-agent/src/lib.rs` — 追加 1 个 `pub mod` + 重导出（surgical：仅追加，不修改 v0.72.0/v0.85.0/v0.86.0/v0.87.0 既有代码）
- **MODIFIED**：`crates/agents/energy-market-agent/Cargo.toml` — `description` 字段追加（**无新依赖**）
- **ADDED**：`configs/multi_objective.toml` — 权重配置模板
- **ADDED**：`docs/agents/multi-objective-design.md` — 设计文档（12 章 + Mermaid 图 + D1~D14 偏差表）
- **MODIFIED**：根 `Cargo.toml` workspace 版本 `0.87.0` → `0.88.0`
- **MODIFIED**：`Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs` 版本同步
- **未新增 crate**：新模块追加到既有 `eneros-energy-market-agent` crate（D1）

无 **BREAKING** 变更：v0.72.0/v0.85.0/v0.86.0/v0.87.0 全部既有公共 API 保留；新增类型与函数仅追加。

## Impact

- **Affected specs**：v0.87.0 多设备调度（复用 `DevicePool` / `DeviceCapability` / `DispatchPlan` / `DispatchError` / `equal_split`）；为 v0.92.0 仲裁提供多目标权衡基础
- **Affected code**：
  - `crates/agents/energy-market-agent/src/multi_objective.rs`（新建）
  - `crates/agents/energy-market-agent/src/lib.rs`（追加 1 个 `pub mod` + 重导出 + 文档段落）
  - `crates/agents/energy-market-agent/Cargo.toml`（description 字段更新）
  - 根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`（版本同步）
- **依赖不变**：复用既有 `eneros-solver-core`（`Solver` / `LpProblem` / `SolveStatus` / `SolverError`）与同 crate v0.87.0 类型；无新第三方依赖；SBOM 不变
- **回归面**：既有 144 tests（v0.72.0 24 + v0.85.0 42 + v0.86.0 38 + v0.87.0 40）必须全部通过；grid-agent 130、device-agent 24、tsn-time 84、agent-bus-dds 63 无回归

## ADDED Requirements

### Requirement: Objective 与 WeightedSum 数据结构

系统 SHALL 提供目标枚举与权重模型（`multi_objective.rs`）：

- `Objective` 枚举（4 变体：`Economy` / `BatteryLife` / `Safety` / `Carbon`），派生 `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default`（`#[default]` on `Economy`）
- `WeightedSum` 结构体（字段 `weights: BTreeMap<Objective, f32>`，pub），派生 `Debug, Clone, Default`
- `WeightedSum::new() -> Self`（空权重）
- `WeightedSum::set(&mut self, obj: Objective, w: f32)`（同 obj 覆盖）
- `WeightedSum::get(&self, obj: Objective) -> f32`（缺失返回 0.0）
- `WeightedSum::normalized(&self) -> BTreeMap<Objective, f32>` — 权重归一化（D10）：
  - 收集 4 目标权重（缺失 = 0.0）
  - 任一权重 NaN / 负值 / 总和 ≤ 0 / 总和非有限 → 返回均权（每目标 0.25）（蓝图 §4.4 "权重非法 → 默认均权"）
  - 否则归一化为总和 1

#### Scenario: Weight normalization
- **WHEN** `WeightedSum` 设置 Economy=2.0 / BatteryLife=1.0 / Safety=1.0（Carbon 缺失）
- **THEN** `normalized()` 返回 Economy=0.5 / BatteryLife=0.25 / Safety=0.25 / Carbon=0.0

#### Scenario: Invalid weights fall back to equal
- **WHEN** 权重含 NaN 或负值，或全部权重为 0
- **THEN** `normalized()` 返回 4 目标各 0.25

### Requirement: 目标成本向量（确定性，D8）

系统 SHALL 提供每目标的设备成本系数自由函数：

```rust
pub fn objective_costs(obj: Objective, caps: &[(u64, DeviceCapability)]) -> Vec<f64>
```

每目标成本系数（D8，蓝图未定义，本版本确定性定义）：

| Objective | 成本系数 cost_i | 物理含义 |
|-----------|----------------|---------|
| `Economy` | `1.0 - efficiency_i` | 损耗最小 = 运行成本最低 |
| `BatteryLife` | `1.0 / p_max_i`（`p_max_i <= 0.0` → 1.0） | 浅充放：大功率设备 C-rate 低，优先承担 |
| `Safety` | `1.0 / ramp_rate_i`（`ramp_rate_i <= 0.0` → 1.0） | 响应能力：高爬坡设备优先，预留调节裕度 |
| `Carbon` | `1.0 - efficiency_i` | 排放强度代理：高效率 = 低排放（MVP 与 Economy 同代理，D8） |

- `normalize_costs(costs: &mut [f64])` — 除以最大值归一化到 [0,1]（蓝图 §8.5 量纲坑点）；`max <= 0.0` 或全非有限 → 全部置 0.0（D9）

#### Scenario: Cost vectors
- **WHEN** 2 设备（eff=0.9/0.8，p_max=5.0/10.0，ramp=1.0/2.0）
- **THEN** Economy costs = [0.1, 0.2]；BatteryLife costs = [0.2, 0.1]；Safety costs = [1.0, 0.5]；归一化后各向量最大值 = 1.0

### Requirement: MultiObjectiveOptimizer::weighted

系统 SHALL 提供加权聚合优化：

```rust
pub struct MultiObjectiveOptimizer {
    pub pool: DevicePool,
    pub solver: Box<dyn Solver>,
    pub last_setpoints: BTreeMap<u64, f32>,
}

impl MultiObjectiveOptimizer {
    pub fn new(pool: DevicePool, solver: Box<dyn Solver>) -> Self;
    pub fn weighted(&mut self, target: f32, socs: &BTreeMap<u64, f32>, w: &WeightedSum, now_ms: u64)
        -> Result<DispatchPlan, DispatchError>;
}
```

`weighted` 严格按序执行（沿用 v0.87.0 dispatch 语义）：

1. **目标校验**：`!target.is_finite()` → `Err(InvalidTarget)`
2. **陈旧清理**：`last_setpoints` 移除已不在 pool 中的设备条目
3. **SOC 过滤**：`socs.get(id)` 为 `Some(soc)` 且 `soc <= 0.0` → 跳过；收集 `eligible: Vec<(u64, DeviceCapability)>`
4. **空池校验**：过滤后为空 → `Err(EmptyPool)`
5. **加权目标构建**（D7/D8/D9）：
   - 对 4 目标分别计算 `objective_costs` 并 `normalize_costs` 归一化
   - `w.normalized()` 得归一化权重
   - 组合系数：`combined_i = Σ_obj w_obj * normalized_cost_obj_i`
6. **LP 构建**：变量 `p_i ∈ [p_min, p_max]`（Continuous，名 `p_{id}`）；`objective = combined`；`sense = Minimize`；平衡行 `Σ p_i = target`；爬坡行（有 last_setpoint 时）`prev - ramp <= p <= prev + ramp`（与 v0.87.0 D9 一致）
7. **求解**：`Ok` 且 `Optimal` 且 `solution.len() == n` → 采用解（clamp [p_min, p_max]），`objective_value = result.objective_value as f32`；否则 `equal_split` 兜底，`objective_value = 0.0`
8. **状态更新**：`last_setpoints` 更新为本次 setpoint
9. **返回**：`Ok(DispatchPlan { timestamp: now_ms, assignments, total_power: Σ setpoints, objective_value })`

#### Scenario: Weighted happy path
- **WHEN** 2 设备（id=1 eff=0.9 p∈[0,5] / id=2 eff=0.8 p∈[0,5]），weights Economy=1.0，solver 返回 Optimal [3.0, 2.0] objective 0.4，target=5.0
- **THEN** `Ok(plan)`：assignments [id=1 sp=3.0, id=2 sp=2.0]，total_power==5.0，objective_value==0.4，timestamp==now_ms；`last_setpoints` 更新为 {1:3.0, 2:2.0}

#### Scenario: Weight affects LP objective coefficients
- **WHEN** 2 设备（eff=0.9/0.8，p_max=5.0/10.0，ramp=1.0/2.0），weights Economy=1.0 / BatteryLife=1.0（各 0.5 归一化）
- **THEN** LP `objective[i] == 0.5 * norm_economy[i] + 0.5 * norm_battery[i]`（容差 1e-6）

#### Scenario: Economy-only weight reproduces loss minimization
- **WHEN** weights 仅 Economy=1.0
- **THEN** LP `objective[i] == 1.0`（归一化后 max(0.1,0.2)=0.2，[0.5,1.0]）——等比例缩放不改变最优解语义

### Requirement: MultiObjectiveOptimizer::pareto

系统 SHALL 提供 Pareto 前沿采样：

```rust
pub fn pareto(&mut self, target: f32, socs: &BTreeMap, samples: u32, now_ms: u64)
    -> Result<ParetoFront, DispatchError>;
```

1. `samples == 0` → 返回 `Ok(ParetoFront { solutions: vec![] })`（蓝图 §4.4 "采样不足 → 提示"，MVP 返回空前沿，D13）
2. 对 `i in 0..samples`：`generate_weight_sample(i, samples)` 生成确定性权重 → `weighted(...)`：
   - `Ok(plan)` → `eval_plan_objectives(&plan, &self.pool)` 计算 4 目标总值 → 收集 `ParetoSolution`
   - `Err(EmptyPool)` → 立即返回 `Err(EmptyPool)`（空池不可恢复）
   - `Err(InvalidTarget)` → 立即返回 `Err(InvalidTarget)`
3. `filter_dominated(solutions)` 支配过滤 → `Ok(ParetoFront)`

- `generate_weight_sample(i: u32, samples: u32) -> WeightedSum`（D11）：`w_j = ((i * (j+1)) % samples + 1) as f32`（j = 0..3 对应 Economy/BatteryLife/Safety/Carbon），由 `WeightedSum::normalized` 保证总和 1；同 `(i, samples)` 必得同权重（确定性可测试）
- `eval_plan_objectives(plan: &DispatchPlan, pool: &DevicePool) -> BTreeMap<Objective, f32>`（D13）：每目标 `Σ cost_obj_i * setpoint_i`（成本系数同 `objective_costs`，未经归一化的原始值）
- `filter_dominated(solutions: Vec<ParetoSolution>) -> Vec<ParetoSolution>`（D14）：最小化语义下 A 支配 B ⟺ A 全部目标 ≤ B 且至少一个严格 <；保留非支配解；完全相同的解向量保留先出现者；O(n²) 两两比较（samples ≤ 数十，性能足够）

#### Scenario: Pareto domination filter
- **WHEN** 3 解：A(econ=1, life=2) / B(econ=2, life=1) / C(econ=3, life=3)
- **THEN** C 被 A、B 支配而移除；front 保留 A、B

#### Scenario: Pareto happy path
- **WHEN** 2 设备 + RecordingSolver 恒返回 Optimal，samples=4
- **THEN** `Ok(front)`：`solutions.len() <= 4`（支配过滤后）；每解 `objectives` 含全部 4 目标键

### Requirement: no_std Compliance

所有新增代码 MUST 满足 no_std 合规：
- 新文件不添加 `#![cfg_attr(not(test), no_std)]`（继承 lib.rs crate 级属性）
- 仅使用 `alloc::boxed::Box` / `alloc::collections::BTreeMap` / `alloc::vec::Vec` / `alloc::vec!` / `alloc::format!` / `core::*`
- 禁止 `use std::*` / `async` / `panic!` / `unsafe` / `todo!` / `unimplemented!` / `unwrap()`（主代码）/ `HashMap`（std）/ `Arc` / `Instant::now()`
- 复用 `crate::device_pool::{DeviceCapability, DevicePool}` 与 `crate::multi_dispatch::{equal_split, DeviceAssignment, DispatchError, DispatchPlan}`（同 crate v0.87.0 类型，零适配）

## MODIFIED Requirements

### Requirement: eneros-energy-market-agent crate 公共 API

v0.72.0/v0.85.0/v0.86.0/v0.87.0 全部既有公共 API 保留不变。

本版本追加以下公共 API（仅追加，不修改既有签名）：
- 模块：`pub mod multi_objective;`
- 重导出：
  - `pub use multi_objective::{eval_plan_objectives, filter_dominated, generate_weight_sample, normalize_costs, objective_costs, MultiObjectiveOptimizer, Objective, ParetoFront, ParetoSolution, WeightedSum};`
- crate `description` 字段追加 ` + v0.88.0 多目标优化 (经济/寿命/安全/碳排加权和 + Pareto 前沿, no_std)`

### Requirement: 版本同步

- 根 `Cargo.toml` `[workspace.package] version = "0.88.0"`
- `Makefile` VERSION 变量 + header 注释 → `0.88.0`
- `.github/workflows/ci.yml` header 注释 → `0.88.0`
- `ci/src/gate.rs` clippy 段 + test 段注释追加：`+ v0.88.0 多目标优化：Objective / WeightedSum / ParetoFront / ParetoSolution / MultiObjectiveOptimizer / objective_costs / normalize_costs / generate_weight_sample / filter_dominated / eval_plan_objectives`
- workspace members 列表**不变**（新模块是既有 crate 的新文件）

## REMOVED Requirements

无。本版本仅追加，不删除任何既有功能。

## 偏差声明（D1~D14，Karpathy "Think Before Coding"）

| 偏差 | 蓝图原文 | 本版本处理 | 理由 |
|------|---------|-----------|------|
| **D1** | 代码于 `crates/agents/energy_agent/src/multi_objective.rs` | 扩展既有 `crates/agents/energy-market-agent` | v0.72.0 D12 已合并 Energy+Market 单 crate；新建 energy_agent crate 会重复概念（沿用 v0.85.0~v0.87.0 D2 模式） |
| **D2** | `weights: HashMap<Objective, f32>` / `objectives: HashMap<Objective, f32>` | `BTreeMap<Objective, f32>`（`Objective` 派生 `Ord`） | no_std 无 std HashMap；BTreeMap 迭代有序，目标值输出确定性（沿用 v0.87.0 D3） |
| **D3** | `solver: Arc<dyn Solver>` | `solver: Box<dyn Solver>` | Arc 需原子+线程语义，no_std 单线程用 Box（沿用 v0.87.0 D5）；eneros-solver-core 已是既有依赖 |
| **D4** | `weighted(&self, w: &WeightedSum)`（无 target/socs/时间参数） | `weighted(&mut self, target, socs, w, now_ms)` | 蓝图签名缺失目标功率（无法构建平衡约束）与 SOC；`&mut` 因 Solver::solve 需 `&mut` + last_setpoints 更新；`now_ms` 参数注入（沿用 v0.87.0 D1/D11） |
| **D5** | `Result<..., SolveError>` | 复用 v0.87.0 `DispatchError`（EmptyPool / InvalidTarget） | `SolveError` 不存在；蓝图 §4.4 两条规则均为回退（均权/提示）非硬错误；硬错误语义与 v0.87.0 一致 |
| **D6** | `OptProblem::new()` / `add_var` DSL + `plan_from(sol)` | 直接构建既有 `LpProblem` CSR 结构 | 蓝图 DSL 不存在；v0.64.0 solver-core 的 `LpProblem` 为权威接口（沿用 v0.87.0 D6）；避免重复造轮子（§5.5） |
| **D7** | 未提及 LP 构建复用 | 模块内私有 `build_weighted_lp` 自包含实现（不改 v0.87.0 私有 `build_lp_problem` 可见性） | Surgical Changes：v0.87.0 文件完全不动；LP 构建 ~40 行重复换零耦合（对比 DRY，surgical 约束优先） |
| **D8** | 4 目标的成本函数未定义 | 确定性定义：Economy=`1-eff` / BatteryLife=`1/p_max` / Safety=`1/ramp_rate` / Carbon=`1-eff`（退化兜底 1.0） | 蓝图仅列目标名无量纲；成本系数必须是 DeviceCapability 的确定性函数才可测试；Carbon 缺排放数据，MVP 用效率代理并显式声明 |
| **D9** | §5.4 "目标量纲不一致 → 归一化" 未给方法 | `normalize_costs`：除以最大值归一化到 [0,1]；max≤0/非有限 → 全 0 | 蓝图 §8.5 自认坑点"量纲归一化不当导致某目标主导"；max-归一化最简单且保持零值语义 |
| **D10** | §4.4 "权重非法 → 默认均权" 未定义"非法" | 具体化：任一 NaN / 负值 / 总和 ≤ 0 / 非有限 → 4 目标各 0.25；否则归一化总和 1 | 规则必须确定性可测试（T 覆盖 NaN/负值/全零/正常） |
| **D11** | `generate_weight_sample(i, samples)` 引用但未定义 | `w_j = ((i*(j+1)) % samples + 1) as f32`（j=0..3），再归一化 | 确定性权重扫描（非随机，no_std 无 RNG）；同输入必同输出；4 目标单纯形上散布 |
| **D12** | `docs/phase2/multi_objective.md` + `tests/multi_obj.rs` | `docs/agents/multi-objective-design.md` + 文件内 `#[cfg(test)] mod tests` | 工作区规则 §2.3.3 禁止 docs/phase2 平面化；内嵌测试沿用 v0.82.0~v0.87.0 模式 |
| **D13** | `eval(&plan)` 引用但未定义；§4.4 "采样不足 → 提示" | `eval_plan_objectives(plan, pool)`：每目标 `Σ cost_i * setpoint_i`（原始未归一化值）；samples=0 → `Ok(空 front)` | eval 必须确定性；空 front 即"提示"的 MVP 语义（无 log crate） |
| **D14** | `filter_dominated(solutions)` 引用但未定义 | O(n²) 两两支配比较（最小化：全 ≤ 且至少一 <）；完全相同向量保留先出现者 | 支配语义标准定义；samples ≤ 数十，O(n²) 足够；确定性保留顺序可测试 |

## 测试计划（T121~T160，沿用 crate 内连续编号）

- `multi_objective.rs`：T121~T160（40 个）
  - T121~T124：`Objective` 枚举（default=Economy / 4 变体互异 / Ord 排序作 BTreeMap 键 / Debug）
  - T125~T130：`WeightedSum`（new 空 / set+get / 缺失返回 0.0 / 同 obj 覆盖 / normalized 总和=1 / Clone）
  - T131~T134：权重归一化（非法 NaN→均权 / 负值→均权 / 全零→均权 / 正常归一化值）
  - T135~T138：`ParetoSolution` / `ParetoFront` 数据结构（构造 / Clone / Default 空 / Debug）
  - T139~T142：`objective_costs`（Economy=1-eff / BatteryLife=1/p_max / Safety=1/ramp / 退化 p_max≤0→1.0）
  - T143~T144：`normalize_costs`（max 归一化 / max≤0 全零）
  - T145~T150：`weighted` 校验与 happy path（InvalidTarget / EmptyPool / SOC 过滤 / assignments+total+objective+timestamp / last_setpoints 更新 / ids 有序）
  - T151~T152：LP 结构（平衡行 rhs==target / 组合系数 = Σ w*norm_cost 容差 1e-6）
  - T153~T154：`weighted` 兜底（solver Err → equal_split + objective_value=0.0 / 非 Optimal → 兜底）
  - T155~T157：`generate_weight_sample`（确定性同参同果 / 不同 i 不同权重 / samples=1 归一化）
  - T158：`filter_dominated`（支配移除 / 非支配保留 / 相同向量保留先者）
  - T159：`pareto` happy path（samples=4 → solutions ≤ 4 且 objectives 4 键齐全 / samples=0 → 空 front）
  - T160：3 目标权衡场景（§6.2：Economy vs BatteryLife vs Safety 权重变化 → LP 目标系数变化）
- 测试辅助：复用 `FixedSolver` 桩（impl `Solver`，返回预设 `SolveResult` 或 `Err(SolverError::RunFailed(-1))`）
- crate 总测试数：144（既有）+ 40（新增）= **184 tests**
