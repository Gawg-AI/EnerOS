# Tasks — v0.110.0 云边数据同步

> Spec：`spec.md`（develop-v11000-cloud-sync）。T1→T2→T3 顺序（T2 消费 T1 的 SyncError/Event 基型；T3 消费 T2 的 SyncBatch）；T4 依赖 T1~T3（编排方法 + 传输抽象消费全部模块）；T5/T6 顺序收尾。

- [x] **T1：新建 cloud-sync crate 骨架 + lib.rs 基座 + event_store.rs — 事件溯源存储与 CRC32**
  - [x] 1.1 `crates/agents/cloud-sync/Cargo.toml`：`eneros-cloud-sync`，workspace 继承，零依赖（D1）
  - [x] 1.2 `src/lib.rs`：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 模块声明（event_store/delta_sync/retry_queue）+ 重导出 + `SyncError`（5 变体：StoreFull/TransportError/ServerError(u16)/ChecksumMismatch/InvalidConfig，D12，derive Debug/Clone/Copy/PartialEq）+ crate 文档（版本定位 + 核心类型清单 + D1~D12 偏差表 + no_std 合规声明，风格对齐 cloud-coordinator/fault-recorder）
  - [x] 1.3 `src/event_store.rs`：`crc32(data)`（CRC32-IEEE 0xEDB88320 const 256 表，D8）+ `EventType`（6 变体，derive Debug/Clone/Copy/PartialEq）+ `Event`（6 pub 字段 + `verify()`，derive Debug/Clone/PartialEq）+ `EventStore`（events/base_offset/current_offset/max_events 私有）：`new(max_events)`（==0 → InvalidConfig，D9）/ `append(event_type, payload, now)`（满先 compact 仍满 → StoreFull，D7/D9）/ `get_unsynced(max)` / `mark_synced(up_to_offset)`（幂等）/ `compact()`（保留未同步 + 最近 100 条已同步，推进 base_offset）/ `len()` / `is_empty()` / `current_offset()` / `base_offset()`
  - [x] 1.4 测试 ES1~ES9（9 个，见 spec 测试规划表）
  - 验证：`cargo test -p eneros-cloud-sync event_store::` 9/9 全过 ✅

- [x] **T2：delta_sync.rs — 增量同步批次与二进制序列化**
  - [x] 2.1 `src/delta_sync.rs`：`CompressionType`（3 变体，derive Debug/Clone/Copy/PartialEq）+ `SyncBatch`（6 pub 字段，derive Debug/Clone/PartialEq，D10 归属本模块）+ `SyncStats`（4 pub 字段，derive Debug/Clone/Copy/PartialEq，D12）+ `DeltaSync`（batch_size/compression/last_synced_offset/stats 私有，D4 endpoint 移出）
  - [x] 2.2 `new(batch_size, compression)`（batch_size==0 或 compression!=None → InvalidConfig，D5）；`build_batch(store, now)`（空 → None；否则 batch_id=from_offset、to_offset=last.offset、retry_count=0、created_at=now）；`serialize(batch)`（D11 帧：`magic u16 LE=0xC537 + version u8=1 + event_count u16 LE` + 每事件 `offset u64 + timestamp u64 + event_type u8 + payload_len u32 + payload + checksum u32` 全 LE）
  - [x] 2.3 测试 DS10~DS13（4 个：build_batch 空 None / build_batch 语义 / new 校验 / serialize 帧布局，见 spec 测试规划表）
  - 验证：`cargo test -p eneros-cloud-sync delta_sync::` 4/4 全过 ✅

- [x] **T3：retry_queue.rs — 指数退避重试队列与死信**
  - [x] 3.1 `src/retry_queue.rs`：`RetryQueue`（pending: VecDeque<SyncBatch>/max_retries/backoff_base_ms/dead_letters: VecDeque<SyncBatch>/dead_letter_count 私有，D10）：`new(max_retries, backoff_base_ms)` / `enqueue(batch)` / `retry_pending(now)`（队首 retry_count >= max_retries → 移死信（有界 8，丢最旧）+计数 → None；now-created_at >= backoff → 弹出 retry_count+1 → Some；否则 None；生产零 unwrap，D10）/ `calculate_backoff(retry_count)`（min(base<<retry_count, 300_000) + xorshift32 确定性抖动 ∈ [0, base]，D6）/ `pending_len()` / `dead_letter_count()` / `dead_letters()`
  - [x] 3.2 测试 RQ17~RQ23（7 个，见 spec 测试规划表）
  - 验证：`cargo test -p eneros-cloud-sync retry_queue::` 7/7 全过 ✅

- [x] **T4：lib.rs SyncTransport/Mock + DeltaSync 编排方法 + 集成测试 — 断网补传闭环**
  - [x] 4.1 lib.rs 追加 `SyncTransport` trait（`send_batch(&mut self, batch: &SyncBatch, payload: &[u8]) -> Result<u64, SyncError>`，sync 无 Send+Sync bound，D4）+ `MockSyncTransport { fail_times, sent }`（new/with_fail_times；fail_times>0 递减返回 TransportError，否则 sent 记录 payload 克隆返回 Ok(batch.to_offset)，D4）
  - [x] 4.2 `delta_sync.rs` 追加编排方法：`sync_once(store, transport, queue, now)`（build → None → Ok(None) → serialize → send：成功 mark_synced(ack)+last_synced_offset=ack+total_sent+=1 → Ok(Some(ack))；失败 enqueue+total_retry_enqueued+=1 → Err 原样返回不标记）+ `retry_once(store, transport, queue, now)`（retry_pending → None → Ok(None) → send：成功同 sync_once；失败 batch.created_at=now 重入队 → Err，D10）
  - [x] 4.3 测试 DS14~DS16（sync_once 成功 / sync_once 失败入队 / retry_once 成功+失败重入队 created_at 更新）+ INT24~INT28（断网→补传→数据不丢 / 幂等重发 / 混合 6 类事件 / 长断网 StoreFull 不丢已有 / mock payload 可解析回放）+ PERF29（1000 事件 build+serialize+mock send < 2000ms，`std::time::Instant` 仅 cfg(test)，D12）
  - 验证：`cargo test -p eneros-cloud-sync` 29/29 全过 ✅

- [x] **T5：workspace 接线 + 配置 + 设计文档**
  - [x] 5.1 根 `Cargo.toml` members 追加 `"crates/agents/cloud-sync"`（agents 段 cloud-coordinator 之后）
  - [x] 5.2 `configs/cloud-sync.toml`：`[event_store]` max_events=10000 + `[delta_sync]` batch_size/compression + `[retry_queue]` max_retries=10/backoff_base_ms=1000/dead_letter_capacity=8 + 中文注释 ≥7 点（事件溯源选型 §5.1 / SyncTransport 抽象 D4 / 断网补传策略 §4.4 / 指数退避+确定性抖动 D6 / 性能口径 1000 事件<2s D12 / 内存预算 max_events×(payload+40)B 记忆 §5.6 / GPU 不适用 §6.6 / 下游 v0.111.0 模型 OTA 复用通道）
  - [x] 5.3 `docs/agents/cloud-sync-design.md`：12 章节 + ≥2 Mermaid（蓝图 §4.3 断网补传时序图重绘 + Event 状态迁移图 unsynced→synced→compacted/dead_letter）+ D1~D12 偏差表（与 spec.md 逐字一致）+ 性能口径声明（D12）+ 内存预算声明
  - 验证：`cargo metadata` 解析成功；crate 测试全过（D1~D12 表自动化比对零差异）✅

- [x] **T6：版本同步 0.110.0 + 全量构建验证 + checklist 核验收工**
  - [x] 6.1 根 `Cargo.toml` version = "0.110.0"；`Makefile` VERSION + L3 头部注释；`ci.yml` L3 注释；`gate.rs` 注释串尾 2 处追加 v0.110.0 类型清单（11 类型：EventStore/Event/EventType/DeltaSync/CompressionType/RetryQueue/SyncBatch/SyncTransport/MockSyncTransport/SyncError/SyncStats 按实际定稿）
  - [x] 6.2 §2.4.2 构建校验：C6 metadata / C7 本 crate 29 测试 + 全 workspace 回归（含 eneros-cloud-coordinator 零改动回归）/ C8 aarch64 交叉编译 / C9 fmt / C10 clippy -D warnings / C11 cargo deny
  - [x] 6.3 `checklist.md` 逐项核验勾选 + 验收记录
  - 验证：C6~C11 全绿，checklist 全勾 + 验收记录已填，收工

# Task Dependencies

- T1 先行（T2 依赖 lib.rs 的 SyncError/模块声明与 Event 基型）
- T2 depends on T1；T3 depends on T2（RetryQueue 消费 SyncBatch）
- T4 depends on T1 + T2 + T3（SyncTransport/编排方法消费全部模块）
- T5 depends on T4（文档需最终代码签名）
- T6 depends on T5
