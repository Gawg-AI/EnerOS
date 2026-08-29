//! svc-mms — 协议 EL0 服务（S1：装配阶梯单源 `protocol_ladder`，bin 纯接线
//! + `drive_mms_read` 数据面后处理；46.3 配置即 peer 唯一来源）。
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
use eneros_protocol_abstract::PointRegistry;
#[cfg(target_os = "none")]
use eneros_protocol_services::protocol_ladder;

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

/// 46.2/46.3 + S1：手柄移交（virtio-net 槽）→ 统一传输工厂 → 配置装配
/// 插件；未移交/配置不可得/装配失败 → 能力壳（fail-closed）。
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
    let parts = protocol_ladder::ProtocolParts {
        service: "svc-mms",
        config_client: "mms-config",
        config_id: "protocol-mms",
        capability: "eneros_iec61850_mms::plugin::MmsPlugin",
        points: protocol_ladder::PointsPolicy::All,
        config_dump: false,
        self_check_rounds: 150,
    };
    let mut io = protocol_ladder::Sel4ServiceIo::new(
        parts.service,
        parts.config_client,
        mmio_vaddr,
        dma_vaddr,
        dma_phys,
    );
    // 39：协议插件在服务侧构造（descriptor 随 readiness 报告）。
    let plugin = MmsPlugin::new();
    protocol_ladder::print_descriptor(parts.service, &plugin);
    let mut service: Box<dyn eneros_drv_svc::runtime::DriverService> =
        match protocol_ladder::run(wall_epoch, &parts, plugin, &mut io) {
            protocol_ladder::Ladder::Ready {
                mut plugin,
                mut registry,
            } => {
                // 46.5：关联建立后发起一次 MMS Read——独立对端回
                // Read-Response（L2 transcript CONNECT/CC/AARE/READ_RESP ≥3）；
                // 插件保持请求驱动契约，失败不阻断装配（如实健康观测）。
                drive_mms_read(&mut plugin, &mut registry);
                let mut io = eneros_protocol_services::service::PluginIoService::new(
                    plugin,
                    parts.capability,
                );
                // 46.5：启动期自检轮询（post-connect 交换尽早完成；有界）。
                io.self_check_poll(parts.self_check_rounds);
                Box::new(io)
            }
            protocol_ladder::Ladder::Degraded(_) => protocol_ladder::capability_shell(),
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
