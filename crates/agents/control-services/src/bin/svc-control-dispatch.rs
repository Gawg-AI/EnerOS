//! svc-control-dispatch — 蓝图 40 WP-40.6：唯一 ControlDispatch 服务（EL0）。
//!
//! 装配（40 收尾 + SPEC-SERIES-2026-08-28/B WP-B.2）：产品链/投递通道/
//! 审计链等装配面全部下沉 `eneros_control_services::dispatch_assembly`
//! （fail-closed 阶梯 + 出站通道，host 可读、target 装配）；本 bin 只做
//! 装配接线（服务声明 + 槽位 + 判定行）。未授予 → UnboundChannel 显式
//! 失败（fail-closed）。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
extern crate eneros_control_services;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_drv_svc::allocator::HeapAllocator =
    eneros_drv_svc::allocator::HeapAllocator::new();

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start(
    ep: u64,
    ipc_buffer: u64,
    ready_ntfn: u64,
    eipc_mask: u64,
    heartbeat_slot: u64,
) -> ! {
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    // 装配面（fail-closed 阶梯 + 通道 + 审计）在 dispatch_assembly 单点；
    // 本 bin 只声明服务并进主循环。
    let mut handler = eneros_control_services::dispatch_assembly::DispatchHandler::new(eipc_mask);
    eneros_control_services::control_service_main(
        ep,
        "control-dispatch",
        ready_ntfn,
        eneros_drv_svc::now_ms,
        eneros_drv_svc::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // 死亡必须可见（svc-device-agent 词形先例；svc-telemetry 同款订正）。
    eneros_runtime::println!("[svc-control-dispatch] PANIC: {info}");
    loop {
        core::hint::spin_loop();
    }
}

/// host 构建 no-op 入口。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
