# v0.87.0 Multi-Device Dispatch Spec — Energy Agent 多设备调度

## Why

v0.72.0 完成 Energy Agent 单设备双脑调度，v0.86.0 完成 Market Agent 报价生成，但 Energy Agent 尚不具备**多设备功率分配与全局优化**能力。本版本扩展 `eneros-energy-market-agent` crate 增加 2 个新模块：`device_pool.rs`（设备能力模型 + 设备池管理）与 `multi_dispatch.rs`（多设备 LP 调度器 + 平均分配兜底），实现园区多设备（储能+光伏+充电桩）协同优化，从单设备调度升级为多设备协同，为 v0.88.0 多目标优化与 VPP 聚合奠定基础。

## What Changes

- **ADDED**：`crates/agents/energy-market-agent/src/device_pool.rs` — 设备能力模型与设备池
  - `DeviceMode` 枚举（2 变体：`Auto` / `Manual`，默认 `Auto`）
  - `DeviceCapability` 结构体（4 字段：`p_min` / `p_max` / `ramp_rate` / `efficiency`，Copy）
  - `DevicePool` 结构体（字段 `devices: BTreeMap<u64, DeviceCapability>`）+ `new` / `add_device` / `remove_device` / `get` / `len` / `is_empty`
- **ADDED**：`crates/agents/energy-market-agent/src/multi_dispatch.rs` — 多设备调度器
  - `DeviceAssignment` 结构体（3 字段：`device_id: u64` / `setpoint: f32` / `mode: DeviceMode`，Copy）
  - `DispatchPlan` 结构体（4 字段：`timestamp: u64` / `assignments: Vec<DeviceAssignment>` / `total_power: f32` / `objective_value: f32`）
  - `DispatchError` 枚举（2 变体：`EmptyPool` / `InvalidTarget`）
  - `MultiDeviceDispatcher` 结构体（3 字段：`pool: DevicePool` / `solver: Box<dyn Solver>` / `last_setpoints: BTreeMap<u64, f32>`）
  - `dispatch(target, socs, now_ms)` — LP 构建（容量/爬坡/SOC 约束）→ Solver 求解 → 失败回退 `equal_split` 平均分配兜底（蓝图 §4.4）
  - `equal_split(target, caps)` 公开自由函数（平均分配 + [p_min, p_max] clamp）
- **MODIFIED**：`crates/agents/energy-market-agent/src/lib.rs` — 追加 2 个 `pub mod` + 重导出（surgical：仅追加，不修改 v0.72.0/v0.85.0/v0.86.0 既有代码）
- **MODIFIED**：`crates/agents/energy-market-agent/Cargo.toml` — `description` 字段追加（**无新依赖**：复用既有 `eneros-solver-core` 依赖）
- **ADDED**：`configs/device_pool.toml` — 设备清单配置模板
- **ADDED**：`docs/agents/multi-dispatch-design.md` — 设计文档（12 章 + Mermaid 图 + D1~D14 偏差表）
- **MODIFIED**：根 `Cargo.toml` workspace 版本 `0.86.0` → `0.87.0`
- **MODIFIED**：`Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs` 版本同步
- **未新增 crate**：2 个新模块追加到既有 `eneros-energy-market-agent` crate（D2）

无 **BREAKING** 变更：v0.72.0/v0.85.0/v0.86.0 全部既有公共 API 保留；新增类型与函数仅追加。

## Impact

- **Affected specs**：v0.72.0 Energy/Market Agent（追加多设备调度子模块）；为 v0.88.0 多目标优化提供 `MultiDeviceDispatcher` 基础；VPP 聚合的前置
- **Affected code**：
  - `crates/agents/energy-market-agent/src/device_pool.rs`（新建）
  - `crates/agents/energy-market-agent/src/multi_dispatch.rs`（新建）
  - `crates/agents/energy-market-agent/src/lib.rs`（追加 2 个 `pub mod` + 重导出 + 文档段落）
  - `crates/agents/energy-market-agent/Cargo.toml`（description 字段更新）
  - 根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`（版本同步）
- **依赖不变**：复用既有 `eneros-solver-core`（`Solver` trait / `LpProblem` / `SolveResult` / `SolveStatus` / `SolverError` / `ObjectiveSense` / `VarType` / `ConstraintMatrix`）；无新第三方依赖；SBOM 不变
- **回归面**：既有 104 tests（v0.72.0 24 + v0.85.0 42 + v0.86.0 38）必须全部通过；grid-agent 130、device-agent 24、tsn-time 84、agent-bus-dds 63 无回归

## ADDED Requirements

### Requirement: Device Pool Data Structures

系统 SHALL 提供设备能力模型与设备池（`device_pool.rs`）：

- `DeviceMode` 枚举（2 变体：`Auto` / `Manual`），派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `Auto`）
- `DeviceCapability` 结构体（4 字段：`p_min: f32` / `p_max: f32` / `ramp_rate: f32` / `efficiency: f32`），派生 `Debug, Clone, Copy, PartialEq, Default`
- `DevicePool` 结构体（1 字段：`devices: BTreeMap<u64, DeviceCapability>`，D3/D4），派生 `Debug, Clone, Default`
- `DevicePool::new() -> Self`（空池）
- `DevicePool::add_device(&mut self, id: u64, cap: DeviceCapability)`（同 id 覆盖）
- `DevicePool::remove_device(&mut self, id: u64) -> bool`（蓝图 §4.4 "设备不可用 → 从 pool 移除"；存在移除返回 true，不存在返回 false）
- `DevicePool::get(&self, id: u64) -> Option<&DeviceCapability>`
- `DevicePool::len(&self) -> usize` / `DevicePool::is_empty(&self) -> bool`

#### Scenario: Pool management
- **WHEN** 新建 pool + `add_device(1, cap)` + `add_device(2, cap2)`
- **THEN** `len() == 2`，`get(1) == Some(&cap)`；`remove_device(1) == true` 后 `get(1) == None`、`len() == 1`；`remove_device(99) == false`

#### Scenario: Deterministic iteration order
- **WHEN** 按 30、10、20 顺序插入 3 台设备
- **THEN** `devices.keys()` 迭代顺序为 [10, 20, 30]（BTreeMap 有序，保证 LP 列映射确定性，D3）

### Requirement: Dispatch Data Structures

系统 SHALL 提供调度结果数据模型（`multi_dispatch.rs`）：

- `DeviceAssignment` 结构体（3 字段：`device_id: u64` / `setpoint: f32` / `mode: DeviceMode`），派生 `Debug, Clone, Copy, PartialEq, Default`
- `DispatchPlan` 结构体（4 字段：`timestamp: u64` / `assignments: Vec<DeviceAssignment>` / `total_power: f32` / `objective_value: f32`），派生 `Debug, Clone, PartialEq, Default`（含 Vec 不派生 Copy）
- `DispatchError` 枚举（2 变体：`EmptyPool` / `InvalidTarget`），派生 `Debug, Clone, Copy, PartialEq, Eq`

### Requirement: equal_split Fallback

系统 SHALL 提供平均分配兜底自由函数（蓝图 §4.4 "Solver 失败 → 平均分配兜底"）：

```rust
pub fn equal_split(target: f32, caps: &[(u64, DeviceCapability)]) -> Vec<DeviceAssignment>
```

- `caps` 为空 → 返回空 Vec（无 panic）
- 每台设备 `setpoint = (target / n).max(p_min).min(p_max)`（clamp 到设备容量界，蓝图 §7.3 "不超设备容量"）
- `mode = DeviceMode::Auto`

#### Scenario: Equal split with clamping
- **WHEN** `equal_split(10.0, [(1, cap{p_max:3.0,...}), (2, cap{p_max:5.0,...})])`
- **THEN** 设备 1 setpoint == 3.0（clamp p_max），设备 2 setpoint == 5.0

### Requirement: MultiDeviceDispatcher

系统 SHALL 提供多设备调度器：

```rust
pub struct MultiDeviceDispatcher {
    pub pool: DevicePool,
    pub solver: Box<dyn Solver>,
    pub last_setpoints: BTreeMap<u64, f32>,
}

impl MultiDeviceDispatcher {
    pub fn new(pool: DevicePool, solver: Box<dyn Solver>) -> Self;
    pub fn dispatch(&mut self, target: f32, socs: &BTreeMap<u64, f32>, now_ms: u64)
        -> Result<DispatchPlan, DispatchError>;
}
```

`dispatch` 严格按序执行：

1. **目标校验**：`!target.is_finite()`（NaN/±∞）→ `Err(InvalidTarget)`
2. **陈旧清理**：`last_setpoints` 移除已不在 pool 中的设备条目
3. **SOC 过滤**（D10）：遍历 `pool.devices`（BTreeMap 有序），`socs.get(&id)` 为 `Some(soc)` 且 `soc <= 0.0` → 跳过该设备；收集 `Vec<(u64, DeviceCapability)>`
4. **空池校验**：过滤后为空 → `Err(EmptyPool)`
5. **LP 构建**（D6/D9/D14）：
   - 变量 `p_i ∈ [p_min, p_max]`（Continuous），每设备 1 个，顺序与过滤后列表一致
   - 目标：`Minimize Σ (1.0 - efficiency_i) · p_i`（损耗最小，D14）
   - 平衡行：`Σ p_i = target`（`rhs_lower[0] == rhs_upper[0] == target`）
   - 爬坡行（仅 `last_setpoints` 含该设备时）：`prev_i - ramp_rate_i <= p_i <= prev_i + ramp_rate_i`（单行双边 rhs，D9）
6. **求解**：`self.solver.solve(&problem, now_ms)`：
   - `Ok(result)` 且 `result.status == SolveStatus::Optimal` 且 `result.solution.len() == n` → 采用解，每 setpoint clamp 到 `[p_min, p_max]`；`objective_value = result.objective_value as f32`
   - 其他（`Err` / 非 Optimal / 解长度不符）→ `equal_split(target, &eligible)` 兜底；`objective_value = 0.0`
7. **状态更新**：`last_setpoints` 更新为本次 setpoint
8. **返回**：`Ok(DispatchPlan { timestamp: now_ms, assignments, total_power: setpoints 之和, objective_value })`（D13）

#### Scenario: Happy path LP dispatch
- **WHEN** 2 台设备（id=1 p∈[0,5] eff=0.9 / id=2 p∈[0,5] eff=0.8），solver 返回 Optimal solution [3.0, 2.0] objective 0.5，target=5.0
- **THEN** `Ok(plan)`：assignments [id=1 setpoint=3.0, id=2 setpoint=2.0]，total_power==5.0，objective_value==0.5，timestamp==now_ms，mode==Auto；`last_setpoints` 更新为 {1:3.0, 2:2.0}

#### Scenario: LP problem construction
- **WHEN** 首次 dispatch（无 last_setpoints），RecordingSolver 记录 LpProblem
- **THEN** `variables.len() == n`；`lower_bounds[i] == p_min_i`；`upper_bounds[i] == p_max_i`；`var_types` 全 Continuous；`sense == Minimize`；`objective[i] == 1.0 - eff_i`；`constraints.num_rows == 1`（仅平衡行）；`row_start == [0, n]`；`col_index == [0..n-1]`；`rhs_lower[0] == rhs_upper[0] == target`

#### Scenario: Ramp constraint on second dispatch
- **WHEN** 首次 dispatch 成功（设备 1 setpoint=3.0，ramp_rate=1.0），第二次 dispatch
- **THEN** 记录的 LpProblem `constraints.num_rows == 2`；爬坡行 `rhs_lower[1] == 3.0 - 1.0`、`rhs_upper[1] == 3.0 + 1.0`，该设备列系数 1.0

#### Scenario: Solver failure falls back to equal split
- **WHEN** solver `solve` 返回 `Err(SolverError::RunFailed(-1))` 或 `SolveStatus::Infeasible` 或解长度不符
- **THEN** `Ok(plan)`：assignments 为平均分配 clamp 结果，`objective_value == 0.0`（蓝图 §4.4）

#### Scenario: SOC filter
- **WHEN** 设备 1 soc=0.0、设备 2 soc=0.5，dispatch target=4.0
- **THEN** assignments 仅含设备 2（设备 1 被 SOC 过滤，D10）

#### Scenario: All devices SOC exhausted
- **WHEN** 全部设备 soc <= 0.0
- **THEN** `Err(EmptyPool)`

#### Scenario: Device offline re-dispatch
- **WHEN** 3 设备 dispatch 成功后 `pool.remove_device(2)`，再次 dispatch
- **THEN** assignments 仅含设备 1、3；`last_setpoints` 中设备 2 条目被清理（蓝图 §6.5 故障注入）

### Requirement: no_std Compliance

所有新增代码 MUST 满足 no_std 合规：
- 2 个新文件不添加 `#![cfg_attr(not(test), no_std)]`（继承 lib.rs crate 级属性）
- 仅使用 `alloc::boxed::Box` / `alloc::collections::BTreeMap` / `alloc::vec::Vec` / `alloc::string::String`（LP 变量名）/ `alloc::format!` / `core::*`
- 禁止 `use std::*` / `async` / `panic!` / `unsafe` / `todo!` / `unimplemented!` / `unwrap()`（主代码）/ `HashMap`（std）/ `Arc` / `Instant::now()`
- 复用 `eneros_solver_core::{solver::Solver, problem::{LpProblem, ConstraintMatrix, VarType, ObjectiveSense}, result::{SolveResult, SolveStatus}, error::SolverError}`（既有依赖，无新增）

## MODIFIED Requirements

### Requirement: eneros-energy-market-agent crate 公共 API

v0.72.0/v0.85.0/v0.86.0 全部既有公共 API 保留不变。

本版本追加以下公共 API（仅追加，不修改既有签名）：
- 模块：`pub mod device_pool;` + `pub mod multi_dispatch;`
- 重导出：
  - `pub use device_pool::{DeviceCapability, DeviceMode, DevicePool};`
  - `pub use multi_dispatch::{equal_split, DeviceAssignment, DispatchError, DispatchPlan, MultiDeviceDispatcher};`
- crate `description` 字段追加 ` + v0.87.0 多设备调度 (多设备 LP 功率分配/爬坡与 SOC 约束/平均分配兜底, no_std)`

### Requirement: 版本同步

- 根 `Cargo.toml` `[workspace.package] version = "0.87.0"`
- `Makefile` VERSION 变量 + header 注释 → `0.87.0`
- `.github/workflows/ci.yml` header 注释 → `0.87.0`
- `ci/src/gate.rs` clippy 段 + test 段注释追加：`+ v0.87.0 多设备调度：DeviceMode / DeviceCapability / DevicePool / DeviceAssignment / DispatchPlan / DispatchError / MultiDeviceDispatcher / equal_split`
- workspace members 列表**不变**（2 个新模块是既有 crate 的新文件）

## REMOVED Requirements

无。本版本仅追加，不删除任何既有功能。

## 偏差声明（D1~D14，Karpathy "Think Before Coding"）

| 偏差 | 蓝图原文 | 本版本处理 | 理由 |
|------|---------|-----------|------|
| **D1** | `pub async fn dispatch(&self, target, socs)` | sync `fn dispatch(&mut self, target, socs, now_ms)` | no_std 无 async runtime；`&mut self` 因 `Solver::solve` 需 `&mut` + `last_setpoints` 更新；`now_ms` 参数注入（沿用 v0.82~v0.86 D1/D2） |
| **D2** | 代码于 `crates/agents/energy_agent/src/` | 扩展既有 `crates/agents/energy-market-agent` | v0.72.0 D12 已合并 Energy+Market 单 crate；新建 energy_agent crate 会重复概念（沿用 v0.85.0 D2 / v0.86.0 D2） |
| **D3** | `DevicePool { devices: HashMap<DeviceId, DeviceCapability> }` | `devices: BTreeMap<u64, DeviceCapability>` | no_std 无 std HashMap；BTreeMap 来自 alloc 且**迭代有序**——LP 变量列映射必须确定性（csr col_index ↔ device id 顺序）；设备数小（≤ 数十），BTreeMap 性能足够 |
| **D4** | `device_id: String` / `DeviceId`（暗示 String） | `device_id: u64` | no_std 无堆 String；`DeviceAssignment` 保持 Copy；与 v0.85.0 D4 / v0.86.0 D4 一致 |
| **D5** | `solver: Arc<dyn Solver>` | `solver: Box<dyn Solver>`，直接复用 `eneros_solver_core::solver::Solver` | Arc 需原子+线程语义，no_std 单线程用 Box；eneros-solver-core **已是本 crate 既有依赖**（v0.72.0 引入），直接复用 trait 无需本地抽象（对比 v0.86.0 D6：彼时为避免**新**依赖才定义本地 BidOptimizer） |
| **D6** | `OptProblem::new()` / `add_var` / `add_constraint` DSL | 直接构建既有 `LpProblem` CSR 结构（variables/lower_bounds/upper_bounds/objective/ConstraintMatrix/rhs_lower/rhs_upper） | 蓝图 DSL 不存在；v0.64.0 solver-core 已定义 CSR 格式 `LpProblem` 为权威接口；避免重复造轮子（§5.5） |
| **D7** | `DeviceMode::Auto` 引用但 `DeviceMode` 未定义 | 定义 `DeviceMode` 枚举（2 变体 `Auto` / `Manual`，默认 `Auto`） | 蓝图引用未定义类型；MVP 最小变体集，Manual 为后续人工设定点预留 |
| **D8** | `DispatchError` 引用但未定义 | 2 变体：`EmptyPool` / `InvalidTarget` | 蓝图 §4.4 两条错误规则中 "Solver 失败 → 平均分配兜底" 为**回退非错误**；硬错误仅"无可调度设备"（空池/全 SOC 耗尽）与"目标非法"（NaN/∞）；与 v0.86.0 D8 MVP 错误分类风格一致 |
| **D9** | 爬坡约束 `add_constraint(ramp_i, [(p_i,1.0)], Le, ramp_rate)`（即 `p_i <= ramp_rate`） | `prev_i - ramp_rate_i <= p_i <= prev_i + ramp_rate_i`（相对上次设定点的变化率约束，单行双边 rhs） | 蓝图代码将功率上限在 ramp_rate（5MW 储能配 1MW/min 爬坡则永远 ≤ 1MW），蓝图 §8.5 自认"爬坡约束过紧导致不可行"；正确语义为 |Δp| ≤ ramp，需跟踪 `last_setpoints`（首次 dispatch 无爬坡行） |
| **D10** | `socs: &HashMap<DeviceId, f32>` 参数在蓝图代码中**未被使用** | SOC 过滤规则：`soc <= 0.0` → 该设备本轮跳过（视为临时不可用） | 蓝图 §4.3 声称 "约束： 容量/爬坡/SOC" 但代码未使用 socs；DeviceCapability 无能量容量字段，SOC 无法换算功率界；MVP 采用确定性可用性过滤（> 0 可用，≤ 0 跳过），不臆造能量换算公式（Simplicity First） |
| **D11** | `timestamp: now_ms()` | `now_ms: u64` 参数注入 | no_std 无 `Instant::now()`（沿用 v0.82~v0.86 全部版本模式） |
| **D12** | 无上次设定点状态 | `last_setpoints: BTreeMap<u64, f32>` 字段于 `MultiDeviceDispatcher`（非 DeviceCapability） | DeviceCapability 保持蓝图 4 字段纯静态能力模型；设定点是运行时状态，归属调度器；dispatch 开头清理已移除设备的陈旧条目 |
| **D13** | `total_power: target`（直接赋值） | `total_power = Σ 最终 setpoints`（solver 路径 clamp 后之和 / 兜底路径 clamp 后之和） | 解 clamp 或兜底 clamp 后实际功率可能偏离 target；如实上报实际值（可观测性 §9）；`objective_value` 兜底路径为 0.0（无优化目标值） |
| **D14** | 目标函数未定义（`sol.objective()` 引用但无系数） | `Minimize Σ (1.0 - efficiency_i) · p_i` | 蓝图未定义优化目标；损耗最小使 efficiency 字段有语义（高效设备优先承担功率），系数确定性可测试 |

## 测试计划（T81~T120，沿用 crate 内连续编号）

- `device_pool.rs`：T81~T92（12 个）— DeviceMode/DeviceCapability/DevicePool 默认值、派生、增删查、有序迭代
- `multi_dispatch.rs`：T93~T120（28 个）— 数据结构、equal_split（含 clamp/空输入）、dispatch 校验（InvalidTarget/EmptyPool/SOC 过滤）、LP 构造正确性（变量界/平衡行/目标系数/爬坡行，经 RecordingSolver 记录验证）、求解路径（happy path/clamp/total_power/last_setpoints 更新）、三类兜底（Err/非 Optimal/解长度不符）、5 设备协同（§6.2）、设备增减兼容（§6.4）、设备离线重分配（§6.5）
- 测试辅助：`RecordingSolver`（测试模块内定义，impl `Solver`：记录 `LpProblem` 克隆 + 返回预设 `SolveResult` 或 `Err(SolverError::RunFailed(-1))`）
- crate 总测试数：104（既有）+ 40（新增）= **144 tests**
