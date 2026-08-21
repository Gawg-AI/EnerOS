//! svc-modbus-rtu — iec101 协议 EL0 服务（蓝图 48 WP-48 能力壳——codec SpecBlocked，传输移交/真机路径后续卷；本 bin=descriptor 随 readiness 报告 + EXECUTE 显式 ST_UNAVAILABLE，诚实降级）。

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
    let plugin = eneros_iec101::plugin::Iec101Plugin::new();
    eneros_runtime::println!(
        "[protocol-service] iec101 plugin={} role={:?} interop={:?}",
        plugin.descriptor().id,
        plugin.descriptor().role,
        plugin.descriptor().interop_level
    );
    let mut service = eneros_protocol_services::service::UnassignedProtocolService;
    eneros_drv_svc::runtime::driver_service_main(
        ep,
        "iec101",
        ready_ntfn,
        eneros_protocol_services::now_ms,
        eneros_protocol_services::instance_epoch(),
        &mut service,
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
