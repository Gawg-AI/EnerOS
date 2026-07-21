# Checklist — v0.110.0 云边数据同步

> 逐项核验后勾选。分组：A 蓝图合规 / B 目录结构 / C crate 骨架 no_std / D event_store.rs / E delta_sync.rs / F retry_queue.rs / G 传输抽象与集成 / H 配置与文档 / I 版本同步与构建验证。

## A. 蓝图合规与 spec 对齐（C1~C10）

- [x] C1: 交付物对齐蓝图 §3：event_store.rs / delta_sync.rs / retry_queue.rs 三模块 + EventStore/DeltaSync/RetryQueue 接口齐全
- [x] C2: 接口对齐 spec 接口契约：`EventStore` 含 new/append/get_unsynced/mark_synced/compact/len/is_empty/current_offset/base_offset；`DeltaSync` 含 new/build_batch/serialize/sync_once/retry_once/stats/last_synced_offset；`RetryQueue` 含 new/enqueue/retry_pending/calculate_backoff/pending_len/dead_letter_count/dead_letters
- [x] C3: 数据结构对齐 spec：Event/EventType/SyncBatch/SyncStats 字段一致
- [x] C4: `SyncError` 5 变体齐全（StoreFull/TransportError/ServerError(u16)/ChecksumMismatch/InvalidConfig，D12）
- [x] C5: `EventType` 6 变体齐全（Telemetry/Status/Alarm/ControlLog/TradeRecord/ConfigChange）
- [x] C6: `SyncBatch.batch_id == from_offset`（蓝图简化约定，云端幂等去重键）
- [x] C7: 断网补传链路对齐蓝图 §4.3 时序：失败入 retry → 退避 → 恢复重发 → mark_synced
- [x] C8: CRC32-IEEE 完整性（蓝图 §7.3）：已知向量 `"123456789" → 0xCBF43926`；`Event::verify()` 篡改可检出
- [x] C9: `SyncTransport` trait + `MockSyncTransport` 存在（D4），零 `std::net`、零 async、零 HttpClient
- [x] C10: spec.md D1~D12 偏差表与 lib.rs crate 文档偏差表、设计文档偏差表逐字一致

## B. 目录结构（C11~C16，记忆 §2.4.1）

- [x] C11: crate 位于 `crates/agents/cloud-sync/`，未放根目录（D1）
- [x] C12: 根 `Cargo.toml` members 已追加 `"crates/agents/cloud-sync"`（cloud-coordinator 之后）
- [x] C13: `Cargo.toml` 零第三方依赖（无 path 引用需求）；package 名 `eneros-cloud-sync`
- [x] C14: 文档位于 `docs/agents/cloud-sync-design.md`，未平面化放 docs/ 根（D2）
- [x] C15: 测试全部 src 内嵌 `#[cfg(test)]`，未新增 tests/ 文件（D3）
- [x] C16: `cargo metadata --format-version 1` 解析成功（exit=0）

## C. crate 骨架与 no_std（C17~C22）

- [x] C17: lib.rs 顶部 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`；子模块不重复 no_std 声明
- [x] C18: 全 crate 零 `std::*` 引用（仅 `alloc::*`/`core::*`；Instant 仅 cfg(test) 内）
- [x] C19: 零 `panic!`/`todo!`/`unimplemented!`（生产路径）；零 `unwrap()` 于生产路径（D10）
- [x] C20: 零第三方依赖；零 unsafe；零 extern "C"
- [x] C21: `cargo build -p eneros-cloud-sync --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C22: lib.rs crate 文档含版本定位 + 核心类型清单 + D1~D12 偏差表 + no_std 合规声明（风格对齐 cloud-coordinator）

## D. event_store.rs（C23~C31）

- [x] C23: `Event` 6 个 pub 字段（offset/timestamp/event_type/payload/checksum/synced），derive Debug/Clone/PartialEq；`EventType` derive Debug/Clone/Copy/PartialEq
- [x] C24: `append` 返回递增 offset（0 起），timestamp == 注入 now（D7）
- [x] C25: `checksum == crc32(payload)`；`verify()` 正常为真、篡改 payload 为假（D8）
- [x] C26: `get_unsynced(max)` 仅返回未同步、按 offset 升序、最多 max 条
- [x] C27: `mark_synced(up_to_offset)` 标记 offset ≤ 参数者；重复调用幂等（状态不再变化）
- [x] C28: `compact()` 保留全部未同步 + 最近 100 条已同步，base_offset 正确推进
- [x] C29: 存储满时 append 先 compact 后写入；全未同步且已满 → `Err(StoreFull)` 且既有事件零丢失（D9）
- [x] C30: `crc32(b"123456789") == 0xCBF43926`（CRC32-IEEE 已知向量）；`new(0)` → `InvalidConfig`（D9）
- [x] C31: 测试 ES1~ES9 共 9 个全部通过

## E. delta_sync.rs（C32~C43）

- [x] C32: `CompressionType` 3 变体（None/Snappy/Gzip，蓝图对齐），derive Debug/Clone/Copy/PartialEq（D5）
- [x] C33: `new` 校验：batch_size==0 → InvalidConfig；compression != None → InvalidConfig（D5）
- [x] C34: `build_batch` 无未同步事件 → None
- [x] C35: `build_batch` 语义：batch_id==from_offset、to_offset==末事件 offset、retry_count==0、created_at==now、events 为克隆且 ≤ batch_size
- [x] C36: `serialize` 帧布局：magic u16 LE 0xC537 + version u8 1 + event_count u16 LE + 每事件 offset/timestamp u64 + event_type u8 + payload_len u32 + payload + checksum u32（D11）
- [x] C37: `sync_once` 成功：transport 返回 ack → mark_synced(ack) + last_synced_offset==ack + total_sent+=1 → Ok(Some(ack))
- [x] C38: `sync_once` 失败：批次入 RetryQueue、store 零标记、total_retry_enqueued+=1、Err 原样返回
- [x] C39: `sync_once` 空存储 → Ok(None)，transport 零调用
- [x] C40: `retry_once` 退避未到 → Ok(None)；退避到且成功 → mark_synced + stats 更新
- [x] C41: `retry_once` 失败：batch.created_at 更新为 now（重试时间基线，D10）后重入队，Err 返回
- [x] C42: `stats()` 四项累计正确（total_sent/total_retry_enqueued/total_dead_letter/last_synced_offset，D12）
- [x] C43: 测试 DS10~DS16 共 7 个全部通过

## F. retry_queue.rs（C44~C52）

- [x] C44: `new(max_retries, backoff_base_ms)` 创建；字段私有（D10）
- [x] C45: `enqueue` 队尾压入，`pending_len()` 正确
- [x] C46: `retry_pending` 队首 `now - created_at < backoff` → None（批次留队）
- [x] C47: `retry_pending` 退避到 → 弹出且 retry_count+1 返回 Some
- [x] C48: `retry_pending` 队首 retry_count >= max_retries → 移入死信队列 + dead_letter_count+=1 → None；死信有界 8，溢出丢最旧死信（D10）
- [x] C49: `calculate_backoff` 指数递增（base=1000：rc=0/1/2 → ≥1000/2000/4000），rc=20 → 下界 300000（封顶 5 分钟）
- [x] C50: 抖动确定性：同 retry_count 两次调用结果相同；结果 ∈ [exp, exp+base]（D6）
- [x] C51: max_retries=0 → 首次 retry_pending 即死信；生产路径零 unwrap（if let 替代，D10）
- [x] C52: 测试 RQ17~RQ23 共 7 个全部通过

## G. 传输抽象与集成（C53~C60）

- [x] C53: `SyncTransport` 为 sync trait，无 Send+Sync bound（D4，v0.95.0 CloudChannel 惯例）
- [x] C54: `MockSyncTransport`：fail_times>0 递减返回 TransportError；成功时 sent 记录 payload 克隆、返回 Ok(batch.to_offset)
- [x] C55: INT24 断网→补传→数据不丢：5 事件 + 1 次失败注入全流程后 `get_unsynced(10)` 为空、last_synced_offset==4
- [x] C56: INT25 幂等重发：mark_synced 重复调用状态一致，无重复副作用（蓝图 §6.4/§8.5）
- [x] C57: INT26 混合 6 类事件类型全流程同步成功
- [x] C58: INT27 长断网存储满 → StoreFull 且既有事件零丢失（蓝图 §7.1）
- [x] C59: INT28 mock sent payload 可解析回放：帧头 magic/version/count 正确、首事件 offset 正确（D11）
- [x] C60: PERF29 1000 事件 build+serialize+mock send < 2000ms（cfg(test) Instant 断言，D12）

## H. 配置与文档（C61~C66）

- [x] C61: `configs/cloud-sync.toml` 存在，`[event_store]` + `[delta_sync]` + `[retry_queue]` 节齐全 + 中文注释 ≥7 点
- [x] C62: 配置中文注释覆盖：事件溯源选型 §5.1 / SyncTransport 抽象 D4 / 断网补传策略 §4.4 / 指数退避+确定性抖动 D6 / 性能口径 D12 / 内存预算 / GPU 不适用 / 下游 v0.111.0
- [x] C63: `docs/agents/cloud-sync-design.md` 存在，12 章节齐全
- [x] C64: 文档含 ≥2 个 Mermaid 图：断网补传时序图（蓝图 §4.3 重绘）+ Event 状态迁移图
- [x] C65: 文档含 D1~D12 偏差表，与 spec.md 逐字一致
- [x] C66: 文档含性能口径声明（D12）+ 内存预算声明（记忆 §5.6）

## I. 版本同步与构建验证（C67~C80）

- [x] C67: 根 `Cargo.toml` version == "0.110.0"
- [x] C68: `Makefile` VERSION == 0.110.0 且 L3 头部注释同步
- [x] C69: `ci.yml` L3 版本注释 == v0.110.0
- [x] C70: `gate.rs` 注释串尾 2 处追加 v0.110.0 类型清单（11 类型）
- [x] C71: `cargo test -p eneros-cloud-sync` 29/29 通过
- [x] C72: eneros-cloud-coordinator 回归通过（零改动验证）
- [x] C73: 全 workspace 回归通过（cargo test --workspace --exclude eneros-kernel --exclude eneros-hello，exit=0，零回归）
- [x] C74: `cargo build -p eneros-cloud-sync --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C75: `cargo fmt --all -- --check` 通过
- [x] C76: `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 0 warning
- [x] C77: `cargo deny check advisories licenses bans sources` 通过（零新增第三方依赖）
- [x] C78: `git status` 无 target/elf/bin/dtb/IDE 缓存被追踪
- [x] C79: spec.md / tasks.md / checklist.md 三件齐全且内容一致；tasks.md 全部复选框已勾选；无超范围交付（Karpathy Simplicity First）
- [x] C80: 内存预算声明已落地文档（max_events×(payload+40)B，Agent Runtime ≤64MB 分区，蓝图 §43.6）

## 验收记录

- **核验日期**：2026-07-20
- **核验人**：Trae Agent
- **通过项数**：80/80
- **核验方式**：
  - C1~C5/C23~C28/C32~C36/C44~C48/C53~C54：源码审阅（lib.rs / event_store.rs / delta_sync.rs / retry_queue.rs 结构与接口签名逐项比对）
  - C6~C9/C24~C31/C37~C43/C49~C52/C55~C60：`cargo test -p eneros-cloud-sync` 29/29 通过（ES1~ES9 ×9 + DS10~DS16 ×7 + RQ17~RQ23 ×7 + INT24~INT28 ×5 + PERF29 ×1）
  - C10/C65：spec.md / lib.rs crate 文档 / cloud-sync-design.md §11 三处 D1~D12 偏差表逐字一致（已比对）
  - C11~C15/C17~C20：目录结构 + no_std 合规审阅（零 std:: 生产引用、零 panic!/todo!/unimplemented!、零 unsafe、零 extern "C"、零第三方依赖、生产路径零 unwrap）
  - C16：`cargo metadata --format-version 1` exit=0
  - C21/C74：`cargo build -p eneros-cloud-sync --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` exit=0
  - C61~C64/C66/C80：configs/cloud-sync.toml（3 节 + 9 注释点）与 docs/agents/cloud-sync-design.md（12 章节 + 3 Mermaid + 内存预算/性能口径声明）审阅
  - C67~C70：根 Cargo.toml version=0.110.0 / Makefile VERSION=0.110.0 + L3 注释 / ci.yml L3 注释 / gate.rs L144+L233 类型清单审阅
  - C71~C73：29/29 + eneros-cloud-coordinator 回归 + 全 workspace 回归（--exclude kernel/hello）exit=0
  - C75：`cargo fmt --all -- --check` exit=0
  - C76：`cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` exit=0（0 warning）
  - C77：`cargo deny check advisories licenses bans sources` exit=0
  - C78：`git status --porcelain` 过滤 target/elf/bin/dtb/.idea/.vscode 零命中
  - C79：spec/tasks/checklist 三件齐全；tasks.md T1~T6 全勾；交付物未超 spec 范围
