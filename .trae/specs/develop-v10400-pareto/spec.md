# v0.104.0 多目标 Pareto 优化 Spec

> 蓝图：`e:\eneros\蓝图\phase2.md` §v0.104.0（P2-F 第 3 版，Solver 扩展收尾，9 节齐全）。新建 crate `crates/ai/solver-pareto/`（eneros-solver-pareto）。蓝图检索确认无 v0.104.x 刚性子版本（Phase 2 刚性子版本仅 v0.98.1）。

## Why

日前/联邦调度需兼顾经济性、碳排放、设备寿命三目标，单目标 LP/MILP 无法表达目标间权衡。蓝图要求实现 NSGA-II 多目标 Pareto 前沿生成 + 决策者加权选择，为联邦多目标协调奠基。v0.103.0 已落地 MILP 神经热启动，本版为 Solver 扩展收尾（P2-F 闭环）。

## What Changes

- **新建** `crates/ai/solver-pareto/`（`eneros-solver-pareto`，no_std + alloc，依赖 eneros-solver-core）：
  - `src/pareto_front.rs`：`MultiObjectiveProblem`/`Objective`/`OptDirection`/`VariableSpec`/`ParetoSolution`/`ParetoFront`（`non_dominated`/`select_by_weight`）+ `ParetoSolver` trait + 支配/拥挤度核心算法
  - `src/nsga2.rs`：`Nsga2Solver`（内置确定性 xorshift64* PRNG，seed 注入；初始化 → 评估 → 非支配排序 → 拥挤度 → 锦标赛选择 + 均匀交叉 + 均匀变异 → 输出 rank 0 前沿）
  - `src/decision.rs`：`DecisionMaker`（偏好权重归一化 → 调用 `select_by_weight` 选出最终方案）
  - `src/lib.rs`：模块声明 + 重导出 + crate 文档（含 D1~D12 偏差表）
- **新增** `configs/solver-pareto.toml`：`[pareto]` pop_size / gen / crossover_rate / mutation_rate / 三目标权重 + 中文注释 ≥6 点
- **新增** `docs/ai/pareto-design.md`：12 章节 + ≥2 Mermaid + D1~D12 偏差表
- **新增 30 个单元测试**（src 内嵌 `#[cfg(test)]`：PF1~PF10 + NS11~NS22 + DM23~DM30）
- 根 `Cargo.toml`：members 追加 `"crates/ai/solver-pareto"` + version 0.103.0 → 0.104.0；`Makefile` / `ci.yml` / `gate.rs` 注释串尾 2 处同步
- **无 BREAKING**：纯新增 crate，既有 crate 零改动

## Impact

- Affected specs：develop-v10400-pareto（新建）
- Affected code：`crates/ai/solver-pareto/`（新建）、`configs/`、`docs/ai/`、根 4 文件版本号
- 上游：v0.103.0 solver-warm（热启动加速底座）、v0.66.0 energy-lp（单目标 LP，前沿为空时编排层兜底）、v0.64.0 solver-core（SolverError 复用）
- 下游：v0.109.0 故障录波；联邦多目标协调（Phase 2 出口）

## ADDED Requirements

### Requirement: Pareto 前沿数据结构与核心算法（pareto_front.rs）

The system SHALL provide `MultiObjectiveProblem { objectives, variables }`、`Objective { name, direction, weight }`、`VariableSpec { lower, upper }`、`ParetoSolution { variables, objectives, rank, crowding }`、`ParetoFront { solutions }`：支配判定（全目标不劣且至少一项更优，统一最小化口径）、`non_dominated()` 过滤 rank==0、`select_by_weight(weights)` 归一化后返回加权和最小解。

#### Scenario: 支配判定（蓝图 §4.5 dominates）
- **WHEN** a.objectives=[1.0, 2.0]，b.objectives=[2.0, 3.0]（最小化口径）
- **THEN** dominates(a, b) == true；dominates(b, a) == false；相等向量互不支配

#### Scenario: 加权选择
- **WHEN** front 3 解 objectives 分别为 [1.0, 5.0]/[3.0, 3.0]/[5.0, 1.0]，weights=[0.8, 0.2]
- **THEN** `select_by_weight` 返回 objectives=[1.0, 5.0] 的解（0.8×1+0.2×5=1.8 最小）；空 front 返回 None

#### Scenario: 权重非法归一化（蓝图 §4.4）
- **WHEN** weights 含负值或全零
- **THEN** 负值 clamp 为 0；和为 0 时按均匀权重处理，不 panic

### Requirement: NSGA-II 求解器（nsga2.rs）

The system SHALL provide `Nsga2Solver { crossover_rate, mutation_rate, seed }` 实现 `ParetoSolver`：`solve(problem, pop_size, gen)` 输出 rank==0 的 Pareto 前沿；同 seed 两次求解结果逐比特一致（确定性）；Maximize 目标评估时取负归一（蓝图 lifespan 取负的一般化）。

#### Scenario: 三目标前沿生成（蓝图 §6.2）
- **WHEN** 4 变量问题，objectives=[cost(Min)/carbon(Min)/lifespan(Max)]，pop_size=100，gen=50
- **THEN** 返回 front 非空；每解 `objectives.len() == 3`；rank==0；耗时 < 10s（§6.3）

#### Scenario: 确定性复现
- **WHEN** 同 seed 构造两个 Nsga2Solver 求解同问题
- **THEN** 两次 front.solutions 的 variables/objectives 完全相等

#### Scenario: 非法输入
- **WHEN** problem.variables 为空 / objectives 为空 / pop_size == 0
- **THEN** 返回 `Err(SolverError::InvalidProblem(_))`，不 panic

### Requirement: 决策者选择（decision.rs）

The system SHALL provide `DecisionMaker { preferences }`：`choose(&front)` 将 preferences 归一化（负值 clamp 0、全零→均匀）后委托 `select_by_weight`，返回 `Option<&ParetoSolution>`；单目标问题退化为最小值选择。

#### Scenario: 偏好决定选择
- **WHEN** 同上 3 解前沿，preferences=[1.0, 0.0]（纯成本偏好）vs [0.0, 1.0]（纯碳偏好）
- **THEN** 前者选 [1.0, 5.0]，后者选 [5.0, 1.0]

## MODIFIED Requirements

无（纯新增 crate，既有 crate 零改动）。

## REMOVED Requirements

无。

## 偏差声明（D1~D12，相对蓝图 §3/§4/§6）

| 编号 | 偏差 | 理由 |
|------|------|------|
| **D1** | 蓝图 `crates/solver_pareto/` → `crates/ai/solver-pareto/` | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；与 solver-core/solver-milp/solver-warm 同 AI 子系统 |
| **D2** | 蓝图 `docs/phase2/pareto.md` → `docs/ai/pareto-design.md` | 记忆 §2.3.3 强制：文档按方向分类 |
| **D3** | 蓝图 `tests/pareto_front.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.103.0 项目惯例，不新增 tests/ 文件 |
| **D4** | 蓝图 `use rand::Rng` + `thread_rng()` → 内置确定性 xorshift64* PRNG（~20 行），seed 构造注入（`Nsga2Solver::with_seed(seed)`，`new()` 默认固定 seed） | rand crate 依赖 std，违反全项目 no_std（记忆 §4.3，蓝图 §43.1）；确定性 seed 使测试可复现（Karpathy Goal-Driven） |
| **D5** | 蓝图 `ParetoSolver: Send + Sync` → 去除 bound | 与 v0.64.0 `Solver`/v0.103.0 `WarmStartProvider` 惯例一致；NSGA-II 单线程种群算法无跨线程需求 |
| **D6** | 蓝图 §4.1 `constraints: Vec<Constraint>` 删除（`Constraint` 类型蓝图未定义且算法全程未消费） | 界约束已由 `VariableSpec.{lower,upper}` 表达；功能约束属目标评估层（Karpathy Simplicity First，不引入死字段） |
| **D7** | Maximize 目标在评估出口统一取负（蓝图 `"lifespan" => -sum` 硬编码的一般化），dominates/crowding/select 统一最小化口径 | 蓝图支配判定隐含全最小化假设但未声明；归一化后算法与方向解耦，目标可扩展（蓝图 §8.4/§9 可扩展要求） |
| **D8** | 蓝图 `partial_cmp(...).unwrap()` → `f64::total_cmp` | NaN 输入时 unwrap 会 panic，违反 no_std 禁 `panic!`（项目规则）；total_cmp 全序确定性（core 可用，≥1.62） |
| **D9** | 蓝图 solve 每代 `population = front1.take(pop_size)`（种群随 front 萎缩、无真实交叉变异，注释自承"简化"）→ 实现锦标赛选择（rank 优先、平手比拥挤度）+ 均匀交叉 + 均匀变异补满 pop_size | 对齐蓝图 §4.3 Mermaid（选择/交叉/变异为流程必经节点）与 §5.1"NSGA-II 采用"承诺；骨架可用标准（记忆 §4.4） |
| **D10** | 蓝图 §4.4"前沿为空 → 返回 LP 单目标解" → 本 crate 不内联 LP 兜底：`solve` 前沿为空时返回空 front（`is_empty()` 可判），由编排层回退 v0.66.0 单目标 LP | crate 无 LP 问题输入，内联 LP 造成依赖反转（Simplicity First）；`select_by_weight`/`choose` 空 front 返回 None |
| **D11** | `SolverError` 复用 eneros-solver-core（`InvalidProblem` 变体），不新建 ParetoError | 蓝图 §4.2 签名即 `SolverError`；v0.103.0 复用先例；避免平行错误体系 |
| **D12** | 性能 50 代 × 100 种群 < 10s 落地为 `#[cfg(test)]` 断言（std `Instant` 仅测试可用）；算法复杂度 O(gen × pop² × obj) 声明于文档 | no_std 无计时器（v0.64.0 D1 `now_ms` 注入先例；测试外不注入计时，保持 solve 签名与蓝图一致） |

## 接口契约

```rust
// pareto_front.rs
pub struct MultiObjectiveProblem {
    pub objectives: Vec<Objective>, pub variables: Vec<VariableSpec>,
}  // Debug/Clone
pub struct Objective {
    pub name: String, pub direction: OptDirection, pub weight: f64,
}  // Debug/Clone
pub enum OptDirection { Minimize, Maximize }  // Debug/Clone/Copy/PartialEq
pub struct VariableSpec { pub lower: f64, pub upper: f64 }  // Debug/Clone/Copy
pub struct ParetoSolution {
    pub variables: Vec<f64>, pub objectives: Vec<f64>,
    pub rank: usize, pub crowding: f64,
}  // Debug/Clone
pub struct ParetoFront { pub solutions: Vec<ParetoSolution> }  // Debug/Clone/Default
impl ParetoFront {
    pub fn non_dominated(&self) -> Vec<&ParetoSolution>;           // rank == 0
    pub fn select_by_weight(&self, weights: &[f64]) -> Option<&ParetoSolution>; // 归一化加权和最小
    pub fn is_empty(&self) -> bool;
    pub fn len(&self) -> usize;
}
pub trait ParetoSolver {        // 无 Send+Sync（D5）
    fn solve(&self, problem: &MultiObjectiveProblem, pop_size: usize, gen: usize)
        -> Result<ParetoFront, SolverError>;   // SolverError 复用 solver-core（D11）
}

// nsga2.rs
pub struct Nsga2Solver {
    pub crossover_rate: f64, pub mutation_rate: f64, pub seed: u64,
}  // Debug/Clone
impl Nsga2Solver {
    pub fn new() -> Self;                  // 0.9 / 0.1 / 默认固定 seed（D4）
    pub fn with_seed(seed: u64) -> Self;
}
impl ParetoSolver for Nsga2Solver { /* init → evaluate(方向归一 D7) → gen × {排序+拥挤度+锦标赛+均匀交叉+变异} → rank0 */ }

// decision.rs
pub struct DecisionMaker { pub preferences: Vec<f64> }  // Debug/Clone
impl DecisionMaker {
    pub fn new(preferences: Vec<f64>) -> Self;
    pub fn choose<'a>(&self, front: &'a ParetoFront) -> Option<&'a ParetoSolution>; // 归一化 → select_by_weight
}
```

## 测试规划（solver-pareto 30 个，src 内嵌）

| 文件 | 编号 | 数量 | 覆盖 |
|------|------|------|------|
| pareto_front.rs | PF1~PF10 | 10 | Objective/OptDirection 构造 / VariableSpec 界 / dominates 全劣支配 / 单项更优即支配 / 相等互不支配 / non_dominated 过滤 rank0 / 空 front is_empty / select_by_weight 加权最小 / 空 front None / 负权重 clamp+全零均匀 |
| nsga2.rs | NS11~NS22 | 12 | init 种群界内 / 种群大小==pop_size / 同 seed 逐比特一致 / 异 seed 不同 / evaluate 三目标值（cost/carbon/lifespan 口径）/ Maximize 取负归一 / non_dominated_sort rank 赋值 / crowding ≤2 边界 MAX / crowding 中间值累加 / solve e2e rank0 非空且 objectives.len==3 / InvalidProblem（空 variables/objectives/pop_size 0）/ 50×100 < 10s |
| decision.rs | DM23~DM30 | 8 | preferences 归一化选择 / 纯成本 vs 纯碳不同选择 / 全零偏好均匀 / 单目标退化最小值 / 空 front None / 与 select_by_weight 一致性 / DecisionMaker Debug/Clone / 偏好长度 < 目标数缺省补 0 不 panic |

## 配置与文档

- `configs/solver-pareto.toml`：`[pareto]` pop_size = 100 / gen = 50 / crossover_rate = 0.9 / mutation_rate = 0.1 / seed / 三目标权重 + 中文注释 ≥6 点（NSGA-II 选型 §5.1 / 性能 <10s §6.3 / 确定性 seed D4 / 方向归一 D7 / LP 兜底编排层 D10 / 内存预算 ≤128MB §5.6 / GPU 不适用 §6.6）
- `docs/ai/pareto-design.md`：12 章节 + ≥2 Mermaid（蓝图 §4.3 NSGA-II 流程图重绘 + solve 一代进化时序图）+ D1~D12 偏差表 + 性能口径声明（D12）

## 版本同步

根 `Cargo.toml` version = "0.104.0"；`Makefile` VERSION；`ci.yml` 注释；`gate.rs` 注释串尾 2 处追加 v0.104.0 类型清单（MultiObjectiveProblem/Objective/OptDirection/VariableSpec/ParetoSolution/ParetoFront/ParetoSolver/Nsga2Solver/DecisionMaker）。
