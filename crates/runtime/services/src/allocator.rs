//! 静态 bump 分配器（VRF-003：EL0 服务 bin 在 host clippy/测试编译下
//! 需要 `#[global_allocator]`；no_std 固件路径用本分配器——静态 64KB 堆 +
//! 单调 bump，永不释放（占位服务语义，svc-hog 除外——它专门测 OOM 隔离）。

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

/// 静态堆大小（64 KiB）。
const HEAP_SIZE: usize = 64 * 1024;

/// 单调 bump 分配器（no_std 静态）。
///
/// # 并发安全（v0.243.0 修复 UB）
/// 旧实现经 `self.heap.as_ptr()` 从共享引用派生可写指针，违反共享引用
/// 别名规则（真 UB）。现以 [`UnsafeCell`] 封装堆存储，写指针统一经
/// `self.heap.get()` 派生；区间互不重叠由 `offset` CAS 保证——`alloc`
/// 只在 `compare_exchange_weak` 成功后返回该区间，且偏移单调递增，
/// 故任意两个成功分配返回的区间互不重叠，并发写不同区间合法。
pub struct BumpAllocator {
    heap: UnsafeCell<[u8; HEAP_SIZE]>,
    offset: AtomicUsize,
}

// SAFETY: `heap` 经 UnsafeCell 提供内部可变性；`offset` 为原子，CAS
// 保证每个成功分配获得的区间互不重叠且仅被一个调用者写，因此并发
// `alloc` 不会造成数据竞争（GlobalAlloc 的 `&self` 方法可安全共享）。
unsafe impl Sync for BumpAllocator {}

impl BumpAllocator {
    /// 创建空分配器。
    pub const fn new() -> Self {
        Self {
            heap: UnsafeCell::new([0u8; HEAP_SIZE]),
            offset: AtomicUsize::new(0),
        }
    }
}

impl Default for BumpAllocator {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: 见结构体文档「并发安全」——UnsafeCell 封装堆存储，offset CAS
// 保证区间互不重叠，`&self` 方法可安全并发调用（GlobalAlloc 契约）。
unsafe impl GlobalAlloc for BumpAllocator {
    /// # Safety
    /// 遵循 GlobalAlloc 契约：返回指针满足 `layout` 对齐与大小，失败返回
    /// null；调用方保证 layout 合法。
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // heap 基址自身对齐到 1 字节，故按**绝对地址**对齐取整
        // （相对偏移取整在 heap 基址未对齐到 `layout.align()` 时不成立）。
        let heap_ptr = self.heap.get() as *mut u8;
        let heap_start = heap_ptr as usize;
        let align_mask = !(layout.align() - 1);
        let mut off = self.offset.load(Ordering::Relaxed);
        loop {
            // 绝对地址对齐：`heap_start + off` 对齐到 `layout.align()`。
            let aligned_abs = (heap_start + off + layout.align() - 1) & align_mask;
            let aligned = aligned_abs - heap_start; // 相对偏移（单调不减）
            let end = aligned.saturating_add(layout.size());
            if end > HEAP_SIZE {
                return core::ptr::null_mut(); // OOM → 返回 null（GlobalAlloc 契约）
            }
            match self
                .offset
                .compare_exchange_weak(off, end, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => {
                    // SAFETY: CAS 成功后 `aligned..end` 区间不与任何已分配
                    // 区间重叠，且未越界（`end <= HEAP_SIZE`）；经 UnsafeCell
                    // 派生写指针，满足共享引用的别名规则。返回指针为绝对
                    // 地址 `aligned_abs`，按 `layout.align()` 对齐。
                    return unsafe { heap_ptr.add(aligned) };
                }
                Err(cur) => off = cur,
            }
        }
    }

    /// # Safety
    /// 遵循 GlobalAlloc 契约：`ptr` 必须由本分配器 `alloc` 返回且未被释放。
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // bump 分配器不释放（占位服务语义）
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 单线程下分配区间互不重叠且对齐正确。
    #[test]
    fn alloc_intervals_do_not_overlap() {
        let allocator = BumpAllocator::new();
        let layout = Layout::from_size_align(128, 8).unwrap();
        // SAFETY: 测试专用，分配器生命周期覆盖指针使用。
        let p1 = unsafe { allocator.alloc(layout) };
        let p2 = unsafe { allocator.alloc(layout) };
        assert!(!p1.is_null() && !p2.is_null());
        let a = p1 as usize;
        let b = p2 as usize;
        assert!(a % 8 == 0 && b % 8 == 0);
        assert!(a + 128 <= b, "区间必须单调递增且不重叠");
        // SAFETY: 写回分配区间验证可写（bump 语义：永不释放）。
        unsafe {
            core::ptr::write_bytes(p1, 0xAA, 128);
            core::ptr::write_bytes(p2, 0xBB, 128);
            assert_eq!(core::ptr::read(p1), 0xAA);
            assert_eq!(core::ptr::read(p2), 0xBB);
        }
    }

    /// 对齐边界：分配 3 字节对齐 4096 的块，返回地址必须 4096 对齐。
    #[test]
    fn alloc_respects_alignment() {
        let allocator = BumpAllocator::new();
        let layout = Layout::from_size_align(3, 4096).unwrap();
        // SAFETY: 测试专用。
        let p = unsafe { allocator.alloc(layout) };
        assert!(!p.is_null());
        assert_eq!(p as usize % 4096, 0);
    }

    /// OOM：分配超出堆容量的布局必须返回 null 而不是越界。
    #[test]
    fn alloc_oom_returns_null() {
        let allocator = BumpAllocator::new();
        let layout = Layout::from_size_align(HEAP_SIZE + 1, 1).unwrap();
        // SAFETY: 测试专用。
        let p = unsafe { allocator.alloc(layout) };
        assert!(p.is_null());
    }

    /// 顺序分配直到 OOM：总分配量不超过堆容量。
    #[test]
    fn alloc_exhausts_heap_cleanly() {
        let allocator = BumpAllocator::new();
        let layout = Layout::from_size_align(64, 1).unwrap();
        let mut count = 0usize;
        // SAFETY: 测试专用。
        while !unsafe { allocator.alloc(layout) }.is_null() {
            count += 1;
            assert!(count * 64 <= HEAP_SIZE, "分配总量越界");
        }
        assert_eq!(count, HEAP_SIZE / 64);
    }
}
