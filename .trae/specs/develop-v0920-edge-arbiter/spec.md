# v0.92.0 Edge Coordinator — 域内仲裁 Spec

## Why

v0.87.0 多设备调度与 v0.88.0 多目标优化解决了"单 Agent 怎么算"，但多 Agent 争抢同一共享资源（PCC 并网容量、变压器容量等）时需要域内仲裁者。蓝图 phase2 v0.92.0（P2-D 起点，联邦治理域内层）要求实现 Edge Coordinator 域内仲裁，按"竞价为主 + 安全底线"原则三级仲裁（安全 > deadline > 竞价），避免资源争抢冲突、保障安全优先，为 v0.93.0 域级优化与 VPP 域内协同打基础。

## What Changes

- **新建 crate `eneros-coordinator`**（`crates/agents/coordinator/`，P2-D 仲裁/优化/聚合共用，v0.93.0 domain_optimizer、v0.94.0 vpp_aggregator 后续同 crate 追加）：
  - `src/bid.rs` — `Priority` 5 级 / `Claim`（含 deadline，D3）/ `cmp_bid` 全序比较（NaN 防御 D11）/ `Claim::is_urgent`
  - `src/arbiter.rs` — `ArbitrationRequest` / `ArbitrationResult`（`Option<AgentId>`，D8）/ `ArbitrationReason` / `ArbiterPolicy`（D12）/ `DomainArbiter`（三级仲裁 + 6 个可观测计数器，D9）
  - `src/conflict.rs` — `has_safety_conflict`（D10）/ `detect_deadlock`（wait-for 图三色 DFS 环检测，蓝图 §5.4/§6.5）
- `src/lib.rs` — crate 文档（v0.92.0 + D1~D12 偏差简表）+ 3 模块 + 全部重导出
- 新增 `configs/edge_arbiter.toml`（`[arbiter]` urgent_window_ms + 三级仲裁原则注释）
- 新增 `docs/agents/edge-arbiter-design.md`（12 章节 + 2 Mermaid + D1~D12 偏差表）
- 根目录 4 文件版本同步 0.91.0 → 0.92.0（Cargo.toml 含 members 追加 / Makefile / ci.yml / gate.rs 注释）
- 内嵌单元测试 40 个（bid T1~T8 / arbiter T9~T30 / conflict T31~T40），含 NaN 报价与死锁环故障注入
- **无 BREAKING**：全新 crate，既有全部 crate 零改动

## Impact

- Affected specs：无既有 spec 修改（全新能力）；关联 develop-v0880-multi-objective（上游多目标）
- Affected code：新增 `crates/agents/coordinator/`、`configs/`、`docs/agents/`、根 4 文件
- 依赖：`eneros-agent`（复用 `AgentId(u128)`，单一事实源，同 energy-market-agent v0.72.0 D3 惯例）
- 下游解锁：v0.93.0 域级优化、v0.94.0 VPP 聚合、v0.96.0 Cloud Coordinator

## 偏差声明（D1~D12，Karpathy Think Before Coding：显式取舍）

| 偏差 | 蓝图原文 | 本版本处理 |
|------|---------|-----------|
| **D1** | crate 路径 `crates/coordinator/src/` | `crates/agents/coordinator/`（项目 §2.3.1 硬规则优先：crate 必须归入既有 7 子系统；coordinator 属 Phase 2+ 治理 Agent 归 agents/；v0.93/v0.94 模块后续同 crate 追加） |
| **D2** | `resource_id: String` | `resource_id: &'static str`（无堆分配，同 v0.90.0/v0.91.0 D2 惯例） |
| **D3** | §4.1 `Claim` 无 deadline 字段，§4.5 代码却用 `c.deadline`（蓝图自相矛盾） | `Claim` 增加 `deadline: u64`（每 claimant 独立紧急度，三级仲裁必需）；`ArbitrationRequest.deadline` 保留为请求级字段（预留仲裁超时逻辑，本版本三级仲裁不使用） |
| **D4** | `now_ms()` 内部时间源 | `arbitrate(&mut self, req, now_ms: u64)` 外部时间注入（no_std 无 Instant，全项目统一惯例）；`result.timestamp = now_ms` |
| **D5** | `docs/phase2/edge_arbiter.md` | `docs/agents/edge-arbiter-design.md`（记忆 §2.3.3 文档分类强制） |
| **D6** | `tests/arbiter.rs` 独立集成测试 | src 内嵌单元测试 T1~T40（项目惯例）；仲裁 <10ms 标注**集成阶段验收** |
| **D7** | `Priority::value()` 方法 + 声明序 Safety 在前 | derive `PartialOrd/Ord`，声明序 `Low < Normal < High < Critical < Safety`（Safety 最大，`max_by_key` 直接可用）；Default=Normal |
| **D8** | §4.4"无 claimants → 返回空结果"（`winner: AgentId` 非空类型无法表达） | `winner: Option<AgentId>`（空 → `None` + `reason = ArbitrationReason::Default`） |
| **D9** | `arbitrate(&self, req)` | `arbitrate(&mut self, ...)`（蓝图 §9 可观测要求：6 个 pub 计数器 total/safety/deadline/bid/empty/conflict，仲裁记录 metric 本地可查） |
| **D10** | §4.4"多个 safety_critical → 选优先级最高，并告警冲突" | `ArbitrationResult.conflict: bool`（no_std 无 log crate，告警可观测化为结果字段）+ `conflict_count` 计数器；同优先级时确定性取输入序首个 |
| **D11** | §4.5 `partial_cmp(b.bid).unwrap()`（NaN panic 风险） | `cmp_bid` 全序：NaN 视为最低（v0.88.0 C140 教训），±Inf 保留偏序；全 NaN 确定性取首个，不 panic |
| **D12** | §4.5 硬编码 `now + 1000` 紧急窗口 | `ArbiterPolicy { urgent_window_ms: u64 }`（Default=1000，蓝图 §9 策略配置化）；`now_ms.saturating_add(window)` 防 u64 溢出 |

## ADDED Requirements

### Requirement: 竞价数据模型（bid.rs）

系统 SHALL 提供：`Priority { Low, Normal, High, Critical, Safety }`（derive Ord 升序声明，Safety 最大，Default=Normal，D7）、`Claim { agent_id: AgentId, priority: Priority, bid: f32, safety_critical: bool, deadline: u64 }`（Debug/Clone/Copy/PartialEq，D3）、`cmp_bid(a: &f32, b: &f32) -> Ordering`（NaN 最低全序，D11）、`Claim::is_urgent(&self, now_ms: u64, window_ms: u64) -> bool`（`deadline < now_ms.saturating_add(window_ms)`，D12）。

#### Scenario: 报价全序比较

- **WHEN** 比较 `cmp_bid(NaN, 1.0)` / `cmp_bid(NaN, NaN)` / `cmp_bid(f32::INFINITY, 1.0)`
- **THEN** Less / Equal / Greater（NaN 恒最低，不 panic）

#### Scenario: 紧急判定

- **WHEN** claim.deadline=500、now_ms=1000、window=1000 → **THEN** urgent（过去 deadline 必紧急）
- **WHEN** deadline=5000、now_ms=1000、window=1000 → **THEN** 非紧急
- **WHEN** now_ms=u64::MAX → **THEN** saturating 不 panic

### Requirement: 三级仲裁器（arbiter.rs）

系统 SHALL 提供：`ArbitrationRequest { resource_id: &'static str, claimants: Vec<Claim>, deadline: u64 }`、`ArbitrationReason { SafetyFirst, HighestBid, Deadline, Default }`、`ArbitrationResult { winner: Option<AgentId>, reason: ArbitrationReason, timestamp: u64, conflict: bool }`、`ArbiterPolicy { urgent_window_ms: u64 }`（Default=1000）、`DomainArbiter { policy, total_count, safety_count, deadline_count, bid_count, empty_count, conflict_count }`（字段全 pub）与 `DomainArbiter::new(policy)`、`arbitrate(&mut self, req: &ArbitrationRequest, now_ms: u64) -> ArbitrationResult`：① 有 safety_critical → 最高 priority 胜出（SafetyFirst，≥2 个 safety 置 conflict=true）；② 否则有 urgent（D12）→ 最早 deadline 胜出（Deadline）；③ 否则最高 bid 胜出（HighestBid，cmp_bid 全序）；空 claimants → None + Default。每路径更新对应计数器，timestamp = now_ms。

#### Scenario: 安全永远胜出（蓝图 §7.3）

- **WHEN** 3 个 claimants：A（bid=999.0）、B（safety_critical, priority=High）、C（deadline 已过）
- **THEN** winner == B.agent_id，reason == SafetyFirst（高报价与紧急 deadline 均被安全压制）

#### Scenario: 多安全冲突告警

- **WHEN** 2 个 safety_critical（priority 同为 Safety）
- **THEN** 确定性取输入序首个，conflict == true，conflict_count += 1（蓝图 §4.4）

#### Scenario: deadline 优先于竞价

- **WHEN** 无 safety，X（deadline=1500 < now+1000）、Y（bid=100.0 非紧急）
- **THEN** winner == X，reason == Deadline

#### Scenario: 空请求

- **WHEN** claimants 为空
- **THEN** winner == None，reason == Default，empty_count += 1，不 panic

### Requirement: 冲突与死锁检测（conflict.rs）

系统 SHALL 提供 `has_safety_conflict(claims: &[Claim]) -> bool`（safety_critical ≥ 2，D10）与 `detect_deadlock(waits: &[(AgentId, AgentId)]) -> bool`（wait-for 图：边 (a,b) 表示 a 等待 b 持有资源；BTreeMap 邻接 + 三色 DFS 环检测，蓝图 §5.4/§6.5）。

#### Scenario: 死锁环检测

- **WHEN** waits = [(a,b), (b,a)]（互相等待）→ **THEN** true
- **WHEN** waits = [(a,b), (a,c), (b,d), (c,d)]（菱形无环）→ **THEN** false
- **WHEN** waits = [(a,a)]（自环）→ **THEN** true

### Requirement: 仲裁策略配置

系统 SHALL 提供 `configs/edge_arbiter.toml`：`[arbiter] urgent_window_ms = 1000`（D12），中文注释含三级仲裁顺序 / safety_critical 永远胜出（§7.3）/ 仲裁 <10ms（集成阶段验收）/ 结果需广播所有 claimants（§8.5 坑点，下游集成）/ 仲裁饥饿老化机制（§8.1，后续版本）。

## MODIFIED Requirements

### Requirement: workspace 版本与成员

根 `Cargo.toml`：`version = "0.92.0"`，members 追加 `"crates/agents/coordinator"`（既有 member 不变）；`Makefile` / `ci.yml` / `gate.rs` 注释同步 v0.92.0。

## REMOVED Requirements

无。
