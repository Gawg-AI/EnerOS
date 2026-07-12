//! NVMe driver skeleton with controller register map.
//!
//! On the host target, [`NvmeDriver`] implements [`BlockDevice`] but returns
//! [`StorageError::NotInitialized`] for all I/O operations — there is no real
//! PCIe MMIO hardware. The register constants are real and validated, so
//! Phase 3 only needs to wire `StorageMmio` to the PCIe BAR mapping.

use crate::bad_block::BadBlockTable;
use crate::driver::types::{DeviceHealth, StorageConfig};
use crate::driver::BlockDevice;
use crate::error::StorageError;

// ============================================================================
// NVMe Controller Register offsets (per NVMe 1.4 spec, §3.1)
// ============================================================================

/// Controller Capabilities (CAP).
pub const NVME_CAP: usize = 0x000;
/// Version (VS).
pub const NVME_VS: usize = 0x008;
/// Interrupt Mask Set (INTMS).
pub const NVME_INTMS: usize = 0x00C;
/// Interrupt Mask Clear (INTMC).
pub const NVME_INTMC: usize = 0x010;
/// Controller Configuration (CC).
pub const NVME_CC: usize = 0x014;
/// Controller Status (CSTS).
pub const NVME_CSTS: usize = 0x01C;
/// NSS Reset (NSSR).
pub const NVME_NSSR: usize = 0x020;
/// Admin Queue Attributes (AQA).
pub const NVME_AQA: usize = 0x024;
/// Admin Submission Queue Base Address (ASQ).
pub const NVME_ASQ: usize = 0x028;
/// Admin Completion Queue Base Address (ACQ).
pub const NVME_ACQ: usize = 0x030;

// ============================================================================
// NvmeDriver
// ============================================================================

/// NVMe driver skeleton.
///
/// On the host, all I/O operations return [`StorageError::NotInitialized`].
/// The driver tracks `block_count` and a [`BadBlockTable`] so that the
/// health-reporting path is fully functional even without hardware.
pub struct NvmeDriver {
    /// Controller configuration (MMIO base, IRQ, DMA, geometry).
    config: StorageConfig,
    /// Total block count (set by [`NvmeDriver::init`]).
    block_count: u64,
    /// Bad block table for the device.
    bad_block_table: BadBlockTable,
    /// Whether [`NvmeDriver::init`] has been called with a non-zero count.
    initialized: bool,
}

impl NvmeDriver {
    /// Creates a new NVMe driver from `config`. The driver starts
    /// uninitialized; call [`NvmeDriver::init`] to set the block count.
    pub fn new(config: StorageConfig) -> Self {
        NvmeDriver {
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

impl BlockDevice for NvmeDriver {
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
        // No real PCIe MMIO on host — would submit an NVMe Read command here.
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
        // NVMe has no explicit erase command (deallocate/trim via DSM).
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
    fn test_nvme_register_offsets() {
        // Verify register offsets are distinct and match the NVMe 1.4 spec.
        assert_eq!(NVME_CAP, 0x000);
        assert_eq!(NVME_VS, 0x008);
        assert_eq!(NVME_INTMS, 0x00C);
        assert_eq!(NVME_INTMC, 0x010);
        assert_eq!(NVME_CC, 0x014);
        assert_eq!(NVME_CSTS, 0x01C);
        assert_eq!(NVME_NSSR, 0x020);
        assert_eq!(NVME_AQA, 0x024);
        assert_eq!(NVME_ASQ, 0x028);
        assert_eq!(NVME_ACQ, 0x030);
    }

    #[test]
    fn test_nvme_new_uninitialized() {
        let cfg = StorageConfig::default();
        let dev = NvmeDriver::new(cfg);
        assert!(!dev.is_initialized());
        assert_eq!(dev.block_count(), 0);
    }

    #[test]
    fn test_nvme_init_sets_block_count() {
        let cfg = StorageConfig::default();
        let mut dev = NvmeDriver::new(cfg);
        dev.init(4096);
        assert!(dev.is_initialized());
        assert_eq!(dev.block_count(), 4096);
    }

    #[test]
    fn test_nvme_read_uninitialized() {
        let cfg = StorageConfig::default();
        let dev = NvmeDriver::new(cfg);
        let mut buf = [0u8; 512];
        let err = dev.read_block(0, &mut buf).unwrap_err();
        assert_eq!(err, StorageError::NotInitialized);
    }

    #[test]
    fn test_nvme_write_uninitialized() {
        let cfg = StorageConfig::default();
        let mut dev = NvmeDriver::new(cfg);
        let buf = [0u8; 512];
        let err = dev.write_block(0, &buf).unwrap_err();
        assert_eq!(err, StorageError::NotInitialized);
    }

    #[test]
    fn test_nvme_erase_uninitialized() {
        let cfg = StorageConfig::default();
        let mut dev = NvmeDriver::new(cfg);
        let err = dev.erase_block(0).unwrap_err();
        assert_eq!(err, StorageError::NotInitialized);
    }

    #[test]
    fn test_nvme_read_initialized_out_of_range() {
        let cfg = StorageConfig::default();
        let mut dev = NvmeDriver::new(cfg);
        dev.init(100);
        let mut buf = [0u8; 512];
        let err = dev.read_block(100, &mut buf).unwrap_err();
        assert!(matches!(err, StorageError::OutOfRange { .. }));
    }

    #[test]
    fn test_nvme_read_initialized_in_range_no_hardware() {
        let cfg = StorageConfig::default();
        let mut dev = NvmeDriver::new(cfg);
        dev.init(100);
        let mut buf = [0u8; 512];
        let err = dev.read_block(0, &mut buf).unwrap_err();
        assert_eq!(err, StorageError::NotInitialized);
    }

    #[test]
    fn test_nvme_flush_uninitialized() {
        let cfg = StorageConfig::default();
        let mut dev = NvmeDriver::new(cfg);
        let err = dev.flush().unwrap_err();
        assert_eq!(err, StorageError::NotInitialized);
    }

    #[test]
    fn test_nvme_flush_initialized_ok() {
        let cfg = StorageConfig::default();
        let mut dev = NvmeDriver::new(cfg);
        dev.init(100);
        assert!(dev.flush().is_ok());
    }

    #[test]
    fn test_nvme_health_status_initialized() {
        let cfg = StorageConfig::default();
        let mut dev = NvmeDriver::new(cfg);
        dev.init(2000);
        let h = dev.health_status();
        assert_eq!(h.total_blocks, 2000);
        assert_eq!(h.bad_blocks, 0);
        assert_eq!(h.remaining_life, 100);
    }

    #[test]
    fn test_nvme_config_access() {
        let cfg = StorageConfig {
            storage_type: StorageType::Nvme,
            base_addr: 0x6000_0000,
            irq_num: 16,
            dma_channel: 0,
            block_size: 4096,
            max_transfer_blocks: 64,
        };
        let dev = NvmeDriver::new(cfg.clone());
        assert_eq!(dev.config().base_addr, 0x6000_0000);
        assert_eq!(dev.config().irq_num, 16);
        assert_eq!(dev.block_size(), 4096);
    }
}
