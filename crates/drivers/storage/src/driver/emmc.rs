//! eMMC driver skeleton with register map and command encoding.
//!
//! On the host target, [`EmmcDriver`] implements [`BlockDevice`] but returns
//! [`StorageError::NotInitialized`] for all I/O operations — there is no real
//! MMIO hardware. The register constants and [`encode_cmd`] are real and
//! validated, so Phase 3 only needs to wire `StorageMmio` to volatile memory.

use crate::bad_block::BadBlockTable;
use crate::driver::types::{DeviceHealth, StorageConfig};
use crate::driver::BlockDevice;
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
/// Status register (busy/data lines).
pub const EMMC_STATUS: usize = 0x24;
/// Interrupt status register.
pub const EMMC_INT_STATUS: usize = 0x30;

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
}

/// Encodes a command word for the given [`EmmcCmdType`] and argument.
///
/// The argument is masked to 17 bits (the SD/eMMC argument width for block
/// addresses) and OR'd into the base command constant.
pub fn encode_cmd(cmd_type: EmmcCmdType, arg: u32) -> u32 {
    let base = match cmd_type {
        EmmcCmdType::ReadSingleBlock => CMD_READ_SINGLE_BLOCK,
        EmmcCmdType::ReadMultiBlock => CMD_READ_MULTI_BLOCK,
        EmmcCmdType::WriteSingleBlock => CMD_WRITE_SINGLE_BLOCK,
        EmmcCmdType::WriteMultiBlock => CMD_WRITE_MULTI_BLOCK,
        EmmcCmdType::StopTransmission => CMD_STOP_TRANSMISSION,
        EmmcCmdType::Erase => CMD_ERASE,
    };
    base | (arg & 0x1_FFFF)
}

// ============================================================================
// EmmcDriver
// ============================================================================

/// eMMC driver skeleton.
///
/// On the host, all I/O operations return [`StorageError::NotInitialized`].
/// The driver tracks `block_count` and a [`BadBlockTable`] so that the
/// health-reporting path is fully functional even without hardware.
pub struct EmmcDriver {
    /// Controller configuration (MMIO base, IRQ, DMA, geometry).
    config: StorageConfig,
    /// Total block count (set by [`EmmcDriver::init`]).
    block_count: u64,
    /// Bad block table for the device.
    bad_block_table: BadBlockTable,
    /// Whether [`EmmcDriver::init`] has been called with a non-zero count.
    initialized: bool,
}

impl EmmcDriver {
    /// Creates a new eMMC driver from `config`. The driver starts
    /// uninitialized; call [`EmmcDriver::init`] to set the block count.
    pub fn new(config: StorageConfig) -> Self {
        EmmcDriver {
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
    #[allow(dead_code)]
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Returns a reference to the driver's configuration.
    #[allow(dead_code)]
    pub fn config(&self) -> &StorageConfig {
        &self.config
    }
}

impl BlockDevice for EmmcDriver {
    fn read_block(&self, block_idx: u64, _buf: &mut [u8]) -> Result<(), StorageError> {
        if !self.initialized {
            return Err(StorageError::NotInitialized);
        }
        if block_idx >= self.block_count {
            return Err(StorageError::OutOfRange {
                block_idx,
                max: self.block_count,
            });
        }
        if self.bad_block_table.is_bad(block_idx) {
            return Err(StorageError::BadBlock { block_idx });
        }
        // No real MMIO on host — would issue CMD_READ_SINGLE_BLOCK here.
        Err(StorageError::NotInitialized)
    }

    fn write_block(&mut self, block_idx: u64, _buf: &[u8]) -> Result<(), StorageError> {
        if !self.initialized {
            return Err(StorageError::NotInitialized);
        }
        if block_idx >= self.block_count {
            return Err(StorageError::OutOfRange {
                block_idx,
                max: self.block_count,
            });
        }
        if self.bad_block_table.is_bad(block_idx) {
            // Redirect to a replacement block (still no real I/O on host).
            let _ = self.bad_block_table.get_replacement(block_idx)?;
        }
        Err(StorageError::NotInitialized)
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
        Err(StorageError::NotInitialized)
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
        Ok(())
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

    #[test]
    fn test_encode_cmd_read_single_block() {
        let cmd = encode_cmd(EmmcCmdType::ReadSingleBlock, 0x1000);
        assert_eq!(cmd, CMD_READ_SINGLE_BLOCK | 0x1000);
    }

    #[test]
    fn test_encode_cmd_read_multi_block() {
        let cmd = encode_cmd(EmmcCmdType::ReadMultiBlock, 0x2000);
        assert_eq!(cmd, CMD_READ_MULTI_BLOCK | 0x2000);
    }

    #[test]
    fn test_encode_cmd_write_single_block() {
        let cmd = encode_cmd(EmmcCmdType::WriteSingleBlock, 0x3000);
        assert_eq!(cmd, CMD_WRITE_SINGLE_BLOCK | 0x3000);
    }

    #[test]
    fn test_encode_cmd_write_multi_block() {
        let cmd = encode_cmd(EmmcCmdType::WriteMultiBlock, 0x4000);
        assert_eq!(cmd, CMD_WRITE_MULTI_BLOCK | 0x4000);
    }

    #[test]
    fn test_encode_cmd_stop_transmission() {
        let cmd = encode_cmd(EmmcCmdType::StopTransmission, 0);
        assert_eq!(cmd, CMD_STOP_TRANSMISSION);
    }

    #[test]
    fn test_encode_cmd_erase() {
        let cmd = encode_cmd(EmmcCmdType::Erase, 0x5000);
        assert_eq!(cmd, CMD_ERASE | 0x5000);
    }

    #[test]
    fn test_encode_cmd_arg_mask() {
        // Argument is masked to 17 bits — bits above 16 must be stripped.
        let cmd = encode_cmd(EmmcCmdType::ReadSingleBlock, 0xFFFF_FFFF);
        assert_eq!(cmd, CMD_READ_SINGLE_BLOCK | 0x1_FFFF);
    }

    #[test]
    fn test_encode_cmd_zero_arg() {
        let cmd = encode_cmd(EmmcCmdType::ReadSingleBlock, 0);
        assert_eq!(cmd, CMD_READ_SINGLE_BLOCK);
    }

    #[test]
    fn test_emmc_new_uninitialized() {
        let cfg = StorageConfig::default();
        let dev = EmmcDriver::new(cfg);
        assert!(!dev.is_initialized());
        assert_eq!(dev.block_count(), 0);
        assert_eq!(dev.block_size(), 512);
    }

    #[test]
    fn test_emmc_init_sets_block_count() {
        let cfg = StorageConfig::default();
        let mut dev = EmmcDriver::new(cfg);
        dev.init(1000);
        assert!(dev.is_initialized());
        assert_eq!(dev.block_count(), 1000);
    }

    #[test]
    fn test_emmc_read_uninitialized() {
        let cfg = StorageConfig::default();
        let dev = EmmcDriver::new(cfg);
        let mut buf = [0u8; 512];
        let err = dev.read_block(0, &mut buf).unwrap_err();
        assert_eq!(err, StorageError::NotInitialized);
    }

    #[test]
    fn test_emmc_write_uninitialized() {
        let cfg = StorageConfig::default();
        let mut dev = EmmcDriver::new(cfg);
        let buf = [0u8; 512];
        let err = dev.write_block(0, &buf).unwrap_err();
        assert_eq!(err, StorageError::NotInitialized);
    }

    #[test]
    fn test_emmc_erase_uninitialized() {
        let cfg = StorageConfig::default();
        let mut dev = EmmcDriver::new(cfg);
        let err = dev.erase_block(0).unwrap_err();
        assert_eq!(err, StorageError::NotInitialized);
    }

    #[test]
    fn test_emmc_read_initialized_out_of_range() {
        let cfg = StorageConfig::default();
        let mut dev = EmmcDriver::new(cfg);
        dev.init(100);
        let mut buf = [0u8; 512];
        let err = dev.read_block(100, &mut buf).unwrap_err();
        assert!(matches!(err, StorageError::OutOfRange { .. }));
    }

    #[test]
    fn test_emmc_read_initialized_in_range_no_hardware() {
        let cfg = StorageConfig::default();
        let mut dev = EmmcDriver::new(cfg);
        dev.init(100);
        let mut buf = [0u8; 512];
        // No real MMIO on host → NotInitialized.
        let err = dev.read_block(0, &mut buf).unwrap_err();
        assert_eq!(err, StorageError::NotInitialized);
    }

    #[test]
    fn test_emmc_flush_uninitialized() {
        let cfg = StorageConfig::default();
        let mut dev = EmmcDriver::new(cfg);
        let err = dev.flush().unwrap_err();
        assert_eq!(err, StorageError::NotInitialized);
    }

    #[test]
    fn test_emmc_flush_initialized_ok() {
        let cfg = StorageConfig::default();
        let mut dev = EmmcDriver::new(cfg);
        dev.init(100);
        assert!(dev.flush().is_ok());
    }

    #[test]
    fn test_emmc_health_status_initialized() {
        let cfg = StorageConfig::default();
        let mut dev = EmmcDriver::new(cfg);
        dev.init(1000);
        let h = dev.health_status();
        assert_eq!(h.total_blocks, 1000);
        assert_eq!(h.bad_blocks, 0);
        assert_eq!(h.wear_level, 0);
        assert_eq!(h.remaining_life, 100);
    }

    #[test]
    fn test_emmc_config_access() {
        let cfg = StorageConfig {
            storage_type: StorageType::Emmc,
            base_addr: 0xDEAD_BEEF,
            irq_num: 99,
            dma_channel: 7,
            block_size: 4096,
            max_transfer_blocks: 32,
        };
        let dev = EmmcDriver::new(cfg.clone());
        assert_eq!(dev.config().base_addr, 0xDEAD_BEEF);
        assert_eq!(dev.config().irq_num, 99);
        assert_eq!(dev.block_size(), 4096);
    }
}
