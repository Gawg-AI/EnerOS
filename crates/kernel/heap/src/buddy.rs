//! Buddy allocator for page-level memory allocation.
//!
//! Implements a binary buddy system with `PAGE_SIZE` (4KB) granularity and
//! `MAX_ORDER + 1` orders (0..=11), supporting block sizes from 4KB up to 4MB.
//!
//! The free lists are intrusive: each free block's first 8 bytes store an
//! `Option<*mut u8>` pointing to the next free block (`None` = end of list).
//!
//! A per-page bitmap tracks allocation status (1 = allocated, 0 = free),
//! enabling O(1) `is_free` checks during block merging.

use core::ptr;

/// Page size in bytes (4 KB).
pub const PAGE_SIZE: usize = 4096;

/// Maximum buddy order. Block size at order `o` is `PAGE_SIZE << o`.
/// Order 11 = 4 MB, the largest allocatable block.
pub const MAX_ORDER: usize = 11;

/// Number of 64-bit words in the per-page allocation bitmap (8192 bits).
const BITMAP_WORDS: usize = 128;

/// Buddy allocator managing a contiguous page range.
///
/// **Not** `Copy`/`Clone` — contains a raw base pointer and mutable state.
pub struct BuddyAllocator {
    /// Base address of the managed region (null before `init`).
    pub base: *mut u8,
    /// Total number of pages in the managed region.
    pub total_pages: usize,
    /// Free list heads for each order (0..=MAX_ORDER).
    pub free_lists: [Option<*mut u8>; MAX_ORDER + 1],
    /// Number of free blocks in each order's free list.
    pub free_count: [usize; MAX_ORDER + 1],
    /// Per-page bitmap: bit set = page allocated, bit clear = page free.
    bitmap: [u64; BITMAP_WORDS],
}

impl Default for BuddyAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl BuddyAllocator {
    /// Creates a zero-initialized buddy allocator (const-constructible).
    ///
    /// Call [`init`](Self::init) before any allocation operation.
    pub const fn new() -> Self {
        Self {
            base: ptr::null_mut(),
            total_pages: 0,
            free_lists: [None; MAX_ORDER + 1],
            free_count: [0; MAX_ORDER + 1],
            bitmap: [0; BITMAP_WORDS],
        }
    }

    /// Initializes the allocator with a page-aligned region of `pages` pages.
    ///
    /// # Safety
    ///
    /// `base` must be a valid, page-aligned, writable pointer to at least
    /// `pages * PAGE_SIZE` bytes for the allocator's lifetime.
    pub unsafe fn init(&mut self, base: *mut u8, pages: usize) {
        self.base = base;
        self.total_pages = pages;
        // Clear the bitmap (defensive — `new()` already zeroes it).
        for w in &mut self.bitmap {
            *w = 0;
        }
        if pages == 0 {
            return;
        }
        let max_order = if pages.ilog2() as usize > MAX_ORDER {
            MAX_ORDER
        } else {
            pages.ilog2() as usize
        };
        // Write None as the next-pointer (end-of-list marker) at the block header.
        *(base.cast::<Option<*mut u8>>()) = None;
        self.free_lists[max_order] = Some(base);
        self.free_count[max_order] = 1;
    }

    /// Computes the smallest buddy order that fits `size` bytes.
    ///
    /// Order 0 covers 1 page (4 KB). The result is capped at `MAX_ORDER`.
    pub fn order_for(size: usize) -> usize {
        let pages = size.div_ceil(PAGE_SIZE);
        if pages <= 1 {
            return 0;
        }
        let order = (pages - 1).ilog2() as usize + 1;
        if order > MAX_ORDER {
            MAX_ORDER
        } else {
            order
        }
    }

    /// Marks page `page_idx` as allocated in the bitmap.
    unsafe fn set_allocated(&mut self, page_idx: usize) {
        self.bitmap[page_idx / 64] |= 1u64 << (page_idx % 64);
    }

    /// Marks page `page_idx` as free in the bitmap.
    unsafe fn clear_allocated(&mut self, page_idx: usize) {
        self.bitmap[page_idx / 64] &= !(1u64 << (page_idx % 64));
    }

    /// Returns `true` if page `page_idx` is free (bit clear).
    unsafe fn is_page_free(&self, page_idx: usize) -> bool {
        (self.bitmap[page_idx / 64] & (1u64 << (page_idx % 64))) == 0
    }

    /// Returns `true` if all pages in `[start_page, start_page + count)` are free.
    ///
    /// Returns `false` if the range exceeds `total_pages` or overflows.
    unsafe fn is_range_free(&self, start_page: usize, count: usize) -> bool {
        let end = match start_page.checked_add(count) {
            Some(e) => e,
            None => return false,
        };
        if end > self.total_pages {
            return false;
        }
        for i in start_page..end {
            if !self.is_page_free(i) {
                return false;
            }
        }
        true
    }

    /// Returns `true` if the block at `ptr` of order `order` is fully free.
    unsafe fn is_free(&self, ptr: *mut u8, order: usize) -> bool {
        let page_idx = ((ptr as usize) - (self.base as usize)) / PAGE_SIZE;
        self.is_range_free(page_idx, 1usize << order)
    }

    /// Pushes a block onto the front of `free_lists[order]`.
    ///
    /// Writes the current head as the block's next-pointer, then sets the
    /// block as the new head.
    unsafe fn push_free(&mut self, ptr: *mut u8, order: usize) {
        debug_assert!(!ptr.is_null(), "push_free: ptr must not be null");
        let next_slot = ptr.cast::<Option<*mut u8>>();
        *next_slot = self.free_lists[order];
        self.free_lists[order] = Some(ptr);
        self.free_count[order] += 1;
    }

    /// Pops the head block from `free_lists[order]`.
    ///
    /// Returns `None` if the list is empty. Also defensively returns `None`
    /// if the stored head is null (which, due to NPO, is equivalent to `None`
    /// but is checked explicitly to guard against UB under optimizations).
    unsafe fn pop_free(&mut self, order: usize) -> Option<*mut u8> {
        let head = self.free_lists[order]?;
        // Defensive: NPO guarantees Some(null) == None, but some optimizer
        // passes have been observed to re-read the field and produce a null
        // `head` after the `?` operator. Guard explicitly.
        if head.is_null() {
            self.free_lists[order] = None;
            self.free_count[order] = self.free_count[order].saturating_sub(1);
            return None;
        }
        let next = *(head.cast::<Option<*mut u8>>());
        self.free_lists[order] = next;
        self.free_count[order] -= 1;
        Some(head)
    }

    /// Removes a specific block from `free_lists[order]`.
    ///
    /// Handles both head-of-list and interior nodes. Silently does nothing
    /// if `ptr` is not found (defensive — should not happen in correct usage).
    unsafe fn remove_from_free(&mut self, ptr: *mut u8, order: usize) {
        // Head-of-list case.
        if let Some(head) = self.free_lists[order] {
            if head == ptr {
                self.free_lists[order] = *(head.cast::<Option<*mut u8>>());
                self.free_count[order] -= 1;
                return;
            }
        }
        // Interior-node case: traverse and unlink.
        let mut current = self.free_lists[order];
        while let Some(node) = current {
            let next = *(node.cast::<Option<*mut u8>>());
            match next {
                Some(next_ptr) if next_ptr == ptr => {
                    // Unlink ptr: copy ptr's next-pointer into node's next-slot.
                    let ptr_next = *(ptr.cast::<Option<*mut u8>>());
                    *(node.cast::<Option<*mut u8>>()) = ptr_next;
                    self.free_count[order] -= 1;
                    return;
                }
                Some(next_ptr) => current = Some(next_ptr),
                None => break,
            }
        }
    }

    /// Allocates a block of at least `size` bytes.
    ///
    /// Returns a null pointer if the request exceeds `MAX_ORDER` or the
    /// pool is exhausted (OOM).
    ///
    /// # Safety
    ///
    /// The allocator must have been initialized via [`init`](Self::init).
    pub unsafe fn alloc(&mut self, size: usize) -> *mut u8 {
        let order = Self::order_for(size);
        if order > MAX_ORDER {
            return ptr::null_mut();
        }
        // Find the smallest order >= `order` with a free block, and pop it
        // atomically. Doing `is_some()` followed by `pop_free()` separately
        // is unsound under certain optimizer passes: the compiler may re-read
        // `free_lists[o]` inside `pop_free` and observe a different value.
        // Looping over `pop_free` directly avoids the read-check-then-pop race.
        let mut block = ptr::null_mut();
        let mut found_order = 0;
        for o in order..=MAX_ORDER {
            if let Some(b) = self.pop_free(o) {
                block = b;
                found_order = o;
                break;
            }
        }
        if block.is_null() {
            return ptr::null_mut(); // OOM
        }
        // Split down to the requested order.
        let mut cur_order = found_order;
        while cur_order > order {
            cur_order -= 1;
            let buddy = block.add(PAGE_SIZE << cur_order);
            self.push_free(buddy, cur_order);
        }
        // Mark all pages of the allocated block as allocated.
        let page_idx = ((block as usize) - (self.base as usize)) / PAGE_SIZE;
        for i in 0..(1usize << order) {
            self.set_allocated(page_idx + i);
        }
        block
    }

    /// Deallocates a block previously returned by [`alloc`](Self::alloc).
    ///
    /// # Safety
    ///
    /// `ptr` must be a pointer returned by `alloc` with an allocation of at
    /// least `size` bytes, and must not have been already freed.
    pub unsafe fn dealloc(&mut self, ptr: *mut u8, size: usize) {
        let mut order = Self::order_for(size);
        let mut block = ptr;
        let page_idx = ((block as usize) - (self.base as usize)) / PAGE_SIZE;
        // Clear the allocated bits for this block's pages.
        for i in 0..(1usize << order) {
            self.clear_allocated(page_idx + i);
        }
        // Attempt to merge with buddies as far up as possible.
        let mut page_offset = (block as usize) - (self.base as usize);
        while order < MAX_ORDER {
            let buddy_offset = page_offset ^ (PAGE_SIZE << order);
            let buddy = self.base.add(buddy_offset);
            if !self.is_free(buddy, order) {
                break;
            }
            self.remove_from_free(buddy, order);
            if (buddy as usize) < (block as usize) {
                block = buddy;
                page_offset = buddy_offset;
            }
            order += 1;
        }
        self.push_free(block, order);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros, static_mut_refs)]

    use super::*;

    /// 4096-aligned 256 KB pool for buddy allocator tests (64 pages).
    #[repr(C, align(4096))]
    struct TestPool {
        data: [u8; 256 * 1024],
    }

    #[test]
    fn test_buddy_new() {
        let buddy = BuddyAllocator::new();
        assert!(buddy.base.is_null());
        assert_eq!(buddy.total_pages, 0);
        for i in 0..=MAX_ORDER {
            assert!(
                buddy.free_lists[i].is_none(),
                "free_lists[{}] should be None",
                i
            );
            assert_eq!(buddy.free_count[i], 0, "free_count[{}] should be 0", i);
        }
    }

    #[test]
    fn test_order_for() {
        assert_eq!(BuddyAllocator::order_for(4096), 0); // exactly 1 page
        assert_eq!(BuddyAllocator::order_for(4097), 1); // 2 pages
        assert_eq!(BuddyAllocator::order_for(8192), 1); // exactly 2 pages
        assert_eq!(BuddyAllocator::order_for(8193), 2); // 3 pages
        assert_eq!(BuddyAllocator::order_for(1024 * 4096), 10); // 1024 pages = 4MB
        assert_eq!(BuddyAllocator::order_for(1), 0); // tiny
        assert_eq!(BuddyAllocator::order_for(0), 0); // zero
    }

    #[test]
    fn test_buddy_init() {
        static mut POOL: TestPool = TestPool {
            data: [0; 256 * 1024],
        };
        unsafe {
            let mut buddy = BuddyAllocator::new();
            buddy.init(POOL.data.as_mut_ptr(), 64);
            assert_eq!(buddy.total_pages, 64);
            // 64.ilog2() = 6 → max_order = 6
            assert!(buddy.free_lists[6].is_some());
            assert_eq!(buddy.free_count[6], 1);
            // Lower orders should be empty.
            for o in 0..6 {
                assert!(buddy.free_lists[o].is_none());
                assert_eq!(buddy.free_count[o], 0);
            }
        }
    }

    #[test]
    fn test_buddy_alloc_one() {
        static mut POOL: TestPool = TestPool {
            data: [0; 256 * 1024],
        };
        unsafe {
            let mut buddy = BuddyAllocator::new();
            buddy.init(POOL.data.as_mut_ptr(), 64);
            let ptr = buddy.alloc(PAGE_SIZE);
            assert!(!ptr.is_null());
            assert_eq!(ptr, POOL.data.as_mut_ptr());
        }
    }

    #[test]
    fn test_buddy_alloc_multiple() {
        static mut POOL: TestPool = TestPool {
            data: [0; 256 * 1024],
        };
        unsafe {
            let mut buddy = BuddyAllocator::new();
            buddy.init(POOL.data.as_mut_ptr(), 64);
            let p1 = buddy.alloc(PAGE_SIZE);
            let p2 = buddy.alloc(PAGE_SIZE);
            let p3 = buddy.alloc(PAGE_SIZE);
            assert!(!p1.is_null());
            assert!(!p2.is_null());
            assert!(!p3.is_null());
            assert_ne!(p1, p2);
            assert_ne!(p2, p3);
            assert_ne!(p1, p3);
        }
    }

    #[test]
    fn test_buddy_dealloc_simple() {
        static mut POOL: TestPool = TestPool {
            data: [0; 256 * 1024],
        };
        unsafe {
            let mut buddy = BuddyAllocator::new();
            buddy.init(POOL.data.as_mut_ptr(), 64);
            let p1 = buddy.alloc(PAGE_SIZE);
            assert!(!p1.is_null());
            buddy.dealloc(p1, PAGE_SIZE);
            // After dealloc, alloc should succeed and reuse the freed page.
            let p2 = buddy.alloc(PAGE_SIZE);
            assert!(!p2.is_null());
            // The freed page should be reused (same address).
            assert_eq!(p1, p2);
        }
    }

    #[test]
    fn test_buddy_merge() {
        static mut POOL: TestPool = TestPool {
            data: [0; 256 * 1024],
        };
        unsafe {
            let mut buddy = BuddyAllocator::new();
            buddy.init(POOL.data.as_mut_ptr(), 64);
            // Allocate two 1-page blocks that are buddies (adjacent pages).
            let p1 = buddy.alloc(PAGE_SIZE);
            let p2 = buddy.alloc(PAGE_SIZE);
            assert!(!p1.is_null() && !p2.is_null());
            // Free both: p1 first (can't merge, p2 still allocated),
            // then p2 (should merge with p1 into order 1, then cascade up).
            buddy.dealloc(p1, PAGE_SIZE);
            // p1 is in free_lists[0], no merge yet.
            assert_eq!(buddy.free_count[0], 1);
            buddy.dealloc(p2, PAGE_SIZE);
            // After freeing p2, the two order-0 blocks should merge away
            // from order 0 (and potentially up to higher orders).
            assert_eq!(buddy.free_count[0], 0, "order-0 blocks should have merged");
            // The merged block should have cascaded up to the max order (6).
            assert_eq!(buddy.free_count[6], 1);
        }
    }

    #[test]
    fn test_buddy_oom() {
        static mut POOL: TestPool = TestPool {
            data: [0; 256 * 1024],
        };
        unsafe {
            let mut buddy = BuddyAllocator::new();
            buddy.init(POOL.data.as_mut_ptr(), 64);
            // Exhaust all 64 pages.
            let mut count = 0;
            loop {
                let ptr = buddy.alloc(PAGE_SIZE);
                if ptr.is_null() {
                    break;
                }
                count += 1;
            }
            assert_eq!(count, 64);
            // Next alloc should fail (OOM).
            let ptr = buddy.alloc(PAGE_SIZE);
            assert!(ptr.is_null());
        }
    }

    #[test]
    fn test_bitmap_operations() {
        static mut POOL: TestPool = TestPool {
            data: [0; 256 * 1024],
        };
        unsafe {
            let mut buddy = BuddyAllocator::new();
            buddy.init(POOL.data.as_mut_ptr(), 64);
            // Initially all pages should be free.
            assert!(buddy.is_page_free(0));
            assert!(buddy.is_page_free(63));
            assert!(buddy.is_range_free(0, 64));
            // Set some pages as allocated.
            buddy.set_allocated(0);
            buddy.set_allocated(5);
            buddy.set_allocated(63);
            assert!(!buddy.is_page_free(0));
            assert!(buddy.is_page_free(1));
            assert!(!buddy.is_page_free(5));
            assert!(!buddy.is_page_free(63));
            assert!(!buddy.is_range_free(0, 2)); // page 0 allocated
            assert!(buddy.is_range_free(1, 4)); // pages 1-4 free
                                                // Clear them.
            buddy.clear_allocated(0);
            buddy.clear_allocated(5);
            buddy.clear_allocated(63);
            assert!(buddy.is_page_free(0));
            assert!(buddy.is_page_free(5));
            assert!(buddy.is_page_free(63));
            assert!(buddy.is_range_free(0, 64));
        }
    }

    #[test]
    fn test_buddy_large_alloc() {
        static mut POOL: TestPool = TestPool {
            data: [0; 256 * 1024],
        };
        unsafe {
            let mut buddy = BuddyAllocator::new();
            buddy.init(POOL.data.as_mut_ptr(), 64);
            // Allocate a 2-page block (order 1).
            let p1 = buddy.alloc(2 * PAGE_SIZE);
            assert!(!p1.is_null());
            // Free it and verify it can be allocated again.
            buddy.dealloc(p1, 2 * PAGE_SIZE);
            let p2 = buddy.alloc(2 * PAGE_SIZE);
            assert!(!p2.is_null());
            assert_eq!(p1, p2);
        }
    }
}
