//! 可回收堆分配器（38 WP-38.9）：linked_list_allocator（vetted、no_std）。
//!
//! drv-svc 服务 ELF 需要全局分配器（println/format 等 alloc 路径）；
//! runtime 本体保持无 alloc——堆仅服务进程级需求。

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

use linked_list_allocator::LockedHeap;

/// 堆大小（字节）：编译期注入（`ENEROS_HEAP_SIZE_BYTES`），默认 64 KiB。
const HEAP_SIZE: usize = match option_env!("ENEROS_HEAP_SIZE_BYTES") {
    Some(s) => match parse_heap_size(s) {
        Some(n) if n >= 4096 => n,
        _ => 64 * 1024,
    },
    None => 64 * 1024,
};

const fn parse_heap_size(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut v: usize = 0;
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit() {
            return None;
        }
        v = match v.checked_mul(10) {
            Some(x) => x,
            None => return None,
        };
        v = match v.checked_add((b[i] - b'0') as usize) {
            Some(x) => x,
            None => return None,
        };
        i += 1;
    }
    Some(v)
}

/// 静态堆区（BSS；分配器持锁访问）。`UnsafeCell` 包装——消除
/// `static mut`（38.5 收口精神；`_start` 单线程首调 init，此后经
/// `LockedHeap` 内部自旋锁并发安全）。
struct HeapSpace(UnsafeCell<[u8; HEAP_SIZE]>);

// SAFETY: `_start` 单线程首调 init（CAS 防重入）；此后 LockedHeap 内部
// 自旋锁串行化访问——与 HeapAllocator 的 Sync 证明同源。
unsafe impl Sync for HeapSpace {}

static HEAP_SPACE: HeapSpace = HeapSpace(UnsafeCell::new([0u8; HEAP_SIZE]));

/// 全局分配器。
pub struct HeapAllocator {
    heap: UnsafeCell<LockedHeap>,
    init: AtomicUsize,
}

impl HeapAllocator {
    pub const fn new() -> Self {
        Self {
            heap: UnsafeCell::new(LockedHeap::empty()),
            init: AtomicUsize::new(0),
        }
    }

    /// 初始化（`_start` 早期、首次分配前调用一次）。
    ///
    /// # Safety
    /// 单线程启动阶段调用（服务 spawn 后单线程）；此后 `LockedHeap` 内部
    /// 自旋锁保证并发安全。
    pub unsafe fn init(&self) {
        if self
            .init
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            // SAFETY: 静态 BSS 区；`_start` 单线程首调。UnsafeCell::get
            // 对不可变 static 合法（无 static_mut_refs 面）。
            // LockedHeap 仅 Deref（无 DerefMut）——经 Spinlock guard 访问
            // Heap::init（`Heap::init` 为不安全方法，guard 不自动解锁）。
            unsafe {
                (*self.heap.get())
                    .lock()
                    .init(HEAP_SPACE.0.get() as *mut u8, HEAP_SIZE);
            }
        }
    }
}

// SAFETY: LockedHeap 内部自旋锁 + UnsafeCell 单初始化（init CAS 防重入）。
unsafe impl Sync for HeapAllocator {}

impl Default for HeapAllocator {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl GlobalAlloc for HeapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: 初始化已完成（_start 首调 init）；LockedHeap 实现
        // GlobalAlloc（内部自旋锁）。
        unsafe { core::alloc::GlobalAlloc::alloc(&*self.heap.get(), layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: 同 alloc。
        unsafe { core::alloc::GlobalAlloc::dealloc(&*self.heap.get(), ptr, layout) }
    }
}
