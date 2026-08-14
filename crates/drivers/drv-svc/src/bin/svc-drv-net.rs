//! svc-drv-net — 驱动 EL0 服务（38 WP-38.9：真实 IPC loop，非自旋占位）。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
extern crate eneros_drv_svc;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_drv_svc::allocator::HeapAllocator =
    eneros_drv_svc::allocator::HeapAllocator::new();

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start(ep: u64, ipc_buffer: u64, ready_ntfn: u64) -> ! {
    // K2.3：设置 IPC buffer TLS（__sel4_ipc_buffer）——libsel4 的
    // ipc_recv/get_mr 依赖该指针（21-A 遗留缺陷修复先例：svc-hello）。
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    // 38：通用驱动服务 IPC loop——envelope + bounded queue + deadline +
    // epoch；设备未分配（39）→ capability None → EXECUTE 显式
    // ST_UNAVAILABLE（不伪造设备 I/O）。
    let mut service = eneros_drv_svc::runtime::UnassignedService;
    eneros_drv_svc::runtime::driver_service_main(
        ep,
        "drv-net",
        ready_ntfn,
        eneros_drv_svc::now_ms,
        eneros_drv_svc::instance_epoch(),
        &mut service,
    )
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // fail-closed：panic 挂起，由 root/watchdog 复位（不伪装继续服务）。
    loop {
        core::hint::spin_loop();
    }
}

/// host 构建 no-op 入口（target 用 [`_start`]；与 svc-hello 同模式——
/// 使 `cargo clippy`/`cargo build` 在 host 上能编译本 target-only bin）。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
