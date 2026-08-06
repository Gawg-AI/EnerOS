//! 静态 bump 分配器（VRF-003：EL0 服务 bin 在 host clippy/测试编译下
//! 需要 `#[global_allocator]`；no_std 固件路径用本分配器——静态 64KB 堆 +
//! 单调 bump，永不释放（占位服务语义，svc-hog 除外——它专门测 OOM 隔离）。

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

/// 静态堆大小（64 KiB）。
const HEAP_SIZE: usize = 64 * 1024;

/// 单调 bump 分配器（no_std 静态）。
pub struct BumpAllocator {
    heap: [u8; HEAP_SIZE],
    offset: AtomicUsize,
}

impl BumpAllocator {
    /// 创建空分配器。
    pub const fn new() -> Self {
        Self {
            heap: [0u8; HEAP_SIZE],
            offset: AtomicUsize::new(0),
        }
    }
}

impl Default for BumpAllocator {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut off = self.offset.load(Ordering::Relaxed);
        loop {
            let aligned = (off + layout.align() - 1) & !(layout.align() - 1);
            let end = aligned.saturating_add(layout.size());
            if end > HEAP_SIZE {
                return core::ptr::null_mut(); // OOM → 返回 null（GlobalAlloc 契约）
            }
            match self.offset.compare_exchange_weak(
                off,
                end,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return (self.heap.as_ptr() as *mut u8).add(aligned),
                Err(cur) => off = cur,
            }
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // bump 分配器不释放（占位服务语义）
    }
}
