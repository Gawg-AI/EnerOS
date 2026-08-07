//! svc-pi — EnerOS CPU 密集型（π 计算）参考 EL0 服务（K2.3：Endpoint IPC 入口）.

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
pub extern "C" fn _start(ep: u64, _ipc_buffer: u64) -> ! {
    // K2.3：π 计算由 IPC 请求驱动（pi=<n>），见 handle_request。
    eneros_runtime::init();
    eneros_services::svc_entry(ep, "pi")
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    eneros_services::svc_loop()
}
