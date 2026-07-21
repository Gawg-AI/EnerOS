# EnerOS v0.101.0 断网处理与孤岛模式设计文档

> **版本**：v0.101.0
> **蓝图**：phase2.md §v0.101.0（P2-E 收尾版）
> **Crate**：`eneros-federation`（`crates/agents/federation/src/{detector.rs,partition.rs,cache.rs,recovery.rs}`，既有 crate 追加 4 模块）

---

## 1. 版本目标

实现 **联邦网络分区检测 → 冻结交易 → 本地自治缓存 → 恢复增量同步** 的完整闭环（**Phase 2 P2-E 收尾版**），交付五大能力：

- **分区检测**：`PartitionDetector` 四态状态机（Connected / Suspected / Partitioned / Recovering），以 PBFT `quorum` 判据驱动状态迁移——quorum 不可达即确认分区，与 v0.99.0 共识语义闭环；
- **交易冻结**：Partitioned / Recovering 期间 `trading_frozen() == true`，AuctionEngine 使用侧查询该标志冻结撮合，防脑裂污染；
- **本地自治缓存**：`IslandMode<T>` 激活后缓存本地业务事件，`EventCache<T>` 溢出丢弃最旧并计数（`overflow_count` 可观测）；
- **恢复增量同步**：`RecoverySync::sync` 按队序增量上传，Conflict（sink 时间戳仲裁）跳过计数继续，UploadFailed（硬错误）立即中止保留缓存待重试；
- **显式完成防脑裂**：Recovering 须经 `complete_recovery()` 显式确认同步完成后方回 Connected 解冻，不自动回退。

**业务价值**：联邦网络中断时（跨运营商/跨主体 VPP 链路不可靠），系统仍安全运行——分区检测 <5s 冻结交易、数据不丢、恢复后不污染；P2-E 联邦协议出口的最后安全兜底。

**Phase 定位**：Phase 2 P2-E 收尾版；**上游解锁**：v0.99.0 联邦共识 / v0.100.0 竞价；**下游解锁**：v0.110.0 云边同步。

**性能目标**（蓝图 §7.2）：断网检测延迟 < 5s（心跳超时 3000ms + 上层 check 轮询 ≤1000ms，4 节点 quorum=3 场景）—— **集成阶段验收**，本版本交付状态机骨架 + Mock 单元验证（真实 Agent Runtime 注入后实测验收）。

---

## 2. 前置依赖

- **v0.99.0 联邦共识协议**（前序版本，P2-E 第 3 版）：复用 `consensus::NodeId`（`pub type NodeId = u64`）与 `pbft::quorum(n)` 数学——分区判据与共识法定人数语义闭环；
- **v0.100.0 资源争抢竞价**（前序版本，P2-E 第 4 版）：AuctionEngine 使用侧在撮合前查询 `trading_frozen()`，分区期间拒绝开新轮/执行分配，参数见 `configs/federation-auction.toml`；
- **v0.84.0 并离网切换**（grid_agent）：电网物理并离网检测（PCC 电气检测，`crates/agents/grid_agent/src/island_detect.rs`）；与 v0.101.0 **层次区分见 §4.4**——物理孤岛未必网络分区，反之亦然，正交可组合；
- **eneros-crypto**（workspace 既有 crate）：本版本无新增密码学操作，crypto 依赖已在 v0.98.0 引入（零新增第三方依赖，SBOM 不变）；
- 蓝图 `phase2.md` v0.101.0 章节（9 节版本模板，§4.3 状态机 / §7.2 <5s / §7.3 冻结 / §8.5 重同步）；
- **no_std + alloc**：`core` / `alloc` only——`alloc::collections::VecDeque` / `BTreeMap`；禁止 `std::*`（蓝图 §43.1 硬性要求）；
- **后续注入**：真实 Agent Runtime 驱动 detector 轮询 + island 激活/退出 + recovery sync 生命周期，recovery 模块不持有传输实现（D4 seam 分离）。

**上游解锁**：v0.99.0 联邦共识 / v0.100.0 竞价冻结保护；**下游解锁**：v0.110.0 云边同步（事件缓存 + 增量同步机制复用）。

---

## 3. 交付物清单

- `crates/agents/federation/src/detector.rs` — **新增**：`PartitionState`（四态枚举）+ `PartitionDetector`（心跳表 + quorum 判据 + `trading_frozen()` + `complete_recovery()` + `partition_count`）
- `crates/agents/federation/src/partition.rs` — **新增**：`IslandMode<T>`（activate/deactivate/cache_event + `activated_count` 可观测）
- `crates/agents/federation/src/cache.rs` — **新增**：`EventCache<T>`（VecDeque + max_size + 溢出丢弃最旧 + `overflow_count`）
- `crates/agents/federation/src/recovery.rs` — **新增**：`SyncError` / `SyncReport` / `SyncSink<T>` trait / `MockSyncSink<T>` / `RecoverySync::sync`（同步化增量上传）
- `crates/agents/federation/Cargo.toml` — **修改**：description 追加 v0.101.0 段；依赖不变（eneros-crypto path 引用已在 v0.98.0 引入）
- `crates/agents/federation/src/lib.rs` — **修改**：`pub mod cache; pub mod detector; pub mod partition; pub mod recovery;` + 新增类型全量重导出 + crate 文档追加 v0.101.0 说明与 D1~D10 偏差表（既有 10 模块零改动）
- `configs/federation-island.toml` — **新增**：`[island]` 段（heartbeat_timeout_ms / cache_max_size + 中文注释 ≥7 点）
- `docs/agents/island-mode-design.md` — 本设计文档（12 章节 + 2 Mermaid）
- **36 个单元测试** TC1~TC7（cache.rs）/ TD8~TD19（detector.rs）/ TI20~TI27（partition.rs）/ TR28~TR36（recovery.rs 含 e2e）（src 内嵌 `#[cfg(test)]`，v0.87.0~v0.100.0 项目惯例，不新增 tests/ 文件，D3）
- 根目录 4 文件版本同步 0.100.0 → 0.101.0（`Cargo.toml` / `Makefile` / `ci.yml` / `gate.rs` 注释）
- **无 BREAKING**：既有全部 crate 公共 API 零改动

---

## 4. 详细设计

### 4.0 Mermaid 状态机图

```mermaid
stateDiagram-v2
    [*] --> Connected: new(nodes, now_ms)

    Connected --> Suspected: alive < total && alive >= quorum
    note right of Connected --> Suspected : 部分失联，抖动容忍

    Connected --> Partitioned: alive < quorum
    note right of Connected --> Partitioned : 全失联直接升级（C36）\npartition_count += 1

    Suspected --> Connected: alive == total
    Suspected --> Partitioned: alive < quorum
    note right of Suspected --> Partitioned : 失联扩大确认分区\npartition_count += 1

    Partitioned --> Recovering: alive >= quorum
    note right of Partitioned --> Recovering : 心跳恢复但同步未完成\ntrading_frozen 仍为 true

    Recovering --> Connected: complete_recovery()
    note right of Recovering --> Connected : 显式完成同步后解冻\ntrading_frozen 变 false

    Recovering --> Partitioned: alive < quorum
    note right of Recovering --> Partitioned : 再失联回退（relapse）\npartition_count += 1
```

**状态机规则摘要**：
- **Connected**：alive == total（全部活跃）保持；alive < quorum 直接落 Partitioned（不经 Suspected，C36）；否则（quorum ≤ alive < total）落 Suspected；
- **Suspected**：alive == total 回 Connected；alive < quorum 落 Partitioned + partition_count；否则保持 Suspected（抖动容忍）；
- **Partitioned**：alive ≥ quorum 升 Recovering；否则保持 Partitioned；
- **Recovering**：alive < quorum 回退 Partitioned + partition_count；否则保持 Recovering（须 `complete_recovery()` 显式完成）。

### 4.1 数据结构

| 类型 | 字段 | 说明 | 派生 / 备注 |
|------|------|------|-------------|
| `pub enum PartitionState` | `Connected` / `Suspected` / `Partitioned` / `Recovering` | 四态状态机 | Debug + Clone + Copy + PartialEq + Eq |
| `pub struct PartitionDetector` | `heartbeat_timeout_ms: u64` | 心跳超时（ms） | — |
| | `last_contact: BTreeMap<NodeId, u64>` | 各节点最后联系时刻 | D6 替代 HashMap |
| | `state: PartitionState` | 当前状态 | — |
| | `total_nodes: usize` | 节点总数 | — |
| | `partition_count: u64` | 进入分区次数（可观测） | D7 字段化 metric |
| `pub struct EventCache<T>` | `events: VecDeque<T>` | 事件队列（队首最旧） | Debug + Clone |
| | `max_size: usize` | 最大容量 | — |
| | `overflow_count: u64` | 溢出丢弃计数（可观测） | clear 不归零 |
| `pub struct IslandMode<T>` | `active: bool` | 孤岛激活标志 | Debug + Clone |
| | `since: u64` | 进入孤岛时刻（ms，注入时钟 D6） | — |
| | `cache: EventCache<T>` | 本地事件缓存 | 退出后保留待同步 |
| | `activated_count: u64` | 进入孤岛次数（可观测） | D7 字段化 metric |
| `pub enum SyncError` | `UploadFailed` / `Conflict` | 同步错误 | Debug + Clone + Copy + PartialEq + Eq |
| `pub struct SyncReport` | `uploaded: u64` | 成功上传事件数 | Debug + Clone + Copy + PartialEq + Eq |
| | `conflicts: u64` | 冲突跳过事件数 | — |
| `pub trait SyncSink<T>` | `fn upload(&mut self, event: &T) -> Result<(), SyncError>` | 同步目标 seam | D4 生产由 channel/tunnel 适配注入 |
| `pub struct MockSyncSink<T>` | `uploaded: Vec<T>` / `fail_times: u32` / `conflict_times: u32` | 测试用故障注入 | — |
| `pub struct RecoverySync` | — | 恢复同步器（无状态） | — |

### 4.2 状态机迁移表

| 当前态 | 条件（alive = 活跃节点数） | 下一态 | 副作用 |
|--------|---------------------------|--------|--------|
| Connected | alive == total | Connected | 无 |
| Connected | alive < quorum | Partitioned | `partition_count += 1`（C36 不经 Suspected） |
| Connected | quorum ≤ alive < total | Suspected | 无 |
| Suspected | alive == total | Connected | 无 |
| Suspected | alive < quorum | Partitioned | `partition_count += 1` |
| Suspected | quorum ≤ alive < total | Suspected | 无（抖动容忍） |
| Partitioned | alive ≥ quorum | Recovering | 无 |
| Partitioned | alive < quorum | Partitioned | 无 |
| Recovering | alive < quorum | Partitioned | `partition_count += 1`（再失联回退） |
| Recovering | alive ≥ quorum | Recovering | 无（须 `complete_recovery()` 显式完成） |
| Recovering | `complete_recovery()` 调用 | Connected | 仅 Recovering 态有效（幂等保护） |

### 4.3 Mermaid 时序图：断网→自治→恢复全流程

```mermaid
sequenceDiagram
    participant AR as Agent Runtime
    participant PD as PartitionDetector
    participant IM as IslandMode&lt;T&gt;
    participant EC as EventCache&lt;T&gt;
    participant RS as RecoverySync
    participant SS as SyncSink&lt;T&gt;

    Note over AR,SS: 步骤 1：初始 Connected（4 节点联邦）
    AR->>PD: new([1,2,3,4], timeout=1000, now=0)
    PD-->>AR: state=Connected, trading_frozen=false

    Note over AR,SS: 步骤 2：节点 3,4 失联 → detector check
    AR->>PD: on_heartbeat(1, 500); on_heartbeat(2, 500)
    AR->>PD: check(1001)  // alive=2 < quorum=3
    PD-->>AR: state=Partitioned, trading_frozen=true

    Note over AR,SS: 步骤 3：冻结交易 + 进入孤岛
    AR->>IM: activate(1001)
    IM-->>AR: active=true, since=1001, activated_count=1
    AR->>IM: cache_event(e1)  // → EC.push
    IM->>EC: push(e1)
    AR->>IM: cache_event(e2)
    IM->>EC: push(e2)
    AR->>IM: cache_event(e3)
    IM->>EC: push(e3)
    IM-->>AR: cache.len=3, all accepted

    Note over AR,SS: 步骤 4：心跳恢复 → Recovering（仍冻结）
    AR->>PD: on_heartbeat(3, 1500)
    AR->>PD: check(1500)  // alive=3 >= quorum=3
    PD-->>AR: state=Recovering, trading_frozen=true

    Note over AR,SS: 步骤 5：恢复同步上传
    AR->>RS: sync(&EC, &mut SS)
    RS->>SS: upload(&e1) → Ok
    RS->>SS: upload(&e2) → Ok
    RS->>SS: upload(&e3) → Ok
    SS-->>RS: all Ok
    RS-->>AR: Ok(SyncReport { uploaded:3, conflicts:0 })

    Note over AR,SS: 步骤 6：显式完成 → Connected 解冻
    AR->>PD: complete_recovery(1600)
    PD-->>AR: state=Connected, trading_frozen=false

    Note over AR,SS: 步骤 7：退出孤岛，缓存保留
    AR->>IM: deactivate()
    IM-->>AR: active=false, cache 保留（数据不丢）

    Note over AR,SS: 步骤 8：上层确认后显式 clear
    AR->>EC: clear()
    EC-->>AR: events 空, overflow_count 保留
```

### 4.4 与 v0.84.0 grid_agent IslandDetector 层次区分

| 维度 | v0.84.0 grid_agent IslandDetector | v0.101.0 eneros-federation PartitionDetector |
|------|----------------------------------|----------------------------------------------|
| **检测对象** | 电网物理并离网（PCC 断路器 + 频率/电压越限） | 联邦网络分区（PBFT quorum 通信层面） |
| **所在 crate** | `crates/agents/grid_agent/src/island_detect.rs` | `crates/agents/federation/src/detector.rs` |
| **切换组件** | `GridTransfer` 物理切换（PCC breaker 命令） | `IslandMode<T>` 逻辑自治缓存 |
| **触发源** | 电气量测（频率/电压/PCC 状态） | 心跳超时 + quorum 判据 |
| **冻结对象** | 并网功率交换（物理 PCC 开关） | 联邦资源竞价撮合（AuctionEngine 交易） |
| **恢复条件** | 主网电压/频率恢复 + 连续确认 | 活跃数回升 ≥ quorum + 增量同步完成 + `complete_recovery()` |
| **四态/三态** | `IslandResult` 三态（Islanded / GridOk / Uncertain） | `PartitionState` 四态（Connected/Suspected/Partitioned/Recovering） |

**正交可组合关系**：物理孤岛未必网络分区（单节点与主网物理断开，但其与其他联邦节点仍可通过备用通道通信，PBFT quorum 仍可达）；反之网络分区也未必物理孤岛（通信链路中断但电网物理连接完好）。两者独立检测、独立处置，由上层 Agent Runtime 组合决策。

---

## 5. 技术交底

### 5.1 选型对比表（蓝图 §5.1）

| 断网策略 | 数据完整性 | 复杂度 | 结论 |
|---------|-----------|--------|------|
| **事件缓存 + 补传** | 高（队序保留，冲突仲裁） | 中（状态机 + 缓存 + 同步 seam） | **⭐ 采用**（蓝图 §5.1） |
| 状态快照 | 中（快照体积大，频繁快照内存开销高） | 中（快照一致性保障复杂） | 备选（Phase 2+ 大状态 Agent 场景） |
| 忽略 | 低（断网期间数据全部丢失） | 低 | 不采用（能源调控数据不可丢） |

### 5.2 关键技术

- **事件溯源 + 增量同步**：孤岛期间事件按 `VecDeque` 队序缓存（队首最旧），恢复后按同一队序增量上传，保持事件因果序；sink 端时间戳仲裁解决冲突（云端已有更新则本地事件跳过）；
- **quorum 语义闭环**：分区判据 `alive < quorum(n)` 直接复用 v0.99.0 `pbft::quorum`，确保"分区 = 无法提交任何共识决议"——业务上断网与共识不可达等价；
- **显式完成防脑裂**：Recovering → Connected 不自动发生，须经 `complete_recovery()` 显式确认——防止网络抖动导致"假恢复"后两侧各自解冻交易（脑裂）。

### 5.3 难点

- **同步冲突 → sink 时间戳仲裁**：同一可调资源在分区期间可能被本地 Agent 与云端 Agent 分别调整，恢复后 sink 端（云端）以时间戳判断"谁更新"，`Conflict` 跳过旧事件并计数——不抛异常、不阻塞后续上传；
- **缓存内存上限**：`max_size` 配置化（默认 1024 条），按每事件 ~100B 估算 ≤ ~100KB，在 Agent Runtime 64MB 分区预算内（蓝图 §43.6）；溢出丢弃最旧并以 `overflow_count` 可观测替代告警日志（D7）；
- **恢复重同步策略**：`UploadFailed` 硬错误立即中止，`cache` 保留不丢待上层重试（蓝图 §8.5）；已上传部分由 sink 侧幂等去重（`uploaded` Vec 保序 + 逐条 `upload` 语义）；
- **检测延迟与抖动平衡**：`heartbeat_timeout_ms = 3000ms` + 上层轮询 ≤1000ms，满足 <5s 验收；过短易受跨运营商链路抖动误剔，过长拖延冻结保护生效。

### 5.4 交互

- **上游**：v0.99.0 `ConsensusEngine`（复用 `NodeId` / `quorum`）/ v0.100.0 `AuctionEngine`（冻结保护使用侧，`trading_frozen()` 查询）；
- **下游**：v0.110.0 云边同步（事件缓存 + 增量同步机制复用，`SyncSink<T>` seam 适配云端上传）；
- **同层**：eneros-federation 内部仅新增 4 模块，既有 10 模块零改动；`RecoverySync` 不持有传输实现，字节流/事件交由上层 channel/tunnel 注入。

---

## 6. 测试计划

36 个单元测试 TC1~TC7（cache.rs）/ TD8~TD19（detector.rs）/ TI20~TI27（partition.rs）/ TR28~TR36（recovery.rs 含 e2e）（src 内嵌 `#[cfg(test)]`，v0.87.0~v0.100.0 项目惯例，不新增 tests/ 文件，D3）：

| 分组 | 编号 | 覆盖点 |
|------|------|--------|
| 缓存基础（TC1~TC7） | TC1 | new 初始：events 空 / max_size / overflow_count==0 / is_empty |
| | TC2 | push 顺序保持：队首最旧，迭代序正确 |
| | TC3 | 溢出丢弃最旧：max_size=2，push 1,2,3 → events==[2,3]，overflow_count==1 |
| | TC4 | 连续溢出计数：push 1..=5 → overflow_count==3 |
| | TC5 | max_size=1 边界：每次 push 都丢弃最旧，len 恒 1 |
| | TC6 | clear 清空 events 保留 overflow_count；再 push 正常 |
| | TC7 | 泛型双型实例化：EventCache\<u64\> 与 EventCache\<TestEvent\> 均正常 |
| 检测基础（TD8~TD11） | TD8 | new 初始 Connected / total_nodes / last_contact |
| | TD9 | on_heartbeat 已知节点更新 |
| | TD10 | on_heartbeat 未知节点忽略（C30） |
| | TD11 | alive_count 边界含等：now - last == timeout 仍活跃 |
| 状态机迁移（TD12~TD19） | TD12 | Connected→Suspected（quorum ≤ alive < total） |
| | TD13 | Suspected→Connected 回退（全部恢复） |
| | TD14 | Suspected 保持（≥ quorum 未全恢复） |
| | TD15 | Suspected→Partitioned（失联扩大 alive < quorum） |
| | TD16 | Connected→Partitioned 直接升级（不经 Suspected，C36） |
| | TD17 | trading_frozen 四态真值表（D9） |
| | TD18 | Partitioned→Recovering + complete_recovery 解冻 |
| | TD19 | Recovering 再失联回退 Partitioned（partition_count 累加） |
| 孤岛模式（TI20~TI27） | TI20 | new 初始：active=false / since=0 / activated_count=0 |
| | TI21 | activate 置位 since + 计数 |
| | TI22 | 重复激活幂等：since 不变，activated_count 不增 |
| | TI23 | cache_event 激活入队 true |
| | TI24 | 未激活拒 false |
| | TI25 | 溢出经 IslandMode 透传计数 |
| | TI26 | deactivate 缓存保留（数据不丢） |
| | TI27 | deactivate 后拒缓存 |
| 恢复同步（TR28~TR35） | TR28 | 空缓存 sync → Ok(0,0) |
| | TR29 | 全量上传保序 |
| | TR30 | UploadFailed 立即中止；后续换 sink 重试成功 |
| | TR31 | Conflict 跳过计数继续 |
| | TR32 | 多 Conflict 连续 |
| | TR33 | UploadFailed 后缓存不丢（sync 不自动 clear） |
| | TR34 | 队序保持 |
| | TR35 | 泛型 struct Mock |
| e2e 全流程（TR36） | TR36 | 4 节点联邦：Connected→Partitioned（孤岛缓存）→Recovering→sync→Connected；冻结/解冻断言 |

**覆盖点摘要**：状态机全部迁移路径（含回退 relapse）、边界含等、四态真值表、缓存溢出/保留/clear 语义、同步错误分治（Conflict/UploadFailed）、泛型双型、e2e 组合流程。

**GPU 规则说明（蓝图 §6.6）**：本版本为纯标量 CPU 计算（状态机比较、BTreeMap 遍历、VecDeque 入队出队、计数器），无张量操作，**不涉及 GPU**。

---

## 7. 验收标准

- **功能**：断网检测（四态状态机全部迁移路径）/ 冻结交易（Partitioned/Recovering 期 trading_frozen）/ 本地自治缓存（activate→cache→deactivate 保留）/ 恢复同步（Conflict 跳过 + UploadFailed 中止 + 队序保持）；
- **性能**：断网检测延迟 < 5s（心跳超时 3000ms + 轮询 ≤1000ms，4 节点场景）；
- **安全**：冻结交易防脑裂（quorum 判据 + complete_recovery 显式完成）；
- **可靠**：数据不丢（缓存保留 + sync 不自动 clear + UploadFailed 保留重试）；
- **文档**：本设计文档 + `configs/federation-island.toml` 配置模板（中文注释 ≥7 点）；
- **出口判定**：P2-E 收尾版达成，解锁 v0.110.0 云边同步。

---

## 8. 风险

| 风险 | 说明 | 缓解 |
|------|------|------|
| 脑裂（双主） | 网络分区两侧各自认为自己是主网，各自解冻交易导致同一资源被重复分配 | quorum 判据（alive < quorum 才确认分区，Suspected 不冻结）；Recovering→Connected 须 `complete_recovery()` 显式完成（不自动解冻）；同步完成前 trading_frozen 保持 true |
| 缓存内存超限 | 长时间孤岛导致缓存溢出，最旧事件被丢弃 | max_size 配置化（默认 1024，≤ ~100KB）；overflow_count 可观测告警；Agent Runtime 64MB 分区预算内 |
| 恢复重同步策略 | 恢复后链路仍不稳定，sync 反复 UploadFailed | UploadFailed 保留缓存重试（不丢数据）；sink 侧幂等去重已上传部分；上层 Agent Runtime 指数退避重试（蓝图 §8.5） |
| 与 v0.84.0 物理孤岛混淆 | 运维/开发人员误将联邦网络分区与电网物理并离网混为一谈 | 本设计文档 §4.4 层次区分表明确；命名上 PartitionDetector（网络）vs IslandDetector（电网物理） |
| 时钟回拨 | 注入时钟 `now_ms` 由上层提供，存在回拨可能 | `alive_count` 用 `saturating_sub` 防下溢（v0.97.0 D10 惯例）；回拨时 last_seen > now → 差值为 0 ≤ timeout，节点保留（不误剔） |

---

## 9. 多角度要求

- **功能**（蓝图 §9）：四态状态机全部迁移路径 + 交易冻结 + 本地自治缓存 + 恢复增量同步 + 显式完成防脑裂；
- **性能**（蓝图 §9）：检测 < 5s（heartbeat_timeout_ms + 轮询周期）；
- **安全**（蓝图 §9）：quorum 判据防误剔；Partitioned/Recovering 冻结交易；complete_recovery 显式完成防自动解冻；
- **可靠**（蓝图 §9）：数据不丢（缓存保留、sync 不自动 clear、UploadFailed 保留重试）；
- **可维护**（蓝图 §9）：`configs/federation-island.toml` 参数化 heartbeat_timeout_ms / cache_max_size；状态机代码固化（跨节点必须逐字节一致）；
- **可观测**（蓝图 §9）：`partition_count` / `activated_count` / `overflow_count` / `SyncReport` + `PartitionState` pub；no_std 无 log crate，metric 全部字段化本地可查；
- **可扩展**（蓝图 §9）：大规模分区场景下 `last_contact` 用 `BTreeMap` O(log n)，VecDeque 队序遍历 O(n)，n 为缓存事件数（远小于节点数）；
- **no_std**（蓝图 §43.1）：`core` / `alloc` only；禁止 `std::*`；aarch64-unknown-none 交叉编译友好；path 依赖 eneros-crypto 既有 crate，零新增第三方依赖，SBOM 不变。

---

## 10. 接口契约

pub 项签名清单（与 spec.md 一致，含实现修正标注）：

```rust
// ===== cache.rs =====

/// 事件缓存（蓝图 §4.1 EventCache，D5 泛型化）
pub struct EventCache<T> {
    pub events: VecDeque<T>,
    pub max_size: usize,
    pub overflow_count: u64,
}

impl<T> EventCache<T> {
    pub fn new(max_size: usize) -> Self;
    /// 溢出丢弃最旧 + overflow_count += 1
    pub fn push(&mut self, e: T);
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    /// 清空事件，保留 overflow_count（历史可观测不归零，C21）
    pub fn clear(&mut self);
}

// ===== detector.rs =====

/// 四态状态机（蓝图 §4.3）
pub enum PartitionState { Connected, Suspected, Partitioned, Recovering }

pub struct PartitionDetector {
    pub heartbeat_timeout_ms: u64,
    pub last_contact: BTreeMap<NodeId, u64>,
    pub state: PartitionState,
    pub total_nodes: usize,
    pub partition_count: u64,
}

impl PartitionDetector {
    /// 全部节点 last_contact = now_ms，state = Connected
    pub fn new(nodes: &[NodeId], heartbeat_timeout_ms: u64, now_ms: u64) -> Self;
    /// 已知节点更新 last_contact；未知节点忽略（C30）
    pub fn on_heartbeat(&mut self, from: NodeId, now_ms: u64);
    /// 活跃节点数：now - last_contact <= timeout（边界含等，C31）
    pub fn alive_count(&self, now_ms: u64) -> usize;
    /// 状态机检查（D8 quorum 判据），返回迁移后状态
    pub fn check(&mut self, now_ms: u64) -> PartitionState;
    /// 交易冻结查询（D9）：Partitioned | Recovering → true
    pub fn trading_frozen(&self) -> bool;
    /// 显式完成恢复（D9）：仅 Recovering → Connected 返回 true；其余 false
    pub fn complete_recovery(&mut self, _now_ms: u64) -> bool;
}

// ===== partition.rs =====

pub struct IslandMode<T> {
    pub active: bool,
    pub since: u64,
    pub cache: EventCache<T>,
    pub activated_count: u64,
}

impl<T> IslandMode<T> {
    pub fn new(cache_max_size: usize) -> Self;
    /// 幂等激活：已 active 时不重置 since、不递增 activated_count（C50）
    pub fn activate(&mut self, now_ms: u64);
    /// 退出孤岛：active=false；缓存保留（蓝图 §4.5 数据不丢）
    pub fn deactivate(&mut self);
    /// 缓存事件（D10 返回 bool 可观测）：!active → false 不入缓存
    pub fn cache_event(&mut self, e: T) -> bool;
}

// ===== recovery.rs =====

pub enum SyncError { UploadFailed, Conflict }
pub struct SyncReport { pub uploaded: u64, pub conflicts: u64 }

/// 同步目标 trait seam（D4：生产由 channel/tunnel 适配注入）
pub trait SyncSink<T> {
    fn upload(&mut self, event: &T) -> Result<(), SyncError>;
}

/// Mock 同步目标（测试用故障注入）
pub struct MockSyncSink<T> {
    pub uploaded: Vec<T>,
    pub fail_times: u32,
    pub conflict_times: u32,
}

pub struct RecoverySync;

impl RecoverySync {
    /// 同步化增量上传（D4）：Conflict 跳过计数继续，UploadFailed 立即 Err
    /// 保留缓存（蓝图 §8.5）；空缓存 → Ok(0,0)；缓存由上层显式 clear
    pub fn sync<T, S: SyncSink<T>>(
        cache: &EventCache<T>,
        sink: &mut S,
    ) -> Result<SyncReport, SyncError>;
}
```

**关键偏差说明**：
- **D4 同步化**：蓝图 `async fn sync` → 同步 `fn sync` + `SyncSink<T>` trait seam。no_std 硬规则禁 async（v0.99.0 D3 先例）；生产由 channel/tunnel 适配注入真实传输，测试用 `MockSyncSink<T>` 故障注入；
- **D5 泛型化**：`EventCache<T>` 不直接引用 v0.96.0 `EventRecord`，上层以 `cloud-coordinator::EventRecord` / `DomainData` 实例化；
- **D6 注入时钟**：蓝图 `Duration` / `HashMap` / `now_ms()` → `u64` ms / `BTreeMap` / 注入 `now_ms` 参数。no_std alloc 无 `HashMap`；注入时钟保证确定性可复现 + 可测（v0.99.0 D12 先例）。

---

## 11. 偏差声明

| 偏差 | 蓝图原文 | 本版本处理 |
|------|---------|----------|
| **D1** | 蓝图 `crates/federation/src/` | `crates/agents/federation/src/` —— 记忆 §2.3.1 强制：所有 crate 归 `crates/<subsystem>/`；既有 crate 增量扩展（v0.98.0~v0.100.0 同例） |
| **D2** | 蓝图 `docs/phase2/island_mode.md` | `docs/agents/island-mode-design.md` —— 记忆 §2.3.3 强制：文档按方向分类 |
| **D3** | 蓝图 `tests/partition.rs` | src 内嵌 `#[cfg(test)]` 36 测试 —— v0.87.0~v0.100.0 项目惯例，不新增 tests/ 文件 |
| **D4** | 蓝图 `async fn sync` | **同步 fn** `RecoverySync::sync` + `SyncSink<T>` trait seam —— no_std 硬规则禁 async（v0.99.0 D3 先例）；上传经 trait seam，生产由 channel/tunnel 适配注入 |
| **D5** | `EventCache` 引用 `EventRecord` | **`EventCache<T>` 泛型化** —— eneros-federation 保持仅依赖 eneros-crypto（SBOM 不变）；避免 agents 子系统内横向耦合（v0.100.0 D11 先例）；上层以 `EventRecord`/`DomainData` 实例化 |
| **D6** | 蓝图 `Duration`/`HashMap`/`now_ms()` | `u64` ms / `BTreeMap` / **注入时钟参数** —— no_std alloc 无 HashMap；注入时钟保证确定性可复现 + 可测（v0.99.0 D12 先例） |
| **D7** | 蓝图 `info!`/`warn!` 日志 | **计数器字段**（`overflow_count`/`activated_count`）+ 状态 pub —— no_std 无 log crate，metric 字段化（v0.99.0 D12/v0.100.0 D9 同例） |
| **D8** | 蓝图"确认断网"判据未细化 | **alive < quorum(n)**（复用 `pbft::quorum`） —— 与 v0.99.0 共识语义闭环：quorum 不可达即无法提交任何决议，业务上等同断网；Suspected（部分失联但 ≥ quorum）不冻结，容忍抖动 |
| **D9** | 蓝图无 `trading_frozen` / `complete_recovery` | 增 `trading_frozen()` 查询 + `complete_recovery()` 显式完成 —— 蓝图 §7.3"断网冻结交易"的落地接口（AuctionEngine 使用侧查询）；Recovering→Connected 需同步完成事件驱动（蓝图 §4.3 状态图"同步完成"迁移） |
| **D10** | 蓝图 `cache_event` 空返回 | **`cache_event` 返回 `bool`** —— 未激活静默丢弃需可观测（蓝图 §4.5 return 语义的可测化） |

---

## 12. 附录

### A. 蓝图 v0.101.0 章节映射表

| 蓝图章节 | 实现落点 | 说明 |
|---------|---------|------|
| §1 版本目标 | 本文档 §1 | 业务价值 / P2-E 收尾 / 性能目标 <5s |
| §2 前置依赖 | 本文档 §2 | v0.99.0 / v0.100.0 / v0.84.0 区分 / no_std |
| §3 交付物 | 本文档 §3 | 4 模块 + lib.rs/Cargo.toml + 配置 + 文档 + 36 测试 |
| §4.1 数据结构 | detector.rs / partition.rs / cache.rs / recovery.rs | 四态 / IslandMode / EventCache / SyncSink |
| §4.2 接口 | recovery.rs D4 seam / lib.rs re-export | 同步化 + trait seam 注入 |
| §4.3 核心算法 | 本文档 §4.0 Mermaid 图 | 状态机迁移路径（含 C36 直接升级） |
| §4.4 错误处理 | cache.rs overflow / recovery.rs Conflict+UploadFailed | 丢弃最旧计数 / sink 仲裁 / 硬错误保留 |
| §4.5 关键代码 | partition.rs / recovery.rs | 注入时钟 / 同步 fn / 幂等激活 |
| §5.1 选型 | 本文档 §5.1 | 事件缓存+补传 ⭐ 采用 |
| §5.2 关键技术 | 本文档 §5.2 | 事件溯源+增量同步 / quorum 闭环 / 显式完成 |
| §5.3 实现路径 | detector→partition→cache→recovery→lib.rs→config→doc | 4 模块 + 配置 + 设计文档 |
| §5.4 难点 | 本文档 §5.3 | 冲突仲裁 / 缓存内存 / 重同步策略 |
| §5.5 交互 | 本文档 §5.4 | 上游 v0.99.0/v0.100.0，下游 v0.110.0 |
| §6 测试 | 本文档 §6 + 源码 `#[cfg(test)]` | TC/TD/TI/TR 共 36 测试 |
| §7 验收 | 本文档 §7 | 功能/性能/安全/可靠/文档 |
| §8 风险 | 本文档 §8 | 脑裂/缓存内存/重同步/层次混淆/时钟回拨 |
| §9 多角度 | 本文档 §9 | 功能/性能/安全/可靠/可维护/可观测/可扩展/no_std |

### B. 源码路径

- `crates/agents/federation/src/detector.rs`
- `crates/agents/federation/src/partition.rs`
- `crates/agents/federation/src/cache.rs`
- `crates/agents/federation/src/recovery.rs`
- `crates/agents/federation/src/lib.rs`
- crate 根：`crates/agents/federation/`

### C. 配置路径

- `../../configs/federation-island.toml`
- 竞价配置：`../../configs/federation-auction.toml`
- 共识配置：`../../configs/federation-consensus.toml`

### D. 相关文档

- [auction-design.md](./auction-design.md) — v0.100.0 资源争抢竞价设计文档（P2-E 第 4 版，冻结保护使用侧）
- [pbft-consensus-design.md](./pbft-consensus-design.md) — v0.99.0 联邦共识协议设计文档（P2-E 第 3 版，quorum 复用）
- [federation-discovery-design.md](./federation-discovery-design.md) — v0.97.0 联邦发现设计文档（P2-E 第 1 版）
- [cross-domain-channel-design.md](./cross-domain-channel-design.md) — v0.98.0 跨域通信通道设计文档（P2-E 第 2 版）
- [vertical-encrypt-design.md](./vertical-encrypt-design.md) — v0.98.1 纵向加密认证设计文档
- Spec：`.trae/specs/develop-v10100-island-mode/spec.md`
- 蓝图：`蓝图/phase2.md` §v0.101.0（P2-E 收尾版；§4.3 状态机 / §7.2 <5s / §7.3 冻结 / §8.5 重同步）

### E. TR36 e2e 状态序列摘要

TR36 复现的完整状态序列（4 节点联邦，timeout=1000ms）：

| 步骤 | 操作 | detector.state | trading_frozen | island.active | cache.len | 备注 |
|------|------|----------------|----------------|---------------|-----------|------|
| 1 | new([1,2,3,4], 1000, 0) | Connected | false | — | — | 初始 |
| 2 | on_hb(1,500); on_hb(2,500); check(1001) | Partitioned | true | 未激活 | — | alive=2 < quorum=3，直接升级 |
| 3 | activate(1001) | Partitioned | true | true | 0 | 进入孤岛 |
| 4 | cache_event(101/102/103) | Partitioned | true | true | 3 | 缓存 3 条事件 |
| 5 | on_hb(3,1500); check(1500) | Recovering | true | true | 3 | alive=3 ≥ quorum=3 |
| 6 | sync(cache, sink) | Recovering | true | true | 3 | Ok(uploaded=3, conflicts=0) |
| 7 | complete_recovery(1600) | Connected | false | true | 3 | 显式完成解冻 |
| 8 | deactivate() | Connected | false | false | 3 | 退出孤岛，缓存保留 |
| 9 | cache.clear() | Connected | false | false | 0 | 上层确认后清空 |

**断言点**：
- `partition_count == 1`（仅进入 Partitioned 一次）
- `activated_count == 1`（仅 activate 一次）
- `overflow_count == 0`（无溢出）
- 全程 `cache.events` 队序保持 [101, 102, 103]

### F. 版本阶梯

- **Phase 定位**：Phase 2 多机联邦 P2-E 收尾版（前序 v0.100.0 竞价 P2-E 第 4 版）
- **下游解锁**：v0.110.0 云边同步（事件缓存 + 增量同步机制复用）
- **版本阶梯**：v0.97.0 联邦发现 → v0.98.0 跨域通信通道 → v0.98.1 纵向加密 → v0.99.0 联邦共识 → v0.100.0 资源争抢竞价 → **v0.101.0 断网处理与孤岛模式（本版本）** → v0.110.0 云边同步
