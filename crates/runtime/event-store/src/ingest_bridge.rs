//! 蓝图 61 [07] §4.6.3/§4.7（v0.314.32）：接入桥——IngestionModule 的事件
//! 日志接缝换成分区事件日志（`StoreSink`）。
//!
//! 不变量（[07] §4.1/§4.6）：**已 ACK 接入的事件在规定 RPO 内可恢复、有序
//! 重放和去重**。本桥的 ACK 语义：
//!
//! - 事件统一以 Q0（写透）追加：`store.flush()` 在 append 返回前完成
//!   （ACK 即 durable，RPO = 0）。QoS 分级攒批（Q1–Q3 按
//!   [`StoreConfig::batch_records`] 到量落盘）的 ACK 语义 = RPO（批量
//!   间隔）内可恢复——事件 QoS 来自 PointPlan，本桥不臆造，装配面按
//!   注册策略开启分级。
//! - 分区键（[07] §4.3：tenant/site/QoS/hash(source_id)）：FNV-1a
//!   (source_id) 低 24 位——同源事件同分区，顺序重放保序。
//! - 交付面：[`StoreSink::drain_frames`] 以 delivery consumer group 位点
//!   续读、提交——重复 drain 不重复交付（位点双槽持久）。
//! - 语义去重/序号纪律由 [`IngestionModule`]（SourceRegistry + 同源
//!   watermark）承担；store 只做幂等位点（[07] §4.4：重放保留原
//!   event_id——消费侧以 event_id 幂等关联）。

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use eneros_ingestion_core::{AppendAck, IngestReject, ObservationSink};
use eneros_telemetry_model::event::{ObservationEvent, OBSERVATION_WIRE_LEN};

use crate::{
    ConsumerGroupId, EventStore, LogOffset, PartitionId, PartitionOffset, RecordRef, RecordVisitor,
    StoreError,
};

/// 本桥的 delivery consumer group（与消费面其他组隔离）。
pub const DELIVERY_GROUP: ConsumerGroupId = 1;

/// 分区键取模范围（FNV-1a 低 24 位 ≈ 16M 分区）。
const PARTITION_MOD: u32 = 1 << 24;

/// 分区键（[07] §4.3：hash(source_id)；同源顺序保序）。
pub fn partition_of(event: &ObservationEvent) -> PartitionId {
    fnv1a(event.source_id.to_le_bytes()).wrapping_rem(PARTITION_MOD)
}

/// FNV-1a 32（非密码学——只作分区散列）。
fn fnv1a(bytes: [u8; 4]) -> u32 {
    let mut h: u32 = 0x811C_9DC5;
    for b in bytes {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// 事件日志 = 分区事件日志（[`EventStore`]）的 [`ObservationSink`] 适配。
pub struct StoreSink<S: EventStore> {
    store: S,
    /// source → (epoch, sequence)（同 InMemorySink 语义；模块 watermark
    /// 复核接缝）。
    watermarks: BTreeMap<u32, (u64, u64)>,
    /// 本桥见过的分区（drain 枚举面）。
    partitions: BTreeSet<PartitionId>,
    /// 追加失败计数（可观测）。
    pub append_failures: u64,
}

impl<S: EventStore> StoreSink<S> {
    pub fn new(store: S) -> Self {
        Self {
            store,
            watermarks: BTreeMap::new(),
            partitions: BTreeSet::new(),
            append_failures: 0,
        }
    }

    pub fn into_inner(self) -> S {
        self.store
    }

    pub fn len(&self) -> usize {
        self.partitions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.partitions.is_empty()
    }

    /// 排空待交付帧（[07] §4.4 重放语义：保留原 event_id/payload；
    /// 位点推进幂等）。
    ///
    /// 每分区：读起点 = delivery 已提交位点（无提交 → 0）→ 全量读 →
    /// 位点推进到 [`crate::ReadReport::next_offset`] → commit（双槽持久）。
    /// 只交付 128B 观测帧（其余类型帧跳过但仍计位点——后续卷消费面
    /// 扩展）；失败 = 显式 Err（调用方重试幂等）。
    pub fn collect_delivery(&mut self) -> Result<Vec<[u8; 128]>, StoreError> {
        let mut out = Vec::new();
        // 分区全集 = store 扫描面 ∪ 本桥写面（掉电重开后 store 面主导）。
        let mut all: BTreeSet<PartitionId> = self.store.partitions();
        all.extend(self.partitions.iter().copied());
        let partitions: Vec<PartitionId> = all.into_iter().collect();
        for part in partitions {
            let from = self
                .store
                .committed_offset(DELIVERY_GROUP, part)
                .unwrap_or(0);
            let mut frames: Vec<[u8; 128]> = Vec::new();
            let next_from = {
                let mut c = FrameCollect {
                    frames: &mut frames,
                };
                self.store.read(part, from, &mut c)?.next_offset
            };
            // 位点推进（含跳过的非观测帧——消费面不重复见）。
            self.store
                .commit_consumer(DELIVERY_GROUP, &[PartitionOffset::new(part, next_from)])?;
            out.extend(frames);
        }
        Ok(out)
    }

    /// 某分区已提交交付位点（诊断）。
    pub fn delivery_offset(&self, partition: PartitionId) -> Option<LogOffset> {
        self.store.committed_offset(DELIVERY_GROUP, partition)
    }
}

struct FrameCollect<'a> {
    frames: &'a mut Vec<[u8; 128]>,
}

impl RecordVisitor for FrameCollect<'_> {
    fn record(&mut self, r: RecordRef<'_>) -> bool {
        if r.payload.len() == OBSERVATION_WIRE_LEN {
            let mut f = [0u8; OBSERVATION_WIRE_LEN];
            f.copy_from_slice(r.payload);
            self.frames.push(f);
        }
        true
    }
}

impl<S: EventStore> ObservationSink for StoreSink<S> {
    fn append(&mut self, event: &ObservationEvent) -> Result<AppendAck, IngestReject> {
        // 1. 同源 watermark 复核（跨实例回退/重放——InMemorySink 同语义）。
        if let Some(&(we, ws)) = self.watermarks.get(&event.source_id) {
            if event.source_epoch < we {
                return Err(IngestReject::EpochRegression {
                    epoch: event.source_epoch,
                    expected: we,
                });
            }
            if we == event.source_epoch && event.sequence <= ws {
                return Err(IngestReject::SequenceReplay {
                    sequence: event.sequence,
                    last: ws,
                });
            }
        }
        // 2. 分区日志追加（Q0 写透——ACK 即 durable，[07] §4.6）。
        let part = partition_of(event);
        let wire = event.encode_v1();
        let offset = self
            .store
            .append(part, event.event_id(), 0, &wire)
            .map_err(|e| {
                self.append_failures += 1;
                store_err_to_reject(e)
            })?;
        self.partitions.insert(part);
        // 3. watermark 推进（同源单调）。
        self.watermarks
            .insert(event.source_id, (event.source_epoch, event.sequence));
        Ok(AppendAck {
            event_id: event.event_id(),
            offset,
        })
    }

    fn watermark(&self, source_id: u32) -> Option<(u64, u64)> {
        self.watermarks.get(&source_id).copied()
    }

    fn drain_frames(&mut self) -> Vec<[u8; 128]> {
        self.collect_delivery().unwrap_or_default()
    }
}

/// StoreError → IngestReject 映射（[07] §2.5：背压显式化——Full/高水位 →
/// Overflow）。
fn store_err_to_reject(e: StoreError) -> IngestReject {
    match e {
        StoreError::Full | StoreError::HighWatermark => IngestReject::Overflow,
        _ => IngestReject::Overflow,
    }
}
