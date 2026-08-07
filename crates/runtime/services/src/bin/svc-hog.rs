//! svc-hog — EnerOS 内存占用参考 EL0 服务（OOM 隔离验证，M0 占位）.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![allow(clippy::empty_loop)]

#[cfg(not(test))]
#[global_allocator]
static ALLOC: eneros_services::allocator::BumpAllocator =
    eneros_services::allocator::BumpAllocator::new();

#[cfg(not(test))]
use core::panic::PanicInfo;

#[cfg(not(test))]
extern crate eneros_services;

#[cfg(not(test))]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // v0.200.0 (P0-3)：真实内存占用——分配 64 MiB（16384 × 4 KiB）并保持。
    // OOM 隔离由 seL4 地址空间保证（QEMU 验证项）。
    let _held = eneros_services::hog_memory(16_384);
    eneros_services::svc_loop()
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    eneros_services::svc_loop()
}
