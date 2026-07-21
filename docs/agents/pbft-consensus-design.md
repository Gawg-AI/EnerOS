# EnerOS v0.99.0 联邦共识协议（PBFT 变体）设计文档

> **版本**：v0.99.0
> **蓝图**：phase2.md §v0.99.0（P2-E 第 3 版）
> **Crate**：`eneros-federation`（`crates/agents/federation/src/{consensus.rs,pbft.rs,view_change.rs}`，既有 crate 追加 3 模块）

---

## 1. 版本目标

实现 **Edge Coordinator 间联邦共识协议（PBFT 变体）**（**Phase 2 P2-E 第 3 版，多机联邦协议一致性层**），交付三大能力：

- **跨域决策一致性**：`submit(request)` → 三阶段（PrePrepare/Prepare/Commit）→ 各诚实节点对同一 digest/sequence 达成 committed，确保跨信任域策略协同（如馈线容量分配、竞价优先级）的全局一致视图；
- **拜占庭容错**：n ≥ 3f+1 节点假设下容忍 ≤ f 个拜占庭节点（伪造消息/静默/重复投票/双重提案），SM2 全签名验证 + BTreeSet 票集去重保证安全性；
- **主节点故障恢复**：`check_timeout` 超时检测 → ViewChange 广播 → 2f+1 法定人数达新视图 → 新主节点重发未提交日志 → 共识恢复（蓝图 §4.4/§8.5）。

辅助能力：

- **同步化接口**：`submit` / `poll` / `handle_message` 全同步（D3：no_std 硬规则禁 async），`poll(bus, now_ms)` 排空邮箱驱动状态机推进，返回新提交结果集，由上层定时轮询；
- **拜占庭 5 类攻击防护**：伪签（InvalidSignature 丢弃）、假 digest（StaleMessage 丢弃）、重复票（BTreeSet 去重）、错 view（StaleMessage 丢弃）、equivocation（同 seq 异 digest 无法收齐 quorum，安全不提交）；
- **可观测计数器**：4 个 pub 计数器 `submit_count` / `committed_count` / `rejected_count` / `view_change_count` + `last_latency_ms`（注入时钟，D12）。

**业务价值**：v0.98.0 完成跨域加密通信通道（安全传输基础），但通道仅保证报文机密性与完整性——恶意节点仍可伪造合法签名消息扰乱决策。本版本建立**联邦共识层**（PBFT 三阶段 + ViewChange + 国密签名验证），为跨域决策提供"一致性 + 容错 + 可观测"三重保障。

**Phase 定位**：P2-E 第 3 版；**下游解锁 v0.100.0 资源争抢竞价机制（竞价结果须经联邦共识确认后方可执行）与 Phase 2 联邦协议一致性出口**。

**性能目标**（蓝图 §7.2）：共识达成 < 1s（4 节点）—— **集成阶段验收**，本版本交付算法骨架 + Mock 单元验证（真实 SecureTransport 适配器注入后实测验收）。

---

## 2. 前置依赖

- **v0.98.0 跨域加密通信通道**（前序版本，P2-E 第 2 版）：联邦成员间安全传输底座（mTLS + SM4-GCM），本版本共识消息经该通道加密传输（蓝图 §5.5 上游交互）；`ConsensusBus` trait 为抽象 seam，生产环境由 channel/tunnel 适配器注入真实网络传输；
- **n ≥ 3f+1 假设**（蓝图 §2）：本版本 4 节点示例容忍 f=1 拜占庭节点；节点表 + 全网点公钥表（peers）为 `ConsensusEngine::new` 注入前置条件；
- **eneros-crypto**（workspace 既有 crate，v0.33.0 国密 SM2/SM3/SM4 + CSRNG）：SM3 摘要（请求 digest 与签名域分离拼接中的 digest 分量）、SM2 签名/验签（`sm2_sign`/`sm2_verify` 复用，§5.5 防重复造轮子）；`Cargo.toml` 中 path 依赖已在 v0.98.0 引入（D9，零新增第三方依赖，SBOM 不变）；
- 蓝图 `phase2.md` v0.99.0 章节（9 节版本模板，§4.3 三阶段时序图 / §4.4 ViewChange / §7.2 <1s / §7.3 拜占庭容错 / §8.5 ViewChange 风暴为落地依据）；
- **no_std + alloc**：`core` / `alloc` only——`alloc::vec::Vec` / `alloc::collections::BTreeMap` / `alloc::collections::BTreeSet`；禁止 `std::*`（蓝图 §43.1 硬性要求）；
- **后续注入**：真实网络传输栈（集成阶段 Agent Runtime 适配层实现 `ConsensusBus` trait）后续以 `Box<dyn ConsensusBus>` 注入，consensus 层零改动对接。

**下游解锁**：v0.100.0 资源争抢竞价机制（竞价结果须经联邦共识确认）/ Phase 2 联邦协议一致性出口。

---

## 3. 交付物清单

- `crates/agents/federation/src/consensus.rs` — **新增**：`NodeId` / `ConsensusState` / `MsgType` / `PbftMessage` / `LogEntry` / `ConsensusResult` / `ConsensusError` / `ConsensusBus` trait / `MockConsensusBus` / `ConsensusEngine`（核心字段 + new/submit/poll/handle_message/is_committed + 4 计数器 + 延迟观测）
- `crates/agents/federation/src/pbft.rs` — **新增**：三阶段逻辑（`impl ConsensusEngine` 扩展：on_pre_prepare/on_prepare/on_commit）+ 自由函数 `f()` / `quorum()` / `primary_of()` / `sign_message()` / `verify_message()`
- `crates/agents/federation/src/view_change.rs` — **新增**：ViewChange 逻辑（`impl ConsensusEngine` 扩展：check_timeout/on_view_change/enter_view）+ 指数退避
- `crates/agents/federation/Cargo.toml` — **修改**：description 升级为三版本说明（v0.97.0 联邦发现 + v0.98.0/v0.98.1 通道与纵向加密 + v0.99.0 联邦共识）；依赖不变（eneros-crypto path 引用已在 v0.98.0 引入）
- `crates/agents/federation/src/lib.rs` — **修改**：`pub mod consensus; pub mod pbft; pub mod view_change;` + 新增类型全量重导出 + crate 文档追加 v0.99.0 说明与 D1~D12 偏差表（既有 membership/discovery/channel/tunnel 零改动）
- `configs/federation-consensus.toml` — **新增**：`[consensus]` 段（mode / timeout_ms / max_view_change_backoff / nodes + 中文注释 ≥6 点）
- `docs/agents/pbft-consensus-design.md` — 本设计文档
- **40 个单元测试** TC1~TC12（consensus.rs）/ TP13~TP28（pbft.rs）/ TV29~TV40（view_change.rs）（src 内嵌 `#[cfg(test)]`，v0.87.0~v0.98.1 项目惯例，不新增 tests/ 文件，D11）
- 根目录 4 文件版本同步 0.98.0 → 0.99.0（`Cargo.toml` / `Makefile` / `ci.yml` / `gate.rs` 注释）
- **无 BREAKING**：既有全部 crate 公共 API 零改动

---

## 4. 详细设计

### 4.0 PBFT 三阶段时序

```mermaid
sequenceDiagram
    participant C as 客户端 / 上层 Agent
    participant P as 主节点 (Primary)
    participant R1 as 备份节点 1
    participant R2 as 备份节点 2
    participant R3 as 备份节点 3

    Note over P: is_primary() == true<br/>submit(b"馈线容量分配")
    C->>P: request
    P->>P: seq=sequence+1<br/>digest=SM3(request)<br/>签名 PrePrepare
    P->>R1: PrePrepare<br/>(view, seq, digest, payload=request, 签名)
    P->>R2: PrePrepare
    P->>R3: PrePrepare

    R1->>R1: 验签 + digest==SM3(payload)<br/>建 LogEntry(prepare_voters={primary})<br/>state=Prepare
    R2->>R2: 同上
    R3->>R3: 同上

    R1->>P: Prepare (同 digest, 签名)
    R1->>R2: Prepare
    R1->>R3: Prepare
    R2->>P: Prepare
    R2->>R1: Prepare
    R2->>R3: Prepare
    R3->>P: Prepare
    R3->>R1: Prepare
    R3->>R2: Prepare
    P->>P: 收集 Prepare 票<br/>prepare_voters.len() >= quorum(4)=3<br/>prepared=true → 广播 Commit

    Note over P,R3: 【D7 主票计入】<br/>主节点 PrePrepare 已计入自身 prepare 票<br/>n=4 时主票 + 2 备份 = 3 = quorum<br/>容忍 1 备份静默/作恶仍达成 prepared

    P->>R1: Commit (签名)
    P->>R2: Commit
    P->>R3: Commit
    R1->>P: Commit
    R1->>R2: Commit
    R1->>R3: Commit
    R2->>P: Commit
    R2->>R1: Commit
    R2->>R3: Commit
    R3->>P: Commit
    R3->>R1: Commit
    R3->>R2: Commit

    R1->>R1: commit_voters.len() >= 3<br/>committed/executed=true<br/>state=Done → committed_count+=1<br/>last_latency_ms = now − submit_ms
    R2->>R2: 同上
    R3->>R3: 同上
    P->>P: 同上

    Note over C,P: poll() 返回 Vec&lt;ConsensusResult&gt;<br/>sequence=1, digest 全节点一致
```

### 4.1 数据结构（7 类型）

| 类型 | 说明 | 派生 |
|------|------|------|
| `pub type NodeId = u64` | 节点标识（D2：无堆字符串，v0.97.0 D1 惯例） | — |
| `ConsensusState { Idle, PrePrepare, Prepare, Commit, Done }` | 共识状态机（D6 增 Idle 初始/视图切换后静止态） | Debug/Clone/Copy/PartialEq/Eq |
| `MsgType { PrePrepare, Prepare, Commit, Reply, ViewChange }` | 消息类型（D4 增 ViewChange 复用消息帧） | Debug/Clone/Copy/PartialEq/Eq + `to_u8()/from_u8()` |
| `PbftMessage { msg_type, view, sequence, digest, payload, sender, signature }` | 共识消息帧（D4 增 sender 投票去重 + payload 请求本体） | Debug/Clone/PartialEq |
| `LogEntry { sequence, request, digest, prepare_voters, commit_voters, prepared, committed, executed }` | 日志条目（D5 BTreeSet 票集去重） | Clone |
| `ConsensusResult { sequence, digest, view }` | 提交结果 | Debug/Clone/PartialEq |
| `ConsensusError { NotPrimary, UnknownNode, InvalidSignature, ViewMismatch, StaleMessage, NotEnoughNodes, BusError }` | 错误枚举（D10 7 变体最小完备） | Debug/Clone/Copy/PartialEq/Eq |

### 4.2 ConsensusEngine 字段表

| 字段 | 类型 | 说明 |
|------|------|------|
| `nodes` | `Vec<NodeId>` | 共识节点表（按视图轮换主节点顺序） |
| `local_id` | `NodeId` | 本节点标识 |
| `view` | `u64` | 当前视图号（enter_view 后更新） |
| `sequence` | `u64` | 最新已提交 sequence（0 起始） |
| `state` | `ConsensusState` | 当前状态（Idle/PrePrepare/Prepare/Commit/Done） |
| `log` | `Vec<LogEntry>` | 请求日志（sequence 索引） |
| `kp` | `Sm2KeyPair` | 本节点 SM2 密钥对（签名用） |
| `peers` | `BTreeMap<NodeId, Sm2PublicKey>` | 全网点公钥表（验签用） |
| `timeout_ms` | `u64` | 配置超时（ms） |
| `last_progress_ms` | `u64` | 上次有效消息时刻（check_timeout 参照） |
| `consecutive_vc` | `u32` | 连续 ViewChange 次数（退避指数基数） |
| `submit_count` | `u64` | 提交请求计数（pub 可观测，D12） |
| `committed_count` | `u64` | 成功提交计数（pub 可观测，D12） |
| `rejected_count` | `u64` | 拒绝消息计数（pub 可观测，D12） |
| `view_change_count` | `u64` | ViewChange 次数（pub 可观测，D12） |
| `last_latency_ms` | `u64` | 最近提交延迟 ms（注入时钟，D12） |
| `vc_votes` | `BTreeMap<u64, BTreeSet<NodeId>>` | 【实现修正】各目标视图的 VC 票集（new_view → 投票节点集合） |
| `rng` | `CsRng` | 【实现修正】签名随机源（sign_message 注入） |
| `submitted_ms` | `u64` | 【实现修正】最近 submit 请求受理时刻（last_latency_ms 计算用） |

> **实现修正说明**：spec.md ADDED Requirements 中 ConsensusEngine 字段表未列 `vc_votes`、`rng`、`submitted_ms`；实现中 `vc_votes` 为 ViewChange 票集独立存储（log 外 BTreeMap），`rng` 为 CsRng 签名随机源，`submitted_ms` 记录 submit 时刻用于 last_latency_ms 计算。文档接口契约按此为准。

### 4.3 三阶段流程

1. **submit（主节点路径）**：`is_primary()` 为 false → `Err(NotPrimary)`；seq = sequence + 1；digest = SM3(request)；构造 `PbftMessage { msg_type: PrePrepare, view, sequence: seq, digest, payload: request.clone(), sender: local_id, signature }`（签名域见 §4.5）；`bus.broadcast`；追加 `LogEntry { prepare_voters: {local_id}, ... }`（D7 主票计入）；state = PrePrepare；submit_count += 1；返回 Ok(seq)。
2. **on_pre_prepare（备份路径）**：sender ≠ `primary_of(nodes, view)` → rejected_count += 1 + `Err(ViewMismatch)`；digest ≠ SM3(payload) → rejected_count += 1 + `Err(StaleMessage)`；seq 已存在日志 → 忽略 Ok(None)；新建 LogEntry（prepare_voters = {primary}）；广播 Prepare（同 digest）；state = Prepare。
3. **on_prepare**：找到 seq 日志（无 → `Err(StaleMessage)`）；prepare_voters.insert(sender)（重复自然去重）；`prepare_voters.len() >= quorum(n) && !prepared` → prepared = true；commit_voters.insert(local_id)；广播 Commit；state = Commit。
4. **on_commit**：commit_voters.insert(sender)；`commit_voters.len() >= quorum(n) && !committed` → committed = executed = true；sequence = seq；state = Done；committed_count += 1；last_latency_ms = now_ms − submitted_ms；返回 `Ok(Some(ConsensusResult { sequence: seq, digest, view }))`。

### 4.4 ViewChange 流程

1. **check_timeout**：state ∈ {Idle, Done} → `Ok(false)`（无活跃请求不触发）；now_ms − last_progress_ms ≤ 有效超时 → `Ok(false)`；否则构造 ViewChange（msg.view = self.view + 1，sequence = 0，签名）→ bus.broadcast → view_change_count += 1 → consecutive_vc += 1 → `Ok(true)`。
2. **on_view_change**：msg.view ≤ self.view → `Ok(false)`（陈旧忽略）；msg.sender 不在 nodes → `Err(UnknownNode)`；验签失败 → rejected_count += 1 + `Err(InvalidSignature)`；vc_votes[new_view].insert(from)（BTreeSet 去重）；达 quorum → `enter_view(new_view, bus, now_ms)` → `Ok(true)`。
3. **enter_view**：view = new_view；state = Idle；consecutive_vc = 0；last_progress_ms = now_ms；若 `is_primary()` 为 true 且 log 尾部有未 committed 条目 → 以其 digest/request 重发 PrePrepare（同 sequence，D8 恢复共识）。

### 4.5 签名域分离格式（D9）

签名消息体 msg_body 按以下域顺序拼接（大端序）：

```
msg_type:u8 ‖ view:u64be ‖ sequence:u64be ‖ digest:[u8;32] ‖ sender:u64be [ ‖ payload ]
```

- `msg_type`：u8（PrePrepare=0, Prepare=1, Commit=2, Reply=3, ViewChange=4）
- `view`、`sequence`、`sender`：u64 大端序 8 字节
- `digest`：SM3 摘要 32 字节
- `payload`：PrePrepare 携带请求本体（‖payload 仅 PrePrepare 包含，其余消息类型无 payload）

SM2 签名：`sign_message(kp, msg_body, rng) -> [u8; 64]`  
SM2 验签：`verify_message(pk, msg_body, sig) -> bool`

---

## 5. 技术交底

### 5.1 选型对比表（照蓝图 §5.1）

| 共识 | 容错类型 | 吞吐 | 节点数敏感 | 结论 |
|------|---------|------|-----------|------|
| PBFT | 拜占庭（f ≤ (n-1)/3） | 中（O(n²) 消息） | 敏感（4~7 节点为宜） | ⭐ 采用（跨信任域 Byzantine 容错刚需） |
| Raft | 崩溃（f ≤ (n-1)/2） | 高 | 中 | 不防拜占庭（恶意节点可伪造日志） |
| PoW | 拜占庭 | 低 | 不敏感 | 不适用（延迟不可接受，能耗不符边缘场景） |

PBFT 三阶段消息复杂度 O(n²) 在 4~7 节点联邦规模可控；蓝图评审 P2 判定默认部署用主从仲裁（v0.92.0 DomainArbiter），PBFT 仅跨信任域启用（详见 §6 P2 降级说明）。

### 5.2 D7 主票优化说明

蓝图 §4.3 "收集 2f+1 Prepare" 落地为 `quorum(n) = 2f+1` + PrePrepare 计入主节点 prepare 票（PBFT 经典变体优化）：

- n=4, f=1, quorum=3；主节点 PrePrepare 计 1 票 + 2 备份 Prepare = 3 票即 prepared；
- 与蓝图 "2f+1" 数值一致，但主票计入后仅需 2 个备份响应即可达成 prepared（而非 3 个备份），在 1 备份静默/作恶场景下仍可提交；
- 安全性无损：主节点作恶发送错误 digest → 备份验证 SM3(payload)==digest 失败 → StaleMessage 丢弃，无法 prepared。

### 5.3 D8 无 NewView 收敛说明

蓝图 §4.4 "主节点超时 → ViewChange" 未定义 NewView 消息细节。本版本采用无独立 NewView 消息设计：

- ViewChange 消息全网广播（msg.view = 目标视图），诚实节点自然收敛同一 new_view；
- 达 quorum 后各节点自主 `enter_view(new_view)`，消除 NewView 伪造攻击面（恶意主节点无法伪造 NewView 劫持视图）；
- 新主节点对最近未 committed 日志重发 PrePrepare 恢复共识；
- 连续 VC 超时指数退避：`timeout_ms << min(consecutive_vc, max_view_change_backoff)`（蓝图 §8.5 坑点对策）。

### 5.4 交互

- **上游**：v0.98.0 跨域加密通信通道（channel.rs）—— 本版本 `ConsensusBus` 为抽象 seam，生产环境由 channel/tunnel 适配真实网络传输（`broadcast` 映射为 channel.call 多播，`receive` 映射为 channel 邮箱轮询）；共识报文经 SM4-GCM 加密 + SM2 签名双层防护；
- **下游**：v0.100.0 资源争抢竞价机制 —— 竞价报价/撮合结果须经联邦共识确认（`is_committed(seq)` 为 true 后方可执行资源分配），防恶意节点伪造竞价结果。

---

## 6. P2 降级说明

> **蓝图评审 P2 原文要义**：PBFT 变体联邦共识对园区 VPP 可能过重。多数场景 Edge Coordinator 主从或仲裁即可。PBFT 作为可选高安全共识模式保留，默认部署使用主从仲裁模式；仅在需要 Byzantine 容错的跨信任域场景启用 PBFT。

**落地决策**：

- **默认部署**：`configs/federation-consensus.toml` 中 `mode = "primary-backup"`（或省略 mode 由运维脚本默认填充）—— 使用 v0.92.0 DomainArbiter 既有崩溃容错能力，消息量 O(n)、延迟低、无 ViewChange 风暴风险；
- **PBFT 启用条件**：跨运营商 / 跨主体 VPP 互不信任场景（如 A 电网与 B 电网联邦），经显式配置 `mode = "pbft"` 启用 Byzantine 容错；
- **配置开关**：mode 字段为纯配置语义，eneros-federation 本版本交付 PBFT 引擎完整实现；primary-backup 模式由 v0.92.0 仲裁器 crate 承载，不在本 crate 内实现分支，避免共识算法复杂度污染崩溃容错路径；
- **性能权衡**：PBFT O(n²) 消息量 vs 主从仲裁 O(n) 消息量——4 节点 PBFT 单次共识 12 条消息（PrePrepare 3 + Prepare 6 + Commit 6），主从仲裁仅 3 条（主→从广播 + 从→主确认），园区内同信任域场景优先主从仲裁。

---

## 7. 测试计划

40 个单元测试 TC1~TC12（consensus.rs）/ TP13~TP28（pbft.rs）/ TV29~TV40（view_change.rs）（src 内嵌 `#[cfg(test)]`，v0.87.0~v0.98.1 项目惯例，不新增 tests/ 文件，D11）：

| 分组 | 编号 | 覆盖点 |
|------|------|--------|
| 数据结构 + 派生（TC1~TC4） | TC1~TC4 | NodeId 类型别名；ConsensusState 5 变体互不等、Copy/Eq；MsgType to_u8/from_u8 往返一致；PbftMessage/LogEntry/ConsensusResult/ConsensusError Clone 独立性 |
| MockConsensusBus（TC5~TC9） | TC5~TC9 | new 初始状态（queues 空 / isolated 空 / fail_times 零）；broadcast 向除 isolated 外所有节点邮箱投递；isolated 节点不投出也不收信；fail_times=2 → 前 2 次 Err(BusError) 递减，第 3 次 Ok；receive 弹出队首，空 → None |
| 引擎构造（TC10~TC12） | TC10~TC12 | new 合法输入 → view==0/sequence==0/state==Idle/4 计数器全零/last_latency_ms==0/consecutive_vc==0；nodes 为空/含重复 → Err(NotEnoughNodes)；nodes 不含 local_id 或 peers 缺节点公钥 → Err(UnknownNode) |
| 自由函数（TP13~TP16） | TP13~TP16 | f(4)=1, f(7)=2, f(1)=0；quorum(4)=3, quorum(7)=5, quorum(1)=1；primary_of([1,2,3,4], 0)=1, primary_of(...,1)=2, primary_of(...,4)=1（循环）；sign_message/verify_message 正签可验、错签拒验、错公钥拒验 |
| submit + on_pre_prepare（TP17~TP20） | TP17~TP20 | submit 非主 → Err(NotPrimary)；submit 主节点：seq=1、digest==SM3(request)、PrePrepare 含 payload 与有效签名、log[0].prepare_voters=={1}、state==PrePrepare、submit_count==1；on_pre_prepare 合法 → prepare_voters=={1}、广播 Prepare、state==Prepare；重复 PrePrepare 同 seq → 忽略 |
| on_prepare + on_commit（TP21~TP22） | TP21~TP22 | on_prepare 达 quorum（主票+2 备份=3）→ prepared=true、commit_voters 含 local_id、广播 Commit、state==Commit；on_commit 达 quorum → committed/executed=true、sequence=seq、state==Done、committed_count+=1、last_latency_ms 已记录 |
| 拜占庭 5 类攻击（TP23~TP27） | TP23~TP27 | **TP23** 伪造签名 Prepare → rejected_count+=1、日志无该票；**TP24** 主节点 digest≠SM3(payload) → 备份拒绝、StaleMessage、不 prepared；**TP25** 重复投票（同节点多次 Prepare）→ BTreeSet 去重、只计 1 票；**TP26** 错 view Prepare → StaleMessage 丢弃；**TP27** equivocation（同 seq 异 digest PrePrepare）→ 任一 digest 均无法收齐 quorum、无节点 committed |
| 多规模共识（TP28） | TP28 | 7 节点 f=2 quorum=5 共识达成 + n=1 f=0 quorum=1 自共识达成 + 连续 submit seq 1→2→3 逐序提交 |
| view_change 基础（TV29~TV31） | TV29~TV31 | check_timeout state=Idle/Done → Ok(false)；check_timeout 未超时 → Ok(false)；check_timeout 超时 → 广播 ViewChange、view_change_count+=1、consecutive_vc+=1、Ok(true) |
| 退避防风暴（TV32） | TV32 | 连续两轮主离线 → 二次 VC 成功且有效超时增大（3s→6s→...） |
| VC 票集与收敛（TV33~TV37） | TV33~TV37 | on_view_change msg.view≤self.view → Ok(false) 忽略；on_view_change 伪造签名 → rejected_count+=1 + Err(InvalidSignature)；VC 票集 BTreeSet 去重；VC 达 quorum → enter_view、view 更新、state=Idle、consecutive_vc=0；enter_view 后新主节点重发未 committed PrePrepare；主节点 isolated → 备份超时 → 3 备份 VC 达 quorum → enter_view(1) → 新主重发 → 3 诚实节点 committed |
| enter_view 后状态（TV38~TV39） | TV38~TV39 | enter_view 后 is_primary() 反映新视图主节点；正常推进中 check_timeout → Ok(false) 不误触发 |
| 计数器综合（TV40） | TV40 | 混合 submit/committed/rejected/view_change/last_latency_ms 场景后 4 计数器 + last_latency_ms 精确等于预期值 |

**性能目标**（共识 < 1s，4 节点，蓝图 §7.2）标注：**集成阶段验收，本版本交付算法骨架 + Mock 单元验证**。

**GPU 规则说明（蓝图 §6.6）**：本版本为纯标量 CPU 计算（SM3 哈希 / SM2 签名验签 / BTreeSet 投票计数 / 状态机迁移），无张量操作，**不涉及 GPU**。

---

## 8. 验收标准

- **功能**：PBFT 三阶段共识达成（PrePrepare/Prepare/Commit 全链路，蓝图 §4.3）；ViewChange 容忍主节点故障（蓝图 §4.4/§5.2）；新主节点重发未提交日志恢复共识（D8）；
- **性能**：共识达成 < 1s（4 节点，蓝图 §7.2）—— **集成阶段验收**，测量口径为注入时钟 `last_latency_ms = 提交时刻 − 请求受理时刻`（D12），非墙钟基准测试；本版本交付算法骨架 + Mock 单元验证；
- **安全**：容忍 f 拜占庭节点（n=3f+1，quorum=2f+1，蓝图 §7.3）；SM2 全签名验证 + BTreeSet 投票去重 + equivocation 防护（§5.1 拜占庭 5 类攻击）；
- **文档**：本设计文档 + `configs/federation-consensus.toml` 配置模板（中文注释 ≥6 点）；
- **出口判定**：P2-E 第 3 版达成，解锁 v0.100.0 资源争抢竞价机制 / Phase 2 联邦协议一致性出口。

---

## 9. 风险与注意事项

| 风险 | 说明 | 缓解 |
|------|------|------|
| 节点数增加延迟 | PBFT 消息复杂度 O(n²)，7 节点单次共识 30+ 条消息，网络延迟线性放大 | 联邦规模控制在 4~7 节点（蓝图 §8.1）；默认部署主从仲裁（v0.92.0），PBFT 仅跨信任域启用（§6 P2 降级） |
| 通信开销大 | 每请求全广播 3 轮，带宽占用高于主从仲裁 | 共识层仅承载策略决策/竞价结果等低频关键操作（非 10ms 控制路径）；Payload 限 1KB 以内（digest 主导，大数据经通道层分段） |
| ViewChange 风暴 | 网络分区抖动期连续主节点故障 → 反复切换视图 → 共识活锁（蓝图 §8.5） | 指数退避：`timeout_ms << min(consecutive_vc, max_view_change_backoff)`，封顶 3 级（3s→24s）；抖动期退避后视图趋于稳定 |
| 与 DDS 互补 | DDS 提供实时数据分发（控制大区 10ms 周期），PBFT 提供管理信息大区决策一致性；二者互补而非替代 | 控制路径不经过 PBFT（硬实时零 GC 要求）；PBFT 仅用于策略/竞价/配置变更等管理面共识 |
| 内存（蓝图 §43.6） | log Vec + vc_votes BTreeMap + peers BTreeMap 堆分配 | Agent Runtime 分区 ≤ 64MB 预算内；sequence 单调递增、log 可经快照截断（后续版本）；共识为低频管理面操作 |

---

## 10. 多角度要求

- **功能**（蓝图 §9）：PBFT 三阶段（PrePrepare/Prepare/Commit）全流程；ViewChange 主节点故障恢复；新主重发未提交日志（D8）；
- **性能**（蓝图 §9）：共识 < 1s（4 节点，蓝图 §7.2）；测量口径为注入时钟 last_latency_ms（D12）；
- **安全**（蓝图 §9/§7.3）：拜占庭容错 f ≤ (n-1)/3；SM2 全签名验证 + BTreeSet 票集去重 + equivocation 防护；错误 view/伪签/未知节点全部 rejected_count 留痕丢弃；
- **可靠**（蓝图 §9）：ViewChange 超时检测 + 指数退避防风暴（§8.5）；新主节点自动恢复未提交日志；check_timeout 不误触发（Idle/Done 态不广播）；
- **可维护**（蓝图 §9）：节点表注入 + toml 配置化（`configs/federation-consensus.toml`）；模式开关 mode（pbft/primary-backup）降级部署；
- **可观测**（蓝图 §9）：4 个 pub 计数器 submit_count / committed_count / rejected_count / view_change_count + last_latency_ms（注入时钟）；no_std 无 log crate，metric 全部字段化本地可查；
- **可扩展**（蓝图 §9）：`ConsensusBus` trait 注入式适配（Mock → 真实 channel/tunnel 传输，consensus 层零改动）；节点表变更须经离线协商同步公钥后滚动重启；
- **no_std**（蓝图 §43.1）：`core` / `alloc` only（`alloc::vec::Vec` / `alloc::collections::BTreeMap` / `alloc::collections::BTreeSet`），禁止 `std::*`；aarch64-unknown-none 交叉编译友好；path 依赖 eneros-crypto 既有 crate，零新增第三方依赖，SBOM 不变（D9）。

---

## 11. 接口契约

pub 项签名清单（与 spec.md ADDED Requirements 一致，含实现修正标注）：

```rust
// ===== consensus.rs =====

/// 节点标识（D2：无堆字符串，v0.97.0 惯例）
pub type NodeId = u64;

/// 共识状态机（D6 增 Idle），Debug + Clone + Copy + PartialEq + Eq
pub enum ConsensusState { Idle, PrePrepare, Prepare, Commit, Done }

/// 消息类型（D4 增 ViewChange），Debug + Clone + Copy + PartialEq + Eq
pub enum MsgType { PrePrepare, Prepare, Commit, Reply, ViewChange }

impl MsgType {
    pub fn to_u8(&self) -> u8;
    pub fn from_u8(v: u8) -> Option<Self>;
}

/// 共识消息帧（D4 含 sender + payload），Debug + Clone + PartialEq，字段全 pub
pub struct PbftMessage {
    pub msg_type: MsgType,
    pub view: u64,
    pub sequence: u64,
    pub digest: [u8; 32],
    pub payload: Vec<u8>,
    pub sender: NodeId,
    pub signature: [u8; 64],
}

/// 日志条目（D5 BTreeSet 票集），Clone，字段全 pub
pub struct LogEntry {
    pub sequence: u64,
    pub request: Vec<u8>,
    pub digest: [u8; 32],
    pub prepare_voters: BTreeSet<NodeId>,
    pub commit_voters: BTreeSet<NodeId>,
    pub prepared: bool,
    pub committed: bool,
    pub executed: bool,
}

impl LogEntry {
    pub fn prepare_count(&self) -> usize;
    pub fn commit_count(&self) -> usize;
}

/// 提交结果，Debug + Clone + PartialEq
pub struct ConsensusResult {
    pub sequence: u64,
    pub digest: [u8; 32],
    pub view: u64,
}

/// 错误枚举（D10 7 变体最小完备），Debug + Clone + Copy + PartialEq + Eq
pub enum ConsensusError {
    NotPrimary,
    UnknownNode,
    InvalidSignature,
    ViewMismatch,
    StaleMessage,
    NotEnoughNodes,
    BusError,
}

/// 共识总线抽象（sync，无 Send+Sync 约束，D3）：
/// 真实传输由集成阶段以 Box<dyn ConsensusBus> 注入
pub trait ConsensusBus {
    fn broadcast(&mut self, from: NodeId, msg: &PbftMessage) -> Result<(), ConsensusError>;
    fn receive(&mut self, to: NodeId) -> Option<(NodeId, PbftMessage)>;
}

/// Mock 总线：故障注入（isolated 节点离线模拟 + fail_times 递减），字段全 pub
pub struct MockConsensusBus {
    pub queues: BTreeMap<NodeId, Vec<(NodeId, PbftMessage)>>,
    pub isolated: BTreeSet<NodeId>,
    pub fail_times: u32,
}

impl MockConsensusBus {
    pub fn new() -> Self;
}

impl ConsensusBus for MockConsensusBus {
    fn broadcast(&mut self, from: NodeId, msg: &PbftMessage) -> Result<(), ConsensusError>;
    fn receive(&mut self, to: NodeId) -> Option<(NodeId, PbftMessage)>;
}

/// 共识引擎（不 derive Debug，含 Sm2KeyPair 私钥保护），字段全 pub
pub struct ConsensusEngine {
    pub nodes: Vec<NodeId>,
    pub local_id: NodeId,
    pub view: u64,
    pub sequence: u64,
    pub state: ConsensusState,
    pub log: Vec<LogEntry>,
    pub kp: Sm2KeyPair,
    pub peers: BTreeMap<NodeId, Sm2PublicKey>,
    pub timeout_ms: u64,
    pub last_progress_ms: u64,
    pub consecutive_vc: u32,
    pub submit_count: u64,
    pub committed_count: u64,
    pub rejected_count: u64,
    pub view_change_count: u64,
    pub last_latency_ms: u64,
    // 【实现修正】spec 字段表未列，实现中必需
    pub vc_votes: BTreeMap<u64, BTreeSet<NodeId>>,
    // 【实现修正】spec 字段表未列，签名随机源
    pub rng: CsRng,
    // 【实现修正】spec 字段表未列，last_latency_ms 计算用
    pub submitted_ms: u64,
}

impl ConsensusEngine {
    /// 构造引擎：nodes 空/含重复 → Err(NotEnoughNodes)；
    /// nodes 不含 local_id 或 peers 缺节点公钥 → Err(UnknownNode)；
    /// 合法输入：view=0/sequence=0/state=Idle/计数器全零
    pub fn new(
        nodes: Vec<NodeId>,
        local_id: NodeId,
        kp: Sm2KeyPair,
        peers: BTreeMap<NodeId, Sm2PublicKey>,
        timeout_ms: u64,
    ) -> Result<Self, ConsensusError>;

    /// 是否当前视图主节点
    pub fn is_primary(&self) -> bool;

    /// 指定 sequence 是否已提交
    pub fn is_committed(&self, seq: u64) -> bool;

    /// 提交请求（主节点路径，sync，D3）：
    /// 非主 → Err(NotPrimary)；构造 PrePrepare → broadcast →
    /// 建 LogEntry（prepare_voters={local_id}，D7）→ state=PrePrepare →
    /// submit_count+=1 → Ok(seq)
    pub fn submit(
        &mut self,
        request: Vec<u8>,
        bus: &mut dyn ConsensusBus,
        now_ms: u64,
    ) -> Result<u64, ConsensusError>;

    /// 轮询驱动（sync，D3）：
    /// 排空 bus.receive 邮箱，逐条 handle_message，推进状态机；
    /// 返回本次 poll 新产生的 ConsensusResult 集
    pub fn poll(
        &mut self,
        bus: &mut dyn ConsensusBus,
        now_ms: u64,
    ) -> Result<Vec<ConsensusResult>, ConsensusError>;

    /// 单条消息处理（sync，D3）：
    /// msg.view < self.view 或（msg.view > self.view 且非 ViewChange）→
    /// rejected_count+=1 + Err(StaleMessage)；
    /// sender 不在 nodes → Err(UnknownNode)；
    /// 验签失败 → rejected_count+=1 + Err(InvalidSignature)；
    /// 接受后 last_progress_ms = now_ms；按 msg_type 分发
    pub fn handle_message(
        &mut self,
        from: NodeId,
        msg: PbftMessage,
        bus: &mut dyn ConsensusBus,
        now_ms: u64,
    ) -> Result<Option<ConsensusResult>, ConsensusError>;
}

// ===== pbft.rs =====

/// f(n) = (n-1)/3，n≥1
pub fn f(n: usize) -> usize;

/// quorum(n) = 2*f(n)+1
pub fn quorum(n: usize) -> usize;

/// 指定视图的主节点：nodes[view % len]
pub fn primary_of(nodes: &[NodeId], view: u64) -> NodeId;

/// SM2 签名（msg_body = D9 域分离拼接）
pub fn sign_message(kp: &Sm2KeyPair, msg_body: &[u8], rng: &mut CsRng) -> [u8; 64];

/// SM2 验签
pub fn verify_message(pk: &Sm2PublicKey, msg_body: &[u8], sig: &[u8; 64]) -> bool;

// ===== view_change.rs =====

impl ConsensusEngine {
    /// 超时检测：state∈{Idle,Done} → Ok(false)；
    /// 未超时 → Ok(false)；超时 → 广播 ViewChange →
    /// view_change_count+=1 → consecutive_vc+=1 → Ok(true)
    pub fn check_timeout(
        &mut self,
        bus: &mut dyn ConsensusBus,
        now_ms: u64,
    ) -> Result<bool, ConsensusError>;

    /// ViewChange 消息处理：msg.view ≤ self.view → Ok(false) 忽略；
    /// 收集 new_view 投票 → 达 quorum → enter_view → Ok(true)
    pub fn on_view_change(
        &mut self,
        from: NodeId,
        msg: PbftMessage,
        now_ms: u64,
    ) -> Result<bool, ConsensusError>;

    /// 进入新视图：view=new_view / state=Idle / consecutive_vc=0 / last_progress_ms=now_ms；
    /// 若 self 为新主且 log 尾部未 committed → 重发 PrePrepare 恢复共识
    pub fn enter_view(
        &mut self,
        new_view: u64,
        bus: &mut dyn ConsensusBus,
        now_ms: u64,
    );
}
```

**sync 化 D3 说明**：蓝图 `pub async fn submit / handle_message` + `broadcast().await` / `wait_for().await` 落地为同步方法 + `poll()` 驱动（no_std 硬规则禁 async）：`submit` 广播 PrePrepare 后返回 seq；`poll(bus, now_ms)` 排空邮箱推进状态机，返回新提交结果集；`wait_for` 语义由增量式投票计数（BTreeSet 去重 + quorum 判定）替代。真实网络适配器以 `Box<dyn ConsensusBus>` 注入，consensus 层零改动。

---

## 12. 偏差声明

| 偏差 | 蓝图原文 | 本版本处理 |
|------|---------|-----------|
| **D1** | crate 路径 `crates/federation/src/{consensus,pbft,view_change}.rs` | 既有 `crates/agents/federation/src/` 追加同名 3 模块（项目 §2.3.1 硬规则：crate 必须按子系统分组；v0.97.0/v0.98.0 同 crate 先例） |
| **D2** | `NodeId` 未定义类型 | `pub type NodeId = u64`（无堆字符串标识，v0.97.0 D1 惯例） |
| **D3** | `pub async fn submit / handle_message` + `broadcast().await` / `wait_for().await` | sync 方法 + `poll()` 驱动（no_std 硬规则禁 async）：`submit` 广播 PrePrepare 后返回 seq；`poll(bus, now_ms)` 排空邮箱推进状态机，返回新提交结果集；`wait_for` 语义由增量式投票计数替代 |
| **D4** | `PbftMessage { msg_type, view, sequence, digest, signature }` | 增 `sender: NodeId`（投票去重与签名验证必需）+ `payload: Vec<u8>`（PrePrepare 携带请求本体，蓝图仅有 digest 无法让备份节点获得请求）；`MsgType` 增 `ViewChange` 变体（VC 消息复用同一消息帧/总线，`view` 字段承载目标视图）；`Reply` 保留占位（执行结果经 `ConsensusResult` 同步返回，网络 Reply 后置） |
| **D5** | `LogEntry { prepare_count: u32, commit_count: u32 }` | `prepare_voters / commit_voters: BTreeSet<NodeId>`（u32 计数无法识别拜占庭节点重复投票；BTreeSet 确定性去重，D4 集合惯例）+ `prepare_count() / commit_count()` 访问器保持蓝图语义 |
| **D6** | `ConsensusState { PrePrepare, Prepare, Commit, Done }` | 增 `Idle` 初始/视图切换后状态（引擎启动与 enter_view 后的合法静止态，否则初始状态无定义） |
| **D7** | §4.3 收集 2f+1 Prepare / 2f+1 Commit | 法定人数 `quorum(n) = 2f+1`：`f(n) = (n-1)/3`；PrePrepare 计入主节点 prepare 票（PBFT 经典变体优化，与蓝图 2f+1 数值一致；n=4 时主节点 PrePrepare + 2 备份 Prepare 即 prepared，容忍 1 备份静默/作恶） |
| **D8** | §4.4 主节点超时 → ViewChange（未定义消息细节） | 无独立 NewView 消息：ViewChange 广播达 2f+1 法定人数后各节点自主 `enter_view(new_view)`（VC 消息全网广播，诚实节点自然收敛同一 new_view，消除 NewView 伪造面）；新主节点对最近未提交日志重发 PrePrepare 恢复共识；连续 VC 超时指数退避（`timeout_ms << min(连续 vc 次数, 3)`，蓝图 §8.5 坑点"ViewChange 风暴"对策） |
| **D9** | 签名验证失败 → 丢弃（算法未指定） | SM2 签名（eneros-crypto 既有 `sm2_sign`/`sm2_verify` 复用，§5.5 防重复造轮子）：签名消息 = `msg_type:u8‖view:u64be‖sequence:u64be‖digest‖sender:u64be（‖payload）`；验签失败/未知 sender → rejected_count+=1 丢弃 |
| **D10** | 错误处理仅 2 条（超时/验签失败） | `ConsensusError { NotPrimary, UnknownNode, InvalidSignature, ViewMismatch, StaleMessage, NotEnoughNodes, BusError }`（7 变体最小完备） |
| **D11** | 测试 `tests/consensus.rs` | crate 内嵌 `#[cfg(test)]` 40 测试（v0.87.0~v0.98.1 项目惯例；Mock 总线故障注入覆盖主节点离线/拜占庭伪造/重复投票） |
| **D12** | §9 可观测"共识延迟 metric" | 4 个 pub 计数器（`submit_count` / `committed_count` / `rejected_count` / `view_change_count`）+ `last_latency_ms`（注入时钟：提交时刻 − 提交请求时刻） |

### 12.1 实现期补充偏差（pinned 修正，EX1~EX9）

实现期对 spec 的 pinned 修正，与 `crates/agents/federation/src/lib.rs` crate 文档 EX 表一致：

| 编号 | 偏差 | 理由 |
|------|------|------|
| **EX1** | `ConsensusEngine` 增 `pub rng: CsRng` 字段 + `new()` 增 `rng` 参数 | SM2 签名需要随机数（k 值），无法确定性派生；channel.rs/tunnel.rs 注入 CsRng 先例 |
| **EX2** | `ConsensusEngine` 增 `pub vc_votes: BTreeMap<u64, BTreeSet<NodeId>>` 字段 | spec 正文要求"LogEntry 外的独立 VC 票集"，字段表漏列；按目标视图分桶收集 VC 票 |
| **EX3** | `LogEntry` 增 `pub submitted_ms: u64` 字段 | D12 延迟口径"提交时刻 − 请求受理时刻"需要记录受理时刻，否则无法计算 |
| **EX4** | `MockConsensusBus` 增 `register(id)` 方法 | 投递集合 = 已注册且非 isolated 节点邮箱；引擎集合在 register 时建邮箱，测试为每节点 register |
| **EX5** | 未来视图 PrePrepare 乐观视图同步：先 `enter_view(msg.view)` 再按正常流程处理 | 签名有效的新视图 PrePrepare 证明新主已获 VC 法定人数；诚实节点随新主收敛，避免分区恢复后卡死在旧视图 |
| **EX6** | 未 committed 条目收到新视图 PrePrepare → 投票集重置为 {主} 并重广播 Prepare | D8 恢复路径的备份侧配套：新主重发 PrePrepare 后，备份须丢弃旧视图投票重新计票才能达成新视图 quorum |
| **EX7** | `MockConsensusBus::receive` 用 `remove(0)` FIFO 弹队首 | spec "弹出队首"语义；FIFO 保证测试消息序确定性（LIFO 会乱序 Prepare/Commit 处理） |
| **EX8** | `on_pre_prepare` 状态迁移（Prepare）先于投票广播 | 活性修正：广播失败（BusError）时节点仍保持 Prepare 态，`check_timeout` 可触发 VC 恢复；否则故障注入场景永久停滞 Idle 无法自愈（TV38 依赖） |
| **EX9** | `check_timeout` 发起 VC 时 `last_progress_ms = now_ms` 重启退避计时 | 防风暴修正：不重启则退避到 8x 封顶后每次调用都触发 VC（违背 §8.5 目的）；重启后 VC 间隔 1x/2x/4x/8x/8x… 频率有界（TV33 依赖） |

---

## 附录

### A. 消息帧格式表

| 消息类型 | msg_type(u8) | 签名 payload 说明 | 广播方向 |
|---------|-------------|------------------|---------|
| PrePrepare | 0 | 携带 request 本体（payload = request.clone()） | 主节点 → 全部备份 |
| Prepare | 1 | 无 payload（msg_body 不含 payload 域） | 备份节点 → 全部节点 |
| Commit | 2 | 无 payload | 各节点 prepared 后 → 全部节点 |
| Reply | 3 | 保留占位（执行结果经 ConsensusResult 同步返回） | — |
| ViewChange | 4 | 无 payload（sequence = 0，view 承载目标视图） | 超时备份 → 全部节点 |

**签名域分离格式**（D9）：`msg_type:u8‖view:u64be‖sequence:u64be‖digest:[u8;32]‖sender:u64be[‖payload]`

### B. 状态机迁移表

| 当前状态 | 触发事件 | 条件 | 下一状态 | 动作 |
|---------|---------|------|---------|------|
| Idle | submit（主节点） | is_primary()==true | PrePrepare | seq+=1, digest=SM3(request), broadcast PrePrepare, log 追加, prepare_voters={local_id} |
| Idle | on_pre_prepare（备份） | sender==primary, digest==SM3(payload) | Prepare | 建 log, prepare_voters={primary}, broadcast Prepare |
| PrePrepare | on_prepare | prepare_voters.len() < quorum | PrePrepare | prepare_voters.insert(sender) |
| PrePrepare | on_prepare | prepare_voters.len() >= quorum | Commit | prepared=true, commit_voters={local_id}, broadcast Commit |
| Prepare | on_commit | commit_voters.len() < quorum | Prepare | commit_voters.insert(sender) |
| Prepare | on_commit | commit_voters.len() >= quorum | Done | committed=executed=true, sequence=seq, committed_count+=1, last_latency_ms=now−submitted_ms |
| Done | check_timeout | now−last_progress_ms > 有效超时 | Idle（经 enter_view） | broadcast ViewChange, view_change_count+=1, consecutive_vc+=1 |
| *任意* | enter_view | VC 达 quorum | Idle | view=new_view, consecutive_vc=0, last_progress_ms=now；新主重发未 committed PrePrepare |

### C. ViewChange 决策流程图

```mermaid
flowchart TD
    A[check_timeout<br/>now_ms] --> B{state ∈<br/>{Idle, Done}?}
    B -->|是| C[Ok false<br/>无活跃请求不触发]
    B -->|否| D{now − last_progress_ms<br/>≤ 有效超时?}
    D -->|是| E[Ok false<br/>共识正常推进]
    D -->|否| F[有效超时 =<br/>timeout_ms << min(consecutive_vc, 3)]
    F --> G[构造 ViewChange<br/>msg.view = self.view + 1<br/>签名 → broadcast]
    G --> H[view_change_count += 1<br/>consecutive_vc += 1<br/>Ok true]

    I[on_view_change<br/>from, msg, now_ms] --> J{msg.view ≤<br/>self.view?}
    J -->|是| K[Ok false<br/>StaleMessage 陈旧忽略]
    J -->|否| L{sender 在<br/>nodes 中?}
    L -->|否| M[Err UnknownNode]
    L -->|是| N{verify_message<br/>验签通过?}
    N -->|否| O[rejected_count += 1<br/>Err InvalidSignature]
    N -->|是| P[vc_votes[new_view]<br/>.insert from]
    P --> Q{vc_votes[new_view]<br/>.len ≥ quorum?}
    Q -->|否| R[Ok false<br/>继续收集]
    Q -->|是| S[enter_view new_view]
    S --> T[view = new_view<br/>state = Idle<br/>consecutive_vc = 0]
    T --> U{is_primary()<br/>且尾部未 committed?}
    U -->|是| V[重发 PrePrepare<br/>恢复共识]
    U -->|否| W[Ok true]
    V --> W

    style C fill:#90EE90
    style E fill:#90EE90
    style K fill:#FFD700
    style M fill:#FF6B6B
    style O fill:#FF6B6B
    style R fill:#87CEEB
    style W fill:#90EE90
    style V fill:#DDA0DD
```

### D. 相关文档

- [cross-domain-channel-design.md](./cross-domain-channel-design.md) — v0.98.0 跨域通信通道设计文档（P2-E 第 2 版，上游安全传输 seam）
- [vertical-encrypt-design.md](./vertical-encrypt-design.md) — v0.98.1 纵向加密认证设计文档（同 crate tunnel.rs，联邦安全通道族）
- 源码路径：`../../crates/agents/federation/src/`（`consensus.rs` / `pbft.rs` / `view_change.rs` / `lib.rs`；crate 根：`crates/agents/federation/`）
- 配置模板：`../../configs/federation-consensus.toml`
- Spec：`.trae/specs/develop-v0990-pbft-consensus/spec.md`
- 蓝图：`蓝图/phase2.md` §v0.99.0（P2-E 第 3 版；§4.3 三阶段时序图 / §4.4 ViewChange / §7.2 <1s / §7.3 拜占庭容错 / §8.5 ViewChange 风暴 / §9 多角度要求）

### E. 版本基线

- **Phase 定位**：Phase 2 多机联邦 P2-E 第 3 版（前序 v0.98.0 跨域通信通道 P2-E 第 2 版）
- **下游解锁**：v0.100.0 资源争抢竞价机制（竞价结果须经联邦共识确认）/ Phase 2 联邦协议一致性出口
- **版本阶梯**：v0.97.0 联邦发现 → v0.98.0 跨域通信通道 → v0.98.1 纵向加密 → **v0.99.0 联邦共识（本版本）** → v0.100.0 资源争抢竞价
