# v0.93.0 Edge Coordinator — 域级优化 Spec

## Why

v0.92.0 解决了多 Agent 争抢单资源的仲裁问题，但域内多台 Edge Box 各自为政仍会导致园区级局部最优。蓝图 phase2 v0.93.0（P2-D 第 2 版）要求实现 Edge Coordinator 域级能源优化：收集域内所有 Edge Box 状态 → 构建域级 LP（各 box 容量/平衡约束）→ Solver 求解 → 下发各 EdgeBox，使园区级整体收益优于单机独立调度，为 v0.94.0 VPP 聚合提供优化基础。

## What Changes

- 在既有 `crates/agents/coordinator/` 内**新增 1 个源文件**（Surgical：arbiter.rs / bid.rs / conflict.rs 零改动）：
  - `src/domain_optimizer.rs` — `EdgeBoxState`（含 `online`，D8）/ `DomainPlan` / `OptError` / `DomainOptimizer`（BTreeMap 盒管理 + 域级 LP + 容量比例兜底 + 3 个可观测计数器，D9）+ `build_domain_lp` 内部构造函数
- `src/lib.rs` 仅追加 `pub mod domain_optimizer;` + 4 项重导出 + crate 文档升级 v0.93.0（含 D1~D12 偏差简表）
- `Cargo.toml`（coordinator）dependencies 追加 `eneros-solver-core` + `eneros-energy-market-agent`（复用 DevicePool/DeviceCapability/DeviceAssignment/DispatchPlan/equal_split/MarketData，防重复造轮子）
- 新增 `configs/domain_optimizer.toml`（`[domain_optimizer]` 域级优化配置 + 中文注释）
- 新增 `docs/agents/domain-optimizer-design.md`（12 章节 + 2 Mermaid + D1~D12 偏差表）
- 根目录 4 文件版本同步 0.92.0 → 0.93.0（Cargo.toml / Makefile / ci.yml / gate.rs 注释）
- 内嵌单元测试 40 个（T1~T40），含 5-Box 集成、Box 离线重优化故障注入、NaN 风暴防御
- **无 BREAKING**：既有 40 个 v0.92.0 测试与全部下游 crate 零影响

## Impact

- Affected specs：develop-v0920-edge-arbiter（同 crate 追加模块）；关联 develop-v0870-multi-dispatch（LP/equal_split 复用源）、develop-v0850-market-subscription（MarketData 复用源）
- Affected code：`crates/agents/coordinator/`（新增 1 文件 + lib.rs 追加 + Cargo.toml 依赖追加）、`configs/`、`docs/agents/`、根 4 文件
- 依赖：`eneros-solver-core`（Solver/LpProblem，v0.64.0）、`eneros-energy-market-agent`（DevicePool/DeviceCapability/DispatchPlan/equal_split/MarketData，v0.85.0/v0.87.0）
- 下游解锁：v0.94.0 VPP 聚合（Phase 2 出口标准）、v0.96.0 Cloud Coordinator

## 偏差声明（D1~D12，Karpathy Think Before Coding：显式取舍）

| 偏差 | 蓝图原文 | 本版本处理 |
|------|---------|-----------|
| **D1** | crate 路径 `crates/coordinator/src/domain_optimizer.rs` | 既有 `crates/agents/coordinator/src/domain_optimizer.rs`（项目 §2.3.1 硬规则，v0.92.0 D1 惯例；同 crate 追加模块） |
| **D2** | `box_id: String` / `box_plans: HashMap<String, DispatchPlan>` | `box_id: u64` / `BTreeMap<u64, DispatchPlan>`（无堆字符串 + 确定性迭代，v0.87.0 D3 惯例；BTreeMap 保证 LP 列映射与计划下发顺序可重放） |
| **D3** | `socs: HashMap<DeviceId, f32>` | `socs: BTreeMap<u64, f32>`（DeviceId=u64，同 v0.87.0 dispatch 签名，确定性） |
| **D4** | `solver: Arc<dyn Solver>` | `solver: Box<dyn Solver>`（no_std 单线程无共享所有权需求，v0.87.0 D5 惯例；Solver trait 本就 `&mut self` 不可共享） |
| **D5** | `pub async fn optimize(&self, ...)` | sync `optimize(&mut self, market, target_mw, now_ms)`（no_std 无 async runtime；`&mut` 因 `Solver::solve` 需 `&mut` 且计数器更新，v0.87.0 D1 惯例） |
| **D6** | `now_ms()` 内部时间源 | `now_ms: u64` 外部时间注入（no_std 无 Instant，全项目统一惯例）；`DomainPlan.timestamp = now_ms` |
| **D7** | 蓝图 LP 退化：`domain_balance` 约束系数为空、`Σp ≤ Σcapacity` 被单变量上界隐含（无实际耦合），且无 target | 实现**有实际域级耦合的 LP**：每在线 box 的每台合格设备一个变量 `p_{box}_{dev}`（bounds [p_min, p_max]）；① 域平衡行 `Σp = target_mw`；② 每 box 容量行 `Σ_{i∈box} p_i ≤ capacity_mw`；目标 Minimize `Σ(1−eff_i)·p_i`（损耗最小，v0.87.0 D14 一致）。蓝图 `optimize(market)` 无 target 参数 → 增加 `target_mw: f32` 注入（域级调度目标，下游由仲裁/计划给定） |
| **D8** | 蓝图 `EdgeBoxState` 无 online 字段，但 §4.4 要求"Edge Box 离线 → 从优化中排除"且 §6.5 要求离线故障注入 | `EdgeBoxState` 增加 `online: bool` + `DomainOptimizer::set_online(box_id, online)`（离线 box 从 LP 与 DomainPlan 中排除，不删除状态便于恢复） |
| **D9** | 蓝图 §9 可观测要求"优化收益 metric" | `DomainOptimizer` 3 个 pub 计数器：`optimize_count` / `fallback_count` / `empty_count`（v0.92.0 D9 惯例；收益经 `DomainPlan.total_revenue` 可观测） |
| **D10** | §4.4"LP 不可行 → 放松约束" | 落地为**确定性容量比例兜底**（不迭代重试 LP）：solver Err / Infeasible / 解长度不符 → 活跃 box 间按 `capacity_mw` 比例分摊 target，box 内复用 v0.87.0 `equal_split`（clamp [p_min, p_max]）；`objective_value = 0.0`（v0.87.0 D8 惯例：失败为兜底非错误） |
| **D11** | §7.3 安全"不超域级容量" | `target_mw > Σ在线 capacity` → **clamp 到总在线容量**后再建 LP（构造上保证不超发，不报错中断调度）；`target_mw` 非有限 → `Err(OptError::InvalidTarget)` |
| **D12** | 蓝图未定义 `total_revenue` 公式；未覆盖 NaN | 净收益语义：`total_revenue = price × (total_power − total_loss)`（`total_loss = Σ(1−eff_i)·p_i`，使损耗最小化直接转化为收益，支撑 §7.2"收益 > 单机"可判定）；NaN 防御（v0.88.0 C140 教训）：soc NaN → 按耗尽跳过该设备；capacity 非有限或 ≤0 → box 排除；efficiency 非有限或越出 [0,1] → clamp（NaN→0.5 中性）；price 非有限 → revenue 按 0.0 |

## ADDED Requirements

### Requirement: 域状态数据结构

系统 SHALL 提供：`EdgeBoxState { box_id: u64, devices: DevicePool, socs: BTreeMap<u64, f32>, capacity_mw: f32, online: bool }`（Debug/Clone）、`DomainPlan { box_plans: BTreeMap<u64, DispatchPlan>, total_revenue: f32, timestamp: u64 }`（Debug/Clone/PartialEq/Default）、`OptError { EmptyDomain, InvalidTarget }`（Debug/Clone/Copy/PartialEq/Eq，Solver 失败为兜底非错误，D10），全部 no_std + alloc 兼容；`DevicePool`/`DispatchPlan` 等复用 `eneros-energy-market-agent`（不重复定义）。

#### Scenario: 盒管理与状态回显

- **WHEN** 创建 `EdgeBoxState { box_id: 7, capacity_mw: 5.0, online: true, .. }` 并 `add_box` 入优化器
- **THEN** `edge_boxes[7]` 字段回显正确；同 id 再次 add 覆盖；`remove_box(7)` → true，再删 → false
- **WHEN** `set_online(7, false)`
- **THEN** 该 box `online == false` 且后续 optimize 不再纳入；不存在的 id → 返回 false

### Requirement: 域级 LP 构建

系统 SHALL 提供内部 `build_domain_lp`（私有，测试直接调用）：合格设备 = 所属 box `online && sanitize(capacity) > 0` 且设备 SOC 合格（有 soc 记录时 NaN 或 ≤0 → 跳过，D12）；变量每合格设备一个 `p_{box_id}_{dev_id}`，bounds `[p_min, p_max]`，Continuous；目标系数 `1 − sanitize(eff)`（D12 clamp [0,1]，NaN→0.5）；约束行 0 为域平衡 `Σp = clamped_target`（rhs 上下界相等），其后每合格 box 一行容量约束 `Σ_{i∈box} p_i ≤ capacity`（rhs_lower = `-f64::INFINITY`，rhs_upper = capacity）；列序按 (box_id, dev_id) 升序（BTreeMap 确定性，D2）。

#### Scenario: LP 结构确定性

- **WHEN** 2 个在线 box（各 2 台合格设备）target=8.0
- **THEN** 变量 4 个按 box/dev 升序；共 3 行约束（1 平衡 + 2 容量）；平衡行 rhs == 8.0；容量行 rhs_upper 分别为两 box 容量
- **WHEN** 同一输入两次 build
- **THEN** LpProblem 逐字段一致（确定性可重放）

### Requirement: DomainOptimizer 域级优化

系统 SHALL 提供 `DomainOptimizer { edge_boxes: BTreeMap<u64, EdgeBoxState>, solver: Box<dyn Solver>, optimize_count, fallback_count, empty_count }`（字段全 pub，D9）与 `new(solver)`（计数器全零）、`add_box` / `remove_box` / `set_online`、`optimize(&mut self, market: &MarketData, target_mw: f32, now_ms: u64) -> Result<DomainPlan, OptError>`：`optimize_count += 1` → target 非有限 `Err(InvalidTarget)` → 无合格 box/设备 `empty_count += 1` + `Err(EmptyDomain)` → target clamp 到总在线容量（D11）→ build_domain_lp → solve：Optimal 且解长度匹配 → 按 box 聚合为 `DispatchPlan`（逐设备 clamp [p_min, p_max]，box 内 `objective_value = Σ(1−eff)·p_i`），否则 → `fallback_count += 1` + D10 容量比例兜底；`total_revenue` 按 D12 净收益计算；`timestamp = now_ms`。

#### Scenario: 多 Box 协同分配

- **WHEN** 2 在线 box（cap 6/4 MW，各 1 设备 eff 0.95/0.75）target=8.0，solver 返回 Optimal 解 [6.0, 2.0]
- **THEN** `box_plans` 含 2 项，box1 total_power=6.0、box2=2.0；`timestamp == now_ms`；`total_revenue = price × (8.0 − (0.05×6 + 0.25×2))`

#### Scenario: 离线 Box 排除重优化（蓝图 §4.4/§6.5）

- **WHEN** 3 在线 box 优化后 `set_online(2, false)` 再 optimize
- **THEN** 第二次 `box_plans` 不含 box 2，target 全部分摊给 box 1/3；`set_online(2, true)` 后恢复纳入

#### Scenario: LP 失败容量比例兜底（D10）

- **WHEN** solver 返回 Err / Infeasible / 解长度不符，2 box cap 6/4，target=10
- **THEN** `fallback_count += 1`；box1 分得 6.0、box2 分得 4.0（容量比例 + equal_split clamp）；`objective_value == 0.0`；revenue 仍按实际分配计算

#### Scenario: 域容量安全（蓝图 §7.3，D11）

- **WHEN** target=100.0 但总在线 capacity=10.0
- **THEN** 不报错，LP 平衡目标被 clamp 到 10.0，plan 总出力 ≤ 10.0

#### Scenario: 空域与非法目标

- **WHEN** 无 box / 全离线 / 全设备 SOC 耗尽 → **THEN** `Err(EmptyDomain)` + `empty_count == 1`
- **WHEN** target 为 NaN / ±Inf → **THEN** `Err(InvalidTarget)`，不产生任何 plan

#### Scenario: 收益优于单机（蓝图 §7.2）

- **WHEN** 高效 box（eff 0.95）与低效 box（eff 0.75）共存，LP 最优解将出力集中于高效 box
- **THEN** 优化路径 `total_revenue` 严格大于同输入兜底（容量比例）路径的 revenue（净收益语义可判定，D12）

### Requirement: 域级优化配置

系统 SHALL 提供 `configs/domain_optimizer.toml`：`[domain_optimizer]` 段（`max_boxes` 内存上限 / `fallback = "capacity_proportional"`），中文注释含求解 <2s（蓝图 §6.3，集成阶段验收）/ 域容量安全（§7.3，D11）/ 离线排除（§4.4，D8）/ 状态一致性坑点（§8.5）/ 动态 EdgeBox 增删（§9 可扩展）/ NaN 防御（D12）。

## MODIFIED Requirements

### Requirement: coordinator crate 集成与版本

`src/lib.rs` crate 文档升级为 v0.92.0 + v0.93.0 双版本说明（域内仲裁 + 域级优化），追加 `pub mod domain_optimizer;` 与 4 项重导出（`DomainOptimizer, EdgeBoxState, DomainPlan, OptError`）。**既有 pub 项与 3 个既有模块零改动。** coordinator `Cargo.toml` dependencies 追加 `eneros-solver-core = { path = "../../ai/solver-core" }` 与 `eneros-energy-market-agent = { path = "../energy-market-agent" }`（均为 workspace 内既有 crate，SBOM 无新第三方依赖）。根 `Cargo.toml` version = "0.93.0"，`Makefile` / `ci.yml` / `gate.rs` 注释同步。

## REMOVED Requirements

无。
