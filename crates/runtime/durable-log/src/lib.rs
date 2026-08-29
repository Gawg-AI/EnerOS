//! EnerOS DurableLog（WP-27.5 / 30 §9 WP-30.7，S5 持久化分层第三层）。
//!
//! 块设备日志：**CRC 真校验 + 原子追加 + 掉电恢复**（复用 22-W3 的
//! CRC/原子写逻辑——代码复用非类型复用）。基于已活体的 virtio-blk 后端
//! （27.3 D4）与 `BlockDevice` trait（host 用 MockBlockDevice 测试）。
//!
//! 本 crate 只提供**附加日志原语**（`BlockDurableLog`）；审计哈希链的
//! 持久化语义唯一出处 = `security/audit`（SPEC-R4 WP-R4.2：112B 摘要
//! 模型的审计存储死副本已删除——掉电后不可查询/恢复，被权威实现取代）。
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
    fn read_block(
        &self,
        block_idx: u64,
        buf: &mut [u8],
    ) -> Result<(), eneros_storage::StorageError> {
        self.0.read_block(block_idx, buf)
    }

    fn write_block(
        &mut self,
        block_idx: u64,
        buf: &[u8],
    ) -> Result<(), eneros_storage::StorageError> {
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

/// 打开已存在日志的错误（**零写**——open_existing 绝不写盘）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenError {
    /// 底层块 I/O 失败。
    Io,
    /// 头块全零（从未格式化——需要显式 provisioning，见
    /// [`BlockDurableLog::provision_new`]；探测阶段保持零写）。
    Uninitialized,
    /// 头块非零但 magic 不匹配（外来格式/其他文件系统）。
    ForeignFormat,
    /// 版本或 CRC 不符（损坏头）。
    Corrupt,
}

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
    /// provisioning 授权校验失败（39 收尾复验：profile/角色/容量
    /// 与授权不符——fail-closed，不写盘）。
    ProvisionDenied,
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
    /// 打开**已存在**的日志（39 WP-39.6：零写——magic 不符绝不写盘）。
    ///
    /// - 头块全零 → `Uninitialized`（需要显式 provisioning）；
    /// - 非零但非本格式 → `ForeignFormat`（不覆盖外来数据）；
    /// - 版本/CRC 不符 → `Corrupt`（只读导出，不静默重建）。
    ///
    /// committed 由只读扫描得出（同 recover 判据，但不重写头块）。
    ///
    /// 39 收尾复验（Exit Gate #2）：只接受**角色分区作用域**
    /// （[`eneros_storage::ScopedBlockDevice`]）——整盘裸 handle
    /// 在类型上无法传入，写面永不触及 LBA0/GPT。
    pub fn open_existing(dev: D, block_size: usize) -> Result<Self, OpenError>
    where
        D: eneros_storage::ScopedBlockDevice,
    {
        let mut log = Self {
            dev,
            block_size,
            data_start: 1,
            tail_seq: 1,
            committed_seq: 0,
        };
        let mut header = [0u8; 512];
        log.dev
            .read_block(0, &mut header[..log.block_size.min(512)])
            .map_err(|_| OpenError::Io)?;
        if header[..6] != LOG_MAGIC {
            if header[..log.block_size.min(512)].iter().all(|b| *b == 0) {
                return Err(OpenError::Uninitialized);
            }
            return Err(OpenError::ForeignFormat);
        }
        let (head, tail) = log.read_header().map_err(|e| match e {
            StoreError::BadHeader => OpenError::Corrupt,
            StoreError::Io => OpenError::Io,
            _ => OpenError::Corrupt,
        })?;
        let _ = head;
        log.tail_seq = tail;
        log.committed_seq = 0;
        // 只读扫描 committed（损坏/空洞即停——与 recover 一致但不写盘）。
        let mut s: Seq = 1;
        while s < tail {
            let block = log.data_start + (s - 1);
            let mut buf = [0u8; 4096];
            log.dev
                .read_block(block, &mut buf[..log.block_size.min(4096)])
                .map_err(|_| OpenError::Io)?;
            let rec_seq = u64::from_le_bytes(buf[0..8].try_into().unwrap());
            let len = u32::from_le_bytes(buf[8..12].try_into().unwrap()) as usize;
            let stored_crc = u32::from_le_bytes(buf[12..16].try_into().unwrap());
            if rec_seq != s
                || len > log.record_capacity()
                || crc32(&buf[16..16 + len]) != stored_crc
            {
                break;
            }
            s += 1;
        }
        log.committed_seq = s - 1;
        Ok(log)
    }

    /// 显式 provisioning 格式化（39 WP-39.3/39.6）——**唯一**带授权
    /// capability 的格式化入口。`auth` 只能由
    /// [`eneros_storage::partition::ProvisioningGate::authorize`] 产出；
    /// normal release profile 构造不出授权（编译期+运行期双门）。
    ///
    /// 39 收尾复验（P0-3）：授权不再被丢弃——执行点再次校验：
    /// ① profile 允许；② 目标作用域角色与授权请求一致；③ 作用域
    /// 容量 ≥ 请求最小值；格式化后执行**操作后 readback**（头块
    /// magic/版本/CRC 回读验证，WP-39.3 条件⑧）。
    pub fn provision_new(
        dev: D,
        block_size: usize,
        auth: &eneros_storage::partition::ProvisionAuthorization,
    ) -> Result<Self, StoreError>
    where
        D: eneros_storage::ScopedBlockDevice,
    {
        if !auth.profile_class().allows_provisioning() {
            return Err(StoreError::ProvisionDenied);
        }
        if dev.scope_role() != auth.request().role {
            return Err(StoreError::ProvisionDenied);
        }
        let capacity = dev.block_count().saturating_mul(block_size as u64);
        if capacity < auth.request().min_capacity_bytes {
            return Err(StoreError::ProvisionDenied);
        }
        let log = Self::format(dev, block_size)?;
        // 条件⑧：操作后 readback——头块回读必须通过 magic/版本/CRC。
        log.read_header().map_err(|e| match e {
            StoreError::BadHeader => StoreError::Corrupt,
            other => other,
        })?;
        Ok(log)
    }

    /// 格式化设备为日志（幂等：已格式化则恢复）。
    ///
    /// 39 收尾复验（P0-3）：**不再公开**——唯一生产入口是带授权
    /// 校验的 [`Self::provision_new`]；本函数仅供 crate 内测试与
    /// provision_new 内部使用（pub(crate) 封堵旁路）。
    pub(crate) fn format(dev: D, block_size: usize) -> Result<Self, StoreError> {
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

    /// 已确认的最大连续序号（open_existing 只读扫描结果；不写盘）。
    pub fn committed_seq(&self) -> Seq {
        self.committed_seq
    }

    /// 释放日志，取回底层设备（测试/迁移用）。
    pub fn into_inner(self) -> D {
        self.dev
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

    /// 单块可容纳的最大记录字节数（16B 头 + payload；每条记录独占一块，
    /// 块内剩余 padding）。block_size < 16 的设备 → 0（任何记录拒绝，
    /// fail-closed）。
    fn record_capacity(&self) -> usize {
        self.block_size.saturating_sub(16).min(4096 - 16)
    }
}

impl<D: BlockDevice> DurableLog for BlockDurableLog<D> {
    fn append(&mut self, rec: &[u8]) -> Result<Seq, StoreError> {
        // 审查修复（30 版后）：记录必须 ≤ 单块容量——旧实现仅查
        // `rec.len() > 0xFFFF_FFFF`，超出 4080B 时 `buf[..record.len()]`
        // 越界 panic、超出块大小时写入被块边界静默截断（append 返回
        // Ok 但数据损坏）。fail-closed：超容量拒绝（不 panic、不截断）。
        let cap = self.record_capacity();
        if rec.len() > cap {
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
        while s < tail {
            // tail = 下一条序号，读 [seq, tail-1]
            let block = self.data_start + (s - 1);
            let mut buf = [0u8; 4096];
            self.dev
                .read_block(block, &mut buf[..self.block_size.min(4096)])
                .map_err(|_| StoreError::Io)?;
            let rec_seq = u64::from_le_bytes(buf[0..8].try_into().unwrap());
            let len = u32::from_le_bytes(buf[8..12].try_into().unwrap()) as usize;
            let stored_crc = u32::from_le_bytes(buf[12..16].try_into().unwrap());
            // 长度上限与 append 的 record_capacity 一致（防半写/损坏记录
            // 的 payload 越界切片——buf 内切片本身安全，超限即 Corrupt）。
            if rec_seq != s || len > self.record_capacity() {
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
            // 长度上限与 append/read_from 一致；超限/CRC 不符即视为
            // 损坏/半写记录，停在该序号（不返回损坏数据）。
            if rec_seq != s
                || len > self.record_capacity()
                || crc32(&buf[16..16 + len]) != stored_crc
            {
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

#[cfg(test)]
mod tests {
    use eneros_storage::partition::BlockRole;
    use eneros_storage::{MockBlockDevice, MockPartitionScope};

    use super::*;

    /// 测试作用域：审计角色分区（启动计数日志等附加日志语义）。
    type AuditScope = MockPartitionScope<MockBlockDevice>;

    fn audit_scope(blocks: u64, block_size: usize) -> AuditScope {
        MockPartitionScope::new(MockBlockDevice::new(blocks, block_size), BlockRole::Audit)
    }

    fn make_log() -> BlockDurableLog<AuditScope> {
        let dev = audit_scope(64, 4096);
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

    /// 审查修复判据（512B 块设备）：记录 ≤ 单块容量（512-16=496B）才可
    /// 追加；497B 拒绝（旧实现静默截断——append 返回 Ok 但数据损坏）。
    #[test]
    fn append_rejects_record_larger_than_block() {
        let dev = audit_scope(64, 512);
        let mut log = BlockDurableLog::format(dev, 512).expect("format");
        let ok = log.append(&[0x5A; 496]).expect("496B 恰好一块，必须成功");
        assert_eq!(ok, 1);
        assert_eq!(log.append(&[0x5A; 497]), Err(StoreError::Full));
    }

    /// 审查修复判据（4096B 块设备）：4080B 恰好容量；4081B 拒绝（旧实现
    /// 在 `buf[..record.len()]` 越界 panic——库公开契约不得 panic）。
    #[test]
    fn append_rejects_record_exceeding_buffer() {
        let dev = audit_scope(64, 4096);
        let mut log = BlockDurableLog::format(dev, 4096).expect("format");
        assert_eq!(log.append(&[0x5A; 4080]).expect("4080B 恰好容量"), 1);
        assert_eq!(log.append(&[0x5A; 4081]), Err(StoreError::Full));
    }

    /// 33 WP-33.5 判据：掉电 ≥20 次恢复（host 模拟）——每次"断电"后
    /// recover committed 单调续增（recover 读磁盘头块/记录区，等价新实例
    /// 重启恢复）；每 5 轮在记录区注入半写脏字节（记录区损坏但头块已
    /// 提交）模拟断电窗口，recover 以已提交序号截断（不返回损坏数据、
    /// 不重复）。
    #[test]
    fn powercut_20_cycles_monotonic_recovery() {
        let dev = audit_scope(256, 4096);
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

    /// 39 WP-39.6：open_existing 语义与零写。
    #[test]
    #[allow(clippy::disallowed_macros)] // 测试内判定 panic（clippy.toml 测试模块豁免）
    fn open_existing_reports_kinds_without_writing() {
        use eneros_storage::partition::BlockTrace;
        // 未初始化（全零）→ Uninitialized
        let traced = BlockTrace::new(audit_scope(64, 4096));
        match BlockDurableLog::open_existing(traced, 4096) {
            Err(OpenError::Uninitialized) => {}
            other => panic!("expected Uninitialized, got {:?}", other.map(|_| ())),
        }
        // 外来格式（非零、非本 magic）→ ForeignFormat
        let mut dev = audit_scope(64, 4096);
        let foreign = [0xFFu8; 4096];
        dev.write_block(0, &foreign).unwrap();
        let traced = BlockTrace::new(dev);
        match BlockDurableLog::open_existing(traced, 4096) {
            Err(OpenError::ForeignFormat) => {}
            other => panic!("expected ForeignFormat, got {:?}", other.map(|_| ())),
        }
        // 本 magic 但版本非法 → Corrupt
        let mut dev = audit_scope(64, 4096);
        let mut bad = [0u8; 4096];
        bad[..6].copy_from_slice(&LOG_MAGIC);
        bad[6..8].copy_from_slice(&99u16.to_le_bytes());
        dev.write_block(0, &bad).unwrap();
        match BlockDurableLog::open_existing(dev, 4096) {
            Err(OpenError::Corrupt) => {}
            other => panic!("expected Corrupt, got {:?}", other.map(|_| ())),
        }
    }

    /// 39 WP-39.6：open_existing 不得重写头块（字节级证明）。
    #[test]
    fn open_existing_does_not_rewrite_header() {
        let dev = audit_scope(64, 4096);
        let log = BlockDurableLog::format(dev, 4096).expect("format");
        let dev = log.into_inner();
        let mut before = [0u8; 4096];
        dev.read_block(0, &mut before).unwrap();
        let log2 = BlockDurableLog::open_existing(dev, 4096).expect("open");
        assert_eq!(log2.committed_seq(), 0);
        let dev = log2.into_inner();
        let mut after = [0u8; 4096];
        dev.read_block(0, &mut after).unwrap();
        assert_eq!(before, after, "open_existing 不得重写头块（零写）");
    }

    /// 39 收尾复验（P0-3）：provision_new 真校验授权——角色不符/
    /// 容量不足拒绝（零写）；合规授权成功且 readback 通过。
    #[allow(clippy::disallowed_macros)] // 测试内判定 panic（clippy.toml 测试模块豁免）
    #[test]
    fn provision_new_validates_authorization() {
        use eneros_storage::partition::{
            BlockIdentity, ProfileClass, ProvisionRequest, ProvisioningGate,
        };
        // factory 授权（Audit 角色，最小容量 4096）。
        let gate = ProvisioningGate::new(ProfileClass::Factory, 1000);
        let mut req = ProvisionRequest {
            target_stable_path: [0u8; 64],
            target_stable_path_len: 4,
            role: BlockRole::Audit,
            min_capacity_bytes: 4096,
            nonce: [9; 16],
            expires_at_unix_s: 2000,
        };
        req.target_stable_path[..4].copy_from_slice(b"disk");
        let mut identity = BlockIdentity {
            device_id: 1,
            stable_path: [0u8; 64],
            stable_path_len: 4,
            capacity_bytes: 64 * 4096,
            logical_sector: 512,
            physical_sector: 512,
            gpt_disk_guid: None,
            media_serial_hash: None,
        };
        identity.stable_path[..4].copy_from_slice(b"disk");
        let auth = gate
            .authorize(&req, &[3u8; 32], &identity, &identity, true)
            .expect("authorize");

        // 角色不符（Data 作用域 vs Audit 授权）→ ProvisionDenied 零写。
        let wrong_role = MockPartitionScope::new(MockBlockDevice::new(64, 4096), BlockRole::Data);
        assert!(matches!(
            BlockDurableLog::provision_new(wrong_role, 4096, &auth),
            Err(StoreError::ProvisionDenied)
        ));
        // 容量不足（min_capacity_bytes 提高后重新授权）。
        let mut req_big = req.clone();
        req_big.nonce = [10; 16];
        req_big.min_capacity_bytes = 64 * 4096 + 1;
        let mut id_big = identity.clone();
        id_big.capacity_bytes = 64 * 4096;
        assert!(
            gate.authorize(&req_big, &[3u8; 32], &id_big, &id_big, true)
                .is_err(),
            "容量不足应在 gate 层拒绝（CapacityBelowMinimum）"
        );

        // 合规路径：Audit 作用域 + 匹配授权 → 成功，readback 通过。
        let dev = audit_scope(64, 4096);
        let log = BlockDurableLog::provision_new(dev, 4096, &auth).expect("provision");
        assert_eq!(log.committed_seq(), 0);
    }
}
