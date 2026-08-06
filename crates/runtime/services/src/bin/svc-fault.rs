//! svc-fault — EnerOS 故障注入参考 EL0 服务（M0 占位）.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![allow(clippy::empty_loop)]

#[cfg(not(test))]
#[global_allocator]
static ALLOC: eneros_services::allocator::BumpAllocator = eneros_services::allocator::BumpAllocator::new();

#[cfg(not(test))]
use core::panic::PanicInfo;

#[cfg(not(test))]
extern crate eneros_services;

#[cfg(not(test))]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // v0.200.0 (P0-3)：故障注入服务——默认无故障（供 root-task 按需注入）。
    // 故障类型经 IPC 控制（M2.1 后）；当前保持无故障运行。
    let _ = eneros_services::inject_fault(None);
    eneros_services::svc_loop()
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    eneros_services::svc_loop()
}
