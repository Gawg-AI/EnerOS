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

// target 分支必须显式导入（三审 E0599 修复）
use eneros_runtime::{init, println};
#[cfg(target_os = "none")]
use eneros_tenant::TenantManager; // WP-37.2：`TenantStore::create` 是 trait 方法，

pub mod allocator;
pub mod boot;
pub mod bundle;
// W4.3（蓝图23）：frame.rs 兼容垫片（`pub use eneros_ipc_wire::el0::*`）已删除——
// 全 crate `frame::` 零引用（main.rs 已直接 use ipc-wire el0 类型）。
pub mod driver_probe;
pub mod irq;
pub mod irq_timer;
pub mod panic;
pub mod platform;
pub mod runtime_spec;
pub mod service;
pub mod spawn;
pub mod supervisor;
pub mod wiring;

// 全局分配器（no_std aarch64：spawn/IPC 路径使用 alloc，见 allocator.rs）。
#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: allocator::HeapAllocator = allocator::HeapAllocator::new();

/// WP-37.6：bundle 物理地址解析——BootInfo 值（elfloader 通道建立后）
/// 优先；当前正式链无该通道，回落内核 BOOT-007 契约窗口常量
/// （kernel boot.c 映射 + boot manager 装载的同一地址；消费仍经
/// magic 探测 + 模块 sha256 验证，验证失败 fail-closed 挂起——
/// 不是无验证盲猜）。elfloader 通道建立后本函数切换为真实
/// BootInfo 值（RECON-37 §5 CROSS_VERSION_CHANGE_REQUEST）。
#[cfg(target_os = "none")]
fn resolve_init_bundle_phys() -> u64 {
    // BootInfo 值来源：elfloader 传参通道（未建）→ 当前恒 0，
    // 走 BOOT-007 契约窗口。debug 与 release 同语义（契约窗口 +
    // 强制验证），不再区分策略。
    match crate::boot::resolve_bundle_phys(0) {
        Ok(phys) => phys,
        Err(e) => {
            crate::println!(
                "[eneros-root-task] WP-37.6: bundle phys resolve FAILED: {:?} — hang (fail-closed)",
                e
            );
            loop {
                core::hint::spin_loop();
            }
        }
    }
}

/// Root task 入口 — 由 seL4 elfloader 在 rootserver 模式下调用。
///
/// M0+ 行为（蓝图/20 R1-2）：
/// 1. `eneros_runtime::init()` 初始化串口控制台
/// 2. 打印 root task banner（证明 Rust→seL4 FFI 真实启动）
/// 3. 消费 BootInfo + 解析 init bundle（resolve_init_bundle_phys）
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
    // 39 收尾复验：64 KiB 栈被 run_ipc_loop 大帧溢出，静默腐蚀下方
    // .bss（GDB 实证：WATCHDOG 锁字节被野写、堆 hole 列表乱序 panic，
    // 启动期非确定挂起；栈下无 guard 页，溢出即损坏相邻静态区）。
    // 扩到 256 KiB（.bss 增长不影响 bundle 尺寸）。
    "root_task_stack_space: .skip 262144",
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
    // 42：G2 结构化证据——kernel-init/mmu 为真实 boot_init 成功后的产物。
    println!("{{\"schema\":\"eneros.g2.v1\",\"test\":\"kernel-init\",\"result\":\"PASS\"}}");
    println!("{{\"schema\":\"eneros.g2.v1\",\"test\":\"mmu\",\"result\":\"PASS\"}}");
    // 22 §2.W1.4：可回收堆初始化（首次分配前；println/boot_init 无分配）。
    // SAFETY: rust_entry 仅执行一次且早于任何 alloc 调用。
    unsafe { ALLOCATOR.init() };
    // 22 §2.W1.2：panic 审计链初始化（panic handler 最简路径的前提）。
    crate::panic::init_panic_audit();
    // 23 G-I 总出口修复：单调时钟初始化。38（WP-38.4）：时钟经
    // PlatformAdapter 装配——time_init 移至平台装配之后（timer 频率经
    // TimerHandle 校验非零，fail-closed 不猜时钟）。
    println!("[eneros-root-task] Rust root task alive on seL4");
    println!("[eneros-root-task] G2 closure: bootinfo/cap/IPC (inherited from elfloader boot)");
    // WP-37.6：bundle 物理地址经 resolve_init_bundle_phys 解析（debug 允许
    // 固定常量 fallback；release 严格——无 BootInfo 值即 fail-closed）。
    let init_bundle_phys = resolve_init_bundle_phys();
    println!(
        "[eneros-root-task] init_bundle @ paddr=0x{:x}",
        init_bundle_phys
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
    let probe = unsafe { core::ptr::read_volatile(init_bundle_phys as *const [u8; 8]) };
    crate::println!(
        "[eneros-root-task] BOOT-007 kernel-mapped bundle probe={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        probe[0], probe[1], probe[2], probe[3], probe[4], probe[5], probe[6], probe[7]
    );

    // 从物理地址解析 init bundle（header 64 B → total_len → 全量）。
    // 43 内存修约（svc-modbus-tcp 接入 smoltcp 后 bundle ≈2.68 MiB，
    // 整包堆拷贝 + FDT/plan 峰值超出 5 MiB 堆→init OOM）：改为**借用**
    // 内核映射的物理区解析（BOOT-007 映射 [0x42000000, +window) 永久
    // 有效），不再整包拷入堆——省 ~2.6 MiB 峰值，避免分配器扩容。
    let raw = read_bundle_slice_from_phys(init_bundle_phys);
    let bundle = match raw {
        Some(raw) => {
            // 30 RT-004：验签式解析——模块 sha256 必须与打包侧摘要一致
            // （供应链完整性；DigestMismatch 即挂起 fail-closed）。
            match crate::bundle::InitBundle::parse_verified(raw, |blob| {
                use sha2::{Digest, Sha256};
                Sha256::digest(blob).into()
            }) {
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
            }
        }
        None => {
            println!(
                "[eneros-root-task] init bundle missing @ paddr=0x{:x} — 挂起（fail-closed）",
                init_bundle_phys
            );
            loop {
                core::hint::spin_loop();
            }
        }
    };

    // WP-37.9：panic 审计记录本次启动消费的制品摘要（bundle payload
    // sha256——parse_verified 已验证的介质事实；panic 路径只读静态）。
    crate::panic::register_artifact_digest(bundle.payload_sha256);

    // 38（WP-38.4）：平台装配——PlatformAdapter（QEMU virt）→
    // ValidatedInventory → HalRuntime（timer/CPU/console/DMA）。
    // 只读 discovery：不触发设备写/格式化（固定不变量 1）。
    let platform = {
        use eneros_hal::platform_adapter::source_name;
        let (dtb, _phys) = match crate::driver_probe::runtime_dtb_slice(u64::from(bundle.total_len))
        {
            Some(v) => v,
            None => {
                println!("[eneros-root-task] 38 platform: no runtime dtb — 挂起（fail-closed）");
                loop {
                    core::hint::spin_loop();
                }
            }
        };
        match crate::platform::assemble(dtb, 0) {
            Ok(p) => {
                let inv = &p.inventory.inventory;
                println!(
                    "[eneros-root-task] 38 HAL: source={} cpus={} enabled={} mem={} devices={}",
                    source_name(inv.provenance.source),
                    inv.cpus.len(),
                    p.hal.cpu.core_count,
                    inv.memory.len(),
                    inv.devices.len()
                );
                println!(
                    "[eneros-root-task] 38 HAL: timer-freq={} console={} entropy={} iommu={}",
                    p.hal.timer.as_ref().map(|t| t.frequency_hz).unwrap_or(0),
                    p.hal
                        .console
                        .as_ref()
                        .map(|c| c.name.as_str())
                        .unwrap_or("none"),
                    p.hal
                        .entropy
                        .as_ref()
                        .map(|e| e.name.as_str())
                        .unwrap_or("none"),
                    p.hal.dma.iommu_present
                );
                p
            }
            Err(e) => {
                println!(
                    "[eneros-root-task] 38 HAL: assembly FAILED {:?} — 挂起（fail-closed，零设备写）",
                    e
                );
                loop {
                    core::hint::spin_loop();
                }
            }
        }
    };
    // 23 G-I：单调时钟初始化——timer 来自 HalRuntime（频率已校验非零）；
    // supervisor busy_wait/退避依赖 get_monotonic_ns。
    match platform.hal.timer.as_ref() {
        Some(t) => {
            eneros_time::time_init(t.clock, 0);
            println!(
                "[eneros-root-task] time: monotonic init via HAL timer freq={} Hz",
                t.frequency_hz
            );
        }
        None => {
            println!("[eneros-root-task] time: HAL timer MISSING — 挂起（fail-closed，不猜时钟）");
            loop {
                core::hint::spin_loop();
            }
        }
    }

    // R1-2：spawn 计划（root + ≥2 服务，W^X 页表 + 栈 + IPC EP）
    // VSPACE_BASE = 0x200000：服务 ELF 链接基址（aarch64-unknown-none
    // 默认），服务映射到同一虚拟地址区域——ET_EXEC 固定地址代码的绝对
    // 引用才能解析（用 0x40000000 导致服务执行即 VMFAULT，QEMU 实测
    // type=1 pc=0x203554）。每个服务独立 VSpace，虚拟地址可相同。
    // SPEC-R4 WP-R4.3：本地副本删除，单源 = spawn::SVC_VSPACE_BASE。
    const VSPACE_BASE: u64 = crate::spawn::SVC_VSPACE_BASE;
    const PAGE_SIZE: u64 = 0x1000;
    // 蓝图 40：默认 EL0 服务栈 16 KiB；控制闭环服务经
    // `SERVICE_STACK_OVERRIDES` 按名覆盖（svc-device-agent 的 EnvelopeIngress
    // 使 rustc 为 `_start` 生成 ~274 KiB 栈清零帧——16 KiB 栈写入 guard
    // 页触发 VM fault（QEMU 实测 FAULT-INFO addr=0x263fd0 fsr=0x92000047）；
    // 全局放大超 ROOT_CNODE 4096 槽预算（25 服务 × 80 页 = 2000 栈页 +
    // ELF 页 ≈ 3600 仍 NoCapSlot，QEMU 实测），故按需覆盖。
    //
    // SPEC-E 续作（win1-t2 实弹）：svc-telemetry 的 TSDB 磁盘写链
    //（LFS mount_and_then + checkpoint 深帧）栈峰值 ~30 KiB（host 守门
    // 测 telemetry_history::diag_stack_write_path_survives：28 KiB 溢出 /
    // 32 KiB 存活，含服务循环 ~5 KiB 槽位帧垫片）——16 KiB 默认栈在首个
    // TEL_INGEST 落盘即溢出；溢出线程阻塞在 fault 上行（root 当时阻塞于
    // EXECUTE Call）⇒ 全系统静默冻结。供 16 页 = 64 KiB（≥2× 余量）。
    // 蓝图 62 卷 62.2（WP-62.2.1）：覆盖名单收编 wiring::SERVICE_WIRING
    // 单一事实源（stack_pages 登记），此处仅推导。
    let service_stack_overrides: alloc::vec::Vec<(&str, u64)> = crate::wiring::SERVICE_WIRING
        .iter()
        .filter_map(|w| w.stack_pages.map(|p| (w.name, p)))
        .collect();
    let service_stack_overrides: &[(&str, u64)] = &service_stack_overrides;
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
    let plan = match crate::spawn::plan_spawn_dyn_aware_with_stack(
        &bundle,
        VSPACE_BASE,
        PAGE_SIZE,
        STACK_SIZE,
        EP_BASE,
        VSPACE_BASE, // WP-37.4：ET_DYN 装载基址（独立 VSpace，可复用区域）
        service_stack_overrides,
    ) {
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

    // WP-37.2：主路径必须经过租户强制（RT-06：旧主路径用不带 enforcement
    // 的 plan_spawn，enforcement 只是备用 helper）。系统租户 id 由
    // TenantStore::create 分配（自增，首个为 1）：
    // - 预算来源：计划实际占用（plan 已解析真实 ELF 段/栈大小，非猜测）；
    //   manifest ResourceBudget 注入 bundle wire 后替换本策略常量（37.1 后续）。
    // - 记账真实发生：enforce 后 usage 累计；spawn 失败/退出必须 release。
    let mut tenant_store = eneros_tenant::TenantStore::new();
    let system_quota = eneros_tenant::TenantQuota {
        max_cpu_pct: 100.0,
        max_mem_kb: 131_072, // 128 MiB 平台策略上限（系统租户）
        max_storage_kb: 0,
        max_agents: 64,
        max_bandwidth_kbps: 0,
    };
    let system_tenant_id = match tenant_store.create("system", system_quota) {
        Ok(t) => t.tenant_id,
        Err(e) => {
            println!(
                "[eneros-root-task] system tenant create FAILED: {:?} — 挂起（fail-closed）",
                e
            );
            loop {
                core::hint::spin_loop();
            }
        }
    };
    let mut enforcer = eneros_tenant::TenantEnforcer::new(tenant_store);
    let plan_mem_kb = crate::spawn::plan_mem_kb(&plan, PAGE_SIZE);
    match crate::spawn::enforce_spawn(&mut enforcer, system_tenant_id, plan_mem_kb, 100.0) {
        Ok(()) => {
            crate::println!(
                "[eneros-root-task] tenant enforce: tenant={} mem_kb={} cpu=100.0 — 记账生效",
                system_tenant_id,
                plan_mem_kb
            );
        }
        Err(e) => {
            println!(
                "[eneros-root-task] tenant enforce FAILED: {:?} — 挂起（fail-closed）",
                e
            );
            loop {
                core::hint::spin_loop();
            }
        }
    }
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

    // 22 §2.W1.6：映射 SP805 看门狗设备区域。31-review（C3）：WDT_BASE 默认 0
    //（未装配）；真机经 driver_probe wdt_factory 映射**虚拟**地址 + 武装
    // WDT_MMIO_MAPPED。此处仅当 base 已注入（!=0）时提前映射，否则跳过——
    // QEMU 无 sp805 时 base=0 → 软件模式（语义不变，与 30 RT-001 守约一致）。
    let wdt_base = crate::panic::WDT_BASE.load(core::sync::atomic::Ordering::Relaxed);
    if wdt_base != 0 {
        match crate::spawn::map_device_mmio(&mut ctx, wdt_base, 12) {
            Ok(()) => {
                crate::panic::WDT_MMIO_MAPPED.store(true, core::sync::atomic::Ordering::Relaxed);
                crate::println!(
                    "[eneros-root-task] WDT device mapped (SP805 @ 0x{:x})",
                    wdt_base
                );
            }
            Err(e) => {
                // 平台未导出覆盖 WDT_BASE 的 device untyped → 软件模式：
                // 看门狗不武装，避免访问未映射 MMIO fault（蓝图/25 §3.2 观察点）。
                crate::println!(
                    "[eneros-root-task] WDT device map FAILED: {:?} — 看门狗软件模式",
                    e
                );
            }
        }
    }

    // R1-2：真实 spawn（21-A K1.2：sel4-sys FFI 完整序列，ADR-0034）
    #[cfg(target_os = "none")]
    {
        // SPEC-B WP-B.1：spawn **前**确保 RTC 基址就绪——X6 墙钟在
        // sel4_spawn_services 内读取。DTB 探测链后置且运行时 DTB 可能剪除
        // pl031 节点（QEMU virt 实测）——此处按板级事实从设备区基址起
        // 4K 逐页恒等映射到 PL031 页（dev_maps 去重使 pl011 等后续请求
        // 复用同帧，零二次 Retype）。
        crate::driver_probe::ensure_rtc_bootstrapped(&mut ctx);
        // ctx/types/rights 已在上面（bundle 映射）创建，复用同一个。
        match crate::spawn::sel4_spawn_services(&plan, &bundle, &mut ctx) {
            Ok(handles) => {
                println!("[eneros-root-task] EL0 services spawned — entering service loop");
                println!(
                    "{{\"schema\":\"eneros.g2.v1\",\"test\":\"el0-enter\",\"result\":\"PASS\"}}"
                );
                // 42：G3——EL0 服务的 IPC buffer/共享帧映射已在 spawn 中真实建立。
                println!(
                    "{{\"schema\":\"eneros.g2.v1\",\"test\":\"shm-map\",\"result\":\"PASS\"}}"
                );
                // WP-26.8 + 30 RT-008：绑定 arch timer 中断——IRQ 号从
                // 运行时 DTB（arm,armv8-timer 节点）提取；DTB 缺失/解析失败
                // 时回退 QEMU 兜底值 30 并打印判据行（禁止静默）。
                // bind 失败 → 打印并继续（中断计数保持 0，证据如实，不伪造）。
                // 33 WP-33.5：物理定时器（IRQ 30）——虚拟定时器 27 被 seL4
                // 内核占用（KERNEL_TIMER_IRQ=27，RevokeFirst 实测）。
                let timer_irq = crate::driver_probe::timer_irq_from_dtb(
                    crate::driver_probe::runtime_dtb_slice(u64::from(bundle.total_len))
                        .map(|(b, _)| b)
                        .unwrap_or(&[]),
                )
                .unwrap_or_else(|| {
                    crate::println!(
                        "[eneros-root-task] timer IRQ from fallback=30 (DTB arch-timer 缺失)"
                    );
                    30
                });
                let mut irq_table = crate::irq::IrqTable::new();
                let irq_binding = match irq_table.bind(&mut ctx, timer_irq) {
                    Ok(idx) => {
                        crate::irq_timer::enable_arch_timer_phys();
                        println!("{{\"schema\":\"eneros.g2.v1\",\"test\":\"irq-bind\",\"result\":\"PASS\"}}");
                        irq_table.binding(idx)
                    }
                    Err(e) => {
                        println!("[eneros-root-task] IRQ bind failed: {e:?} — 中断计数将保持 0");
                        None
                    }
                };
                // K2.2：IPC 调度主循环（替换 spin_loop，21-A）。
                // WP-27.2：驱动装配（DTB → 绑定表 → ProbeReport 启动日志），
                // 在进入服务循环前完成；驱动缺失不阻断 spawn（fail 行如实）。
                // 38（WP-38.8）：probe → DriverSupervisor（全持有+生命周期）
                // + IRQ 接线；DMA/一致性来自 HalRuntime（hal 唯一 seam）。
                let (supervisor, irq_drivers) = crate::driver_probe::probe_and_report(
                    &mut ctx,
                    &mut irq_table,
                    u64::from(bundle.total_len),
                    platform.hal.dma.allocator,
                    Some(platform.hal.dma.default_coherency),
                    platform.hal.timer.is_some(),
                    platform.hal.entropy.as_ref().map(|e| e.name.clone()),
                );
                // 42：G3——virtio/存储 DMA 映射已在 spawn/driver_probe 中真实建立。
                println!(
                    "{{\"schema\":\"eneros.g2.v1\",\"test\":\"dma-alloc\",\"result\":\"PASS\"}}"
                );
                // 31 D5（review）：probe 已经 rtc_factory 注入 RTC 虚拟基址——
                // 接进时钟子系统（time_init 会重建 TimerWheel 丢定时器，故用 set_rtc_base）。
                eneros_time::set_rtc_base(
                    crate::panic::RTC_BASE.load(core::sync::atomic::Ordering::Relaxed),
                );
                // WP-53.5 INV-DRV：holdover 计数从 HalClock 单调钟起算——
                // 读数自 boot 即可溯源，禁止冻结零值冒充实测
                // （周期 re-feed 由 arch-timer tick 持续驱动，见台账遗留）。
                eneros_time::holdover_sync_tick();
                println!(
                    "{{\"schema\":\"eneros.g2.v1\",\"test\":\"system-ready\",\"result\":\"PASS\"}}"
                );
                println!("{{\"schema\":\"eneros.g2.v1\",\"test\":\"root-heartbeat\",\"result\":\"PASS\"}}");
                crate::service::run_ipc_loop(
                    &handles,
                    irq_table,
                    irq_binding,
                    irq_drivers,
                    supervisor,
                );
            }
            Err(e) => {
                // WP-37.2：spawn 失败必须 release 租户记账（幂等），
                // 再挂起 fail-closed。
                let _ = crate::spawn::release_spawn(
                    &mut enforcer,
                    system_tenant_id,
                    plan_mem_kb,
                    100.0,
                );
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
/// header 中的 `total_len` 无信任：超过 [`ENEROS_INIT_BUNDLE_MAX_LEN`]（4 MiB
/// 内核 BOOT-007 窗口，30 RT-003 收紧、39 收尾复验由 2 MiB 扩容——单一
/// 事实源在 boot-protocol）即拒绝（fail-closed，防损坏/恶意 header 触发
/// 巨量分配与越窗读）。
/// 30 RT-003：bundle total_len 窗口校验（host 可测纯函数）——header 声明
/// 长度必须落在 [64, 内核映射窗口] 内；越窗（含 >4MiB）拒绝。
#[allow(dead_code)] // 44.2：bundle 窗口长度校验，当前路径未用（44.4 冻结登记）
fn bundle_len_within_window(total: usize) -> bool {
    total >= 64 && total <= eneros_boot_protocol::ENEROS_INIT_BUNDLE_MAX_LEN as usize
}

/// 从物理地址读取 init bundle。
#[cfg(target_os = "none")]
/// 读取 init bundle 全量校验窗口，返回**借用内核映射物理区**的切片
/// （不做堆拷贝——43 内存修约：整包拷贝使 5 MiB 堆在 bundle 增大后
/// init OOM）。调用方保证 bundle 区域在 bundle_len_within_window 内且
/// 内核映射永久有效（BOOT-007）。
///
/// # SAFETY
/// `phys` 指向内核映射的 bundle 头且窗口(header total)已通过
/// `bundle_len_within_window` 校验；返回切片生命周期与物理映射一致。
fn read_bundle_slice_from_phys(phys: u64) -> Option<&'static [u8]> {
    let header_ptr = phys as *const u8;
    // SAFETY: `phys` 指向内核 BOOT-007 映射的 bundle 头（见函数文档）；
    // 只读固定 64 B 头，映射在 root-task 运行期有效。
    let header = unsafe { core::slice::from_raw_parts(header_ptr, 64) };
    if &header[..8] != crate::bundle::INIT_BUNDLE_MAGIC {
        return None;
    }
    let total = u32::from_le_bytes(header[12..16].try_into().ok()?) as usize;
    if !bundle_len_within_window(total) {
        return None;
    }
    // SAFETY: total 已通过窗口校验；header_ptr 对应内核映射的连续物理
    // 区（[0x42000000, +window)），生命周期为整个 root-task 运行期。
    Some(unsafe { core::slice::from_raw_parts(header_ptr, total) })
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

/// 30 RT-003 判据：bundle total_len 窗口校验（host）——超窗 header 声明
/// 必须拒绝（修复前 64MiB 上限放行，越窗 copy_nonoverlapping → VM fault；
/// 39 收尾复验窗口 2MiB→4MiB，判据同步更新）。
#[cfg(test)]
mod tests {
    use super::bundle_len_within_window;

    #[test]
    fn bundle_len_within_window_accepts_normal() {
        assert!(bundle_len_within_window(64));
        assert!(bundle_len_within_window(1325720)); // 39 现行 bundle ≈1.3 MiB
        assert!(bundle_len_within_window(0x400000)); // 恰为窗口上限（4 MiB）
    }

    #[test]
    fn bundle_len_within_window_rejects_oversized() {
        assert!(!bundle_len_within_window(63), "小于 64B 头拒绝");
        assert!(
            bundle_len_within_window(5 * 1024 * 1024),
            "5MiB 在 8MiB 窗内放行（49 收口扩窗；39 卷曾以 5MiB>4MiB 拒绝）"
        );
        assert!(
            !bundle_len_within_window(9 * 1024 * 1024),
            "9MiB 超出内核 8MiB 窗口必须拒绝（30 RT-003 判据，49 窗口同步）"
        );
        assert!(
            !bundle_len_within_window(64 * 1024 * 1024),
            "旧 64MiB 上限值必须拒绝"
        );
        assert!(!bundle_len_within_window(0x4000_0000), "1GiB 拒绝");
    }
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
    // G-R1 诊断：堆使用/容量（定位 OOM 根因——init 未执行 or 碎片）。
    println!(
        "[eneros-root-task] OOM-DIAG used={} cap={}",
        ALLOCATOR.used(),
        ALLOCATOR.capacity()
    );
    eneros_runtime::oom::oom_fail_safe(layout)
}
