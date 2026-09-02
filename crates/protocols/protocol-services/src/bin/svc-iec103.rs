//! svc-iec103 — IEC 60870-5-103 EL0 服务（蓝图 48 WP-48 能力壳——codec SpecBlocked，传输移交/真机路径后续卷；本 bin=descriptor 随 readiness 报告 + EXECUTE 显式 ST_UNAVAILABLE，诚实降级）。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
extern crate eneros_protocol_services;

#[cfg(target_os = "none")]
use eneros_protocol_abstract::plugin::ProtocolPlugin;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_protocol_services::HeapAllocator =
    eneros_protocol_services::HeapAllocator::new();

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start(ep: u64, ipc_buffer: u64, ready_ntfn: u64) -> ! {
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    // 39：协议插件在服务侧构造（descriptor 随 readiness 报告）；传输
    // capability 未接通（40 移交）→ 未分配 → EXECUTE 显式 ST_UNAVAILABLE。
    let plugin = eneros_iec103::plugin::Iec103Plugin::new();
    // 能力壳装配单点（63.2 WP-63.2.4 P1 收编）：descriptor 行 + 壳 +
    // driver_service_main 进 protocol_ladder——bin 直呼
    // 未分配壳服务的 bin 直呼违纪（C1 阶梯归零面）修除。
    eneros_protocol_services::protocol_ladder::run_capability_shell(
        ep, "iec103", ready_ntfn, plugin,
    )
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// host 构建 no-op 入口（与 svc-drv-* 同模式）。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
