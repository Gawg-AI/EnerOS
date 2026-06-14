# Phase 1 内核基座 — 开发执行计划

## 概述

本计划基于对代码库的深入审查，修复已发现的关键 Bug，并完成剩余的 Phase 1 开发任务。

## 当前状态分析

### 已完成
- **eneros-core**: BranchParams、TopologyChange 已补全
- **eneros-topology**: 环检测（DFS parent-tracking）、TopologyEngine 测试
- **eneros-equipment**: 6 种设备 to_admittance 实现、MultiAdmittanceContribution

### 关键 Bug（导致 IEEE 14 不收敛）
通过逐行审查 solver.rs，发现 **3 个致命 Bug**：

#### Bug #1 (P0): 修正量应用顺序错误 — `solver.rs:72-82`
`dx` 向量结构为 `[Δθ₁..Δθₙ, ΔV₁..ΔVₘ]`（先全部 θ 再全部 V），但修正循环在遍历母线时交错取值，导致 Δθ 和 ΔV 被错误地应用到不同变量上。

**当前错误代码：**
```rust
let mut idx = 0;
for (i, &bt) in bus_types.iter().enumerate() {
    if bt != BusTypeNR::Slack { theta[i] += dx[idx]; idx += 1; }
    if bt == BusTypeNR::PQ { v[i] += dx[idx]; idx += 1; }  // 错误：取到了下一个 Δθ
}
```

**修复：** 分两个循环，先应用所有 Δθ，再应用所有 ΔV。

#### Bug #2 (P0): J1 非对角元素多了一个负号 — `solver.rs:186`
标准公式：`∂P_i/∂θ_j = V_i·V_j·(G_ij·sin(θ_i-θ_j) - B_ij·cos(θ_i-θ_j))`（j≠i）
代码多了负号：`-v[i]*v[j]*(g*sin - b*cos)`

#### Bug #3 (P0): J3 非对角元素缺少负号 — `solver.rs:229`
标准公式：`∂Q_i/∂θ_j = -V_i·V_j·(G_ij·cos(θ_i-θ_j) + B_ij·sin(θ_i-θ_j))`（j≠i）
代码缺少负号：`v[i]*v[j]*(g*cos + b*sin)`

### IEEE 14 数据问题
- **Bus 3 应为 PV 母线**（有同步调相机），当前错误标记为 PQ
- **缺少变压器变比（tap ratio）**：支路 4-7(0.978)、4-9(0.969)、5-6(0.932)、7-8(0.969)
- **缺少并联电容器**：Bus 9 的 19.0 MVar 电容器（B=0.19 pu）

### 未完成模块
- **eneros-constraint**: N-1/Stability 检查走 `_ => None`，完全未实现
- **eneros-network**: 空壳（无 lib.rs），未加入 workspace members

---

## 执行计划

### Step 1: 修复 solver.rs 三个致命 Bug

**文件**: `crates/eneros-powerflow/src/solver.rs`

1. **修复修正量应用顺序**（第 72-82 行）：
   - 将单个交错循环拆分为两个独立循环
   - 第一个循环：遍历所有非 Slack 母线，应用 Δθ
   - 第二个循环：遍历所有 PQ 母线，应用 ΔV

2. **修复 J1 非对角符号**（第 186 行）：
   - 移除多余负号：`-v[i]*v[j]*(...)` → `v[i]*v[j]*(...)`

3. **修复 J3 非对角符号**（第 229 行）：
   - 添加缺失负号：`v[i]*v[j]*(...)` → `-v[i]*v[j]*(...)`

### Step 2: 修复 IEEE 14 数据 + 添加变比支持

**文件**: `crates/eneros-powerflow/src/ieee.rs`

1. **Bus 3 类型修正**：`bus_type: 2` → `bus_type: 1`（PV 母线）
2. **Ieee14Branch 添加 `tap_ratio` 字段**：`pub tap_ratio: f64`（默认 1.0）
3. **添加 4 条变压器支路的变比数据**
4. **添加 Bus 9 并联电容器**：在 `Ieee14BusData` 中添加 `shunt_susceptances: Vec<(u32, f64)>` 字段

**文件**: `crates/eneros-powerflow/src/matrix.rs`

1. **YBusMatrix::from_branches 添加变比参数**：
   - 签名改为 `from_branches(branches: &[(ElementId, ElementId, f64, f64, f64, f64)], bus_map, ...)`，第 6 个参数为 tap_ratio
   - 变比为 a 的变压器：`Y_ii += y/a²`, `Y_jj += y`, `Y_ij = Y_ji = -y/a`
2. **添加 `add_shunt` 方法**：`add_shunt(bus_idx, g, b)` 向对角元素添加并联导纳
3. **更新 `from_branches` 调用点**（ieee.rs 中的 `to_solver_input`）

### Step 3: 验证 IEEE 14 收敛

1. 运行 `test_ieee14_convergence` 测试
2. 确认收敛且迭代次数 ≤ 20
3. 验证母线电压在 0.9~1.2 pu 范围内
4. 验证总损耗在合理范围（< 20 MW）

### Step 4: eneros-constraint — N-1 与稳定性检查

**文件**: `crates/eneros-constraint/src/engine.rs`

1. **实现 N-1 检查**：
   - 新增 `check_n1` 方法，接受潮流结果和拓扑信息
   - 对每条支路进行开断模拟：移除支路 → 重新求解潮流 → 检查是否产生新的越限
   - 返回 `Vec<N1Violation>`，包含支路 ID、越限类型、严重程度

2. **实现稳定性检查**：
   - 新增 `check_stability` 方法
   - 检查电压稳定性（连续潮流灵敏度指标）
   - 检查功角稳定性（简化小干扰分析）

**文件**: `crates/eneros-constraint/src/rules.rs`

1. 添加 `N1Violation` 结构体
2. 添加 `StabilityViolation` 结构体

**文件**: `crates/eneros-constraint/src/violation.rs`

1. 更新 `Violation` 枚举或扩展以支持 N-1/稳定性越限

### Step 5: eneros-network — 统一管线实现

**文件**: `Cargo.toml`（根目录）

1. 将 `"crates/eneros-network"` 添加到 workspace members

**文件**: `crates/eneros-network/Cargo.toml`

1. 添加 `eneros-constraint` 依赖

**新建文件**: `crates/eneros-network/src/lib.rs`

核心结构 `PowerNetwork`：
```rust
pub struct PowerNetwork {
    topology: TopologyEngine,
    equipment: EquipmentLibrary,
    solver: PowerFlowSolver,
    constraint: ConstraintEngine,
    config: PowerFlowConfig,
}

impl PowerNetwork {
    // 从设备库构建网络
    pub fn from_equipment(lib: &EquipmentLibrary) -> Result<Self>;

    // 执行潮流计算
    pub fn solve(&self) -> Result<PowerFlowResult>;

    // N-1 安全分析
    pub fn check_n1(&self) -> Result<Vec<N1Result>>;

    // 约束检查
    pub fn check_constraints(&self, result: &PowerFlowResult) -> Vec<Violation>;

    // 完整管线：from_equipment → solve → check_n1 → check_constraints
    pub fn full_analysis(&self) -> Result<NetworkAnalysisResult>;
}
```

**新建文件**: `crates/eneros-network/src/network.rs` — PowerNetwork 实现

**新建文件**: `crates/eneros-network/src/pipeline.rs` — 管线编排逻辑

### Step 6: 全局验证与清理

1. `cargo clippy --workspace` — 修复所有警告
2. `cargo test --workspace` — 确保所有测试通过
3. 清理未使用的依赖和导入
4. 统一代码风格（注释语言、命名规范）

---

## 假设与决策

1. **YBusMatrix::from_branches 签名变更**：添加 tap_ratio 参数，所有现有调用点需更新（2-bus、3-bus 测试中的 tap_ratio 设为 1.0）
2. **N-1 检查策略**：采用简化方案 — 遍历支路开断 + 重解潮流，不做并行优化（Phase 1 目标是功能正确性）
3. **eneros-network 作为集成入口**：PowerNetwork 持有所有子引擎的引用，提供统一 API
4. **稳定性检查范围**：Phase 1 仅实现电压稳定性的简化指标（dV/dP 灵敏度），不做完整的特征值分析

## 验证步骤

1. `cargo test -p eneros-powerflow` — IEEE 14 收敛测试通过
2. `cargo test -p eneros-constraint` — N-1 和稳定性检查测试通过
3. `cargo test -p eneros-network` — 集成管线测试通过
4. `cargo test --workspace` — 全 workspace 测试通过
5. `cargo clippy --workspace` — 无警告
