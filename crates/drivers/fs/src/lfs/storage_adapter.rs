//! Adapter that bridges [`eneros_storage::BlockDevice`] to littlefs2's
//! [`Storage`] trait.
//!
//! littlefs2 requires a [`Storage`] implementation with compile-time constant
//! geometry (block size, block count, cache size). This adapter wraps a
//! runtime `Box<dyn BlockDevice>` and exposes it through the `Storage` trait
//! by converting byte offsets to block indices.
//!
//! # Geometry
//!
//! | Constant | Value | Meaning |
//! |----------|-------|---------|
//! | `READ_SIZE` | 4096 | Minimum read granularity |
//! | `WRITE_SIZE` | 4096 | Minimum write granularity |
//! | `BLOCK_SIZE` | 4096 | Erase block size |
//! | `BLOCK_COUNT` | 64 | Total blocks (256 KB) |
//! | `BLOCK_CYCLES` | 100 000 | Wear-leveling window (SLC) |
//! | `CACHE_SIZE` | U4096 | One-block read/write cache |
//! | `LOOKAHEAD_SIZE` | U8 | 8 × 8 = 64-byte lookahead bitmap |
//!
//! The `BLOCK_COUNT` of 64 (256 KB) is chosen for host-testability with
//! `MockBlockDevice`. For production deployments on larger flash parts,
//! this constant should be increased (e.g. 65536 for 256 MB).

use alloc::boxed::Box;
use core::fmt;

use eneros_storage::BlockDevice;
use generic_array::typenum::{U4096, U8};
use littlefs2::driver::Storage;
use littlefs2::io::{Error as LfsError, Result as LfsResult};

use crate::error::FsError;

/// Adapter wrapping a [`BlockDevice`] as a littlefs2 [`Storage`].
///
/// Construct with [`BlockDeviceStorage::new`], passing a boxed block device
/// (e.g. `MockBlockDevice`, `EmmcDriver`, or `NvmeDriver`). The device's
/// `block_size()` must equal 4096 and its `block_count()` must not exceed
/// [`Self::BLOCK_COUNT`].
pub struct BlockDeviceStorage {
    device: Box<dyn BlockDevice>,
    /// Actual block count of the underlying device (≤ `BLOCK_COUNT`).
    block_count: usize,
}

impl fmt::Debug for BlockDeviceStorage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlockDeviceStorage")
            .field("block_count", &self.block_count)
            .field("block_size", &self.device.block_size())
            .finish()
    }
}

impl BlockDeviceStorage {
    /// Creates a new adapter wrapping the given block device.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::InvalidArgument`] if the device's block size is not
    /// 4096, or [`FsError::NoSpace`] if the device's block count exceeds the
    /// adapter's `BLOCK_COUNT`.
    pub fn new(device: Box<dyn BlockDevice>) -> Result<Self, FsError> {
        let bs = device.block_size();
        let bc = device.block_count() as usize;
        if bs != Self::BLOCK_SIZE {
            return Err(FsError::InvalidArgument);
        }
        if bc > Self::BLOCK_COUNT {
            return Err(FsError::NoSpace);
        }
        if bc == 0 {
            return Err(FsError::InvalidArgument);
        }
        Ok(Self {
            device,
            block_count: bc,
        })
    }

    /// Returns the actual block count of the underlying device.
    pub fn device_block_count(&self) -> usize {
        self.block_count
    }

    /// Returns a reference to the underlying block device.
    pub fn device(&self) -> &dyn BlockDevice {
        self.device.as_ref()
    }
}

impl Storage for BlockDeviceStorage {
    type CACHE_SIZE = U4096;
    type LOOKAHEAD_SIZE = U8;

    const READ_SIZE: usize = 4096;
    const WRITE_SIZE: usize = 4096;
    const BLOCK_SIZE: usize = 4096;
    const BLOCK_COUNT: usize = 64;
    const BLOCK_CYCLES: isize = 100_000;

    fn read(&mut self, off: usize, buf: &mut [u8]) -> LfsResult<usize> {
        let block_size = Self::BLOCK_SIZE;
        let block_idx = off / block_size;
        let num_blocks = buf.len() / block_size;

        for i in 0..num_blocks {
            let start = i * block_size;
            let end = start + block_size;
            self.device
                .read_block((block_idx + i) as u64, &mut buf[start..end])
                .map_err(map_storage_error)?;
        }
        Ok(buf.len())
    }

    fn write(&mut self, off: usize, data: &[u8]) -> LfsResult<usize> {
        let block_size = Self::BLOCK_SIZE;
        let block_idx = off / block_size;
        let num_blocks = data.len() / block_size;

        for i in 0..num_blocks {
            let start = i * block_size;
            let end = start + block_size;
            self.device
                .write_block((block_idx + i) as u64, &data[start..end])
                .map_err(map_storage_error)?;
        }
        Ok(data.len())
    }

    fn erase(&mut self, off: usize, len: usize) -> LfsResult<usize> {
        let block_size = Self::BLOCK_SIZE;
        let block_idx = off / block_size;
        let num_blocks = len / block_size;

        for i in 0..num_blocks {
            self.device
                .erase_block((block_idx + i) as u64)
                .map_err(map_storage_error)?;
        }
        Ok(len)
    }
}

/// Maps a [`eneros_storage::StorageError`] to a littlefs2 [`LfsError`].
fn map_storage_error(err: eneros_storage::StorageError) -> LfsError {
    match err {
        eneros_storage::StorageError::OutOfRange { .. } => LfsError::IO,
        eneros_storage::StorageError::BadBlock { .. } => LfsError::CORRUPTION,
        eneros_storage::StorageError::CrcMismatch { .. } => LfsError::CORRUPTION,
        eneros_storage::StorageError::WriteProtected => LfsError::IO,
        _ => LfsError::IO,
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use eneros_storage::MockBlockDevice;

    use super::*;

    fn make_storage(blocks: u64) -> BlockDeviceStorage {
        let dev: Box<dyn BlockDevice> = Box::new(MockBlockDevice::new(blocks, 4096));
        BlockDeviceStorage::new(dev).expect("storage creation should succeed")
    }

    #[test]
    fn test_new_success() {
        let s = make_storage(32);
        assert_eq!(s.device_block_count(), 32);
    }

    #[test]
    fn test_new_wrong_block_size() {
        let dev: Box<dyn BlockDevice> = Box::new(MockBlockDevice::new(32, 512));
        let err = BlockDeviceStorage::new(dev).unwrap_err();
        assert_eq!(err, FsError::InvalidArgument);
    }

    #[test]
    fn test_new_too_many_blocks() {
        let dev: Box<dyn BlockDevice> = Box::new(MockBlockDevice::new(128, 4096));
        let err = BlockDeviceStorage::new(dev).unwrap_err();
        assert_eq!(err, FsError::NoSpace);
    }

    #[test]
    fn test_new_zero_blocks() {
        let dev: Box<dyn BlockDevice> = Box::new(MockBlockDevice::new(0, 4096));
        let err = BlockDeviceStorage::new(dev).unwrap_err();
        assert_eq!(err, FsError::InvalidArgument);
    }

    #[test]
    fn test_read_single_block() {
        let mut s = make_storage(8);
        // Write data to block 0 via the underlying device.
        let data = [0xABu8; 4096];
        s.device
            .write_block(0, &data)
            .expect("write should succeed");

        // Read via the Storage trait (byte offset 0).
        let mut buf = [0u8; 4096];
        let n = s.read(0, &mut buf).expect("read should succeed");
        assert_eq!(n, 4096);
        assert_eq!(buf, data);
    }

    #[test]
    fn test_read_multiple_blocks() {
        let mut s = make_storage(8);
        // Write to blocks 0 and 1.
        let data0 = [0x11u8; 4096];
        let data1 = [0x22u8; 4096];
        s.device.write_block(0, &data0).unwrap();
        s.device.write_block(1, &data1).unwrap();

        // Read 2 blocks starting at byte offset 0.
        let mut buf = [0u8; 8192];
        let n = s.read(0, &mut buf).expect("read should succeed");
        assert_eq!(n, 8192);
        assert_eq!(&buf[0..4096], &data0[..]);
        assert_eq!(&buf[4096..8192], &data1[..]);
    }

    #[test]
    fn test_read_at_offset() {
        let mut s = make_storage(8);
        let data = [0x33u8; 4096];
        s.device.write_block(2, &data).unwrap();

        // Read block 2 (byte offset 2 * 4096 = 8192).
        let mut buf = [0u8; 4096];
        let n = s.read(8192, &mut buf).expect("read should succeed");
        assert_eq!(n, 4096);
        assert_eq!(buf, data);
    }

    #[test]
    fn test_write_single_block() {
        let mut s = make_storage(8);
        let data = [0xCDu8; 4096];
        let n = s.write(0, &data).expect("write should succeed");
        assert_eq!(n, 4096);

        // Verify via the underlying device.
        let mut buf = [0u8; 4096];
        s.device.read_block(0, &mut buf).unwrap();
        assert_eq!(buf, data);
    }

    #[test]
    fn test_write_multiple_blocks() {
        let mut s = make_storage(8);
        let data = vec![0xEEu8; 8192];
        let n = s.write(0, &data).expect("write should succeed");
        assert_eq!(n, 8192);

        let mut buf = [0u8; 4096];
        s.device.read_block(0, &mut buf).unwrap();
        assert_eq!(buf, [0xEE; 4096]);
        s.device.read_block(1, &mut buf).unwrap();
        assert_eq!(buf, [0xEE; 4096]);
    }

    #[test]
    fn test_write_at_offset() {
        let mut s = make_storage(8);
        let data = [0x55u8; 4096];
        // Write to block 3 (byte offset 3 * 4096 = 12288).
        let n = s.write(12288, &data).expect("write should succeed");
        assert_eq!(n, 4096);

        let mut buf = [0u8; 4096];
        s.device.read_block(3, &mut buf).unwrap();
        assert_eq!(buf, data);
    }

    #[test]
    fn test_erase_single_block() {
        let mut s = make_storage(8);
        // Write data first.
        let data = [0xFFu8; 4096];
        s.device.write_block(0, &data).unwrap();

        // Erase block 0 (byte offset 0, length 4096).
        let n = s.erase(0, 4096).expect("erase should succeed");
        assert_eq!(n, 4096);

        // MockBlockDevice erases to 0x00.
        let mut buf = [0xFFu8; 4096];
        s.device.read_block(0, &mut buf).unwrap();
        assert_eq!(buf, [0u8; 4096]);
    }

    #[test]
    fn test_erase_multiple_blocks() {
        let mut s = make_storage(8);
        // Write to blocks 0 and 1.
        s.device.write_block(0, &[0xAA; 4096]).unwrap();
        s.device.write_block(1, &[0xBB; 4096]).unwrap();

        // Erase 2 blocks.
        let n = s.erase(0, 8192).expect("erase should succeed");
        assert_eq!(n, 8192);

        let mut buf = [0xFFu8; 4096];
        s.device.read_block(0, &mut buf).unwrap();
        assert_eq!(buf, [0u8; 4096]);
        s.device.read_block(1, &mut buf).unwrap();
        assert_eq!(buf, [0u8; 4096]);
    }

    #[test]
    fn test_read_write_roundtrip() {
        let mut s = make_storage(8);
        let original = vec![0x42u8; 4096];

        // Write via Storage trait.
        s.write(4096, &original).unwrap();

        // Read back via Storage trait.
        let mut buf = vec![0u8; 4096];
        let n = s.read(4096, &mut buf).unwrap();
        assert_eq!(n, 4096);
        assert_eq!(buf, original);
    }

    #[test]
    fn test_read_out_of_range() {
        let mut s = make_storage(4);
        let mut buf = [0u8; 4096];
        // Block 10 is out of range for a 4-block device.
        let err = s.read(10 * 4096, &mut buf).unwrap_err();
        assert_eq!(err, LfsError::IO);
    }

    #[test]
    fn test_write_out_of_range() {
        let mut s = make_storage(4);
        let data = [0u8; 4096];
        let err = s.write(10 * 4096, &data).unwrap_err();
        assert_eq!(err, LfsError::IO);
    }

    #[test]
    fn test_erase_out_of_range() {
        let mut s = make_storage(4);
        let err = s.erase(10 * 4096, 4096).unwrap_err();
        assert_eq!(err, LfsError::IO);
    }

    #[test]
    fn test_storage_constants() {
        assert_eq!(BlockDeviceStorage::READ_SIZE, 4096);
        assert_eq!(BlockDeviceStorage::WRITE_SIZE, 4096);
        assert_eq!(BlockDeviceStorage::BLOCK_SIZE, 4096);
        assert_eq!(BlockDeviceStorage::BLOCK_COUNT, 64);
        assert_eq!(BlockDeviceStorage::BLOCK_CYCLES, 100_000);
    }

    #[test]
    fn test_device_accessor() {
        let s = make_storage(16);
        assert_eq!(s.device().block_count(), 16);
        assert_eq!(s.device().block_size(), 4096);
    }
}
