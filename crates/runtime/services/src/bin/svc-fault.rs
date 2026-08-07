//! svc-fault — EnerOS 故障注入参考 EL0 服务（K2.3：Endpoint IPC 入口）.

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
    // K2.3：故障注入由 IPC 请求驱动（fault=<kind>），见 handle_request；
    // 触发真实异常路径（除零/非法访问）→ 服务 TCB 被内核挂起（隔离验证）。
    eneros_runtime::init();
    eneros_services::svc_entry(ep, "fault")
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    eneros_services::svc_loop()
}
