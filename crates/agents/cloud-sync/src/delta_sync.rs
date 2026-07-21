//! 增量批量同步与二进制帧序列化（D5/D10/D11）+ `sync_once`/`retry_once` 编排。
//!
//! 帧格式（全小端，D11）：
//! `[magic u16 LE = 0xC537][version u8 = 1][event_count u16 LE]` +
//! 每事件 `[offset u64 LE][timestamp u64 LE][event_type u8][payload_len u32 LE]
//! [payload][checksum u32 LE]`。
//! magic + version 支撑云端 API 版本演进（蓝图 §8.4）；per-event CRC32 支撑
//! 蓝图 §4.4 校验和不匹配重发。

use alloc::vec::Vec;

use crate::event_store::{Event, EventStore};
use crate::retry_queue::RetryQueue;
use crate::{SyncError, SyncTransport};

/// 同步帧魔数（u16 LE，D11）。
const FRAME_MAGIC: u16 = 0xC537;
/// 同步帧版本（D11，云端 API 演进）。
const FRAME_VERSION: u8 = 1;

/// 压缩类型（D5：对齐蓝图 3 变体，本版仅 None 可构造——零第三方依赖约束，
/// snappy/gzip 无 no_std 成熟实现；压缩为传输层可选增强，后续版本按需引入）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CompressionType {
    /// 不压缩（本版唯一可用）。
    None,
    /// Snappy（预留，构造时返回 InvalidConfig）。
    Snappy,
    /// Gzip（预留，构造时返回 InvalidConfig）。
    Gzip,
}

/// 同步批次（D10 归属本模块——`build_batch` 产出地）。
///
/// `batch_id == from_offset`（蓝图简化约定，云端据此幂等去重，§6.4）。
#[derive(Debug, Clone, PartialEq)]
pub struct SyncBatch {
    /// 批次 ID（= from_offset，云端幂等去重键）。
    pub batch_id: u64,
    /// 批次内事件（克隆，offset 升序）。
    pub events: Vec<Event>,
    /// 起始 offset（含）。
    pub from_offset: u64,
    /// 结束 offset（含）。
    pub to_offset: u64,
    /// 已重试次数（重试队列推进）。
    pub retry_count: u32,
    /// 批次创建/上次重试时刻（ms，退避时间基线，D10）。
    pub created_at: u64,
}

/// 同步统计（D12：落地蓝图 §9 可观测要求）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SyncStats {
    /// 累计成功发送批次数。
    pub total_sent: u64,
    /// 累计入重试队列批次数。
    pub total_retry_enqueued: u64,
    /// 累计死信批次数（镜像 RetryQueue::dead_letter_count）。
    pub total_dead_letter: u64,
    /// 云端已确认的最大 offset。
    pub last_synced_offset: u64,
}

/// 增量同步器（D4：endpoint 字段移出，由真实 transport 实现承载）。
pub struct DeltaSync {
    /// 单批最大事件数。
    batch_size: usize,
    /// 压缩类型（D5：仅 None）。
    compression: CompressionType,
    /// 云端已确认的最大 offset。
    last_synced_offset: u64,
    /// 同步统计。
    stats: SyncStats,
}

impl DeltaSync {
    /// 构造增量同步器；`batch_size == 0` 或 `compression != None` 返回
    /// `InvalidConfig`（D5）。
    pub fn new(batch_size: usize, compression: CompressionType) -> Result<Self, SyncError> {
        if batch_size == 0 || compression != CompressionType::None {
            return Err(SyncError::InvalidConfig);
        }
        Ok(Self {
            batch_size,
            compression,
            last_synced_offset: 0,
            stats: SyncStats {
                total_sent: 0,
                total_retry_enqueued: 0,
                total_dead_letter: 0,
                last_synced_offset: 0,
            },
        })
    }

    /// 取前 `batch_size` 条未同步事件克隆组批；无未同步事件返回 `None`。
    pub fn build_batch(&self, store: &EventStore, now: u64) -> Option<SyncBatch> {
        let unsynced = store.get_unsynced(self.batch_size);
        let first = unsynced.first()?;
        let from_offset = first.offset;
        let to_offset = unsynced[unsynced.len() - 1].offset;
        let events: Vec<Event> = unsynced.into_iter().cloned().collect();
        Some(SyncBatch {
            batch_id: from_offset,
            events,
            from_offset,
            to_offset,
            retry_count: 0,
            created_at: now,
        })
    }

    /// 序列化批次为二进制帧（D11，全小端）。
    pub fn serialize(batch: &SyncBatch) -> Vec<u8> {
        let mut frame = Vec::new();
        frame.extend_from_slice(&FRAME_MAGIC.to_le_bytes());
        frame.push(FRAME_VERSION);
        frame.extend_from_slice(&(batch.events.len() as u16).to_le_bytes());
        for e in &batch.events {
            frame.extend_from_slice(&e.offset.to_le_bytes());
            frame.extend_from_slice(&e.timestamp.to_le_bytes());
            frame.push(e.event_type as u8);
            frame.extend_from_slice(&(e.payload.len() as u32).to_le_bytes());
            frame.extend_from_slice(&e.payload);
            frame.extend_from_slice(&e.checksum.to_le_bytes());
        }
        frame
    }

    /// 增量同步一轮：build → serialize → send。
    ///
    /// 无未同步事件返回 `Ok(None)`；成功标记 `offset <= ack` 已同步并推进统计；
    /// 失败批次入重试队列（不标记任何事件），原样返回错误（蓝图 §4.4）。
    pub fn sync_once<S: SyncTransport>(
        &mut self,
        store: &mut EventStore,
        transport: &mut S,
        queue: &mut RetryQueue,
        now: u64,
    ) -> Result<Option<u64>, SyncError> {
        let batch = match self.build_batch(store, now) {
            Some(b) => b,
            None => return Ok(None),
        };
        let payload = Self::serialize(&batch);
        match transport.send_batch(&batch, &payload) {
            Ok(ack) => {
                store.mark_synced(ack);
                self.last_synced_offset = ack;
                self.stats.total_sent += 1;
                self.stats.last_synced_offset = ack;
                Ok(Some(ack))
            }
            Err(e) => {
                queue.enqueue(batch);
                self.stats.total_retry_enqueued += 1;
                Err(e)
            }
        }
    }

    /// 重试一轮：取出到期批次 → serialize → send。
    ///
    /// 无到期批次返回 `Ok(None)`；成功同 `sync_once`；失败批次更新
    /// `created_at = now`（重试时间基线，D10）后重新入队，原样返回错误。
    pub fn retry_once<S: SyncTransport>(
        &mut self,
        store: &mut EventStore,
        transport: &mut S,
        queue: &mut RetryQueue,
        now: u64,
    ) -> Result<Option<u64>, SyncError> {
        self.stats.total_dead_letter = queue.dead_letter_count();
        let mut batch = match queue.retry_pending(now) {
            Some(b) => b,
            None => return Ok(None),
        };
        let payload = Self::serialize(&batch);
        match transport.send_batch(&batch, &payload) {
            Ok(ack) => {
                store.mark_synced(ack);
                self.last_synced_offset = ack;
                self.stats.total_sent += 1;
                self.stats.last_synced_offset = ack;
                Ok(Some(ack))
            }
            Err(e) => {
                batch.created_at = now;
                queue.enqueue(batch);
                Err(e)
            }
        }
    }

    /// 压缩类型（D5：本版恒为 None）。
    pub fn compression(&self) -> CompressionType {
        self.compression
    }

    /// 同步统计（D12 可观测）。
    pub fn stats(&self) -> &SyncStats {
        &self.stats
    }

    /// 云端已确认的最大 offset。
    pub fn last_synced_offset(&self) -> u64 {
        self.last_synced_offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventType, MockSyncTransport};

    /// DS10 build_batch 空 → None。
    #[test]
    fn ds10_build_batch_empty_none() {
        let store = EventStore::new(10).unwrap();
        let sync = DeltaSync::new(3, CompressionType::None).unwrap();
        assert!(sync.build_batch(&store, 100).is_none());
        // 全部已同步同样返回 None
        let mut store2 = EventStore::new(10).unwrap();
        store2.append(EventType::Telemetry, b"t", 1).unwrap();
        store2.mark_synced(0);
        assert!(sync.build_batch(&store2, 100).is_none());
    }

    /// DS11 build_batch 语义（batch_id=from、to=last、retry_count=0、created_at=now）。
    #[test]
    fn ds11_build_batch_semantics() {
        let mut store = EventStore::new(10).unwrap();
        for i in 0..5u64 {
            store.append(EventType::Telemetry, b"t", i).unwrap();
        }
        let sync = DeltaSync::new(3, CompressionType::None).unwrap();
        let batch = sync.build_batch(&store, 777).unwrap();
        assert_eq!(batch.batch_id, 0);
        assert_eq!(batch.from_offset, 0);
        assert_eq!(batch.to_offset, 2);
        assert_eq!(batch.events.len(), 3);
        assert_eq!(batch.retry_count, 0);
        assert_eq!(batch.created_at, 777);
        assert_eq!(batch.events[2].offset, 2);
    }

    /// DS12 new 校验（batch_size==0 / Snappy / Gzip → InvalidConfig，D5）。
    #[test]
    fn ds12_new_validation() {
        assert!(matches!(
            DeltaSync::new(0, CompressionType::None),
            Err(SyncError::InvalidConfig)
        ));
        assert!(matches!(
            DeltaSync::new(10, CompressionType::Snappy),
            Err(SyncError::InvalidConfig)
        ));
        assert!(matches!(
            DeltaSync::new(10, CompressionType::Gzip),
            Err(SyncError::InvalidConfig)
        ));
        assert!(DeltaSync::new(10, CompressionType::None).is_ok());
    }

    /// DS13 serialize 帧布局（magic 0xC537 + version + count + 事件 TLV + CRC，D11）。
    #[test]
    fn ds13_serialize_frame_layout() {
        let mut store = EventStore::new(10).unwrap();
        store.append(EventType::Telemetry, b"p=1", 123).unwrap();
        store.append(EventType::Alarm, b"al", 124).unwrap();
        let sync = DeltaSync::new(10, CompressionType::None).unwrap();
        let batch = sync.build_batch(&store, 1).unwrap();
        let frame = DeltaSync::serialize(&batch);

        // 帧头：magic LE + version + event_count LE
        assert_eq!(frame[0], 0x37);
        assert_eq!(frame[1], 0xC5);
        assert_eq!(frame[2], 1);
        assert_eq!(u16::from_le_bytes([frame[3], frame[4]]), 2);

        // 事件 0：offset u64 LE = 0
        assert_eq!(
            u64::from_le_bytes([
                frame[5], frame[6], frame[7], frame[8], frame[9], frame[10], frame[11], frame[12]
            ]),
            0
        );
        // timestamp u64 LE = 123
        assert_eq!(
            u64::from_le_bytes([
                frame[13], frame[14], frame[15], frame[16], frame[17], frame[18], frame[19],
                frame[20]
            ]),
            123
        );
        // event_type = Telemetry(0)
        assert_eq!(frame[21], 0);
        // payload_len u32 LE = 3
        assert_eq!(
            u32::from_le_bytes([frame[22], frame[23], frame[24], frame[25]]),
            3
        );
        // payload
        assert_eq!(&frame[26..29], b"p=1");
        // checksum u32 LE = crc32(b"p=1")
        assert_eq!(
            u32::from_le_bytes([frame[29], frame[30], frame[31], frame[32]]),
            crate::crc32(b"p=1")
        );
        // 事件 1 起点 = 5 + 21 + 3 + 4 = 33
        assert_eq!(frame[33 + 16], 2); // Alarm 类型码
        assert_eq!(&frame[33 + 21..33 + 23], b"al");
        // 帧总长 = 33 + 21 + 2 + 4
        assert_eq!(frame.len(), 60);
    }

    /// DS14 sync_once 成功 mark_synced + stats。
    #[test]
    fn ds14_sync_once_success() {
        let mut store = EventStore::new(10).unwrap();
        for i in 0..3u64 {
            store.append(EventType::Status, b"s", i).unwrap();
        }
        let mut sync = DeltaSync::new(10, CompressionType::None).unwrap();
        let mut t = MockSyncTransport::new();
        let mut q = RetryQueue::new(3, 100);
        let r = sync.sync_once(&mut store, &mut t, &mut q, 500);
        assert_eq!(r, Ok(Some(2)));
        assert!(store.get_unsynced(10).is_empty());
        assert_eq!(sync.last_synced_offset(), 2);
        assert_eq!(sync.stats().total_sent, 1);
        assert_eq!(sync.stats().last_synced_offset, 2);
        assert_eq!(t.sent.len(), 1);
        // 无未同步事件时再调返回 Ok(None)
        let r2 = sync.sync_once(&mut store, &mut t, &mut q, 600);
        assert_eq!(r2, Ok(None));
    }

    /// DS15 sync_once 失败入队不标记（蓝图 §4.4）。
    #[test]
    fn ds15_sync_once_failure_enqueued() {
        let mut store = EventStore::new(10).unwrap();
        for i in 0..3u64 {
            store.append(EventType::Status, b"s", i).unwrap();
        }
        let mut sync = DeltaSync::new(10, CompressionType::None).unwrap();
        let mut t = MockSyncTransport::with_fail_times(1);
        let mut q = RetryQueue::new(3, 100);
        let r = sync.sync_once(&mut store, &mut t, &mut q, 500);
        assert_eq!(r, Err(SyncError::TransportError));
        assert_eq!(store.get_unsynced(10).len(), 3); // 零标记
        assert_eq!(q.pending_len(), 1);
        assert_eq!(sync.stats().total_retry_enqueued, 1);
        assert_eq!(sync.stats().total_sent, 0);
    }

    /// DS16 retry_once 成功 + 失败重入队 created_at 更新（D10）。
    #[test]
    fn ds16_retry_once_success_and_reenqueue() {
        let mut store = EventStore::new(10).unwrap();
        for i in 0..2u64 {
            store.append(EventType::Status, b"s", i).unwrap();
        }
        let mut sync = DeltaSync::new(10, CompressionType::None).unwrap();
        let mut q = RetryQueue::new(5, 1000);

        // 先入队一批（模拟断网失败），created_at = 500
        let mut t = MockSyncTransport::with_fail_times(1);
        assert!(sync.sync_once(&mut store, &mut t, &mut q, 500).is_err());
        assert_eq!(q.pending_len(), 1);

        // 退避未到期 → Ok(None)
        assert_eq!(sync.retry_once(&mut store, &mut t, &mut q, 600), Ok(None));

        // 到期重试仍失败 → Err + 重入队 + created_at 更新为失败时刻
        let mut t2 = MockSyncTransport::with_fail_times(1);
        let r = sync.retry_once(&mut store, &mut t2, &mut q, 2_000_000);
        assert_eq!(r, Err(SyncError::TransportError));
        assert_eq!(q.pending_len(), 1);

        // created_at 已更新为 2_000_000：100ms 后远未到 backoff(1)≥2000ms → Ok(None)
        // （若仍用旧 created_at=500，此刻早已到期弹出）
        let mut t3 = MockSyncTransport::new();
        assert_eq!(
            sync.retry_once(&mut store, &mut t3, &mut q, 2_000_100),
            Ok(None)
        );
        assert_eq!(q.pending_len(), 1);

        // 再次到期重试成功
        let r2 = sync.retry_once(&mut store, &mut t3, &mut q, 4_000_000);
        assert_eq!(r2, Ok(Some(1)));
        assert!(store.get_unsynced(10).is_empty());
        assert_eq!(sync.last_synced_offset(), 1);
    }
}
