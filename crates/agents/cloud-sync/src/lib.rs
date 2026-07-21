//! EnerOS v0.110.0 云边数据同步（P2-H 第 2 版）.
//!
//! 边缘侧遥测/状态/告警/控制日志/交易/配置六类事件必须可靠汇聚到云端 TSDB，支撑
//! 全局分析与审计。断网期间事件不得丢失，网络恢复后需增量补传。本 crate 在 v0.96.0
//! 云端数据汇聚、v0.101.0 断网处理（EventCache/RecoverySync）、v0.109.0 COMTRADE
//! 录波文件基座上，实现事件溯源存储 + 增量批量同步 + 指数退避重试队列，打通
//! 「事件 → 存储 → 同步 → 补传」链路，为 v0.111.0 模型 OTA 提供云边通道基础。
//!
//! # 核心类型
//!
//! - [`EventStore`] / [`Event`] / [`EventType`] — 事件溯源存储（容量有界、满则
//!   compact 已同步、CRC32 完整性，D8/D9）
//! - [`crc32`] — CRC32-IEEE 校验和（const 表，D8）
//! - [`DeltaSync`] / [`SyncBatch`] / [`SyncStats`] / [`CompressionType`] — 增量
//!   批量同步 + 二进制帧序列化（D11）+ `sync_once`/`retry_once` 编排
//! - [`RetryQueue`] — 指数退避重试队列（封顶 300s + 确定性 xorshift32 抖动，D6；
//!   有界死信队列，D10）
//! - [`SyncTransport`] / [`MockSyncTransport`] — 传输抽象 + mock 实现（D4）
//! - [`SyncError`] — 错误枚举（StoreFull / TransportError / ServerError /
//!   ChecksumMismatch / InvalidConfig，D12）
//!
//! # 偏差声明（D1~D12，相对蓝图 §3/§4/§5/§6）
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | **D1** | 蓝图 `crates/cloud_sync/` → `crates/agents/cloud-sync/`（eneros-cloud-sync） | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；云边同步与 v0.95.0 cloud-coordinator / v0.96.0 云端汇聚同属 agents 子系统 |
//! | **D2** | 蓝图 `docs/phase2/cloud_sync.md` → `docs/agents/cloud-sync-design.md` | 记忆 §2.3.3 强制：文档按方向分类（cloud-aggregation-design.md 同目录先例） |
//! | **D3** | 蓝图 `tests/sync_retry.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.109.0 项目惯例，不新增 tests/ 文件 |
//! | **D4** | 蓝图 `async send_batch` + `HttpClient` → `SyncTransport` sync trait（`send_batch(batch, payload) -> Result<u64, SyncError>` 返回 ack_offset）+ `MockSyncTransport`（fail_times 故障注入 + sent 记录，置于 lib.rs）；`endpoint` 字段移出 `DeltaSync`（真实 transport 实现承载） | no_std 无 async runtime/无 std::net（v0.95.0 D3/D8 CloudChannel、v0.106.0 D4 MmsTransport 同先例）；主机可测；真实 HTTP/gRPC 适配器在集成层注入 |
//! | **D5** | `CompressionType` 保留 None/Snappy/Gzip 3 变体（对齐蓝图数据结构），但本版仅 None 可构造——`DeltaSync::new` 遇 Snappy/Gzip 返回 `InvalidConfig` | 零第三方依赖约束；记忆 §5.5 集成清单未列入 no_std 压缩库（snappy/gzip 无 no_std 成熟实现）；压缩为传输层可选增强，后续版本按需引入 |
//! | **D6** | 蓝图 `rand::thread_rng().gen_range(0..=base)` 抖动 → 确定性 xorshift32 抖动：`xorshift32(retry_count×2654435761\|1) mod (base+1)` | `rand` 为 std 专用，no_std 不可用；确定性抖动同 retry_count 同结果，测试可断言，零依赖零状态 |
//! | **D7** | 蓝图 `current_time_ms()` 全局时间函数 → `now: u64` 参数注入（append/build_batch/retry_pending/sync_once/retry_once） | no_std 无系统时间（v0.108.0 D9 KeyMgmt / v0.109.0 D11 同先例）；集成层由 v0.12.0 RTC 供给 |
//! | **D8** | 蓝图 `crc32_checksum` 未定义 → 自实现 CRC32-IEEE（多项式 0xEDB88320，const 256 项表，纯 core 零依赖）+ `Event::verify()` | 蓝图 §7.3 要求事件完整性 CRC32；eneros-crypto 无 CRC32 实现（SM 系列不含）；表驱动 ~30 行成熟算法不属重复造轮子 |
//! | **D9** | 蓝图 `append` 返回 u64 → `Result<u64, SyncError>`：存储满且 compact 后仍无可压缩已同步事件 → `Err(StoreFull)`；`EventStore::new` 校验 max_events>0 → `InvalidConfig` | 蓝图 append 在「全未同步且已满」时静默越界增长或丢数据，违反 §7.1「断网后数据不丢」；显式错误让上游（RTOS 采样侧）感知背压 |
//! | **D10** | ① 超 max_retries 直接丢弃 → 有界死信队列（容量 8 批，溢出丢最旧死信）+ `dead_letter_count` 统计；② 重试失败重入队时 `created_at` 更新为失败时刻（重试时间基线）；③ `SyncBatch` 归 delta_sync.rs（build_batch 产出地）；④ `retry_pending` 蓝图 `pop_front().unwrap()` 改 if let（生产零 unwrap） | ① 蓝图注释「进入死信队列」但无实现，丢弃即数据丢失；② 蓝图用原始 created_at 判定退避，长断网后全部批次立即到期、退避失效形成重试风暴；③ 内聚；④ 记忆 §4.3 no_std 合规 |
//! | **D11** | 蓝图 `serialize_batch` / `snappy_compress` 未定义 → 自定义二进制帧：`[magic u16 LE=0xC537][version u8=1][event_count u16 LE]` + 每事件 `[offset u64][timestamp u64][event_type u8][payload_len u32][payload][checksum u32]`（全 LE） | 零第三方依赖（serde/postcard 不入仓）；magic+version 支撑云端 API 版本演进（蓝图 §8.4）；帧内含 per-event CRC32 支撑 §4.4 校验和不匹配重发 |
//! | **D12** | 错误模型 `SyncError` = StoreFull / TransportError / ServerError(u16) / ChecksumMismatch / InvalidConfig（5 变体，Debug/Clone/Copy/PartialEq）；新增 `SyncStats { total_sent, total_retry_enqueued, total_dead_letter, last_synced_offset }`（Debug/Clone/Copy/PartialEq）落地蓝图 §9 可观测要求；性能「1000 事件 < 2s」落地为 cfg(test) Instant 主机断言 | 蓝图引用 SyncError 未定义；变体覆盖 §4.4 各失败面（对齐 v0.95.0 CloudError Copy 惯例）；性能口径与 v0.109.0 D12 一致（真实网络时延为实验室项） |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，零第三方依赖，零 unsafe，零 extern "C"，
//! 不调用 `panic!` / `todo!` / `unimplemented!`，可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod delta_sync;
pub mod event_store;
pub mod retry_queue;

use alloc::vec::Vec;

pub use delta_sync::{CompressionType, DeltaSync, SyncBatch, SyncStats};
pub use event_store::{crc32, Event, EventStore, EventType};
pub use retry_queue::RetryQueue;

/// 同步错误（D12：5 变体覆盖蓝图 §4.4 各失败面）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SyncError {
    /// 事件存储已满且无可压缩已同步事件（断网期间背压，D9）。
    StoreFull,
    /// 传输层错误（网络不可达/超时等，由 `SyncTransport` 上报）。
    TransportError,
    /// 云端返回服务端错误（HTTP 状态码）。
    ServerError(u16),
    /// 批次 CRC32 校验和不匹配（蓝图 §4.4 触发重发）。
    ChecksumMismatch,
    /// 配置无效（max_events/batch_size 为 0、压缩非 None 等）。
    InvalidConfig,
}

/// 云边传输抽象（D4：sync trait，no_std 单线程惯例，不要求 Send+Sync；
/// 真实 HTTP/gRPC 适配器在集成层注入）。
pub trait SyncTransport {
    /// 发送序列化批次；成功返回云端确认的 ack_offset（语义 ≤ `batch.to_offset`）。
    fn send_batch(&mut self, batch: &SyncBatch, payload: &[u8]) -> Result<u64, SyncError>;
}

/// Mock 云边传输（D4，v0.95.0 MockCloudChannel 先例：故障注入 + 发送记录）。
pub struct MockSyncTransport {
    /// 剩余故障注入次数：>0 时递减并返回 `Err(TransportError)`。
    pub fail_times: u32,
    /// 已成功发送的 payload 记录（克隆）。
    pub sent: Vec<Vec<u8>>,
}

impl MockSyncTransport {
    /// 构造零故障 mock。
    pub fn new() -> Self {
        Self {
            fail_times: 0,
            sent: Vec::new(),
        }
    }

    /// 构造注入 `fail_times` 次连续 TransportError 的 mock。
    pub fn with_fail_times(fail_times: u32) -> Self {
        Self {
            fail_times,
            sent: Vec::new(),
        }
    }
}

impl Default for MockSyncTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncTransport for MockSyncTransport {
    fn send_batch(&mut self, batch: &SyncBatch, payload: &[u8]) -> Result<u64, SyncError> {
        if self.fail_times > 0 {
            self.fail_times -= 1;
            return Err(SyncError::TransportError);
        }
        self.sent.push(payload.to_vec());
        Ok(batch.to_offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// INT24 断网→补传→数据不丢（蓝图 §6.2 集成测试）。
    #[test]
    fn int24_offline_then_catchup_no_loss() {
        let mut store = EventStore::new(100).unwrap();
        for i in 0..5u64 {
            store.append(EventType::Telemetry, b"x", 1000 + i).unwrap();
        }
        let mut sync = DeltaSync::new(3, CompressionType::None).unwrap();
        let mut t = MockSyncTransport::with_fail_times(1);
        let mut q = RetryQueue::new(10, 1000);

        // 断网：sync_once 失败，批次（offset 0..2）入重试队列，store 零标记
        assert_eq!(
            sync.sync_once(&mut store, &mut t, &mut q, 10_000),
            Err(SyncError::TransportError)
        );
        assert_eq!(q.pending_len(), 1);
        assert_eq!(store.get_unsynced(10).len(), 5);

        // 网络恢复：retry_once 补传成功（ack=2）
        let r = sync.retry_once(&mut store, &mut t, &mut q, 2_000_000);
        assert_eq!(r, Ok(Some(2)));
        assert_eq!(q.pending_len(), 0);

        // 增量同步剩余 2 条（offset 3..4）
        let r2 = sync.sync_once(&mut store, &mut t, &mut q, 3_000_000);
        assert_eq!(r2, Ok(Some(4)));

        // 全部事件 mark_synced，零丢失
        assert!(store.get_unsynced(10).is_empty());
        assert_eq!(sync.last_synced_offset(), 4);
        assert_eq!(sync.stats().total_sent, 2);
        assert_eq!(sync.stats().total_retry_enqueued, 1);
    }

    /// INT25 幂等重发（蓝图 §6.4/§8.5）：mark_synced 重复调用状态一致。
    #[test]
    fn int25_idempotent_remark() {
        let mut store = EventStore::new(10).unwrap();
        for _ in 0..3 {
            store.append(EventType::Status, b"s", 1).unwrap();
        }
        store.mark_synced(2);
        let unsynced_after_first = store.get_unsynced(10).len();
        let len_after_first = store.len();
        store.mark_synced(2);
        store.mark_synced(1);
        assert_eq!(store.get_unsynced(10).len(), unsynced_after_first);
        assert_eq!(store.len(), len_after_first);
    }

    /// INT26 混合 6 类事件类型同步全流程。
    #[test]
    fn int26_mixed_event_types() {
        let types = [
            EventType::Telemetry,
            EventType::Status,
            EventType::Alarm,
            EventType::ControlLog,
            EventType::TradeRecord,
            EventType::ConfigChange,
        ];
        let mut store = EventStore::new(16).unwrap();
        for (i, ty) in types.iter().enumerate() {
            store.append(*ty, b"p", 100 + i as u64).unwrap();
        }
        let mut sync = DeltaSync::new(10, CompressionType::None).unwrap();
        let mut t = MockSyncTransport::new();
        let mut q = RetryQueue::new(3, 100);
        let r = sync.sync_once(&mut store, &mut t, &mut q, 200);
        assert_eq!(r, Ok(Some(5)));
        assert!(store.get_unsynced(10).is_empty());

        // 帧内事件类型码按 0..5 顺序排列
        let frame = &t.sent[0];
        assert_eq!(frame[0], 0x37);
        assert_eq!(frame[1], 0xC5);
        assert_eq!(frame[2], 1);
        assert_eq!(u16::from_le_bytes([frame[3], frame[4]]), 6);
        let mut pos = 5usize;
        for (i, _) in types.iter().enumerate() {
            // 跳过 offset(8) + timestamp(8)，读 event_type
            let type_byte = frame[pos + 16];
            assert_eq!(type_byte, i as u8);
            let plen = u32::from_le_bytes([
                frame[pos + 17],
                frame[pos + 18],
                frame[pos + 19],
                frame[pos + 20],
            ]) as usize;
            pos += 21 + plen + 4; // 头 21 + payload + crc32
        }
        assert_eq!(pos, frame.len());
    }

    /// INT27 长断网存储满 → StoreFull 且不丢已有。
    #[test]
    fn int27_long_offline_store_full_no_loss() {
        let mut store = EventStore::new(4).unwrap();
        for i in 0..4u64 {
            store.append(EventType::Alarm, b"a", i).unwrap();
        }
        // 全未同步且已满 → StoreFull，既有 4 条不丢
        assert_eq!(
            store.append(EventType::Alarm, b"a", 99),
            Err(SyncError::StoreFull)
        );
        assert_eq!(store.len(), 4);
        assert_eq!(store.get_unsynced(10).len(), 4);
        assert_eq!(store.current_offset(), 4);
    }

    /// INT28 mock sent payload 可解析回放（帧头+事件数+首事件 offset）。
    #[test]
    fn int28_mock_payload_parseable() {
        let mut store = EventStore::new(10).unwrap();
        store.append(EventType::Telemetry, b"v=1", 42).unwrap();
        store.append(EventType::Status, b"ok", 43).unwrap();
        let mut sync = DeltaSync::new(10, CompressionType::None).unwrap();
        let mut t = MockSyncTransport::new();
        let mut q = RetryQueue::new(3, 100);
        sync.sync_once(&mut store, &mut t, &mut q, 50).unwrap();

        assert_eq!(t.sent.len(), 1);
        let frame = &t.sent[0];
        assert_eq!(frame[0], 0x37); // magic LE 低字节
        assert_eq!(frame[1], 0xC5); // magic LE 高字节
        assert_eq!(frame[2], 1); // version
        assert_eq!(u16::from_le_bytes([frame[3], frame[4]]), 2); // event_count
                                                                 // 首事件 offset（u64 LE）
        let first_offset = u64::from_le_bytes([
            frame[5], frame[6], frame[7], frame[8], frame[9], frame[10], frame[11], frame[12],
        ]);
        assert_eq!(first_offset, 0);
    }

    /// PERF29 批量同步性能（蓝图 §6.3/§7.2，cfg(test) Instant 口径，D12）。
    #[test]
    fn perf29_batch_sync_1000_events() {
        let start = std::time::Instant::now();
        let mut store = EventStore::new(2048).unwrap();
        for i in 0..1000u64 {
            store
                .append(EventType::Telemetry, b"p=100,q=50", i)
                .unwrap();
        }
        let mut sync = DeltaSync::new(100, CompressionType::None).unwrap();
        let mut t = MockSyncTransport::new();
        let mut q = RetryQueue::new(5, 100);
        // 10 批 × (build_batch + serialize + mock send)
        for round in 0..10u64 {
            let r = sync.sync_once(&mut store, &mut t, &mut q, 10_000 + round);
            assert!(r.unwrap().is_some());
        }
        assert!(store.get_unsynced(10).is_empty());
        assert_eq!(t.sent.len(), 10);
        assert!(
            start.elapsed().as_millis() < 2000,
            "1000 事件批量同步耗时 {:?} 超 2000ms",
            start.elapsed()
        );
    }
}
