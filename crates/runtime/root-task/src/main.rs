//! EnerOS root task — seL4 用户态根任务（第五册 M0 地基）.
//!
//! 由 seL4 elfloader 加载为 rootserver，是用户态第一个进程。
//! 职责（按交付阶段递进）：
//! - M0：G2 闭环（bootinfo/cap/IPC）+ 串口 banner，证明 Rust root task 真实启动；
//!   init bundle 已由 boot manager 加载到 0x4200_0000，本阶段只确认其存在并报告，
//!   不 spawn EL0 服务（spawn 需要 seL4 TCB/CSpace/VSpace FFI，见 M2.1）。
//! - M2+：读取 init bundle，spawn 11 个 EL0 服务（svc-hello/.../svc-drv-fault）。
//!
//! 构建入口：xtask image raw → build_user_elf("eneros-root-task", "eneros-root-task")。
//! 交叉编译：cargo build -p eneros-root-task --target aarch64-unknown-none \
//!           -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem
//!
//! no_std 合规：仅 core/alloc，子模块不重复 no_std 声明。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![cfg_attr(target_os = "none", feature(lang_items))]
#![allow(internal_features)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

extern crate eneros_runtime;

#[cfg(target_os = "none")]
extern crate alloc;

use eneros_runtime::{init, println};

pub mod boot;
pub mod bundle;
pub mod frame;
pub mod service;
pub mod spawn;

/// boot manager 加载 init bundle 的物理地址（boot-protocol boot_info.rs）。
/// M0 阶段只读 header 确认存在；M2+ 阶段解析 modules 并 spawn。
#[cfg(target_os = "none")]
const INIT_BUNDLE_PHYS: u64 = 0x4200_0000;

/// Root task 入口 — 由 seL4 elfloader 在 rootserver 模式下调用。
///
/// M0+ 行为（蓝图/20 R1-2）：
/// 1. `eneros_runtime::init()` 初始化串口控制台
/// 2. 打印 root task banner（证明 Rust→seL4 FFI 真实启动）
/// 3. 消费 BootInfo + 解析 init bundle（INIT_BUNDLE_PHYS）
/// 4. `spawn::plan_spawn` 生成 spawn 计划（root + ≥2 服务，W^X 页表）
/// 5. `spawn::sel4_spawn_services` 真实 spawn（M2.1 sel4-sys FFI）
/// 6. 失败即挂起（fail-closed，不进入伪运行态）
#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    init();
    println!("[eneros-root-task] Rust root task alive on seL4");
    println!("[eneros-root-task] G2 closure: bootinfo/cap/IPC (inherited from elfloader boot)");
    println!(
        "[eneros-root-task] init_bundle @ paddr=0x{:x}",
        INIT_BUNDLE_PHYS
    );

    // R1-2：从物理地址读取 init bundle（header 64 B → total_len → 全量）
    let raw = read_bundle_from_phys(INIT_BUNDLE_PHYS);
    let bundle = match raw {
        Some(raw) => match crate::bundle::InitBundle::parse(&raw) {
            Ok(b) => b,
            Err(e) => {
                println!(
                    "[eneros-root-task] init bundle parse FAILED: {:?} — 挂起（fail-closed）",
                    e
                );
                loop {
                    core::hint::spin_loop();
                }
            }
        },
        None => {
            println!(
                "[eneros-root-task] init bundle missing @ paddr=0x{:x} — 挂起（fail-closed）",
                INIT_BUNDLE_PHYS
            );
            loop {
                core::hint::spin_loop();
            }
        }
    };

    // R1-2：spawn 计划（root + ≥2 服务，W^X 页表 + 栈 + IPC EP）
    const VSPACE_BASE: u64 = 0x4000_0000;
    const PAGE_SIZE: u64 = 0x1000;
    const STACK_SIZE: u64 = 0x4000;
    const EP_BASE: u64 = 100;
    let plan = match crate::spawn::plan_spawn(&bundle, VSPACE_BASE, PAGE_SIZE, STACK_SIZE, EP_BASE)
    {
        Ok(plan) => plan,
        Err(e) => {
            println!(
                "[eneros-root-task] spawn plan FAILED: {:?} — 挂起（fail-closed）",
                e
            );
            loop {
                core::hint::spin_loop();
            }
        }
    };
    println!(
        "[eneros-root-task] spawn plan: root={} services={} (≥2 验收口径)",
        plan.root.name,
        plan.service_count()
    );
    for svc in &plan.services {
        println!(
            "[eneros-root-task]   spawn {} entry=0x{:x} stack_top=0x{:x} ipc_ep={} pages={}",
            svc.name,
            svc.entry,
            svc.stack_top,
            svc.ipc_ep,
            svc.pages.len()
        );
    }

    // R1-2：真实 spawn（21-A K1.2：sel4-sys FFI 完整序列，ADR-0034）
    #[cfg(target_os = "none")]
    {
        use eneros_sel4_sys::safe::{get_boot_info, BootInfoReader, CapInitThreadTable};
        // 从 bootinfo 构建 spawn 上下文（untyped 分配器 + cap 槽 + 固定 cap）。
        let bi = get_boot_info();
        let reader = BootInfoReader::from_bootinfo_ptr(bi);
        let caps = CapInitThreadTable::resolve();
        let mut ctx = crate::spawn::SpawnCtx::from_bootinfo_reader(&reader, &caps);
        match crate::spawn::sel4_spawn_services(&plan, &bundle, &mut ctx) {
            Ok(handles) => {
                println!("[eneros-root-task] EL0 services spawned — entering service loop");
                // K2.2：IPC 调度主循环（替换 spin_loop，21-A）。
                crate::service::run_ipc_loop(&handles);
            }
            Err(e) => {
                println!(
                    "[eneros-root-task] spawn FAILED: {:?} — 挂起（fail-closed）",
                    e
                );
                loop {
                    core::hint::spin_loop();
                }
            }
        }
    }
    // host 构建：无 seL4，不 spawn（保持原行为，仅计划打印）。
    #[cfg(not(target_os = "none"))]
    {
        println!("[eneros-root-task] host build: spawn skipped (no seL4)");
    }

    // host 构建下的静态路径（sel4_target 分支已进入 run_ipc_loop 不返回）。
    loop {
        core::hint::spin_loop();
    }
}

/// 从物理地址读取 init bundle（目标环境：seL4 页表直接映射，物理地址可读）。
///
/// header 中的 `total_len` 无信任：超过 [`MAX_BUNDLE_LEN`]（64 MiB）即拒绝
/// （fail-closed，防损坏/恶意 header 触发巨量分配）。
#[cfg(target_os = "none")]
const MAX_BUNDLE_LEN: usize = 64 * 1024 * 1024;

/// 从物理地址读取 init bundle。
#[cfg(target_os = "none")]
fn read_bundle_from_phys(phys: u64) -> Option<alloc::vec::Vec<u8>> {
    let header_ptr = phys as *const u8;
    let mut header = [0u8; 64];
    unsafe {
        core::ptr::copy_nonoverlapping(header_ptr, header.as_mut_ptr(), 64);
    }
    if &header[..8] != crate::bundle::INIT_BUNDLE_MAGIC {
        return None;
    }
    let total = u32::from_le_bytes(header[12..16].try_into().ok()?) as usize;
    if total < 64 || total > MAX_BUNDLE_LEN {
        return None;
    }
    let mut buf = alloc::vec![0u8; total];
    unsafe {
        core::ptr::copy_nonoverlapping(header_ptr, buf.as_mut_ptr(), total);
    }
    Some(buf)
}

/// Host 侧入口 — 演示 BootInfo 消费链（KRN-002），同时让 workspace 构建可链接。
#[cfg(not(target_os = "none"))]
fn main() {
    init();
    let bi = eneros_boot_protocol::boot_info::BootInfo::new(
        eneros_boot_protocol::boot_info::BootSlot::A,
        eneros_boot_protocol::boot_info::BootReason::Cold,
    );
    let raw = bi.to_le_bytes();
    if let Ok(parsed) = boot::consume_boot_info(&raw) {
        println!(
            "[eneros-root-task] host demo: BootInfo consumed (slot={:?}, init_bundle_phys=0x{:x})",
            parsed.selected_slot, parsed.init_bundle_phys
        );
    }
    println!("[eneros-root-task] host build (no seL4)");
}
#[cfg(target_os = "none")]
#[lang = "eh_personality"]
extern "C" fn eh_personality() {}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    println!("[eneros-root-task] PANIC");
    loop {
        core::hint::spin_loop();
    }
}
