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

/// boot manager 加载 init bundle 的物理地址（boot-protocol boot_info.rs）。
/// M0 阶段只读 header 确认存在；M2+ 阶段解析 modules 并 spawn。
#[cfg(target_os = "none")]
const INIT_BUNDLE_PHYS: u64 = 0x4200_0000;

/// Root task 入口 — 由 seL4 elfloader 在 rootserver 模式下调用。
///
/// M0 行为：
/// 1. `eneros_runtime::init()` 初始化串口控制台
/// 2. 打印 root task banner（证明 Rust→seL4 FFI 真实启动）
/// 3. 探测 init bundle header（确认 boot manager 传递了 init bundle）
/// 4. 永久 loop（M2+ 改为 spawn EL0 服务后进入调度循环）
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

    // KRN-002：BootInfo 消费路径已就绪（boot::consume_boot_info：decode + validate，
    // host 侧 4 个回归测试覆盖合法/坏 magic/截断/未知 flag）。
    // M2.1（seL4 VSpace FFI）完成物理页映射后，从 INIT_BUNDLE_PHYS 读 BOOT_INFO_SIZE
    // 字节并调用 consume_boot_info，boot_id/selected_slot/init_bundle 由此进入运行时——
    // 08 §2「启动链断头路」闭合（验签链尾接物 = root-task 消费）。
    // EL0 服务装载复用 eneros-elf-loader（W^X），消息帧复用 eneros-ipc-wire（frame::*）。
    println!("[eneros-root-task] M0 milestone reached — BootInfo consumer ready (KRN-002)");
    println!("[eneros-root-task] KRN-004: service loop ready (dispatch_frame / config_service_roundtrip)");

    // KRN-004：真实服务进程主循环。M2.1 前以纯逻辑存在（host 可测）；
    // M2.1（seL4 Endpoint IPC）后：接收 EL0 帧 → dispatch_frame 分发 →
    // 心跳/故障监控（svc-fault 崩溃通知经 seL4 fault handler 到达）。
    // init bundle 解析 + EL0 spawn（W^X 校验后）在 M2.1 接入；本阶段
    // （v0.199.0）已实现 bundle 解析 + 服务注册表（host 可测），
    // 真实 Endpoint IPC 装载待 M2.1（sel4-sys FFI，v0.198.0 已提供封装）。
    loop {
        core::hint::spin_loop();
    }
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
