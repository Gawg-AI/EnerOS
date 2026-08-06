//! svc-drv-fault — 驱动故障注入 EL0 服务（M0 占位）.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![allow(clippy::empty_loop)]

#[cfg(not(test))]
use core::panic::PanicInfo;

#[cfg(not(test))]
extern crate eneros_drv_svc;

#[cfg(not(test))]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    eneros_drv_svc::drv_loop()
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    eneros_drv_svc::drv_loop()
}
