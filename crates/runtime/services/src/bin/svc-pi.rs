//! svc-pi — EnerOS CPU 密集型（π 计算）参考 EL0 服务（M0 占位）.

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
    // v0.200.0 (P0-3)：真实 CPU 密集计算——π 级数循环（100 万项/轮）。
    // 迭代结果（收敛于 π）可作 CPU 负载真实性的可验证证据。
    loop {
        let _ = eneros_services::compute_pi(1_000_000);
        core::hint::spin_loop();
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    eneros_services::svc_loop()
}
