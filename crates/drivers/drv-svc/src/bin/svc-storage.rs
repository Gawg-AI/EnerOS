//! svc-storage — 存储事务日志 EL0 服务（STO-002；38 WP-38.9：真实 IPC
//! loop，非自旋占位）。
//!
//! 41（WP-41 存储句柄移交，CVR-39-02 剩余面）：数据盘 virtio-blk 由
//! root spawn 时移交——MMIO 页 + 9 页连续 DMA 帧映射进服务 VSpace，
//! `_start` 收到 X3=mmio_vaddr / X4=dma_vaddr / X5=dma_phys。
//!
//! - 移交完成（三参数非零）→ [`VirtioBlkBackend`] 初始化真驱动 →
//!   `StorageService` capability "block" → EXECUTE 真读真写（QEMU
//!   数据盘，cache=none 断电语义）；
//! - 未移交（三参数为零，旧装配）→ 无后端 → EXECUTE 显式
//!   ST_UNAVAILABLE（不伪造 I/O）。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
extern crate eneros_drv_svc;

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
    mmio_vaddr: u64,
    dma_vaddr: u64,
    dma_phys: u64,
) -> ! {
    // K2.3：设置 IPC buffer TLS（__sel4_ipc_buffer）——libsel4 的
    // ipc_recv/get_mr 依赖该指针（21-A 遗留缺陷修复先例：svc-hello）。
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    // 41：存储句柄移交——三参数非零即初始化真 virtio-blk 后端；失败
    // 显式降级（无后端 → capability None → EXECUTE ST_UNAVAILABLE，
    // fail-closed 不伪造 I/O）。
    use eneros_drv_svc::block_cmd::BlockBackend as _;
    let mut service = match eneros_drv_svc::blk_backend::VirtioBlkBackend::try_new(
        mmio_vaddr, dma_vaddr, dma_phys,
    ) {
        Ok(mut backend) => {
            // 41 诊断：attach 前自检——读数据盘块 0 前 16 字节（真 I/O；
            // 失败打印具体错误，不伪装）。
            let mut probe = [0u8; 16];
            // 41 诊断：health（capacity）+ 读块 0。
            let (blocks, bs, h) = backend.health();
            eneros_runtime::println!(
                "[svc-storage] SELFCHECK: health blocks={} bs={} h={}",
                blocks,
                bs,
                h
            );
            match backend.read(0, &mut probe) {
                Ok(()) => {
                    eneros_runtime::println!(
                        "[svc-storage] SELFCHECK: block0 read OK {:02x?}",
                        probe
                    );
                }
                Err(e) => {
                    eneros_runtime::println!("[svc-storage] SELFCHECK: block0 read FAILED {:?}", e);
                }
            }
            let mut s = eneros_drv_svc::block_cmd::StorageService::new();
            s.attach(backend);
            eneros_runtime::println!(
                "[svc-storage] real block backend attached (mmio=0x{:x} dma=0x{:x} phys=0x{:x})",
                mmio_vaddr,
                dma_vaddr,
                dma_phys
            );
            s
        }
        Err(e) => {
            eneros_runtime::println!(
                "[svc-storage] block backend UNAVAILABLE: {} —— 显式降级（不伪造 I/O）",
                e
            );
            eneros_drv_svc::block_cmd::StorageService::new()
        }
    };
    // 38：通用驱动服务 IPC loop——envelope + bounded queue + deadline +
    // epoch；设备已移交 → capability Some("block") → EXECUTE 真读写。
    eneros_drv_svc::runtime::driver_service_main(
        ep,
        "storage",
        ready_ntfn,
        eneros_drv_svc::now_ms,
        eneros_drv_svc::instance_epoch(),
        &mut service,
    )
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // fail-closed：panic 挂起，由 root/watchdog 复位（不伪装继续服务）。
    loop {
        core::hint::spin_loop();
    }
}

/// host 构建 no-op 入口（target 用 [`_start`]；与 svc-hello 同模式——
/// 使 `cargo clippy`/`cargo build` 在 host 上能编译本 target-only bin）。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
