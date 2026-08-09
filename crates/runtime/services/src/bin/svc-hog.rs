//! svc-hog — EnerOS 内存占用参考 EL0 服务（K2.3：Endpoint IPC 入口）.

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
pub extern "C" fn _start(ep: u64, ipc_buffer: u64) -> ! {
    // K2.3：设置 IPC buffer TLS（__sel4_ipc_buffer）——被 spawn 的服务无
    // bootinfo，libsel4 的 ipc_recv/get_mr 依赖该指针；不设置会在 NULL
    // buffer 上解引用（21-A 遗留缺陷：svc_ipc_loop 首轮 recv 卡死）。
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // K2.3：内存占用由 IPC 请求驱动（hog=<n>），见 handle_request。
    eneros_runtime::init();
    eneros_services::svc_entry(ep, "hog")
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    eneros_services::svc_loop()
}
