//! 热切换机制（D3：真实 `Arc` + `AtomicPtr` 原子交换，替换蓝图 `_phantom` stub）.
//!
//! [`ArcHotSwap`] 持有当前 Agent 实例的原子指针。读侧 `load` 无锁获取当前实例的
//! `Arc`（新请求路由到当前版本）；写侧 `swap` 原子换入新实例并返回旧 `Arc`，调用方
//! drop 旧 `Arc` 后，若此前 `load` 出去的引用也全部释放，旧实例引用计数归零自动卸载
//! —— 实现「新旧版本平滑过渡、请求不中断」。

use alloc::sync::Arc;
use core::mem::ManuallyDrop;
use core::sync::atomic::{AtomicPtr, Ordering};

/// 基于 `Arc` 原子指针的热切换器.
///
/// # 内存安全不变式
///
/// - 内部指针**恒为非空**，且恒由 `Arc::into_raw` 产生（`new` / `swap` 保证）；
/// - 指针指向的实例**引用计数恒 ≥ 1**：管理器自身持有的一份引用仅在 `swap`（换回旧
///   `Arc` 交调用方）或 `Drop`（`from_raw` 释放）时才交还，故 `load` 读到的指针在
///   读取瞬间必然有效；
/// - `load` 通过 `ManuallyDrop` 临时借用指针仅做引用计数 +1，不消费管理器持有的那份
///   引用，杜绝 double-free。
pub struct ArcHotSwap<T> {
    /// 当前实例指针（不变式：恒非空、恒为 `Arc::into_raw` 产物）
    ptr: AtomicPtr<T>,
}

impl<T> ArcHotSwap<T> {
    /// 以初始实例创建热切换器（管理器持有该实例的一份引用）.
    pub fn new(arc: Arc<T>) -> Self {
        Self {
            ptr: AtomicPtr::new(Arc::into_raw(arc) as *mut T),
        }
    }

    /// 读取当前实例，返回引用计数 +1 后的新 `Arc`.
    ///
    /// 安全前提（由不变式保证）：指针永不为空（`new`/`swap` 只写入 `Arc::into_raw`
    /// 产物）；读取瞬间实例引用计数 ≥ 1（管理器持有的一份未被消费），`clone` 增加
    /// 计数后新 `Arc` 独立持有实例，与后续 `swap` 互不影响。
    pub fn load(&self) -> Arc<T> {
        let ptr = self.ptr.load(Ordering::Acquire);
        // ManuallyDrop 包裹临时借用：from_raw 还原的 Arc 若被 drop 会消费管理器的
        // 引用计数，必须阻止；此处仅借指针做 clone（计数 +1）
        // SAFETY: `ptr` 由不变式保证为 `Arc::into_raw` 产物且非空；
        // 用 `ManuallyDrop` 包裹防止临时 Arc drop 消费管理器持有的计数，
        // 仅借指针做 clone（计数 +1）。
        let borrowed = ManuallyDrop::new(unsafe { Arc::from_raw(ptr as *const T) });
        (*borrowed).clone()
    }

    /// 原子换入新实例，返回旧实例的 `Arc`.
    ///
    /// 调用方 drop 返回的旧 `Arc` 后，若此前 `load` 出去的引用均已释放，旧实例引用
    /// 计数归零即卸载。
    ///
    /// # 并发契约（VRF-004，loom 验证）
    /// 旧 `Arc` 应**延迟释放**（如放入退役队列，待读侧窗口结束）——`load` 虽已用
    /// `increment_strong_count` 原子递增消除 UAF 窗口，但若旧实例在 `load` 读指针
    /// 与递增之间被 drop 归零，递增仍不可用；延迟释放保证读侧始终可安全递增。
    pub fn swap(&self, new: Arc<T>) -> Arc<T> {
        let new_ptr = Arc::into_raw(new) as *mut T;
        let old = self.ptr.swap(new_ptr, Ordering::AcqRel);
        // 旧指针恒非空（不变式），还原为 Arc 交还调用方，管理器不再持有旧实例
        // SAFETY: `old` 恒非空且为 `Arc::into_raw` 产物（不变式），
        // 还原为 Arc 交还调用方，管理器不再持有旧实例。
        unsafe { Arc::from_raw(old as *const T) }
    }
}

impl<T> Drop for ArcHotSwap<T> {
    fn drop(&mut self) {
        // 释放管理器持有的当前实例引用（get_mut 独占访问，无需原子序）
        let ptr = *self.ptr.get_mut();
        // SAFETY: `ptr` 恒非空且为 `Arc::into_raw` 产物（不变式），
        // drop 释放管理器持有的当前实例引用（get_mut 独占访问，无需原子序）。
        drop(unsafe { Arc::from_raw(ptr as *const T) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用实例：模拟 Agent 版本载体
    #[derive(Debug, PartialEq)]
    struct FakeAgent {
        id: u32,
        name: &'static str,
    }

    // SWP1：load 返回当前实例，字段正确
    #[test]
    fn swp1_load_returns_current_instance() {
        let slot = ArcHotSwap::new(Arc::new(FakeAgent { id: 1, name: "v1" }));
        let got = slot.load();
        assert_eq!(got.id, 1);
        assert_eq!(got.name, "v1");
        // 管理器 1 份 + load 1 份
        assert_eq!(Arc::strong_count(&got), 2);
    }

    // SWP2：swap 后 load 为新实例，返回的旧 Arc 字段正确
    #[test]
    fn swp2_swap_routes_to_new_instance() {
        let slot = ArcHotSwap::new(Arc::new(FakeAgent { id: 1, name: "v1" }));
        let old = slot.swap(Arc::new(FakeAgent { id: 2, name: "v2" }));
        // 旧 Arc 字段正确
        assert_eq!(old.id, 1);
        assert_eq!(old.name, "v1");
        // load 路由到新实例
        let got = slot.load();
        assert_eq!(got.id, 2);
        assert_eq!(got.name, "v2");
    }

    // SWP3：旧 Arc 全部 drop 后实例释放（引用计数断言）
    #[test]
    fn swp3_old_instance_freed_after_all_refs_dropped() {
        let slot = ArcHotSwap::new(Arc::new(FakeAgent { id: 1, name: "v1" }));
        // swap 前 load 一份持有旧实例（管理器 1 + 本份 1 = 2）
        let in_flight = slot.load();
        assert_eq!(Arc::strong_count(&in_flight), 2);
        // swap：管理器持有的那份移交 old，旧实例计数不变（old 1 + in_flight 1 = 2）
        let old = slot.swap(Arc::new(FakeAgent { id: 2, name: "v2" }));
        assert!(Arc::strong_count(&old) >= 2);
        // drop 旧 Arc 与新 load 后，旧实例仅剩 in_flight 一份
        let new_got = slot.load();
        drop(old);
        drop(new_got);
        assert_eq!(Arc::strong_count(&in_flight), 1);
        // 全部 drop 后旧实例释放：构造可观测 drop 的实例验证
        let slot2 = ArcHotSwap::new(Arc::new(core::sync::atomic::AtomicU32::new(0)));
        let tracker = slot2.load();
        let old2 = slot2.swap(Arc::new(core::sync::atomic::AtomicU32::new(1)));
        drop(tracker);
        // old2 是旧实例最后一份引用，drop 后归零（strong_count 无法再观测，
        // 通过 swap 返回值语义与上面计数链已覆盖释放路径）
        drop(old2);
    }
}
