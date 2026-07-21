# v0.94.0 Edge Coordinator — VPP 聚合 Spec

## Why

v0.93.0 实现域级 LP 优化，但域内 DER 仍需聚合成可市场交易容量才能参与电网调度/需求响应。蓝图 phase2 v0.94.0（P2-D 关键版，★ Phase 2 出口标准）要求实现 VPP 聚合：聚合域内 DER 容量形成 VppProfile（容量/爬坡）→ 聚合出力控制（target 分配到各资源）→ 市场申报（生成 Sell 报价），为 v0.95.0 云端策略下发与 v0.96.0 Cloud Coordinator 提供聚合基础。

## What Changes

- 在既有 `crates/agents/coordinator/` 内**新增 1 个源文件**（Surgical：bid.rs / arbiter.rs / conflict.rs 零改动）：
  - `src/vpp_aggregator.rs` — `ResourceType` / `VppResource`（含 `online`/`efficiency`，D6/D7）/ `VppProfile` / `Allocation` / `AggregatedDispatch` / `VppError` / `VppAggregator`（BTreeMap 资源管理 + aggregate 容量聚合 + dispatch 复用 v0.93.0 DomainOptimizer 分配 + market_bid 复用 v0.86.0 Bid 生成 + 3 个可观测计数器，D9）
- `src/domain_optimizer.rs` **仅可见性调整**：4 个 sanitize 函数 `fn` → `pub(crate) fn`（D12 复用，零逻辑改动）
- `src/lib.rs` 仅追加 `pub mod vpp_aggregator;` + 7 项重导出 + crate 文档升级 v0.94.0（三版本说明 + D1~D12 偏差简表）
- `Cargo.toml`（coordinator）description 追加 v0.94.0（**无新依赖**：Bid/BidSide/BidStrategy/MarketType/Period/MarketData 来自既有依赖 eneros-energy-market-agent）
- 新增 `configs/vpp_aggregator.toml`（`[vpp_aggregator]` + `[[vpp_resource]]` VPP 资源清单示例 + 中文注释）
- 新增 `docs/agents/vpp-aggregation-design.md`（12 章节 + 2 Mermaid + D1~D12 偏差表）
- 根目录 4 文件版本同步 0.93.0 → 0.94.0（Cargo.toml / Makefile / ci.yml / gate.rs 注释）
- 内嵌单元测试 40 个（T1~T40），含 5 资源集成、资源离线重算故障注入、NaN 风暴防御
- **无 BREAKING**：既有 80 个 v0.92.0/v0.93.0 测试与全部下游 crate 零影响

## Impact

- Affected specs：develop-v0930-domain-optimizer（同 crate 追加模块 + sanitize 可见性放宽）；关联 develop-v0860-bid-generation（Bid/BidSide/BidStrategy 复用源）
- Affected code：`crates/agents/coordinator/`（新增 1 文件 + lib.rs 追加 + domain_optimizer.rs 可见性 + Cargo.toml description）、`configs/`、`docs/agents/`、根 4 文件
- 依赖：无新依赖（复用同 crate `DomainOptimizer` v0.93.0、crate 既有依赖 `eneros-energy-market-agent` v0.86.0 Bid 族）
- 下游解锁：v0.95.0 云端策略下发、v0.96.0 Cloud Coordinator（Phase 2 出口标准 VPP 聚合达成）

## 偏差声明（D1~D12，Karpathy Think Before Coding：显式取舍）

| 偏差 | 蓝图原文 | 本版本处理 |
|------|---------|-----------|
| **D1** | crate 路径 `crates/coordinator/src/vpp_aggregator.rs`；文档 `docs/phase2/vpp_aggregation.md` | 既有 `crates/agents/coordinator/src/vpp_aggregator.rs` + `docs/agents/vpp-aggregation-design.md`（项目 §2.3.1/§2.3.3 硬规则，v0.92.0/v0.93.0 D1 惯例；同 crate 追加模块） |
| **D2** | `resource_id: String` / `resources: Vec<VppResource>` / `Allocation.resource_id: String` | `resource_id: u64` / `BTreeMap<u64, VppResource>` / `Allocation.resource_id: u64`（无堆字符串 + 确定性迭代，v0.87.0 D3 / v0.93.0 D2 惯例；聚合与分配顺序可重放） |
| **D3** | `pub async fn dispatch(&self, target_mw)` | sync `dispatch(&mut self, market, target_mw, now_ms)`（no_std 无 async runtime；`&mut` 因 `DomainOptimizer::optimize` 需 `&mut` 且计数器更新，v0.93.0 D5 惯例） |
| **D4** | 蓝图 `AggregatedDispatch` 无 timestamp；`aggregate(&self)` | `AggregatedDispatch.timestamp = now_ms`（u64 ms 外部时间注入，全项目统一惯例）；`aggregate(&mut self)` 因 `aggregate_count` 计数器更新（内部 profile 计算为私有 `&self` 免计数） |
| **D5** | 蓝图 dispatch 调 `MarketData::default()` | `MarketData` **未派生 Default**（v0.85.0 实现核实），蓝图代码无法编译 → dispatch 增加 `market: &MarketData` 显式注入（净收益在 DomainPlan 内计算，v0.93.0 D12 链路透传） |
| **D6** | 蓝图 `VppResource` 无 online 字段，但 §6.5 要求"资源离线 → 聚合重算"且 §9 可靠性要求相同 | `VppResource` 增加 `online: bool` + `set_online(resource_id, online)` + `set_available(resource_id, available_mw)`（§5.4 容量动态变化）；离线资源从聚合与分配排除，状态保留便于恢复（v0.93.0 D8 惯例） |
| **D7** | 蓝图 `VppResource` 5 字段（capacity/available/ramp/type） | 增加 `efficiency: f32`（使复用的 DomainOptimizer 损耗最小目标可区分高效/低效 DER，否则 LP 目标退化为常数、解任意；sanitize NaN→0.5 clamp [0,1]，v0.93.0 D12 一致） |
| **D8** | `dispatch` 调 `self.optimizer.optimize(...)`；"分配失败 → 重新优化" | 复用 v0.93.0 `DomainOptimizer`：sync_boxes 将每在线资源映射为**单设备 box**（box_id=device_id=resource_id，`p_min=0`、`p_max=box capacity=available_mw`，`soc=1.0` 恒通过合格过滤）；"重新优化"落地为 optimizer 内建容量比例兜底（不迭代重试 LP，v0.93.0 D10 惯例） |
| **D9** | 蓝图 §9 可观测要求"聚合容量 metric" | `VppAggregator` 3 个 pub 计数器：`aggregate_count` / `dispatch_count` / `reject_count`（拒绝 = InvalidTarget + InsufficientCapacity + NoResource 三路合计；聚合容量经 `VppProfile` 可观测） |
| **D10** | §4.4"资源不足 → 拒绝或部分响应" | 落地为**拒绝**（确定性， blueprint 关键代码一致）：`target_mw.abs() > profile.available_mw` → `Err(VppError::InsufficientCapacity)`（含负 target 充电场景，abs 判定）；`VppError` 3 变体 `InsufficientCapacity / InvalidTarget / NoResource`（Debug/Clone/Copy/PartialEq/Eq） |
| **D11** | 蓝图 aggregate 实现 `ramp_down = ramp_up`（对称） | 保持对称实现（蓝图关键代码一致）；`ramp_rate` 非有限或 <0 → 按 0 计入 profile（ramp 仅上报不参与分配，不阻断调度） |
| **D12** | 蓝图未覆盖 NaN | NaN 防御（v0.88.0 C140 / v0.93.0 D12 教训）：capacity 非有限或 ≤0 → 资源从聚合排除；available 非有限 → 0 且 clamp [0, capacity]；efficiency NaN→0.5 clamp [0,1]；price 非有限 → bid price 按 0+margin。复用 domain_optimizer 的 `sanitize_capacity`/`sanitize_efficiency`/`sanitize_price`（可见性放宽为 `pub(crate)`，零逻辑改动） |

## ADDED Requirements

### Requirement: VPP 资源与聚合数据结构

系统 SHALL 提供：`ResourceType { Battery, Pv, Load, Charger }`（Debug/Clone/Copy/PartialEq/Eq/Default，默认 Battery）、`VppResource { resource_id: u64, capacity_mw: f32, available_mw: f32, ramp_rate: f32, efficiency: f32, type_: ResourceType, online: bool }`（Debug/Clone/Copy/PartialEq）、`VppProfile { total_capacity_mw, available_mw, ramp_up_mw_per_min, ramp_down_mw_per_min }`（Debug/Clone/Copy/PartialEq/Default）、`Allocation { resource_id: u64, setpoint_mw: f32 }`（Debug/Clone/Copy/PartialEq）、`AggregatedDispatch { target_mw: f32, allocations: Vec<Allocation>, timestamp: u64 }`（Debug/Clone/PartialEq/Default）、`VppError { InsufficientCapacity, InvalidTarget, NoResource }`（Debug/Clone/Copy/PartialEq/Eq），全部 no_std + alloc 兼容；`Bid`/`BidSide`/`BidStrategy`/`MarketType`/`Period`/`MarketData` 复用 `eneros-energy-market-agent`（不重复定义）。

#### Scenario: 资源管理与状态回显

- **WHEN** 创建 `VppResource { resource_id: 3, capacity_mw: 5.0, available_mw: 4.0, online: true, .. }` 并 `add_resource` 入聚合器
- **THEN** `resources[3]` 字段回显正确；同 id 再次 add 覆盖；`remove_resource(3)` → true，再删 → false
- **WHEN** `set_online(3, false)` / `set_available(3, 2.5)`
- **THEN** 对应字段更新且返回 true；不存在的 id → 返回 false；离线资源状态保留不删除（D6）

### Requirement: 容量聚合

系统 SHALL 提供 `aggregate(&mut self) -> VppProfile`（`aggregate_count += 1`）：仅统计 `online && sanitize_capacity(capacity_mw) 有效` 的资源；`total_capacity_mw = Σ capacity`、`available_mw = Σ sanitize(available)`（非有限→0，clamp [0, capacity]，D12）、`ramp_up = ramp_down = Σ sanitize(ramp)`（非有限/负 → 0，D11）；空聚合器/全离线 → 全零 profile。

#### Scenario: 聚合计算与离线重算（蓝图 §6.5）

- **WHEN** 3 在线资源（cap 5/3/2，avail 4/3/2，ramp 1/0.5/0.5）
- **THEN** `total=10.0, available=9.0, ramp_up=ramp_down=2.0`
- **WHEN** `set_online(2, false)` 后再 aggregate
- **THEN** `total=7.0, available=6.0, ramp=1.5`（离线资源即时排除，聚合重算）

### Requirement: 聚合出力分配

系统 SHALL 提供 `VppAggregator { resources: BTreeMap<u64, VppResource>, optimizer: DomainOptimizer, aggregate_count, dispatch_count, reject_count }`（字段全 pub，D9）与 `new(solver)`（计数器全零）、`add_resource` / `remove_resource` / `set_online` / `set_available`、`dispatch(&mut self, market: &MarketData, target_mw: f32, now_ms: u64) -> Result<AggregatedDispatch, VppError>`：`dispatch_count += 1` → target 非有限 `reject_count += 1` + `Err(InvalidTarget)` → `|target| > available` `reject_count += 1` + `Err(InsufficientCapacity)`（D10）→ sync_boxes（D8）→ `optimizer.optimize(market, target, now_ms)`：`Ok(plan)` → flat_map box_plans assignments 为 `allocations`（device_id 即 resource_id，D8 映射），`Err(EmptyDomain)` → `reject_count += 1` + `Err(NoResource)`；`timestamp = now_ms`。

#### Scenario: 多资源聚合分配

- **WHEN** 2 在线资源（r1 avail 6.0 eff 0.95 / r2 avail 4.0 eff 0.75）target=8.0，solver 返回 Optimal 解 [6.0, 2.0]
- **THEN** `allocations` 含 2 项（r1=6.0、r2=2.0，按 resource_id 升序）；`timestamp == now_ms`；`target_mw == 8.0` 回显

#### Scenario: 资源不足拒绝（蓝图 §4.4，D10）

- **WHEN** 总 available=9.0，target=10.0（或 target=-10.0 充电）
- **THEN** `Err(InsufficientCapacity)` + `reject_count == 1`，不产生任何 dispatch

#### Scenario: 离线资源排除分配（蓝图 §6.5）

- **WHEN** 3 在线资源 dispatch 后 `set_online(2, false)` 再 dispatch
- **THEN** 第二次 allocations 不含资源 2，target 全部分配给资源 1/3；`set_online(2, true)` 后恢复纳入

#### Scenario: 分配失败兜底（D8 链路）

- **WHEN** solver 返回 Err / Infeasible / 解长度不符，2 资源 avail 6/4，target=10
- **THEN** DomainOptimizer 内建容量比例兜底生效（v0.93.0 D10）：r1 分得 6.0、r2 分得 4.0；dispatch 仍返回 Ok

### Requirement: 市场申报

系统 SHALL 提供 `market_bid(&self, market: &MarketData, strategy: &BidStrategy, now_ms: u64) -> Vec<Bid>`（纯查询无计数器）：按 resource_id 升序遍历在线资源，跳过 sanitize(available) ≤ 0 者；`quantity = min(available, strategy.max_quantity)`（`max_quantity` 非有限或 ≤0 → 按 available 全额）；`price = sanitize_price(market.current_price as f32) + strategy.margin`；`Bid { bid_id: 从 1 顺序递增（resource_id 升序）, market_type: MarketType::Spot, resource_id, price, quantity, side: BidSide::Sell, period: Period::Flat, timestamp: now_ms }`；空聚合器/全离线 → 空 Vec。

#### Scenario: VPP 卖出报价生成

- **WHEN** 2 在线资源（avail 4.0/6.0），strategy `{ margin: 5.0, max_quantity: 3.0, .. }`，market.current_price=400.0
- **THEN** 2 个 Sell 报价：bid_id 1/2，quantity 3.0/3.0（max_quantity clamp），price 405.0，timestamp == now_ms

### Requirement: VPP 聚合配置

系统 SHALL 提供 `configs/vpp_aggregator.toml`：`[vpp_aggregator]` 段（`max_resources` 内存上限 / `default_margin` 默认报价边际）+ `[[vpp_resource]]` 资源清单示例（≥3 个，覆盖 Battery/Pv/Charger 类型），中文注释含响应 <30s（蓝图 §6.3/§7.2，集成阶段验收）/ 不超聚合容量（§7.3，D10）/ 资源离线重算（§6.5，D6）/ 申报与执行偏差坑点（§8.5）/ 资源清单配置化（§9 可维护）/ NaN 防御（D12）。

## MODIFIED Requirements

### Requirement: coordinator crate 集成与版本

`src/lib.rs` crate 文档升级为 v0.92.0 + v0.93.0 + v0.94.0 三版本说明（域内仲裁 + 域级优化 + VPP 聚合），追加 `pub mod vpp_aggregator;` 与 7 项重导出（`VppAggregator, VppResource, VppProfile, AggregatedDispatch, Allocation, VppError, ResourceType`）。**既有 pub 项与 4 个既有模块零改动**（domain_optimizer.rs 仅 4 个 sanitize 函数 `fn` → `pub(crate) fn` 可见性放宽，零逻辑改动）。coordinator `Cargo.toml` description 追加 v0.94.0（无新依赖）。根 `Cargo.toml` version = "0.94.0"，`Makefile` / `ci.yml` / `gate.rs` 注释同步。

## REMOVED Requirements

无。
