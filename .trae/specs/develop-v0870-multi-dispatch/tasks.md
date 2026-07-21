# Tasks

- [x] Task 1: 创建 `crates/agents/energy-market-agent/src/device_pool.rs` — 设备能力模型 + 设备池
  - [x] SubTask 1.1: `DeviceMode` 枚举（2 变体 `Auto` / `Manual`），派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `Auto`）
  - [x] SubTask 1.2: `DeviceCapability` 结构体（4 字段：`p_min: f32` / `p_max: f32` / `ramp_rate: f32` / `efficiency: f32`），派生 `Debug, Clone, Copy, PartialEq, Default`，每字段中文 doc（含单位 MW / MW·min⁻¹ / 0~1）
  - [x] SubTask 1.3: `DevicePool` 结构体（字段 `devices: BTreeMap<u64, DeviceCapability>`，pub），派生 `Debug, Clone, Default`
  - [x] SubTask 1.4: `DevicePool::new() -> Self` / `add_device(&mut self, id: u64, cap: DeviceCapability)`（同 id 覆盖）/ `remove_device(&mut self, id: u64) -> bool` / `get(&self, id: u64) -> Option<&DeviceCapability>` / `len(&self) -> usize` / `is_empty(&self) -> bool`
  - [x] SubTask 1.5: 中文模块文档注释（v0.87.0 设备池 + 偏差 D3/D4/D7 引用）；`use alloc::collections::BTreeMap;`；无 std/async/panic!/unsafe/todo!/unimplemented!/HashMap/String 字段

- [x] Task 2: 在 `device_pool.rs` 添加 `#[cfg(test)] mod tests` 单元测试 T81~T92
  - [x] SubTask 2.1: T81 — `DeviceMode::default() == Auto`；2 变体 Debug 非空
  - [x] SubTask 2.2: T82 — `DeviceCapability::default()` 4 字段全 0.0
  - [x] SubTask 2.3: T83 — `DeviceCapability` 显式构造与字段访问（p_min=0.0/p_max=5.0/ramp_rate=1.0/efficiency=0.9）
  - [x] SubTask 2.4: T84 — `DeviceCapability` Copy 可复制
  - [x] SubTask 2.5: T85 — `DevicePool::new()`：`is_empty() == true` / `len() == 0` / `devices.is_empty()`
  - [x] SubTask 2.6: T86 — `add_device(1, cap)` → `len()==1` / `get(1)==Some(&cap)` / `is_empty()==false`
  - [x] SubTask 2.7: T87 — 同 id 重复 `add_device` 覆盖（get 返回新 cap，len 仍 1）
  - [x] SubTask 2.8: T88 — `remove_device(1)` 存在 → `true` + `get(1)==None` + `len()==0`
  - [x] SubTask 2.9: T89 — `remove_device(99)` 不存在 → `false`，len 不变
  - [x] SubTask 2.10: T90 — 按 30/10/20 插入 3 台，`devices.keys().copied().collect::<Vec<_>>() == [10, 20, 30]`（有序迭代确定性，D3）
  - [x] SubTask 2.11: T91 — `DevicePool::default()` 与 `new()` 等价；Clone 后 get 结果一致
  - [x] SubTask 2.12: T92 — 3 台设备 add 后 `len()==3`；remove 1 台后 `len()==2`

- [x] Task 3: 创建 `crates/agents/energy-market-agent/src/multi_dispatch.rs` — 数据结构 + DispatchError + equal_split + MultiDeviceDispatcher
  - [x] SubTask 3.1: `DeviceAssignment` 结构体（3 字段：`device_id: u64` / `setpoint: f32` / `mode: DeviceMode`），派生 `Debug, Clone, Copy, PartialEq, Default`
  - [x] SubTask 3.2: `DispatchPlan` 结构体（4 字段：`timestamp: u64` / `assignments: Vec<DeviceAssignment>` / `total_power: f32` / `objective_value: f32`），派生 `Debug, Clone, PartialEq, Default`
  - [x] SubTask 3.3: `DispatchError` 枚举（2 变体：`EmptyPool` / `InvalidTarget`），派生 `Debug, Clone, Copy, PartialEq, Eq`
  - [x] SubTask 3.4: `equal_split(target: f32, caps: &[(u64, DeviceCapability)]) -> Vec<DeviceAssignment>` — 空 caps 返回空 Vec；`share = target / n`；每台 `setpoint = share.max(p_min).min(p_max)`；`mode = Auto`
  - [x] SubTask 3.5: `MultiDeviceDispatcher` 结构体（3 字段全 pub：`pool: DevicePool` / `solver: Box<dyn Solver>` / `last_setpoints: BTreeMap<u64, f32>`）
  - [x] SubTask 3.6: `MultiDeviceDispatcher::new(pool, solver) -> Self`（`last_setpoints = BTreeMap::new()`）
  - [x] SubTask 3.7: `dispatch(&mut self, target: f32, socs: &BTreeMap<u64, f32>, now_ms: u64) -> Result<DispatchPlan, DispatchError>` 严格按序：
    1. `!target.is_finite()` → `Err(InvalidTarget)`
    2. `self.last_setpoints.retain(|id, _| self.pool.devices.contains_key(id));`（陈旧清理）
    3. 遍历 `self.pool.devices` 过滤 SOC（`socs.get(id)` 为 `Some(soc)` 且 `*soc <= 0.0` → 跳过），收集 `eligible: Vec<(u64, DeviceCapability)>`
    4. `eligible.is_empty()` → `Err(EmptyPool)`
    5. 构建 `LpProblem`：n 变量（名 `format!("p_{}", id)`，`p_min..p_max`，Continuous）；`objective[i] = 1.0 - eff_i as f64`；`sense = Minimize`；平衡行 0（n 个系数 1.0，`rhs_lower[0] == rhs_upper[0] == target as f64`）；每设备若 `last_setpoints.get(id)` 存在 → 追加爬坡行（该列系数 1.0，`rhs_lower = prev - ramp`、`rhs_upper = prev + ramp`）
    6. `self.solver.solve(&problem, now_ms)`：`Ok(result)` 且 `result.status == SolveStatus::Optimal` 且 `result.solution.len() == n` → setpoint = `solution[i].max(p_min as f64).min(p_max as f64) as f32`，`objective_value = result.objective_value as f32`；否则 `equal_split(target, &eligible)` + `objective_value = 0.0`
    7. 更新 `last_setpoints` 为本次 setpoint
    8. `Ok(DispatchPlan { timestamp: now_ms, assignments, total_power: setpoints 之和, objective_value })`
  - [x] SubTask 3.8: LP 构建抽为私有自由函数 `build_lp_problem(eligible: &[(u64, DeviceCapability)], target: f32, last_setpoints: &BTreeMap<u64, f32>) -> LpProblem`（便于测试与审查）
  - [x] SubTask 3.9: 中文模块文档注释（v0.87.0 多设备调度 + 偏差 D1/D5/D6/D8/D9/D10/D13/D14 引用）；use 仅 `alloc::boxed::Box` / `alloc::collections::BTreeMap` / `alloc::string::String` / `alloc::vec::Vec` / `alloc::format!` + `crate::device_pool::{DeviceCapability, DeviceMode}` + `eneros_solver_core::{solver::Solver, problem::{ConstraintMatrix, LpProblem, ObjectiveSense, VarType}, result::SolveStatus}`；无 std/async/panic!/unsafe/todo!/unimplemented!/unwrap/Arc/HashMap/Instant

- [x] Task 4: 在 `multi_dispatch.rs` 添加 `#[cfg(test)] mod tests` 单元测试 T93~T120（含 `RecordingSolver` 测试辅助）
  - [x] SubTask 4.1: 测试辅助 `RecordingSolver`（impl `Solver`：字段 `recorded: Option<LpProblem>` / `result: Option<SolveResult>` / `fail: bool`；`solve` 记录 problem 克隆，`fail` → `Err(SolverError::RunFailed(-1))`，否则返回预设 result；`name`/`version`/`set_param`/`status` 简单实现）
  - [x] SubTask 4.2: T93 — `DeviceAssignment::default()` 全零 + `mode == Auto`
  - [x] SubTask 4.3: T94 — `DeviceAssignment` 3 字段显式构造 + Copy 可复制
  - [x] SubTask 4.4: T95 — `DispatchPlan::default()`：timestamp 0 / assignments 空 / total_power 0.0 / objective_value 0.0
  - [x] SubTask 4.5: T96 — `DispatchPlan` 4 字段显式构造 + Clone
  - [x] SubTask 4.6: T97 — `DispatchError` 2 变体 PartialEq（相等+互不相等）+ Debug 非空
  - [x] SubTask 4.7: T98 — `equal_split(10.0, 2 台 p_max≥5)` → setpoint 均 5.0，mode Auto
  - [x] SubTask 4.8: T99 — `equal_split(10.0, [p_max=3.0, p_max=5.0])` → [3.0, 5.0]（clamp p_max，§7.3）
  - [x] SubTask 4.9: T100 — `equal_split(-10.0, [p_min=-2.0 ×2])` → [-2.0, -2.0]（clamp p_min，负 target 充电场景）
  - [x] SubTask 4.10: T101 — `equal_split(10.0, &[])` → 空 Vec，无 panic
  - [x] SubTask 4.11: T102 — `dispatch` target 为 NaN 与 `f32::INFINITY` → `Err(InvalidTarget)`（solver 未被调用）
  - [x] SubTask 4.12: T103 — 空 pool dispatch → `Err(EmptyPool)`
  - [x] SubTask 4.13: T104 — 全部设备 soc=0.0 → `Err(EmptyPool)`
  - [x] SubTask 4.14: T105 — 设备 1 soc=0.0（跳过）+ 设备 2 soc=0.5 → assignments 仅含设备 2（RecordingSolver solution 长度 1）
  - [x] SubTask 4.15: T106 — happy path：2 设备 + RecordingSolver 返回 Optimal `[3.0, 2.0]` objective 0.5 → assignments `[id=1 sp=3.0, id=2 sp=2.0]` / total_power==5.0 / objective_value==0.5 / timestamp==now_ms / mode Auto / ids 有序
  - [x] SubTask 4.16: T107 — LP 变量构造：`variables.len()==2` / `lower_bounds==[p_min]` / `upper_bounds==[p_max]` / `var_types` 全 Continuous / 变量名 `p_1` `p_2`
  - [x] SubTask 4.17: T108 — 平衡行：`num_rows==1`（首次无爬坡）/ `row_start==[0,2]` / `col_index==[0,1]` / `values==[1.0,1.0]` / `rhs_lower[0]==rhs_upper[0]==target`
  - [x] SubTask 4.18: T109 — 目标系数：`objective[i]==1.0-eff_i` / `sense==Minimize`（eff 0.9→0.1，eff 0.8→0.2，f64 EPSILON）
  - [x] SubTask 4.19: T110 — 首次 dispatch `num_rows==1`；成功后第二次 dispatch `num_rows==3`（2 设备均有 last_setpoint 各加 1 爬坡行）
  - [x] SubTask 4.20: T111 — 爬坡行语义：prev=3.0 / ramp=1.0 → `rhs_lower[1]==2.0` / `rhs_upper[1]==4.0` / 该设备列系数 1.0
  - [x] SubTask 4.21: T112 — dispatch 后 `last_setpoints` 更新为 setpoint（{1:3.0, 2:2.0}）
  - [x] SubTask 4.22: T113 — solver `fail=true`（Err(RunFailed)）→ fallback：平均分配 clamp + `objective_value==0.0` + `Ok(plan)`
  - [x] SubTask 4.23: T114 — solver 返回 `SolveStatus::Infeasible` → fallback（同 T113 断言）
  - [x] SubTask 4.24: T115 — solver 返回 Optimal 但 `solution.len() != n`（如空 Vec）→ fallback
  - [x] SubTask 4.25: T116 — solver 返回 setpoint 超 p_max（如 9.0 > 5.0）→ clamp 到 5.0
  - [x] SubTask 4.26: T117 — solver 路径 `total_power == setpoints 之和`（clamp 后，D13）
  - [x] SubTask 4.27: T118 — 5 设备协同（§6.2 集成语义）：5 设备 RecordingSolver 返回 `[1.0,2.0,3.0,4.0,5.0]` → 5 条 assignments、id 有序、total==15.0
  - [x] SubTask 4.28: T119 — 设备增减兼容（§6.4）：dispatch → `add_device(3, cap)` → 再 dispatch → assignments 含设备 3
  - [x] SubTask 4.29: T120 — 设备离线重分配（§6.5）：3 设备 dispatch → `remove_device(2)` → 再 dispatch → assignments 仅 1、3 且 `last_setpoints` 无设备 2 条目

- [x] Task 5: 修改 `crates/agents/energy-market-agent/src/lib.rs` — 追加 2 个 `pub mod` + 重导出（surgical）
  - [x] SubTask 5.1: 追加 `pub mod device_pool;` + `pub mod multi_dispatch;`（既有 5 私有 mod + v0.85.0 3 pub mod + v0.86.0 1 pub mod 全部保留）
  - [x] SubTask 5.2: 追加 `pub use device_pool::{DeviceCapability, DeviceMode, DevicePool};`
  - [x] SubTask 5.3: 追加 `pub use multi_dispatch::{equal_split, DeviceAssignment, DispatchError, DispatchPlan, MultiDeviceDispatcher};`
  - [x] SubTask 5.4: 顶部文档注释追加 `# v0.87.0 多设备调度` 段落（核心类型列表 + D1~D14 偏差表，从 spec.md 复制）
  - [x] SubTask 5.5: 不修改任何 v0.72.0/v0.85.0/v0.86.0 既有代码行；既有 104 tests 保留
  - [x] SubTask 5.6: `lib.rs` 无 std/async/panic!/unsafe

- [x] Task 6: 修改 `crates/agents/energy-market-agent/Cargo.toml` — 更新 description（surgical）
  - [x] SubTask 6.1: `description` 末尾追加 ` + v0.87.0 多设备调度 (多设备 LP 功率分配/爬坡与 SOC 约束/平均分配兜底, no_std)`
  - [x] SubTask 6.2: `[dependencies]` 段不变（复用既有 eneros-solver-core，无新依赖，D5/D6）
  - [x] SubTask 6.3: workspace members 列表不变

- [x] Task 7: 创建配置文件 `configs/device_pool.toml`
  - [x] SubTask 7.1: `[[device]]` 数组段 ×3 示例（储能/光伏/充电桩）：`id` / `name`（注释说明仅文档用途）/ `p_min` / `p_max` / `ramp_rate` / `efficiency`
  - [x] SubTask 7.2: 中文注释说明各字段单位（MW / MW·min⁻¹ / 0~1）与 D4（id 用 u64 非 String）
  - [x] SubTask 7.3: 注释说明 SOC 过滤规则（soc <= 0 本轮跳过，D10）与平均分配兜底（蓝图 §4.4）

- [x] Task 8: 创建设计文档 `docs/agents/multi-dispatch-design.md`
  - [x] SubTask 8.1: 12 章节完整（版本目标 / 前置依赖 / 交付物清单 / 数据结构 / 接口设计 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险与坑点 / 偏差声明）
  - [x] SubTask 8.2: Mermaid 图 1：蓝图 §4.3 核心算法（目标功率 → 构建多设备 LP → 容量/爬坡/SOC 约束 → Solver 求解 → DispatchPlan → 下发各设备）
  - [x] SubTask 8.3: Mermaid 图 2：dispatch 决策流程（目标校验 → 陈旧清理 → SOC 过滤 → 空池校验 → LP 构建 → solve 成功且 Optimal 且长度匹配 ?/fallback → clamp → 更新 last_setpoints → Ok）
  - [x] SubTask 8.4: D1~D14 偏差声明表完整（从 spec.md 复制）
  - [x] SubTask 8.5: 前置依赖引用 v0.72.0 Energy Agent + v0.64.0 Solver trait + v0.66.0 LP 模型
  - [x] SubTask 8.6: 性能目标（求解 < 500ms，蓝图 §6.3/§7.2，标注"集成阶段验收，本版本仅算法骨架"）
  - [x] SubTask 8.7: 下游引用 v0.88.0 多目标优化 + VPP 聚合出口关联
  - [x] SubTask 8.8: 选型对比表（比例分配/贪心/LP，蓝图 §5.1：LP ⭐ 全局最优、比例分配兜底、贪心不采用）
  - [x] SubTask 8.9: 错误处理章节：DispatchError 2 变体 + solver 失败→平均分配兜底映射（蓝图 §4.4）

- [x] Task 9: 版本同步根目录文件
  - [x] SubTask 9.1: 根 `Cargo.toml` `[workspace.package] version = "0.86.0"` → `"0.87.0"`（members 不变）
  - [x] SubTask 9.2: `Makefile` `# Version: v0.87.0` + `VERSION := 0.87.0`
  - [x] SubTask 9.3: `.github/workflows/ci.yml` `# Version: v0.87.0`
  - [x] SubTask 9.4: `ci/src/gate.rs` clippy 段 + test 段注释追加 `+ v0.87.0 多设备调度：DeviceMode / DeviceCapability / DevicePool / DeviceAssignment / DispatchPlan / DispatchError / MultiDeviceDispatcher / equal_split`

- [x] Task 10: 构建校验（§2.4.2 C6~C11）
  - [x] SubTask 10.1: `cargo metadata --format-version 1` 成功
  - [x] SubTask 10.2: `cargo test -p eneros-energy-market-agent` 全部通过（104 既有 + T81~T120 40 新增 = 144 tests，0 failures）
  - [x] SubTask 10.3: `cargo build -p eneros-energy-market-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] SubTask 10.4: `cargo fmt -p eneros-energy-market-agent -- --check` 通过
  - [x] SubTask 10.5: `cargo clippy -p eneros-energy-market-agent --all-targets -- -D warnings` 无 warning
  - [x] SubTask 10.6: `cargo deny check advisories licenses bans sources` 通过（无新依赖）
  - [x] SubTask 10.7: 回归 — `cargo test -p eneros-grid-agent`（130 + 1 doctest）/ `cargo test -p eneros-device-agent`（24）
  - [x] SubTask 10.8: 回归 — `cargo test -p eneros-tsn-time`（84）/ `cargo test -p eneros-agent-bus-dds`（63）

# Task Dependencies

- [Task 2] depends on [Task 1]
- [Task 3] depends on [Task 1]（multi_dispatch.rs use device_pool 类型）
- [Task 4] depends on [Task 3]
- [Task 5] depends on [Task 1, Task 3]
- [Task 6] 独立（可与 1~4 并行）
- [Task 7, Task 8] 独立（可与 1~6 并行）
- [Task 9] depends on [Task 5, Task 6]
- [Task 10] depends on [Task 5, Task 9]

# 并行执行计划

- **Sub-Agent A**：Task 1 + Task 2 + Task 3 + Task 4（同 crate 源文件，串行单 agent 保证一致性）
- **Sub-Agent B**：Task 7 + Task 8（configs + docs，与 A 并行）
- **Sub-Agent C**：Task 5 + Task 6 + Task 9（lib.rs + Cargo.toml + 版本同步，待 A 完成后启动）
- **主 agent**：Task 10（全部完成后统一构建校验）
