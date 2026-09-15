//! [`BlockEventStore`] — 基于 [`BlockDevice`] 的分区事件日志实现
//! （[07] §4.3 物理格式 + §4.4 恢复 + §4.5 容量保留）。
//!
//! # 布局与不变量
//!
//! 设备布局/段格式/记录格式见 [`crate::format`]（唯一出处）。核心不变量：
//!
//! - **ACK 即 durable 的口径**：Q0（`flags&3==0`，默认批量 1）在 `append`
//!   返回前完成 record 字节落盘 + `dev.flush()`；Q1–Q3 攒批，落盘点为
//!   批量到量（[`StoreConfig::batch_records`]）或显式 [`EventStore::flush`]。
//!   staged 记录仅存 RAM，读路径合并可见，掉电即失（调用方 durable policy
//!   决定是否 ACK，[07] §4.6）。
//! - **记录写入 = 段内字节流 RMW**：record 编码后按段内相对偏移跨块
//!   read-modify-write 写入；半写故障只损伤当前记录自己的字节（旧块内容
//!   保留），recover 按 CRC 截断。
//! - **seal（commit marker）**：段写满旋转时向段尾块写 trailer
//!   （count/last_offset/SHA-256）+ `dev.flush()`；trailer 有效 = 完整段
//!   （retention 只删完整段）。
//! - **recover 扫描**（`scan`）：header 空白 → 空槽；header/CRC/config
//!   digest 坏 → `bad_segments`（槽位 fail-closed 保留）；否则逐 record
//!   校验 len 上限/payload CRC/offset 连续，停于首个坏点并把其后的记录形
//!   残片计入 `truncated_records`（best effort）、剩余字节计入
//!   `truncated_bytes`；trailer 与扫描结果吻合 → 完整段，否则开放段。
//!   陈旧开放段（非尾段且 count>0）由 recover 补写 trailer（`resealed`）。
//! - **consumer 双槽**：世代 u64 单调，块 1/2 交替写，写后读回校验再
//!   flush；撕裂写使新槽无效 → 取高世代有效槽 → 掉电不回退。

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec;
use alloc::vec::Vec;

use eneros_storage::BlockDevice;
use sha2::{Digest, Sha256};

use crate::format::FIXED_BLOCKS;
use crate::format::{
    self, config_digest, decode_offset_slot, decode_record_header, decode_segment_header,
    decode_superblock, decode_trailer, encode_offset_slot, encode_record_into,
    encode_segment_header, encode_superblock, encode_trailer, encoded_record_len, is_blank,
    BLOCK_SIZE, FIXED_BYTES, OFFSET_MAX_ENTRIES, PAYLOAD_MAX, RECORD_HEADER_LEN, RECORD_MAX,
    SEGMENT_HEADER_LEN,
};
use crate::{
    ConsumerGroupId, DeletedSegment, EventStore, LogOffset, PartitionId, PartitionOffset,
    PartitionWatermark, ReadReport, RecordRef, RecordVisitor, RecoveryReport, StoreConfig,
    StoreError, WatermarkReport,
};

/// 槽位状态（RAM 索引；介质为准，scan 重建）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotState {
    Free,
    Bad,
    Segment(PartitionId),
}

/// 段元数据（RAM 索引；对应介质上一个 segment）。
#[derive(Debug)]
struct SegMeta {
    slot: u32,
    segment_id: u32,
    first_offset: LogOffset,
    /// = first_offset + count。
    next_offset: LogOffset,
    /// Σ record 编码字节数。
    data_bytes: u64,
    count: u64,
    sealed: bool,
    digest: Option<[u8; 32]>,
    hasher: Sha256,
}

#[derive(Debug)]
struct StagedRec {
    offset: LogOffset,
    event_id: [u8; 16],
    flags: u16,
    payload: Vec<u8>,
}

#[derive(Debug, Default)]
struct BlockPart {
    /// 按 first_offset 升序。
    segments: Vec<SegMeta>,
    next_offset: LogOffset,
    next_segment_id: u32,
    staged: VecDeque<StagedRec>,
}

/// scan 统计（recover 报告来源）。
#[derive(Debug, Default)]
struct ScanStats {
    segments_scanned: u32,
    valid_records: u64,
    truncated_records: u64,
    truncated_bytes: u64,
    bad_segments: u32,
    offset_gaps: u32,
}

/// 块设备分区事件日志。
///
/// `D` 为 [`BlockDevice`]（block_size 必须为 4096）。生产接线应传入角色
/// 分区作用域句柄的适配（见 crate 文档 §1）；`format` 是显式 provisioning
/// 入口（覆盖写），`open` 为零写打开。
#[derive(Debug)]
pub struct BlockEventStore<D: BlockDevice> {
    /// 底层设备（测试/装配面故障注入入口——`FaultyBlockDevice` 包装）。
    pub dev: D,
    config: StoreConfig,
    geo: format::Geo,
    slots: Vec<SlotState>,
    parts: BTreeMap<PartitionId, BlockPart>,
    consumers: BTreeMap<ConsumerGroupId, BTreeMap<PartitionId, LogOffset>>,
    consumer_gen: u64,
}

impl<D: BlockDevice> BlockEventStore<D> {
    /// 显式 provisioning：格式化设备（**覆盖写**，调用方负责目标分区授权；
    /// 参照 durable-log 的分工——授权 capability 门在调用方/存储栈）。
    pub fn format(mut dev: D, config: StoreConfig) -> Result<Self, StoreError> {
        if dev.block_size() != BLOCK_SIZE {
            return Err(StoreError::BadConfig);
        }
        config.validate()?;
        let geo = format::Geo::new(&config, dev.block_count()).ok_or(StoreError::BadConfig)?;
        let superblock = encode_superblock(&config, (config.clock_unix_s)());
        dev.write_block(0, &superblock_pad(superblock))
            .map_err(|_| StoreError::Io)?;
        for b in 1..FIXED_BLOCKS {
            dev.erase_block(b).map_err(|_| StoreError::Io)?;
        }
        dev.flush().map_err(|_| StoreError::Io)?;
        let mut store = BlockEventStore {
            dev,
            config,
            geo,
            slots: vec![SlotState::Free; geo.num_slots as usize],
            parts: BTreeMap::new(),
            consumers: BTreeMap::new(),
            consumer_gen: 0,
        };
        store.scan()?;
        Ok(store)
    }

    /// 打开已存在 store（**零写**——只读扫描建立索引；恢复截断由
    /// [`EventStore::recover`] 显式执行）。
    pub fn open(dev: D, config: StoreConfig) -> Result<Self, StoreError> {
        if dev.block_size() != BLOCK_SIZE {
            return Err(StoreError::BadConfig);
        }
        config.validate()?;
        let mut block = [0u8; BLOCK_SIZE];
        dev.read_block(0, &mut block).map_err(|_| StoreError::Io)?;
        let info = match decode_superblock(&block) {
            Ok(info) => info,
            Err(format::SuperDecode::Blank) => return Err(StoreError::NotFormatted),
            Err(format::SuperDecode::Foreign) => return Err(StoreError::ForeignFormat),
            Err(format::SuperDecode::Corrupt) => return Err(StoreError::BadSuperblock),
        };
        if info.segment_bytes != config.segment_bytes
            || info.config_digest != config_digest(&config)
        {
            return Err(StoreError::BadConfig);
        }
        let geo = format::Geo::new(&config, dev.block_count()).ok_or(StoreError::BadConfig)?;
        let mut store = BlockEventStore {
            dev,
            config,
            geo,
            slots: vec![SlotState::Free; geo.num_slots as usize],
            parts: BTreeMap::new(),
            consumers: BTreeMap::new(),
            consumer_gen: 0,
        };
        store.scan()?;
        Ok(store)
    }

    /// 释放 store，取回底层设备。
    pub fn into_inner(self) -> D {
        self.dev
    }

    /// 配置副本。
    pub fn config(&self) -> StoreConfig {
        self.config
    }

    // ------------------------------------------------------------------
    // 设备 I/O：段内相对字节读写（跨块 RMW）
    // ------------------------------------------------------------------

    fn read_seg_rel(&self, slot: u32, rel: u64, out: &mut [u8]) -> Result<(), StoreError> {
        let base = self.geo.slot_base_block(slot);
        let bs = BLOCK_SIZE as u64;
        let mut scratch = [0u8; BLOCK_SIZE];
        let mut done = 0usize;
        while done < out.len() {
            let pos = rel + done as u64;
            let blk = base + pos / bs;
            let off = (pos % bs) as usize;
            let n = (BLOCK_SIZE - off).min(out.len() - done);
            self.dev
                .read_block(blk, &mut scratch)
                .map_err(|_| StoreError::Io)?;
            out[done..done + n].copy_from_slice(&scratch[off..off + n]);
            done += n;
        }
        Ok(())
    }

    fn write_seg_rel(&mut self, slot: u32, rel: u64, data: &[u8]) -> Result<(), StoreError> {
        let base = self.geo.slot_base_block(slot);
        let bs = BLOCK_SIZE as u64;
        let mut scratch = [0u8; BLOCK_SIZE];
        let mut done = 0usize;
        while done < data.len() {
            let pos = rel + done as u64;
            let blk = base + pos / bs;
            let off = (pos % bs) as usize;
            let n = (BLOCK_SIZE - off).min(data.len() - done);
            // RMW：保留块内其他字节（前一条记录的尾部等）。
            self.dev
                .read_block(blk, &mut scratch)
                .map_err(|_| StoreError::Io)?;
            scratch[off..off + n].copy_from_slice(&data[done..done + n]);
            self.dev
                .write_block(blk, &scratch)
                .map_err(|_| StoreError::Io)?;
            done += n;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // 段生命周期
    // ------------------------------------------------------------------

    /// 确保 `partition` 的尾段开放且能容纳 `reclen` 字节；必要时 seal 旧段
    /// 并分配新槽位写 header。失败时 staged 不受影响；新段 header 写失败
    /// 则回退槽位（介质残留由 recover 计 bad_segments）。
    fn ensure_open_segment(
        &mut self,
        partition: PartitionId,
        reclen: u64,
    ) -> Result<(), StoreError> {
        let need_new = match self.parts.get(&partition).and_then(|p| p.segments.last()) {
            None => true,
            Some(seg) => {
                seg.sealed || SEGMENT_HEADER_LEN + seg.data_bytes + reclen > self.geo.data_end()
            }
        };
        if !need_new {
            return Ok(());
        }
        // 1. seal 旧尾段（trailer + flush）。
        let seal_old = self
            .parts
            .get(&partition)
            .and_then(|p| p.segments.last())
            .is_some_and(|seg| !seg.sealed);
        if seal_old {
            self.seal_segment(partition)?;
        }
        // 2. 分配槽位（最低空闲）。
        let Some(slot) = self
            .slots
            .iter()
            .position(|s| *s == SlotState::Free)
            .map(|i| i as u32)
        else {
            return Err(StoreError::Full); // staged 保留，等调用方释放
        };
        let (segment_id, first_offset) = {
            let p = self.parts.get(&partition).expect("part exists");
            (
                p.next_segment_id,
                p.staged.front().expect("front exists").offset,
            )
        };
        let header = encode_segment_header(
            partition,
            segment_id,
            first_offset,
            (self.config.clock_unix_s)(),
            config_digest(&self.config),
        );
        // 3. 写 header；失败回退槽位（RAM 不建段）。
        if let Err(e) = self.write_seg_rel(slot, 0, &header) {
            self.slots[slot as usize] = SlotState::Free;
            return Err(e);
        }
        let p = self.parts.get_mut(&partition).expect("part exists");
        p.segments.push(SegMeta {
            slot,
            segment_id,
            first_offset,
            next_offset: first_offset,
            data_bytes: 0,
            count: 0,
            sealed: false,
            digest: None,
            hasher: Sha256::new(),
        });
        p.next_segment_id += 1;
        self.slots[slot as usize] = SlotState::Segment(partition);
        Ok(())
    }

    /// seal 段（写 trailer = commit marker + flush）。空段不写 trailer
    /// （RAM 标记 sealed；介质上仍开放，reopen 后归位为开放空段）。
    fn seal_segment(&mut self, partition: PartitionId) -> Result<(), StoreError> {
        let (slot, count, last_offset, sha) = {
            let seg = self
                .parts
                .get(&partition)
                .and_then(|p| p.segments.last())
                .expect("segment exists");
            let h = seg.hasher.clone();
            let d = h.finalize();
            let mut sha = [0u8; 32];
            sha.copy_from_slice(&d);
            (seg.slot, seg.count, seg.next_offset.wrapping_sub(1), sha)
        };
        if count > 0 {
            let trailer = encode_trailer(count, last_offset, &sha);
            let pos = self.geo.segment_bytes - BLOCK_SIZE as u64;
            self.write_seg_rel(slot, pos, &trailer)?;
            self.dev.flush().map_err(|_| StoreError::Io)?;
        }
        let seg = self
            .parts
            .get_mut(&partition)
            .and_then(|p| p.segments.last_mut())
            .expect("segment exists");
        seg.sealed = true;
        if count > 0 {
            seg.digest = Some(sha);
        }
        Ok(())
    }

    /// staged 记录落盘（批量协议核心；任何失败保持 staged 完整可重试）。
    fn write_staged(&mut self, partition: PartitionId) -> Result<(), StoreError> {
        while self
            .parts
            .get(&partition)
            .is_some_and(|p| !p.staged.is_empty())
        {
            let reclen = {
                let front = self
                    .parts
                    .get(&partition)
                    .and_then(|p| p.staged.front())
                    .expect("non-empty checked");
                encoded_record_len(front.payload.len()) as u64
            };
            self.ensure_open_segment(partition, reclen)?;
            // 编码到栈缓冲再写（避免跨 borrow；写失败 → staged 完整）。
            let mut enc = [0u8; RECORD_MAX];
            let (slot, pos, n) = {
                let p = self.parts.get(&partition).expect("part exists");
                let rec = p.staged.front().expect("front exists");
                let seg = p.segments.last().expect("segment ensured");
                let n = encode_record_into(
                    &mut enc,
                    rec.offset,
                    rec.event_id,
                    rec.flags,
                    0,
                    &rec.payload,
                );
                (seg.slot, SEGMENT_HEADER_LEN + seg.data_bytes, n)
            };
            self.write_seg_rel(slot, pos, &enc[..n])?;
            self.dev.flush().map_err(|_| StoreError::Io)?;
            let p = self.parts.get_mut(&partition).expect("part exists");
            let _rec = p.staged.pop_front().expect("front exists");
            let seg = p.segments.last_mut().expect("segment ensured");
            seg.hasher.update(&enc[..n]);
            seg.data_bytes += n as u64;
            seg.count += 1;
            seg.next_offset += 1;
        }
        Ok(())
    }

    /// 全局占用 permille（与 mem 实现同公式）。
    fn used_permille(&self) -> u32 {
        let capacity = self.dev.block_count() * self.dev.block_size() as u64;
        let allocated = self.slots.iter().filter(|s| **s != SlotState::Free).count() as u64;
        let used = FIXED_BYTES + allocated * self.geo.segment_bytes;
        (((used * 1000) / capacity).min(1000)) as u32
    }

    // ------------------------------------------------------------------
    // 消费位点双槽
    // ------------------------------------------------------------------

    fn scan_consumers(&mut self) -> Result<(), StoreError> {
        self.consumers.clear();
        self.consumer_gen = 0;
        let mut best: Option<crate::OffsetSlotImage> = None;
        for b in 1..FIXED_BLOCKS {
            let mut block = [0u8; BLOCK_SIZE];
            self.dev
                .read_block(b, &mut block)
                .map_err(|_| StoreError::Io)?;
            if let Some((gen, entries)) = decode_offset_slot(&block) {
                if best.as_ref().map_or(true, |(g, _)| gen > *g) {
                    best = Some((gen, entries));
                }
            }
        }
        if let Some((gen, entries)) = best {
            self.consumer_gen = gen;
            for (group, partition, offset) in entries {
                self.consumers
                    .entry(group)
                    .or_default()
                    .insert(partition, offset);
            }
        }
        Ok(())
    }

    fn revert_commits(
        consumers: &mut BTreeMap<ConsumerGroupId, BTreeMap<PartitionId, LogOffset>>,
        group: ConsumerGroupId,
        backup: &[(PartitionId, Option<LogOffset>)],
    ) {
        let m = match consumers.get_mut(&group) {
            Some(m) => m,
            None => return,
        };
        for (partition, old) in backup {
            match old {
                Some(v) => {
                    m.insert(*partition, *v);
                }
                None => {
                    m.remove(partition);
                }
            }
        }
        if m.is_empty() {
            consumers.remove(&group);
        }
    }

    // ------------------------------------------------------------------
    // 全量扫描（open 零写 / recover 截断）
    // ------------------------------------------------------------------

    fn scan(&mut self) -> Result<ScanStats, StoreError> {
        // 保留 staged（RAM 未落盘数据跨 recover 存活）。
        let old_parts = core::mem::take(&mut self.parts);
        let mut staged_by_part: BTreeMap<PartitionId, VecDeque<StagedRec>> = BTreeMap::new();
        for (pid, part) in old_parts {
            if !part.staged.is_empty() {
                staged_by_part.insert(pid, part.staged);
            }
        }
        self.slots = vec![SlotState::Free; self.geo.num_slots as usize];
        self.parts = BTreeMap::new();
        self.scan_consumers()?;
        let mut stats = ScanStats::default();
        let mut payload_buf = [0u8; PAYLOAD_MAX];
        for slot in 0..self.geo.num_slots {
            let mut block = [0u8; BLOCK_SIZE];
            self.dev
                .read_block(self.geo.slot_base_block(slot), &mut block)
                .map_err(|_| StoreError::Io)?;
            if is_blank(&block[..8]) {
                continue;
            }
            let Some(h) = decode_segment_header(&block) else {
                self.slots[slot as usize] = SlotState::Bad;
                stats.bad_segments += 1;
                continue;
            };
            if h.config_digest != config_digest(&self.config) {
                self.slots[slot as usize] = SlotState::Bad;
                stats.bad_segments += 1;
                continue;
            }
            stats.segments_scanned += 1;
            // 逐 record 校验扫描（len 上限 / CRC / offset 连续）。
            let mut hasher = Sha256::new();
            let mut expected = h.first_offset;
            let mut data_bytes = 0u64;
            let mut count = 0u64;
            let mut broken = false;
            loop {
                let pos = SEGMENT_HEADER_LEN + data_bytes;
                if pos + RECORD_HEADER_LEN as u64 > self.geo.data_end() {
                    break;
                }
                let mut hdr = [0u8; RECORD_HEADER_LEN];
                self.read_seg_rel(slot, pos, &mut hdr)?;
                // 擦除态（全零/全 FF）头 = 数据区终点——不是损坏。
                if is_blank(&hdr) {
                    break;
                }
                let Some(rh) = decode_record_header(&hdr) else {
                    broken = true;
                    break;
                };
                if rh.len as usize > PAYLOAD_MAX
                    || pos + RECORD_HEADER_LEN as u64 + u64::from(rh.len) > self.geo.data_end()
                {
                    broken = true;
                    break;
                }
                self.read_seg_rel(
                    slot,
                    pos + RECORD_HEADER_LEN as u64,
                    &mut payload_buf[..rh.len as usize],
                )?;
                if !format::verify_record_crc(&hdr, &payload_buf[..rh.len as usize], rh.payload_crc)
                {
                    broken = true;
                    break;
                }
                if rh.offset != expected {
                    // offset 空洞：介质级损坏，fail-closed 截断。
                    broken = true;
                    stats.offset_gaps += 1;
                    break;
                }
                hasher.update(hdr);
                hasher.update(&payload_buf[..rh.len as usize]);
                stats.valid_records += 1;
                expected += 1;
                data_bytes += RECORD_HEADER_LEN as u64 + u64::from(rh.len);
                count += 1;
            }
            if broken {
                // best effort：断链记录与其后可解析残片**唯一计位点**
                // （68 三轮复查：主循环 break 处不再 +1——残片步进从同一
                // pos 重启，主循环计数会把断链头记录双计，每坏段恒 +1）。
                let mut pos = SEGMENT_HEADER_LEN + data_bytes;
                loop {
                    if pos + RECORD_HEADER_LEN as u64 > self.geo.data_end() {
                        break;
                    }
                    let mut hdr = [0u8; RECORD_HEADER_LEN];
                    self.read_seg_rel(slot, pos, &mut hdr)?;
                    if is_blank(&hdr) {
                        break;
                    }
                    match decode_record_header(&hdr) {
                        Some(rh)
                            if rh.len as usize <= PAYLOAD_MAX
                                && pos + RECORD_HEADER_LEN as u64 + u64::from(rh.len)
                                    <= self.geo.data_end() =>
                        {
                            stats.truncated_records += 1;
                            pos += RECORD_HEADER_LEN as u64 + u64::from(rh.len);
                        }
                        _ => {
                            stats.truncated_records += 1;
                            break;
                        }
                    }
                }
                stats.truncated_bytes += self.geo.data_end() - (SEGMENT_HEADER_LEN + data_bytes);
            }
            // trailer（commit marker）：与扫描结果吻合 → 完整段。
            let mut sealed = false;
            let mut digest = None;
            let mut trailer_block = [0u8; BLOCK_SIZE];
            self.dev
                .read_block(
                    self.geo.slot_base_block(slot) + self.geo.segment_blocks - 1,
                    &mut trailer_block,
                )
                .map_err(|_| StoreError::Io)?;
            if let Some(t) = decode_trailer(&trailer_block) {
                if count > 0 && t.record_count == count && t.last_offset == expected.wrapping_sub(1)
                {
                    let h = hasher.clone();
                    let d = h.finalize();
                    let mut sha = [0u8; 32];
                    sha.copy_from_slice(&d);
                    if sha == t.sha256 {
                        sealed = true;
                        digest = Some(sha);
                    }
                }
            }
            self.slots[slot as usize] = SlotState::Segment(h.partition);
            let part = self.parts.entry(h.partition).or_default();
            part.segments.push(SegMeta {
                slot,
                segment_id: h.segment_id,
                first_offset: h.first_offset,
                next_offset: expected,
                data_bytes,
                count,
                sealed,
                digest,
                hasher,
            });
        }
        // 每分区段序/游标重建 + staged 重挂（offset 必须与扫描终点连续）。
        for (pid, part) in self.parts.iter_mut() {
            part.segments.sort_by_key(|s| s.first_offset);
            if let Some(last) = part.segments.last() {
                part.next_offset = last.next_offset;
                part.next_segment_id = part
                    .segments
                    .iter()
                    .map(|s| s.segment_id)
                    .max()
                    .unwrap_or(0)
                    .wrapping_add(1);
            }
            if let Some(staged) = staged_by_part.remove(pid) {
                let ok = staged
                    .front()
                    .map_or(true, |r| r.offset == part.next_offset);
                if ok {
                    part.staged = staged;
                }
                // 不连续 → staged 丢弃（fail-closed；未 ACK-durable 数据）。
            }
        }
        Ok(stats)
    }

    /// recover 为陈旧开放段补写 trailer（seal 窗口崩溃的收尾）。
    fn reseal_stale_opens(&mut self) -> Result<u32, StoreError> {
        let mut resealed = 0u32;
        let partitions: Vec<PartitionId> = self.parts.keys().copied().collect();
        for pid in partitions {
            let count = self.parts[&pid].segments.len();
            for idx in 0..count.saturating_sub(1) {
                let need = {
                    let seg = &self.parts[&pid].segments[idx];
                    !seg.sealed && seg.count > 0
                };
                if need {
                    self.seal_segment_at(pid, idx)?;
                    resealed += 1;
                }
            }
        }
        Ok(resealed)
    }

    /// seal 指定段（reseal 用；与 [`Self::seal_segment`] 同判据）。
    fn seal_segment_at(&mut self, partition: PartitionId, index: usize) -> Result<(), StoreError> {
        let (slot, count, last_offset, sha) = {
            let seg = &self.parts[&partition].segments[index];
            let h = seg.hasher.clone();
            let d = h.finalize();
            let mut sha = [0u8; 32];
            sha.copy_from_slice(&d);
            (seg.slot, seg.count, seg.next_offset.wrapping_sub(1), sha)
        };
        let trailer = encode_trailer(count, last_offset, &sha);
        let pos = self.geo.segment_bytes - BLOCK_SIZE as u64;
        self.write_seg_rel(slot, pos, &trailer)?;
        self.dev.flush().map_err(|_| StoreError::Io)?;
        let seg = &mut self.parts.get_mut(&partition).expect("part").segments[index];
        seg.sealed = true;
        seg.digest = Some(sha);
        Ok(())
    }
}

/// superblock 补零成整块写入。
fn superblock_pad(superblock: [u8; format::SUPERBLOCK_BYTES]) -> [u8; BLOCK_SIZE] {
    let mut b = [0u8; BLOCK_SIZE];
    b[..format::SUPERBLOCK_BYTES].copy_from_slice(&superblock);
    b
}

impl<D: BlockDevice> EventStore for BlockEventStore<D> {
    fn partitions(&self) -> alloc::collections::BTreeSet<PartitionId> {
        // 扫描重建的 parts 键（segment 头的 partition 字段）——掉电
        // 重开后 open→scan 已恢复完整分区全集。
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
            if from > 0 {
                return Err(StoreError::OffsetOutOfRange);
            }
            return Ok(ReadReport {
                records_read: 0,
                payload_bytes: 0,
                next_offset: from,
            });
        };
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
        let mut payload_buf = [0u8; PAYLOAD_MAX];
        for seg in &part.segments {
            if seg.next_offset <= from {
                continue;
            }
            if seg.first_offset > expected {
                return Err(StoreError::OffsetGap);
            }
            let end = SEGMENT_HEADER_LEN + seg.data_bytes;
            let mut pos = SEGMENT_HEADER_LEN;
            while pos + RECORD_HEADER_LEN as u64 <= end {
                let mut hdr = [0u8; RECORD_HEADER_LEN];
                self.read_seg_rel(seg.slot, pos, &mut hdr)?;
                // 索引与介质不一致 = 静默损坏（fail-closed，绝不返回数据）。
                let rh = decode_record_header(&hdr).ok_or(StoreError::Corrupt)?;
                if rh.len as usize > PAYLOAD_MAX
                    || pos + RECORD_HEADER_LEN as u64 + u64::from(rh.len) > end
                {
                    return Err(StoreError::Corrupt);
                }
                if rh.offset >= from {
                    if rh.offset != expected {
                        return Err(StoreError::OffsetGap);
                    }
                    self.read_seg_rel(
                        seg.slot,
                        pos + RECORD_HEADER_LEN as u64,
                        &mut payload_buf[..rh.len as usize],
                    )?;
                    if !format::verify_record_crc(
                        &hdr,
                        &payload_buf[..rh.len as usize],
                        rh.payload_crc,
                    ) {
                        return Err(StoreError::Corrupt);
                    }
                    let r = RecordRef {
                        offset: rh.offset,
                        event_id: rh.event_id,
                        flags: rh.flags,
                        schema: rh.schema,
                        payload: &payload_buf[..rh.len as usize],
                    };
                    if !out.record(r) {
                        return Ok(ReadReport {
                            records_read: records,
                            payload_bytes,
                            next_offset: expected,
                        });
                    }
                    records += 1;
                    payload_bytes += u64::from(rh.len);
                    expected += 1;
                }
                pos += RECORD_HEADER_LEN as u64 + u64::from(rh.len);
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
            if !out.record(r) {
                return Ok(ReadReport {
                    records_read: records,
                    payload_bytes,
                    next_offset: expected,
                });
            }
            records += 1;
            payload_bytes += rec.payload.len() as u64;
            expected += 1;
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
        // 容量：双槽承载全部 (group, partition) 条目（≤ OFFSET_MAX_ENTRIES）。
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
        if total + fresh.len() > OFFSET_MAX_ENTRIES {
            return Err(StoreError::CommitTooLarge);
        }
        // 备份 → 应用 → 写槽 → 读回校验 → flush；失败逐项回退。
        let backup: Vec<(PartitionId, Option<LogOffset>)> = offsets
            .iter()
            .map(|o| {
                (
                    o.partition,
                    self.consumers
                        .get(&group)
                        .and_then(|m| m.get(&o.partition))
                        .copied(),
                )
            })
            .collect();
        {
            let m = self.consumers.entry(group).or_default();
            for off in offsets {
                m.insert(off.partition, off.offset);
            }
        }
        let mut entries: Vec<(u32, u32, u64)> = Vec::with_capacity(total + fresh.len());
        for (&g, m) in &self.consumers {
            for (&p, &off) in m {
                entries.push((g, p, off));
            }
        }
        self.consumer_gen += 1;
        let gen = self.consumer_gen;
        let slot_block = 1 + (gen & 1); // 双槽交替（块 1/2）
        let img = encode_offset_slot(gen, &entries);
        let write_result = self
            .dev
            .write_block(slot_block, &img)
            .map_err(|_| StoreError::Io);
        if let Err(e) = write_result {
            Self::revert_commits(&mut self.consumers, group, &backup);
            self.consumer_gen -= 1;
            return Err(e);
        }
        // 写后读回（[07] §4.4 掉电不回退：commit 读回 ≥ 已 commit 值）。
        let mut back = [0u8; BLOCK_SIZE];
        let readback = self
            .dev
            .read_block(slot_block, &mut back)
            .map_err(|_| StoreError::Io)
            .and_then(|_| match decode_offset_slot(&back) {
                Some((g2, e2)) if g2 == gen && e2 == entries => Ok(()),
                _ => Err(StoreError::Io),
            });
        if let Err(e) = readback {
            Self::revert_commits(&mut self.consumers, group, &backup);
            self.consumer_gen -= 1;
            return Err(e);
        }
        // flush 失败：读回已确认落盘，保持状态（不回退），报告错误。
        self.dev.flush().map_err(|_| StoreError::Io)?;
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
        let stats = self.scan()?;
        let resealed = self.reseal_stale_opens()?;
        Ok(RecoveryReport {
            segments_scanned: stats.segments_scanned,
            valid_records: stats.valid_records,
            truncated_records: stats.truncated_records,
            truncated_bytes: stats.truncated_bytes,
            bad_segments: stats.bad_segments,
            offset_gaps: stats.offset_gaps,
            resealed_segments: resealed,
        })
    }

    fn watermarks(&self) -> WatermarkReport {
        let capacity = self.dev.block_count() * self.dev.block_size() as u64;
        let allocated = self.slots.iter().filter(|s| **s != SlotState::Free).count() as u64;
        let used = FIXED_BYTES + allocated * self.geo.segment_bytes;
        let mut partitions = Vec::new();
        for (&pid, part) in &self.parts {
            let log_bytes = part.segments.iter().map(|s| s.data_bytes).sum::<u64>()
                + part
                    .staged
                    .iter()
                    .map(|r| encoded_record_len(r.payload.len()) as u64)
                    .sum::<u64>();
            let cap = part.segments.len() as u64 * self.geo.segment_bytes;
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
            capacity_bytes: capacity,
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
        let mut deleted = Vec::new();
        loop {
            if deleted.len() >= max_segments {
                break; // 单次预算：多窗摊销（REG-70.15-PRUNE-MEGAOP）
            }
            let Some((sealed, next_offset, slot, segment_id, first_offset, count, digest)) = self
                .parts
                .get(&partition)
                .and_then(|p| p.segments.first())
                .map(|s| {
                    (
                        s.sealed,
                        s.next_offset,
                        s.slot,
                        s.segment_id,
                        s.first_offset,
                        s.count,
                        s.digest,
                    )
                })
            else {
                break;
            };
            if !sealed || next_offset > keep_after_offset {
                break; // 只删完整段；开放尾段绝不删
            }
            let base = self.geo.slot_base_block(slot);
            for b in base..base + self.geo.segment_blocks {
                self.dev.erase_block(b).map_err(|_| StoreError::Io)?;
            }
            self.dev.flush().map_err(|_| StoreError::Io)?;
            self.slots[slot as usize] = SlotState::Free;
            deleted.push(DeletedSegment {
                partition,
                segment_id,
                first_offset,
                last_offset: next_offset - 1,
                record_count: count,
                digest: digest.unwrap_or([0u8; 32]),
            });
            self.parts
                .get_mut(&partition)
                .expect("part exists")
                .segments
                .remove(0);
        }
        Ok(deleted)
    }

    fn flush(&mut self) -> Result<(), StoreError> {
        let parts: Vec<PartitionId> = self.parts.keys().copied().collect();
        for p in parts {
            self.write_staged(p)?;
        }
        self.dev.flush().map_err(|_| StoreError::Io)
    }

    fn staged_records(&self, partition: PartitionId) -> u64 {
        self.parts
            .get(&partition)
            .map(|p| p.staged.len() as u64)
            .unwrap_or(0)
    }
}

// alloc_reached_full 目前仅供后续容量策略扩展保留（Full 判定走
// ensure_open_segment 内联逻辑）。
#[allow(dead_code)]
fn _unused(_: &BlockEventStore<crate::memdev::MemBlockDevice>) -> bool {
    unreachable!()
}
