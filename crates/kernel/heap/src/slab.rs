//! Slab allocator for small object allocation (8 B – 1 KB).
//!
//! Each [`SlabCache`] manages a pool of fixed-size object slots backed by
//! pages from the [`BuddyAllocator`]. Free slots form an intrusive singly
//! linked list: the first 8 bytes of each free slot store an
//! `Option<*mut u8>` pointing to the next free slot (`None` = end of list).
//!
//! All slot sizes in [`SLAB_SIZES`] are powers of two ≥ 8, guaranteeing
//! 8-byte alignment for the next-pointer.

use core::ptr;

use crate::buddy::{BuddyAllocator, PAGE_SIZE};

/// Slab bucket object sizes (bytes). All are powers of two, ≥ 8.
pub const SLAB_SIZES: [usize; 8] = [8, 16, 32, 64, 128, 256, 512, 1024];

/// A slab cache for fixed-size objects.
///
/// **Not** `Copy`/`Clone` — contains a raw free-list head pointer.
pub struct SlabCache {
    /// Size of each object slot in bytes.
    pub obj_size: usize,
    /// Head of the intrusive free-slot linked list.
    pub free_head: Option<*mut u8>,
    /// Total number of slots allocated from the buddy allocator.
    pub total: usize,
    /// Number of slots currently in use (allocated to callers).
    pub used: usize,
}

impl SlabCache {
    /// Creates a new empty slab cache for objects of `obj_size` bytes.
    pub const fn new(obj_size: usize) -> Self {
        Self {
            obj_size,
            free_head: None,
            total: 0,
            used: 0,
        }
    }

    /// Allocates one object slot, growing from the buddy allocator if needed.
    ///
    /// Returns a null pointer if the buddy allocator is out of memory.
    ///
    /// # Safety
    ///
    /// `buddy` must be a valid, initialized buddy allocator.
    pub unsafe fn alloc(&mut self, buddy: &mut BuddyAllocator) -> *mut u8 {
        // Fast path: pop from the free list.
        if let Some(slot) = self.free_head {
            self.free_head = *(slot.cast::<Option<*mut u8>>());
            self.used += 1;
            return slot;
        }
        // Slow path: request a new page from the buddy allocator.
        let page = buddy.alloc(PAGE_SIZE);
        if page.is_null() {
            return ptr::null_mut();
        }
        let slots = PAGE_SIZE / self.obj_size;
        // Build the intrusive free list: slot[0] → slot[1] → … → slot[n-1] → None.
        for i in 0..slots {
            let slot = page.add(i * self.obj_size);
            let next = if i + 1 < slots {
                Some(page.add((i + 1) * self.obj_size))
            } else {
                None
            };
            *(slot.cast::<Option<*mut u8>>()) = next;
        }
        self.free_head = Some(page);
        self.total += slots;
        // The free list is now populated; recurse to pop the first slot.
        self.alloc(buddy)
    }

    /// Frees an object slot, returning it to the free list.
    ///
    /// # Safety
    ///
    /// `ptr` must be a pointer previously returned by [`alloc`](Self::alloc)
    /// on this `SlabCache`, and must not have been already freed.
    pub unsafe fn dealloc(&mut self, ptr: *mut u8) {
        // Push ptr to the front of the free list.
        *(ptr.cast::<Option<*mut u8>>()) = self.free_head;
        self.free_head = Some(ptr);
        self.used = self.used.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros, static_mut_refs)]

    use super::*;

    /// 4096-aligned 256 KB pool for slab allocator tests (64 pages).
    #[repr(C, align(4096))]
    struct TestPool {
        data: [u8; 256 * 1024],
    }

    #[test]
    fn test_slab_new() {
        let slab = SlabCache::new(64);
        assert_eq!(slab.obj_size, 64);
        assert!(slab.free_head.is_none());
        assert_eq!(slab.total, 0);
        assert_eq!(slab.used, 0);
    }

    #[test]
    fn test_slab_alloc_first() {
        static mut POOL: TestPool = TestPool {
            data: [0; 256 * 1024],
        };
        unsafe {
            let mut buddy = BuddyAllocator::new();
            buddy.init(POOL.data.as_mut_ptr(), 64);
            let mut slab = SlabCache::new(64);
            let ptr = slab.alloc(&mut buddy);
            assert!(!ptr.is_null());
            // First alloc should have requested a page and created slots.
            assert!(slab.total > 0, "total should be > 0 after first alloc");
            assert_eq!(slab.used, 1);
            // PAGE_SIZE / 64 = 64 slots per page.
            assert_eq!(slab.total, PAGE_SIZE / 64);
        }
    }

    #[test]
    fn test_slab_alloc_cached() {
        static mut POOL: TestPool = TestPool {
            data: [0; 256 * 1024],
        };
        unsafe {
            let mut buddy = BuddyAllocator::new();
            buddy.init(POOL.data.as_mut_ptr(), 64);
            let mut slab = SlabCache::new(64);
            // First alloc: requests a page from buddy.
            let p1 = slab.alloc(&mut buddy);
            assert!(!p1.is_null());
            let total_after_first = slab.total;
            // Second alloc: should come from the free list (no new page).
            let p2 = slab.alloc(&mut buddy);
            assert!(!p2.is_null());
            assert_eq!(
                slab.total, total_after_first,
                "total should not increase on cached alloc"
            );
            assert_eq!(slab.used, 2);
            assert_ne!(p1, p2);
        }
    }

    #[test]
    fn test_slab_dealloc() {
        static mut POOL: TestPool = TestPool {
            data: [0; 256 * 1024],
        };
        unsafe {
            let mut buddy = BuddyAllocator::new();
            buddy.init(POOL.data.as_mut_ptr(), 64);
            let mut slab = SlabCache::new(64);
            let ptr = slab.alloc(&mut buddy);
            assert!(!ptr.is_null());
            assert_eq!(slab.used, 1);
            slab.dealloc(ptr);
            assert_eq!(slab.used, 0);
            // After dealloc, the slot should be reusable.
            let ptr2 = slab.alloc(&mut buddy);
            assert!(!ptr2.is_null());
            assert_eq!(ptr, ptr2, "freed slot should be reused");
            assert_eq!(slab.used, 1);
        }
    }

    #[test]
    fn test_slab_multiple_buckets() {
        static mut POOL: TestPool = TestPool {
            data: [0; 256 * 1024],
        };
        unsafe {
            let mut buddy = BuddyAllocator::new();
            buddy.init(POOL.data.as_mut_ptr(), 64);

            // Test all slab sizes.
            for &size in &SLAB_SIZES {
                let mut slab = SlabCache::new(size);
                let ptr = slab.alloc(&mut buddy);
                assert!(!ptr.is_null(), "alloc for size {size} failed");
                assert_eq!(slab.obj_size, size);
                assert_eq!(slab.total, PAGE_SIZE / size);
                assert_eq!(slab.used, 1);
                slab.dealloc(ptr);
                assert_eq!(slab.used, 0);
            }
        }
    }

    #[test]
    fn test_slab_oom() {
        static mut POOL: TestPool = TestPool {
            data: [0; 256 * 1024],
        };
        unsafe {
            let mut buddy = BuddyAllocator::new();
            buddy.init(POOL.data.as_mut_ptr(), 64);
            let mut slab = SlabCache::new(1024);
            // Each page yields 4 slots (4096 / 1024). Exhaust all pages.
            let mut count = 0;
            loop {
                let ptr = slab.alloc(&mut buddy);
                if ptr.is_null() {
                    break;
                }
                count += 1;
            }
            // 64 pages * 4 slots/page = 256 slots.
            assert_eq!(count, 256);
        }
    }
}
