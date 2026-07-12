//! EnerOS Storage Driver — Phase 1 P1-A (v0.23.0).
//!
//! This crate provides the block device abstraction layer for EnerOS,
//! supporting eMMC and NVMe storage backends with a unified
//! [`driver::BlockDevice`] trait interface.
//!
//! # v0.23.0 Deliverables
//!
//! - **`BlockDevice` trait** — unified read/write/erase/flush/health interface
//!   for eMMC, NVMe, and SD card storage backends.
//! - **`StorageError`** — 8-variant error type with recoverability
//!   classification (`is_recoverable`).
//! - **`BadBlockTable`** — bad block tracking with reserved-block replacement,
//!   wear leveling, and remaining life estimation.
//! - **`crc32`** — IEEE 802.3 CRC32 (polynomial 0xEDB88320, reversed) with
//!   table-based implementation for data integrity verification.
//! - **`MockBlockDevice`** — RAM-backed block device for host-side testing of
//!   the storage stack without real hardware.
//! - **`EmmcDriver`** — eMMC driver skeleton with register map and command
//!   encoding (host returns `NotInitialized`; real I/O wired in Phase 3).
//! - **`NvmeDriver`** — NVMe driver skeleton with controller register map
//!   (host returns `NotInitialized`; real I/O wired in Phase 3).
//! - **`DmaTransfer` trait + `MockDmaTransfer`** — DMA transfer abstraction
//!   for bulk block movement.
//! - **`create_block_device`** — factory function dispatching on
//!   [`driver::StorageType`].
//!
//! # Design Principles
//!
//! - **no_std**: All production code uses `core::*` and `alloc::*` only.
//! - **Host-testable**: `MockBlockDevice` enables ≥40 unit tests on the host
//!   without QEMU or real hardware.
//! - **Trait-based**: `BlockDevice` allows swapping eMMC/NVMe/mock via the
//!   factory without changing call sites.
//! - **Forward-compatible**: eMMC/NVMe drivers expose register maps and
//!   command encoding so Phase 3 only needs to wire MMIO accesses.
//!
//! # Usage
//!
//! ```ignore
//! use eneros_storage::{create_block_device, StorageConfig, StorageType};
//!
//! let config = StorageConfig {
//!     storage_type: StorageType::Emmc,
//!     base_addr: 0xFF3F_0000,
//!     irq_num: 62,
//!     dma_channel: 0,
//!     block_size: 512,
//!     max_transfer_blocks: 256,
//! };
//! let mut dev = create_block_device(&config).unwrap();
//! dev.init(1024 * 1024); // 1M blocks
//! ```

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod bad_block;
pub mod crc32;
pub mod driver;
pub mod error;

pub use bad_block::BadBlockTable;
pub use crc32::crc32;
pub use driver::{
    create_block_device, BlockDevice, DeviceHealth, DmaTransfer, MockBlockDevice, StorageConfig,
    StorageMmio, StorageType,
};
pub use driver::{DmaBuffer, EmmcCmdType, EmmcDriver, MockDmaTransfer, NvmeDriver};
pub use error::StorageError;
