//! DMA transfer abstraction and mock implementation.
//!
//! [`DmaBuffer`] wraps a raw pointer plus physical address for DMA descriptor
//! construction. [`MockDmaTransfer`] implements [`DmaTransfer`] using a plain
//! `Vec<u8>` backing store so the DMA code path is exercisable on the host.

use alloc::vec::Vec;

use crate::driver::DmaTransfer;
use crate::error::StorageError;

/// Owned DMA buffer: raw pointer + size + physical address.
///
/// The pointer is owned exclusively by this `DmaBuffer` and is not shared
/// across threads, so `Send` is sound.
pub struct DmaBuffer {
    /// Virtual address (host or kernel) of the buffer.
    ptr: *mut u8,
    /// Size in bytes.
    size: usize,
    /// Physical address for the DMA engine.
    phys_addr: u64,
}

// SAFETY: DmaBuffer owns its pointer exclusively. It is not shared across
// threads (no interior mutability, no `Rc`/`Arc`), so transferring it to
// another thread is safe.
unsafe impl Send for DmaBuffer {}

impl DmaBuffer {
    /// Creates a new DMA buffer descriptor.
    ///
    /// # Safety
    ///
    /// The caller must ensure `ptr` is valid for `size` bytes and remains
    /// valid for the lifetime of this `DmaBuffer`.
    pub fn new(ptr: *mut u8, size: usize, phys_addr: u64) -> Self {
        DmaBuffer {
            ptr,
            size,
            phys_addr,
        }
    }

    /// Returns the virtual address of the buffer.
    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr
    }

    /// Returns the size in bytes.
    pub fn len(&self) -> usize {
        self.size
    }

    /// Returns `true` if the buffer has zero length.
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Returns the physical address of the buffer (for the DMA engine).
    pub fn phys_addr(&self) -> u64 {
        self.phys_addr
    }
}

/// Mock DMA transfer engine backed by a `Vec<u8>`.
///
/// Implements [`DmaTransfer`] so the DMA code path (bounds checking, buffer
/// slicing, error classification) is fully exercisable on the host without
/// real DMA hardware.
pub struct MockDmaTransfer {
    /// Backing store (device memory in the mock).
    storage: Vec<u8>,
    /// Block size in bytes.
    block_size: usize,
}

impl MockDmaTransfer {
    /// Creates a new mock DMA engine with `total_bytes` of backing storage
    /// and the given `block_size`.
    pub fn new(total_bytes: usize, block_size: usize) -> Self {
        MockDmaTransfer {
            storage: alloc::vec![0u8; total_bytes],
            block_size,
        }
    }

    /// Returns the block size in bytes.
    #[allow(dead_code)]
    pub fn block_size(&self) -> usize {
        self.block_size
    }

    /// Returns the total backing storage size in bytes.
    #[allow(dead_code)]
    pub fn capacity(&self) -> usize {
        self.storage.len()
    }
}

impl DmaTransfer for MockDmaTransfer {
    fn dma_read(&mut self, block_idx: u64, count: u32, buf: &mut [u8]) -> Result<(), StorageError> {
        let start = (block_idx as usize)
            .checked_mul(self.block_size)
            .ok_or(StorageError::DmaError { code: 0x01 })?;
        let len = (count as usize)
            .checked_mul(self.block_size)
            .ok_or(StorageError::DmaError { code: 0x02 })?;
        if start + len > self.storage.len() {
            return Err(StorageError::DmaError { code: 0x10 });
        }
        if buf.len() < len {
            return Err(StorageError::DmaError { code: 0x11 });
        }
        buf[..len].copy_from_slice(&self.storage[start..start + len]);
        Ok(())
    }

    fn dma_write(&mut self, block_idx: u64, count: u32, buf: &[u8]) -> Result<(), StorageError> {
        let start = (block_idx as usize)
            .checked_mul(self.block_size)
            .ok_or(StorageError::DmaError { code: 0x01 })?;
        let len = (count as usize)
            .checked_mul(self.block_size)
            .ok_or(StorageError::DmaError { code: 0x02 })?;
        if start + len > self.storage.len() {
            return Err(StorageError::DmaError { code: 0x10 });
        }
        if buf.len() < len {
            return Err(StorageError::DmaError { code: 0x11 });
        }
        self.storage[start..start + len].copy_from_slice(&buf[..len]);
        Ok(())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dma_buffer_construction() {
        let mut data = [0u8; 16];
        let buf = DmaBuffer::new(data.as_mut_ptr(), 16, 0x4000_0000);
        assert_eq!(buf.len(), 16);
        assert!(!buf.is_empty());
        assert_eq!(buf.phys_addr(), 0x4000_0000);
    }

    #[test]
    fn test_dma_buffer_empty() {
        let buf = DmaBuffer::new(core::ptr::null_mut(), 0, 0);
        assert_eq!(buf.len(), 0);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_dma_buffer_as_ptr() {
        let mut data = [0u8; 8];
        let ptr = data.as_mut_ptr();
        let buf = DmaBuffer::new(ptr, 8, 0);
        assert_eq!(buf.as_ptr(), ptr);
    }

    #[test]
    fn test_mock_dma_new() {
        let dma = MockDmaTransfer::new(4096, 512);
        assert_eq!(dma.capacity(), 4096);
        assert_eq!(dma.block_size(), 512);
    }

    #[test]
    fn test_dma_read_write_roundtrip() {
        let mut dma = MockDmaTransfer::new(4096, 512);
        let input = [0xAA; 512];
        dma.dma_write(0, 1, &input).expect("write should succeed");
        let mut out = [0u8; 512];
        dma.dma_read(0, 1, &mut out).expect("read should succeed");
        assert_eq!(out, input);
    }

    #[test]
    fn test_dma_multi_block_roundtrip() {
        let mut dma = MockDmaTransfer::new(4096, 512);
        let input = [0xBB; 1024]; // 2 blocks
        dma.dma_write(2, 2, &input).expect("write 2 blocks");
        let mut out = [0u8; 1024];
        dma.dma_read(2, 2, &mut out).expect("read 2 blocks");
        assert_eq!(out, input);
    }

    #[test]
    fn test_dma_write_out_of_bounds() {
        let mut dma = MockDmaTransfer::new(1024, 512);
        let input = [0u8; 512];
        // Block 2 (offset 1024) is exactly at the end → out of bounds.
        let err = dma.dma_write(2, 1, &input).unwrap_err();
        assert!(matches!(err, StorageError::DmaError { .. }));
    }

    #[test]
    fn test_dma_read_out_of_bounds() {
        let mut dma = MockDmaTransfer::new(1024, 512);
        let mut out = [0u8; 512];
        let err = dma.dma_read(3, 1, &mut out).unwrap_err();
        assert!(matches!(err, StorageError::DmaError { .. }));
    }

    #[test]
    fn test_dma_read_buffer_too_small() {
        let mut dma = MockDmaTransfer::new(4096, 512);
        // Request 2 blocks (1024 bytes) but provide a 512-byte buffer.
        let mut out = [0u8; 512];
        let err = dma.dma_read(0, 2, &mut out).unwrap_err();
        assert!(matches!(err, StorageError::DmaError { .. }));
    }

    #[test]
    fn test_dma_write_buffer_too_small() {
        let mut dma = MockDmaTransfer::new(4096, 512);
        let input = [0u8; 512];
        let err = dma.dma_write(0, 2, &input).unwrap_err();
        assert!(matches!(err, StorageError::DmaError { .. }));
    }

    #[test]
    fn test_dma_non_overlapping_blocks() {
        let mut dma = MockDmaTransfer::new(4096, 512);
        let a = [0x11; 512];
        let b = [0x22; 512];
        dma.dma_write(0, 1, &a).expect("write block 0");
        dma.dma_write(1, 1, &b).expect("write block 1");
        let mut out = [0u8; 512];
        dma.dma_read(0, 1, &mut out).expect("read block 0");
        assert_eq!(out, a);
        dma.dma_read(1, 1, &mut out).expect("read block 1");
        assert_eq!(out, b);
    }

    #[test]
    fn test_dma_send() {
        // Verify DmaBuffer is Send (compiles if true).
        fn assert_send<T: Send>() {}
        assert_send::<DmaBuffer>();
    }
}
