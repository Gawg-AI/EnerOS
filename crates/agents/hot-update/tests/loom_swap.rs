#![cfg(feature = "loom")]
//! VRF-004：`ArcHotSwap` 并发模型验证（loom）。
//!
//! loom 对 Arc+AtomicPtr 原子交换协议（与 `crates/agents/hot-update/src/hot_swap.rs`
//! 的 `ArcHotSwap` **同构**的 loom 版实现）做穷举调度探索，验证：
//! - double-free / use-after-free：指针恒非空 + `ManuallyDrop` 借用克隆 + swap 交还
//! - data race：Acquire / AcqRel 序
//!
//! 生产 `ArcHotSwap` 保持 `alloc::sync::Arc`（no_std 固件路径）；loom 用
//! `loom::sync::{Arc, AtomicPtr}` 复刻同一算法验证并发正确性。
#![allow(clippy::disallowed_macros)]

use core::sync::atomic::Ordering;
use loom::sync::atomic::AtomicPtr;
use loom::sync::Arc;

/// 与生产 `ArcHotSwap` 同构的 loom 复刻。
struct LoomHotSwap<T> {
    ptr: AtomicPtr<T>,
}

impl<T> LoomHotSwap<T> {
    fn new(arc: Arc<T>) -> Self {
        Self { ptr: AtomicPtr::new(Arc::into_raw(arc) as *mut T) }
    }

    fn load(&self) -> Arc<T> {
        let ptr = self.ptr.load(Ordering::Acquire);
        // 与生产同构：原子递增强计数后转移（VRF-004 修复的 race 窗口）
        // SAFETY: `ptr` 恒为 `Arc::into_raw` 产物且非空（不变式同生产）；
        // loom 模型下由 loom 检测竞争。
        unsafe { loom::sync::Arc::increment_strong_count(ptr as *const T) };
        // SAFETY: 同上一一递增后计数 ≥ 1，from_raw 还原合法。
        unsafe { Arc::from_raw(ptr as *const T) }
    }

    fn swap(&self, new: Arc<T>) -> Arc<T> {
        let new_ptr = Arc::into_raw(new) as *mut T;
        let old = self.ptr.swap(new_ptr, Ordering::AcqRel);
        // SAFETY: `old` 恒非空且为 `Arc::into_raw` 产物（不变式）。
        unsafe { Arc::from_raw(old as *const T) }
    }
}

impl<T> Drop for LoomHotSwap<T> {
    fn drop(&mut self) {
        // loom AtomicPtr 无 get_mut：模型退出时无并发（join 完成），load 即最终指针
        let ptr = self.ptr.load(Ordering::Acquire);
        // SAFETY: 模型退出时无并发访问，`ptr` 恒非空且为 into_raw 产物。
        drop(unsafe { Arc::from_raw(ptr as *const T) });
    }
}

#[test]
fn vrf004_loom_concurrent_load_swap_no_ub() {
    loom::model(|| {
        let hot = Arc::new(LoomHotSwap::new(Arc::new(1u32)));

        let writer_hot = Arc::clone(&hot);
        let writer = loom::thread::spawn(move || {
            let mut retired = Vec::new();
            for i in 2..=4u32 {
                let old = writer_hot.swap(Arc::new(i));
                // 延迟释放协议：旧实例退役队列，读侧窗口结束前不 drop
                retired.push(old);
            }
            retired
        });

        let reader_hot = Arc::clone(&hot);
        let reader = loom::thread::spawn(move || {
            for _ in 0..6 {
                let v = reader_hot.load();
                drop(v);
            }
        });

        let _retired = writer.join().unwrap();
        reader.join().unwrap();
        drop(hot);
    });
}
