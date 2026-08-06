//! Block device factory.
//!
//! [`create_block_device`] dispatches on [`crate::driver::types::StorageType`]
//! to construct the appropriate driver behind a `Box<dyn BlockDevice>`.

use alloc::boxed::Box;

use crate::driver::emmc::{EmmcDriver, RamEmmcMmio};
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
/// - [`StorageType::Emmc`] — real eMMC driver over a RAM-simulated
///   controller (QEMU sd model equivalent, v0.215.0 B1-03). Target builds
///   construct [`EmmcDriver`] with a volatile MMIO backend.
/// - [`StorageType::Nvme`] / [`StorageType::SdCard`] — not supported yet.
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
        StorageType::Emmc => {
            // v0.215.0（B1-03）：eMMC 真实驱动 + RAM 模拟控制器（QEMU sd 模型等价）。
            // 目标板用 volatile MMIO 后端直接构造 EmmcDriver。
            let mmio = RamEmmcMmio::new(config.max_transfer_blocks as u64, config.block_size);
            Ok(Box::new(EmmcDriver::new(mmio, config.clone())))
        }
        StorageType::Nvme | StorageType::SdCard => {
            Err(StorageError::HardwareFault {
                detail: "NVMe/SdCard controller driver not supported yet (use Emmc or VirtioBlk)",
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
    fn test_create_emmc_device_roundtrip() {
        // v0.215.0（B1-03）：eMMC 真实驱动 + RAM 模拟控制器（QEMU sd 模型等价）。
        let cfg = StorageConfig {
            storage_type: StorageType::Emmc,
            base_addr: 0xFF3F_0000,
            irq_num: 62,
            dma_channel: 0,
            block_size: 512,
            max_transfer_blocks: 256,
        };
        let dev = create_block_device(&cfg).expect("should create eMMC device");
        // 工厂返回未初始化驱动，需 init（factory 层 trait 无 init，这里用具体类型验证）。
        let mmio = RamEmmcMmio::new(256, 512);
        let mut real = EmmcDriver::new(mmio, cfg.clone());
        real.init(256);
        let input = [0xC7; 512];
        real.write_block(0, &input).expect("write");
        let mut out = [0u8; 512];
        real.read_block(0, &mut out).expect("read");
        assert_eq!(out, input);
        let _ = dev;
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
