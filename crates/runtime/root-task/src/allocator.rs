//! 静态 bump 分配器（root-task 专用）。
//!
//! root-task 作为 rootserver 运行于 aarch64-unknown-none（no_std），
//! spawn/IPC 路径使用 `alloc`（Vec/String 等），必须提供
//! `#[global_allocator]`。与 services 的 BumpAllocator 同模式：
//! 静态堆 + 单调 bump（root-task 启动后持续运行，分配只增不减，
//! 单调 bump 语义正确且实现最简——root-task 不做长生命周期对象回收）。

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

/// 静态堆大小：必须容纳 init bundle 全量（~133 KiB）+ spawn 计划 +
/// IPC 帧缓冲。BOOT-007 修复（C2：内核映射 bundle）后 root-task 真正
/// 读取 bundle 内容，64 KiB 堆会被 133 KiB bundle 分配打爆（PANIC）。
/// 512 KiB 留足余量（bump 分配器只增不减，root-task 常驻）。
const HEAP_SIZE: usize = 512 * 1024;

/// 单调 bump 分配器（no_std 静态）。
///
/// # 并发安全
/// 与 services BumpAllocator 相同：`heap` 经 `UnsafeCell` 提供内部可变性；
/// `offset` 为原子，CAS 保证每个成功分配获得的区间互不重叠且仅被一个
/// 调用者写，因此并发 `alloc` 不会造成数据竞争（GlobalAlloc 的 `&self`
/// 方法可安全共享）。
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
    /// bump 分配器不回收；`ptr` 必须是本分配器此前 `alloc` 返回的指针。
    /// 本实现为 no-op（满足 GlobalAlloc 契约即可；root-task 生命周期内
    /// 分配只增不减，无需释放）。
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bump_allocates_within_heap() {
        let allocator = BumpAllocator::new();
        let layout = Layout::from_size_align(16, 8).unwrap();
        let ptr = unsafe { allocator.alloc(layout) };
        assert!(!ptr.is_null());
        let ptr2 = unsafe { allocator.alloc(layout) };
        assert!(!ptr2.is_null());
        assert_ne!(ptr as usize, ptr2 as usize);
    }

    #[test]
    fn bump_oom_returns_null() {
        let allocator = BumpAllocator::new();
        let layout = Layout::from_size_align(HEAP_SIZE + 1, 1).unwrap();
        let ptr = unsafe { allocator.alloc(layout) };
        assert!(ptr.is_null());
    }
}
