# EnerOS Phase 1 内核基座开发计划

## 概述

聚焦 Phase 1 内核基座，以 eneros-network 为集成目标，将 topology + powerflow + equipment 串联为统一管线。按依赖关系自底向上推进。

---

## 当前状态分析

| Crate | 完成度 | 关键缺口 |
|-------|--------|----------|
| eneros-core | 80% | TopologyChange 缺少 branch 参数 |
| eneros-topology | 65% | Cycle 检测未实现、TopologyEngine 无测试 |
| eneros-powerflow | 55% | 缺 IEEE 标准测试验证、JacobianMatrix 未被使用、rated_current_ka 硬编码 |
| eneros-equipment | 65% | 6/10 设备 to_admittance 未实现 |
| eneros-constraint | 50% | N-1/Stability 检查未实现、无测试 |
| eneros-network | 0% | 空壳，无源码 |

---

## 开发任务（按依赖顺序）

### Task 1: eneros-core — 补全 TopologyChange

**文件**: `crates/eneros-core/src/types.rs`

**问题**: `TopologyChange::BranchAdded` 只有 from_bus/to_bus，缺少电气参数 (r, x, b, rate_mva)

**改动**:
- 新增 `BranchParams` 结构体: `r: f64, x: f64, b: f64, rate_mva: f64, name: Option<String>`
- 修改 `TopologyChange::BranchAdded` 携带 `BranchParams`
- 修改 `TopologyChange::BranchRemoved` 携带 `ElementId`（当前已有）

**验证**: `cargo test -p eneros-core` + `cargo check` 全 workspace

---

### Task 2: eneros-topology — 补全功能与测试

**文件**:
- `crates/eneros-topology/src/graph.rs` — 新增 cycle 检测
- `crates/eneros-topology/src/search.rs` — 实现 Cycle 检测
- `crates/eneros-topology/src/engine.rs` — 新增测试

**改动**:
1. `NetworkGraph` 新增 `has_cycle() -> bool` 方法（基于 DFS 回边检测）
2. `TopologySearcher` 实现 `detect_cycle()` 返回 `SearchResult::Cycle`
3. 为 `TopologyEngine` 添加单元测试（并发安全、版本号、批量变更）
4. 适配 Task 1 中 `TopologyChange::BranchAdded` 的新签名

**验证**: `cargo test -p eneros-topology`

---

### Task 3: eneros-equipment — 补全 to_admittance

**文件**: `crates/eneros-equipment/src/models.rs`

**改动** — 为 6 种设备实现 `to_admittance()`:

1. **SynchronousGenerator**: 返回 `AdmittanceContribution { y_series: 1/(jx_d_trans), y_shunt: None }` 连接到 Slack/PV 节点
2. **ThreeWindingTransformer**: 利用已有 `star_impedance()` 方法，计算三侧导纳贡献（返回 3 个 AdmittanceContribution，需调整 trait 或提供专用方法）
3. **ConstantPowerLoad**: PQ 节点不贡献导纳矩阵元素（返回 None），但需文档说明
4. **StaticGenerator**: 同 ConstantPowerLoad，返回 None
5. **ZipLoad**: 根据恒阻抗比例计算 y_shunt = (Z_pct/100) * S_base / V_base^2
6. **CircuitBreaker**: 合闸时 y_series = 无穷大导纳（1e12），分闸时返回 None

**三绕组变压器特殊处理**: `EquipmentModel::to_admittance()` 返回单个贡献，但三绕组变压器需要 3 个。方案：新增 `to_admittance_multi() -> Vec<AdmittanceContribution>` 默认方法，三绕组变压器覆写。

**验证**: `cargo test -p eneros-equipment`

---

### Task 4: eneros-powerflow — IEEE 标准验证 + 修复

**文件**:
- `crates/eneros-powerflow/src/solver.rs` — 修复硬编码、移除未使用参数
- `crates/eneros-powerflow/src/matrix.rs` — 移除未使用的 JacobianMatrix 或让 solver 使用它
- 新增 `crates/eneros-powerflow/src/ieee.rs` — IEEE 标准测试数据

**改动**:
1. 新增 `ieee.rs` 模块，包含 IEEE 14 节点标准测试系统数据（母线、支路参数）
2. 新增测试：`test_ieee14_convergence` — 验证潮流收敛且结果与参考值偏差 < 1e-4 pu
3. 修复 `calculate_branch_flows` 中 `rated_current_ka` 硬编码为 1.0 的问题，改为从参数传入
4. 修复 `calculate_bus_results` 中未使用的 `_ybus`/`_bus_types` 参数
5. 决策：移除 `JacobianMatrix` 结构体（solver 自己构建 Vec<Vec<f64>>），避免冗余

**验证**: `cargo test -p eneros-powerflow`，IEEE 14 测试必须通过

---

### Task 5: eneros-constraint — 实现 N-1 与 Stability 检查

**文件**: `crates/eneros-constraint/src/engine.rs`

**改动**:
1. 实现 `check_stability()` — 基于功率不平衡量和电压偏差判断稳定裕度
2. 实现 `check_n1()` — 需要 topology 和 powerflow 数据：
   - 新增 `N1CheckInput` 结构体：包含当前拓扑和潮流结果
   - 对每个支路逐一断开，重新计算潮流，检查是否越限
   - 返回所有不满足 N-1 的支路列表
3. 修改 `ConstraintEngine` 的 `check_all` 方法，将 N-1 和 Stability 从 `_ => None` 改为调用新实现
4. 添加单元测试

**依赖**: 此任务需要 powerflow 的求解能力，但 constraint 目前 Cargo.toml 已依赖 eneros-powerflow（只是未使用）。需要实际集成。

**验证**: `cargo test -p eneros-constraint`

---

### Task 6: eneros-network — 统一管线实现

**文件**: 新建 `crates/eneros-network/src/`

**改动**:
1. 创建 `src/lib.rs` — 公共 API 重新导出
2. 创建 `src/network.rs` — 核心网络模型

**核心 API 设计**:
```rust
// 从设备库构建网络
let network = PowerNetwork::from_equipment(&equipment_library)?;

// 设置拓扑（母线、支路、开关）
network.add_bus(bus);
network.add_branch(branch);
network.add_switch(switch);

// 一站式求解
let result = network.solve()?;

// N-1 安全校验
let n1_result = network.check_n1()?;

// 约束校验
let violations = network.check_constraints(&constraint_engine)?;
```

**PowerNetwork 结构体**:
- 内部持有 `TopologyEngine` + `EquipmentLibrary` + `PowerFlowSolver` + `ConstraintEngine`
- `from_equipment()` 从设备库自动构建拓扑图和导纳矩阵
- `solve()` 串联：拓扑验证 → 导纳矩阵构建 → 潮流计算 → 结果缓存
- `check_n1()` 串联：遍历支路 → 断开 → 重算潮流 → 约束校验 → 恢复
- `check_constraints()` 对当前潮流结果执行约束校验

3. 创建 `src/builder.rs` — NetworkBuilder 模式
4. 创建 `src/error.rs` — 网络层错误类型
5. 添加集成测试

**验证**: `cargo test -p eneros-network`，集成测试覆盖完整 from_equipment → solve → check_constraints 流程

---

### Task 7: 全局验证与清理

**改动**:
1. `cargo check` 全 workspace 无警告
2. `cargo test` 全 workspace 通过
3. `cargo clippy -- -D warnings` 无警告
4. 清理 `eneros-timeseries/src/engine.rs:63` 中英文混写注释
5. 清理各 crate Cargo.toml 中声明但未使用的依赖

---

## 执行顺序

```
Task 1 (core) → Task 2 (topology) → Task 3 (equipment) → Task 4 (powerflow) → Task 5 (constraint) → Task 6 (network) → Task 7 (全局验证)
```

Task 1-3 可部分并行（core 先行，topology 和 equipment 可并行）。
Task 4 依赖 Task 3（powerflow 需要完整的 to_admittance）。
Task 5 依赖 Task 4（constraint 需要 powerflow 集成）。
Task 6 依赖 Task 2-5 全部完成。
Task 7 最后执行。

---

## 假设与决策

1. **JacobianMatrix 处理**: 移除冗余结构体，solver 内联构建雅可比矩阵
2. **三绕组变压器**: 在 EquipmentModel trait 新增 `to_admittance_multi()` 默认方法
3. **N-1 检查**: 采用逐一断开支路+重算潮流的朴素方法（后续可优化为灵敏度法）
4. **IEEE 测试数据**: 仅实现 IEEE 14 节点（最常用），30/118 留后续
5. **eneros-network**: 不引入新的外部依赖，仅聚合已有 crate

---

## 验证步骤

1. 每个 Task 完成后运行对应 crate 的 `cargo test`
2. Task 6 完成后运行全 workspace `cargo test`
3. Task 7 执行 `cargo clippy -- -D warnings` 全量检查
4. 最终验证：构建一个 IEEE 14 节点网络 → 求解潮流 → N-1 校验 → 约束校验的端到端流程
