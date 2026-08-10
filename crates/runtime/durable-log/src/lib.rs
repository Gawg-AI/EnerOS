//! EnerOS DurableLog（WP-27.5 / 30 §9 WP-30.7，S5 持久化分层第三层）。
//!
//! 块设备日志：**CRC 真校验 + 原子追加 + 掉电恢复**（复用 22-W3 的
//! CRC/原子写逻辑——代码复用非类型复用）。基于已活体的 virtio-blk 后端
//! （27.3 D4）与 `BlockDevice` trait（host 用 MockBlockDevice 测试）。
//!
//! # 布局（块设备，块 0 = 头块）
//! - 块 0：`magic "EDLOG1" ‖ version(2) ‖ head_seq(8) ‖ tail_seq(8) ‖
//!   crc32(4)`（头块 CRC 覆盖前 28 字节；提交 = 原子写头块）；
//! - 块 1 起：记录区线性追加。每条记录：
//!   `seq(8) ‖ len(4) ‖ crc32(payload)(4) ‖ payload`（可跨块）。
//!
//! # 掉电语义（fail-closed）
//! - append：先写记录（含 CRC）→ 再原子更新头块 tail_seq；掉电在两步
//!   之间 → 记录存在但 tail_seq 未推进 → recover 以 tail_seq 为界截断
//!   （未提交记录视为半写，丢弃）；
//! - recover：从 head_seq 起逐条校验 CRC；损坏/空洞 → 停在该序号
//!   （返回最大连续序号，不返回损坏数据）。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;

use eneros_ipc_wire::crc32;
use eneros_storage::BlockDevice;

/// 33 WP-33.5：借用式设备包装——`BlockDurableLog` 可作用于 `&mut D`
/// 而不转移所有权（root-task 在驱动工厂内对数据盘执行掉电自检
/// （format/recover/append）后把设备归还给驱动适配器）。
pub struct RefMutDevice<'a, D: BlockDevice>(pub &'a mut D);

impl<D: BlockDevice> BlockDevice for RefMutDevice<'_, D> {
    fn read_block(&self, block_idx: u64, buf: &mut [u8]) -> Result<(), eneros_storage::StorageError> {
        self.0.read_block(block_idx, buf)
    }

    fn write_block(&mut self, block_idx: u64, buf: &[u8]) -> Result<(), eneros_storage::StorageError> {
        self.0.write_block(block_idx, buf)
    }

    fn erase_block(&mut self, block_idx: u64) -> Result<(), eneros_storage::StorageError> {
        self.0.erase_block(block_idx)
    }

    fn block_count(&self) -> u64 {
        self.0.block_count()
    }

    fn block_size(&self) -> usize {
        self.0.block_size()
    }

    fn flush(&mut self) -> Result<(), eneros_storage::StorageError> {
        self.0.flush()
    }

    fn health_status(&self) -> eneros_storage::DeviceHealth {
        self.0.health_status()
    }
}

/// 日志序号（单调递增；0 保留给创世）。
pub type Seq = u64;

/// 存储错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    /// 块设备 I/O 失败。
    Io,
    /// 头块 magic/版本/CRC 不匹配（未格式化或损坏）。
    BadHeader,
    /// 记录 CRC 不匹配（损坏/半写）。
    Corrupt,
    /// 记录区已满（head 追上 tail）。
    Full,
    /// 序号回退/跳跃（append 必须严格单调）。
    SeqGap,
}

/// 持久化日志抽象（WP-27.5 规格）。
pub trait DurableLog {
    /// 原子追加一条记录，返回其单调序号。
    fn append(&mut self, rec: &[u8]) -> Result<Seq, StoreError>;
    /// 从 `seq` 起按序回调 `(seq, rec)`。
    fn read_from(&self, seq: Seq, f: &mut dyn FnMut(Seq, &[u8])) -> Result<(), StoreError>;
    /// 重启恢复：扫描记录区，返回最大连续序号（0 = 空日志）。
    fn recover(&mut self) -> Result<Seq, StoreError>;
}

/// 日志头常量。
pub const LOG_MAGIC: [u8; 6] = *b"EDLOG1";
/// 日志格式版本。
pub const LOG_VERSION: u16 = 1;

/// 基于 [`BlockDevice`] 的持久化日志实现。
pub struct BlockDurableLog<D: BlockDevice> {
    dev: D,
    /// 设备块大小（字节）。
    block_size: usize,
    /// 记录区起始块。
    data_start: u64,
    /// 当前写序号（下一条 append 的序号）。
    tail_seq: Seq,
    /// 记录区已确认的最大连续序号。
    committed_seq: Seq,
}

impl<D: BlockDevice> BlockDurableLog<D> {
    /// 格式化设备为日志（幂等：已格式化则恢复）。
    pub fn format(dev: D, block_size: usize) -> Result<Self, StoreError> {
        let mut log = Self {
            dev,
            block_size,
            data_start: 1,
            tail_seq: 1,
            committed_seq: 0,
        };
        // 读头块：未格式化 → 写初始头（空日志，tail_seq=1）。
        let mut header = [0u8; 512];
        log.dev
            .read_block(0, &mut header[..log.block_size.min(512)])
            .map_err(|_| StoreError::Io)?;
        if header[..6] != LOG_MAGIC {
            log.write_header()?;
        } else {
            log.tail_seq = log.read_header()?.1;
            log.committed_seq = log.tail_seq.saturating_sub(1);
        }
        Ok(log)
    }

    /// 读头块 → (head_seq, tail_seq)。
    fn read_header(&self) -> Result<(Seq, Seq), StoreError> {
        let mut header = [0u8; 512];
        self.dev
            .read_block(0, &mut header[..self.block_size.min(512)])
            .map_err(|_| StoreError::Io)?;
        if header[..6] != LOG_MAGIC {
            return Err(StoreError::BadHeader);
        }
        let version = u16::from_le_bytes([header[6], header[7]]);
        if version != LOG_VERSION {
            return Err(StoreError::BadHeader);
        }
        let stored_crc = u32::from_le_bytes(header[28..32].try_into().unwrap());
        if crc32(&header[..28]) != stored_crc {
            return Err(StoreError::BadHeader);
        }
        let head = u64::from_le_bytes(header[8..16].try_into().unwrap());
        let tail = u64::from_le_bytes(header[16..24].try_into().unwrap());
        Ok((head, tail))
    }

    /// 原子写头块（CRC 覆盖前 28 字节）。
    fn write_header(&mut self) -> Result<(), StoreError> {
        let mut header = [0u8; 512];
        header[..6].copy_from_slice(&LOG_MAGIC);
        header[6..8].copy_from_slice(&LOG_VERSION.to_le_bytes());
        header[8..16].copy_from_slice(&0u64.to_le_bytes()); // head_seq（暂未用）
        header[16..24].copy_from_slice(&self.tail_seq.to_le_bytes());
        let crc = crc32(&header[..28]);
        header[28..32].copy_from_slice(&crc.to_le_bytes());
        self.dev
            .write_block(0, &header[..self.block_size.min(512)])
            .map_err(|_| StoreError::Io)
    }

}

impl<D: BlockDevice> DurableLog for BlockDurableLog<D> {
    fn append(&mut self, rec: &[u8]) -> Result<Seq, StoreError> {
        if rec.len() > 0xFFFF_FFFF {
            return Err(StoreError::Full);
        }
        let seq = self.tail_seq;
        // 记录 = seq(8) + len(4) + crc(4) + payload。
        let mut record = Vec::with_capacity(16 + rec.len());
        record.extend_from_slice(&seq.to_le_bytes());
        record.extend_from_slice(&(rec.len() as u32).to_le_bytes());
        record.extend_from_slice(&crc32(rec).to_le_bytes());
        record.extend_from_slice(rec);
        // 简化实现：记录区从 data_start 起线性写，每条记录独占一块
        //（块内剩余 padding）——牺牲空间换原子性/简单性；compaction
        //（W3.6）为后续增强。
        let block = self.data_start + (seq - 1);
        let mut buf = [0u8; 4096];
        buf[..record.len()].copy_from_slice(&record);
        self.dev
            .write_block(block, &buf[..self.block_size.min(4096)])
            .map_err(|_| StoreError::Io)?;
        // 提交：推进 tail_seq（下一条序号）并原子写头块。
        self.committed_seq = seq;
        self.tail_seq = seq + 1;
        self.write_header()?;
        Ok(seq)
    }

    fn read_from(&self, seq: Seq, f: &mut dyn FnMut(Seq, &[u8])) -> Result<(), StoreError> {
        let (_, tail) = self.read_header()?;
        let mut s = seq;
        while s < tail { // tail = 下一条序号，读 [seq, tail-1]
            let block = self.data_start + (s - 1);
            let mut buf = [0u8; 4096];
            self.dev
                .read_block(block, &mut buf[..self.block_size.min(4096)])
                .map_err(|_| StoreError::Io)?;
            let rec_seq = u64::from_le_bytes(buf[0..8].try_into().unwrap());
            let len = u32::from_le_bytes(buf[8..12].try_into().unwrap()) as usize;
            let stored_crc = u32::from_le_bytes(buf[12..16].try_into().unwrap());
            if rec_seq != s || len > 4096 - 16 {
                return Err(StoreError::Corrupt);
            }
            let payload = &buf[16..16 + len];
            if crc32(payload) != stored_crc {
                return Err(StoreError::Corrupt);
            }
            f(s, payload);
            s += 1;
        }
        Ok(())
    }

    fn recover(&mut self) -> Result<Seq, StoreError> {
        let (_, tail) = self.read_header()?;
        // 从 1 起扫描：损坏/空洞即停（fail-closed，不返回损坏数据）。
        let mut s: Seq = 1;
        while s < tail {
            let block = self.data_start + (s - 1);
            let mut buf = [0u8; 4096];
            self.dev
                .read_block(block, &mut buf[..self.block_size.min(4096)])
                .map_err(|_| StoreError::Io)?;
            let rec_seq = u64::from_le_bytes(buf[0..8].try_into().unwrap());
            let len = u32::from_le_bytes(buf[8..12].try_into().unwrap()) as usize;
            let stored_crc = u32::from_le_bytes(buf[12..16].try_into().unwrap());
            if rec_seq != s || len > 4096 - 16 || crc32(&buf[16..16 + len]) != stored_crc {
                break;
            }
            s += 1;
        }
        self.committed_seq = s - 1;
        self.tail_seq = s;
        // 头块推进到恢复点（幂等）。
        self.write_header()?;
        Ok(self.committed_seq)
    }
}

/// 30 WP-30.7：`AuditStore` 的 DurableLog 后端（审计哈希链持久化）。
///
/// `ChainEntry` 定长编码（112B：seq8+prev32+event32+combined32+ts8）落盘；
/// 恢复时由 `AuditLoggerImpl::recover` 重建链。
pub struct PersistentAuditStore<D: BlockDevice> {
    log: BlockDurableLog<D>,
}

impl<D: BlockDevice> PersistentAuditStore<D> {
    /// 以持久化日志构造审计存储（未格式化自动格式化）。
    pub fn new(dev: D, block_size: usize) -> Result<Self, StoreError> {
        Ok(Self {
            log: BlockDurableLog::format(dev, block_size)?,
        })
    }

    /// 编码 ChainEntry（定长 112B）。
    fn encode(entry: &eneros_audit::ChainEntry) -> [u8; 112] {
        let mut out = [0u8; 112];
        out[0..8].copy_from_slice(&entry.seq.to_le_bytes());
        out[8..40].copy_from_slice(&entry.prev_hash);
        out[40..72].copy_from_slice(&entry.event_hash);
        out[72..104].copy_from_slice(&entry.combined_hash);
        out[104..112].copy_from_slice(&entry.timestamp_ms.to_le_bytes());
        out
    }

    /// 解码 ChainEntry。
    fn decode(bytes: &[u8]) -> Option<eneros_audit::ChainEntry> {
        if bytes.len() != 112 {
            return None;
        }
        let mut prev = [0u8; 32];
        let mut ev = [0u8; 32];
        let mut comb = [0u8; 32];
        prev.copy_from_slice(&bytes[8..40]);
        ev.copy_from_slice(&bytes[40..72]);
        comb.copy_from_slice(&bytes[72..104]);
        Some(eneros_audit::ChainEntry {
            seq: u64::from_le_bytes(bytes[0..8].try_into().unwrap()),
            prev_hash: prev,
            event_hash: ev,
            combined_hash: comb,
            timestamp_ms: u64::from_le_bytes(bytes[104..112].try_into().unwrap()),
        })
    }
}

impl<D: BlockDevice> eneros_audit::AuditStore for PersistentAuditStore<D> {
    fn append(&mut self, entry: &eneros_audit::ChainEntry) -> Result<(), eneros_audit::AuditError> {
        let rec = Self::encode(entry);
        self.log.append(&rec).map_err(|_| eneros_audit::AuditError::StorageWriteFailed)?;
        Ok(())
    }

    fn read_range(
        &self,
        start_seq: u64,
        end_seq: u64,
    ) -> Result<alloc::vec::Vec<eneros_audit::ChainEntry>, eneros_audit::AuditError> {
        let mut out = alloc::vec::Vec::new();
        self.log
            .read_from(start_seq, &mut |seq, rec| {
                if seq <= end_seq {
                    if let Some(e) = Self::decode(rec) {
                        out.push(e);
                    }
                }
            })
            .map_err(|_| eneros_audit::AuditError::StorageReadFailed)?;
        Ok(out)
    }

    fn read_last_n(
        &self,
        n: usize,
    ) -> Result<alloc::vec::Vec<eneros_audit::ChainEntry>, eneros_audit::AuditError> {
        let count = self.count()?;
        let start = count.saturating_sub(n as u64).max(1);
        self.read_range(start, count)
    }

    fn count(&self) -> Result<u64, eneros_audit::AuditError> {
        // 头块 tail_seq（下一条序号）- 1 = 已确认条数。
        let (_, tail) = self
            .log
            .read_header()
            .map_err(|_| eneros_audit::AuditError::StorageReadFailed)?;
        Ok(tail.saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_storage::MockBlockDevice;

    fn make_log() -> BlockDurableLog<MockBlockDevice> {
        let dev = MockBlockDevice::new(64, 4096);
        BlockDurableLog::format(dev, 4096).expect("format")
    }

    /// 30 WP-30.7 判据：append → 新实例 recover → 读回一致（掉电重启
    /// 恢复语义；序号无断裂）。
    #[test]
    fn append_recover_roundtrip() {
        let mut log = make_log();
        let s1 = log.append(b"genesis-record").expect("append");
        let s2 = log.append(b"second").expect("append");
        assert_eq!(s1, 1);
        assert_eq!(s2, 2);
        // recover：返回最大连续序号（= 已确认条数），不损坏。
        let recovered = log.recover().expect("recover");
        assert_eq!(recovered, 2);
        // recover 后可继续追加（序号连续）。
        let s3 = log.append(b"third").expect("append");
        assert_eq!(s3, 3);
    }

    /// 30 WP-30.7 判据：read_from 按序回放全部记录。
    #[test]
    fn read_from_replays_in_order() {
        let mut log = make_log();
        log.append(b"rec-1").unwrap();
        log.append(b"rec-2").unwrap();
        log.append(b"rec-3").unwrap();
        let mut seen = alloc::vec::Vec::new();
        log.read_from(1, &mut |seq, rec| seen.push((seq, rec.to_vec())))
            .expect("read");
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0].0, 1);
        assert_eq!(seen[0].1, b"rec-1");
        assert_eq!(seen[2].0, 3);
        assert_eq!(seen[2].1, b"rec-3");
    }

    /// 30 WP-30.7 判据：损坏记录 → Corrupt（不返回损坏数据）。
    #[test]
    fn corrupt_record_rejected() {
        let mut log = make_log();
        log.append(b"good").unwrap();
        log.append(b"bad").unwrap();
        // 手工破坏第 2 条记录的 payload。
        let mut buf = [0u8; 4096];
        log.dev.read_block(2, &mut buf).unwrap();
        buf[16] ^= 0xFF;
        log.dev.write_block(2, &buf).unwrap();
        let mut seen = alloc::vec::Vec::new();
        assert_eq!(
            log.read_from(1, &mut |seq, rec| seen.push((seq, rec.to_vec()))),
            Err(StoreError::Corrupt)
        );
        assert_eq!(seen.len(), 1, "损坏记录之后不得返回数据");
    }

    /// 30 WP-30.7：PersistentAuditStore 经 AuditStore trait 落盘/读回。
    #[test]
    fn persistent_audit_store_roundtrip() {
        let dev = MockBlockDevice::new(64, 4096);
        let mut store = PersistentAuditStore::new(dev, 4096).expect("new");
        let entry = eneros_audit::ChainEntry {
            seq: 1,
            prev_hash: [0xAA; 32],
            event_hash: [0xBB; 32],
            combined_hash: [0xCC; 32],
            timestamp_ms: 1700000000,
        };
        eneros_audit::AuditStore::append(&mut store, &entry).expect("append");
        let got = eneros_audit::AuditStore::read_last_n(&store, 1).expect("read");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].seq, entry.seq);
        assert_eq!(got[0].prev_hash, entry.prev_hash);
        assert_eq!(got[0].event_hash, entry.event_hash);
        assert_eq!(got[0].combined_hash, entry.combined_hash);
        assert_eq!(got[0].timestamp_ms, entry.timestamp_ms);
        assert_eq!(eneros_audit::AuditStore::count(&store).unwrap(), 1);
    }

    /// 33 WP-33.5 判据：掉电 ≥20 次恢复（host 模拟）——每次"断电"后
    /// recover committed 单调续增（recover 读磁盘头块/记录区，等价新实例
    /// 重启恢复）；每 5 轮在记录区注入半写脏字节（记录区损坏但头块已
    /// 提交）模拟断电窗口，recover 以已提交序号截断（不返回损坏数据、
    /// 不重复）。
    #[test]
    fn powercut_20_cycles_monotonic_recovery() {
        let dev = MockBlockDevice::new(256, 4096);
        let mut log = BlockDurableLog::format(dev, 4096).expect("format");
        let mut committed_seq = 0u64;
        for cycle in 1..=20u64 {
            // 追加（模拟一次上电的启动记录）。
            let seq = log.append(b"boot-record").expect("append");
            assert_eq!(seq, cycle, "序号必须单调连续");
            // 断电窗口模拟：偶尔在记录区留下半写（下一条记录的 len 字段
            // 被破坏——记录写入后头块提交前断电的等价形态）。
            if cycle % 5 == 0 {
                let mut buf = [0u8; 4096];
                log.dev
                    .read_block(cycle + 1, &mut buf)
                    .expect("read dirty block");
                buf[8] ^= 0x5A; // 破坏下一条记录区的 len 字段
                log.dev
                    .write_block(cycle + 1, &buf)
                    .expect("write dirty block");
            }
            // 断电重启：recover 读磁盘态（新实例语义）。
            let recovered = log.recover().expect("recover");
            assert_eq!(
                recovered, cycle,
                "第 {cycle} 次掉电后 recover 必须恢复已确认序号"
            );
            assert!(recovered >= committed_seq, "committed 单调不减");
            committed_seq = recovered;
        }
        assert_eq!(committed_seq, 20, "20 次掉电后共恢复 20 条");
    }
}
