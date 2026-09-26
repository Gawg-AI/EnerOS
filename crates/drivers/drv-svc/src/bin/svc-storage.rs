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
extern crate alloc;
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
    wall_epoch: u64,
    // 70.7（WP-70.7.2）：boot 池 1:1 基址（spawn 独立连续分配，**X7** 直传
    // ——第 8 参；X7 对 storage 空闲，UART 移交集互斥；0 = 启动后端不可用
    // 诚实降级）。不可用第 9 参：aarch64 C-ABI 第 9 参不经寄存器（run12/13
    // 实弹恒 0）。禁止按 "数据池 + N 页" 派生——REG-70.7-BOOT-BACKEND-
    // ZEROSIZE 根因：first-fit 落位不保证紧邻，派生地址写 ghost 页 FAULT。
    boot_dma_base: u64,
) -> ! {
    let _ = wall_epoch;
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
            // 70.7（WP-70.7.2）：启动盘（slot 0）受限安装面——boot 池基址
            // 经 X7 直传（0=不可用）；MMIO = 同页 slot 0（数据盘页内偏移
            // +0x400，boot = −0x400 回页基址）。
            if boot_dma_base != 0 && boot_dma_base & 0xfff == 0 {
                // F70-02（S1）：本次运行槽身份（root 只读身份页注入；缺失/
                // 无效 ⇒ 安装与确认写面整体关闭，fail-closed 不猜槽）。
                let running = eneros_drv_svc::boot_guard::running_identity_from_page(
                    eneros_handover_abi::STORAGE_BOOT_IDENTITY_VADDR,
                );
                match running {
                    Some(r) => {
                        eneros_runtime::println!(
                            "[svc-storage] BOOT-IDENTITY: slot={} secver={} release={:02x?}",
                            if r.is_a { "A" } else { "B" },
                            r.security_version,
                            &r.release_id[..4]
                        );
                    }
                    None => {
                        eneros_runtime::println!(
                            "[svc-storage] BOOT-IDENTITY: missing/invalid —— 安装与确认写面关闭（fail-closed）"
                        );
                    }
                }
                match eneros_drv_svc::blk_backend::BootSlotBackend::try_new_boot(
                    mmio_vaddr - 0x400,
                    boot_dma_base,
                    boot_dma_base,
                    running,
                ) {
                    Ok(boot) => {
                        use eneros_drv_svc::block_cmd::InstallBackend as _;
                        // 具名字段访问（73 批A：换位/漏位不可表示）；打印
                        // 词形与值逐字节不变。
                        let ranges = boot.install_ranges();
                        s.attach_install(alloc::boxed::Box::new(boot));
                        eneros_runtime::println!(
                            "[svc-storage] BOOT-INSTALL attached: inactive_lba={} lbas={} bootctrl_lba={}",
                            ranges.inactive_start_lba,
                            ranges.inactive_lbas,
                            ranges.bootctrl_start_lba
                        );
                    }
                    Err(e) => {
                        eneros_runtime::println!(
                            "[svc-storage] BOOT-INSTALL unavailable ({e}) —— 安装面禁用（诚实降级）"
                        );
                    }
                }
            } else {
                eneros_runtime::println!(
                    "[svc-storage] BOOT-INSTALL unavailable (no-handover) —— 安装面禁用（诚实降级）"
                );
            }
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

eneros_runtime::svc_panic!("svc-storage");

/// host 构建 no-op 入口（target 用 [`_start`]；与 svc-hello 同模式——
/// 使 `cargo clippy`/`cargo build` 在 host 上能编译本 target-only bin）。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
