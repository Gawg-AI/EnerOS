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
    // 蓝图 70.1 WP-70.1.1：堆水位探针注入（HEAP-WM 曲线的 bin 半边——
    // 堆所有权在本 bin 的 global_allocator 静态上）。
    eneros_control_services::dispatch_assembly::install_heap_probe(|| ALLOC.used_free());
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

eneros_runtime::svc_panic!("svc-control-dispatch");

/// host 构建 no-op 入口。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
