//! Storage type, configuration, and health status types.

/// Storage backend type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageType {
    /// Embedded MMC (eMMC).
    Emmc,
    /// NVMe SSD.
    Nvme,
    /// SD card (uses eMMC-like interface).
    SdCard,
}

/// Storage controller configuration.
///
/// Describes the MMIO base address, interrupt, DMA channel, and geometry
/// parameters used to instantiate a storage driver.
#[derive(Clone, Debug)]
pub struct StorageConfig {
    /// Storage backend type.
    pub storage_type: StorageType,
    /// MMIO base address of the controller.
    pub base_addr: usize,
    /// Interrupt number used by the controller.
    pub irq_num: u32,
    /// DMA channel (0–15) used for bulk transfers.
    pub dma_channel: u8,
    /// Block size in bytes (typically 512 or 4096).
    pub block_size: usize,
    /// Maximum blocks per single DMA transfer.
    pub max_transfer_blocks: u32,
}

impl Default for StorageConfig {
    fn default() -> Self {
        StorageConfig {
            storage_type: StorageType::Emmc,
            base_addr: 0xFF3F_0000,
            irq_num: 62,
            dma_channel: 0,
            block_size: 512,
            max_transfer_blocks: 256,
        }
    }
}

/// Device health snapshot reported by [`crate::driver::BlockDevice::health_status`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DeviceHealth {
    /// Total number of blocks on the device.
    pub total_blocks: u64,
    /// Number of bad blocks currently recorded.
    pub bad_blocks: u64,
    /// Wear level score (0–100, 0 = no wear).
    pub wear_level: u8,
    /// Device temperature in degrees Celsius (signed; may be negative).
    pub temperature: i16,
    /// Remaining life score (0–100, 100 = full life).
    pub remaining_life: u8,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_type_variants() {
        assert_ne!(StorageType::Emmc, StorageType::Nvme);
        assert_ne!(StorageType::Nvme, StorageType::SdCard);
        assert_ne!(StorageType::Emmc, StorageType::SdCard);
    }

    #[test]
    fn test_storage_type_copy() {
        let a = StorageType::Nvme;
        let b = a; // Copy
        assert_eq!(a, b);
    }

    #[test]
    fn test_storage_config_construction() {
        let cfg = StorageConfig {
            storage_type: StorageType::Nvme,
            base_addr: 0x4000_0000,
            irq_num: 32,
            dma_channel: 4,
            block_size: 4096,
            max_transfer_blocks: 128,
        };
        assert_eq!(cfg.storage_type, StorageType::Nvme);
        assert_eq!(cfg.base_addr, 0x4000_0000);
        assert_eq!(cfg.irq_num, 32);
        assert_eq!(cfg.dma_channel, 4);
        assert_eq!(cfg.block_size, 4096);
        assert_eq!(cfg.max_transfer_blocks, 128);
    }

    #[test]
    fn test_storage_config_default() {
        let cfg = StorageConfig::default();
        assert_eq!(cfg.storage_type, StorageType::Emmc);
        assert_eq!(cfg.base_addr, 0xFF3F_0000);
        assert_eq!(cfg.irq_num, 62);
        assert_eq!(cfg.dma_channel, 0);
        assert_eq!(cfg.block_size, 512);
        assert_eq!(cfg.max_transfer_blocks, 256);
    }

    #[test]
    fn test_storage_config_clone() {
        let cfg = StorageConfig {
            storage_type: StorageType::SdCard,
            base_addr: 0x1000,
            irq_num: 5,
            dma_channel: 2,
            block_size: 512,
            max_transfer_blocks: 64,
        };
        let cloned = cfg.clone();
        assert_eq!(cfg.storage_type, cloned.storage_type);
        assert_eq!(cfg.base_addr, cloned.base_addr);
        assert_eq!(cfg.irq_num, cloned.irq_num);
        assert_eq!(cfg.dma_channel, cloned.dma_channel);
        assert_eq!(cfg.block_size, cloned.block_size);
        assert_eq!(cfg.max_transfer_blocks, cloned.max_transfer_blocks);
    }

    #[test]
    fn test_device_health_default() {
        let h = DeviceHealth::default();
        assert_eq!(h.total_blocks, 0);
        assert_eq!(h.bad_blocks, 0);
        assert_eq!(h.wear_level, 0);
        assert_eq!(h.temperature, 0);
        assert_eq!(h.remaining_life, 0);
    }

    #[test]
    fn test_device_health_construction() {
        let h = DeviceHealth {
            total_blocks: 1000,
            bad_blocks: 10,
            wear_level: 1,
            temperature: 45,
            remaining_life: 99,
        };
        assert_eq!(h.total_blocks, 1000);
        assert_eq!(h.bad_blocks, 10);
        assert_eq!(h.wear_level, 1);
        assert_eq!(h.temperature, 45);
        assert_eq!(h.remaining_life, 99);
    }

    #[test]
    fn test_device_health_negative_temperature() {
        let h = DeviceHealth {
            total_blocks: 100,
            bad_blocks: 0,
            wear_level: 0,
            temperature: -20,
            remaining_life: 100,
        };
        assert_eq!(h.temperature, -20);
    }

    #[test]
    fn test_device_health_equality() {
        let a = DeviceHealth {
            total_blocks: 100,
            bad_blocks: 5,
            wear_level: 5,
            temperature: 30,
            remaining_life: 95,
        };
        let b = a;
        assert_eq!(a, b);
    }
}
