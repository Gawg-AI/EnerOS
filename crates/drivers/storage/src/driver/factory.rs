//! Block device factory.
//!
//! [`create_block_device`] dispatches on [`crate::driver::types::StorageType`]
//! to construct the appropriate driver behind a `Box<dyn BlockDevice>`.

use alloc::boxed::Box;

use crate::driver::mock::MockBlockDevice;
use crate::driver::types::{StorageConfig, StorageType};
use crate::driver::BlockDevice;
use crate::error::StorageError;

/// Creates a block device driver matching `config.storage_type`.
///
/// - [`StorageType::VirtioBlk`] — real virtio-blk driver (QEMU-verifiable,
///   virtio-mmio transport). Requires a concrete [`VirtioMmio`]/[`DmaPool`]/
///   [`ArmCache`] wiring; `create_block_device` is the host-test entry that
///   wires them, while target builds construct [`VirtioBlkDriver`] directly.
/// - [`StorageType::Emmc`] / [`StorageType::Nvme`] / [`StorageType::SdCard`]
///   — **not supported in this release** (阶段 A：真实控制器推后到阶段 B).
///   Returns [`StorageError::HardwareFault`] instead of a broken skeleton.
///
/// The returned driver is uninitialized — call `init(block_count)` on the
/// concrete type (or use [`create_mock_device`] for host tests).
pub fn create_block_device(
    config: &StorageConfig,
) -> Result<Box<dyn BlockDevice>, StorageError> {
    match config.storage_type {
        StorageType::VirtioBlk => {
            // Host-test wiring: RAM-simulated MMIO + DMA pool + no-op cache.
            // Real targets construct VirtioBlkDriver with their own backends.
            let mmio = crate::driver::virtio_blk::RamMmio::new(config.max_transfer_blocks as u64);
            let pool = crate::driver::virtio_blk::RamPool::new(9);
            let driver = crate::driver::virtio_blk::VirtioBlkDriver::new(
                mmio,
                pool,
                crate::driver::virtio_blk::NoopCache,
                config.clone(),
            )?;
            Ok(Box::new(driver))
        }
        StorageType::Emmc | StorageType::Nvme | StorageType::SdCard => {
            // 阶段 A（v0.194.0）：真实 eMMC/NVMe 控制器驱动推后到阶段 B。
            // 不再产出恒 NotInitialized 的不可读写骨架（P0-4）。
            Err(StorageError::HardwareFault {
                detail: "eMMC/NVMe controller driver not supported until Phase B (use VirtioBlk)",
            })
        }
    }
}

/// Convenience helper for creating a [`MockBlockDevice`] in host tests.
pub fn create_mock_device(block_count: u64, block_size: usize) -> MockBlockDevice {
    MockBlockDevice::new(block_count, block_size)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_emmc_device_rejected_phase_a() {
        // P0-4（v0.194.0）：eMMC 真实控制器驱动推后到阶段 B，不再产出
        // 恒 NotInitialized 的不可读写骨架。
        let cfg = StorageConfig {
            storage_type: StorageType::Emmc,
            base_addr: 0xFF3F_0000,
            irq_num: 62,
            dma_channel: 0,
            block_size: 512,
            max_transfer_blocks: 256,
        };
        assert!(
            matches!(create_block_device(&cfg), Err(StorageError::HardwareFault { .. })),
            "eMMC/NVMe must be rejected in Phase A"
        );
    }

    #[test]
    fn test_create_nvme_device_rejected_phase_a() {
        let cfg = StorageConfig {
            storage_type: StorageType::Nvme,
            base_addr: 0x6000_0000,
            irq_num: 16,
            dma_channel: 0,
            block_size: 4096,
            max_transfer_blocks: 64,
        };
        assert!(
            matches!(create_block_device(&cfg), Err(StorageError::HardwareFault { .. })),
            "eMMC/NVMe must be rejected in Phase A"
        );
    }

    #[test]
    fn test_create_sdcard_device_rejected_phase_a() {
        let cfg = StorageConfig {
            storage_type: StorageType::SdCard,
            base_addr: 0xFF3F_0000,
            irq_num: 62,
            dma_channel: 1,
            block_size: 512,
            max_transfer_blocks: 128,
        };
        assert!(
            matches!(create_block_device(&cfg), Err(StorageError::HardwareFault { .. })),
            "eMMC/NVMe must be rejected in Phase A"
        );
    }

    #[test]
    fn test_create_virtio_blk_device() {
        let cfg = StorageConfig {
            storage_type: StorageType::VirtioBlk,
            base_addr: 0x0A00_0000,
            irq_num: 48,
            dma_channel: 0,
            block_size: 512,
            max_transfer_blocks: 256,
        };
        let dev = create_block_device(&cfg).expect("should create virtio-blk device");
        assert_eq!(dev.block_size(), 512);
        // RamMmio 盘默认 16 扇区上限受 max_transfer_blocks 影响，直接读写验证往返。
        let mut out = [0u8; 512];
        dev.read_block(0, &mut out).expect("read block 0");
        assert_eq!(out, [0u8; 512]);
    }

    #[test]
    fn test_create_mock_device_helper() {
        let dev = create_mock_device(100, 512);
        assert_eq!(dev.block_count(), 100);
        assert_eq!(dev.block_size(), 512);
    }

    #[test]
    fn test_create_mock_device_roundtrip() {
        let mut dev = create_mock_device(4, 256);
        let input = [0x42; 256];
        dev.write_block(0, &input).expect("write");
        let mut out = [0u8; 256];
        dev.read_block(0, &mut out).expect("read");
        assert_eq!(out, input);
    }

    #[test]
    fn test_factory_returns_trait_object() {
        let cfg = StorageConfig {
            storage_type: StorageType::VirtioBlk,
            base_addr: 0x0A00_0000,
            irq_num: 48,
            dma_channel: 0,
            block_size: 512,
            max_transfer_blocks: 256,
        };
        let _dev: Box<dyn BlockDevice> = create_block_device(&cfg).expect("should create");
    }
}
