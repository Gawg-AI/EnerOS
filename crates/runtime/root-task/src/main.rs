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
#![cfg_attr(target_os = "none", feature(alloc_error_handler))]
#![allow(internal_features)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

extern crate eneros_runtime;

#[cfg(target_os = "none")]
extern crate alloc;

use eneros_runtime::{init, println};

pub mod allocator;
pub mod boot;
pub mod bundle;
pub mod frame;
pub mod panic;
pub mod service;
pub mod spawn;
pub mod supervisor;

// 全局分配器（no_std aarch64：spawn/IPC 路径使用 alloc，见 allocator.rs）。
#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: allocator::HeapAllocator = allocator::HeapAllocator::new();

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
///
/// # 入口 SP（BOOT-004）
/// seL4 `create_initial_thread` 只设置 PC 与 capRegister（x0），**不设置 SP**；
/// Rust 函数序言（`stp x29,x30,[sp,#-N]!`）在 SP 未初始化时即触发 vm fault
/// （QEMU 实测 `vm fault on data at 0xffffffffffffffa0 ... at 0x215780`）。
/// 故 `_start` 必须为汇编：先设 SP（.bss 栈顶）再跳 `rust_entry`——与
/// `third_party/sel4-root/root_task.c` 的 C 入口同模式。
#[cfg(target_os = "none")]
core::arch::global_asm!(
    ".pushsection .bss",
    ".balign 16",
    "root_task_stack_space: .skip 65536",
    "root_task_stack_top:",
    ".popsection",
    ".text",
    ".global _start",
    ".type _start, %function",
    "_start:",
    "    mov x19, x0", // 保存 bootinfo 指针（capRegister，内核设置）
    "    adrp x0, root_task_stack_top",
    "    add x0, x0, :lo12:root_task_stack_top",
    "    mov sp, x0",
    "    mov x0, x19", // 恢复 bootinfo 指针
    "    b {rust_entry}",
    rust_entry = sym rust_entry,
);

/// Rust 侧入口（由汇编 `_start` 设置 SP 后跳转）。
///
/// `bi` = seL4 内核在 capRegister（x0）传入的 bootinfo 物理指针。
/// 必须先 `boot_init(bi)`（BOOT-008）：root-task 的 Rust 汇编 `_start` 不经过
/// libsel4 的 sel4_start，libsel4 的 `bootinfo` 全局与 `__sel4_ipc_buffer` TLS
/// 均未初始化（GetBootInfo 返回 NULL、IPC 的 MR 访问会 vm fault）。
#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn rust_entry(bi: u64) -> ! {
    init();
    eneros_sel4_sys::safe::boot_init(bi);
    // 22 §2.W1.4：可回收堆初始化（首次分配前；println/boot_init 无分配）。
    // SAFETY: rust_entry 仅执行一次且早于任何 alloc 调用。
    unsafe { ALLOCATOR.init() };
    // 22 §2.W1.2：panic 审计链初始化（panic handler 最简路径的前提）。
    crate::panic::init_panic_audit();
    println!("[eneros-root-task] Rust root task alive on seL4");
    println!("[eneros-root-task] G2 closure: bootinfo/cap/IPC (inherited from elfloader boot)");
    println!(
        "[eneros-root-task] init_bundle @ paddr=0x{:x}",
        INIT_BUNDLE_PHYS
    );

    // BOOT-007（C2 修复）：init bundle 区域由 seL4 内核 boot.c 显式映射进
    // 初始线程 VSpace（1:1 @ 0x4200_0000，DTB /reserved-memory 排除 untyped
    // 防止 retype 消耗）。root-task 直接读物理地址即可，无需自建映射。
    // （21-A 方案 A 的 map_bundle_window 已被证伪：seL4_Untyped_Retype 的
    // node_offset 是 cnode 槽号而非 untyped 偏移，且 bundle 区域在 untyped
    // 列表中会被分配器 freeIndex 消耗——见 docs/adr/ 与 21-A K1.3 修订。）
    use eneros_sel4_sys::safe::{get_boot_info, BootInfoReader, CapInitThreadTable};
    let bi = get_boot_info();
    let reader = BootInfoReader::from_bootinfo_ptr(bi);
    let caps = CapInitThreadTable::resolve();
    let mut ctx = crate::spawn::SpawnCtx::from_bootinfo_reader(&reader, &caps);

    // 校验内核映射生效：读 bundle 首 8 字节（magic）。
    // SAFETY: 内核 boot.c 已把 [0x42000000, 0x42200000) 映射进初始 VSpace。
    let probe = unsafe { core::ptr::read_volatile(INIT_BUNDLE_PHYS as *const [u8; 8]) };
    crate::println!(
        "[eneros-root-task] BOOT-007 kernel-mapped bundle probe={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        probe[0], probe[1], probe[2], probe[3], probe[4], probe[5], probe[6], probe[7]
    );

    // 从物理地址读取 init bundle（header 64 B → total_len → 全量）
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
    // VSPACE_BASE = 0x200000：服务 ELF 链接基址（aarch64-unknown-none
    // 默认），服务映射到同一虚拟地址区域——ET_EXEC 固定地址代码的绝对
    // 引用才能解析（用 0x40000000 导致服务执行即 VMFAULT，QEMU 实测
    // type=1 pc=0x203554）。每个服务独立 VSpace，虚拟地址可相同。
    const VSPACE_BASE: u64 = 0x200000;
    const PAGE_SIZE: u64 = 0x1000;
    const STACK_SIZE: u64 = 0x4000;
    const EP_BASE: u64 = 100;
    // BOOT-007 诊断：打印 bundle 与 root 模块头部（定位 BadElf 根因）。
    crate::println!(
        "[eneros-root-task] DBG bundle total_len={} modules={}",
        bundle.total_len,
        bundle.modules.len()
    );
    for (i, m) in bundle.modules.iter().take(2).enumerate() {
        crate::println!(
            "[eneros-root-task] DBG mod[{}] kind={} name='{}' blob_len={} blob_magic={:02x}{:02x}{:02x}{:02x}",
            i,
            m.kind,
            m.name,
            m.blob.len(),
            m.blob.first().copied().unwrap_or(0),
            m.blob.get(1).copied().unwrap_or(0),
            m.blob.get(2).copied().unwrap_or(0),
            m.blob.get(3).copied().unwrap_or(0)
        );
    }
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
        // ctx/types/rights 已在上面（bundle 映射）创建，复用同一个。
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
fn panic(info: &PanicInfo) -> ! {
    // 22 §2.W1.2：审计 + 重启计数 + 武装看门狗短超时复位（不返回）。
    crate::panic::panic_reboot(info)
}

// 22 §2.W1.1：OOM 不 abort——记录指标 + 打印 + 挂起（fail-closed；
// 512 KiB bump 堆满 = spawn 计划失败，挂起由看门狗兜底，见 W1.2/W1.6）。
#[cfg(target_os = "none")]
#[alloc_error_handler]
fn oom(layout: core::alloc::Layout) -> ! {
    eneros_runtime::oom::oom_fail_safe(layout)
}
