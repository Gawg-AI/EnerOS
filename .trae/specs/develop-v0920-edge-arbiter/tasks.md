# Tasks

- [x] Task 1: crate 骨架 `crates/agents/coordinator/`
  - [x] SubTask 1.1: `Cargo.toml` — 包名 `eneros-coordinator`，`version.workspace = true` / `edition.workspace = true`（仿 twin-agent），dependencies 仅 `eneros-agent = { path = "../agent" }`
  - [x] SubTask 1.2: 根 `Cargo.toml` workspace members 追加 `"crates/agents/coordinator"`（既有 member 不变，保持列表顺序追加在 twin-agent 之后）
  - [x] SubTask 1.3: `src/lib.rs` — `#![cfg_attr(not(test), no_std)]` + `extern crate alloc;` + `pub mod arbiter;` + `pub mod bid;` + `pub mod conflict;`（字母序）
  - [x] SubTask 1.4: lib.rs 重导出：bid 3 项（cmp_bid / Claim / Priority）+ arbiter 5 项（ArbitrationReason / ArbitrationRequest / ArbitrationResult / ArbiterPolicy / DomainArbiter）+ conflict 2 项（detect_deadlock / has_safety_conflict），共 10 项
  - [x] SubTask 1.5: lib.rs 中文 crate 文档：v0.92.0 版本目标（域内仲裁，竞价为主+安全底线，P2-D 起点）+ 核心类型清单 + D1~D12 偏差简表（从 spec.md 复制）
  - [x] SubTask 1.6: `cargo metadata --format-version 1` 成功（新 member 解析）

- [x] Task 2: 实现 `src/bid.rs` — Priority + Claim + cmp_bid + 测试 T1~T8
  - [x] SubTask 2.1: `Priority { Low, Normal, High, Critical, Safety }`（D7：升序声明使 Safety 最大；derive Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default，`#[default] Normal`；中文变体 doc）
  - [x] SubTask 2.2: `Claim { agent_id: AgentId, priority: Priority, bid: f32, safety_critical: bool, deadline: u64 }`（derive Debug, Clone, Copy, PartialEq；D3 deadline 字段；中文字段 doc）
  - [x] SubTask 2.3: `cmp_bid(a: &f32, b: &f32) -> Ordering`（D11 全序）：双 NaN → Equal；a NaN → Less；b NaN → Greater；否则 `a.partial_cmp(b).unwrap_or(Ordering::Equal)`（±Inf 保留偏序）
  - [x] SubTask 2.4: `Claim::is_urgent(&self, now_ms: u64, window_ms: u64) -> bool`：`self.deadline < now_ms.saturating_add(window_ms)`（D12；过去 deadline 必 urgent）
  - [x] SubTask 2.5: 中文模块文档（D3/D7/D11/D12 引用）；use 仅 core::cmp::Ordering + eneros_agent::AgentId；无 std/panic!/unsafe/todo!/unimplemented!/unwrap（主代码）
  - [x] SubTask 2.6: T1 — Priority 序：Low<Normal<High<Critical<Safety + Default==Normal + Copy
  - [x] SubTask 2.7: T2 — Claim 构造 + Copy + 5 字段回显（AgentId(42) / Critical / 9.5 / true / 5000）
  - [x] SubTask 2.8: T3 — cmp_bid 正常：1.0<2.0 Less / 2.0>1.0 Greater / 相等 Equal
  - [x] SubTask 2.9: T4 — cmp_bid NaN 最低：NaN vs 1.0 → Less；1.0 vs NaN → Greater
  - [x] SubTask 2.10: T5 — cmp_bid 双 NaN → Equal；+Inf vs 1.0 → Greater；-Inf vs 1.0 → Less
  - [x] SubTask 2.11: T6 — is_urgent：deadline=500（过去）→ true；deadline=1500（now+window 内）→ true
  - [x] SubTask 2.12: T7 — 非 urgent：deadline=5000、now=1000、window=1000 → false；边界 deadline==now+window → false（严格 <）
  - [x] SubTask 2.13: T8 — saturating 防溢出：now=u64::MAX → is_urgent 不 panic 且任意 deadline → true

- [x] Task 3: 实现 `src/arbiter.rs` — 三级仲裁 + 测试 T9~T30
  - [x] SubTask 3.1: `ArbitrationRequest { resource_id: &'static str, claimants: Vec<Claim>, deadline: u64 }`（derive Debug, Clone；D2/D3）
  - [x] SubTask 3.2: `ArbitrationReason { SafetyFirst, HighestBid, Deadline, Default }`（derive Debug, Clone, Copy, PartialEq, Eq）
  - [x] SubTask 3.3: `ArbitrationResult { winner: Option<AgentId>, reason: ArbitrationReason, timestamp: u64, conflict: bool }`（derive Debug, Clone, Copy, PartialEq；D8/D10）
  - [x] SubTask 3.4: `ArbiterPolicy { urgent_window_ms: u64 }`（derive Debug, Clone, Copy, PartialEq + Default=1000，D12）
  - [x] SubTask 3.5: `DomainArbiter` 7 字段全 pub：policy / total_count / safety_count / deadline_count / bid_count / empty_count / conflict_count（u64；D9 可观测）；`new(policy) -> Self` 计数器全零
  - [x] SubTask 3.6: `arbitrate(&mut self, req: &ArbitrationRequest, now_ms: u64) -> ArbitrationResult`：total_count+=1 → 空 claimants（empty_count+=1，None+Default）→ safety 分支（filter safety_critical；max priority 手写循环保首个最大；≥2 个 safety → conflict=true + conflict_count+=1；safety_count+=1）→ urgent 分支（is_urgent(policy.urgent_window_ms)；min deadline 手写循环保首个；deadline_count+=1）→ bid 分支（cmp_bid max 手写循环保首个；bid_count+=1）；timestamp=now_ms
  - [x] SubTask 3.7: 中文模块文档（D2/D4/D8/D9/D10/D11/D12 引用）；use 仅 alloc::vec::Vec + core + eneros_agent::AgentId + crate::bid；无 std/panic!/unsafe/unwrap（主代码）
  - [x] SubTask 3.8: 测试辅助 — `fn claim(id: u128, priority: Priority, bid: f32, safety: bool, deadline: u64) -> Claim` + `fn req(claimants: Vec<Claim>) -> ArbitrationRequest`（resource_id="pcc"）
  - [x] SubTask 3.9: T9 — ArbiterPolicy::default().urgent_window_ms == 1000
  - [x] SubTask 3.10: T10 — new 计数器全零 + policy 回显
  - [x] SubTask 3.11: T11 — 空 claimants → winner None + reason Default + conflict false + empty_count==1 + total_count==1
  - [x] SubTask 3.12: T12 — 单 safety_critical（B，priority High）vs 高 bid 999 vs 过期 deadline → B 胜出 + SafetyFirst（蓝图 §7.3 安全压制）
  - [x] SubTask 3.13: T13 — 多 safety 不同优先级：High vs Safety → Safety 级胜出（D7 序）
  - [x] SubTask 3.14: T14 — 多 safety 同优先级 → 输入序首个胜出 + conflict==true + conflict_count==1（D10）
  - [x] SubTask 3.15: T15 — safety 路径计数器：total==1 / safety_count==1 / 其余 0
  - [x] SubTask 3.16: T16 — deadline 紧急（过去 deadline）→ Deadline reason + winner 正确
  - [x] SubTask 3.17: T17 — 多 urgent → 最早 deadline 胜出
  - [x] SubTask 3.18: T18 — urgent + safety 同时存在 → safety 优先（三级顺序不可逾越）
  - [x] SubTask 3.19: T19 — deadline 远超窗口（now=1000, window=1000, deadline=5000）→ 落 HighestBid 而非 Deadline
  - [x] SubTask 3.20: T20 — 最高 bid 胜出（3 个非紧急非安全 claimant，bid 5/9/7 → bid=9 者）
  - [x] SubTask 3.21: T21 — 等 bid（5/5）→ 确定性取首个
  - [x] SubTask 3.22: T22 — NaN bid 不胜出：claimants [NaN, 1.0] → 1.0 者胜出（D11）
  - [x] SubTask 3.23: T23 — 全 NaN bid → 确定性首个胜出 + 不 panic
  - [x] SubTask 3.24: T24 — timestamp == 传入 now_ms 回显（三条路径各验一次合并断言）
  - [x] SubTask 3.25: T25 — 计数器组合：连续 3 次仲裁（safety/deadline/bid 各一）→ total==3 且 safety+deadline+bid==3
  - [x] SubTask 3.26: T26 — 单 claimant（非安全非紧急）→ HighestBid + winner 正确
  - [x] SubTask 3.27: T27 — 自定义 window=0：deadline<now 才 urgent（deadline==now 不 urgent → 落 bid）
  - [x] SubTask 3.28: T28 — 确定性：同输入两次 arbitrate（不同 arbiter 实例）→ winner/reason/timestamp/conflict 全等
  - [x] SubTask 3.29: T29 — ArbitrationResult 构造 + Clone + PartialEq + Debug 含 "SafetyFirst"
  - [x] SubTask 3.30: T30 — 100 个 claimants 大输入：1 safety 在末位仍胜出（线性扫描正确）+ 无 panic

- [x] Task 4: 实现 `src/conflict.rs` — 冲突/死锁检测 + 测试 T31~T40
  - [x] SubTask 4.1: `has_safety_conflict(claims: &[Claim]) -> bool`（D10：safety_critical 计数 ≥ 2）
  - [x] SubTask 4.2: `detect_deadlock(waits: &[(AgentId, AgentId)]) -> bool`：wait-for 图（边 (a,b)=a 等待 b）；BTreeMap<AgentId, Vec<AgentId>> 邻接 + BTreeMap<AgentId, u8> 三色标记（0 白/1 灰/2 黑），DFS 遇灰 → true；全节点扫描覆盖不连通分量（蓝图 §5.4/§6.5）
  - [x] SubTask 4.3: 中文模块文档（D10 + 蓝图 §5.4 死锁检测引用）；use 仅 alloc + eneros_agent::AgentId + crate::bid::Claim；无 std/panic!/unsafe/unwrap（主代码）
  - [x] SubTask 4.4: T31 — has_safety_conflict：0 个 safety → false
  - [x] SubTask 4.5: T32 — 1 个 safety → false（独占不冲突）
  - [x] SubTask 4.6: T33 — 2 个 safety（夹杂非 safety）→ true
  - [x] SubTask 4.7: T34 — detect_deadlock 空边表 → false
  - [x] SubTask 4.8: T35 — 单边链 a→b→c 无环 → false
  - [x] SubTask 4.9: T36 — 自环 (a,a) → true
  - [x] SubTask 4.10: T37 — 2-cycle (a,b),(b,a) → true
  - [x] SubTask 4.11: T38 — 3-cycle (a,b),(b,c),(c,a) → true
  - [x] SubTask 4.12: T39 — 菱形 (a→b, a→c, b→d, c→d) 无环 → false（重复访问黑节点不误报）
  - [x] SubTask 4.13: T40 — 不连通双分量：分量 1 无环（x→y）+ 分量 2 有环（p→q→p）→ true（全节点扫描覆盖）

- [x] Task 5: 创建 `configs/edge_arbiter.toml`
  - [x] SubTask 5.1: `[arbiter]` 段：`urgent_window_ms = 1000`（D12，与 ArbiterPolicy::default 一致）
  - [x] SubTask 5.2: 中文注释：三级仲裁顺序（安全 > deadline > 竞价）/ safety_critical 永远胜出（蓝图 §7.3）/ 多安全冲突 conflict 告警（D10）/ 仲裁 <10ms（蓝图 §7.2，集成阶段验收）/ NaN 报价防御（D11）/ 结果需广播所有 claimants（§8.5 坑点，下游总线集成）/ 仲裁饥饿老化机制（§8.1，后续版本）

- [x] Task 6: 创建 `docs/agents/edge-arbiter-design.md`
  - [x] SubTask 6.1: 12 章节（版本目标 / 前置依赖 / 交付物清单 / 数据结构 / 接口设计 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险与坑点 / 偏差声明）
  - [x] SubTask 6.2: Mermaid 图 1：三级仲裁决策流（蓝图 §4.3 扩展：请求 → 空? → safety? → urgent? → 最高 bid → 结果 + 计数器）
  - [x] SubTask 6.3: Mermaid 图 2：死锁检测流程（wait-for 边表 → 建邻接表 → 三色 DFS → 遇灰成环 → true/false）
  - [x] SubTask 6.4: D1~D12 偏差声明表（从 spec.md 复制）
  - [x] SubTask 6.5: 前置依赖引用 v0.88.0 多目标优化（上游）+ v0.77.0 路由器 + v0.33.0 AgentId（复用）；下游 v0.93.0 域级优化 / v0.94.0 VPP 聚合
  - [x] SubTask 6.6: 选型对比表（安全优先+竞价 ⭐ / 纯竞价 安全风险 / 轮询 不适用，蓝图 §5.1）
  - [x] SubTask 6.7: 性能目标（仲裁 <10ms，标注"集成阶段验收，本版本交付算法骨架+单元验证"）+ GPU 规则（蓝图 §6.6：不涉及 GPU，纯标量 CPU）
  - [x] SubTask 6.8: 安全语义（§7.3 safety_critical 永远胜出 + §7.5 出口：多 Agent 争抢仲裁可用）+ 可观测（D9 六计数器，§9）
  - [x] SubTask 6.9: 风险：仲裁饥饿→老化机制后续版本（§8.1）/ 结果需广播（§8.5，下游集成）/ 死锁预防（§5.4：本版本仅检测不预防）

- [x] Task 7: 版本同步根目录文件
  - [x] SubTask 7.1: 根 `Cargo.toml` `[workspace.package] version = "0.91.0"` → `"0.92.0"`（members 已在 Task 1 追加，此处仅版本号）
  - [x] SubTask 7.2: `Makefile` `# Version: v0.92.0` + `VERSION := 0.92.0`
  - [x] SubTask 7.3: `.github/workflows/ci.yml` `# Version: v0.92.0`
  - [x] SubTask 7.4: `ci/src/gate.rs` clippy 段 + test 段注释追加 `+ v0.92.0 域内仲裁：DomainArbiter / ArbiterPolicy / ArbitrationRequest / ArbitrationResult / ArbitrationReason / Claim / Priority / detect_deadlock`

- [x] Task 8: 构建校验（§2.4.2 C6~C11）
  - [x] SubTask 8.1: `cargo metadata --format-version 1` 成功
  - [x] SubTask 8.2: `cargo test -p eneros-coordinator` 40 tests 全过（T1~T40，0 failures）
  - [x] SubTask 8.3: `cargo build -p eneros-coordinator --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] SubTask 8.4: `cargo fmt -p eneros-coordinator -- --check` 通过
  - [x] SubTask 8.5: `cargo clippy -p eneros-coordinator --all-targets -- -D warnings` 无 warning
  - [x] SubTask 8.6: `cargo deny check licenses bans sources` 通过（无新第三方依赖）
  - [x] SubTask 8.7: 回归 — `cargo test -p eneros-agent` / `cargo test -p eneros-twin-agent` / `cargo test -p eneros-energy-market-agent` / `cargo test -p eneros-agent-bus-dds` 全过

# Task Dependencies

- [Task 2] depends on [Task 1]
- [Task 3] depends on [Task 2]（arbiter 引用 bid::Claim/cmp_bid/is_urgent）
- [Task 4] depends on [Task 2]（conflict 引用 bid::Claim）
- [Task 5, Task 6] 独立（可与 1~4 并行）
- [Task 7] depends on [Task 1]（members 追加后版本号同步不冲突，仅根目录 4 文件）
- [Task 8] depends on [Task 3, Task 4, Task 5, Task 6, Task 7]

# 并行执行计划

- **Sub-Agent A**：Task 1 + Task 2 + Task 3 + Task 4（同 crate 源文件，串行单 agent 保证一致性）
- **Sub-Agent B**：Task 5 + Task 6（configs + docs，与 A 并行）
- **Sub-Agent C**：Task 7（版本同步，与 A/B 并行；仅根目录 3 文件 + gate.rs，不碰根 Cargo.toml members）
- **主 agent**：Task 8（全部完成后统一构建校验 + 回归）
