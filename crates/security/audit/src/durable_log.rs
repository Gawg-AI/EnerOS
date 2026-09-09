//! SPEC-SERIES-2026-08-27/E WP-E.2：SM3/SHA-256 决策审计链的**真持久化**后端。
//!
//! 修复 F-E3 桩：`MemStore` 装配的哈希链注释自称"持久化"而重启即失忆。
//! 本模块把控制事件以 [`DurableAuditRecord`](crate::durable::DurableAuditRecord)
//! （完整事件 + 链字段 + schema/crc）写入真实块区段：
//!
//! - **boot 校验先行**：[`DurableAuditLog::open`] 顺序扫描槽位 → 解码 →
//!   复用既有 [`last_valid_prefix`](crate::durable::last_valid_prefix) /
//!   [`verify_log`](crate::durable::verify_log) 判定——**不发明新校验**：
//!   中间篡改 ⇒ [`OpenVerdict::Corrupt`]（携带违规定位）；掉电尾撕裂 ⇒
//!   [`OpenVerdict::TailTrimmed`]（保留完整前缀）；干净链 ⇒
//!   [`OpenVerdict::Recovered`] 续链（prev = 末条 record_hash）。
//! - **fail-stop 纪律**：Corrupt 后进入降级态——`log` 一律
//!   [`AuditError::StorageWriteFailed`]（停止追加），**不静默重建 genesis**
//!   （重建 = 洗账，禁止）。
//! - **容量策略显式**：槽满丢新 + [`AuditError::BufferFull`] +
//!   `dropped_count()`（对齐 `BoundedMemStore` 既有纪律，词形
//!   `AUDIT-FULL dropped=`）。
//! - **双哈希面分离**：磁盘权威 = record_hash 链（`H(prev ‖ canonical)`，
//!   durable.rs 既有代数）；[`AuditLogger`] trait 面按 entry 代数
//!   （`H(prev ‖ event_hash)`）合成 [`ChainEntry`] 视图供观测——两链输入
//!   同源（同一事件的规范编码），磁盘面为裁决权威。
//!
//! 传输面 [`RegionIo`] 为最小字节区段抽象（read/write），EL0 服务以
//! agent-sdk `BlockIo`（EIPC → svc-storage）适配，host 测试以共享内存
//! 区段模拟断电/篡改。

use alloc::vec::Vec;

use crate::chain::{compute_combined_hash, compute_event_hash, AuditLogger};
use crate::durable::{
    last_valid_prefix, record_from_bytes, record_to_bytes, verify_log, DurableAuditRecord,
    RecordViolation,
};
use crate::events::{AuditEvent, ChainEntry, HashChain};
use crate::AuditError;

/// 传输面：字节区段读写（错误归一 [`RegionError`]；实现方保证区段外读写
/// 返回 Err，不越界）。
pub trait RegionIo {
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), RegionError>;
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), RegionError>;
}

/// 区段 IO 错误（归一面；具体介质语义由实现方日志/诊断承载）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionError {
    /// 读/写失败（介质不可用/越界）。
    Io,
}

/// 槽长（1024B：最大记录 ~575B + len/crc 头尾 + 余量；与 4KB FS 几何无耦合——
/// 本链为裸槽追加，不走 LFS）。
pub const AUDIT_SLOT_LEN: usize = 1024;

/// 槽内记录字节上限（len u16 + record + crc u32，含 6B 头尾）。
pub const AUDIT_SLOT_RECORD_CAP: usize = AUDIT_SLOT_LEN - 6;

/// 开机判定（串口判定行依据；Corrupt = fail-stop 降级）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenVerdict {
    /// 空区段（首次使用）。
    Fresh,
    /// 干净链恢复——续链（`entries` = 恢复条数）。
    Recovered { entries: u64 },
    /// 尾槽撕裂（掉电窗口）——保留完整前缀续链（`kept` = 保留条数）。
    TailTrimmed { kept: u64 },
    /// 中间篡改/链违规——**停止追加**（fail-stop；不重建 genesis）。
    Corrupt(RecordViolation),
}

/// 持久化审计链（实现 [`AuditLogger`]；替换 MemStore 伪持久装配面）。
pub struct DurableAuditLog<Io: RegionIo> {
    io: Io,
    /// 区段起始字节偏移（介质绝对偏移——svc-storage 整盘字节空间）。
    base: u64,
    /// 区段槽容量（region_len / [`AUDIT_SLOT_LEN`]）。
    capacity_slots: usize,
    /// 创世锚（= genesis(domain)，域种子同源复核）。
    anchor: [u8; 32],
    /// 本次启动标识（boot 区分用；informational，入 crc 保护域）。
    boot_id: [u8; 16],
    /// 链头状态（entry 代数视图）。
    chain: HashChain,
    /// 末条 record_hash（record 代数链头——磁盘权威）。
    last_record_hash: [u8; 32],
    /// 降级态（Corrupt 后停止追加）。
    degraded: bool,
    /// 槽满丢新计数。
    dropped: u64,
}

impl<Io: RegionIo> DurableAuditLog<Io> {
    /// 构造（随后必须 [`open`](Self::open)）。
    pub fn new(io: Io, base: u64, region_len: u64, anchor: [u8; 32], boot_id: [u8; 16]) -> Self {
        let capacity_slots = (region_len as usize) / AUDIT_SLOT_LEN;
        DurableAuditLog {
            io,
            base,
            capacity_slots,
            anchor,
            boot_id,
            chain: HashChain {
                last_hash: anchor,
                last_seq: 0,
                entry_count: 0,
                genesis_hash: anchor,
            },
            last_record_hash: anchor,
            degraded: false,
            dropped: 0,
        }
    }

    /// 区段槽容量（容量策略显式可见面）。
    pub fn capacity_slots(&self) -> usize {
        self.capacity_slots
    }

    /// 槽满丢新计数（`AUDIT-FULL dropped=` 词形依据）。
    pub fn dropped_count(&self) -> u64 {
        self.dropped
    }

    /// 开机打开：扫描 → 解码 → 既有校验函数裁决。
    ///
    /// 扫描止于首个全零槽（干净端）或首个解码失败槽（格式损坏端）；
    /// 链级判定全部委托 [`last_valid_prefix`] / [`verify_log`]（既有件，
    /// 非新校验）。裁决规则：
    /// - 链中部密码学违规（前缀 < 存量-1）⇒ [`OpenVerdict::Corrupt`]，
    ///   违规定位由 `verify_log` 给出（中间篡改/删头，fail-stop）；
    /// - **末槽**损坏（前缀 = 存量-1，或格式级解析失败）⇒
    ///   [`OpenVerdict::TailTrimmed`]——掉电撕裂容忍窗（撕裂与末条篡改
    ///   在内容上不可区分；尾部按有损边界处理，下一条追加覆写该槽，
    ///   前缀链始终锚定可验）；
    /// - 首条锚不符（前缀 = 0 非空）⇒ `Corrupt(AnchorMismatch)`
    ///   （删头不可恢复；重建 = 洗账，禁止）。
    pub fn open(&mut self) -> Result<OpenVerdict, RegionError> {
        let mut records: Vec<DurableAuditRecord> = Vec::new();
        let mut tail_format_violation = false;
        for slot in 0..self.capacity_slots {
            let mut buf = [0u8; AUDIT_SLOT_LEN];
            self.io
                .read(self.base + (slot as u64) * AUDIT_SLOT_LEN as u64, &mut buf)?;
            if buf.iter().all(|&b| b == 0) {
                break; // 干净端（未写区 = 全零）。
            }
            match parse_slot(&buf) {
                Some(r) => records.push(r),
                None => {
                    tail_format_violation = true;
                    break; // 格式损坏端——按尾槽损坏裁决。
                }
            }
        }
        let prefix = last_valid_prefix(&records, self.anchor);
        if prefix < records.len() {
            if prefix > 0 && prefix == records.len() - 1 {
                // 末条密码学损坏：撕裂容忍窗——保留完整前缀续链。
                self.adopt(&records[..prefix]);
                return Ok(OpenVerdict::TailTrimmed {
                    kept: prefix as u64,
                });
            }
            // 中部违规 / 删头（前缀 0 非空）：verify_log 精确定位，fail-stop。
            let violation = verify_log(&records, self.anchor)
                .err()
                .unwrap_or(RecordViolation::EmptyLog);
            self.degraded = true;
            return Ok(OpenVerdict::Corrupt(violation));
        }
        if tail_format_violation {
            self.adopt(&records);
            return Ok(OpenVerdict::TailTrimmed {
                kept: records.len() as u64,
            });
        }
        if records.is_empty() {
            return Ok(OpenVerdict::Fresh);
        }
        self.adopt(&records);
        Ok(OpenVerdict::Recovered {
            entries: records.len() as u64,
        })
    }

    /// 以已校验记录集接续链头（entry 视图与 record 视图同步重建）。
    fn adopt(&mut self, records: &[DurableAuditRecord]) {
        let mut prev_entry_hash = self.anchor;
        for r in records {
            let entry_hash = compute_event_hash(&r.event);
            let combined = compute_combined_hash(&prev_entry_hash, &entry_hash);
            prev_entry_hash = combined;
        }
        self.chain = HashChain {
            last_hash: prev_entry_hash,
            last_seq: records.last().map(|r| r.sequence).unwrap_or(0),
            entry_count: records.len() as u64,
            genesis_hash: self.anchor,
        };
        self.last_record_hash = records.last().map(|r| r.record_hash).unwrap_or(self.anchor);
    }

    /// 只读读回区段内已解析记录（校验/对账面；host 测试与重启用）。
    pub fn read_records(&mut self) -> Result<Vec<DurableAuditRecord>, RegionError> {
        let mut out = Vec::new();
        for slot in 0..self.capacity_slots {
            let mut buf = [0u8; AUDIT_SLOT_LEN];
            self.io
                .read(self.base + (slot as u64) * AUDIT_SLOT_LEN as u64, &mut buf)?;
            if buf.iter().all(|&b| b == 0) {
                break;
            }
            match parse_slot(&buf) {
                Some(r) => out.push(r),
                None => break,
            }
        }
        Ok(out)
    }
}

/// 槽解码：[len u16 BE][record bytes]。
///
/// 刻意不做槽级 CRC 门——篡改检出的权威在记录自身 crc/链哈希（durable.rs
/// 既有代数，可**定位**）；槽级门只会把篡改与撕裂混同为解析失败。
fn parse_slot(buf: &[u8; AUDIT_SLOT_LEN]) -> Option<DurableAuditRecord> {
    let len = u16::from_be_bytes([buf[0], buf[1]]) as usize;
    if len == 0 || len > AUDIT_SLOT_RECORD_CAP {
        return None;
    }
    record_from_bytes(&buf[2..2 + len]).ok()
}

/// 槽编码（与 [`parse_slot`] 对称）。
fn encode_slot(record_bytes: &[u8]) -> Option<[u8; AUDIT_SLOT_LEN]> {
    if record_bytes.len() > AUDIT_SLOT_RECORD_CAP {
        return None;
    }
    let mut out = [0u8; AUDIT_SLOT_LEN];
    let len = record_bytes.len();
    out[0..2].copy_from_slice(&(len as u16).to_be_bytes());
    out[2..2 + len].copy_from_slice(record_bytes);
    Some(out)
}

impl<Io: RegionIo> AuditLogger for DurableAuditLog<Io> {
    fn log(&mut self, event: AuditEvent) -> Result<ChainEntry, AuditError> {
        // 降级态：停止追加（fail-stop；不洗账重建）。
        if self.degraded {
            return Err(AuditError::StorageWriteFailed);
        }
        let expect = self.chain.last_seq + 1;
        if event.seq != 0 && event.seq != expect {
            return Err(AuditError::SeqOutOfOrder {
                expect,
                got: event.seq,
            });
        }
        if self.chain.entry_count as usize >= self.capacity_slots {
            // 容量策略：丢新 + 计数（对齐 BoundedMemStore 纪律）。
            self.dropped += 1;
            return Err(AuditError::BufferFull);
        }
        let mut ev = event;
        ev.seq = expect;
        let record = DurableAuditRecord::new(self.last_record_hash, ev.clone(), self.boot_id, None);
        let record_bytes = record_to_bytes(&record);
        let Some(slot) = encode_slot(&record_bytes) else {
            // 超限事件不入链也不静默截断——显式失败（容量上限已含余量，
            // 到达此处即装配错误）。
            return Err(AuditError::StorageWriteFailed);
        };
        let event_hash = compute_event_hash(&ev);
        let combined_hash = compute_combined_hash(&self.chain.last_hash, &event_hash);
        let entry = ChainEntry {
            seq: ev.seq,
            prev_hash: self.chain.last_hash,
            event_hash,
            combined_hash,
            timestamp_ms: ev.timestamp_ms,
        };
        // 先落盘再推进链头：写失败链状态不变（与 AuditLoggerImpl 同序）。
        let slot_off = self.base + self.chain.entry_count * AUDIT_SLOT_LEN as u64;
        self.io
            .write(slot_off, &slot)
            .map_err(|_| AuditError::StorageWriteFailed)?;
        self.chain.last_hash = combined_hash;
        self.chain.last_seq = ev.seq;
        self.chain.entry_count += 1;
        self.last_record_hash = record.record_hash;
        Ok(entry)
    }

    fn log_batch(&mut self, events: &[AuditEvent]) -> Result<Vec<ChainEntry>, AuditError> {
        let mut out = Vec::with_capacity(events.len());
        for ev in events {
            out.push(self.log(ev.clone())?);
        }
        Ok(out)
    }

    fn chain_state(&self) -> &HashChain {
        &self.chain
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)] // 测试断言 panic!（mgmt-cli 先例）
    use alloc::rc::Rc;
    use core::cell::RefCell;

    use super::*;
    use crate::durable::RecordViolation;
    use crate::events::AuditEventType;

    /// 共享内存区段（断电撕裂/篡改注入用；未写区读 = 全零）。
    struct MemRegion {
        media: Rc<RefCell<Vec<u8>>>,
        base: u64,
        len: u64,
    }

    impl MemRegion {
        fn shared(len: u64) -> (Self, Rc<RefCell<Vec<u8>>>) {
            let media = Rc::new(RefCell::new(vec![0u8; len as usize]));
            (
                Self {
                    media: Rc::clone(&media),
                    base: 0,
                    len,
                },
                media,
            )
        }
    }

    impl RegionIo for MemRegion {
        fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), RegionError> {
            let m = self.media.borrow();
            let start = offset as usize;
            if start + buf.len() > (self.base + self.len) as usize {
                return Err(RegionError::Io);
            }
            if start >= m.len() {
                buf.fill(0);
                return Ok(());
            }
            let n = core::cmp::min(buf.len(), m.len() - start);
            buf[..n].copy_from_slice(&m[start..start + n]);
            buf[n..].fill(0);
            Ok(())
        }
        fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), RegionError> {
            let mut m = self.media.borrow_mut();
            let start = offset as usize;
            let end = start + data.len();
            if end > (self.base + self.len) as usize {
                return Err(RegionError::Io);
            }
            if m.len() < end {
                m.resize(end, 0);
            }
            m[start..end].copy_from_slice(data);
            Ok(())
        }
    }

    const ANCHOR: [u8; 32] = [0x5A; 32];
    const REGION_LEN: u64 = 8 * AUDIT_SLOT_LEN as u64; // 8 槽测试容量

    fn event(detail: &str) -> AuditEvent {
        let mut e = AuditEvent::new(
            AuditEventType::WriteExecuted,
            "control-dispatch",
            "pcs",
            detail,
        );
        e.timestamp_ms = 1_700_000_000_000;
        e
    }

    fn fresh(media: &Rc<RefCell<Vec<u8>>>) -> DurableAuditLog<MemRegion> {
        DurableAuditLog::new(
            MemRegion {
                media: Rc::clone(media),
                base: 0,
                len: REGION_LEN,
            },
            0,
            REGION_LEN,
            ANCHOR,
            [0x11; 16],
        )
    }

    /// C1 正例：append×N → 「重启」读回逐条与写入序一致（事件/链字段全同）。
    #[test]
    fn c1_append_then_recover_in_order() {
        let (io, media) = MemRegion::shared(REGION_LEN);
        let mut log = DurableAuditLog::new(io, 0, REGION_LEN, ANCHOR, [7; 16]);
        assert_eq!(log.open().expect("open"), OpenVerdict::Fresh);
        for i in 0..5u64 {
            log.log(event(&alloc::format!("evt-{i}"))).expect("入链");
        }
        drop(log);
        // 重启：新实例同区段。
        let mut log2 = fresh(&media);
        let verdict = log2.open().expect("reopen");
        assert_eq!(verdict, OpenVerdict::Recovered { entries: 5 });
        let records = log2.read_records().expect("readback");
        assert_eq!(records.len(), 5);
        for (i, r) in records.iter().enumerate() {
            assert_eq!(r.sequence, i as u64 + 1);
            assert_eq!(r.event.detail.as_str(), &alloc::format!("evt-{i}"));
        }
        // 链级：verify_log 全绿 + entry 链头衔接续写。
        assert_eq!(verify_log(&records, ANCHOR), Ok(()));
        assert_eq!(log2.chain_state().last_seq, 5);
        let e6 = log2.log(event("evt-5")).expect("续链");
        assert_eq!(e6.seq, 6);
        assert_eq!(log2.chain_state().entry_count, 6, "重启后续链计数连续");
    }

    /// C1 负例：中间破坏一条 ⇒ verify_log 定位失败 → open 判 Corrupt 且停止追加。
    #[test]
    fn c1_mid_corruption_detected_and_fail_stop() {
        let (io, media) = MemRegion::shared(REGION_LEN);
        let mut log = DurableAuditLog::new(io, 0, REGION_LEN, ANCHOR, [7; 16]);
        let _ = log.open();
        for i in 0..4 {
            log.log(event(&alloc::format!("evt-{i}"))).expect("入链");
        }
        drop(log);
        // 篡改第 3 条（槽 2）记录体中一字节（canonical 域内——破坏 record_hash）。
        media.borrow_mut()[2 * AUDIT_SLOT_LEN + 40] ^= 0xFF;
        let mut log2 = fresh(&media);
        let verdict = log2.open().expect("open");
        match &verdict {
            OpenVerdict::Corrupt(v) => match v {
                RecordViolation::HashMismatch { sequence }
                | RecordViolation::CrcMismatch { sequence } => {
                    assert_eq!(*sequence, 3, "定位到被篡改的第 3 条");
                }
                other => panic!("应为 hash/crc 违规，got {other:?}"),
            },
            other => panic!("中间破坏必须 Corrupt，got {other:?}"),
        }
        // fail-stop：降级态拒绝追加。
        assert!(
            log2.log(event("after-fail")).is_err(),
            "Corrupt 后必须停止追加"
        );
    }

    /// C1 负例：截尾 ⇒ last_valid_prefix 收敛点正确（保留完整前缀续链）。
    #[test]
    fn c1_tail_truncation_converges_to_prefix() {
        let (io, media) = MemRegion::shared(REGION_LEN);
        let mut log = DurableAuditLog::new(io, 0, REGION_LEN, ANCHOR, [7; 16]);
        let _ = log.open();
        for i in 0..4 {
            log.log(event(&alloc::format!("evt-{i}"))).expect("入链");
        }
        drop(log);
        // 掉电：末槽撕裂（截到第 4 条记录中段——记录 ~151B，80B 处必断）。
        media.borrow_mut().truncate(3 * AUDIT_SLOT_LEN + 80);
        let mut log2 = fresh(&media);
        assert_eq!(
            log2.open().expect("open"),
            OpenVerdict::TailTrimmed { kept: 3 },
            "收敛点 = 最后完整记录"
        );
        // 前缀续链：seq 从 4 继续。
        let e = log2.log(event("post-restart")).expect("续链");
        assert_eq!(e.seq, 4);
    }

    /// 容量策略：槽满丢新 + 计数（BufferFull），已入链条目完好。
    #[test]
    fn capacity_full_drops_new_with_count() {
        let (io, media) = MemRegion::shared(REGION_LEN);
        let mut log = DurableAuditLog::new(io, 0, REGION_LEN, ANCHOR, [7; 16]);
        let _ = log.open();
        for i in 0..8u64 {
            log.log(event(&alloc::format!("evt-{i}"))).expect("容量内");
        }
        assert!(matches!(
            log.log(event("overflow")),
            Err(AuditError::BufferFull)
        ));
        assert_eq!(log.dropped_count(), 1);
        drop(log);
        let mut log2 = fresh(&media);
        assert_eq!(
            log2.open().expect("open"),
            OpenVerdict::Recovered { entries: 8 },
            "溢出条目不得污染已入链"
        );
    }

    /// 创世复核：锚不符（域种子漂移/删头）⇒ Corrupt(AnchorMismatch)。
    #[test]
    fn anchor_mismatch_is_corrupt_not_rebuild() {
        let (io, media) = MemRegion::shared(REGION_LEN);
        let mut log = DurableAuditLog::new(io, 0, REGION_LEN, ANCHOR, [7; 16]);
        let _ = log.open();
        log.log(event("evt-0")).expect("入链");
        drop(log);
        // 异域种子打开（锚不同 ⇒ 首条 AnchorMismatch）。
        let mut foreign = DurableAuditLog::new(
            MemRegion {
                media: Rc::clone(&media),
                base: 0,
                len: REGION_LEN,
            },
            0,
            REGION_LEN,
            [0x77; 32],
            [7; 16],
        );
        assert_eq!(
            foreign.open().expect("open"),
            OpenVerdict::Corrupt(RecordViolation::AnchorMismatch),
            "异域锚不得续链（重建=洗账，禁止）"
        );
    }

    /// 槽编解码往返 + 位翻转由**记录级**校验检出（记录 crc 覆盖 canonical）。
    #[test]
    fn slot_roundtrip_and_crc_guard() {
        let record = DurableAuditRecord::new(ANCHOR, event("rt"), [3; 16], None);
        let bytes = record_to_bytes(&record);
        let slot = encode_slot(&bytes).expect("encode");
        let back = parse_slot(&slot).expect("decode");
        assert_eq!(back.sequence, record.sequence);
        assert_eq!(back.record_hash, record.record_hash);
        // 篡改 canonical 域内一字节 → 解码仍成功（格式合法），
        // 但记录 crc/链校验必须定位（durable.rs 既有职责）。
        let mut tampered = slot;
        tampered[40] ^= 0x01;
        let rec = parse_slot(&tampered).expect("格式仍可解码");
        assert!(rec.verify(ANCHOR).is_err(), "位翻转必须被记录级校验检出");
        // len 字段破坏（格式级）→ 拒读。
        let mut badlen = slot;
        badlen[0] = 0x7F;
        assert!(parse_slot(&badlen).is_none());
    }
}
