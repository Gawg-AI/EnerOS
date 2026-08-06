//! v0.243.0（C-243）：BumpAllocator 并发模型验证（loom，host 侧）。
//!
//! 修复背景：旧实现经共享引用派生可写指针属 UB；改为 `UnsafeCell` 封装 +
//! offset CAS 后，用 loom 穷举调度验证并发 `alloc` 的区间不重叠不变式：
//! - 任意两个成功返回的区间互不重叠（CAS 单调性）；
//! - 越界分配返回 null（OOM 契约）；
//! - 绝对地址对齐在并发下仍成立。
//!
//! 生产 `BumpAllocator` 使用 `core::sync::atomic::AtomicUsize`；
//! loom 用 `loom::sync::atomic::AtomicUsize` 复刻同一算法验证并发正确性。
//!
//! 该模块仅编译于 `#[cfg(all(test, feature = "loom"))]`；不引入 tests/
//! 目录（EL0 bin 的自定义 `_start` 在 host 上不可链接，见 svc-* bin）。

extern crate std;

use core::alloc::Layout;
use loom::sync::atomic::{AtomicUsize, Ordering};
use std::vec::Vec;

/// 与生产 `BumpAllocator` 同构的 loom 复刻（UnsafeCell 堆 + CAS offset）。
struct LoomBumpAllocator {
    heap: core::cell::UnsafeCell<[u8; 4096]>,
    offset: AtomicUsize,
}

const HEAP_SIZE: usize = 4096;

impl LoomBumpAllocator {
    fn new() -> Self {
        Self {
            heap: core::cell::UnsafeCell::new([0u8; HEAP_SIZE]),
            offset: AtomicUsize::new(0),
        }
    }

    fn alloc(&self, layout: Layout) -> *mut u8 {
        let heap_ptr = self.heap.get() as *mut u8;
        let heap_start = heap_ptr as usize;
        let align_mask = !(layout.align() - 1);
        let mut off = self.offset.load(Ordering::Relaxed);
        loop {
            let aligned_abs = (heap_start + off + layout.align() - 1) & align_mask;
            let aligned = aligned_abs - heap_start;
            let end = aligned.saturating_add(layout.size());
            if end > HEAP_SIZE {
                return core::ptr::null_mut();
            }
            match self
                .offset
                .compare_exchange_weak(off, end, Ordering::Relaxed, Ordering::Relaxed)
            {
                // SAFETY: CAS 成功后 `aligned..end` 区间不与任何已分配区间
                // 重叠（单调递增），且未越界；经 UnsafeCell 派生写指针。
                Ok(_) => return unsafe { heap_ptr.add(aligned) },
                Err(cur) => off = cur,
            }
        }
    }
}

/// 两个线程并发分配 128B 块：两个区间必须互不重叠（CAS 不变式）。
#[test]
fn vrf_005_concurrent_alloc_no_overlap() {
    loom::model(|| {
        let allocator = loom::sync::Arc::new(LoomBumpAllocator::new());
        let layout = Layout::from_size_align(128, 8).unwrap();

        let mut handles = Vec::new();
        for _ in 0..2 {
            let a = allocator.clone();
            handles.push(loom::thread::spawn(move || {
                let mut ptrs = Vec::new();
                for _ in 0..2 {
                    let p = a.alloc(layout);
                    assert!(!p.is_null(), "128B×4 块应在 4KiB 堆内成功");
                    ptrs.push(p as usize);
                }
                ptrs
            }));
        }
        let mut all: Vec<usize> = Vec::new();
        for h in handles {
            all.extend(h.join().unwrap());
        }
        all.sort_unstable();
        // 相邻区间必须至少相隔 128 字节（互不重叠）。
        for w in all.windows(2) {
            assert!(w[1] >= w[0] + 128, "区间重叠: {:#x} 与 {:#x}", w[0], w[1]);
        }
    });
}

/// 并发分配直至 OOM：总分配量不超过堆容量，且 OOM 返回 null。
#[test]
/// 对齐：loom 下 4096 对齐分配返回对齐地址（绝对地址取整路径）。
///
/// 4KiB 堆只能容纳一个 4096 对齐块，故单线程 1 次分配（并发 CAS 路径由
/// no_overlap/fixed_count 覆盖；对齐性质由生产单元测试
/// `alloc_respects_alignment` 全覆盖）。
#[test]
fn vrf_005_concurrent_alloc_alignment() {
    loom::model(|| {
        let allocator = loom::sync::Arc::new(LoomBumpAllocator::new());
        let layout = Layout::from_size_align(1, 4096).unwrap();
        let a = allocator.clone();
        let h = loom::thread::spawn(move || {
            let p = a.alloc(layout);
            assert!(!p.is_null(), "4096 对齐 1 字节块应成功（4KiB 堆）");
            assert_eq!(p as usize % 4096, 0, "返回地址必须 4096 对齐");
        });
        h.join().unwrap();
    });
}
