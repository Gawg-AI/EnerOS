//! Block device factory.
//!
//! [`create_block_device`] dispatches on [`crate::driver::types::StorageType`]
//! to construct the appropriate driver behind a `Box<dyn BlockDevice>`.

use alloc::boxed::Box;

use crate::driver::emmc::EmmcDriver;
use crate::driver::mock::MockBlockDevice;
use crate::driver::nvme::NvmeDriver;
use crate::driver::types::{StorageConfig, StorageType};
use crate::driver::BlockDevice;
use crate::error::StorageError;

/// Creates a block device driver matching `config.storage_type`.
///
/// The returned driver is uninitialized — call `init(block_count)` on the
/// concrete type (or use [`create_mock_device`] for host tests).
pub fn create_block_device(config: &StorageConfig) -> Result<Box<dyn BlockDevice>, StorageError> {
    match config.storage_type {
        StorageType::Emmc => {
            let driver = EmmcDriver::new(config.clone());
            Ok(Box::new(driver))
        }
        StorageType::Nvme => {
            let driver = NvmeDriver::new(config.clone());
            Ok(Box::new(driver))
        }
        StorageType::SdCard => {
            // SdCard uses the same SD/eMMC command set for now.
            let driver = EmmcDriver::new(config.clone());
            Ok(Box::new(driver))
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
    fn test_create_emmc_device() {
        let cfg = StorageConfig {
            storage_type: StorageType::Emmc,
            base_addr: 0xFF3F_0000,
            irq_num: 62,
            dma_channel: 0,
            block_size: 512,
            max_transfer_blocks: 256,
        };
        let dev = create_block_device(&cfg).expect("should create eMMC device");
        // Uninitialized → block_count is 0.
        assert_eq!(dev.block_count(), 0);
        assert_eq!(dev.block_size(), 512);
        // read_block should return NotInitialized.
        let mut buf = [0u8; 512];
        let err = dev.read_block(0, &mut buf).unwrap_err();
        assert_eq!(err, StorageError::NotInitialized);
    }

    #[test]
    fn test_create_nvme_device() {
        let cfg = StorageConfig {
            storage_type: StorageType::Nvme,
            base_addr: 0x6000_0000,
            irq_num: 16,
            dma_channel: 0,
            block_size: 4096,
            max_transfer_blocks: 64,
        };
        let dev = create_block_device(&cfg).expect("should create NVMe device");
        assert_eq!(dev.block_count(), 0);
        assert_eq!(dev.block_size(), 4096);
        let mut buf = [0u8; 4096];
        let err = dev.read_block(0, &mut buf).unwrap_err();
        assert_eq!(err, StorageError::NotInitialized);
    }

    #[test]
    fn test_create_sdcard_device() {
        let cfg = StorageConfig {
            storage_type: StorageType::SdCard,
            base_addr: 0xFF3F_0000,
            irq_num: 62,
            dma_channel: 1,
            block_size: 512,
            max_transfer_blocks: 128,
        };
        let dev = create_block_device(&cfg).expect("should create SD card device");
        assert_eq!(dev.block_count(), 0);
        assert_eq!(dev.block_size(), 512);
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
        let cfg = StorageConfig::default();
        let _dev: Box<dyn BlockDevice> = create_block_device(&cfg).expect("should create");
    }
}
