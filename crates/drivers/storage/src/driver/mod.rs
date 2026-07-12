//! Core storage driver traits and re-exports.
//!
//! This module defines the three primary traits of the storage stack:
//! - [`BlockDevice`] — the unified block I/O interface.
//! - [`StorageMmio`] — abstract MMIO register/data access (wired to real
//!   hardware in Phase 3).
//! - [`DmaTransfer`] — bulk DMA transfer interface.
//!
//! Concrete implementations live in the submodules:
//! - [`mock`] — `MockBlockDevice` (RAM-backed, host-testable).
//! - [`emmc`] — `EmmcDriver` skeleton.
//! - [`nvme`] — `NvmeDriver` skeleton.
//! - [`dma`] — `MockDmaTransfer`.
//! - [`factory`] — `create_block_device` dispatcher.

pub mod dma;
pub mod emmc;
pub mod factory;
pub mod mock;
pub mod nvme;
pub mod types;

pub use dma::{DmaBuffer, MockDmaTransfer};
pub use emmc::{EmmcCmdType, EmmcDriver};
pub use factory::create_block_device;
pub use mock::MockBlockDevice;
pub use nvme::NvmeDriver;
pub use types::{DeviceHealth, StorageConfig, StorageType};

use crate::error::StorageError;

/// Unified block device interface for eMMC, NVMe, SD card, and mock backends.
///
/// All indices are 0-based block addresses. Buffer lengths must match
/// [`BlockDevice::block_size`]; callers are responsible for alignment.
pub trait BlockDevice {
    /// Reads block `block_idx` into `buf`.
    ///
    /// `buf.len()` must equal `block_size()`. Returns
    /// [`StorageError::OutOfRange`] if `block_idx >= block_count()`,
    /// [`StorageError::BadBlock`] if the block is marked bad, and
    /// [`StorageError::CrcMismatch`] on CRC verification failure.
    fn read_block(&self, block_idx: u64, buf: &mut [u8]) -> Result<(), StorageError>;

    /// Writes `buf` to block `block_idx`.
    ///
    /// `buf.len()` must equal `block_size()`. If the block is bad, the
    /// implementation may transparently redirect to a replacement block.
    fn write_block(&mut self, block_idx: u64, buf: &[u8]) -> Result<(), StorageError>;

    /// Erases block `block_idx` (sets its contents to a device-specific
    /// erased state, typically all-ones or all-zeros for mock).
    fn erase_block(&mut self, block_idx: u64) -> Result<(), StorageError>;

    /// Returns the total number of blocks on the device.
    fn block_count(&self) -> u64;

    /// Returns the block size in bytes (typically 512 or 4096).
    fn block_size(&self) -> usize;

    /// Flushes any pending writes to persistent media.
    fn flush(&mut self) -> Result<(), StorageError>;

    /// Returns the current health status of the device.
    fn health_status(&self) -> DeviceHealth;
}

/// Abstract MMIO access for storage controllers.
///
/// On real hardware this is backed by volatile memory-mapped registers.
/// In host tests it is backed by a plain `Vec<u8>`. Phase 3 will provide
/// a real `volatile`-backed implementation.
pub trait StorageMmio {
    /// Reads a 32-bit register at `offset`.
    fn read_reg(&self, offset: usize) -> u32;
    /// Writes `value` to the 32-bit register at `offset`.
    fn write_reg(&mut self, offset: usize, value: u32);
    /// Reads a block of bytes starting at `offset` into `buf`.
    fn read_block_at(&self, offset: usize, buf: &mut [u8]);
    /// Writes `buf` to the MMIO region starting at `offset`.
    fn write_block_at(&mut self, offset: usize, buf: &[u8]);
}

/// Bulk DMA transfer interface for high-throughput block movement.
///
/// Implementations typically wrap a DMA engine that can move `count` blocks
/// in a single descriptor chain, avoiding per-block CPU intervention.
pub trait DmaTransfer {
    /// DMA-reads `count` blocks starting at `block_idx` into `buf`.
    ///
    /// `buf.len()` must be at least `count * block_size`.
    fn dma_read(&mut self, block_idx: u64, count: u32, buf: &mut [u8]) -> Result<(), StorageError>;

    /// DMA-writes `count` blocks starting at `block_idx` from `buf`.
    ///
    /// `buf.len()` must be at least `count * block_size`.
    fn dma_write(&mut self, block_idx: u64, count: u32, buf: &[u8]) -> Result<(), StorageError>;
}

// ============================================================================
// Tests — trait object usage
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::mock::MockBlockDevice;

    #[test]
    fn test_trait_object_dyn_block_device() {
        let dev: Box<dyn BlockDevice> = Box::new(MockBlockDevice::new(4, 512));
        assert_eq!(dev.block_count(), 4);
        assert_eq!(dev.block_size(), 512);
        let health = dev.health_status();
        assert_eq!(health.total_blocks, 4);
        assert_eq!(health.bad_blocks, 0);
    }

    #[test]
    fn test_trait_object_read_write() {
        let mut dev: Box<dyn BlockDevice> = Box::new(MockBlockDevice::new(2, 512));
        let input = [0xAA; 512];
        dev.write_block(0, &input).expect("write should succeed");
        let mut out = [0u8; 512];
        dev.read_block(0, &mut out).expect("read should succeed");
        assert_eq!(out, input);
    }

    #[test]
    fn test_trait_object_in_vec() {
        // Multiple trait objects in a Vec — verifies object safety.
        let devs: Vec<Box<dyn BlockDevice>> = vec![
            Box::new(MockBlockDevice::new(4, 512)),
            Box::new(MockBlockDevice::new(8, 4096)),
        ];
        assert_eq!(devs[0].block_count(), 4);
        assert_eq!(devs[1].block_count(), 8);
        assert_eq!(devs[0].block_size(), 512);
        assert_eq!(devs[1].block_size(), 4096);
    }
}
