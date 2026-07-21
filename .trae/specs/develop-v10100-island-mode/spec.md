# v0.101.0 断网处理与孤岛模式 Spec

> 蓝图：`e:\eneros\蓝图\phase2.md` §v0.101.0（P2-E 收尾版，9 节齐全）。Crate：`eneros-federation`（既有 crate 追加 4 模块，v0.97.0~v0.100.0 同例）。

## Why

联邦场景网络中断时（跨运营商/跨主体 VPP 链路不可靠），系统必须：检测分区 → 冻结交易进入孤岛自治 → 本地缓存事件 → 恢复后增量同步，保证"数据不丢、交易不被脑裂污染"。v0.99.0 PBFT 共识要求 quorum 可达，quorum 失联即丧失跨域决策能力，必须有确定性分区处置机制。

## What Changes

- **新增** `crates/agents/federation/src/cache.rs`：`EventCache<T>` 泛型事件缓存（VecDeque + max_size + 溢出丢弃最旧 + overflow_count 可观测）
- **新增** `crates/agents/federation/src/detector.rs`：`PartitionState`（Connected/Suspected/Partitioned/Recovering）+ `PartitionDetector`（心跳表 + quorum 判据状态机 + `trading_frozen()` 交易冻结查询）
- **新增** `crates/agents/federation/src/partition.rs`：`IslandMode<T>`（activate/deactivate/cache_event + activated_count 可观测）
- **新增** `crates/agents/federation/src/recovery.rs`：`RecoverySync::sync`（同步化）+ `SyncSink<T>` trait seam + `MockSyncSink<T>` + `SyncError`/`SyncReport`
- **修改** `crates/agents/federation/src/lib.rs`：4 模块声明 + 全类型重导出 + crate 文档追加 v0.101.0 段与偏差表（既有 10 模块零改动）
- **修改** `crates/agents/federation/Cargo.toml`：description 追加 v0.101.0（依赖不变，仍仅 eneros-crypto）
- **新增** `configs/federation-island.toml`：`[island]` heartbeat_timeout_ms / cache_max_size + 中文注释 ≥6 点
- **新增** `docs/agents/island-mode-design.md`：12 章节 + ≥2 Mermaid + 偏差表
- **新增 36 个单元测试**（src 内嵌 `#[cfg(test)]`，项目惯例，不新增 tests/ 文件）
- 根目录 4 文件版本同步 0.100.0 → 0.101.0（`Cargo.toml` / `Makefile` / `ci.yml` / `gate.rs` 注释串尾 2 处）
- **无 BREAKING**：既有全部 crate 公共 API 零改动

## Impact

- Affected specs：develop-v10100-island-mode（新建）
- Affected code：`crates/agents/federation/`（4 新模块 + lib.rs/Cargo.toml 增量）、`configs/`、`docs/agents/`、根 4 文件版本号
- 上游：v0.99.0 共识（`consensus::NodeId` / `pbft::quorum` 同 crate 复用）、v0.100.0 竞价（Partitioned 冻结交易保护 AuctionEngine 使用侧）
- 下游：v0.110.0 云边同步（事件缓存 + 增量同步机制复用）

## ADDED Requirements

### Requirement: 分区检测状态机（detector.rs）

The system SHALL provide `PartitionDetector`，以注入时钟 `now_ms` 驱动四态状态机（Connected/Suspected/Partitioned/Recovering），分区判据与 PBFT quorum 语义闭环。

#### Scenario: 心跳正常保持 Connected
- **WHEN** 全部节点 `now_ms - last_contact <= heartbeat_timeout_ms`，调用 `check(now_ms)`
- **THEN** 状态保持 `Connected`，`trading_frozen() == false`

#### Scenario: 部分失联进入 Suspected
- **WHEN** 至少 1 节点超时但活跃数 ≥ quorum(n)，调用 `check(now_ms)`
- **THEN** Connected → Suspected，交易不冻结（抖动容忍）

#### Scenario: quorum 不可达确认分区
- **WHEN** 活跃节点数 < quorum(n)（复用 `pbft::quorum`，n=4 时 quorum=3）
- **THEN** 状态 → Partitioned，`trading_frozen() == true`（冻结交易，蓝图 §7.3）

#### Scenario: 恢复与同步完成
- **WHEN** Partitioned 下活跃数恢复 ≥ quorum → Recovering（保持冻结）；上层完成同步后调用 `complete_recovery(now_ms)`
- **THEN** Recovering → Connected，`trading_frozen() == false`；Recovering 中再次失联 < quorum → 回退 Partitioned

#### Scenario: 时钟注入确定性
- **WHEN** `on_heartbeat(from, now_ms)` / `check(now_ms)` 接收外部时钟（蓝图 `Duration/now_ms()` 偏差落地）
- **THEN** 相同输入序列产生相同状态序列（跨节点可复现）

### Requirement: 孤岛模式与事件缓存（partition.rs + cache.rs）

The system SHALL provide `IslandMode<T>` 与 `EventCache<T>`：孤岛激活期间缓存本地事件，缓存满丢弃最旧并计数，退出孤岛停止缓存。

#### Scenario: 激活与缓存
- **WHEN** `activate(now_ms)` 后 `cache_event(e)`
- **THEN** `active == true`、`since == now_ms`、事件入队尾、`activated_count == 1`

#### Scenario: 未激活拒绝缓存
- **WHEN** `active == false` 时 `cache_event(e)`
- **THEN** 返回 `false`，事件不入缓存（蓝图 §4.5 直接 return 的可观测化）

#### Scenario: 溢出丢弃最旧
- **WHEN** 缓存长度 == max_size 时 `cache_event(e)`
- **THEN** 队首（最旧）事件被丢弃，新事件入队尾，`overflow_count += 1`（蓝图 §4.4 丢弃最旧并告警 → no_std 无 log，计数器可观测）

#### Scenario: 退出孤岛保留缓存
- **WHEN** `deactivate()` 后读取缓存
- **THEN** 缓存内容保留待同步（数据不丢，蓝图 §7.2），后续 `cache_event` 返回 `false`

### Requirement: 恢复同步（recovery.rs）

The system SHALL provide 同步化 `RecoverySync::sync(cache, sink) -> Result<SyncReport, SyncError>`（no_std 禁 async，偏差声明）：按队序增量上传，冲突跳过计数，硬错误中止。

#### Scenario: 全量上传成功
- **WHEN** sink 全部接受，`sync(cache, &mut sink)`
- **THEN** `Ok(SyncReport { uploaded: n, conflicts: 0 })`，缓存由上层 `clear()` 清空

#### Scenario: 时间戳仲裁冲突
- **WHEN** sink 对某事件返回 `Err(SyncError::Conflict)`（sink 端时间戳仲裁：云端已有更新数据）
- **THEN** 跳过该事件继续上传，`conflicts += 1`，最终 `Ok`（蓝图 §4.4 冲突仲裁落地：sink 仲裁 + 本地计数）

#### Scenario: 硬错误中止保数据
- **WHEN** sink 返回 `Err(SyncError::UploadFailed)`（链路仍不可用）
- **THEN** `sync` 立即返回 `Err`，已上传事件由 sink 侧幂等去重，缓存不丢待重试（蓝图 §8.5 重同步策略）

### Requirement: 端到端断网全流程（蓝图 §6.2 集成测试）

The system SHALL 在 src 内嵌测试中复现：心跳正常 → 节点失联 → Suspected → Partitioned（冻结+孤岛激活+缓存）→ 恢复 Recovering → sync 上传 → Connected（解冻+停缓存）。

#### Scenario: 断网→自治→恢复
- **WHEN** 4 节点联邦（quorum=3）模拟 2 节点失联再恢复
- **THEN** 状态序列 Connected→Suspected→Partitioned→Recovering→Connected；Partitioned 期间 `trading_frozen()==true` 且事件全部缓存；恢复后缓存全部上传且 overflow/conflict 计数正确

## MODIFIED Requirements

无（纯新增模块，既有 10 模块零改动）。

## REMOVED Requirements

无。

## 偏差声明（D1~D10，相对蓝图 §3/§4/§5）

| 编号 | 偏差 | 理由 |
|------|------|------|
| **D1** | 蓝图 `crates/federation/src/` → `crates/agents/federation/src/` | 记忆 §2.3.1 强制：所有 crate 归 `crates/<subsystem>/`；既有 crate 增量扩展（v0.98.0~v0.100.0 同例） |
| **D2** | 蓝图 `docs/phase2/island_mode.md` → `docs/agents/island-mode-design.md` | 记忆 §2.3.3 强制：文档按方向分类 |
| **D3** | 蓝图 `tests/partition.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.100.0 项目惯例，不新增 tests/ 文件 |
| **D4** | `RecoverySync::sync` 蓝图 `async fn` → **同步 fn** | no_std 硬规则禁 async（v0.99.0 D3 先例）；上传经 `SyncSink<T>` trait seam，生产由 channel/tunnel 适配注入 |
| **D5** | `EventCache<T>` **泛型化**，不直接引用 v0.96.0 `EventRecord` | eneros-federation 保持仅依赖 eneros-crypto（SBOM 不变）；避免 agents 子系统内横向耦合（v0.100.0 D11 先例）；上层以 cloud-coordinator `EventRecord`/`DomainData` 实例化 |
| **D6** | 蓝图 `Duration`/`HashMap`/`now_ms()` → `u64` ms / `BTreeMap` / **注入时钟参数** | no_std alloc 无 HashMap；注入时钟保证确定性可复现 + 可测（v0.99.0 D12 先例） |
| **D7** | `info!`/`warn!` 日志 → **计数器字段**（overflow_count/activated_count）+ 状态 pub | no_std 无 log crate，metric 字段化（v0.99.0 D12/v0.100.0 D9 同例） |
| **D8** | "确认断网"判据落地为 **alive < quorum(n)**（复用 `pbft::quorum`） | 与 v0.99.0 共识语义闭环：quorum 不可达即无法提交任何决议，业务上等同断网；Suspected（部分失联但 ≥ quorum）不冻结，容忍抖动 |
| **D9** | 增 `trading_frozen()` 查询 + `complete_recovery()` 显式完成 | 蓝图 §7.3"断网冻结交易"的落地接口（AuctionEngine 使用侧查询）；Recovering→Connected 需同步完成事件驱动（蓝图 §4.3 状态图"同步完成"迁移） |
| **D10** | `cache_event` 返回 `bool`（蓝图为空返回） | 未激活静默丢弃需可观测（蓝图 §4.5 return 语义的可测化） |

## 接口契约

```rust
// cache.rs
pub struct EventCache<T> { pub events: VecDeque<T>, pub max_size: usize, pub overflow_count: u64 }
impl<T> EventCache<T> {
    pub fn new(max_size: usize) -> Self;
    pub fn push(&mut self, e: T);                    // 溢出丢弃最旧 + overflow_count+=1
    pub fn len(&self) -> usize; pub fn is_empty(&self) -> bool;
    pub fn clear(&mut self);                          // 保留 overflow_count
}

// detector.rs
pub enum PartitionState { Connected, Suspected, Partitioned, Recovering }  // Debug/Clone/Copy/PartialEq/Eq
pub struct PartitionDetector {
    pub heartbeat_timeout_ms: u64,
    pub last_contact: BTreeMap<NodeId, u64>,          // NodeId = crate::consensus::NodeId
    pub state: PartitionState,
    pub total_nodes: usize,
    pub partition_count: u64,                          // 进入分区次数（可观测）
}
impl PartitionDetector {
    pub fn new(nodes: &[NodeId], heartbeat_timeout_ms: u64, now_ms: u64) -> Self;
    pub fn on_heartbeat(&mut self, from: NodeId, now_ms: u64);   // 未知节点忽略
    pub fn check(&mut self, now_ms: u64) -> PartitionState;      // 四态迁移
    pub fn alive_count(&self, now_ms: u64) -> usize;
    pub fn trading_frozen(&self) -> bool;                        // Partitioned|Recovering → true
    pub fn complete_recovery(&mut self, now_ms: u64) -> bool;    // 仅 Recovering 有效
}

// partition.rs
pub struct IslandMode<T> {
    pub active: bool, pub since: u64,
    pub cache: EventCache<T>, pub activated_count: u64,
}
impl<T> IslandMode<T> {
    pub fn new(cache_max_size: usize) -> Self;
    pub fn activate(&mut self, now_ms: u64);    // 重复激活不重置 since（幂等）
    pub fn deactivate(&mut self);               // 缓存保留
    pub fn cache_event(&mut self, e: T) -> bool;
}

// recovery.rs
pub enum SyncError { UploadFailed, Conflict }   // Debug/Clone/Copy/PartialEq/Eq
pub struct SyncReport { pub uploaded: u64, pub conflicts: u64 }  // Debug/Clone/Copy/PartialEq/Eq
pub trait SyncSink<T> { fn upload(&mut self, event: &T) -> Result<(), SyncError>; }
pub struct MockSyncSink<T> { pub uploaded: Vec<T>, pub fail_times: u32, pub conflict_times: u32 }
pub struct RecoverySync;
impl RecoverySync {
    pub fn sync<T, S: SyncSink<T>>(cache: &EventCache<T>, sink: &mut S) -> Result<SyncReport, SyncError>;
}
```

## 测试规划（36 个）

| 文件 | 编号 | 数量 | 覆盖 |
|------|------|------|------|
| cache.rs | TC1~TC7 | 7 | new 初始 / push 入队 / 溢出丢弃最旧 + 计数 / max_size=1 边界 / clear 保留 overflow_count / len/is_empty / 泛型（u64 与 struct 两型实例化） |
| detector.rs | TD8~TD19 | 12 | new 初始 Connected / on_heartbeat 更新与未知节点忽略 / alive_count 边界（==timeout 活跃）/ Connected→Suspected / Suspected→Connected 回退 / Suspected 中 ≥quorum 不升级 / <quorum → Partitioned + partition_count / 全失联直接升级路径 / trading_frozen 四态真值表 / Partitioned→Recovering / complete_recovery 仅 Recovering 有效 / Recovering 再失联回退 Partitioned |
| partition.rs | TI20~TI27 | 8 | new 初始 / activate 置位 since + 计数 / 重复激活幂等（since 不变）/ cache_event 激活入队 true / 未激活拒 false / 溢出经 IslandMode 计数 / deactivate 缓存保留 / deactivate 后拒缓存 |
| recovery.rs | TR28~TR36 | 9 | 空缓存 Ok(0,0) / 全量上传 / fail_times 递减后成功 / UploadFailed 中止 + 缓存保留 / Conflict 跳过计数继续 / 顺序保持（队序）/ Mock 泛型 struct / **e2e 断网全流程**（detector+island+cache+sync 组合，蓝图 §6.2）/ e2e 中 Partitioned 冻结 + 恢复解冻断言 |
