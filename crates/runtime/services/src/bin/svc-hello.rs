//! svc-hello — EnerOS hello-world 参考 EL0 服务（M0 占位）.

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
    // v0.200.0 (P0-3)：真实功能——循环输出带心跳计数的 banner。
    // M2.1 Endpoint IPC 接入后，banner 经 seL4 Endpoint 上报 root-task。
    loop {
        let _ = eneros_services::svc_hello("eneros");
        core::hint::spin_loop();
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    eneros_services::svc_loop()
}
