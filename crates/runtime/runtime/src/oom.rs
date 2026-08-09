//! OOM 记录与 fail-safe 语义（22 §2.W1.1：`#[alloc_error_handler]`）。
//!
//! 目标：堆耗尽**不 abort**。`alloc_error_handler` 由各二进制（root-task +
//! 4 个 EL0 服务 bin）定义为 `#[alloc_error_handler] fn oom(layout) -> !`，
//! 体内调用 [`oom_fail_safe`]：①记录指标（OOM 计数 + 布局信息）→ ②串口
//! 打印（无分配：`println!` 走 `core::fmt` + MMIO，见 console.rs）→ ③挂起
//! （fail-closed，不返回）。
//!
//! 挂起而非 abort 的意义：进程环境保留、串口可观测、不破坏 seL4 隔离；
//! 后续 W1.2/W1.6 接入看门狗后，挂起由 watchdog 超时兜底重启（分级重启
//! 链条的最后一环）。`alloc_error_handler` 契约要求不返回，故挂起是
//! 语义上唯一正确的 fail-safe 动作。
//!
//! 指标通道（W1.1 阶段）：静态原子 + 串口打印，零依赖可观测。W4.1 的
//! metrics 导出接入时从 [`OOM_COUNT`]/[`OOM_LAST_LAYOUT`] 桥接。

use core::alloc::Layout;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// OOM 触发总次数（指标：`heap.oom.count`）。
pub static OOM_COUNT: AtomicUsize = AtomicUsize::new(0);

/// 最近一次 OOM 布局信息（指标：`heap.oom.last_layout`）。
///
/// 打包格式：`layout.size() << 32 | layout.align()`（size 上限 2^32-1，
/// 覆盖堆配置注入上限；align 上限 2^32-1，覆盖合法 `Layout` align 上限
/// 2^29——两字段无重叠）。
static OOM_LAST_LAYOUT: AtomicU64 = AtomicU64::new(0);

/// [`record_oom`] 的记录结果（host 测试可断言）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OomRecord {
    /// 触发后总计数（1 起）。
    pub count: usize,
    /// 打包布局（同 [`OOM_LAST_LAYOUT`] 格式）。
    pub layout: u64,
}

/// 记录一次 OOM：递增计数 + 保存布局（size/align 打包）。
///
/// 无分配、无锁（Relaxed 原子足够：指标只增不减，竞态仅影响打印顺序）。
pub fn record_oom(layout: Layout) -> OomRecord {
    let packed = pack_layout(layout);
    OOM_LAST_LAYOUT.store(packed, Ordering::Relaxed);
    let count = OOM_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    OomRecord {
        count,
        layout: packed,
    }
}

/// 布局打包：`size << 32 | align`。
pub const fn pack_layout(layout: Layout) -> u64 {
    ((layout.size() as u64) << 32) | (layout.align() as u64)
}

/// 解包 size（与 [`pack_layout`] 逆操作；测试与打印用）。
pub const fn unpack_size(packed: u64) -> usize {
    (packed >> 32) as usize
}

/// 解包 align（与 [`pack_layout`] 逆操作；测试与打印用）。
pub const fn unpack_align(packed: u64) -> usize {
    (packed & 0xFFFF_FFFF) as usize
}

/// OOM fail-safe：记录指标 → 串口打印 → 挂起（不返回）。
///
/// 由各二进制的 `#[alloc_error_handler]` 调用。禁止在此分配或做复杂
/// 逻辑（堆已耗尽；`println!` 无分配安全）。host 侧仅测试
/// [`record_oom`]，本函数 target 侧专用（调用即挂起）。
pub fn oom_fail_safe(layout: Layout) -> ! {
    let r = record_oom(layout);
    crate::println!(
        "[oom] OOM #{} layout=size:{},align:{} — 挂起（fail-closed，watchdog 兜底）",
        r.count,
        unpack_size(r.layout),
        unpack_align(r.layout)
    );
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(test)]
mod tests {
    use core::alloc::Layout;

    use super::*;

    #[test]
    fn test_pack_layout_roundtrip() {
        for (size, align) in [(1usize, 1usize), (64, 8), (4096, 4096), (65536, 64)] {
            let layout = Layout::from_size_align(size, align).expect("layout");
            let packed = pack_layout(layout);
            assert_eq!(unpack_size(packed), size, "size 还原失败: {packed:#x}");
            assert_eq!(unpack_align(packed), align, "align 还原失败: {packed:#x}");
        }
    }

    #[test]
    fn test_record_oom_counts_and_layout() {
        // 计数与布局随每次记录推进（跨测试共享原子，断言相对值）。
        let layout = Layout::from_size_align(4096, 16).expect("layout");
        let r1 = record_oom(layout);
        let r2 = record_oom(layout);
        assert_eq!(r2.count, r1.count + 1, "计数必须递增");
        assert_eq!(r2.layout, pack_layout(layout), "布局必须记录");
        // 不同布局被覆盖。
        let layout2 = Layout::from_size_align(32, 4).expect("layout");
        let r3 = record_oom(layout2);
        assert_eq!(r3.layout, pack_layout(layout2), "布局应更新为最近一次");
    }

    /// 布局打包不重叠：size 与 align 字段可独立解出（16-bit align 上限内）。
    #[test]
    fn test_pack_layout_fields_do_not_overlap() {
        let layout = Layout::from_size_align(0x1_0000, 0x1_0000).expect("layout");
        let packed = pack_layout(layout);
        assert_eq!(unpack_size(packed), 0x1_0000);
        assert_eq!(unpack_align(packed), 0x1_0000);
    }
}
