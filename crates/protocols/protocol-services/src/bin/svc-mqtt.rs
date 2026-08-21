//! svc-mqtt — 协议 EL0 服务（46 WP-46.2 句柄移交 + 46.3 配置驱动装配：
//! virtio-net 槽移交 → NetTransportFactory → 插件 start（配置经 svc-config，
//! 源码零 peer 常量——46.3 DoD）。
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
use eneros_protocol_abstract::EmptyPointRegistry;

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
    let plugin = eneros_mqtt::plugin::MqttPlugin::new();
    eneros_runtime::println!(
        "[protocol-service] mqtt plugin={} role={:?} interop={:?}",
        plugin.descriptor().id,
        plugin.descriptor().role,
        plugin.descriptor().interop_level
    );
    // 46.2/46.3：手柄移交（virtio-net 槽，proto_net_slot）→ 统一传输工厂 →
    // 配置装配插件；未移交/配置不可得/装配失败 → 能力壳（fail-closed）。
    let mut service: Box<dyn eneros_drv_svc::runtime::DriverService> =
        match eneros_protocol_services::net_transport::build_net_transport(
            mmio_vaddr, dma_vaddr, dma_phys,
        ) {
            Ok(stack) => {
                let mut factory =
                    eneros_protocol_services::net_transport::NetTransportFactory::new(stack);
                let mut plugin = eneros_mqtt::plugin::MqttPlugin::new();
                // 46.3：配置即 peer/app_id 唯一来源（protocol-protocol-mqtt）；不可得
                // → 能力壳（fail-closed，不猜默认）。
                match eneros_protocol_services::service_cfg::fetch_protocol_config(
                    "mqtt-config",
                    eneros_agent_sdk::EIPC_EP_BASE_SLOT,
                    "protocol-mqtt",
                ) {
                    Some(cfg) => match plugin.start(&cfg, &mut factory, &mut EmptyPointRegistry) {
                        Ok(()) => {
                            eneros_runtime::println!(
                                "[svc-mqtt] NET-TRANSPORT READY capability=eneros_mqtt::plugin::MqttPlugin"
                            );
                            let mut io = eneros_protocol_services::service::PluginIoService::new(
                                plugin,
                                "eneros_mqtt::plugin::MqttPlugin",
                            );
                            // 46.5：启动期自检轮询——post-connect 协议交换
                            // 在进入 request 循环前完成（104 STARTDT/总召唤、
                            // mqtt CONNECT/SUB、raw RX；L2 对端 transcript ≥3
                            // 判据；有界不阻塞对端）。
                            io.self_check_poll(150);
                            Box::new(io)
                        }
                        Err(e) => {
                            eneros_runtime::println!(
                                "[svc-mqtt] plugin start FAILED {:?} —— capability 壳（EXECUTE ST_UNAVAILABLE）",
                                e
                            );
                            Box::new(eneros_protocol_services::service::UnassignedProtocolService)
                        }
                    },
                    None => {
                        eneros_runtime::println!(
                            "[svc-mqtt] protocol config unavailable —— capability 壳（EXECUTE ST_UNAVAILABLE）"
                        );
                        Box::new(eneros_protocol_services::service::UnassignedProtocolService)
                    }
                }
            }
            Err(e) => {
                eneros_runtime::println!(
                    "[svc-mqtt] handover missing ({:?}) —— capability 壳（EXECUTE ST_UNAVAILABLE）",
                    e
                );
                Box::new(eneros_protocol_services::service::UnassignedProtocolService)
            }
        };
    eneros_drv_svc::runtime::driver_service_main(
        ep,
        "mqtt",
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
