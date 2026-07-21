# v0.99.0 联邦共识协议（PBFT 变体） Spec

> 蓝图：`蓝图/phase2.md` v0.99.0（P2-E 第 3 版）。
> 蓝图无 v0.99.x 刚性子版本，本 spec 仅覆盖 v0.99.0。
> ★ 蓝图 PBFT 降级说明（评审 P2）：PBFT 为可选高安全共识模式；默认部署用主从仲裁（v0.92.0 DomainArbiter 既有），仅跨信任域 Byzantine 容错场景经配置启用 PBFT。

## Why

v0.98.0 完成跨域加密通信通道（安全传输基础），蓝图 v0.99.0 要求实现 **联邦共识协议（PBFT 变体）**：跨域决策一致性，容忍 ≤ f 个拜占庭节点（3f+1 总节点），为 v0.100.0 资源争抢竞价提供"跨域决策可信、防恶意节点"的协议一致性基础（Phase 2 联邦出口关联项）。

## What Changes

- **eneros-federation 扩展**（既有 crate 追加 3 模块，membership.rs / discovery.rs / channel.rs / tunnel.rs **零改动**）：
  - `src/consensus.rs`（新增）— `NodeId` / `ConsensusState` / `MsgType` / `PbftMessage` / `LogEntry` / `ConsensusResult` / `ConsensusError` / `ConsensusBus` trait / `MockConsensusBus` / `ConsensusEngine`（核心字段 + new/submit/poll/handle_message/is_committed + 4 计数器 + 延迟观测）
  - `src/pbft.rs`（新增）— 三阶段逻辑：`impl ConsensusEngine` 扩展（on_pre_prepare/on_prepare/on_commit）+ 自由函数 `f()` / `quorum()` / `primary_of()` / `sign_message()` / `verify_message()`
  - `src/view_change.rs`（新增）— ViewChange 逻辑：`impl ConsensusEngine` 扩展（check_timeout/on_view_change/enter_view）+ 指数退避
  - `src/lib.rs`：`pub mod consensus; pub mod pbft; pub mod view_change;` + 重导出 + crate 文档升级为 v0.97.0+v0.98.0/v0.98.1+v0.99.0 说明与 D1~D12 偏差表
  - `Cargo.toml`：description 升级三版本（依赖不变，eneros-crypto 已在 v0.98.0 引入）
- 新增 `configs/federation-consensus.toml`（共识配置：mode / timeout / 节点表）
- 新增 `docs/agents/pbft-consensus-design.md`（12 章节 + 2 Mermaid + D1~D12）
- 根目录 4 文件版本同步 0.98.0 → 0.99.0（Cargo.toml / Makefile / ci.yml / gate.rs 注释）
- 内嵌单元测试 40 个（TP1~TP40）
- **无 BREAKING**：既有全部 crate 公共 API 零改动

## Impact

- Affected specs：无既有 spec 受影响；关联 develop-v0980-cross-domain-channel（前置安全通道）、develop-v0920-edge-arbiter（P2 降级默认主从仲裁）
- Affected code：`crates/agents/federation/`（3 新模块 + lib.rs/Cargo.toml 增量）、`configs/`、`docs/agents/`、根 4 文件
- 依赖：**零新增第三方依赖**（eneros-crypto 为既有 path 依赖，SM2 签名/SM3 摘要复用），SBOM 不变
- 下游解锁：v0.100.0 资源争抢竞价机制、Phase 2 联邦协议一致性出口

## 偏差声明（D1~D12）

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

## ADDED Requirements

### Requirement: 共识数据结构与总线抽象（consensus.rs）

系统 SHALL 提供（`eneros_federation::consensus`，no_std + alloc）：

- `pub type NodeId = u64`
- `ConsensusState { Idle, PrePrepare, Prepare, Commit, Done }`（Debug/Clone/Copy/PartialEq/Eq）
- `MsgType { PrePrepare, Prepare, Commit, Reply, ViewChange }`（Debug/Clone/Copy/PartialEq/Eq）+ `to_u8() / from_u8()`
- `PbftMessage { msg_type, view: u64, sequence: u64, digest: [u8;32], payload: Vec<u8>, sender: NodeId, signature: [u8;64] }`（Debug/Clone/PartialEq，字段全 pub）
- `LogEntry { sequence, request: Vec<u8>, digest: [u8;32], prepare_voters: BTreeSet<NodeId>, commit_voters: BTreeSet<NodeId>, prepared: bool, committed: bool, executed: bool }`（Clone，字段全 pub）+ `prepare_count() / commit_count()` 访问器
- `ConsensusResult { sequence: u64, digest: [u8;32], view: u64 }`（Debug/Clone/PartialEq）
- `ConsensusError { NotPrimary, UnknownNode, InvalidSignature, ViewMismatch, StaleMessage, NotEnoughNodes, BusError }`（Debug/Clone/Copy/PartialEq/Eq）
- sync trait `ConsensusBus { fn broadcast(&mut self, from: NodeId, msg: &PbftMessage) -> Result<(), ConsensusError>; fn receive(&mut self, to: NodeId) -> Option<(NodeId, PbftMessage)>; }`（无 async、无 Send+Sync）
- `MockConsensusBus { pub queues: BTreeMap<NodeId, Vec<(NodeId, PbftMessage)>>, pub isolated: BTreeSet<NodeId>, pub fail_times: u32 }`（字段全 pub）：broadcast 向除 isolated 外所有已知节点邮箱投递（isolated 节点不投出也不收信，模拟离线）；fail_times>0 → Err(BusError) 递减；receive 弹出队首

**ConsensusEngine**（不 derive Debug，因含 Sm2KeyPair；字段全 pub）：
`{ nodes: Vec<NodeId>, local_id: NodeId, view: u64, sequence: u64, state: ConsensusState, log: Vec<LogEntry>, kp: Sm2KeyPair, peers: BTreeMap<NodeId, Sm2PublicKey>, timeout_ms: u64, last_progress_ms: u64, consecutive_vc: u32, submit_count: u64, committed_count: u64, rejected_count: u64, view_change_count: u64, last_latency_ms: u64 }`
- `new(nodes, local_id, kp, peers, timeout_ms) -> Result<Self, ConsensusError>`：nodes 空/含重复/不含 local_id/peers 缺节点 → Err(NotEnoughNodes 或 UnknownNode)；view=0/sequence=0/state=Idle/计数器全零
- `is_primary(&self) -> bool`（`primary_of(nodes, view) == local_id`）
- `is_committed(&self, seq: u64) -> bool`
- `submit(&mut self, request: Vec<u8>, bus: &mut dyn ConsensusBus, now_ms: u64) -> Result<u64, ConsensusError>`（sync，D3）
- `poll(&mut self, bus: &mut dyn ConsensusBus, now_ms: u64) -> Result<Vec<ConsensusResult>, ConsensusError>`
- `handle_message(&mut self, from: NodeId, msg: PbftMessage, bus: &mut dyn ConsensusBus, now_ms: u64) -> Result<Option<ConsensusResult>, ConsensusError>`：msg.view < self.view 或（msg.view > self.view 且非 ViewChange）→ rejected_count+=1 + Err(StaleMessage)；sender 不在 nodes → Err(UnknownNode)；验签失败 → rejected_count+=1 + Err(InvalidSignature)；按 msg_type 分发 pbft/view_change 处理；任何接受的消息更新 `last_progress_ms = now_ms`

#### Scenario: 引擎构造校验
- **WHEN** `new(vec![1,2,3,4], 1, kp1, peers4, 3000)`
- **THEN** Ok：view==0、sequence==0、state==Idle、4 计数器全零、is_primary()==true（view 0 → nodes[0]）
- **WHEN** nodes 不含 local_id / nodes 为空 / peers 缺某节点公钥
- **THEN** `Err(NotEnoughNodes)` 或 `Err(UnknownNode)`

### Requirement: PBFT 三阶段（pbft.rs）

系统 SHALL 提供 `impl ConsensusEngine` 扩展与自由函数：

- `pub fn f(n: usize) -> usize`（`(n-1)/3`，n≥1）；`pub fn quorum(n: usize) -> usize`（`2*f(n)+1`）；`pub fn primary_of(nodes: &[NodeId], view: u64) -> NodeId`（`nodes[(view % len) as usize]`）
- `pub fn sign_message(kp: &Sm2KeyPair, msg_body: &[u8], rng: &mut CsRng) -> [u8; 64]`；`pub fn verify_message(pk: &Sm2PublicKey, msg_body: &[u8], sig: &[u8;64]) -> bool`（msg_body = D9 域分离拼接）
- `submit`（主节点路径）：非主 → Err(NotPrimary)；seq=sequence+1；digest=SM3(request)；构造 PrePrepare（payload=request，签名）→ bus.broadcast → log 追加 LogEntry（prepare_voters={local_id}，D7 主节点票）→ state=PrePrepare → submit_count+=1 → Ok(seq)
- `on_pre_prepare`（备份路径）：sender 非该 view 主节点 → rejected_count+=1 + Err(ViewMismatch)；digest != SM3(payload) → rejected_count+=1 + Err(StaleMessage)；sequence 已存在日志 → 忽略 Ok(None)；建 LogEntry（prepare_voters={primary}）→ 广播 Prepare（同 digest）→ state=Prepare
- `on_prepare`：找到 seq 日志（无 → Err(StaleMessage)）→ prepare_voters.insert(sender)（重复自然去重）→ `prepare_voters.len() >= quorum(n) && !prepared` → prepared=true → commit_voters.insert(local_id) → 广播 Commit → state=Commit
- `on_commit`：commit_voters.insert(sender) → `commit_voters.len() >= quorum(n) && !committed` → committed=true → executed=true → sequence=seq → state=Done → committed_count+=1 → last_latency_ms = now_ms − 请求受理时刻 → Ok(Some(ConsensusResult))

#### Scenario: 4 节点共识达成（蓝图 §6.2 集成场景）
- **WHEN** 4 引擎（各持 SM2 密钥对）+ 1 MockConsensusBus，节点1 submit(b"馈线容量分配")，循环驱动各节点 poll 至无消息
- **THEN** 4 节点 `is_committed(1)==true`、digest 全一致、sequence==1、state==Done；主节点拿到 ConsensusResult；last_latency_ms 已记录
- **WHEN** n=4（f=1）且 1 个备份节点被 isolated（拜占庭静默）
- **THEN** 其余 3 节点仍达成 committed（PrePrepare 主票 + 2 备份 Prepare = 3 = quorum）

### Requirement: 拜占庭防护（蓝图 §7.3 安全）

系统 SHALL 拒绝：伪造签名消息（→ InvalidSignature 丢弃）；digest 与 payload 不符的 PrePrepare；重复投票（BTreeSet 去重）；错误 view 的消息（→ StaleMessage/ViewMismatch）；未知节点消息（→ UnknownNode）。所有拒绝计入 rejected_count。

#### Scenario: 拜占庭节点被识别（蓝图 §6.5）
- **WHEN** 拜占庭节点用错误私钥签名 Prepare 并广播
- **THEN** 各诚实节点 rejected_count+=1，日志无该票，共识不受污染
- **WHEN** 拜占庭主节点向不同节点发不同 digest 的 PrePrepare（equivocation）
- **THEN** 任一 digest 均无法收齐 quorum（诚实备份仅认 SM3(payload)==digest 者），状态机安全不提交

### Requirement: ViewChange（view_change.rs，蓝图 §4.4/§6.5）

系统 SHALL 提供 `impl ConsensusEngine` 扩展：

- `check_timeout(&mut self, bus, now_ms) -> Result<bool, ConsensusError>`：state ∈ {Idle, Done} → Ok(false)；now − last_progress ≤ 有效超时（`timeout_ms << min(consecutive_vc, 3)`）→ Ok(false)；否则发起 VC：广播 ViewChange（msg.view = self.view + 1，sequence=0，签名）→ view_change_count+=1 → consecutive_vc+=1 → Ok(true)
- `on_view_change(&mut self, from, msg, now_ms) -> Result<bool, ConsensusError>`：msg.view ≤ self.view → Ok(false)（陈旧忽略）；收集该 new_view 投票（LogEntry 外的独立 VC 票集，BTreeSet 去重）→ 达 quorum → `enter_view(new_view, bus, now_ms)` → Ok(true)
- `enter_view(&mut self, new_view, bus, now_ms)`：view=new_view → state=Idle → consecutive_vc=0 → last_progress_ms=now_ms；若 self 为新主且 log 尾部有未 committed 条目 → 以其 digest/request 重发 PrePrepare（同 sequence，D8 恢复共识）

#### Scenario: 主节点离线触发 ViewChange（蓝图 §6.5 故障注入）
- **WHEN** 4 节点、节点1（主）被 isolated、节点1 submit 后共识停滞；备份节点以递增 now_ms 调 check_timeout
- **THEN** 超时后备份广播 ViewChange；3 备份互收达 quorum（3）→ enter_view(1)；新主节点2 重发 PrePrepare；继续 poll → 3 诚实节点 committed
- **WHEN** 共识正常推进中 check_timeout
- **THEN** Ok(false)，无 ViewChange 消息

### Requirement: 配置与文档

- `configs/federation-consensus.toml`：`[consensus]` 段 — mode（`"pbft"`，P2 降级说明：默认部署 `"primary-backup"` 用 v0.92.0 主从仲裁，跨信任域才启用 pbft）/ timeout_ms / max_view_change_backoff / nodes 表；中文注释 ≥6 点（3f+1 假设 / quorum 2f+1 / 共识 <1s §7.2 / ViewChange 退避 §8.5 / 签名验证 §7.3 / 4 计数器+延迟 D12）
- `docs/agents/pbft-consensus-design.md`：12 章节 + 2 Mermaid（蓝图 §4.3 三阶段时序图重绘 + ViewChange 决策流程图含 StaleMessage/InvalidSignature/退避分支）+ D1~D12 偏差表与本 spec 一致 + 接口契约与实现签名一致

## MODIFIED Requirements

### Requirement: eneros-federation crate 元数据

- `Cargo.toml` description 升级为三版本说明（v0.97.0 联邦发现 + v0.98.0/v0.98.1 通道与纵向加密 + v0.99.0 联邦共识）；依赖不变
- `lib.rs` 追加 `pub mod consensus; pub mod pbft; pub mod view_change;` + 新增类型全量重导出 + crate 文档追加 v0.99.0 说明与 D1~D12 偏差表（既有模块文档/重导出保留，零改动）
- 根 `Cargo.toml` `[workspace.package] version = "0.99.0"`；`Makefile` / `.github/workflows/ci.yml` 版本注释同步；`ci/src/gate.rs` 注释串尾追加 v0.99.0 类型清单

## REMOVED Requirements

无。
