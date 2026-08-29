//! svc-iec104 — 协议 EL0 服务（S1：装配阶梯单源 `protocol_ladder`，bin 纯接线；
//! 46.3 配置即 peer 唯一来源 + iec104 配置回显——ProtocolParts.config_dump）。
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
use eneros_protocol_services::protocol_ladder;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_protocol_services::HeapAllocator =
    eneros_protocol_services::HeapAllocator::new();

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
        service: "svc-iec104",
        config_client: "iec104-config",
        config_id: "protocol-iec104",
        capability: "eneros_iec104_master::plugin::Iec104Plugin",
        // 46.5：点表经 svc-config 注入——104 设备映射 → STARTDT/总召唤驱动
        // （EmptyPointRegistry 空映射 = 永不发起 104 握手，QEMU 实弹登记）。
        // SPEC-B：全量过滤（|_| true）。
        points: protocol_ladder::PointsPolicy::All,
        // 46.5 诊断：回显实际配置值（poll_interval 是否 5000）。
        config_dump: true,
        // 46.5：自检轮询窗口须覆盖 STARTDT→总召唤启动交换——总召唤由调度器
        // 在 connect + poll_interval_ms(5s) 触发（QEMU 实弹：150 轮≈3s 只到
        // Connected，INTERROGATION 未落地）；300 轮≈6s 使首个召唤落在窗口内。
        self_check_rounds: 300,
        degraded: protocol_ladder::DegradedStyle::Standard,
    };
    let mut io = protocol_ladder::Sel4ServiceIo::new(
        parts.service,
        parts.config_client,
        mmio_vaddr,
        dma_vaddr,
        dma_phys,
    );
    // 39：协议插件在服务侧构造（descriptor 随 readiness 报告）。
    let plugin = eneros_iec104_master::plugin::Iec104Plugin::new();
    protocol_ladder::print_descriptor(parts.service, &plugin);
    let mut service: Box<dyn eneros_drv_svc::runtime::DriverService> =
        match protocol_ladder::run(wall_epoch, &parts, plugin, &mut io) {
            protocol_ladder::Ladder::Ready { plugin, .. } => {
                let mut io = eneros_protocol_services::service::PluginIoService::new(
                    plugin,
                    parts.capability,
                );
                // 46.5：启动期自检轮询（窗口判据见 parts.self_check_rounds 注）。
                io.self_check_poll(parts.self_check_rounds);
                Box::new(io)
            }
            protocol_ladder::Ladder::Degraded(_) => protocol_ladder::capability_shell(),
        };
    eneros_drv_svc::runtime::driver_service_main(
        ep,
        "iec104",
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
