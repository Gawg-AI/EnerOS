//! 可回收堆分配器（22 §2.W1.4）：linked_list_allocator（vetted、no_std）。
//!
//! 替换旧 bump 分配器（`dealloc` 空实现 → 长跑服务堆占用单调增长，
//! 必然 OOM-abort）。`HeapAllocator` 为生产分配器（真实 `dealloc` 回收 +
//! 内部自旋锁，并发安全）；`BumpAllocator` 保留供 loom 并发模型验证与
//! 参考（见 `loom_allocator.rs`，`feature = "loom"`）。
//!
//! 堆大小编译期配置注入：`ENEROS_HEAP_SIZE_BYTES` 环境变量（默认 64 KiB，
//! 真机按板载 RAM 调——构建时注入，无需改代码）。

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

use linked_list_allocator::LockedHeap;

/// 堆大小（字节）：编译期注入（`ENEROS_HEAP_SIZE_BYTES`），默认 64 KiB；
/// 非法/过小值回退默认（fail-closed：不允许配置出不可用堆）。
const HEAP_SIZE: usize = match option_env!("ENEROS_HEAP_SIZE_BYTES") {
    Some(s) => match parse_heap_size(s) {
        Some(n) if n >= 4096 => n,
        _ => 64 * 1024,
    },
    None => 64 * 1024,
};

/// const 十进制解析（`str::parse` 非 const fn；仅用于编译期配置注入）。
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

/// 堆存储（对齐 16：LockedHeap 要求堆基址按指针宽度对齐——`[u8]` 数组
/// 对齐 1 会导致 target 下 alloc 全失败（QEMU 实测 OOM 8776B），G-R1 修复）。
#[repr(align(16))]
#[allow(dead_code)] // 存储介质：仅经指针交给 LockedHeap 使用
struct HeapStorage([u8; HEAP_SIZE]);

/// 生产全局分配器：free-list 可回收堆（22 §2.W1.4）。
pub struct HeapAllocator {
    heap: LockedHeap,
    storage: UnsafeCell<HeapStorage>,
}

// SAFETY: `storage` 经 UnsafeCell 提供内部可变性；LockedHeap 内部自旋锁
// 串行化所有 alloc/dealloc——并发访问安全（GlobalAlloc 的 `&self` 契约）。
unsafe impl Sync for HeapAllocator {}

impl HeapAllocator {
    /// 创建空堆（必须先 [`HeapAllocator::init`] 才能分配）。
    pub const fn new() -> Self {
        Self {
            heap: LockedHeap::empty(),
            storage: UnsafeCell::new(HeapStorage([0u8; HEAP_SIZE])),
        }
    }

    /// 初始化堆存储（bin `_start` 在首次分配前调用一次）。
    ///
    /// # Safety
    /// 调用方保证：①仅调用一次；②在首次 `alloc` 之前调用；③本分配器
    /// 生命周期覆盖全部使用（static 实例满足）。
    pub unsafe fn init(&self) {
        // SAFETY: `storage` 生命周期 = 'static（static 实例）；LockedHeap
        // 只在其生命周期内使用该区域且 init 只调用一次。
        unsafe {
            self.heap
                .lock()
                .init(self.storage.get() as *mut u8, HEAP_SIZE)
        }
    }

    /// 当前已用字节数（长跑稳定性测试/指标用）。
    pub fn used(&self) -> usize {
        self.heap.lock().used()
    }

    /// 堆容量（字节）。
    pub const fn capacity(&self) -> usize {
        HEAP_SIZE
    }
}

impl Default for HeapAllocator {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: 委托 LockedHeap（自旋锁 + free-list，GlobalAlloc 契约完整实现）。
unsafe impl GlobalAlloc for HeapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: 调用方保证 layout 合法；LockedHeap 返回 null 表示 OOM。
        unsafe { self.heap.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: 调用方保证 ptr 由本分配器 alloc 返回且 layout 匹配。
        // 真实回收（22 §2.W1.4：长跑堆占用不单调增长）。
        unsafe { self.heap.dealloc(ptr, layout) }
    }
}

/// 静态 bump 分配器（保留：loom 并发模型验证 + host 参考；生产路径已由
/// [`HeapAllocator`] 取代，见 22 §2.W1.4）。
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

#[cfg(test)]
mod heap_reclaim_tests {
    use super::*;

    /// 长跑稳定性（22 §2.W1.4 验收）：1000 轮分配/释放后 used 不单调增长
    /// （bump 分配器此测试必失败——dealloc 空实现 → used 单调增长到满）。
    #[test]
    fn test_heap_reclaims_memory_stable() {
        let alloc = HeapAllocator::new();
        // SAFETY: 测试专用——init 一次且在首次 alloc 前。
        unsafe { alloc.init() };
        let base = alloc.used();
        let layout = Layout::from_size_align(256, 8).expect("layout");
        for _ in 0..1000 {
            // SAFETY: 测试专用，配对 alloc/dealloc。
            let p = unsafe { alloc.alloc(layout) };
            assert!(!p.is_null(), "堆耗尽（不应发生：每轮都释放）");
            // SAFETY: 写回验证可写（回收后的内存复用）。
            unsafe { core::ptr::write_bytes(p, 0x5A, 256) };
            // SAFETY: 配对释放。
            unsafe { alloc.dealloc(p, layout) };
        }
        let after = alloc.used();
        assert!(
            after <= base + 1024,
            "dealloc 必须真实回收：used {after} > base {base} + 1KiB（单调增长 = bump 语义泄漏）"
        );
    }

    /// 交错大小分配/释放：占用收敛（碎片上限内），不 OOM。
    #[test]
    fn test_heap_reusable_under_mixed_sizes() {
        let alloc = HeapAllocator::new();
        // SAFETY: 测试专用——init 一次且在首次 alloc 前。
        unsafe { alloc.init() };
        for round in 0..200 {
            let mut ptrs = alloc::vec::Vec::new();
            for size in [16usize, 64, 256, 1024, 4096] {
                let layout = Layout::from_size_align(size, 8).expect("layout");
                // SAFETY: 测试专用。
                let p = unsafe { alloc.alloc(layout) };
                assert!(!p.is_null(), "round {round}: {size}B 分配失败（回收失效）");
                ptrs.push((p, layout));
            }
            for (p, layout) in ptrs {
                // SAFETY: 测试专用，配对释放。
                unsafe { alloc.dealloc(p, layout) };
            }
        }
        assert!(
            alloc.used() < alloc.capacity(),
            "used {} 必须低于容量 {}（无泄漏）",
            alloc.used(),
            alloc.capacity()
        );
    }

    /// 堆大小配置：默认 64 KiB，容量与配置一致。
    #[test]
    fn test_heap_capacity_default() {
        assert_eq!(HeapAllocator::new().capacity(), 64 * 1024);
    }

    /// const 解析：合法/非法/溢出。
    #[test]
    fn test_parse_heap_size() {
        assert_eq!(parse_heap_size("131072"), Some(131072));
        assert_eq!(parse_heap_size("0"), Some(0));
        assert_eq!(parse_heap_size("abc"), None);
        assert_eq!(parse_heap_size("12x"), None);
        assert_eq!(parse_heap_size("99999999999999999999999999"), None);
    }
}
