//! EnerOS User-space Heap Allocator — quota-aware buddy allocator.
//!
//! This crate provides a user-space heap allocator that reuses the
//! `BuddyAllocator` from `eneros-heap` (v0.10.0) and adds:
//! - **Quota**: per-partition memory limit to prevent exhaustion
//! - **OOM handler**: customizable out-of-memory policy
//!
//! The `UserHeap` type implements `core::alloc::GlobalAlloc` and is registered
//! as the global allocator via `#[global_allocator]` in non-test builds.
//!
//! # Usage
//!
//! ```ignore
//! use eneros_user_heap::{heap_init, set_quota, used};
//!
//! // Initialize with a 2MB heap pool (caller-provided page-aligned region)
//! unsafe { heap_init(pool_base as *mut u8, 2 * 1024 * 1024); }
//!
//! // Optionally set a quota smaller than the pool
//! set_quota(1024 * 1024); // 1MB quota
//!
//! // Now Vec/String/HashMap work in user space
//! let v: alloc::vec::Vec<u8> = alloc::vec![1, 2, 3];
//!
//! // Query usage
//! let used_bytes = used();
//! ```

#![cfg_attr(not(test), no_std)]

pub mod quota;

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;

use eneros_heap::buddy::{BuddyAllocator, PAGE_SIZE};
use spin::Mutex;

use crate::quota::{OomHandler, Quota};

/// Internal mutable state of the user-space heap.
///
/// All access goes through the [`USER_HEAP`] `Mutex`. Not `Copy`/`Clone`
/// because `BuddyAllocator` owns raw pointers.
pub struct UserHeapInner {
    /// Underlying buddy allocator (reused from v0.10.0).
    pub buddy: BuddyAllocator,
    /// Quota tracker for this heap partition.
    pub quota: Quota,
    /// Optional custom OOM handler (`None` = default panic).
    pub oom_handler: OomHandler,
}

// SAFETY: `UserHeapInner` contains raw pointers inside `BuddyAllocator`.
// These are only accessed under the `USER_HEAP` `Mutex`, which guarantees
// exclusive access. The raw pointers refer to a single heap region provided
// by `heap_init` and are never shared across threads without the mutex.
unsafe impl Send for UserHeapInner {}
unsafe impl Sync for UserHeapInner {}

/// Zero-sized handle type implementing [`GlobalAlloc`].
///
/// All real state lives in the static [`USER_HEAP`]; `UserHeap` is just a
/// marker for the trait implementation.
pub struct UserHeap;

/// Global heap instance.
static USER_HEAP: Mutex<Option<UserHeapInner>> = Mutex::new(None);

// SAFETY: `UserHeap` is a zero-sized type with no interior mutability of its
// own; all mutation happens through the `USER_HEAP` `Mutex`.
unsafe impl Sync for UserHeap {}

impl UserHeap {
    /// Creates a new `UserHeap` handle (const-constructible).
    pub const fn new() -> Self {
        Self
    }
}

impl Default for UserHeap {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl GlobalAlloc for UserHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut guard = USER_HEAP.lock();
        let inner = match guard.as_mut() {
            Some(inner) => inner,
            None => return ptr::null_mut(), // heap not initialised
        };

        let size = layout.size();

        // Quota check — reject if the allocation would exceed the limit.
        if !inner.quota.check(size) {
            let handler = inner.oom_handler;
            drop(guard); // release the lock before potentially panicking
            trigger_oom_handler(handler);
            return ptr::null_mut(); // unreachable if handler diverges
        }

        // Delegate to the buddy allocator.
        let ptr = inner.buddy.alloc(size);
        if ptr.is_null() {
            // Buddy pool exhausted but quota not yet full.
            let handler = inner.oom_handler;
            drop(guard);
            trigger_oom_handler(handler);
            return ptr::null_mut();
        }

        inner.quota.add_used(size);
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let mut guard = USER_HEAP.lock();
        let inner = match guard.as_mut() {
            Some(inner) => inner,
            None => return,
        };

        let size = layout.size();
        inner.buddy.dealloc(ptr, size);
        inner.quota.sub_used(size);
    }
}

/// Invokes the OOM handler, or panics with the default message if none is set.
///
/// Because [`OomHandler`] is `Option<fn() -> !>`, this function never returns
/// normally — every code path diverges.
#[allow(clippy::disallowed_macros)]
fn trigger_oom_handler(handler: OomHandler) {
    match handler {
        Some(f) => f(),
        None => panic!("user heap OOM"),
    }
}

/// Initialises the user-space heap with a page-aligned region of `size` bytes.
///
/// The default quota is set to `size` (the full pool). Use [`set_quota`] to
/// restrict it further.
///
/// # Safety
///
/// `base` must be a valid, page-aligned, writable pointer to at least `size`
/// bytes for the program's lifetime. In practice this is enforced by the
/// caller providing a properly aligned static buffer.
pub unsafe fn heap_init(base: *mut u8, size: usize) {
    let pages = size / PAGE_SIZE;
    let mut buddy = BuddyAllocator::new();
    unsafe { buddy.init(base, pages) };
    let quota = Quota::new(size);
    *USER_HEAP.lock() = Some(UserHeapInner {
        buddy,
        quota,
        oom_handler: None,
    });
}

/// Sets the quota limit (0 = unlimited).
///
/// No-op if the heap has not been initialised.
pub fn set_quota(limit: usize) {
    if let Some(inner) = USER_HEAP.lock().as_mut() {
        inner.quota.limit = limit;
    }
}

/// Returns the number of bytes currently allocated.
///
/// Returns `0` if the heap has not been initialised.
pub fn used() -> usize {
    USER_HEAP.lock().as_ref().map(|h| h.quota.used).unwrap_or(0)
}

/// Sets a custom OOM handler.
///
/// No-op if the heap has not been initialised.
pub fn set_oom_handler(handler: fn() -> !) {
    if let Some(inner) = USER_HEAP.lock().as_mut() {
        inner.oom_handler = Some(handler);
    }
}

/// Triggers the OOM handler manually.
///
/// If no handler is set, this panics with `"user heap OOM"`.
pub fn trigger_oom() {
    let handler = USER_HEAP.lock().as_ref().and_then(|h| h.oom_handler);
    trigger_oom_handler(handler);
}

// Register the global allocator only in non-test builds.
// In test builds `std` provides its own allocator so that `Vec`, `String`,
// etc. work normally in tests.
#[cfg(not(test))]
#[global_allocator]
static ALLOC: UserHeap = UserHeap::new();

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros, static_mut_refs)]

    use core::alloc::{GlobalAlloc, Layout};
    use std::panic::{self, AssertUnwindSafe};

    use super::*;

    /// 4096-aligned 256 KB pool for integration tests (64 pages).
    #[repr(C, align(4096))]
    struct TestPool {
        data: [u8; 256 * 1024],
    }

    /// Integration test covering the full user-heap lifecycle.
    ///
    /// All sub-tests run sequentially in a single function because they share
    /// the global `USER_HEAP` static.
    #[test]
    fn test_user_heap_integration() {
        static mut POOL: TestPool = TestPool {
            data: [0; 256 * 1024],
        };

        // --- 0. Uninitialised heap ---
        // Reset to None in case another test already initialised the heap.
        *USER_HEAP.lock() = None;
        let layout = Layout::from_size_align(64, 8).unwrap();
        let ptr = unsafe { GlobalAlloc::alloc(&UserHeap, layout) };
        assert!(
            ptr.is_null(),
            "alloc on uninitialised heap should return null"
        );
        assert_eq!(used(), 0, "used() should be 0 when uninitialised");

        // --- 1. heap_init ---
        unsafe {
            heap_init(POOL.data.as_mut_ptr(), 256 * 1024);
        }
        assert_eq!(used(), 0, "used() should be 0 right after heap_init");

        // --- 2. alloc / dealloc ---
        let heap = UserHeap;
        let small = Layout::from_size_align(64, 8).unwrap();
        let p1 = unsafe { GlobalAlloc::alloc(&heap, small) };
        assert!(!p1.is_null(), "small alloc should succeed");
        unsafe { GlobalAlloc::dealloc(&heap, p1, small) };

        // Large allocation via buddy (page-level).
        let large = Layout::from_size_align(4096, 8).unwrap();
        let p2 = unsafe { GlobalAlloc::alloc(&heap, large) };
        assert!(!p2.is_null(), "large alloc should succeed");
        unsafe { GlobalAlloc::dealloc(&heap, p2, large) };

        // --- 3. used tracking ---
        let used_before = used();
        let p3 = unsafe { GlobalAlloc::alloc(&heap, small) };
        assert!(!p3.is_null());
        assert!(used() > used_before, "used() should increase after alloc");
        unsafe { GlobalAlloc::dealloc(&heap, p3, small) };
        assert_eq!(
            used(),
            used_before,
            "used() should return to previous level after dealloc"
        );

        // --- 4. Quota exceeded triggers OOM ---
        // Re-init for a clean slate, then set a tight quota.
        unsafe {
            heap_init(POOL.data.as_mut_ptr(), 256 * 1024);
        }
        set_quota(8192); // 2 pages worth
        let page = Layout::from_size_align(4096, 8).unwrap();
        // First two allocs fit within quota (4096 + 4096 = 8192 <= 8192).
        let q1 = unsafe { GlobalAlloc::alloc(&heap, page) };
        assert!(!q1.is_null(), "first alloc within quota should succeed");
        let q2 = unsafe { GlobalAlloc::alloc(&heap, page) };
        assert!(!q2.is_null(), "second alloc within quota should succeed");
        // Third alloc would push used to 12288 > 8192 → OOM.
        // The default OOM handler panics, so catch it.
        let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
            GlobalAlloc::alloc(&heap, page)
        }));
        assert!(
            result.is_err(),
            "alloc beyond quota should trigger OOM panic"
        );
        // Clean up the two successful allocations.
        unsafe {
            GlobalAlloc::dealloc(&heap, q1, page);
            GlobalAlloc::dealloc(&heap, q2, page);
        }

        // --- 5. Buddy exhaustion triggers OOM ---
        // Re-init: quota defaults to pool size (256 KB), so only buddy
        // exhaustion can trigger OOM. With 64 pages, the first 64 single-page
        // allocations succeed; the 65th exhausts the buddy pool and the OOM
        // handler fires (default = panic).
        unsafe {
            heap_init(POOL.data.as_mut_ptr(), 256 * 1024);
        }
        for _ in 0..64 {
            let p = unsafe { GlobalAlloc::alloc(&heap, page) };
            assert!(!p.is_null(), "should allocate all 64 pages");
        }
        let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
            GlobalAlloc::alloc(&heap, page)
        }));
        assert!(
            result.is_err(),
            "alloc after buddy exhaustion should trigger OOM panic"
        );

        // --- 6. Custom OOM handler ---
        fn custom_handler() -> ! {
            panic!("custom OOM handler invoked");
        }
        unsafe {
            heap_init(POOL.data.as_mut_ptr(), 256 * 1024);
        }
        set_quota(4096);
        set_oom_handler(custom_handler);
        let p = unsafe { GlobalAlloc::alloc(&heap, page) };
        assert!(!p.is_null(), "first alloc should succeed");
        // Second alloc exceeds quota → custom handler should panic.
        let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
            GlobalAlloc::alloc(&heap, page)
        }));
        assert!(result.is_err(), "custom OOM handler should be invoked");
        // Verify the panic message matches the custom handler.
        let msg = result.as_ref().err().and_then(|e| {
            e.downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| e.downcast_ref::<&str>().copied())
        });
        assert_eq!(
            msg,
            Some("custom OOM handler invoked"),
            "panic should come from custom handler"
        );
    }

    /// Standalone test for [`trigger_oom`] with the default handler.
    #[test]
    fn test_trigger_oom_default() {
        // Ensure the heap is initialised so trigger_oom reads a valid state.
        static mut POOL: TestPool = TestPool {
            data: [0; 256 * 1024],
        };
        unsafe {
            heap_init(POOL.data.as_mut_ptr(), 256 * 1024);
        }
        // Reset OOM handler to None (in case previous test set one).
        if let Some(inner) = USER_HEAP.lock().as_mut() {
            inner.oom_handler = None;
        }
        let result = panic::catch_unwind(trigger_oom);
        assert!(result.is_err(), "trigger_oom with no handler should panic");
    }
}
