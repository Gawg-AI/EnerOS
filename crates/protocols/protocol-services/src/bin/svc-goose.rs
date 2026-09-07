//! svc-goose — 协议 EL0 服务（S1：装配阶梯单源 `protocol_ladder`，bin 纯接线；
//! 46.3 配置即 peer 唯一来源——源码零 peer 常量。69.3 WP-69.3.1：共同壳收编
//! `bin_body`，bin 只剩 PARTS 声明 + 入口接线）。
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
extern crate eneros_protocol_services;
#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
use eneros_protocol_services::protocol_ladder::{DegradedStyle, PointsPolicy, ProtocolParts};

// 链接期约束留 bin（WP-69.3.1 边界：库导出会改多 bin 链接唯一性语义）。
#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_protocol_services::HeapAllocator =
    eneros_protocol_services::HeapAllocator::new();

/// 8 字段描述符（69.3 提为 static——`_start` 体成纯接线；字面与改造前
/// 内联字面量逐项相同，t-ladder-3 include_str! 锁）。
#[cfg(target_os = "none")]
static PARTS: ProtocolParts = ProtocolParts {
    service: "svc-goose",
    config_client: "goose-config",
    config_id: "protocol-goose",
    capability: "eneros_iec61850_goose::plugin::GoosePlugin",
    points: PointsPolicy::None,
    config_dump: false,
    self_check_rounds: 150,
    degraded: DegradedStyle::Standard,
};

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
    eneros_protocol_services::protocol_ladder::bin_body(
        ep,
        ready_ntfn,
        mmio_vaddr,
        dma_vaddr,
        dma_phys,
        wall_epoch,
        &PARTS,
        eneros_iec61850_goose::plugin::GoosePlugin::new,
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
