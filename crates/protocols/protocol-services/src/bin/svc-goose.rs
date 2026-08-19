//! svc-goose — IEC 61850 GOOSE 协议 EL0 服务（蓝图 39 WP-39.11/12；46 WP-46.2
//! 句柄移交：virtio-net 槽 12 移交 → NetTransportFactory（raw） → GoosePlugin）。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
extern crate eneros_protocol_services;

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::boxed::Box;

#[cfg(target_os = "none")]
use eneros_protocol_abstract::plugin::ProtocolPlugin;
#[cfg(target_os = "none")]
use eneros_protocol_abstract::{EmptyPointRegistry, ProtocolConfig};

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_protocol_services::HeapAllocator =
    eneros_protocol_services::HeapAllocator::new();

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start(
    ep: u64,
    ipc_buffer: u64,
    ready_ntfn: u64,
    mmio_vaddr: u64,
    dma_vaddr: u64,
    dma_phys: u64,
) -> ! {
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    // smoltcp 传输用 eneros_time 单调时钟推进；EL0 服务进程内初始化。
    eneros_time::time_init(eneros_hal::arm64::timer::clock(), 0);
    // 39：协议插件在服务侧构造（descriptor 随 readiness 报告）。
    let plugin = eneros_iec61850_goose::plugin::GoosePlugin::new();
    eneros_runtime::println!(
        "[protocol-service] goose plugin={} role={:?} interop={:?}",
        plugin.descriptor().id,
        plugin.descriptor().role,
        plugin.descriptor().interop_level
    );
    // 46.2：手柄移交（virtio-net 槽 12，raw）→ 统一传输工厂 → 插件装配；
    // 未移交/装配失败 → 能力壳（EXECUTE ST_UNAVAILABLE，fail-closed）。
    let mut service: Box<dyn eneros_drv_svc::runtime::DriverService> =
        match eneros_protocol_services::net_transport::build_net_transport(
            mmio_vaddr, dma_vaddr, dma_phys,
        ) {
            Ok(stack) => {
                let mut factory =
                    eneros_protocol_services::net_transport::NetTransportFactory::new(stack);
                let mut plugin = eneros_iec61850_goose::plugin::GoosePlugin::new();
                let mut cfg = ProtocolConfig::default();
                // 46.2 测试默认（qemu-test 面）；46.3 改 config-bundle 注入。
                cfg.insert("go_cb_ref", "GGIO1.gcb1");
                cfg.insert("dataset_ref", "GGIO1.dsGOOSE");
                match plugin.start(&cfg, &mut factory, &mut EmptyPointRegistry) {
                    Ok(()) => {
                        eneros_runtime::println!(
                            "[svc-goose] NET-TRANSPORT READY capability=net-goose"
                        );
                        Box::new(eneros_protocol_services::service::PluginIoService::new(
                            plugin,
                            "net-goose",
                        ))
                    }
                    Err(e) => {
                        eneros_runtime::println!(
                            "[svc-goose] plugin start FAILED {:?} —— capability 壳（EXECUTE ST_UNAVAILABLE）",
                            e
                        );
                        Box::new(eneros_protocol_services::service::UnassignedProtocolService)
                    }
                }
            }
            Err(e) => {
                eneros_runtime::println!(
                    "[svc-goose] handover missing ({:?}) —— capability 壳（EXECUTE ST_UNAVAILABLE）",
                    e
                );
                Box::new(eneros_protocol_services::service::UnassignedProtocolService)
            }
        };
    eneros_drv_svc::runtime::driver_service_main(
        ep,
        "goose",
        ready_ntfn,
        eneros_protocol_services::now_ms,
        eneros_protocol_services::instance_epoch(),
        &mut *service,
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
