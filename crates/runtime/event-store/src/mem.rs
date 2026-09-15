//! [`MemEventStore`] — 内存 reference model（[07] §4.6 差分测试的对照实现）。
//!
//! 与 [`crate::block::BlockEventStore`] 同 trait、同 wire 编码
//! （[`crate::format`]）、同批量/水位/保留协议，仅无介质 I/O：
//! `BTreeMap<PartitionId, 分区日志>`，分区日志 = sealed/open segment 的
//! `Vec`（记录为解码后的 Rust 结构）。差分测试（tests/differential.rs）
//! 要求两者在同一随机操作序列下逐 API 一致，因此本实现独立于块实现
//! （不共享协议代码），只共享 wire 编码。

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use sha2::{Digest, Sha256};

use crate::format::{
    self, encode_record, encode_record_into, encoded_record_len, FIXED_BYTES, PAYLOAD_MAX,
    RECORD_MAX,
};
use crate::{
    ConsumerGroupId, DeletedSegment, EventStore, LogOffset, PartitionId, PartitionOffset,
    PartitionWatermark, ReadReport, RecordRef, RecordVisitor, RecoveryReport, StoreConfig,
    StoreError, WatermarkReport,
};

/// staged 记录（未落盘，读路径合并可见）。
#[derive(Debug, Clone)]
struct StagedRec {
    offset: LogOffset,
    event_id: [u8; 16],
    flags: u16,
    payload: Vec<u8>,
}

/// segment 的 RAM 形态（记录解码存放；digest 在 seal 时由 SHA-256 得出）。
#[derive(Debug)]
struct MemSeg {
    segment_id: u32,
    first_offset: LogOffset,
    /// = first_offset + count（"已落盘"终点；staged 不在内）。
    next_offset: LogOffset,
    /// Σ record 编码字节数（与块实现同口径）。
    data_bytes: u64,
    count: u64,
    sealed: bool,
    digest: Option<[u8; 32]>,
    hasher: Sha256,
    records: Vec<MemRecord>,
}

#[derive(Debug, Clone)]
struct MemRecord {
    offset: LogOffset,
    event_id: [u8; 16],
    flags: u16,
    payload: Vec<u8>,
}

#[derive(Debug, Default)]
struct MemPart {
    /// 按 first_offset 升序（前缀删除保持连续）。
    segments: Vec<MemSeg>,
    next_offset: LogOffset,
    next_segment_id: u32,
    staged: VecDeque<StagedRec>,
}

/// 内存 reference model。
///
/// `capacity_bytes` 与块实现的设备容量同口径参与水位/Full 判定，
/// 保证差分测试可逐字段比对 [`WatermarkReport`]。
#[derive(Debug)]
pub struct MemEventStore {
    capacity_bytes: u64,
    config: StoreConfig,
    /// 单段 record 区容量（segment_bytes - trailer 块 - header 64B）。
    seg_capacity: u64,
    /// 最大段数（(capacity - 固定区) / segment_bytes）。
    max_slots: u64,
    parts: BTreeMap<PartitionId, MemPart>,
    consumers: BTreeMap<ConsumerGroupId, BTreeMap<PartitionId, LogOffset>>,
}

impl MemEventStore {
    /// 创建内存 store（`capacity_bytes` 模拟设备容量，含 3 块固定区）。
    pub fn new(capacity_bytes: u64, config: StoreConfig) -> Result<Self, StoreError> {
        config.validate()?;
        if capacity_bytes < FIXED_BYTES + u64::from(config.segment_bytes) {
            return Err(StoreError::BadConfig);
        }
        let seg_capacity = u64::from(config.segment_bytes)
            - format::BLOCK_SIZE as u64
            - format::SEGMENT_HEADER_LEN;
        Ok(MemEventStore {
            capacity_bytes,
            seg_capacity,
            max_slots: (capacity_bytes - FIXED_BYTES) / u64::from(config.segment_bytes),
            config,
            parts: BTreeMap::new(),
            consumers: BTreeMap::new(),
        })
    }

    /// 配置副本。
    pub fn config(&self) -> StoreConfig {
        self.config
    }

    /// 全局占用 permille（与块实现同公式：固定区 + 已分配段 × 段大小）。
    fn used_permille(&self) -> u32 {
        let allocated: u64 = self.parts.values().map(|p| p.segments.len() as u64).sum();
        let used = FIXED_BYTES + allocated * u64::from(self.config.segment_bytes);
        (((used * 1000) / self.capacity_bytes).min(1000)) as u32
    }

    /// seal：digest = SHA-256(全段 record 编码字节流)；空段不产生 digest。
    fn seal(seg: &mut MemSeg) {
        if seg.sealed {
            return;
        }
        if seg.count > 0 {
            let h = seg.hasher.clone();
            let d = h.finalize();
            let mut sha = [0u8; 32];
            sha.copy_from_slice(&d);
            seg.digest = Some(sha);
        }
        seg.sealed = true;
    }

    /// staged 落"盘"（旋转/seal 协议与块实现一致，无 I/O）。
    fn write_staged(&mut self, partition: PartitionId) -> Result<(), StoreError> {
        while let Some(front) = self.parts.get(&partition).and_then(|p| p.staged.front()) {
            let reclen = encoded_record_len(front.payload.len()) as u64;
            let need_new = match self.parts.get(&partition).unwrap().segments.last() {
                None => true,
                Some(seg) => seg.sealed || seg.data_bytes + reclen > self.seg_capacity,
            };
            if need_new {
                if let Some(seg) = self.parts.get_mut(&partition).unwrap().segments.last_mut() {
                    Self::seal(seg);
                }
                if self.parts.get(&partition).unwrap().segments.len() as u64 >= self.max_slots {
                    return Err(StoreError::Full); // staged 保留，等调用方释放
                }
                let p = self.parts.get_mut(&partition).unwrap();
                let first_offset = p.staged.front().expect("front checked").offset;
                p.segments.push(MemSeg {
                    segment_id: p.next_segment_id,
                    first_offset,
                    next_offset: first_offset,
                    data_bytes: 0,
                    count: 0,
                    sealed: false,
                    digest: None,
                    hasher: Sha256::new(),
                    records: Vec::new(),
                });
                p.next_segment_id += 1;
            }
            let rec = self
                .parts
                .get_mut(&partition)
                .expect("entry exists")
                .staged
                .pop_front()
                .expect("front checked");
            let seg = self
                .parts
                .get_mut(&partition)
                .expect("entry exists")
                .segments
                .last_mut()
                .expect("segment just ensured");
            let enc = encode_record(rec.offset, rec.event_id, rec.flags, 0, &rec.payload);
            debug_assert_eq!(enc.len(), reclen as usize);
            seg.hasher.update(&enc);
            seg.data_bytes += enc.len() as u64;
            seg.count += 1;
            seg.next_offset += 1;
            seg.records.push(MemRecord {
                offset: rec.offset,
                event_id: rec.event_id,
                flags: rec.flags,
                payload: rec.payload,
            });
        }
        Ok(())
    }
}

impl EventStore for MemEventStore {
    fn partitions(&self) -> alloc::collections::BTreeSet<PartitionId> {
        self.parts.keys().copied().collect()
    }
    fn append(
        &mut self,
        partition: PartitionId,
        event_id: [u8; 16],
        flags: u16,
        payload: &[u8],
    ) -> Result<LogOffset, StoreError> {
        if payload.len() > PAYLOAD_MAX {
            return Err(StoreError::RecordTooLarge);
        }
        if event_id == [0u8; 16] {
            return Err(StoreError::BadEventId);
        }
        let qos = crate::qos_of(flags);
        if self.config.reject_q2q3_at_95_percent && qos >= 2 && self.used_permille() >= 950 {
            return Err(StoreError::HighWatermark);
        }
        let limit = if qos == 0 {
            self.config.q0_batch_records
        } else {
            self.config.batch_records
        };
        let part = self.parts.entry(partition).or_default();
        let offset = part.next_offset;
        part.next_offset += 1;
        part.staged.push_back(StagedRec {
            offset,
            event_id,
            flags,
            payload: Vec::from(payload),
        });
        if part.staged.len() as u32 >= limit {
            self.write_staged(partition)?;
        }
        Ok(offset)
    }

    fn read(
        &self,
        partition: PartitionId,
        from: LogOffset,
        out: &mut dyn RecordVisitor,
    ) -> Result<ReadReport, StoreError> {
        let Some(part) = self.parts.get(&partition) else {
            // 未出现过的分区：next_write = 0。
            if from > 0 {
                return Err(StoreError::OffsetOutOfRange);
            }
            return Ok(ReadReport {
                records_read: 0,
                payload_bytes: 0,
                next_offset: from,
            });
        };
        // 保留下限：最早的段 → 最早的 staged →（全删光时）写入位置。
        let floor = part
            .segments
            .first()
            .map(|s| s.first_offset)
            .or_else(|| part.staged.front().map(|r| r.offset))
            .unwrap_or(part.next_offset);
        if from > part.next_offset {
            return Err(StoreError::OffsetOutOfRange);
        }
        if from < floor {
            return Err(StoreError::OffsetGone);
        }
        let mut expected = from;
        let mut records = 0u64;
        let mut payload_bytes = 0u64;
        let mut emit = |r: RecordRef<'_>,
                        records: &mut u64,
                        payload_bytes: &mut u64,
                        expected: &mut LogOffset|
         -> bool {
            if !out.record(r) {
                return false;
            }
            *records += 1;
            *payload_bytes += r.payload.len() as u64;
            *expected += 1;
            true
        };
        for seg in &part.segments {
            if seg.next_offset <= from {
                continue;
            }
            if seg.first_offset > expected {
                return Err(StoreError::OffsetGap);
            }
            let skip = (from.saturating_sub(seg.first_offset) as usize).min(seg.records.len());
            for rec in &seg.records[skip..] {
                if rec.offset != expected {
                    return Err(StoreError::OffsetGap);
                }
                let r = RecordRef {
                    offset: rec.offset,
                    event_id: rec.event_id,
                    flags: rec.flags,
                    schema: 0,
                    payload: &rec.payload,
                };
                if !emit(r, &mut records, &mut payload_bytes, &mut expected) {
                    return Ok(ReadReport {
                        records_read: records,
                        payload_bytes,
                        next_offset: expected,
                    });
                }
            }
        }
        for rec in &part.staged {
            if rec.offset < from {
                continue;
            }
            if rec.offset != expected {
                return Err(StoreError::OffsetGap);
            }
            let r = RecordRef {
                offset: rec.offset,
                event_id: rec.event_id,
                flags: rec.flags,
                schema: 0,
                payload: &rec.payload,
            };
            if !emit(r, &mut records, &mut payload_bytes, &mut expected) {
                return Ok(ReadReport {
                    records_read: records,
                    payload_bytes,
                    next_offset: expected,
                });
            }
        }
        Ok(ReadReport {
            records_read: records,
            payload_bytes,
            next_offset: expected,
        })
    }

    fn commit_consumer(
        &mut self,
        group: ConsumerGroupId,
        offsets: &[PartitionOffset],
    ) -> Result<(), StoreError> {
        if offsets.is_empty() {
            return Ok(());
        }
        // 容量口径与块实现一致：双槽持久承载全部 (group, partition) 条目。
        let total: usize = self.consumers.values().map(|m| m.len()).sum();
        let mut fresh: Vec<PartitionId> = Vec::new();
        for off in offsets {
            let exists = self
                .consumers
                .get(&group)
                .is_some_and(|m| m.contains_key(&off.partition));
            if !exists && !fresh.contains(&off.partition) {
                fresh.push(off.partition);
            }
        }
        if total + fresh.len() > format::OFFSET_MAX_ENTRIES {
            return Err(StoreError::CommitTooLarge);
        }
        let m = self.consumers.entry(group).or_default();
        for off in offsets {
            m.insert(off.partition, off.offset);
        }
        Ok(())
    }

    fn committed_offset(
        &self,
        group: ConsumerGroupId,
        partition: PartitionId,
    ) -> Option<LogOffset> {
        self.consumers.get(&group)?.get(&partition).copied()
    }

    fn recover(&mut self) -> Result<RecoveryReport, StoreError> {
        // 无介质故障：校验不变量（编码往返/CRC/分段 digest）并如实报告。
        let mut segments_scanned = 0u32;
        let mut valid_records = 0u64;
        for part in self.parts.values() {
            for seg in &part.segments {
                segments_scanned += 1;
                valid_records += seg.count;
                let mut h = Sha256::new();
                for rec in &seg.records {
                    let mut enc = [0u8; RECORD_MAX];
                    let n = encode_record_into(
                        &mut enc,
                        rec.offset,
                        rec.event_id,
                        rec.flags,
                        0,
                        &rec.payload,
                    );
                    h.update(&enc[..n]);
                    let Some(hdr) = format::decode_record_header(&enc[..n]) else {
                        return Err(StoreError::Corrupt);
                    };
                    if hdr.len as usize != rec.payload.len()
                        || hdr.offset != rec.offset
                        || hdr.flags != rec.flags
                        || hdr.event_id != rec.event_id
                        || hdr.payload_crc != format::crc32(&rec.payload)
                    {
                        return Err(StoreError::Corrupt);
                    }
                }
                if seg.sealed {
                    if let Some(digest) = seg.digest {
                        let d = h.finalize();
                        let mut sha = [0u8; 32];
                        sha.copy_from_slice(&d);
                        if sha != digest {
                            return Err(StoreError::Corrupt);
                        }
                    }
                }
                let expect_bytes: u64 = seg
                    .records
                    .iter()
                    .map(|r| encoded_record_len(r.payload.len()) as u64)
                    .sum();
                if expect_bytes != seg.data_bytes
                    || expect_bytes > self.seg_capacity
                    || seg.next_offset != seg.first_offset + seg.count
                {
                    return Err(StoreError::Corrupt);
                }
            }
        }
        Ok(RecoveryReport {
            segments_scanned,
            valid_records,
            truncated_records: 0,
            truncated_bytes: 0,
            bad_segments: 0,
            offset_gaps: 0,
            resealed_segments: 0,
        })
    }

    fn watermarks(&self) -> WatermarkReport {
        let allocated: u64 = self.parts.values().map(|p| p.segments.len() as u64).sum();
        let used = FIXED_BYTES + allocated * u64::from(self.config.segment_bytes);
        let mut partitions = Vec::new();
        for (&pid, part) in &self.parts {
            let log_bytes = part.segments.iter().map(|s| s.data_bytes).sum::<u64>()
                + part
                    .staged
                    .iter()
                    .map(|r| encoded_record_len(r.payload.len()) as u64)
                    .sum::<u64>();
            let cap = part.segments.len() as u64 * u64::from(self.config.segment_bytes);
            let used_permille = ((log_bytes.saturating_mul(1000))
                .checked_div(cap)
                .unwrap_or(0))
            .min(1000) as u32;
            partitions.push(PartitionWatermark {
                partition: pid,
                log_bytes,
                capacity_bytes: cap,
                used_permille,
            });
        }
        WatermarkReport {
            capacity_bytes: self.capacity_bytes,
            used_bytes: used,
            used_permille: self.used_permille(),
            partitions,
        }
    }

    fn delete_full_segments_before(
        &mut self,
        partition: PartitionId,
        keep_after_offset: LogOffset,
        max_segments: usize,
    ) -> Result<Vec<DeletedSegment>, StoreError> {
        let Some(part) = self.parts.get_mut(&partition) else {
            return Ok(Vec::new());
        };
        let mut deleted = Vec::new();
        loop {
            if deleted.len() >= max_segments {
                break; // 单次预算：多窗摊销（REG-70.15-PRUNE-MEGAOP）
            }
            let Some(seg) = part.segments.first() else {
                break;
            };
            if !seg.sealed || seg.next_offset > keep_after_offset {
                break;
            }
            deleted.push(DeletedSegment {
                partition,
                segment_id: seg.segment_id,
                first_offset: seg.first_offset,
                last_offset: seg.next_offset - 1,
                record_count: seg.count,
                digest: seg.digest.unwrap_or([0u8; 32]),
            });
            part.segments.remove(0);
        }
        Ok(deleted)
    }

    fn flush(&mut self) -> Result<(), StoreError> {
        let parts: Vec<PartitionId> = self.parts.keys().copied().collect();
        for p in parts {
            self.write_staged(p)?;
        }
        Ok(())
    }

    fn staged_records(&self, partition: PartitionId) -> u64 {
        self.parts
            .get(&partition)
            .map(|p| p.staged.len() as u64)
            .unwrap_or(0)
    }
}
