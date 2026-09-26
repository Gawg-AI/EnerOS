//! EnerOS user-space runtime library (workspace v0.192.0)
//!
//! Provides `print!`/`println!` macros and console initialization for
//! seL4 user-space Rust components. Depends on `eneros-sel4-sys` for
//! syscall FFI bindings.

#![no_std]
#![allow(clippy::empty_loop)]

pub mod console;
pub mod oom;
pub mod serial;

pub use console::init;

/// EL0 服务 panic 声化单源（蓝图 76 卷一 WP-76.1.1；宏体逐字承 WP-75.2.3
/// `eneros-business-services` 先例，自该 crate 下沉至五族公共依赖）。
///
/// ADR-026 §1 立法「EL0 服务 panic → 审计 + 打印 + 挂起」；本宏提供其中的
/// **打印下限**（`println` 死因 + `spin_loop` 挂起，不复位整机）。词形
/// `[<svc>] PANIC: {info}` 冻结依据 = REG-70.7 排障史（无声 spin 掩盖死因
/// 整整一个排障周期）+ vol5/6/7 PANIC-SCAN `none_of` 对 `"PANIC: panicked at"`
/// 的覆盖：真 panic 时串口行必命中门词。
///
/// host 测试构建展开为零（`cfg(target_os = "none")` 门）——这是宏方案不触
/// 链接期约束的关键：`#[panic_handler]` 定义权留在 bin crate 根（WP-69.3.1
/// 边界，protocol_ladder.rs 注释 + host 结构锁在案）。
///
/// 宏体用 `$crate::println!` 而非硬写 `eneros_runtime::println!`：前者在任意
/// 调用方展开都指回本 crate，消除「依赖本 crate 的 bin 之外误用即自引用」的
/// 陷阱；展开出的字节与后者恒等（词形零变化）。
///
/// 全族静默 handler 回流由 `crates/runtime/runtime/src/panic_guard.rs` 守卫
/// （21 bin 单点 + 豁免登记 REG-76-A/B/C）。
#[macro_export]
macro_rules! svc_panic {
    ($name:expr) => {
        #[cfg(target_os = "none")]
        #[panic_handler]
        fn panic(info: &core::panic::PanicInfo) -> ! {
            $crate::println!("[{}] PANIC: {}", $name, info);
            loop {
                core::hint::spin_loop();
            }
        }
    };
}
