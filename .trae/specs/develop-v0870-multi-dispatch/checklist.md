# Checklist

## Task 1: device_pool.rs — 设备能力模型与设备池
- [x] C1: `crates/agents/energy-market-agent/src/device_pool.rs` 文件创建
- [x] C2: `DeviceMode` 枚举 2 变体 `Auto` / `Manual`
- [x] C3: `DeviceMode` 派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `Auto`）
- [x] C4: `DeviceCapability` 结构体 4 字段（`p_min: f32` / `p_max: f32` / `ramp_rate: f32` / `efficiency: f32`）
- [x] C5: `DeviceCapability` 派生 `Debug, Clone, Copy, PartialEq, Default`
- [x] C6: `DevicePool` 结构体字段 `devices: BTreeMap<u64, DeviceCapability>`，pub
- [x] C7: `DevicePool` 派生 `Debug, Clone, Default`
- [x] C8: `DevicePool::new()` 返回空池
- [x] C9: `DevicePool::add_device(&mut self, id, cap)` 同 id 覆盖
- [x] C10: `DevicePool::remove_device(&mut self, id) -> bool` 存在返回 true，不存在返回 false
- [x] C11: `DevicePool::get(&self, id) -> Option<&DeviceCapability>`
- [x] C12: `DevicePool::len(&self) -> usize` / `is_empty(&self) -> bool`
- [x] C13: `device_pool.rs` 使用 `use alloc::collections::BTreeMap;`（no_std 合规）
- [x] C14: `device_pool.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe` / 无 `todo!` / 无 `unimplemented!`
- [x] C15: `device_pool.rs` 中文模块文档注释（v0.87.0 + 偏差 D3/D4/D7 引用）

## Task 2: device_pool.rs — 单元测试 T81~T92
- [x] C16: T81 — `DeviceMode::default() == Auto`；2 变体 Debug 非空
- [x] C17: T82 — `DeviceCapability::default()` 4 字段全 0.0
- [x] C18: T83 — `DeviceCapability` 显式构造与字段访问（p_min=0.0 / p_max=5.0 / ramp_rate=1.0 / efficiency=0.9）
- [x] C19: T84 — `DeviceCapability` Copy 可复制
- [x] C20: T85 — `DevicePool::new()` `is_empty()==true` / `len()==0`
- [x] C21: T86 — `add_device(1, cap)` → `len()==1` / `get(1)==Some(&cap)`
- [x] C22: T87 — 同 id 重复 add_device 覆盖（get 返回新 cap，len 仍 1）
- [x] C23: T88 — `remove_device(1)` 存在 → `true` + `get(1)==None` + `len()==0`
- [x] C24: T89 — `remove_device(99)` 不存在 → `false`，len 不变
- [x] C25: T90 — 按 30/10/20 插入 3 台，`keys().copied().collect::<Vec<_>>() == [10, 20, 30]`（有序迭代确定性，D3）
- [x] C26: T91 — `DevicePool::default()` 与 `new()` 等价；Clone 后 get 结果一致
- [x] C27: T92 — 3 台设备 add 后 `len()==3`；remove 1 台后 `len()==2`

## Task 3: multi_dispatch.rs — 数据结构 + DispatchError + equal_split + MultiDeviceDispatcher
- [x] C28: `crates/agents/energy-market-agent/src/multi_dispatch.rs` 文件创建
- [x] C29: `DeviceAssignment` 结构体 3 字段（`device_id: u64` / `setpoint: f32` / `mode: DeviceMode`）
- [x] C30: `DeviceAssignment` 派生 `Debug, Clone, Copy, PartialEq, Default`
- [x] C31: `DispatchPlan` 结构体 4 字段（`timestamp: u64` / `assignments: Vec<DeviceAssignment>` / `total_power: f32` / `objective_value: f32`）
- [x] C32: `DispatchPlan` 派生 `Debug, Clone, PartialEq, Default`（含 Vec 不派生 Copy）
- [x] C33: `DispatchError` 枚举 2 变体 `EmptyPool` / `InvalidTarget`
- [x] C34: `DispatchError` 派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C35: `equal_split(target: f32, caps: &[(u64, DeviceCapability)]) -> Vec<DeviceAssignment>` 存在
- [x] C36: `equal_split` 空 caps → 返回空 Vec（无 panic）
- [x] C37: `equal_split` 非空时 `setpoint = (target / n).max(p_min).min(p_max)`，mode = Auto
- [x] C38: `MultiDeviceDispatcher` 结构体 3 字段（`pool: DevicePool` / `solver: Box<dyn Solver>` / `last_setpoints: BTreeMap<u64, f32>`）
- [x] C39: `MultiDeviceDispatcher::new(pool, solver) -> Self`（`last_setpoints = BTreeMap::new()`）
- [x] C40: `dispatch(&mut self, target, socs, now_ms)` step 1：`!target.is_finite()` → `Err(InvalidTarget)`
- [x] C41: `dispatch` step 2：`last_setpoints.retain(|id, _| pool.devices.contains_key(id))`（陈旧清理）
- [x] C42: `dispatch` step 3：SOC 过滤（`socs.get(id)` 为 `Some(soc)` 且 `soc <= 0.0` → 跳过），收集 `eligible: Vec<(u64, DeviceCapability)>`
- [x] C43: `dispatch` step 4：`eligible.is_empty()` → `Err(EmptyPool)`
- [x] C44: `dispatch` step 5：构建 `LpProblem`（n 个 Continuous 变量 `p_{id}`，界 [p_min, p_max]，objective[i]=1.0-eff_i，sense=Minimize，平衡行系数 1.0，rhs_lower=rhs_upper=target）
- [x] C45: `dispatch` step 5：爬坡行（首次无 last_setpoint 时不生成，有 last_setpoint 时生成双边约束 `prev - ramp <= p <= prev + ramp`）
- [x] C46: `dispatch` step 6：`solver.solve` Ok 且 `status == Optimal` 且 `solution.len() == n` → 采用解（setpoint clamp [p_min, p_max]）；否则 `equal_split` + `objective_value = 0.0`
- [x] C47: `dispatch` step 7：`last_setpoints` 更新为本次 setpoint
- [x] C48: `dispatch` step 8：`Ok(DispatchPlan)` 含 `timestamp=now_ms` / `assignments` / `total_power`（setpoints 之和）/ `objective_value`
- [x] C49: 私有 `build_lp_problem` 函数存在（被 dispatch 调用）
- [x] C50: `multi_dispatch.rs` 使用 `alloc::boxed::Box` + `alloc::collections::BTreeMap` + `alloc::string::String` + `alloc::vec::Vec` + `alloc::format!`
- [x] C51: `multi_dispatch.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe` / 无 `todo!` / 无 `unimplemented!` / 无 `unwrap()`
- [x] C52: `multi_dispatch.rs` 中文模块文档注释（v0.87.0 + 偏差 D1/D5/D6/D8/D9/D10/D13/D14 引用）

## Task 4: multi_dispatch.rs — 单元测试 T93~T120（含 RecordingSolver）
- [x] C53: 测试辅助 `RecordingSolver` 存在（impl `Solver`，含 `recorded: Option<LpProblem>` / `result: Option<SolveResult>` / `fail: bool`）
- [x] C54: `RecordingSolver::solve` 记录 `LpProblem` 克隆；fail → `Err(SolverError::RunFailed(-1))`；否则返回预设 result
- [x] C55: T93 — `DeviceAssignment::default()` 全零 + `mode == Auto`
- [x] C56: T94 — `DeviceAssignment` 3 字段显式构造 + Copy 可复制
- [x] C57: T95 — `DispatchPlan::default()`：timestamp 0 / assignments 空 / total_power 0.0 / objective_value 0.0
- [x] C58: T96 — `DispatchPlan` 4 字段显式构造 + Clone
- [x] C59: T97 — `DispatchError` 2 变体 PartialEq（相等+互不相等）+ Debug 非空
- [x] C60: T98 — `equal_split(10.0, 2 台 p_max≥5)` → setpoint 均 5.0，mode Auto
- [x] C61: T99 — `equal_split(10.0, [p_max=3.0, p_max=5.0])` → [3.0, 5.0]（clamp p_max，§7.3）
- [x] C62: T100 — `equal_split(-10.0, [p_min=-2.0 ×2])` → [-2.0, -2.0]（clamp p_min，负 target 充电场景）
- [x] C63: T101 — `equal_split(10.0, &[])` → 空 Vec，无 panic
- [x] C64: T102 — `dispatch` target 为 NaN 与 `f32::INFINITY` → `Err(InvalidTarget)`（solver 未被调用）
- [x] C65: T103 — 空 pool dispatch → `Err(EmptyPool)`
- [x] C66: T104 — 全部设备 soc=0.0 → `Err(EmptyPool)`
- [x] C67: T105 — 设备 1 soc=0.0（跳过）+ 设备 2 soc=0.5 → assignments 仅含设备 2（RecordingSolver solution 长度 1）
- [x] C68: T106 — happy path：2 设备 + RecordingSolver 返回 Optimal [3.0, 2.0] objective 0.5 → assignments [id=1 sp=3.0, id=2 sp=2.0] / total_power==5.0 / objective_value==0.5 / timestamp==now_ms / mode Auto / ids 有序
- [x] C69: T107 — LP 变量构造：`variables.len()==2` / `lower_bounds==[p_min]` / `upper_bounds==[p_max]` / `var_types` 全 Continuous / 变量名 `p_1` `p_2`
- [x] C70: T108 — 平衡行：`num_rows==1`（首次无爬坡）/ `row_start==[0,2]` / `col_index==[0,1]` / `values==[1.0,1.0]` / `rhs_lower[0]==rhs_upper[0]==target`
- [x] C71: T109 — 目标系数：`objective[i]==1.0-eff_i` / `sense==Minimize`（eff 0.9→0.1，eff 0.8→0.2，f64 EPSILON）
- [x] C72: T110 — 首次 dispatch `num_rows==1`；成功后第二次 dispatch `num_rows==3`（2 设备均有 last_setpoint 各加 1 爬坡行）
- [x] C73: T111 — 爬坡行语义：prev=3.0 / ramp=1.0 → `rhs_lower[1]==2.0` / `rhs_upper[1]==4.0` / 该设备列系数 1.0
- [x] C74: T112 — dispatch 后 `last_setpoints` 更新为 setpoint（{1:3.0, 2:2.0}）
- [x] C75: T113 — solver `fail=true`（Err(RunFailed)）→ fallback：平均分配 clamp + `objective_value==0.0` + `Ok(plan)`
- [x] C76: T114 — solver 返回 `SolveStatus::Infeasible` → fallback（同 T113 断言）
- [x] C77: T115 — solver 返回 Optimal 但 `solution.len() != n`（如空 Vec）→ fallback
- [x] C78: T116 — solver 返回 setpoint 超 p_max（如 9.0 > 5.0）→ clamp 到 5.0
- [x] C79: T117 — solver 路径 `total_power == setpoints 之和`（clamp 后，D13）
- [x] C80: T118 — 5 设备协同（§6.2）：5 设备 RecordingSolver 返回 `[1.0,2.0,3.0,4.0,5.0]` → 5 条 assignments、id 有序、total==15.0
- [x] C81: T119 — 设备增减兼容（§6.4）：dispatch → `add_device(3, cap)` → 再 dispatch → assignments 含设备 3
- [x] C82: T120 — 设备离线重分配（§6.5）：3 设备 dispatch → `remove_device(2)` → 再 dispatch → assignments 仅 1、3 且 `last_setpoints` 无设备 2 条目

## Task 5: lib.rs surgical 修改
- [x] C83: `pub mod device_pool;` + `pub mod multi_dispatch;` 追加
- [x] C84: `pub use device_pool::{DeviceCapability, DeviceMode, DevicePool};` 重导出
- [x] C85: `pub use multi_dispatch::{equal_split, DeviceAssignment, DispatchError, DispatchPlan, MultiDeviceDispatcher};` 重导出
- [x] C86: 顶部模块文档注释追加 `# v0.87.0 多设备调度` 段落（核心类型列表 + v0.87.0 D1~D14 偏差表）
- [x] C87: v0.72.0 既有 5 个私有 `mod`（energy_agent/error/market/market_agent/runtime）保留不变
- [x] C88: v0.85.0 3 个 `pub mod`（market_feed/parser/subscriber）+ 3 行 `pub use` + v0.85.0 文档段落保留
- [x] C89: v0.86.0 1 个 `pub mod`（bid_generator）+ 1 行 `pub use` + v0.86.0 文档段落保留
- [x] C90: v0.72.0 既有 24 个测试 + v0.85.0 18 个测试 + v0.86.0 38 个测试 = 104 测试保留不变
- [x] C91: `lib.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe`

## Task 6: Cargo.toml description 更新
- [x] C92: `description` 字段更新为含 "v0.87.0 多设备调度" 字样
- [x] C93: `[dependencies]` 段不变（无新依赖，复用既有 eneros-solver-core）
- [x] C94: workspace members 列表不变

## Task 7: configs/device_pool.toml
- [x] C95: 文件位于 `configs/device_pool.toml`
- [x] C96: 含 `[[device]]` 数组段 ×3（储能/光伏/充电桩），每段含 `id` / `p_min` / `p_max` / `ramp_rate` / `efficiency`
- [x] C97: 中文注释说明各字段单位（MW / MW·min⁻¹ / 0~1）与 D4（id 用 u64 非 String）
- [x] C98: 注释说明 SOC 过滤规则（soc <= 0 本轮跳过，D10）与平均分配兜底（蓝图 §4.4）

## Task 8: docs/agents/multi-dispatch-design.md
- [x] C99: 文件位于 `docs/agents/multi-dispatch-design.md`（非 `docs/phase2/`，D12 + 工作区规则 §2.3.3）
- [x] C100: 12 章节完整（版本目标 / 前置依赖 / 交付物清单 / 数据结构 / 接口设计 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险与坑点 / 偏差声明）
- [x] C101: 至少 1 个 Mermaid 图（蓝图 §4.3 核心算法：目标功率 → 构建 LP → 约束 → Solver → DispatchPlan → 下发）
- [x] C102: 至少 1 个 Mermaid 图（dispatch 决策流程：校验 → 清理 → SOC 过滤 → LP → solve → clamp → Ok）
- [x] C103: D1~D14 偏差声明表完整
- [x] C104: 引用 v0.72.0 Energy Agent + v0.64.0 Solver trait + v0.66.0 LP 模型作为前置依赖
- [x] C105: 包含性能目标说明（求解 < 500ms，蓝图 §6.3/§7.2，标注"集成阶段验收，本版本仅算法骨架"）
- [x] C106: 引用 v0.88.0 多目标优化 + VPP 聚合作为下游消费者
- [x] C107: 包含选型对比表（比例分配 / LP / 贪心，蓝图 §5.1：LP ⭐ 全局最优、比例分配兜底、贪心不采用）
- [x] C108: 错误处理章节含 DispatchError 2 变体 + solver 失败→平均分配兜底映射

## Task 9: 版本同步根目录文件
- [x] C109: 根 `Cargo.toml` 顶层 `[workspace.package] version = "0.87.0"`
- [x] C110: 根 `Cargo.toml` `[workspace.members]` 列表**不变**
- [x] C111: `Makefile` 中 `# Version: v0.87.0` 与 `VERSION := 0.87.0`
- [x] C112: `.github/workflows/ci.yml` 中 `# Version: v0.87.0`
- [x] C113: `ci/src/gate.rs` clippy 段注释含 `+ v0.87.0 多设备调度：DeviceMode / DeviceCapability / DevicePool / DeviceAssignment / DispatchPlan / DispatchError / MultiDeviceDispatcher / equal_split`
- [x] C114: `ci/src/gate.rs` test 段注释同步追加类型列表

## Task 10: 构建校验（§2.4.2）
- [x] C115: `cargo metadata --format-version 1` 成功
- [x] C116: `cargo test -p eneros-energy-market-agent` 全部通过（104 既有 + T81~T120 40 新增 = 144 tests，0 failures）
- [x] C117: `cargo build -p eneros-energy-market-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 退出码 0
- [x] C118: `cargo fmt -p eneros-energy-market-agent -- --check` 退出码 0
- [x] C119: `cargo clippy -p eneros-energy-market-agent --all-targets -- -D warnings` 无 warning，退出码 0
- [x] C120: `cargo deny check advisories licenses bans sources` 通过（无新依赖引入）
- [x] C121: 回归 — `cargo test -p eneros-grid-agent` 仍通过 130 tests + 1 doctest（无回归）
- [x] C122: 回归 — `cargo test -p eneros-device-agent` 仍通过 24 tests（无回归）
- [x] C123: 回归 — `cargo test -p eneros-tsn-time` 仍通过 84 tests + `cargo test -p eneros-agent-bus-dds` 仍通过 63 tests（无回归）

## 总体校验
- [x] C124: 无根目录新 crate（`crates/agents/energy-market-agent/` 既有 crate 追加 2 个新模块文件，符合 §2.3.1）
- [x] C125: 无 `docs/` 根目录平面化文档（新文档在 `docs/agents/` 下）
- [x] C126: 无 `config/` 目录（新配置在 `configs/device_pool.toml`）
- [x] C127: `.gitignore` 未需更新（无新文件类型）
- [x] C128: `git status` 无 `target/` / `*.elf` / `*.bin` / `*.dtb` / IDE 缓存被追踪
- [x] C129: 提交信息遵循 Conventional Commits（如 `feat(agents/energy-market-agent): v0.87.0 实现多设备调度`）
- [x] C130: ADR 决策未被违反（未引入研究特性、未自研已有开源替代组件、未超出 v1.0.0 范围）
- [x] C131: no_std 合规性：2 个新文件继承 crate 级 `#![cfg_attr(not(test), no_std)]`
- [x] C132: 内存预算：调度模块 ≤ 1MB（本版本为算法骨架，实际占用远小于此）
- [x] C133: SBOM 未变化（无新第三方依赖，复用既有 eneros-solver-core）
- [x] C134: 文档同步：v0.72.0/v0.85.0/v0.86.0 历史偏差声明保留，v0.87.0 新增 D1~D14 段落
- [x] C135: Surgical Changes 原则：v0.72.0/v0.85.0/v0.86.0 既有源文件 `energy_agent.rs` / `error.rs` / `market.rs` / `market_agent.rs` / `runtime.rs` / `market_feed.rs` / `parser.rs` / `subscriber.rs` / `bid_generator.rs` 完全未改动
- [x] C136: `lib.rs` 仅追加 2 个 `pub mod` + 2 行 `pub use` + 顶部文档注释（不修改任何 v0.72.0/v0.85.0/v0.86.0 既有代码行）
- [x] C137: v0.72.0/v0.85.0/v0.86.0 既有命名不冲突（device_pool/multi_dispatch 为新命名，无重叠）
