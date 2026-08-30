//! 测试/仿真用块设备后端（pub 供集成测试；host 专用，非生产后端）。
//!
//! - [`MemBlockDevice`]：内存 4KiB 块数组 + `poke`（介质位破坏，CRC 错
//!   注入用）。
//! - [`FaultyBlockDevice`]：包装任意 `D: BlockDevice` 的一次性故障注入——
//!   半写（截断写入：只落前 keep 字节，其余保留旧内容，返回 Io 错误）、
//!   丢写（掉电：整块不落盘）、写后位翻转（静默介质损坏）。
//!
//! 半写语义：块写要么完整落地要么保留旧内容——`TruncateWrite{keep}` 把
//! `buf[0..keep)` 叠加到旧块内容上落盘并返回 `Err`（模拟掉电时 CPU 停转、
//! DMA 只送出一部分），这正是 recover 扫描要截断的"尾块截断"形态。

use alloc::vec::Vec;

use eneros_storage::{BlockDevice, DeviceHealth, StorageError};

/// RAM 块设备：`block_count × block_size` 字节数组（初始全零）。
#[derive(Debug)]
pub struct MemBlockDevice {
    blocks: Vec<Vec<u8>>,
    block_size: usize,
    /// flush 计数（可观测性）。
    flushes: u64,
}

impl MemBlockDevice {
    /// 创建设备（全零内容）。
    pub fn new(block_count: u64, block_size: usize) -> Self {
        MemBlockDevice {
            blocks: (0..block_count)
                .map(|_| alloc::vec![0u8; block_size])
                .collect(),
            block_size,
            flushes: 0,
        }
    }

    /// 直接改介质：把 `bytes` 覆盖写到 `block` 块内 `offset` 处
    /// （绕过任何校验——CRC 破坏/offset 空洞注入用）。
    pub fn poke(&mut self, block: u64, offset: usize, bytes: &[u8]) {
        let b = &mut self.blocks[block as usize];
        b[offset..offset + bytes.len()].copy_from_slice(bytes);
    }

    /// 读原始块内容（测试断言用）。
    pub fn raw_block(&self, block: u64) -> &[u8] {
        &self.blocks[block as usize]
    }

    /// 累计 flush 次数。
    pub fn flush_count(&self) -> u64 {
        self.flushes
    }
}

impl BlockDevice for MemBlockDevice {
    fn read_block(&self, block_idx: u64, buf: &mut [u8]) -> Result<(), StorageError> {
        let max = self.blocks.len() as u64;
        if block_idx >= max {
            return Err(StorageError::OutOfRange { block_idx, max });
        }
        if buf.len() != self.block_size {
            return Err(StorageError::UnsupportedBlockSize);
        }
        buf.copy_from_slice(&self.blocks[block_idx as usize]);
        Ok(())
    }

    fn write_block(&mut self, block_idx: u64, buf: &[u8]) -> Result<(), StorageError> {
        let max = self.blocks.len() as u64;
        if block_idx >= max {
            return Err(StorageError::OutOfRange { block_idx, max });
        }
        if buf.len() != self.block_size {
            return Err(StorageError::UnsupportedBlockSize);
        }
        self.blocks[block_idx as usize].copy_from_slice(buf);
        Ok(())
    }

    fn erase_block(&mut self, block_idx: u64) -> Result<(), StorageError> {
        let max = self.blocks.len() as u64;
        if block_idx >= max {
            return Err(StorageError::OutOfRange { block_idx, max });
        }
        self.blocks[block_idx as usize].fill(0);
        Ok(())
    }

    fn block_count(&self) -> u64 {
        self.blocks.len() as u64
    }

    fn block_size(&self) -> usize {
        self.block_size
    }

    fn flush(&mut self) -> Result<(), StorageError> {
        self.flushes += 1;
        Ok(())
    }

    fn health_status(&self) -> DeviceHealth {
        DeviceHealth {
            total_blocks: self.block_count(),
            bad_blocks: 0,
            wear_level: 0,
            temperature: 0,
            remaining_life: 100,
        }
    }
}

/// 一次性故障注入模式（`arm` 后对下一次 `write_block` 生效一次）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fault {
    /// 无故障。
    #[default]
    None,
    /// 半写（截断写入）：只落 `buf[0..keep)` 到旧块内容上，返回 Io 错误。
    TruncateWrite {
        /// 落盘的前缀字节数。
        keep: usize,
    },
    /// 丢写（掉电）：整块不落盘，返回 Io 错误。
    DropWrite,
    /// 写后位翻转：块正常落盘后翻转 `byte ^ mask`（静默介质损坏，
    /// 返回 Ok——CRC 校验是唯一防线）。
    CorruptAfterWrite {
        /// 块内字节偏移。
        byte: usize,
        /// 异或掩码。
        mask: u8,
    },
}

/// 故障注入包装设备（[`MemBlockDevice`] 或任意真实设备之上）。
#[derive(Debug)]
pub struct FaultyBlockDevice<D: BlockDevice> {
    inner: D,
    armed: Fault,
    injected: u64,
}

impl<D: BlockDevice> FaultyBlockDevice<D> {
    /// 包装设备（初始无故障）。
    pub fn new(inner: D) -> Self {
        FaultyBlockDevice {
            inner,
            armed: Fault::None,
            injected: 0,
        }
    }

    /// 预埋下一次 `write_block` 的故障（一次性）。
    pub fn arm(&mut self, fault: Fault) {
        self.armed = fault;
    }

    /// 当前是否已预埋故障。
    pub fn is_armed(&self) -> bool {
        self.armed != Fault::None
    }

    /// 累计注入次数。
    pub fn injected(&self) -> u64 {
        self.injected
    }

    /// 借用内层设备（直改介质/断言用）。
    pub fn inner(&self) -> &D {
        &self.inner
    }

    /// 可变借用内层设备。
    pub fn inner_mut(&mut self) -> &mut D {
        &mut self.inner
    }

    /// 释放包装，取回内层设备。
    pub fn into_inner(self) -> D {
        self.inner
    }
}

impl<D: BlockDevice> BlockDevice for FaultyBlockDevice<D> {
    fn read_block(&self, block_idx: u64, buf: &mut [u8]) -> Result<(), StorageError> {
        self.inner.read_block(block_idx, buf)
    }

    fn write_block(&mut self, block_idx: u64, buf: &[u8]) -> Result<(), StorageError> {
        let fault = core::mem::replace(&mut self.armed, Fault::None);
        if fault != Fault::None {
            self.injected += 1;
        }
        match fault {
            Fault::None => self.inner.write_block(block_idx, buf),
            Fault::DropWrite => Err(StorageError::HardwareFault {
                detail: "fault injection: dropped write (power cut)",
            }),
            Fault::TruncateWrite { keep } => {
                // 半写：旧块内容保留，仅前 keep 字节被新数据覆盖。
                let mut merged = alloc::vec![0u8; buf.len()];
                self.inner.read_block(block_idx, &mut merged)?;
                let keep = keep.min(buf.len());
                merged[..keep].copy_from_slice(&buf[..keep]);
                self.inner.write_block(block_idx, &merged)?;
                Err(StorageError::HardwareFault {
                    detail: "fault injection: truncated write (power cut)",
                })
            }
            Fault::CorruptAfterWrite { byte, mask } => {
                self.inner.write_block(block_idx, buf)?;
                let mut back = alloc::vec![0u8; buf.len()];
                self.inner.read_block(block_idx, &mut back)?;
                back[byte] ^= mask;
                self.inner.write_block(block_idx, &back)?;
                Ok(())
            }
        }
    }

    fn erase_block(&mut self, block_idx: u64) -> Result<(), StorageError> {
        self.inner.erase_block(block_idx)
    }

    fn block_count(&self) -> u64 {
        self.inner.block_count()
    }

    fn block_size(&self) -> usize {
        self.inner.block_size()
    }

    fn flush(&mut self) -> Result<(), StorageError> {
        self.inner.flush()
    }

    fn health_status(&self) -> DeviceHealth {
        self.inner.health_status()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_device_roundtrip_erase_poke() {
        let mut dev = MemBlockDevice::new(4, 4096);
        let mut buf = [0u8; 4096];
        buf[7] = 0xAB;
        dev.write_block(2, &buf).unwrap();
        let mut out = [0u8; 4096];
        dev.read_block(2, &mut out).unwrap();
        assert_eq!(out[7], 0xAB);
        dev.poke(2, 7, &[0x00]);
        dev.read_block(2, &mut out).unwrap();
        assert_eq!(out[7], 0);
        dev.erase_block(2).unwrap();
        dev.read_block(2, &mut out).unwrap();
        assert!(out.iter().all(|b| *b == 0));
        assert!(matches!(
            dev.read_block(9, &mut out),
            Err(StorageError::OutOfRange { .. })
        ));
        assert!(matches!(
            dev.write_block(0, &[0u8; 512]),
            Err(StorageError::UnsupportedBlockSize)
        ));
    }

    #[test]
    fn faulty_truncate_keeps_old_tail_bytes() {
        let mut dev = FaultyBlockDevice::new(MemBlockDevice::new(2, 4096));
        dev.write_block(1, &[0xEE; 4096]).unwrap();
        // 正常写新内容。
        let mut fresh = [0x11u8; 4096];
        fresh[100] = 0x22;
        dev.write_block(1, &fresh).unwrap();
        let mut out = [0u8; 4096];
        dev.read_block(1, &mut out).unwrap();
        assert_eq!(out[100], 0x22);
        // 半写：只有前 50 字节落地，50..4096 保留旧内容。
        dev.arm(Fault::TruncateWrite { keep: 50 });
        let mut next = [0x33u8; 4096];
        next[100] = 0x44;
        assert!(matches!(
            dev.write_block(1, &next),
            Err(StorageError::HardwareFault { .. })
        ));
        assert_eq!(dev.injected(), 1);
        assert!(!dev.is_armed(), "故障一次性");
        dev.read_block(1, &mut out).unwrap();
        assert_eq!(&out[..50], &[0x33; 50]);
        assert_eq!(out[100], 0x22, "旧内容保留");
        assert_eq!(out[4095], 0x11);
    }

    #[test]
    fn faulty_drop_and_corrupt() {
        let mut dev = FaultyBlockDevice::new(MemBlockDevice::new(2, 4096));
        let buf = [0x5Au8; 4096];
        dev.arm(Fault::DropWrite);
        assert!(matches!(
            dev.write_block(0, &buf),
            Err(StorageError::HardwareFault { .. })
        ));
        let mut out = [0u8; 4096];
        dev.read_block(0, &mut out).unwrap();
        assert!(out.iter().all(|b| *b == 0), "丢写不落盘");
        dev.arm(Fault::CorruptAfterWrite {
            byte: 9,
            mask: 0xFF,
        });
        dev.write_block(0, &buf).unwrap();
        dev.read_block(0, &mut out).unwrap();
        assert_eq!(out[9], 0x5A ^ 0xFF, "静默位翻转");
    }
}
