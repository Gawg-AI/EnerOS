//! RAM-backed mock block device for host-side testing.
//!
//! [`MockBlockDevice`] implements [`crate::driver::BlockDevice`] using a
//! `Vec<Vec<u8>>` backing store. It supports:
//! - bad block injection (returns [`StorageError::BadBlock`] on read/write)
//! - CRC error injection (returns [`StorageError::CrcMismatch`] on read)
//! - replacement-block redirection on write (via [`BadBlockTable`])
//! - health reporting derived from the bad block table
//!
//! This is the primary test vehicle for the storage stack — no QEMU or real
//! hardware is required.

use alloc::vec::Vec;

use crate::bad_block::BadBlockTable;
use crate::driver::types::DeviceHealth;
use crate::driver::BlockDevice;
use crate::error::StorageError;

/// RAM-backed block device for host-side testing.
pub struct MockBlockDevice {
    /// Per-block byte vectors. Each inner Vec has length `block_size`.
    blocks: Vec<Vec<u8>>,
    /// Bad block table tracking bad blocks and replacement pool.
    bad_block_table: BadBlockTable,
    /// Block size in bytes.
    block_size: usize,
    /// Blocks that should return a CRC error on read (simulated corruption).
    crc_error_blocks: Vec<u64>,
}

impl MockBlockDevice {
    /// Creates a new mock device with `block_count` blocks of `block_size`
    /// bytes each. All blocks start zeroed and healthy.
    pub fn new(block_count: u64, block_size: usize) -> Self {
        let count = block_count as usize;
        let blocks = (0..count).map(|_| alloc::vec![0u8; block_size]).collect();
        // Reserve 5% of blocks (min 1) for the replacement pool.
        let reserved = if block_count == 0 {
            0
        } else {
            (block_count / 20).max(1)
        };
        MockBlockDevice {
            blocks,
            bad_block_table: BadBlockTable::new(block_count, reserved),
            block_size,
            crc_error_blocks: Vec::new(),
        }
    }

    /// Marks `block_idx` as bad. Subsequent reads/writes return
    /// [`StorageError::BadBlock`] (writes may transparently redirect to a
    /// replacement block).
    pub fn mark_bad(&mut self, block_idx: u64) {
        self.bad_block_table.mark_bad(block_idx);
    }

    /// Injects a CRC error for `block_idx`. Subsequent reads return
    /// [`StorageError::CrcMismatch`].
    pub fn inject_crc_error(&mut self, block_idx: u64) {
        if !self.crc_error_blocks.contains(&block_idx) {
            self.crc_error_blocks.push(block_idx);
        }
    }

    /// Returns `true` if `block_idx` is in the CRC-error injection list.
    fn has_crc_error(&self, block_idx: u64) -> bool {
        self.crc_error_blocks.contains(&block_idx)
    }
}

impl BlockDevice for MockBlockDevice {
    fn read_block(&self, block_idx: u64, buf: &mut [u8]) -> Result<(), StorageError> {
        let max = self.blocks.len() as u64;
        if block_idx >= max {
            return Err(StorageError::OutOfRange { block_idx, max });
        }
        if self.bad_block_table.is_bad(block_idx) {
            return Err(StorageError::BadBlock { block_idx });
        }
        if self.has_crc_error(block_idx) {
            return Err(StorageError::CrcMismatch {
                expected: 0xDEAD_BEEF,
                actual: 0x1234_5678,
            });
        }
        let src = &self.blocks[block_idx as usize];
        let n = buf.len().min(src.len());
        buf[..n].copy_from_slice(&src[..n]);
        Ok(())
    }

    fn write_block(&mut self, block_idx: u64, buf: &[u8]) -> Result<(), StorageError> {
        let max = self.blocks.len() as u64;
        if block_idx >= max {
            return Err(StorageError::OutOfRange { block_idx, max });
        }
        // If the block is bad, redirect to a replacement from the pool.
        let target = if self.bad_block_table.is_bad(block_idx) {
            self.bad_block_table.get_replacement(block_idx)?
        } else {
            block_idx
        };
        if target >= max {
            return Err(StorageError::OutOfRange {
                block_idx: target,
                max,
            });
        }
        let dst = &mut self.blocks[target as usize];
        let n = buf.len().min(dst.len());
        dst[..n].copy_from_slice(&buf[..n]);
        Ok(())
    }

    fn erase_block(&mut self, block_idx: u64) -> Result<(), StorageError> {
        let max = self.blocks.len() as u64;
        if block_idx >= max {
            return Err(StorageError::OutOfRange { block_idx, max });
        }
        // Erase = zero out the block (mock convention).
        for byte in &mut self.blocks[block_idx as usize] {
            *byte = 0;
        }
        Ok(())
    }

    fn block_count(&self) -> u64 {
        self.blocks.len() as u64
    }

    fn block_size(&self) -> usize {
        self.block_size
    }

    fn flush(&mut self) -> Result<(), StorageError> {
        // RAM-backed — no flush needed.
        Ok(())
    }

    fn health_status(&self) -> DeviceHealth {
        DeviceHealth {
            total_blocks: self.blocks.len() as u64,
            bad_blocks: self.bad_block_table.count() as u64,
            wear_level: self.bad_block_table.wear_level(),
            temperature: 25, // mock: room temperature
            remaining_life: self.bad_block_table.remaining_life(),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    #[test]
    fn test_new_device_geometry() {
        let dev = MockBlockDevice::new(100, 512);
        assert_eq!(dev.block_count(), 100);
        assert_eq!(dev.block_size(), 512);
    }

    #[test]
    fn test_new_device_zeroed() {
        let dev = MockBlockDevice::new(4, 512);
        let mut buf = [0xFF; 512];
        dev.read_block(0, &mut buf).expect("read should succeed");
        assert_eq!(buf, [0u8; 512]);
    }

    #[test]
    fn test_read_write_roundtrip() {
        let mut dev = MockBlockDevice::new(4, 512);
        let input = [0xAB; 512];
        dev.write_block(0, &input).expect("write should succeed");
        let mut out = [0u8; 512];
        dev.read_block(0, &mut out).expect("read should succeed");
        assert_eq!(out, input);
    }

    #[test]
    fn test_write_then_read_different_block() {
        let mut dev = MockBlockDevice::new(4, 512);
        let a = [0x11; 512];
        let b = [0x22; 512];
        dev.write_block(1, &a).expect("write block 1");
        dev.write_block(2, &b).expect("write block 2");
        let mut out = [0u8; 512];
        dev.read_block(1, &mut out).expect("read block 1");
        assert_eq!(out, a);
        dev.read_block(2, &mut out).expect("read block 2");
        assert_eq!(out, b);
    }

    #[test]
    fn test_read_out_of_range() {
        let dev = MockBlockDevice::new(4, 512);
        let mut buf = [0u8; 512];
        let err = dev.read_block(4, &mut buf).unwrap_err();
        match err {
            StorageError::OutOfRange { block_idx, max } => {
                assert_eq!(block_idx, 4);
                assert_eq!(max, 4);
            }
            other => panic!("expected OutOfRange, got {:?}", other),
        }
    }

    #[test]
    fn test_write_out_of_range() {
        let mut dev = MockBlockDevice::new(4, 512);
        let buf = [0u8; 512];
        let err = dev.write_block(100, &buf).unwrap_err();
        assert!(matches!(err, StorageError::OutOfRange { .. }));
    }

    #[test]
    fn test_erase_out_of_range() {
        let mut dev = MockBlockDevice::new(4, 512);
        let err = dev.erase_block(99).unwrap_err();
        assert!(matches!(err, StorageError::OutOfRange { .. }));
    }

    #[test]
    fn test_bad_block_read_error() {
        let mut dev = MockBlockDevice::new(8, 512);
        dev.mark_bad(3);
        let mut buf = [0u8; 512];
        let err = dev.read_block(3, &mut buf).unwrap_err();
        match err {
            StorageError::BadBlock { block_idx } => assert_eq!(block_idx, 3),
            other => panic!("expected BadBlock, got {:?}", other),
        }
    }

    #[test]
    fn test_bad_block_write_redirects() {
        // 100 blocks, 5 reserved → writes to bad blocks redirect to a
        // replacement in the reserved pool.
        let mut dev = MockBlockDevice::new(100, 512);
        dev.mark_bad(10);
        let input = [0xCD; 512];
        // Write to bad block 10 → should redirect to a reserved block and succeed.
        dev.write_block(10, &input)
            .expect("write to bad block should redirect");
        // The bad block itself should still read as BadBlock.
        let mut buf = [0u8; 512];
        let err = dev.read_block(10, &mut buf).unwrap_err();
        assert!(matches!(err, StorageError::BadBlock { .. }));
    }

    #[test]
    fn test_crc_error_injection() {
        let mut dev = MockBlockDevice::new(4, 512);
        dev.inject_crc_error(2);
        let mut buf = [0u8; 512];
        let err = dev.read_block(2, &mut buf).unwrap_err();
        match err {
            StorageError::CrcMismatch { expected, actual } => {
                assert_ne!(expected, actual);
            }
            other => panic!("expected CrcMismatch, got {:?}", other),
        }
    }

    #[test]
    fn test_crc_error_injection_dedup() {
        let mut dev = MockBlockDevice::new(4, 512);
        dev.inject_crc_error(1);
        dev.inject_crc_error(1);
        // Should only be recorded once (no duplicate).
        assert_eq!(dev.crc_error_blocks.len(), 1);
    }

    #[test]
    fn test_erase_zeroes_block() {
        let mut dev = MockBlockDevice::new(4, 512);
        let input = [0xFF; 512];
        dev.write_block(0, &input).expect("write");
        dev.erase_block(0).expect("erase");
        let mut out = [0xFF; 512];
        dev.read_block(0, &mut out).expect("read");
        assert_eq!(out, [0u8; 512]);
    }

    #[test]
    fn test_health_status_clean() {
        let dev = MockBlockDevice::new(100, 512);
        let h = dev.health_status();
        assert_eq!(h.total_blocks, 100);
        assert_eq!(h.bad_blocks, 0);
        assert_eq!(h.wear_level, 0);
        assert_eq!(h.remaining_life, 100);
        assert_eq!(h.temperature, 25);
    }

    #[test]
    fn test_health_status_with_bad_blocks() {
        let mut dev = MockBlockDevice::new(100, 512);
        dev.mark_bad(0);
        dev.mark_bad(1);
        dev.mark_bad(2);
        let h = dev.health_status();
        assert_eq!(h.bad_blocks, 3);
        // 3 bad out of 100 → wear = 3, life = 97.
        assert_eq!(h.wear_level, 3);
        assert_eq!(h.remaining_life, 97);
    }

    #[test]
    fn test_flush_ok() {
        let mut dev = MockBlockDevice::new(4, 512);
        assert!(dev.flush().is_ok());
    }

    #[test]
    fn test_multi_block_sequential_io() {
        let mut dev = MockBlockDevice::new(16, 256);
        // Write a pattern across blocks 0..4.
        for i in 0..4u64 {
            let buf = [(i as u8); 256];
            dev.write_block(i, &buf).expect("write");
        }
        // Read back and verify.
        for i in 0..4u64 {
            let mut buf = [0u8; 256];
            dev.read_block(i, &mut buf).expect("read");
            assert_eq!(buf, [i as u8; 256]);
        }
    }

    #[test]
    fn test_overwrite_block() {
        let mut dev = MockBlockDevice::new(2, 64);
        dev.write_block(0, &[0xAA; 64]).expect("write 1");
        dev.write_block(0, &[0xBB; 64]).expect("write 2");
        let mut out = [0u8; 64];
        dev.read_block(0, &mut out).expect("read");
        assert_eq!(out, [0xBB; 64]);
    }

    #[test]
    fn test_partial_buffer_write() {
        // Buffer smaller than block_size — only the overlapping prefix is
        // written; the rest of the block is preserved.
        let mut dev = MockBlockDevice::new(2, 8);
        dev.write_block(0, &[0xFF; 8]).expect("full write");
        dev.write_block(0, &[0x11; 4]).expect("partial write");
        let mut out = [0u8; 8];
        dev.read_block(0, &mut out).expect("read");
        assert_eq!(out, [0x11, 0x11, 0x11, 0x11, 0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn test_zero_block_count() {
        let dev = MockBlockDevice::new(0, 512);
        assert_eq!(dev.block_count(), 0);
        let mut buf = [0u8; 512];
        let err = dev.read_block(0, &mut buf).unwrap_err();
        assert!(matches!(err, StorageError::OutOfRange { .. }));
    }
}
