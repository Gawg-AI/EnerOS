//! 38（WP-38.5）：boot-only IRQ 注册表（冻结表）——取代 `static mut`。
//!
//! 生命周期：boot 阶段（中断关闭、单写者）`register/unregister` →
//! 装配完成后 `freeze()` → 此后只有中断上下文的只读 `resolve`。
//! 冻结后注册返回 [`crate::HalError::RegistrationAfterFreeze`]（fail-closed）。
//!
//! # SAFETY（Sync 不变量）
//! - 写路径（register/unregister）仅在 freeze 前由单写者执行（boot 装配，
//!   中断未使能——无并发读者）；
//! - 读路径（resolve）在 freeze 后由中断上下文执行（处理器指针宽度读取，
//!   ARM64 对齐指针读写原子）；
//! - `frozen` 标志以 Acquire/Release 排序建立注册表发布边界。

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::types::HalError;
use crate::IrqHandler;

/// 表容量（GICv3 分发器 256 INTID；其他控制器按需扩）。
pub const IRQ_TABLE_CAP: usize = 256;

/// Boot-only IRQ 注册表。
pub struct IrqHandlerTable {
    entries: UnsafeCell<[Option<IrqHandler>; IRQ_TABLE_CAP]>,
    frozen: AtomicBool,
}

// SAFETY: 见模块头——单写者 boot 注册 + 冻结后中断上下文只读；
// `entries` 的写读由 frozen 标志建立 happens-before（Acquire/Release）。
unsafe impl Sync for IrqHandlerTable {}

impl IrqHandlerTable {
    pub const fn new() -> Self {
        Self {
            entries: UnsafeCell::new([None; IRQ_TABLE_CAP]),
            frozen: AtomicBool::new(false),
        }
    }

    /// 注册（boot 阶段；冻结后拒绝）。
    pub fn register(&self, irq: u32, handler: IrqHandler) -> Result<(), HalError> {
        if self.frozen.load(Ordering::Acquire) {
            return Err(HalError::RegistrationAfterFreeze);
        }
        if irq >= IRQ_TABLE_CAP as u32 {
            return Err(HalError::InvalidParam);
        }
        // SAFETY: 单写者（freeze 前）独占访问；`irq < IRQ_TABLE_CAP` 已校验。
        unsafe {
            (*self.entries.get())[irq as usize] = Some(handler);
        }
        Ok(())
    }

    pub fn unregister(&self, irq: u32) -> Result<(), HalError> {
        if self.frozen.load(Ordering::Acquire) {
            return Err(HalError::RegistrationAfterFreeze);
        }
        if irq >= IRQ_TABLE_CAP as u32 {
            return Err(HalError::InvalidParam);
        }
        // SAFETY: 同 `register`（单写者独占）。
        unsafe {
            (*self.entries.get())[irq as usize] = None;
        }
        Ok(())
    }

    /// 冻结注册表（boot 装配完成后调用一次；此后只读）。
    pub fn freeze(&self) {
        self.frozen.store(true, Ordering::Release);
    }

    pub fn is_frozen(&self) -> bool {
        self.frozen.load(Ordering::Acquire)
    }

    /// 只读查找（中断上下文；越界 → None，绝不 panic）。
    pub fn resolve(&self, irq: u32) -> Option<IrqHandler> {
        if irq >= IRQ_TABLE_CAP as u32 {
            return None;
        }
        // SAFETY: `irq < IRQ_TABLE_CAP` 已校验；读路径在 freeze 后由中断
        // 上下文独占执行（或测试单线程）。
        unsafe { (*self.entries.get())[irq as usize] }
    }
}

impl Default for IrqHandlerTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy(_irq: u32) -> crate::IrqAction {
        crate::IrqAction::Handled
    }

    #[test]
    fn register_resolve_unregister_roundtrip() {
        let t = IrqHandlerTable::new();
        t.register(42, dummy).expect("freeze 前注册必须成功");
        assert!(t.resolve(42).is_some());
        assert!(t.resolve(7).is_none());
        t.unregister(42).expect("freeze 前注销必须成功");
        assert!(t.resolve(42).is_none());
    }

    #[test]
    fn out_of_range_rejected() {
        let t = IrqHandlerTable::new();
        assert_eq!(
            t.register(IRQ_TABLE_CAP as u32, dummy),
            Err(HalError::InvalidParam)
        );
        assert_eq!(t.register(u32::MAX, dummy), Err(HalError::InvalidParam));
        assert!(t.resolve(u32::MAX).is_none());
        assert!(t.resolve(IRQ_TABLE_CAP as u32).is_none());
        assert!(t.resolve(1022).is_none(), "保留 INTID 不 panic");
    }

    #[test]
    fn freeze_blocks_registration_keeps_reads() {
        let t = IrqHandlerTable::new();
        t.register(7, dummy).unwrap();
        t.freeze();
        assert!(t.is_frozen());
        assert_eq!(t.register(8, dummy), Err(HalError::RegistrationAfterFreeze));
        assert_eq!(t.unregister(7), Err(HalError::RegistrationAfterFreeze));
        // 冻结后只读路径仍可用（中断上下文）。
        assert!(t.resolve(7).is_some());
        assert!(t.resolve(8).is_none());
    }

    #[test]
    fn duplicate_register_replaces() {
        let t = IrqHandlerTable::new();
        t.register(1, dummy).unwrap();
        t.register(1, dummy).unwrap(); // 覆盖更新（同一 slot）
        assert!(t.resolve(1).is_some());
        assert_eq!(t.resolve(1).unwrap()(1), crate::IrqAction::Handled);
    }
}
