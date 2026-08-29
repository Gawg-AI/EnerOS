//! svc-mms — 协议 EL0 服务（46 WP-46.2 句柄移交 + 46.3 配置驱动装配：
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
use alloc::string::String;

#[cfg(target_os = "none")]
use eneros_iec61850_mms::plugin::MmsPlugin;
#[cfg(target_os = "none")]
use eneros_protocol_abstract::plugin::ProtocolPlugin;
#[cfg(target_os = "none")]
use eneros_protocol_abstract::PointRegistry;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_protocol_services::HeapAllocator =
    eneros_protocol_services::HeapAllocator::new();

/// 46.5：MMS 数据面对端事务——对注入点表的 MMS 对象引用发起一次 Read。
///
/// MMS 插件保持请求驱动契约（`client_mut` 由集成层发起 Read/Write）；本服务
/// 即集成层：关联（COTP+ACSE）建立后读一次映射点，独立对端回 Read-Response
/// 形成闭环（L2 transcript 事务 ≥3 判据）。失败不阻断装配（如实降级）。
#[cfg(target_os = "none")]
fn drive_mms_read(plugin: &mut MmsPlugin, registry: &mut dyn PointRegistry) {
    use eneros_iec61850_mms::VarAccessSpec;
    use eneros_protocol_abstract::ProtocolAddress;
    let mut ids = alloc::vec::Vec::new();
    registry.collect_ids(&mut ids);
    let mut specs = alloc::vec::Vec::new();
    for id in ids {
        if let Some(m) = registry.resolve(id) {
            if let ProtocolAddress::Mms { object_ref } = &m.protocol_addr {
                // 对象引用 "LD/xxx" → domain=LD, item=xxx（VarAccessSpec 布局）。
                if let Some((domain, item)) = object_ref.split_once('/') {
                    specs.push(VarAccessSpec {
                        domain: String::from(domain),
                        item: String::from(item),
                    });
                }
            }
        }
    }
    if specs.is_empty() {
        eneros_runtime::println!("[svc-mms] no MMS points in registry -> skip read");
        return;
    }
    match plugin.client_mut() {
        Some(client) => match client.read(&specs) {
            Ok(eneros_iec61850_mms::MmsResponse::ReadResult { results }) => {
                eneros_runtime::println!(
                    "[svc-mms] READ-RESP ok {} values ({} pts)",
                    results.len(),
                    specs.len()
                );
            }
            Ok(_) => {
                eneros_runtime::println!("[svc-mms] READ-RESP non-ReadResult");
            }
            Err(e) => {
                eneros_runtime::println!("[svc-mms] READ fail {:?}", e);
            }
        },
        None => {
            eneros_runtime::println!("[svc-mms] no MMS client (request-driven seam)");
        }
    };
}

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start(
    ep: u64,
    ipc_buffer: u64,
    ready_ntfn: u64,
    mmio_vaddr: u64,
    dma_vaddr: u64,
    dma_phys: u64,
    wall_epoch: u64,
) -> ! {
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    // smoltcp 传输用 eneros_time 单调时钟推进；EL0 服务进程内初始化。
    eneros_time::time_init(eneros_hal::arm64::timer::clock(), 0);
    // SPEC-B WP-B.1：X6 墙钟 epoch 秒（0=缺失显式降级）。
    eneros_protocol_services::apply_wall_epoch("mms", wall_epoch);
    // 39：协议插件在服务侧构造（descriptor 随 readiness 报告）。
    let plugin = eneros_iec61850_mms::plugin::MmsPlugin::new();
    eneros_runtime::println!(
        "[protocol-service] mms plugin={} role={:?} interop={:?}",
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
                let mut plugin = eneros_iec61850_mms::plugin::MmsPlugin::new();
                // 46.3：配置即 peer/app_id 唯一来源（protocol-protocol-mms）；不可得
                // → 能力壳（fail-closed，不猜默认）。
                match eneros_protocol_services::service_cfg::fetch_protocol_config(
                    "mms-config",
                    eneros_agent_sdk::EIPC_EP_BASE_SLOT,
                    "protocol-mms",
                ) {
                    Some(cfg) => {
                        // 46.5：点表经 svc-config 注入——插件按 MMS 地址登记
                        // 路由点数；本服务另以同一注册表驱动一次 MMS Read
                        //（数据面对端事务见 `drive_mms_read`）。
                        let mut registry =
                            eneros_protocol_services::service_cfg::fetch_points_registry(
                                "mms-config",
                                eneros_agent_sdk::EIPC_EP_BASE_SLOT,
                                |_| true,
                            );
                        match plugin.start(&cfg, &mut factory, &mut registry) {
                            Ok(()) => {
                                eneros_runtime::println!(
                                    "[svc-mms] NET-TRANSPORT READY capability=eneros_iec61850_mms::plugin::MmsPlugin"
                                );
                                // 46.5：关联建立后发起一次 MMS Read——独立对端
                                // 回 Read-Response（L2 transcript CONNECT/CC/AARE/
                                // READ_RESP ≥3）；插件保持请求驱动契约（Read 由
                                // 集成层发起），失败不阻断装配（如实健康观测）。
                                drive_mms_read(&mut plugin, &mut registry);
                                let mut io =
                                    eneros_protocol_services::service::PluginIoService::new(
                                        plugin,
                                        "eneros_iec61850_mms::plugin::MmsPlugin",
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
                                    "[svc-mms] plugin start FAILED {:?} —— capability 壳（EXECUTE ST_UNAVAILABLE）",
                                    e
                                );
                                Box::new(
                                    eneros_protocol_services::service::UnassignedProtocolService,
                                )
                            }
                        }
                    }
                    None => {
                        eneros_runtime::println!(
                            "[svc-mms] protocol config unavailable —— capability 壳（EXECUTE ST_UNAVAILABLE）"
                        );
                        Box::new(eneros_protocol_services::service::UnassignedProtocolService)
                    }
                }
            }
            Err(e) => {
                eneros_runtime::println!(
                    "[svc-mms] handover missing ({:?}) —— capability 壳（EXECUTE ST_UNAVAILABLE）",
                    e
                );
                Box::new(eneros_protocol_services::service::UnassignedProtocolService)
            }
        };
    eneros_drv_svc::runtime::driver_service_main(
        ep,
        "mms",
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
