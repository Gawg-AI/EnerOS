# v0.110.0 云边数据同步 Spec

> 蓝图：`e:\eneros\蓝图\phase2.md` §v0.110.0（P2-H 第 2 版，9 节齐全）。新建 crate `crates/agents/cloud-sync/`（eneros-cloud-sync，零第三方依赖）。蓝图检索确认无 v0.110.x 刚性子版本（Phase 2 刚性子版本仅 v0.98.1）。

## Why

边缘侧遥测/状态/告警/控制日志/交易/配置六类事件必须可靠汇聚到云端 TSDB，支撑全局分析与审计（蓝图 §1）。断网期间事件不得丢失，网络恢复后需增量补传。v0.96.0 已落地云端数据汇聚，v0.101.0 已落地断网处理（EventCache/RecoverySync），v0.109.0 已产出可上传的 COMTRADE 录波文件。本版实现事件溯源存储 + 增量批量同步 + 指数退避重试队列，打通「事件 → 存储 → 同步 → 补传」链路，为 v0.111.0 模型 OTA 提供云边通道基础。

## What Changes

- **新建** `crates/agents/cloud-sync/`（`eneros-cloud-sync`，no_std + alloc，零第三方依赖）：
  - `src/event_store.rs`：`Event` / `EventType`（6 变体）/ `EventStore`（事件溯源存储，容量有界、满则 compact 已同步、CRC32 完整性，D8/D9）+ `crc32()`（CRC32-IEEE const 表，D8）
  - `src/delta_sync.rs`：`CompressionType`（3 变体，仅 None 可构造，D5）/ `SyncBatch`（D10 归属）/ `SyncStats`（D12）/ `DeltaSync`（`build_batch` + 二进制帧 `serialize`，D11 + `sync_once`/`retry_once` 编排）
  - `src/retry_queue.rs`：`RetryQueue`（指数退避封顶 300s + 确定性 xorshift32 抖动，D6；有界死信队列，D10）
  - `src/lib.rs`：`SyncError`（5 变体，D12）/ `SyncTransport` trait + `MockSyncTransport`（D4）+ 模块声明 + 重导出 + crate 文档（含 D1~D12 偏差表）
- **新增** `configs/cloud-sync.toml`：`[event_store]` / `[delta_sync]` / `[retry_queue]` 三节 + 中文注释 ≥7 点
- **新增** `docs/agents/cloud-sync-design.md`：12 章节 + ≥2 Mermaid + D1~D12 偏差表
- **新增 29 个单元测试**（src 内嵌 `#[cfg(test)]`：ES×9 + DS×7 + RQ×7 + INT×5 + PERF×1）
- 根 `Cargo.toml`：members 追加 `"crates/agents/cloud-sync"`（cloud-coordinator 之后）+ version 0.109.0 → 0.110.0；`Makefile`（VERSION + L3 头部注释）/ `ci.yml` L3 注释 / `gate.rs` 注释串尾 2 处同步
- **无 BREAKING**：纯新增 crate，既有 crate 零改动

## Impact

- Affected specs：develop-v11000-cloud-sync（新建）
- Affected code：`crates/agents/cloud-sync/`（新建）、`configs/`、`docs/agents/`、根 4 文件版本号
- 上游：v0.96.0 Cloud Coordinator（云端汇聚对端）、v0.101.0 断网处理（孤岛事件缓存语义对齐）、v0.109.0 故障录波（录波文件上传数据源）
- 下游：v0.111.0 模型 OTA（云边通道复用）

## ADDED Requirements

### Requirement: 事件溯源存储（event_store.rs）

The system SHALL provide `EventStore { events: Vec<Event>, base_offset, current_offset, max_events }`（字段私有）：`new(max_events)` 校验 >0（否则 `InvalidConfig`，D9）；`append(event_type, payload, now)` 计算 `checksum = crc32(payload)`、写入 `Event { offset: current_offset, timestamp: now, event_type, payload, checksum, synced: false }`，返回递增 offset；当 `len == max_events` 时先 `compact()`，仍满（全部未同步不可压缩）则返回 `Err(SyncError::StoreFull)`——宁可报错不静默丢数据（D9，蓝图 §7.1「断网后数据不丢」）；`get_unsynced(max)` 返回前 max 条未同步事件引用（offset 升序）；`mark_synced(up_to_offset)` 将 `offset <= up_to_offset` 的事件标记已同步（幂等，重复调用无状态变化）；`compact()` 移除最旧已同步事件、保留全部未同步 + 最近 100 条已同步，同步推进 `base_offset`；`len()` / `is_empty()` / `current_offset()` / `base_offset()` 访问器。`crc32(data)` 为 CRC32-IEEE（多项式 0xEDB88320，const 256 项表，D8），已知向量 `"123456789" → 0xCBF43926`；`Event::verify()` 校验 `crc32(payload) == checksum`。

#### Scenario: 追加与完整性（蓝图 §7.3）
- **WHEN** `append(EventType::Telemetry, b"p=100", 1000)` 连续 3 次
- **THEN** 返回 offset 0/1/2，各事件 timestamp==1000、`verify()` 为真；篡改 payload 后 `verify()` 为假

#### Scenario: 存储满压缩与 StoreFull（D9）
- **WHEN** max_events=4，append 4 条后 `mark_synced(1)`，再 append 2 条
- **THEN** 第 5 条触发 compact（移除 offset 0/1 已同步）后写入成功；若 4 条全未同步且已满，append 返回 `Err(StoreFull)` 且既有 4 条不丢

### Requirement: 增量同步与序列化（delta_sync.rs）

The system SHALL provide `DeltaSync { batch_size, compression, last_synced_offset, stats }`（字段私有）：`new(batch_size, compression)` 校验 batch_size>0 且 compression==None（否则 `InvalidConfig`，D5）；`build_batch(store, now)` 取前 batch_size 条未同步事件克隆组批——空则 `None`，否则 `SyncBatch { batch_id: from_offset, events, from_offset: first.offset, to_offset: last.offset, retry_count: 0, created_at: now }`（batch_id 用起始 offset，蓝图简化约定，云端据此幂等去重）；`serialize(batch)` 生成二进制帧（D11）：`[magic u16 LE = 0xC537][version u8 = 1][event_count u16 LE]` + 每事件 `[offset u64 LE][timestamp u64 LE][event_type u8（Telemetry=0…ConfigChange=5）][payload_len u32 LE][payload][checksum u32 LE]`；`sync_once(store, transport, queue, now)` 编排（D4）：build_batch → None 返回 `Ok(None)` → serialize → `transport.send_batch(&batch, &payload)`；成功返回 ack_offset → `store.mark_synced(ack)`、`last_synced_offset = ack`、`stats.total_sent += 1` → `Ok(Some(ack))`；失败 → `queue.enqueue(batch)`、`stats.total_retry_enqueued += 1` → 原样返回 `Err`（不标记任何事件）。`retry_once(store, transport, queue, now)`：`queue.retry_pending(now)` → None 返回 `Ok(None)` → serialize → send；成功同 sync_once 成功路径；失败 → batch `created_at = now`（重试时间基线，D10）后重新 enqueue → `Err`。`stats()` / `last_synced_offset()` 访问器（D12）。

#### Scenario: 批量构建（蓝图 §4.5）
- **WHEN** store 含 5 条未同步（offset 0~4）、batch_size=3
- **THEN** `build_batch` 返回 SyncBatch：batch_id==0、from_offset==0、to_offset==2、events.len()==3、retry_count==0

#### Scenario: 同步成功标记
- **WHEN** sync_once 且 transport 成功（ack=batch.to_offset）
- **THEN** store 中 offset ≤ 2 的事件 synced==true，last_synced_offset==2，total_sent==1

#### Scenario: 同步失败入队不丢（蓝图 §4.4）
- **WHEN** sync_once 且 transport 返回 TransportError
- **THEN** 批次入 RetryQueue、store 零标记、返回 `Err(TransportError)`，total_retry_enqueued==1

### Requirement: 重试队列（retry_queue.rs）

The system SHALL provide `RetryQueue { pending: VecDeque<SyncBatch>, max_retries, backoff_base_ms, dead_letters: VecDeque<SyncBatch>, dead_letter_count }`（字段私有，D10）：`new(max_retries, backoff_base_ms)`；`enqueue(batch)` 队尾压入；`retry_pending(now)` 仅检查队首——`retry_count >= max_retries` 则弹出压入死信队列（有界 8 批，超出丢最旧死信）并 `dead_letter_count += 1`，返回 None；`now - created_at >= calculate_backoff(retry_count)` 则弹出、`retry_count += 1` 返回 Some；否则 None；`calculate_backoff(retry_count)` = `min(base << retry_count, 300_000)` + 确定性抖动（xorshift32(retry_count×2654435761|1) mod (base+1)，D6：同 retry_count 同结果，区间 [exp, exp+base]）；`pending_len()` / `dead_letter_count()` / `dead_letters()` 访问器。生产路径零 `unwrap`（D10）。

#### Scenario: 指数退避（蓝图 §4.5）
- **WHEN** base=1000ms，retry_count=0/1/2/20
- **THEN** backoff 下界分别为 1000/2000/4000/300000（封顶 5 分钟），上界 = 下界 + 1000

#### Scenario: 超限死信（D10）
- **WHEN** max_retries=2，批次 retry_count 已达 2 时调用 retry_pending
- **THEN** 批次移入死信队列、dead_letter_count==1、返回 None；死信队列满 8 批后再入新死信丢弃最旧

### Requirement: 传输抽象与断网补传集成（lib.rs）

The system SHALL provide `SyncTransport` trait（sync，no_std 单线程惯例，不要求 Send+Sync，D4）：`send_batch(&mut self, batch: &SyncBatch, payload: &[u8]) -> Result<u64, SyncError>` 返回云端确认的 ack_offset；`MockSyncTransport { fail_times, sent }`（D4，v0.95.0 MockCloudChannel 先例）：fail_times>0 时递减并返回 `Err(TransportError)`，否则记录 payload 克隆并返回 `Ok(batch.to_offset)`。集成语义（蓝图 §4.3 时序图重绘）：断网期间 append 持续累积 → sync_once 失败批次入重试队列 → 网络恢复后 retry_once/sync_once 补传 → 全部事件 mark_synced，零丢失（§7.1 验收）；`mark_synced` 幂等 + batch_id=from_offset 支撑重发幂等（§6.4）；`SyncStats` 提供可观测（§9）。

#### Scenario: 断网→补传→数据不丢（蓝图 §6.2 集成测试）
- **WHEN** store append 5 条、batch_size=3、transport 注入 1 次失败：sync_once（失败入队）→ retry_once(now+退避)（成功）→ sync_once（成功）
- **THEN** `get_unsynced(10)` 为空、last_synced_offset==4、5 条事件全部 synced、零丢失

#### Scenario: 幂等重发（蓝图 §6.4/§8.5）
- **WHEN** 同一批次网络模糊失败后重发成功，mark_synced(2) 被调用两次
- **THEN** store 状态与单次调用一致（幂等），无重复副作用

#### Scenario: 批量同步性能（蓝图 §6.3/§7.2）
- **WHEN** 1000 事件 build_batch + serialize + mock send（cfg(test) Instant 口径，D12）
- **THEN** 总耗时 < 2000ms

## MODIFIED Requirements

无（纯新增 crate，既有 crate 零改动）。

## REMOVED Requirements

无。

## 偏差声明（D1~D12，相对蓝图 §3/§4/§5/§6）

| 编号 | 偏差 | 理由 |
|------|------|------|
| **D1** | 蓝图 `crates/cloud_sync/` → `crates/agents/cloud-sync/`（eneros-cloud-sync） | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；云边同步与 v0.95.0 cloud-coordinator / v0.96.0 云端汇聚同属 agents 子系统 |
| **D2** | 蓝图 `docs/phase2/cloud_sync.md` → `docs/agents/cloud-sync-design.md` | 记忆 §2.3.3 强制：文档按方向分类（cloud-aggregation-design.md 同目录先例） |
| **D3** | 蓝图 `tests/sync_retry.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.109.0 项目惯例，不新增 tests/ 文件 |
| **D4** | 蓝图 `async send_batch` + `HttpClient` → `SyncTransport` sync trait（`send_batch(batch, payload) -> Result<u64, SyncError>` 返回 ack_offset）+ `MockSyncTransport`（fail_times 故障注入 + sent 记录，置于 lib.rs）；`endpoint` 字段移出 `DeltaSync`（真实 transport 实现承载） | no_std 无 async runtime/无 std::net（v0.95.0 D3/D8 CloudChannel、v0.106.0 D4 MmsTransport 同先例）；主机可测；真实 HTTP/gRPC 适配器在集成层注入 |
| **D5** | `CompressionType` 保留 None/Snappy/Gzip 3 变体（对齐蓝图数据结构），但本版仅 None 可构造——`DeltaSync::new` 遇 Snappy/Gzip 返回 `InvalidConfig` | 零第三方依赖约束；记忆 §5.5 集成清单未列入 no_std 压缩库（snappy/gzip 无 no_std 成熟实现）；压缩为传输层可选增强，后续版本按需引入 |
| **D6** | 蓝图 `rand::thread_rng().gen_range(0..=base)` 抖动 → 确定性 xorshift32 抖动：`xorshift32(retry_count×2654435761\|1) mod (base+1)` | `rand` 为 std 专用，no_std 不可用；确定性抖动同 retry_count 同结果，测试可断言，零依赖零状态 |
| **D7** | 蓝图 `current_time_ms()` 全局时间函数 → `now: u64` 参数注入（append/build_batch/retry_pending/sync_once/retry_once） | no_std 无系统时间（v0.108.0 D9 KeyMgmt / v0.109.0 D11 同先例）；集成层由 v0.12.0 RTC 供给 |
| **D8** | 蓝图 `crc32_checksum` 未定义 → 自实现 CRC32-IEEE（多项式 0xEDB88320，const 256 项表，纯 core 零依赖）+ `Event::verify()` | 蓝图 §7.3 要求事件完整性 CRC32；eneros-crypto 无 CRC32 实现（SM 系列不含）；表驱动 ~30 行成熟算法不属重复造轮子 |
| **D9** | 蓝图 `append` 返回 u64 → `Result<u64, SyncError>`：存储满且 compact 后仍无可压缩已同步事件 → `Err(StoreFull)`；`EventStore::new` 校验 max_events>0 → `InvalidConfig` | 蓝图 append 在「全未同步且已满」时静默越界增长或丢数据，违反 §7.1「断网后数据不丢」；显式错误让上游（RTOS 采样侧）感知背压 |
| **D10** | ① 超 max_retries 直接丢弃 → 有界死信队列（容量 8 批，溢出丢最旧死信）+ `dead_letter_count` 统计；② 重试失败重入队时 `created_at` 更新为失败时刻（重试时间基线）；③ `SyncBatch` 归 delta_sync.rs（build_batch 产出地）；④ `retry_pending` 蓝图 `pop_front().unwrap()` 改 if let（生产零 unwrap） | ① 蓝图注释「进入死信队列」但无实现，丢弃即数据丢失；② 蓝图用原始 created_at 判定退避，长断网后全部批次立即到期、退避失效形成重试风暴；③ 内聚；④ 记忆 §4.3 no_std 合规 |
| **D11** | 蓝图 `serialize_batch` / `snappy_compress` 未定义 → 自定义二进制帧：`[magic u16 LE=0xC537][version u8=1][event_count u16 LE]` + 每事件 `[offset u64][timestamp u64][event_type u8][payload_len u32][payload][checksum u32]`（全 LE） | 零第三方依赖（serde/postcard 不入仓）；magic+version 支撑云端 API 版本演进（蓝图 §8.4）；帧内含 per-event CRC32 支撑 §4.4 校验和不匹配重发 |
| **D12** | 错误模型 `SyncError` = StoreFull / TransportError / ServerError(u16) / ChecksumMismatch / InvalidConfig（5 变体，Debug/Clone/Copy/PartialEq）；新增 `SyncStats { total_sent, total_retry_enqueued, total_dead_letter, last_synced_offset }`（Debug/Clone/Copy/PartialEq）落地蓝图 §9 可观测要求；性能「1000 事件 < 2s」落地为 cfg(test) Instant 主机断言 | 蓝图引用 SyncError 未定义；变体覆盖 §4.4 各失败面（对齐 v0.95.0 CloudError Copy 惯例）；性能口径与 v0.109.0 D12 一致（真实网络时延为实验室项） |

## 接口契约

```rust
// ============ crates/agents/cloud-sync/src/lib.rs ============
pub enum SyncError {                            // Debug/Clone/Copy/PartialEq（D12）
    StoreFull, TransportError, ServerError(u16), ChecksumMismatch, InvalidConfig,
}

pub trait SyncTransport {                       // D4（sync，no_std 单线程惯例，不要求 Send+Sync）
    /// 发送序列化批次；成功返回云端确认的 ack_offset（语义 ≤ batch.to_offset）。
    fn send_batch(&mut self, batch: &SyncBatch, payload: &[u8]) -> Result<u64, SyncError>;
}
pub struct MockSyncTransport {                  // D4（v0.95.0 MockCloudChannel 先例）
    pub fail_times: u32,                        // >0 时递减并返回 Err(TransportError)
    pub sent: Vec<Vec<u8>>,                     // 已成功发送的 payload 记录
}
impl MockSyncTransport {
    pub fn new() -> Self;
    pub fn with_fail_times(fail_times: u32) -> Self;
}

// ============ src/event_store.rs ============
pub enum EventType {                            // Debug/Clone/Copy/PartialEq
    Telemetry, Status, Alarm, ControlLog, TradeRecord, ConfigChange,
}
pub struct Event {                              // Debug/Clone/PartialEq
    pub offset: u64, pub timestamp: u64, pub event_type: EventType,
    pub payload: Vec<u8>, pub checksum: u32, pub synced: bool,
}
impl Event {
    pub fn verify(&self) -> bool;               // crc32(payload) == checksum（D8）
}
pub fn crc32(data: &[u8]) -> u32;               // CRC32-IEEE const 表（D8）

pub struct EventStore { /* events/base_offset/current_offset/max_events 私有 */ }
impl EventStore {
    pub fn new(max_events: usize) -> Result<Self, SyncError>;   // ==0 → InvalidConfig（D9）
    pub fn append(&mut self, event_type: EventType, payload: &[u8], now: u64)
        -> Result<u64, SyncError>;              // 满且不可压缩 → StoreFull（D7/D9）
    pub fn get_unsynced(&self, max: usize) -> Vec<&Event>;      // offset 升序
    pub fn mark_synced(&mut self, up_to_offset: u64);           // 幂等
    pub fn compact(&mut self);                  // 保留未同步 + 最近 100 条已同步
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn current_offset(&self) -> u64;
    pub fn base_offset(&self) -> u64;
}

// ============ src/delta_sync.rs ============
pub enum CompressionType { None, Snappy, Gzip } // Debug/Clone/Copy/PartialEq（D5：仅 None 可构造）
pub struct SyncBatch {                          // Debug/Clone/PartialEq（D10 归属本模块）
    pub batch_id: u64, pub events: Vec<Event>,
    pub from_offset: u64, pub to_offset: u64,
    pub retry_count: u32, pub created_at: u64,
}
pub struct SyncStats {                          // Debug/Clone/Copy/PartialEq（D12）
    pub total_sent: u64, pub total_retry_enqueued: u64,
    pub total_dead_letter: u64, pub last_synced_offset: u64,
}
pub struct DeltaSync { /* batch_size/compression/last_synced_offset/stats 私有（D4 endpoint 移出） */ }
impl DeltaSync {
    pub fn new(batch_size: usize, compression: CompressionType) -> Result<Self, SyncError>;  // D5
    pub fn build_batch(&self, store: &EventStore, now: u64) -> Option<SyncBatch>;
    pub fn serialize(batch: &SyncBatch) -> Vec<u8>;             // D11 二进制帧
    pub fn sync_once<S: SyncTransport>(&mut self, store: &mut EventStore,
        transport: &mut S, queue: &mut RetryQueue, now: u64) -> Result<Option<u64>, SyncError>;
    pub fn retry_once<S: SyncTransport>(&mut self, store: &mut EventStore,
        transport: &mut S, queue: &mut RetryQueue, now: u64) -> Result<Option<u64>, SyncError>;
    pub fn stats(&self) -> &SyncStats;
    pub fn last_synced_offset(&self) -> u64;
}

// ============ src/retry_queue.rs ============
pub struct RetryQueue { /* pending/max_retries/backoff_base_ms/dead_letters/dead_letter_count 私有（D10） */ }
impl RetryQueue {
    pub fn new(max_retries: u32, backoff_base_ms: u64) -> Self;
    pub fn enqueue(&mut self, batch: SyncBatch);
    pub fn retry_pending(&mut self, now: u64) -> Option<SyncBatch>;   // D7/D10
    pub fn calculate_backoff(&self, retry_count: u32) -> u64;         // 指数封顶 300s + 确定性抖动（D6）
    pub fn pending_len(&self) -> usize;
    pub fn dead_letter_count(&self) -> u64;
    pub fn dead_letters(&self) -> &VecDeque<SyncBatch>;
}
```

## 测试规划（29 个，src 内嵌 #[cfg(test)]）

| 组 | 编号 | 覆盖点 |
|----|------|--------|
| event_store | ES1~ES9 | append 递增 offset+timestamp 注入 / checksum+verify（含篡改）/ get_unsynced 过滤+take(max) / mark_synced 幂等 / compact 保留未同步+最近 100 已同步+base_offset 推进 / 满自动 compact 后写入 / 全未同步满 → StoreFull 不丢 / new(0) → InvalidConfig / crc32 已知向量（"123456789"→0xCBF43926） |
| delta_sync | DS10~DS16 | build_batch 空 → None / build_batch 语义（batch_id=from、to=last、retry_count=0、created_at=now）/ new 校验（batch_size==0、Snappy/Gzip → InvalidConfig）/ serialize 帧布局（magic 0xC537+version+count+事件 TLV+CRC）/ sync_once 成功 mark_synced+stats / sync_once 失败入队不标记 / retry_once 成功 + 失败重入队 created_at 更新 |
| retry_queue | RQ17~RQ23 | enqueue+退避未到 → None / 退避到 → 出队 retry_count+1 / 超 max_retries → 死信+计数 / 死信有界 8 丢最旧 / 指数退避 1/2/4s…封顶 300s / 抖动确定性（同 retry_count 同结果）+区间 [exp, exp+base] / max_retries=0 → 立即死信 |
| 集成 | INT24~INT28 | 断网→补传→数据不丢（5 事件+1 次失败注入全流程）/ 幂等重发（mark_synced 重复调用状态一致）/ 混合 6 类事件类型 / 长断网存储满 → StoreFull 且不丢已有 / mock sent payload 可解析回放（帧头+事件数+首事件 offset） |
| perf | PERF29 | 1000 事件 build_batch+serialize+mock send < 2000ms（cfg(test) Instant，D12） |

## 内存预算声明（记忆 §5.6）

事件存储默认 max_events=10000，单事件平均 payload 256B + 结构开销约 40B → 约 2.9MB；重试队列/死信队列按批次复用同一份事件克隆，峰值 ≤ 2× 单批体积。整体归入 Agent Runtime 分区（≤64MB，蓝图 §43.6），OOM 余量充足。
