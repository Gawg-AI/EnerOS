//! DMA buffer coherence management — v0.17.0.
//!
//! On non-coherent DMA systems, the CPU cache must be flushed (clean) before
//! a buffer is handed to a DMA device for reading, and invalidated before
//! the CPU reads a buffer written by a DMA device. On coherent systems
//! (hardware IOMMU), these operations are no-ops.
//!
//! `DmaBuffer` holds a raw pointer (`virt`) and is **not** `Send`/`Sync` —
//! the caller is responsible for lifetime and cross-core safety.

#![allow(dead_code)]

use crate::coherence::{cache_clean, cache_invalidate};

/// A DMA buffer with optional manual cache maintenance.
///
/// - `coherent == true`: hardware maintains coherence; sync operations are
///   no-ops.
/// - `coherent == false`: `sync_for_device()` cleans (writes back) the cache
///   before DMA reads; `sync_for_cpu()` invalidates the cache before CPU
///   reads.
#[derive(Debug)]
pub struct DmaBuffer {
    /// Physical address of the buffer (for DMA controller programming).
    pub phys: u64,
    /// Virtual address of the buffer (for CPU access).
    pub virt: *mut u8,
    /// Buffer size in bytes.
    pub size: usize,
    /// Whether the platform maintains DMA coherence in hardware.
    pub coherent: bool,
}

impl DmaBuffer {
    /// Sync the buffer for device access (CPU → DMA).
    ///
    /// On non-coherent systems, cleans (writes back) dirty cache lines so
    /// the device reads fresh data from memory. No-op on coherent systems.
    pub fn sync_for_device(&self) {
        if !self.coherent {
            cache_clean(self.virt as u64, self.size);
        }
    }

    /// Sync the buffer for CPU access (DMA → CPU).
    ///
    /// On non-coherent systems, invalidates cache lines so the CPU reads
    /// fresh data written by the device from memory. No-op on coherent
    /// systems.
    pub fn sync_for_cpu(&self) {
        if !self.coherent {
            cache_invalidate(self.virt as u64, self.size);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn test_dma_buffer_coherent_noop() {
        let _g = lock();
        let buf = DmaBuffer {
            phys: 0x4000_0000,
            virt: core::ptr::null_mut(),
            size: 0,
            coherent: true,
        };
        // Coherent buffers are no-ops; should not panic even with null virt.
        buf.sync_for_device();
        buf.sync_for_cpu();
    }

    #[test]
    fn test_dma_buffer_non_coherent_no_panic() {
        let _g = lock();
        let buf = DmaBuffer {
            phys: 0x5000_0000,
            virt: core::ptr::null_mut(),
            size: 0,
            coherent: false,
        };
        // Non-coherent with size 0 → cache_clean/invalidate loop runs zero
        // iterations; should not panic on host (no-ops).
        buf.sync_for_device();
        buf.sync_for_cpu();
    }

    #[test]
    fn test_dma_buffer_construction() {
        let _g = lock();
        let buf = DmaBuffer {
            phys: 0x6000_0000,
            virt: core::ptr::null_mut(),
            size: 4096,
            coherent: false,
        };
        assert_eq!(buf.phys, 0x6000_0000);
        assert!(buf.virt.is_null());
        assert_eq!(buf.size, 4096);
        assert!(!buf.coherent);
    }
}
