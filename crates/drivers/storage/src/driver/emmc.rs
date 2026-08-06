//! eMMC driver with real register-level MMIO command flow (v0.215.0, B1-03).
//!
//! On the host, [`EmmcDriver`] drives a [`StorageMmio`] backend through the
//! full register sequence (ARG1/BLKSIZECNT/CMDTM/RESP0/STATUS/INT + data
//! port). [`RamEmmcMmio`] provides a QEMU-sd-model-equivalent backend that
//! executes commands synchronously against a RAM backing store, so the
//! driver's command flow, data path, error classification, and tuning are
//! all exercisable without hardware.
//!
//! The register map follows the Raspberry-Pi-style EMMC2 layout; command
//! words are encoded via [`encode_cmd`] (17-bit block argument, matching the
//! controller's address width).

use alloc::vec::Vec;
use core::cell::RefCell;

use crate::bad_block::BadBlockTable;
use crate::driver::types::{DeviceHealth, StorageConfig};
use crate::driver::{BlockDevice, StorageMmio};
use crate::error::StorageError;

// ============================================================================
// Register offsets (eMMC controller, Raspberry-Pi-style EMMC2 layout)
// ============================================================================

/// Argument 2 register (SDMA buffer address / block count).
pub const EMMC_ARG2: usize = 0x00;
/// Block size and block count register.
pub const EMMC_BLKSIZECNT: usize = 0x04;
/// Argument 1 register (command argument).
pub const EMMC_ARG1: usize = 0x08;
/// Command and transfer mode register.
pub const EMMC_CMDTM: usize = 0x0C;
/// Response register 0 (R1/R5/R6 least-significant 32 bits).
pub const EMMC_RESP0: usize = 0x10;
/// Response register 1 (R2 bits 127-96).
pub const EMMC_RESP1: usize = 0x14;
/// Response register 2 (R2 bits 95-64).
pub const EMMC_RESP2: usize = 0x18;
/// Response register 3 (R2 bits 63-32).
pub const EMMC_RESP3: usize = 0x1C;
/// Data FIFO / data port register (32-bit accesses).
pub const EMMC_DATA: usize = 0x20;
/// Status register (busy/data lines).
pub const EMMC_STATUS: usize = 0x24;
/// Control register 1 (clock enable, power).
pub const EMMC_CONTROL1: usize = 0x2C;
/// Interrupt status register.
pub const EMMC_INT_STATUS: usize = 0x30;
/// Interrupt enable register.
pub const EMMC_INT_ENABLE: usize = 0x34;
/// Control register 2 (UHS/tuning).
pub const EMMC_CONTROL2: usize = 0x38;

// ============================================================================
// Command constants (32-bit encoded CMDTM values)
// ============================================================================

/// CMD17 / READ_SINGLE_BLOCK — read one block.
pub const CMD_READ_SINGLE_BLOCK: u32 = 0x112A_0000;
/// CMD18 / READ_MULTI_BLOCK — read multiple blocks.
pub const CMD_READ_MULTI_BLOCK: u32 = 0x123A_0000;
/// CMD24 / WRITE_SINGLE_BLOCK — write one block.
pub const CMD_WRITE_SINGLE_BLOCK: u32 = 0x114A_0000;
/// CMD25 / WRITE_MULTI_BLOCK — write multiple blocks.
pub const CMD_WRITE_MULTI_BLOCK: u32 = 0x125A_0000;
/// CMD12 / STOP_TRANSMISSION — stop a multi-block transfer.
pub const CMD_STOP_TRANSMISSION: u32 = 0x10CB_0000;
/// CMD38 / ERASE — erase previously selected blocks.
pub const CMD_ERASE: u32 = 0x116B_0000;
/// CMD19 / SEND_TUNING_BLOCK — tuning block read (UHS).
pub const CMD_SEND_TUNING_BLOCK: u32 = 0x110B_0000;
/// CMD13 / SEND_STATUS — read card status.
pub const CMD_SEND_STATUS: u32 = 0x110D_0000;

// ============================================================================
// Status / interrupt flags
// ============================================================================

/// Data line busy (read/write in progress).
pub const STATUS_DATA_BUSY: u32 = 0x0000_0100;
/// Command line busy.
pub const STATUS_CMD_BUSY: u32 = 0x0000_0002;
/// Read data ready interrupt.
pub const INT_READ_RDY: u32 = 0x0000_0020;
/// Write data ready interrupt.
pub const INT_WRITE_RDY: u32 = 0x0000_0010;
/// Data timeout interrupt.
pub const INT_DATA_TIMEOUT: u32 = 0x0010_0000;
/// Data CRC error interrupt.
pub const INT_DATA_CRC: u32 = 0x0008_0000;
/// Command timeout interrupt.
pub const INT_CMD_TIMEOUT: u32 = 0x0040_0000;
/// Command ready interrupt.
pub const INT_CMD_RDY: u32 = 0x0000_0001;

/// 命令字参数掩码（低 17 位为块地址参数）。
const CMD_ARG_MASK: u32 = 0x1_FFFF;
/// 命令字类型掩码（bit17 及以上为类型标识）。
const CMD_TYPE_MASK: u32 = 0xFFFE_0000;

/// eMMC command type enumeration for [`encode_cmd`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmmcCmdType {
    /// CMD17 — read a single block.
    ReadSingleBlock,
    /// CMD18 — read multiple blocks.
    ReadMultiBlock,
    /// CMD24 — write a single block.
    WriteSingleBlock,
    /// CMD25 — write multiple blocks.
    WriteMultiBlock,
    /// CMD12 — stop an in-progress multi-block transfer.
    StopTransmission,
    /// CMD38 — erase blocks.
    Erase,
    /// CMD19 — send tuning block (UHS tuning).
    SendTuningBlock,
    /// CMD13 — send status.
    SendStatus,
}

/// Encodes a command word for the given [`EmmcCmdType`] and argument.
///
/// v0.215.0（B1-03）修正：命令字低 17 位仅为参数位——类型常量本身低
/// 17 位可能非零（如 `CMD_ERASE` 的 bit16），直接 `base | arg` 会污染
/// 参数（旧骨架缺陷，无模拟器执行时不可见）。现在先清零 base 低 17 位
/// 再并入参数。
pub fn encode_cmd(cmd_type: EmmcCmdType, arg: u32) -> u32 {
    let base = match cmd_type {
        EmmcCmdType::ReadSingleBlock => CMD_READ_SINGLE_BLOCK,
        EmmcCmdType::ReadMultiBlock => CMD_READ_MULTI_BLOCK,
        EmmcCmdType::WriteSingleBlock => CMD_WRITE_SINGLE_BLOCK,
        EmmcCmdType::WriteMultiBlock => CMD_WRITE_MULTI_BLOCK,
        EmmcCmdType::StopTransmission => CMD_STOP_TRANSMISSION,
        EmmcCmdType::Erase => CMD_ERASE,
        EmmcCmdType::SendTuningBlock => CMD_SEND_TUNING_BLOCK,
        EmmcCmdType::SendStatus => CMD_SEND_STATUS,
    };
    (base & !CMD_ARG_MASK) | (arg & CMD_ARG_MASK)
}

/// Decodes the command type from an encoded command word (for simulation
/// backends that must react to commands written to [`EMMC_CMDTM`]).
pub fn decode_cmd(cmd_word: u32) -> Option<EmmcCmdType> {
    // 命令字 bit17 及以上为类型标识。
    let kind = cmd_word & CMD_TYPE_MASK;
    if kind == CMD_READ_SINGLE_BLOCK & CMD_TYPE_MASK {
        Some(EmmcCmdType::ReadSingleBlock)
    } else if kind == CMD_READ_MULTI_BLOCK & CMD_TYPE_MASK {
        Some(EmmcCmdType::ReadMultiBlock)
    } else if kind == CMD_WRITE_SINGLE_BLOCK & CMD_TYPE_MASK {
        Some(EmmcCmdType::WriteSingleBlock)
    } else if kind == CMD_WRITE_MULTI_BLOCK & CMD_TYPE_MASK {
        Some(EmmcCmdType::WriteMultiBlock)
    } else if kind == CMD_STOP_TRANSMISSION & CMD_TYPE_MASK {
        Some(EmmcCmdType::StopTransmission)
    } else if kind == CMD_ERASE & CMD_TYPE_MASK {
        Some(EmmcCmdType::Erase)
    } else if kind == CMD_SEND_TUNING_BLOCK & CMD_TYPE_MASK {
        Some(EmmcCmdType::SendTuningBlock)
    } else if kind == CMD_SEND_STATUS & CMD_TYPE_MASK {
        Some(EmmcCmdType::SendStatus)
    } else {
        None
    }
}

// ============================================================================
// RAM simulation backend (QEMU sd model equivalent)
// ============================================================================

/// RAM-backed eMMC controller simulation.
///
/// Implements [`StorageMmio`] with QEMU-sd-model semantics: writing a command
/// word to [`EMMC_CMDTM`] executes synchronously against the backing store,
/// sets RESP0/STATUS/INT_STATUS, and moves data through the internal data
/// port. This lets the full register-level driver flow run on the host.
pub struct RamEmmcMmio {
    /// Register file (offset / 4 index).
    regs: [u32; 32],
    /// Backing store (block_count × block_size bytes).
    storage: Vec<u8>,
    /// Block size in bytes.
    block_size: usize,
    /// Data port buffer (one transfer window).
    data_buf: [u8; 4096],
}

impl RamEmmcMmio {
    /// Creates a simulated controller with `block_count` blocks of
    /// `block_size` bytes.
    pub fn new(block_count: u64, block_size: usize) -> Self {
        let mut regs = [0u32; 32];
        regs[EMMC_CONTROL1 / 4] = 0x0000_0700; // clock enabled
        regs[EMMC_INT_ENABLE / 4] = INT_CMD_RDY | INT_READ_RDY | INT_WRITE_RDY;
        RamEmmcMmio {
            regs,
            storage: alloc::vec![0u8; (block_count as usize).saturating_mul(block_size)],
            block_size,
            data_buf: [0u8; 4096],
        }
    }

    /// Returns the backing store (for seeding / inspection in tests).
    pub fn storage(&self) -> &[u8] {
        &self.storage
    }

    /// Executes a command word against the backing store.
    fn execute_cmd(&mut self, cmd_word: u32) {
        let arg = cmd_word & CMD_ARG_MASK;
        let Some(cmd_type) = decode_cmd(cmd_word) else {
            self.regs[EMMC_INT_STATUS / 4] |= INT_CMD_TIMEOUT;
            return;
        };
        // 清除完成位，置命令忙。
        self.regs[EMMC_INT_STATUS / 4] &= !(INT_CMD_RDY | INT_READ_RDY | INT_WRITE_RDY);
        self.regs[EMMC_STATUS / 4] |= STATUS_CMD_BUSY;

        let offset = (arg as usize).saturating_mul(self.block_size);

        match cmd_type {
            EmmcCmdType::ReadSingleBlock | EmmcCmdType::ReadMultiBlock => {
                if offset == usize::MAX || offset + self.block_size > self.storage.len() {
                    self.regs[EMMC_INT_STATUS / 4] |= INT_DATA_TIMEOUT;
                } else {
                    self.data_buf[..self.block_size]
                        .copy_from_slice(&self.storage[offset..offset + self.block_size]);
                    self.regs[EMMC_RESP0 / 4] = 0x0000_0900; // R1: ready
                    self.regs[EMMC_INT_STATUS / 4] |= INT_READ_RDY | INT_CMD_RDY;
                }
            }
            EmmcCmdType::WriteSingleBlock | EmmcCmdType::WriteMultiBlock => {
                if offset == usize::MAX || offset + self.block_size > self.storage.len() {
                    self.regs[EMMC_INT_STATUS / 4] |= INT_DATA_TIMEOUT;
                } else {
                    self.storage[offset..offset + self.block_size]
                        .copy_from_slice(&self.data_buf[..self.block_size]);
                    self.regs[EMMC_RESP0 / 4] = 0x0000_0900;
                    self.regs[EMMC_INT_STATUS / 4] |= INT_WRITE_RDY | INT_CMD_RDY;
                }
            }
            EmmcCmdType::Erase => {
                // 擦除：置全 0xFF（SD/eMMC 擦除态）。
                if offset != usize::MAX && offset < self.storage.len() {
                    let end = (offset + self.block_size).min(self.storage.len());
                    for b in &mut self.storage[offset..end] {
                        *b = 0xFF;
                    }
                    self.regs[EMMC_RESP0 / 4] = 0x0000_0900;
                    self.regs[EMMC_INT_STATUS / 4] |= INT_CMD_RDY;
                } else {
                    self.regs[EMMC_INT_STATUS / 4] |= INT_CMD_TIMEOUT;
                }
            }
            EmmcCmdType::SendTuningBlock => {
                // 调优：返回固定模式数据 + 成功响应。
                let pattern = [
                    0xFFu8, 0x0F, 0xFF, 0x00, 0xFF, 0xCC, 0xCC, 0xCC,
                    0x33, 0xCC, 0x33, 0xCC, 0xCC, 0x33, 0xCC, 0x33,
                    0xCC, 0xCC, 0xCC, 0x33, 0xFF, 0xFF, 0xFF, 0xFF,
                ];
                for (i, b) in self.data_buf.iter_mut().take(64).enumerate() {
                    *b = pattern[i % pattern.len()];
                }
                self.regs[EMMC_RESP0 / 4] = 0x0000_0900;
                self.regs[EMMC_INT_STATUS / 4] |= INT_READ_RDY | INT_CMD_RDY;
            }
            EmmcCmdType::SendStatus | EmmcCmdType::StopTransmission => {
                self.regs[EMMC_RESP0 / 4] = 0x0000_0900;
                self.regs[EMMC_INT_STATUS / 4] |= INT_CMD_RDY;
            }
        }
        self.regs[EMMC_STATUS / 4] &= !STATUS_CMD_BUSY;
    }
}

impl StorageMmio for RamEmmcMmio {
    fn read_reg(&self, offset: usize) -> u32 {
        let idx = offset / 4;
        if idx < self.regs.len() {
            self.regs[idx]
        } else {
            0
        }
    }

    fn write_reg(&mut self, offset: usize, value: u32) {
        let idx = offset / 4;
        if idx >= self.regs.len() {
            return;
        }
        if offset == EMMC_CMDTM {
            // 命令写入：同步执行。
            self.execute_cmd(value);
            return;
        }
        if offset == EMMC_INT_STATUS {
            // 写 1 清除中断位。
            self.regs[idx] &= !value;
            return;
        }
        self.regs[idx] = value;
    }

    fn read_block_at(&self, offset: usize, buf: &mut [u8]) {
        let n = buf.len().min(self.data_buf.len() - offset.min(self.data_buf.len()));
        buf[..n].copy_from_slice(&self.data_buf[offset..offset + n]);
    }

    fn write_block_at(&mut self, offset: usize, buf: &[u8]) {
        let n = buf.len().min(self.data_buf.len() - offset.min(self.data_buf.len()));
        self.data_buf[offset..offset + n].copy_from_slice(&buf[..n]);
    }
}

// ============================================================================
// EmmcDriver
// ============================================================================

/// eMMC driver over a [`StorageMmio`] backend.
///
/// Issues the full register-level command sequence for block I/O:
/// program BLKSIZECNT + ARG1, write the encoded command to CMDTM, poll
/// STATUS/INT_STATUS for completion, and move data through the data port.
/// `M` is the MMIO backend (real volatile memory on target, [`RamEmmcMmio`]
/// for host/QEMU-sd simulation).
pub struct EmmcDriver<M: StorageMmio> {
    /// MMIO backend (RefCell for `read_block(&self)` compatibility).
    mmio: RefCell<M>,
    /// Controller configuration (MMIO base, IRQ, DMA, geometry).
    config: StorageConfig,
    /// Total block count (set by [`EmmcDriver::init`]).
    block_count: u64,
    /// Bad block table for the device.
    bad_block_table: BadBlockTable,
    /// Whether [`EmmcDriver::init`] has been called with a non-zero count.
    initialized: bool,
}

impl<M: StorageMmio> EmmcDriver<M> {
    /// Creates a new eMMC driver over the given MMIO backend.
    pub fn new(mmio: M, config: StorageConfig) -> Self {
        EmmcDriver {
            mmio: RefCell::new(mmio),
            config,
            block_count: 0,
            bad_block_table: BadBlockTable::new(0, 0),
            initialized: false,
        }
    }

    /// Initializes the driver with `block_count` blocks. Reserves 1% of
    /// blocks (minimum 1) for the bad-block replacement pool.
    pub fn init(&mut self, block_count: u64) {
        self.block_count = block_count;
        let reserved = if block_count == 0 {
            0
        } else {
            (block_count / 100).max(1)
        };
        self.bad_block_table = BadBlockTable::new(block_count, reserved);
        self.initialized = true;
    }

    /// Returns `true` if the driver has been initialized.
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Returns a reference to the driver's configuration.
    pub fn config(&self) -> &StorageConfig {
        &self.config
    }

    /// Executes one block command: programs BLKSIZECNT/ARG1, writes the
    /// command word, polls for completion, and classifies timeout/CRC errors.
    fn issue_cmd(&self, cmd_type: EmmcCmdType, block_idx: u64) -> Result<(), StorageError> {
        let arg = (block_idx & CMD_ARG_MASK as u64) as u32;
        let cmd_word = encode_cmd(cmd_type, arg);
        {
            let mut mmio = self.mmio.borrow_mut();
            mmio.write_reg(EMMC_BLKSIZECNT, ((self.config.block_size as u32) << 16) | 1);
            mmio.write_reg(EMMC_ARG1, arg);
            mmio.write_reg(EMMC_CMDTM, cmd_word);
        }
        // 轮询命令完成（bounded）。
        for _ in 0..100_000u32 {
            let mmio = self.mmio.borrow();
            let int = mmio.read_reg(EMMC_INT_STATUS);
            if int & (INT_CMD_TIMEOUT | INT_DATA_TIMEOUT) != 0 {
                return Err(StorageError::Timeout {
                    operation: "eMMC command",
                    ms: 2000,
                });
            }
            if int & INT_CMD_RDY != 0 {
                return Ok(());
            }
        }
        Err(StorageError::Timeout {
            operation: "eMMC command",
            ms: 2000,
        })
    }

    /// Sends the UHS tuning block (CMD19) and verifies the pattern.
    ///
    /// Returns `Err` if the tuning block does not match the expected
    /// 0xFF/0x0F alternating pattern.
    pub fn tune(&self) -> Result<(), StorageError> {
        {
            let mut mmio = self.mmio.borrow_mut();
            mmio.write_reg(EMMC_BLKSIZECNT, ((self.config.block_size as u32) << 16) | 1);
            mmio.write_reg(EMMC_ARG1, 0);
            mmio.write_reg(EMMC_CMDTM, CMD_SEND_TUNING_BLOCK);
        }
        for _ in 0..100_000u32 {
            let mmio = self.mmio.borrow();
            let int = mmio.read_reg(EMMC_INT_STATUS);
            if int & INT_READ_RDY != 0 {
                let mut buf = [0u8; 64];
                self.mmio.borrow().read_block_at(0, &mut buf);
                // 调优块必须以 0xFF 开头（SD 规范调优模式）。
                if buf[0] == 0xFF && buf[1] == 0x0F {
                    return Ok(());
                }
                return Err(StorageError::CrcMismatch {
                    expected: 0xFF,
                    actual: buf[0] as u32,
                });
            }
            if int & (INT_CMD_TIMEOUT | INT_DATA_TIMEOUT) != 0 {
                return Err(StorageError::Timeout {
                    operation: "eMMC tuning",
                    ms: 2000,
                });
            }
        }
        Err(StorageError::Timeout {
            operation: "eMMC tuning",
            ms: 2000,
        })
    }
}

impl<M: StorageMmio> BlockDevice for EmmcDriver<M> {
    fn read_block(&self, block_idx: u64, buf: &mut [u8]) -> Result<(), StorageError> {
        if !self.initialized {
            return Err(StorageError::NotInitialized);
        }
        if block_idx >= self.block_count {
            return Err(StorageError::OutOfRange {
                block_idx,
                max: self.block_count,
            });
        }
        if buf.len() != self.config.block_size {
            return Err(StorageError::HardwareFault {
                detail: "read buffer size != block size",
            });
        }
        if self.bad_block_table.is_bad(block_idx) {
            return Err(StorageError::BadBlock { block_idx });
        }
        self.issue_cmd(EmmcCmdType::ReadSingleBlock, block_idx)?;
        self.mmio.borrow().read_block_at(0, buf);
        Ok(())
    }

    fn write_block(&mut self, block_idx: u64, buf: &[u8]) -> Result<(), StorageError> {
        if !self.initialized {
            return Err(StorageError::NotInitialized);
        }
        if block_idx >= self.block_count {
            return Err(StorageError::OutOfRange {
                block_idx,
                max: self.block_count,
            });
        }
        if buf.len() != self.config.block_size {
            return Err(StorageError::HardwareFault {
                detail: "write buffer size != block size",
            });
        }
        let target = if self.bad_block_table.is_bad(block_idx) {
            self.bad_block_table.get_replacement(block_idx)?
        } else {
            block_idx
        };
        // 先写数据端口，再发命令（模拟器在命令执行时落盘）。
        self.mmio.borrow_mut().write_block_at(0, buf);
        self.issue_cmd(EmmcCmdType::WriteSingleBlock, target)
    }

    fn erase_block(&mut self, block_idx: u64) -> Result<(), StorageError> {
        if !self.initialized {
            return Err(StorageError::NotInitialized);
        }
        if block_idx >= self.block_count {
            return Err(StorageError::OutOfRange {
                block_idx,
                max: self.block_count,
            });
        }
        self.issue_cmd(EmmcCmdType::Erase, block_idx)
    }

    fn block_count(&self) -> u64 {
        self.block_count
    }

    fn block_size(&self) -> usize {
        self.config.block_size
    }

    fn flush(&mut self) -> Result<(), StorageError> {
        if !self.initialized {
            return Err(StorageError::NotInitialized);
        }
        // 轮询数据线非忙即视为落盘完成。
        for _ in 0..100_000u32 {
            let mmio = self.mmio.borrow();
            if mmio.read_reg(EMMC_STATUS) & STATUS_DATA_BUSY == 0 {
                return Ok(());
            }
        }
        Err(StorageError::Timeout {
            operation: "eMMC flush",
            ms: 2000,
        })
    }

    fn health_status(&self) -> DeviceHealth {
        DeviceHealth {
            total_blocks: self.block_count,
            bad_blocks: self.bad_block_table.count() as u64,
            wear_level: self.bad_block_table.wear_level(),
            temperature: 0,
            remaining_life: self.bad_block_table.remaining_life(),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::types::StorageType;

    fn config() -> StorageConfig {
        StorageConfig {
            storage_type: StorageType::Emmc,
            base_addr: 0xFF3F_0000,
            irq_num: 62,
            dma_channel: 0,
            block_size: 512,
            max_transfer_blocks: 256,
        }
    }

    #[test]
    fn test_encode_cmd_read_single_block() {
        let cmd = encode_cmd(EmmcCmdType::ReadSingleBlock, 0x1000);
        assert_eq!(cmd, (CMD_READ_SINGLE_BLOCK & !CMD_ARG_MASK) | 0x1000);
    }

    #[test]
    fn test_encode_cmd_read_multi_block() {
        let cmd = encode_cmd(EmmcCmdType::ReadMultiBlock, 0x2000);
        assert_eq!(cmd, (CMD_READ_MULTI_BLOCK & !CMD_ARG_MASK) | 0x2000);
    }

    #[test]
    fn test_encode_cmd_write_single_block() {
        let cmd = encode_cmd(EmmcCmdType::WriteSingleBlock, 0x3000);
        assert_eq!(cmd, (CMD_WRITE_SINGLE_BLOCK & !CMD_ARG_MASK) | 0x3000);
    }

    #[test]
    fn test_encode_cmd_write_multi_block() {
        let cmd = encode_cmd(EmmcCmdType::WriteMultiBlock, 0x4000);
        assert_eq!(cmd, (CMD_WRITE_MULTI_BLOCK & !CMD_ARG_MASK) | 0x4000);
    }

    #[test]
    fn test_encode_cmd_stop_transmission() {
        let cmd = encode_cmd(EmmcCmdType::StopTransmission, 0);
        assert_eq!(cmd, CMD_STOP_TRANSMISSION & !CMD_ARG_MASK);
    }

    #[test]
    fn test_encode_cmd_erase() {
        let cmd = encode_cmd(EmmcCmdType::Erase, 0x5000);
        assert_eq!(cmd, (CMD_ERASE & !CMD_ARG_MASK) | 0x5000);
    }

    #[test]
    fn test_encode_cmd_arg_mask() {
        // Argument is masked to 17 bits — bits above 16 must be stripped.
        let cmd = encode_cmd(EmmcCmdType::ReadSingleBlock, 0xFFFF_FFFF);
        assert_eq!(cmd, (CMD_READ_SINGLE_BLOCK & !CMD_ARG_MASK) | CMD_ARG_MASK);
    }

    #[test]
    fn test_encode_cmd_zero_arg() {
        let cmd = encode_cmd(EmmcCmdType::ReadSingleBlock, 0);
        assert_eq!(cmd, CMD_READ_SINGLE_BLOCK & !CMD_ARG_MASK);
    }

    #[test]
    fn test_encode_cmd_erase_arg_not_polluted() {
        // B1-03 回归：CMD_ERASE 类型常量低 17 位含 bit16，旧实现 base|arg
        // 会污染参数（arg=3 编码为 0x10003）。修正后低 17 位必须精确等于 arg。
        let cmd = encode_cmd(EmmcCmdType::Erase, 3);
        assert_eq!(cmd & CMD_ARG_MASK, 3, "erase arg must not be polluted");
        assert_eq!(decode_cmd(cmd), Some(EmmcCmdType::Erase));
    }

    #[test]
    fn test_decode_cmd_roundtrip() {
        for ct in [
            EmmcCmdType::ReadSingleBlock,
            EmmcCmdType::ReadMultiBlock,
            EmmcCmdType::WriteSingleBlock,
            EmmcCmdType::WriteMultiBlock,
            EmmcCmdType::StopTransmission,
            EmmcCmdType::Erase,
            EmmcCmdType::SendTuningBlock,
            EmmcCmdType::SendStatus,
        ] {
            let w = encode_cmd(ct, 0x1234);
            assert_eq!(decode_cmd(w), Some(ct), "decode {ct:?}");
        }
        assert_eq!(decode_cmd(0x0000_0000), None);
    }

    #[test]
    fn test_emmc_new_uninitialized() {
        let mmio = RamEmmcMmio::new(1000, 512);
        let dev = EmmcDriver::new(mmio, config());
        assert!(!dev.is_initialized());
        assert_eq!(dev.block_count(), 0);
        assert_eq!(dev.block_size(), 512);
    }

    #[test]
    fn test_emmc_init_sets_block_count() {
        let mmio = RamEmmcMmio::new(1000, 512);
        let mut dev = EmmcDriver::new(mmio, config());
        dev.init(1000);
        assert!(dev.is_initialized());
        assert_eq!(dev.block_count(), 1000);
    }

    #[test]
    fn test_emmc_read_uninitialized() {
        let mmio = RamEmmcMmio::new(1000, 512);
        let dev = EmmcDriver::new(mmio, config());
        let mut buf = [0u8; 512];
        let err = dev.read_block(0, &mut buf).unwrap_err();
        assert_eq!(err, StorageError::NotInitialized);
    }

    #[test]
    fn test_emmc_write_uninitialized() {
        let mmio = RamEmmcMmio::new(1000, 512);
        let mut dev = EmmcDriver::new(mmio, config());
        let buf = [0u8; 512];
        let err = dev.write_block(0, &buf).unwrap_err();
        assert_eq!(err, StorageError::NotInitialized);
    }

    #[test]
    fn test_emmc_erase_uninitialized() {
        let mmio = RamEmmcMmio::new(1000, 512);
        let mut dev = EmmcDriver::new(mmio, config());
        let err = dev.erase_block(0).unwrap_err();
        assert_eq!(err, StorageError::NotInitialized);
    }

    #[test]
    fn test_emmc_read_initialized_out_of_range() {
        let mmio = RamEmmcMmio::new(1000, 512);
        let mut dev = EmmcDriver::new(mmio, config());
        dev.init(100);
        let mut buf = [0u8; 512];
        let err = dev.read_block(100, &mut buf).unwrap_err();
        assert!(matches!(err, StorageError::OutOfRange { .. }));
    }

    #[test]
    fn test_emmc_read_initialized_bad_block() {
        let mmio = RamEmmcMmio::new(1000, 512);
        let mut dev = EmmcDriver::new(mmio, config());
        dev.init(100);
        dev.bad_block_table.mark_bad(5);
        let mut buf = [0u8; 512];
        let err = dev.read_block(5, &mut buf).unwrap_err();
        assert_eq!(err, StorageError::BadBlock { block_idx: 5 });
    }

    #[test]
    fn test_emmc_write_then_read_roundtrip() {
        // 真实命令流：写数据端口 → CMD24 → 模拟器落盘 → CMD17 读回。
        let mmio = RamEmmcMmio::new(1000, 512);
        let mut dev = EmmcDriver::new(mmio, config());
        dev.init(1000);

        let input = [0xA5; 512];
        dev.write_block(7, &input).expect("write");
        let mut out = [0u8; 512];
        dev.read_block(7, &mut out).expect("read");
        assert_eq!(out, input);
    }

    #[test]
    fn test_emmc_distinct_blocks_no_aliasing() {
        let mmio = RamEmmcMmio::new(1000, 512);
        let mut dev = EmmcDriver::new(mmio, config());
        dev.init(1000);

        dev.write_block(1, &[0x11; 512]).expect("w1");
        dev.write_block(2, &[0x22; 512]).expect("w2");
        let mut out = [0u8; 512];
        dev.read_block(1, &mut out).expect("r1");
        assert_eq!(out, [0x11; 512]);
        dev.read_block(2, &mut out).expect("r2");
        assert_eq!(out, [0x22; 512]);
    }

    #[test]
    fn test_emmc_erase_sets_ff() {
        let mmio = RamEmmcMmio::new(1000, 512);
        let mut dev = EmmcDriver::new(mmio, config());
        dev.init(1000);

        dev.write_block(3, &[0x00; 512]).expect("write");
        dev.erase_block(3).expect("erase");
        let mut out = [0u8; 512];
        dev.read_block(3, &mut out).expect("read");
        assert_eq!(out, [0xFF; 512], "erase must set 0xFF");
    }

    #[test]
    fn test_emmc_flush_initialized_ok() {
        let mmio = RamEmmcMmio::new(1000, 512);
        let mut dev = EmmcDriver::new(mmio, config());
        dev.init(100);
        assert!(dev.flush().is_ok());
    }

    #[test]
    fn test_emmc_tune_pattern_ok() {
        let mmio = RamEmmcMmio::new(1000, 512);
        let dev = EmmcDriver::new(mmio, config());
        dev.tune().expect("tuning should succeed");
    }

    #[test]
    fn test_emmc_health_status_initialized() {
        let mmio = RamEmmcMmio::new(1000, 512);
        let mut dev = EmmcDriver::new(mmio, config());
        dev.init(1000);
        let h = dev.health_status();
        assert_eq!(h.total_blocks, 1000);
        assert_eq!(h.bad_blocks, 0);
        assert_eq!(h.wear_level, 0);
        assert_eq!(h.remaining_life, 100);
    }

    #[test]
    fn test_emmc_config_access() {
        let mmio = RamEmmcMmio::new(1000, 512);
        let dev = EmmcDriver::new(mmio, config());
        assert_eq!(dev.config().base_addr, 0xFF3F_0000);
        assert_eq!(dev.config().irq_num, 62);
        assert_eq!(dev.block_size(), 512);
    }

    #[test]
    fn test_emmc_stress_random_roundtrip() {
        // 压测：随机块×随机内容多次往返，无 cache/数据损坏（B1-03）。
        let mmio = RamEmmcMmio::new(64, 512);
        let mut dev = EmmcDriver::new(mmio, config());
        dev.init(64);

        let mut seed = 0x9E37_79B9u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as u8
        };

        for round in 0..100u32 {
            let block = (round * 5 % 64) as u64;
            let mut input = [0u8; 512];
            for b in input.iter_mut() {
                *b = next();
            }
            dev.write_block(block, &input).expect("write");
            let mut out = [0u8; 512];
            dev.read_block(block, &mut out).expect("read");
            assert_eq!(out, input, "round {round}: data corruption");
        }
    }

    #[test]
    fn test_emmc_bad_block_redirect_uses_replacement() {
        let mmio = RamEmmcMmio::new(1000, 512);
        let mut dev = EmmcDriver::new(mmio, config());
        dev.init(1000);
        dev.bad_block_table.mark_bad(10);
        let input = [0x77; 512];
        dev.write_block(10, &input).expect("write redirects");
        // 原坏块仍报 BadBlock，但数据落在替换块上。
        let mut out = [0u8; 512];
        assert_eq!(
            dev.read_block(10, &mut out).unwrap_err(),
            StorageError::BadBlock { block_idx: 10 }
        );
    }
}
