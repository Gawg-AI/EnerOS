//! EnerOS Kernel Heap Allocator — slab + buddy hybrid algorithm.
//!
//! This crate implements a no_std kernel heap allocator combining:
//! - **Slab allocator** for small objects (8–1024 bytes), O(1) allocation
//! - **Buddy allocator** for large blocks (page-level), supports splitting/merging
//!
//! The `KernelHeap` type implements `core::alloc::GlobalAlloc` and is registered
//! as the global allocator via `#[global_allocator]` in non-test builds.
//!
//! # Usage
//!
//! ```no_run
//! # #![allow(unused)]
//! use eneros_heap::{heap_init, heap_stats};
//!
//! // Initialize with a 4MB heap pool (caller-provided page-aligned region).
//! static mut HEAP_POOL: [u8; 4 * 1024 * 1024] = [0; 4 * 1024 * 1024];
//! unsafe { heap_init(HEAP_POOL.as_mut_ptr(), 4 * 1024 * 1024); }
//!
//! // Query statistics
//! let stats = heap_stats();
//! assert!(stats.total_bytes > 0);
//! ```

#![cfg_attr(not(test), no_std)]

pub mod buddy;
pub mod slab;
pub mod stats;

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;

use spin::Mutex;

use crate::buddy::BuddyAllocator;
use crate::slab::{SlabCache, SLAB_SIZES};
use crate::stats::HeapStats;

/// Internal mutable state of the kernel heap.
pub struct KernelHeapInner {
    /// Buddy allocator for page-level blocks (> 1024 bytes).
    pub buddy: BuddyAllocator,
    /// Slab caches for small objects, indexed by [`SLAB_SIZES`].
    pub slabs: [SlabCache; 8],
    /// Heap statistics.
    pub stats: HeapStats,
}

// SAFETY: `KernelHeapInner` contains raw pointers (`*mut u8`) inside
// `BuddyAllocator` and `SlabCache`. These are only accessed under the
// `KERNEL_HEAP` `Mutex`, which guarantees exclusive access. The raw
// pointers refer to a single heap region provided by `heap_init` and are
// never shared across threads without the mutex.
unsafe impl Send for KernelHeapInner {}
unsafe impl Sync for KernelHeapInner {}

/// Zero-sized type implementing [`GlobalAlloc`].
///
/// All state lives in the static [`KERNEL_HEAP`]; `KernelHeap` is just a
/// handle for the trait implementation.
pub struct KernelHeap;

/// Global heap instance.
static KERNEL_HEAP: Mutex<Option<KernelHeapInner>> = Mutex::new(None);

/// Finds the smallest slab bucket index that fits `size` bytes.
///
/// Returns `None` if `size` exceeds the largest slab size (1024 bytes),
/// indicating the allocation should go to the buddy allocator.
fn slab_bucket_for(size: usize) -> Option<usize> {
    SLAB_SIZES.iter().position(|&s| size <= s)
}

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size();
        let mut guard = KERNEL_HEAP.lock();
        let inner = match guard.as_mut() {
            Some(inner) => inner,
            None => return ptr::null_mut(),
        };
        inner.stats.alloc_count += 1;
        inner.stats.allocated_bytes += size as u64;

        if let Some(bucket) = slab_bucket_for(size) {
            inner.stats.slab_hits += 1;
            inner.slabs[bucket].alloc(&mut inner.buddy)
        } else {
            inner.stats.buddy_hits += 1;
            inner.buddy.alloc(size)
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let size = layout.size();
        let mut guard = KERNEL_HEAP.lock();
        let inner = match guard.as_mut() {
            Some(inner) => inner,
            None => return,
        };
        inner.stats.free_count += 1;
        inner.stats.allocated_bytes = inner.stats.allocated_bytes.saturating_sub(size as u64);

        if let Some(bucket) = slab_bucket_for(size) {
            inner.slabs[bucket].dealloc(ptr);
        } else {
            inner.buddy.dealloc(ptr, size);
        }
    }
}

/// Initializes the kernel heap with a page-aligned region of `size` bytes.
///
/// Must be called once before any allocation via `KernelHeap` or the
/// `alloc` crate. Calling it again replaces the previous heap state.
///
/// # Safety
///
/// `base` must be a valid, page-aligned, writable pointer to at least
/// `size` bytes for the program's lifetime.
pub unsafe fn heap_init(base: *mut u8, size: usize) {
    let pages = size / crate::buddy::PAGE_SIZE;
    let mut buddy = BuddyAllocator::new();
    unsafe {
        buddy.init(base, pages);
    }
    let slabs = [
        SlabCache::new(8),
        SlabCache::new(16),
        SlabCache::new(32),
        SlabCache::new(64),
        SlabCache::new(128),
        SlabCache::new(256),
        SlabCache::new(512),
        SlabCache::new(1024),
    ];
    let stats = HeapStats {
        total_bytes: size as u64,
        free_bytes: size as u64,
        ..Default::default()
    };
    *KERNEL_HEAP.lock() = Some(KernelHeapInner {
        buddy,
        slabs,
        stats,
    });
}

/// Returns a snapshot of the current heap statistics.
///
/// Returns default (all-zero) stats if the heap has not been initialized.
pub fn heap_stats() -> HeapStats {
    KERNEL_HEAP
        .lock()
        .as_ref()
        .map(|h| h.stats)
        .unwrap_or_default()
}

// NOTE: This library crate intentionally does NOT register a `#[global_allocator]`.
// Consumer binary crates are responsible for registering `KernelHeap` (or any
// other allocator) as their global allocator. This avoids conflicts when
// multiple crates in the same workspace depend on `eneros-heap`.
//
// Example (in a binary crate):
// ```
// use eneros_heap::KernelHeap;
// #[global_allocator]
// static ALLOC: KernelHeap = KernelHeap;
// ```

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros, static_mut_refs)]

    use core::alloc::GlobalAlloc;

    use super::*;

    /// 4096-aligned 256 KB pool for integration tests (64 pages).
    #[repr(C, align(4096))]
    struct TestPool {
        data: [u8; 256 * 1024],
    }

    /// Integration test covering heap_init, alloc/dealloc, slab/buddy hit
    /// rates, stats consistency, and OOM. All sub-tests run sequentially in
    /// a single function because they share the global `KERNEL_HEAP`.
    #[test]
    fn test_heap_integration() {
        static mut POOL: TestPool = TestPool {
            data: [0; 256 * 1024],
        };
        unsafe {
            // --- 1. heap_init ---
            heap_init(POOL.data.as_mut_ptr(), 256 * 1024);
            let stats = heap_stats();
            assert!(
                stats.total_bytes > 0,
                "total_bytes should be > 0 after heap_init"
            );
            assert_eq!(stats.total_bytes, 256 * 1024);

            // --- 2. alloc/dealloc (small object via slab) ---
            let heap = KernelHeap;
            let small_layout = Layout::from_size_align(64, 8).unwrap();
            let ptr = GlobalAlloc::alloc(&heap, small_layout);
            assert!(!ptr.is_null(), "small alloc should succeed");
            GlobalAlloc::dealloc(&heap, ptr, small_layout);

            // --- 3. alloc/dealloc (large object via buddy) ---
            let large_layout = Layout::from_size_align(2048, 8).unwrap();
            let ptr = GlobalAlloc::alloc(&heap, large_layout);
            assert!(!ptr.is_null(), "large alloc should succeed");
            GlobalAlloc::dealloc(&heap, ptr, large_layout);

            // --- 4. slab/buddy hit rates ---
            let stats_before = heap_stats();
            // Small allocations → slab hits.
            for _ in 0..10 {
                let l = Layout::from_size_align(32, 8).unwrap();
                let p = GlobalAlloc::alloc(&heap, l);
                assert!(!p.is_null());
                GlobalAlloc::dealloc(&heap, p, l);
            }
            // Large allocation → buddy hit.
            let l = Layout::from_size_align(2048, 8).unwrap();
            let p = GlobalAlloc::alloc(&heap, l);
            assert!(!p.is_null());
            GlobalAlloc::dealloc(&heap, p, l);

            let stats_after = heap_stats();
            assert!(
                stats_after.slab_hits > stats_before.slab_hits,
                "slab_hits should increase after small allocations"
            );
            assert!(
                stats_after.buddy_hits > stats_before.buddy_hits,
                "buddy_hits should increase after large allocation"
            );

            // --- 5. stats consistency ---
            let stats_before = heap_stats();
            let alloc_layout = Layout::from_size_align(48, 8).unwrap();
            let p1 = GlobalAlloc::alloc(&heap, alloc_layout);
            assert!(!p1.is_null());
            let p2 = GlobalAlloc::alloc(&heap, alloc_layout);
            assert!(!p2.is_null());
            GlobalAlloc::dealloc(&heap, p1, alloc_layout);
            GlobalAlloc::dealloc(&heap, p2, alloc_layout);
            let stats_after = heap_stats();
            assert_eq!(
                stats_after.alloc_count - stats_before.alloc_count,
                2,
                "alloc_count should increase by 2"
            );
            assert_eq!(
                stats_after.free_count - stats_before.free_count,
                2,
                "free_count should increase by 2"
            );

            // --- 6. OOM: exhaust the pool ---
            // Re-init to get a fresh pool for the OOM test.
            heap_init(POOL.data.as_mut_ptr(), 256 * 1024);
            let buddy_layout = Layout::from_size_align(2048, 8).unwrap();
            let mut count = 0;
            loop {
                let p = GlobalAlloc::alloc(&heap, buddy_layout);
                if p.is_null() {
                    break;
                }
                count += 1;
            }
            assert!(count > 0, "should have allocated some blocks before OOM");
            // The next allocation after OOM should also fail.
            let p = GlobalAlloc::alloc(&heap, buddy_layout);
            assert!(p.is_null(), "alloc after OOM should return null");

            // Oversized allocation (> MAX_ORDER block) should also fail.
            let huge_layout = Layout::from_size_align(8 * 1024 * 1024, 4096).unwrap();
            let p = GlobalAlloc::alloc(&heap, huge_layout);
            assert!(p.is_null(), "oversized alloc should return null");
        }
    }

    #[test]
    fn test_slab_bucket_for() {
        // Sizes within slab range.
        assert_eq!(slab_bucket_for(1), Some(0)); // ≤ 8
        assert_eq!(slab_bucket_for(8), Some(0));
        assert_eq!(slab_bucket_for(9), Some(1)); // ≤ 16
        assert_eq!(slab_bucket_for(16), Some(1));
        assert_eq!(slab_bucket_for(64), Some(3));
        assert_eq!(slab_bucket_for(1024), Some(7));
        // Sizes beyond slab range → None (buddy).
        assert_eq!(slab_bucket_for(1025), None);
        assert_eq!(slab_bucket_for(4096), None);
    }
}
