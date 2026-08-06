//! 用户态 volatile MMIO 访问（DRV-003 SDK）。
//!
//! EL0 驱动通过 SYS_SHM_MAP 映射设备 MMIO 后，用本模块读写寄存器。
//! 所有访问走 volatile 指针；地址必须来自内核映射（不可任意构造）。

use core::ptr;

/// 32 位 MMIO 读。
#[inline(always)]
pub fn read32(addr: usize) -> u32 {
    // SAFETY：addr 为 SYS_SHM_MAP 映射的设备窗口 VA（调用方契约）。
    unsafe { ptr::read_volatile(addr as *const u32) }
}

/// 32 位 MMIO 写。
#[inline(always)]
pub fn write32(addr: usize, value: u32) {
    // SAFETY：同上；设备寄存器写。
    unsafe { ptr::write_volatile(addr as *mut u32, value) }
}

/// 64 位 MMIO 写。
#[inline(always)]
pub fn write64(addr: usize, value: u64) {
    // SAFETY：同上；64 位设备寄存器写（virtio 队列地址等）。
    unsafe { ptr::write_volatile(addr as *mut u64, value) }
}

/// 内存屏障：保证之前的内存访问在设备可见前完成（Device 内存隐含
/// 顺序性，但 DMA 缓冲是 Normal 内存，需要 dmb）。
#[inline(always)]
pub fn barrier() {
    // SAFETY：dmb sy 全屏障，无副作用。
    unsafe {
        core::arch::asm!("dmb sy", options(nostack));
    }
}

/// 结构体指针访问辅助：把 DMA 缓冲 VA 转成类型指针。
#[inline(always)]
pub fn buf_ptr<T>(va: usize) -> *mut T {
    va as *mut T
}
