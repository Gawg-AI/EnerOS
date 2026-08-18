//! 可回收堆分配器（22 §2.W1.4）：linked_list_allocator（vetted、no_std）。
//!
//! 替换旧 bump 分配器（`dealloc` 空实现 → root-task 长跑堆占用单调增长）。
//! `HeapAllocator` 为生产分配器（真实 `dealloc` 回收 + 内部自旋锁）。
//! `BumpAllocator` 保留为参考/测试（与 services 的 loom 验证同源）。
//!
//! 堆大小编译期配置注入：`ENEROS_HEAP_SIZE_BYTES`（默认 2 MiB——init
//! bundle 原始缓冲 + 逐模块 blob 拷贝 + spawn 计划 + ET_DYN staging
//! （WP-37.4：每 PIE 服务 ~316 KiB 重定位镜像，双实例 ~624 KiB）——
//! QEMU 实测 1 MiB 在双 PIE 时 OOM（fail-closed 正确触发，37 记录 §15）；
//! 真机按板载 RAM 注入调整）。

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

use linked_list_allocator::LockedHeap;

/// 堆大小（字节）：编译期注入（`ENEROS_HEAP_SIZE_BYTES`），默认 5 MiB
/// （39 收尾复验由 2 MiB 上调：EL0 服务增至 19 个，bundle ~1.3 MiB +
/// DTB ~1 MiB 使 2 MiB 堆 OOM（used=2056360 时 42KB 申请失败）；
/// 蓝图 40 上调至 4 MiB：控制闭环 5 服务入 bundle（25 服务，init_bytes
/// 1.69 MiB），OOM-DIAG used=3130304 时 132KB 申请失败。
/// 蓝图 41 上调至 5 MiB：业务/运维 8 服务入 bundle（34 服务，init_bytes
/// 2.23 MiB），OOM-DIAG used=4111472 时 89KB 申请失败（svc-config ELF
/// staging）——静态审计后 5 MiB 余量 ~1.1 MiB；kernel 早期崩溃点
/// 4.3 MB 阈值以 QEMU 复验为准（40 已以 4 MiB 实跑通过，5 MiB 复验）。
///
/// 历史：G-R1 QEMU 实测 512 KiB bump 时代刚好够（无 header 开销）；
/// LockedHeap 的块头/对齐开销使 spawn 后剩余不足（OOM 8776B——
/// supervisor 表分配失败），G-R1 提到 1 MiB；WP-37.4 双 PIE staging
/// （bundle 466 KiB + 模块拷贝 + 2×316 KiB 重定位镜像）超出 1 MiB，
/// 37 实测 OOM（OOM-DIAG used=831296 + 319488 申请），提到 4 MiB；第八轮 QEMU 实测 rootserver 4.3 MB 触发 kernel 早期崩溃（SHA-256 处 user exception），降回 2 MiB（OOM 峰值 ~1.15 MiB + 余量足够）。
const HEAP_SIZE: usize = match option_env!("ENEROS_HEAP_SIZE_BYTES") {
    Some(s) => match parse_heap_size(s) {
        Some(n) if n >= 4096 => n,
        _ => 5 * 1024 * 1024,
    },
    None => 5 * 1024 * 1024,
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

    /// 初始化堆存储（`rust_entry` 在首次分配前调用一次）。
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

    /// 当前已用字节数（指标/稳定性测试用）。
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
        // 真实回收（22 §2.W1.4）。
        unsafe { self.heap.dealloc(ptr, layout) }
    }
}

/// 静态 bump 分配器（保留：参考实现与并发模型验证；生产路径已由
/// [`HeapAllocator`] 取代，见 22 §2.W1.4）。
/// 静态堆大小（4 MiB，同默认配置——WP-37.4 双 PIE staging 需求）。
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
        // 4 MiB 内联数组必须 Box（栈上实例化会溢出测试线程栈）。
        let allocator = Box::new(BumpAllocator::new());
        let layout = Layout::from_size_align(16, 8).unwrap();
        // SAFETY: test-only allocation from a fresh BumpAllocator; layout is valid and heap is large enough.
        let ptr = unsafe { allocator.alloc(layout) };
        assert!(!ptr.is_null());
        // SAFETY: second allocation from the same fresh BumpAllocator; layout is valid and heap has remaining space.
        let ptr2 = unsafe { allocator.alloc(layout) };
        assert!(!ptr2.is_null());
        assert_ne!(ptr as usize, ptr2 as usize);
    }

    #[test]
    fn bump_oom_returns_null() {
        let allocator = Box::new(BumpAllocator::new());
        let layout = Layout::from_size_align(HEAP_SIZE + 1, 1).unwrap();
        // SAFETY: test-only allocation with an oversized layout; BumpAllocator returns null without touching memory.
        let ptr = unsafe { allocator.alloc(layout) };
        assert!(ptr.is_null());
    }
}
