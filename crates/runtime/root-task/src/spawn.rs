//! EL0 spawn 计划（蓝图/20 R1-2）：init bundle → 页表 + 权限 → spawn 参数。
//!
//! 本层是 host 可测的完整 spawn 计划层：
//! - root 模块（`MODULE_KIND_ROOT`）→ Root Task spawn 计划（物理页映射页表）；
//! - service/driver 模块 → ELF 解析（W^X 校验）→ 页表条目 + 入口 + 栈 + IPC EP；
//! - 服务不足 2 个 → `TooFewServices`（fail-closed，规格要求 ≥2：svc-fault /
//!   svc-proc-ts）。
//!
//! 真实 seL4 FFI（retype/TCB 寄存器写入/endpoint mint/resume）由
//! `sel4-sys` 在 M2.1 接入点提供（`#[cfg(target_os = "none")]` 的
//! `sel4_spawn_*` 汇编段），本层保证 spawn 前的地址/权限/端点计划正确。

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use eneros_elf_loader::LoadedElf;
// K1.2（21-A）：spawn 真实 FFI 封装（configure_service / copy_segment_via_window
// 等函数签名直接引用，模块级导入保证 cfg(target_os = "none") 路径一致）。
#[cfg(target_os = "none")]
use eneros_sel4_sys::safe::{
    arm_asid_pool_assign, arm_page_map, arm_page_table_map, arm_page_unmap, cnode_mint, retype,
    tcb_resume, tcb_set_ipcbuffer, tcb_set_priority, tcb_set_space, tcb_write_registers,
    ObjectTypeTable, RightsTable,
};
use eneros_sel4_sys::safe::{CapAllocator, Sel4Error, UntypedDesc};
#[cfg(target_os = "none")]
use eneros_sel4_sys::UserContext;

/// spawn 计划错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnError {
    /// bundle 中没有 root 模块。
    NoRootTask,
    /// bundle 中没有服务模块。
    NoServices,
    /// 服务模块不足 2 个（规格：svc-fault / svc-proc-ts 至少 2 个）。
    TooFewServices,
    /// ELF 解析失败。
    BadElf,
    /// 段违反 W^X（可写且可执行）。
    WxViolation,
    /// 租户强制力拒绝（超配/租户不存在，蓝图/20 R2-3）。
    QuotaExceeded,
    /// 虚拟地址空间越界。
    OutOfVaddrSpace,
    /// seL4 系统调用失败（K1.2 真实 spawn）。
    Sel4(Sel4Error),
    /// untyped 内存不足（对象分配失败）。
    NoUntyped,
    /// root CSpace 空闲槽不足。
    NoCapSlot,
    /// spawn 计划与 ELF 段不一致（如段 vaddr 非页对齐）。
    PlanMismatch,
    /// bundle 中找不到与计划匹配的服务模块。
    MissingBundleModule,
}

/// 页表条目（W^X 语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageEntry {
    /// 虚拟地址。
    pub vaddr: u64,
    /// 物理地址（由 boot 阶段填充；计划阶段为 0）。
    pub phys: u64,
    /// 映射大小（页对齐）。
    pub size: u64,
    /// 读权限。
    pub read: bool,
    /// 写权限。
    pub write: bool,
    /// 执行权限。
    pub exec: bool,
}

impl PageEntry {
    /// W^X 校验：可执行段不得可写。
    pub fn is_wx_violation(&self) -> bool {
        self.exec && self.write
    }
}

/// 单个服务的 spawn 计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnedService {
    /// 服务名（bundle manifest）。
    pub name: String,
    /// 入口虚拟地址（ELF entry）。
    pub entry: u64,
    /// 栈顶虚拟地址（栈向下增长）。
    pub stack_top: u64,
    /// 栈大小。
    pub stack_size: u64,
    /// IPC 端点标识（root-task ↔ 服务的通信通道）。
    pub ipc_ep: u64,
    /// 页表条目（W^X 保证）。
    pub pages: Vec<PageEntry>,
}

/// Root Task 的 spawn 计划精简视图（W4.3 蓝图23）。
///
/// root 由 elfloader 加载（不经 EL0 spawn），生产中仅 `name`（main.rs 打印）
/// 与 `stack_top`（服务 vaddr 布局起点）被使用——完整 `SpawnedService` 的
/// entry/pages/ipc_ep 从未被消费，降级为本结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnRoot {
    /// Root Task 名（bundle manifest）。
    pub name: String,
    /// 栈顶虚拟地址（= 服务 vaddr 布局起点）。
    pub stack_top: u64,
}

/// 全部 spawn 计划（root + ≥2 服务）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnPlan {
    /// Root Task 自身（精简视图）。
    pub root: SpawnRoot,
    /// EL0 服务（≥2）。
    pub services: Vec<SpawnedService>,
}

impl SpawnPlan {
    /// 服务计数（验收口径：≥2）。
    pub fn service_count(&self) -> usize {
        self.services.len()
    }
}

/// 向上对齐到页大小。
pub const fn align_up(v: u64, align: u64) -> u64 {
    (v + align - 1) & !(align - 1)
}

/// 为单个 ELF 生成 spawn 计划（页表 + 入口 + 栈 + EP）。
fn plan_one(
    name: &str,
    elf: &LoadedElf,
    vspace_base: u64,
    page_size: u64,
    stack_size: u64,
    ipc_ep: u64,
) -> Result<SpawnedService, SpawnError> {
    if stack_size % page_size != 0 {
        // 栈大小必须页对齐（K1.2 按页映射栈，非对齐会少映射栈底部分）。
        return Err(SpawnError::PlanMismatch);
    }
    let mut pages = Vec::new();
    // 服务映射到 ELF 链接地址（seg.vaddr）：ET_EXEC 固定地址代码的绝对
    // 引用（跳转/数据）指向链接地址，映射必须与链接一致。每个服务独立
    // VSpace，虚拟地址区间可相同（页表隔离）。用 vspace_base 连续分配
    // 会导致服务映射 ≠ 链接 → 执行即 VMFAULT（QEMU 实测 pc=0x203554）。
    let mut vaddr = vspace_base;
    for seg in elf.segments() {
        let flags = seg.flags;
        if flags.is_wx_violation() {
            return Err(SpawnError::WxViolation);
        }
        let size = align_up(seg.memsz.max(1), page_size);
        let mapped_vaddr = if seg.vaddr % page_size == 0 {
            seg.vaddr // 与链接地址一致（页对齐）
        } else {
            return Err(SpawnError::PlanMismatch);
        };
        pages.push(PageEntry {
            vaddr: mapped_vaddr,
            phys: 0,
            size,
            read: flags.read,
            write: flags.write,
            exec: flags.execute,
        });
        vaddr = mapped_vaddr
            .checked_add(size)
            .ok_or(SpawnError::OutOfVaddrSpace)?;
    }
    let stack_top = vaddr
        .checked_add(stack_size)
        .ok_or(SpawnError::OutOfVaddrSpace)?;
    Ok(SpawnedService {
        name: String::from(name),
        entry: elf.entry(),
        stack_top,
        stack_size,
        ipc_ep,
        pages,
    })
}

/// 从 init bundle 构造 spawn 计划（蓝图/20 R1-2 核心）。
///
/// # 参数
/// - `bundle`：init bundle（root + service/driver 模块）
/// - `vspace_base`：root 虚拟地址空间基址（如 0x4000_0000）
/// - `page_size`：页大小（4K）
/// - `stack_size`：每服务栈大小
/// - `ep_base`：IPC 端点编号基址（root=ep_base，服务依次 +1）
pub fn plan_spawn(
    bundle: &crate::bundle::InitBundle,
    vspace_base: u64,
    page_size: u64,
    stack_size: u64,
    ep_base: u64,
) -> Result<SpawnPlan, SpawnError> {
    // 1. Root Task 模块（物理页映射页表：boot.rs 提供 paddr 填充）
    let root_module = bundle
        .modules
        .iter()
        .find(|m| m.kind == crate::bundle::MODULE_KIND_ROOT)
        .ok_or(SpawnError::NoRootTask)?;
    let root_elf = LoadedElf::parse(&root_module.blob).map_err(|_| SpawnError::BadElf)?;
    if !root_elf.has_executable() {
        return Err(SpawnError::BadElf);
    }
    // root 由 elfloader 加载（不经 EL0 spawn）——只需 name + stack_top
    // （服务 vaddr 布局起点）。完整 plan_one 只为 EL0 服务保留。
    let root_plan = plan_one(
        &root_module.name,
        &root_elf,
        vspace_base,
        page_size,
        stack_size,
        ep_base,
    )?;
    let root = SpawnRoot {
        name: root_plan.name,
        stack_top: root_plan.stack_top,
    };

    // 2. EL0 服务（≥2，fail-closed）
    let mut services = Vec::new();
    let mut next_vaddr = root.stack_top;
    let mut ep = ep_base + 1;
    for module in &bundle.modules {
        if module.kind != crate::bundle::MODULE_KIND_SERVICE
            && module.kind != crate::bundle::MODULE_KIND_DRIVER
        {
            continue;
        }
        match LoadedElf::parse(&module.blob) {
            Ok(elf) => {
                let plan = plan_one(&module.name, &elf, next_vaddr, page_size, stack_size, ep)?;
                next_vaddr = plan.stack_top;
                ep += 1;
                services.push(plan);
            }
            Err(_) => {
                // W^X/解析失败的服务不进入 spawn 计划（fail-closed）
                continue;
            }
        }
    }
    if services.is_empty() {
        return Err(SpawnError::NoServices);
    }
    if services.len() < 2 {
        return Err(SpawnError::TooFewServices);
    }

    Ok(SpawnPlan { root, services })
}

/// 租户强制力校验（蓝图/20 R2-3）：spawn 计划就绪后、真实 spawn 前执行。
///
/// 通过 `Enforcer::check_spawn` 记账（超配 → 拒绝，usage 不增长）；
/// 调用方在 spawn 失败/退出时必须 `release_spawn` 扣减。
pub fn enforce_spawn<E: eneros_tenant::Enforcer>(
    enforcer: &mut E,
    tenant_id: u64,
    mem_kb: usize,
    cpu_pct: f32,
) -> Result<(), eneros_tenant::PlatformError> {
    enforcer.check_spawn(&eneros_tenant::SpawnReq {
        tenant_id,
        resource: eneros_tenant::ResourceReq { cpu_pct, mem_kb },
    })
}

/// 带租户强制力的 spawn 计划（蓝图/20 R2-3）：计划生成后、真实 spawn
/// 前执行 `Enforcer::check_spawn`——超配拒绝 → 计划不落地（fail-closed）。
#[allow(clippy::too_many_arguments)]
pub fn plan_spawn_with_enforcement<E: eneros_tenant::Enforcer>(
    enforcer: &mut E,
    bundle: &crate::bundle::InitBundle,
    vspace_base: u64,
    page_size: u64,
    stack_size: u64,
    ep_base: u64,
    tenant_id: u64,
    mem_kb: usize,
    cpu_pct: f32,
) -> Result<SpawnPlan, SpawnError> {
    let plan = plan_spawn(bundle, vspace_base, page_size, stack_size, ep_base)?;
    enforce_spawn(enforcer, tenant_id, mem_kb, cpu_pct).map_err(|_| SpawnError::QuotaExceeded)?;
    Ok(plan)
}

/// 释放一次 spawn 记账（幂等）。
pub fn release_spawn<E: eneros_tenant::Enforcer>(
    enforcer: &mut E,
    tenant_id: u64,
    mem_kb: usize,
    cpu_pct: f32,
) -> Result<(), eneros_tenant::PlatformError> {
    enforcer.release_spawn(tenant_id, mem_kb, cpu_pct)
}

// ============================================================================
// seL4 FFI 接入点（21-A K1.2：真实 spawn，ADR-0034）
// ============================================================================
//
// 真实 spawn 序列（内核源码证据修正版，见 ADR-0034）：
//   对 SpawnPlan 中每个 service：
//     1. retype untyped → TCB（size_bits=12）
//     2. retype untyped → VSpace（arm_vspace，size_bits=12）
//     3. retype untyped → CNode（size_bits=12，服务独立 CSpace）
//     4. asid_pool_assign（aarch64 VSpace 必须绑 ASID）
//     5. retype untyped → 3× PageTable（aarch64 4 级页表：L1/L2/L3）
//     6. 对每页（ELF 段页 + 栈页 + IPC buffer 页）：
//        a. retype untyped → SmallPage frame
//        b. arm_page_table_map ×3 → 服务 VSpace（0x4000_0000 区域）
//        c. ELF 数据页：arm_page_map → root 临时窗口 → 拷贝 → arm_page_unmap
//        d. arm_page_map → 服务 VSpace @ vaddr
//     7. tcb_set_space（CSpace/VSpace/IPC buffer 绑定）
//     8. cnode_mint：ipc_ep → 服务 CSpace 槽 0
//     9. tcb_write_registers：PC=entry_mapped, SP=stack_top, X0=ep, X1=buffer
//    10. tcb_set_priority（authority = root TCB）
//  全部服务配置完成后统一 tcb_resume（两阶段：中途失败时无任何服务在跑，
//  满足红线 4 fail-closed——比"边配置边 resume"更严格）。

/// K1.2 spawn 上下文：bootinfo 派生对象 + root-task 固定 cap（ADR-0034）。
#[derive(Debug)]
pub struct SpawnCtx {
    /// untyped 内存分配器（first-fit）。
    pub untyped: UntypedAllocator,
    /// root CSpace 空闲槽分配器（从 untyped cap 区间之后开始）。
    pub caps: CapAllocator,
    /// root-task 自身 TCB cap（tcb_set_priority 的 authority）。
    pub root_tcb: u64,
    /// root-task 自身 CSpace 根 cap（retype/mint 的 dest_root）。
    pub root_cnode: u64,
    /// root-task 自身 VSpace cap（方案 A 临时窗口映射）。
    pub root_vspace: u64,
    /// root-task 继承的 ASID pool cap。
    pub asid_pool: u64,
}

/// 从 bootinfo 解析的 untyped 描述列表构造 spawn 上下文。
///
/// host 构建下 untyped 列表为空（host 无 seL4 bootinfo）；目标构建由
/// `SpawnCtx::from_bootinfo_reader` 填充。
impl SpawnCtx {
    /// 目标构建：从 BootInfoReader + CapInitThreadTable 构造。
    #[cfg(target_os = "none")]
    pub fn from_bootinfo_reader(
        reader: &eneros_sel4_sys::safe::BootInfoReader,
        caps: &eneros_sel4_sys::safe::CapInitThreadTable,
    ) -> Self {
        let mut descs = Vec::new();
        for i in 0..reader.untyped_count() {
            if let Some(d) = reader.untyped_at(i) {
                crate::println!(
                    "[eneros-root-task] DBG untyped[{}] cap={} paddr=0x{:x} size_bits={} dev={}",
                    i,
                    reader.untyped_start() + i,
                    d.paddr,
                    d.size_bits,
                    d.is_device
                );
                descs.push(d);
            }
        }
        // 空闲槽从 untyped cap 区间之后开始（untyped caps 占据
        // [untyped_start, untyped_end) 槽位，不可复用）。
        crate::println!(
            "[eneros-root-task] DBG untyped_start={} untyped_end={} count={}",
            reader.untyped_start(),
            reader.untyped_end(),
            reader.untyped_count()
        );
        let slots = CapAllocator::new(reader.untyped_end(), ROOT_CNODE_SLOTS);
        Self {
            untyped: UntypedAllocator::new(reader.untyped_start(), descs),
            caps: slots,
            root_tcb: caps.tcb,
            root_cnode: caps.cnode,
            root_vspace: caps.vspace,
            asid_pool: caps.asid_pool,
        }
    }
}

/// root CSpace 容量上限（seL4 14.0.0 aarch64 默认 `seL4_CNodeSizeBits` = 12）。
pub const ROOT_CNODE_SLOTS: u64 = 1 << 12;

/// seL4 对象大小（aarch64 14.0.0，单位：log2 字节）。
///
/// 与 `libsel4/sel4_arch_include/aarch64/sel4/sel4_arch/constants.h` 对齐：
/// - `seL4_TCBBits`：debug build（CONFIG_HARDWARE_DEBUG_API）=12（4 KiB）
/// - `seL4_VSpaceBits`：非 HYP + PA 非 40 =12（4 KiB）
/// - CapTable：`2^(size_bits + seL4_SlotBits)`，`seL4_SlotBits=5`（每槽 32 B）
///   → `OBJ_BITS_CNODE=7` = 128 槽 × 32 B = 4 KiB（服务 CSpace 只需少量槽：
///   endpoint + ipcbuf；21-A 用 12 导致 2^17=128 KiB CapTable，超出
///   untyped 剩余（QEMU 实测 `Insufficient memory (1 * 131072)`））
/// - `seL4_PageTableBits`=12、`seL4_PageBits`=12、`seL4_EndpointBits`=4
/// - `seL4_NotificationBits`=5（非 MCS；21-A 用 4 导致 16 B 对象实际 32 B）
pub const OBJ_BITS_TCB: u64 = 12; // 4 KiB（debug build）
pub const OBJ_BITS_VSPACE: u64 = 12; // 4 KiB（L0 表）
pub const OBJ_BITS_CNODE: u64 = 7; // 128 槽 × 32 B = 4 KiB（seL4_SlotBits=5）
                                   // 服务 CSpace 的 guard_size：内核 lookupCap 总是用 wordBits=64 解析 cptr，
                                   // 一阶 cnode 需 guard_size = wordBits - radix 才能把 cptr 低 7 位映射为槽
                                   // （radix=OBJ_BITS_CNODE=7 → guard_size=57；CapData word = guard<<6 | guardSize）。
pub const SVC_CSPACE_CAPDATA: u64 = 64 - OBJ_BITS_CNODE; // = 57
pub const OBJ_BITS_PAGE_TABLE: u64 = 12; // 4 KiB（L1/L2/L3 通用）
pub const OBJ_BITS_SMALL_PAGE: u64 = 12; // 4 KiB
pub const OBJ_BITS_ENDPOINT: u64 = 4; // 16 B
pub const OBJ_BITS_NOTIFICATION: u64 = 5; // 32 B（非 MCS）
pub const PAGE_SIZE: u64 = 1 << OBJ_BITS_SMALL_PAGE;

/// 方案 A 临时窗口基址（root-task 自身 VSpace，1 GiB 区域；与 root 镜像
/// 0x4000_0000 区域不冲突；L0/L1 页表由 elfloader 提供，spawn 补 L2/L3）。
pub const WINDOW_BASE: u64 = 0x8000_0000;

/// 服务地址空间基址（与 main.rs VSPACE_BASE 一致）。
///
/// 0x200000 = 服务 ELF 链接基址（aarch64-unknown-none 默认）：ET_EXEC
/// 固定地址代码的绝对引用（跳转/数据）指向链接地址，映射必须与链接一致，
/// 否则服务执行即 VMFAULT（QEMU 实测 type=1 pc=0x203554）。每个服务
/// 独立 VSpace，虚拟地址区间可与其他服务/root-task 相同（页表隔离）。
pub const SVC_VSPACE_BASE: u64 = 0x200000;

/// 服务静态优先级（seL4 静态优先级调度，低于 root-task）。
pub const SVC_PRIORITY: u64 = 10;

/// untyped 分配器：从 bootinfo untyped 列表按 size_bits first-fit 切对象。
///
/// 每个 untyped 维护已切对象数 `used[i]`；`alloc` 返回
/// `(untyped_cap, offset)`——`offset` 是 untyped 内对象序号（retype 的
/// `node_offset` 参数）。
#[derive(Debug, Clone)]
pub struct UntypedAllocator {
    /// untyped cap 区间起点（cap 槽 = cap_base + 索引）。
    cap_base: u64,
    /// 过滤掉 device 内存后的 untyped 描述。
    descs: Vec<UntypedDesc>,
    /// 每 untyped 已分配字节偏移（镜像 seL4 freeIndex 语义：按对象大小
    /// 对齐递增；21-A 用「对象数」导致混合 size 对象时容量检查错误——
    /// QEMU 实测 `Insufficient memory (1 * 16 bytes needed, 0 bytes
    /// available)`：64 KiB untyped 分配 16 个 4 KiB 帧后，16 B endpoint
    /// 的检查 `16 << 4 = 256 < 65536` 误判可分配）。
    used: Vec<u64>,
}

impl UntypedAllocator {
    /// 从 untyped 描述列表构造（`cap_base` = bootinfo `untyped.start`）。
    pub fn new(cap_base: u64, descs: Vec<UntypedDesc>) -> Self {
        let used = alloc::vec![0u64; descs.len()];
        Self {
            cap_base,
            descs,
            used,
        }
    }

    /// 分配覆盖指定物理地址的 **device** 对象（FIX-BUGS-0810 B2 两件套之二）。
    ///
    /// [`alloc_device`]（first-fit）**不校验物理地址**——MMIO 映射若用它，
    /// 修好 node_offset 后会把"别的设备的物理页"映射到请求的虚拟地址
    /// （静默乱写，比软件模式更糟）。本函数按物理定位 device untyped
    /// （镜像 [`alloc_at`] 的覆盖检查结构，条件反转为 `is_device` 必须为真），
    /// 返回 `(untyped_cap, byte_offset)`；找不到覆盖 `phys` 的 device untyped
    /// 返回 `None`（调用方 fail-closed 走软件模式兜底）。
    pub fn alloc_device_at(&mut self, phys: u64, size_bits: u64) -> Option<(u64, u64)> {
        for i in 0..self.descs.len() {
            let desc = self.descs[i];
            if !desc.is_device || desc.size_bits < size_bits {
                continue;
            }
            if desc.size_bits >= u64::BITS as u64 {
                continue; // 防移位溢出（红线 6）
            }
            let obj_size = 1u64 << size_bits;
            let aligned = phys & !(obj_size - 1);
            let desc_end = desc.paddr.checked_add(1u64 << desc.size_bits)?;
            if aligned >= desc.paddr && aligned < desc_end {
                let byte_off = aligned - desc.paddr;
                // 标记已用（字节偏移，与 freeIndex 一致）。
                if self.used[i] < byte_off + obj_size {
                    self.used[i] = byte_off + obj_size;
                }
                return Some((self.cap_base + i as u64, byte_off));
            }
        }
        None
    }

    /// 分配一个 `size_bits` 对象，返回 `(untyped_cap, byte_offset)`；不足返回 `None`。
    ///
    /// 记账镜像 seL4 `decodeUntypedInvocation`：freeIndex 先按对象大小对齐
    /// （`alignUp(freeRef, objectSize)`）再递增。简单累加会在对象大小不是
    /// `seL4_MinUntypedBits` 粒度对齐时与 seL4 freeIndex 脱节（QEMU 实测：
    /// 32 B notification 在 80 B 处对齐到 96 B，Rust 记 112 而 seL4 记 128，
    /// 之后 endpoint `Insufficient memory (16 bytes needed, 0 available)`）。
    pub fn alloc(&mut self, size_bits: u64) -> Option<(u64, u64)> {
        for i in 0..self.descs.len() {
            let desc = self.descs[i];
            if desc.is_device || desc.size_bits < size_bits {
                continue;
            }
            if desc.size_bits >= u64::BITS as u64 {
                continue; // 超 64 位的对象不存在；防移位溢出（红线 6）
            }
            let capacity = 1u64 << desc.size_bits;
            let obj_size = 1u64 << size_bits;
            let used = self.used[i];
            // seL4 freeIndex 语义：alignUp(used, obj_size) + obj_size。
            let aligned = used
                .checked_add(obj_size - 1)
                .map(|v| v & !(obj_size - 1))?;
            if aligned
                .checked_add(obj_size)
                .is_some_and(|end| end <= capacity)
            {
                self.used[i] = aligned + obj_size;
                return Some((self.cap_base + i as u64, aligned));
            }
        }
        None
    }

    /// 分配一个 CapTable（CNode）对象：seL4 的 CNode 大小是
    /// `2^(size_bits + seL4_SlotBits)`（每槽 `2^SlotBits` 字节），与
    /// [`alloc`](Self::alloc) 的 `2^size_bits` 记账不同——混用会导致
    /// freeIndex 与 Rust 记账脱节（QEMU 实测 endpoint 的
    /// `Insufficient memory (1 * 16 bytes needed, 0 bytes available)`：
    /// CNode 实际占 4 KiB 但 Rust 只记 128 B，之后 3968 B 被 seL4 判满）。
    pub fn alloc_cap_table(&mut self, size_bits: u64) -> Option<(u64, u64)> {
        for i in 0..self.descs.len() {
            let desc = self.descs[i];
            if desc.is_device || desc.size_bits < size_bits {
                continue;
            }
            if desc.size_bits >= u64::BITS as u64 {
                continue;
            }
            // FIX-BUGS-0810 B4：CNode 槽数 = 2^size_bits、每槽 2^5 字节，
            // 故 size_bits + 5 必须 < 64，否则移位按 64 取模回绕（release
            // 得到错误 obj_size，debug 下 panic）——红线 6 守卫。
            if size_bits.checked_add(5).map_or(true, |b| b >= 64) {
                return None;
            }
            let capacity = 1u64 << desc.size_bits;
            // CNode 实际大小：槽数 × 每槽字节 = 2^size_bits × 2^SlotBits。
            let obj_size = (1u64 << size_bits) << 5; // seL4_SlotBits = 5
            let used = self.used[i];
            // FIX-BUGS-0810 B4：与 [`alloc`] 完全一致的 alignUp 记账
            // （seL4 freeIndex 按对象大小对齐）——CNode 记账同样必须对齐，
            // 否则与内核 freeIndex 脱节（QEMU 实测事故见 [`alloc`] 注释）。
            let aligned = used
                .checked_add(obj_size - 1)
                .map(|v| v & !(obj_size - 1))?;
            if aligned
                .checked_add(obj_size)
                .is_some_and(|end| end <= capacity)
            {
                self.used[i] = aligned + obj_size;
                return Some((self.cap_base + i as u64, aligned));
            }
        }
        None
    }

    /// 分配覆盖指定物理地址的对象（G-KERNEL BOOT-007：init_bundle 的物理页）。
    ///
    /// first-fit [`alloc`](Self::alloc) 不指定物理；bundle 由 boot-manager 加载
    /// 到固定物理地址（`INIT_BUNDLE_PHYS`），必须按物理定位 untyped。
    /// 返回 `(untyped_cap, byte_offset)`；找不到覆盖该物理的 untyped 返回 `None`。
    pub fn alloc_at(&mut self, phys: u64, size_bits: u64) -> Option<(u64, u64)> {
        for i in 0..self.descs.len() {
            let desc = self.descs[i];
            if desc.is_device || desc.size_bits < size_bits {
                continue;
            }
            let obj_size = 1u64 << size_bits;
            let aligned = phys & !(obj_size - 1);
            let desc_end = desc.paddr.checked_add(1u64 << desc.size_bits)?;
            if aligned >= desc.paddr && aligned < desc_end {
                let byte_off = aligned - desc.paddr;
                // 标记已用（字节偏移，与 freeIndex 一致）。
                if self.used[i] < byte_off + obj_size {
                    self.used[i] = byte_off + obj_size;
                }
                return Some((self.cap_base + i as u64, byte_off));
            }
        }
        None
    }

    /// 已分配字节总数（host 测试统计用）。
    pub fn allocated(&self) -> u64 {
        self.used.iter().sum()
    }
}

/// 段权限 → seL4 CapRights（R/W；执行映射由默认 VMAttributes 保证，
/// aarch64 无独立 X cap 位）。
pub fn page_rights(read: bool, write: bool, rights: &eneros_sel4_sys::safe::RightsTable) -> u64 {
    let mut r = rights.none;
    if read {
        r |= rights.read;
    }
    if write {
        r |= rights.write;
    }
    r
}

/// 计算 entry 映射后的实际虚拟地址。
///
/// `plan_one` 把 ELF 段顺序放入连续页（`pages[i]` 对应 `elf.segments()[i]`，
/// 一一对应）。entry 所在段的映射 vaddr + entry 段内偏移即为其实际地址。
/// 要求段 vaddr 页对齐（链接器默认；否则 `PlanMismatch` fail-closed）。
pub fn entry_mapped(plan_pages: &[PageEntry], elf: &LoadedElf) -> Result<u64, SpawnError> {
    let entry = elf.entry();
    let segments = elf.segments();
    if plan_pages.len() != segments.len() {
        return Err(SpawnError::PlanMismatch);
    }
    for (i, seg) in segments.iter().enumerate() {
        if seg.flags.execute && entry >= seg.vaddr && entry - seg.vaddr < seg.memsz {
            if seg.vaddr % PAGE_SIZE != 0 {
                return Err(SpawnError::PlanMismatch);
            }
            let seg_start = plan_pages[i].vaddr;
            return Ok(seg_start + (entry - seg.vaddr));
        }
    }
    Err(SpawnError::PlanMismatch)
}

/// 已 spawn 服务句柄（K2.2：root-task IPC 主循环的服务表）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnedHandle {
    /// 服务名（bundle manifest）。
    pub name: String,
    /// 服务 TCB cap（root CSpace 槽）。
    pub tcb: u64,
    /// 服务 endpoint cap（root CSpace 槽；root 侧发 IPC 用）。
    pub ep: u64,
    /// 服务故障 handler endpoint（root CSpace 槽；服务异常时内核向服务
    /// TCB 的 fault_ep（=服务 CSpace 槽 1 的 mint 副本）发 fault IPC，
    /// root-task 在此 ep 上 recv 检测故障隔离）。
    pub ntfn: u64,
    /// 22 §2.W1.3：重启寄存器参数——ELF 入口（PC）。
    pub entry: u64,
    /// 22 §2.W1.3：重启寄存器参数——栈顶（SP）。
    pub stack_top: u64,
    /// 22 §2.W1.3：重启寄存器参数——IPC buffer 虚拟地址（X1）。
    pub buffer_vaddr: u64,
}

/// 目标端真实 spawn：按计划逐服务执行 seL4 FFI（21-A K1.2，ADR-0034）。
///
/// 两阶段（红线 4 fail-closed 强化）：阶段一完成全部服务的对象创建与配置，
/// 阶段二统一 `tcb_resume`——中途任何一步失败时没有任何服务在运行。
/// 任一步失败返回 `Err`，由调用方挂起（fail-closed）。
///
/// 返回已 spawn 服务句柄表（name/tcb/ep cap），供 root-task IPC 主循环
/// （K2.2）按名寻址服务。
///
/// host 构建下不存在（真实 FFI 在目标验证环境执行）。

#[cfg(target_os = "none")]
pub fn sel4_spawn_services(
    plan: &SpawnPlan,
    bundle: &crate::bundle::InitBundle,
    ctx: &mut SpawnCtx,
) -> Result<Vec<SpawnedHandle>, SpawnError> {
    let types = ObjectTypeTable::resolve();
    let rights = RightsTable::resolve();
    crate::println!(
        "[eneros-root-task] DBG types tcb={} vspace={} cnode={} pt={} frame={} ep={} ntfn={}",
        types.tcb,
        types.arm_vspace,
        types.captable,
        types.arm_page_table,
        types.arm_frame,
        types.endpoint,
        types.notification
    );

    // 阶段 0：root 临时窗口页表（L2/L3；L0/L1 由 elfloader 提供）。
    let window_pt = retype_objects(ctx, &types, OBJ_BITS_PAGE_TABLE, 2)?;
    // SAFETY: 前置条件由 spawn 序列保证（window_pt 为刚 retype 的页表、
    // root_vspace 有效、WINDOW_BASE 1 GiB 对齐）；默认 attr 由封装解析。
    arm_page_table_map(ctx.root_vspace, window_pt[0], WINDOW_BASE).map_err(SpawnError::Sel4)?;
    arm_page_table_map(ctx.root_vspace, window_pt[1], WINDOW_BASE).map_err(SpawnError::Sel4)?;

    // 阶段一：全部服务配置（对象创建 + 映射 + 寄存器 + 优先级）。
    // K2.3：root 侧 fault handler endpoint——每个服务的 TCB fault_ep 指向
    // 同一 endpoint 的 mint 副本（服务 CSpace 槽 1），root-task 在此 ep 上
    // recv 所有服务的 fault 通知。
    let fault_ep = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
    retype_one(ctx, types.endpoint, OBJ_BITS_ENDPOINT, fault_ep)?;
    crate::println!("[eneros-root-task] DBG fault_ep={}", fault_ep);
    let mut handles = Vec::with_capacity(plan.services.len());
    let mut tcb_slots = Vec::with_capacity(plan.services.len());
    for (svc_idx, svc) in plan.services.iter().enumerate() {
        let module = bundle
            .modules
            .iter()
            .find(|m| m.name == svc.name)
            .ok_or(SpawnError::MissingBundleModule)?;
        let elf = LoadedElf::parse(&module.blob).map_err(|_| SpawnError::BadElf)?;
        let entry = entry_mapped(&svc.pages, &elf)?;
        let (tcb, ep, root_fault_ep) = configure_service(
            svc,
            &elf,
            module,
            entry,
            ctx,
            &types,
            &rights,
            fault_ep,
            svc_idx as u64 + 1, // fault_ep badge（0 保留；supervisor 识别服务用）
        )?;
        tcb_slots.push(tcb);
        handles.push(SpawnedHandle {
            name: svc.name.clone(),
            tcb,
            ep,
            // K2.3 fault 检测：root 侧带 badge 的 fault_ep 副本（G-R1 修复，
            // 23 G-I 总出口）——fault IPC 走 fault_ep endpoint，接收方 badge
            // = 持有 cap 的 badge；root 在此 poll 得 sender=badge 识别服务
            // （1368180 的 notification badge 方案错位，QEMU 实测 sender=0）。
            ntfn: root_fault_ep,
            // 22 §2.W1.3：重启寄存器参数（entry 由 ELF 解析；buffer 紧邻
            // 栈顶上方，与 configure_service 的 buffer_vaddr 计算一致）。
            entry,
            stack_top: svc.stack_top,
            buffer_vaddr: align_up(svc.stack_top, PAGE_SIZE),
        });
    }

    // 阶段二：统一 resume。
    for tcb in &tcb_slots {
        // SAFETY: tcb 为刚 retype 且已配置地址空间的 TCB cap。
        tcb_resume(*tcb).map_err(SpawnError::Sel4)?;
    }
    Ok(handles)
}

/// 构造 EL0 服务初始寄存器上下文（W2.1 不变量单一持有）。
///
/// seL4 aarch64 `UserContext` 布局：`pc(0) sp(1) spsr(2) x0(3) x1(4)`，需以
/// `count=5` 写入 `tcb_write_registers`。这是 spawn（`configure_service`）与
/// restart（`restart_service`）共享的唯一构造点——寄存器布局知识不再分散在两处
/// 靠注释同步。
///
/// - `entry`：ELF 入口（PC）。
/// - `stack_top`：栈顶（SP）。
/// - `buffer_vaddr`：IPC buffer 虚拟地址（X1；X0 固定为 0 = 服务 CSpace 槽 0 的 ep cap）。
#[cfg(target_os = "none")]
pub(crate) fn initial_registers(entry: u64, stack_top: u64, buffer_vaddr: u64) -> UserContext {
    let mut regs = UserContext::zeroed();
    regs.set_pc(entry);
    regs.set_sp(stack_top);
    regs.set_x(0, 0); // ep cap 在服务 CSpace 槽 0
    regs.set_x(1, buffer_vaddr);
    regs
}

/// 配置单个服务（阶段一）：对象 retype / ASID / 页表 / 页映射与拷贝 /
/// CSpace mint / 寄存器 / 优先级。返回 `(TCB cap, endpoint cap, fault ep cap)`。
///
/// # 返回值
/// 元组：服务 TCB / 服务 endpoint / root 侧 fault handler ep（同一
/// endpoint 对象的 root 视角 cap；服务侧在 CSpace 槽 1 持 mint 副本）。
#[cfg(target_os = "none")]
#[allow(clippy::too_many_arguments)]
fn configure_service(
    svc: &SpawnedService,
    elf: &LoadedElf,
    module: &crate::bundle::BundleModule,
    entry: u64,
    ctx: &mut SpawnCtx,
    types: &ObjectTypeTable,
    rights: &RightsTable,
    fault_ep: u64,
    ntfn_badge: u64,
) -> Result<(u64, u64, u64), SpawnError> {
    let root_cnode = ctx.root_cnode;

    // 1-3. TCB / VSpace / CNode retype。
    let tcb = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
    retype_one(ctx, types.tcb, OBJ_BITS_TCB, tcb)?;
    let vspace = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
    retype_one(ctx, types.arm_vspace, OBJ_BITS_VSPACE, vspace)?;
    let cnode = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
    retype_one(ctx, types.captable, OBJ_BITS_CNODE, cnode)?;

    // 4. ASID 分配（aarch64 VSpace 必须绑 ASID 才能映射页）。
    // SAFETY: vspace 为刚 retype 的 VSpace；asid_pool 为 root 继承的有效 pool。
    crate::println!(
        "[eneros-root-task] DBG svc '{}' tcb={} vspace={} cnode={} tcb_type={} vspace_type={} cnode_type={}",
        svc.name,
        tcb,
        vspace,
        cnode,
        eneros_sel4_sys::safe::debug_cap_identify(tcb),
        eneros_sel4_sys::safe::debug_cap_identify(vspace),
        eneros_sel4_sys::safe::debug_cap_identify(cnode)
    );
    arm_asid_pool_assign(ctx.asid_pool, vspace).map_err(SpawnError::Sel4)?;

    // 5. 服务 VSpace 页表 ×3（L1/L2/L3，覆盖 SVC_VSPACE_BASE 区域）。
    let pt = retype_objects(ctx, types, OBJ_BITS_PAGE_TABLE, 3)?;
    for pt_cap in &pt {
        // SAFETY: pt_cap 为刚 retype 的页表；vspace 已绑 ASID；
        // SVC_VSPACE_BASE 1 GiB 对齐（各层级 lookup 槽 0）。
        arm_page_table_map(vspace, *pt_cap, SVC_VSPACE_BASE).map_err(SpawnError::Sel4)?;
    }

    // 6a. ELF 段页（pages[i] ↔ segments[i] 一一对应，plan_one 保证）。
    // frame 槽无需跟踪：映射即用，服务侧不持有帧 cap。
    let segments = elf.segments();
    if segments.len() != svc.pages.len() {
        return Err(SpawnError::PlanMismatch);
    }
    for (i, page) in svc.pages.iter().enumerate() {
        let seg = &segments[i];
        if seg.vaddr % PAGE_SIZE != 0 {
            return Err(SpawnError::PlanMismatch);
        }
        // 页权限：W^X 已在计划层保证（exec 段不可写）。
        let r = page_rights(page.read, page.write, rights);
        // 段可能跨多页（memsz > 4 KiB）：每页独立 frame，逐页映射 + 分页
        // 拷贝（review 阻塞修复：禁止单 frame 承载多页段）。
        let pages_needed = align_up(seg.memsz.max(1), PAGE_SIZE) / PAGE_SIZE;
        for k in 0..pages_needed {
            let frame = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
            retype_one(ctx, types.arm_frame, OBJ_BITS_SMALL_PAGE, frame)?;
            let vaddr = page.vaddr + k * PAGE_SIZE;
            // 6c. 方案 A：本页内的文件数据经 root 临时窗口拷贝（ADR-0034）。
            let copy_start = k * PAGE_SIZE;
            let chunk = seg.filesz.saturating_sub(copy_start).min(PAGE_SIZE);
            if chunk > 0 {
                copy_segment_via_window(
                    ctx,
                    frame,
                    module,
                    seg.offset + copy_start,
                    chunk as usize,
                    rights,
                )?;
            }
            // 6d. 映射进服务 VSpace（BSS 部分由 retype 清零保证）。
            // SAFETY: frame 已 retype 且（如有文件数据）已解除窗口映射；
            // vspace 已绑 ASID；vaddr 4 KiB 对齐。
            crate::println!(
                "[eneros-root-task] DBG page_map vspace={} frame={} vaddr=0x{:x} svc='{}' frame_type={}",
                vspace,
                frame,
                vaddr,
                svc.name,
                eneros_sel4_sys::safe::debug_cap_identify(frame)
            );
            arm_page_map(vspace, frame, vaddr, r).map_err(SpawnError::Sel4)?;
        }
    }

    // 栈页（stack_size 字节，R+W；stack_top 向下增长）。
    let stack_pages = svc.stack_size / PAGE_SIZE;
    for k in 0..stack_pages {
        let frame = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
        retype_one(ctx, types.arm_frame, OBJ_BITS_SMALL_PAGE, frame)?;
        let vaddr = svc.stack_top - PAGE_SIZE * (k + 1);
        // SAFETY: frame 刚 retype（全零，无需拷贝）；vaddr 页对齐。
        crate::println!(
            "[eneros-root-task] DBG stack_map vspace={} frame={} vaddr=0x{:x} svc='{}'",
            vspace,
            frame,
            vaddr,
            svc.name
        );
        arm_page_map(vspace, frame, vaddr, rights.read | rights.write).map_err(SpawnError::Sel4)?;
    }

    // IPC buffer 页（栈顶之后，4 KiB 对齐；K2 消息 > 寄存器 MR 时必需）。
    let buffer_vaddr = align_up(svc.stack_top, PAGE_SIZE);
    let buffer = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
    retype_one(ctx, types.arm_frame, OBJ_BITS_SMALL_PAGE, buffer)?;
    // SAFETY: buffer 刚 retype（全零）；vaddr 页对齐。
    crate::println!(
        "[eneros-root-task] DBG buffer_map vspace={} frame={} vaddr=0x{:x} svc='{}' frame_type={}",
        vspace,
        buffer,
        buffer_vaddr,
        svc.name,
        eneros_sel4_sys::safe::debug_cap_identify(buffer)
    );
    arm_page_map(vspace, buffer, buffer_vaddr, rights.read | rights.write)
        .map_err(SpawnError::Sel4)?;

    // 6b 收尾：服务 VSpace 的 IPC buffer 绑定（seL4 14.0.0 API：SetSpace
    // 只绑 CSpace/VSpace + fault_ep，buffer 经 SetIPCBuffer 单独绑定）。
    // K2.3：fault_ep = 服务 CSpace 槽 1（root 的 fault handler ep 的 mint
    // 副本，见 8c）——服务异常时内核向该 endpoint 发 fault IPC（seL4 的
    // fault 通知走 tcbFaultHandler endpoint，非 notification；用 ntfn 等
    // fault 会 `handleNoFaultHandler` → restart 无限循环，QEMU 实测
    // DBG-SCHED prio=10 刷屏且隔离通知永不达）。
    //
    // cspace_root_data：服务 CSpace 必须带 guard_size=wordBits-radix=57
    // 才能被内核 lookupCap（wordBits=64 解析）正确寻址。服务 CSpace 是
    // 一阶 cnode（radix=7, guard=0, guard_size=0）时，cptr=1 的 offset =
    // (1 >> (64-7)) & 127 = 0 → 落到槽 0（服务 ep），fault IPC 发到服务
    // 自己的 ep（QEMU 实测 DBG-FAULTIP ep=0x238f40 = svc-fault 服务 ep，
    // root 在 fault_ep 上永久阻塞）。guard_size=57 → levelBits=64，offset
    // = cptr & 127，槽 0/1 均正确。CapData word = guard<<6 | guard_size
    // （libsel4 生成布局），guard=0 → 57。
    tcb_set_space(tcb, 1, cnode, SVC_CSPACE_CAPDATA, vspace, 0).map_err(SpawnError::Sel4)?;
    // SAFETY: buffer 为刚映射到 buffer_vaddr 的帧 cap（见 6a）。
    tcb_set_ipcbuffer(tcb, buffer, buffer_vaddr).map_err(SpawnError::Sel4)?;

    // 8. endpoint：retype + mint 到服务 CSpace 槽 0。服务侧 cptr 0 即其
    // 唯一 endpoint（服务 CSpace 仅此一槽，非 root 侧的 CapNull 语义）。
    let ep = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
    retype_one(ctx, types.endpoint, OBJ_BITS_ENDPOINT, ep)?;
    // SAFETY: cnode 为服务 CSpace（一阶，dest_depth=0）；槽 0 空闲；
    // rights 使用全部权限（服务经 ep 与 root 通信）；badge=0（无徽章）。
    // dest_depth 必须 ≥1 且 = 服务 CSpace 槽位数（OBJ_BITS_CNODE=7：
    // lookupSlotForCNodeOp 拒绝 depth<1，QEMU 实测 `CNode operation:
    // Target slot invalid`）；src_depth = wordBits（64）：root CNode 的
    // guard size = wordBits - CONFIG_ROOT_CNODE_SIZE_BITS = 52（boot.c
    // create_root_cnode），`resolveAddressBits` 要求 n_bits = radix+guard
    // = 64 才能 guard 匹配且 bitsRemaining=0——用 12/17 均 depth mismatch
    // → `Invalid source slot`（QEMU 实测 FailedLookup）。
    cnode_mint(cnode, 0, OBJ_BITS_CNODE, root_cnode, ep, 64, rights.all, 0)
        .map_err(SpawnError::Sel4)?;

    // 8c. K2.3：fault handler endpoint mint 到服务 CSpace 槽 1——服务
    // TCB 的 fault_ep（SetSpace 参数）在该槽解析；服务异常时内核向此
    // endpoint 发 fault IPC，root-task 在 root 侧副本（fault_ep cap）上
    // recv 检测故障。mint 用全部权限（服务侧持有可回复的 ep 副本）。
    //
    // 23 G-I 总出口修复：fault IPC 的 sender = fault_ep 的 badge（seL4
    // doFaultTransfer 把 badge 写入接收方 badge 寄存器）——每服务 mint 时
    // 设 badge = 服务索引+1，root-task poll 得 sender 即可识别 faulting 服务
    // （1368180 想实现此机制但错放在 notification 上，QEMU 实测 sender=0）。
    cnode_mint(
        cnode,
        1,
        OBJ_BITS_CNODE,
        root_cnode,
        fault_ep,
        64,
        rights.all,
        ntfn_badge,
    )
    .map_err(SpawnError::Sel4)?;
    crate::println!(
        "[eneros-root-task] DBG fault_mint svc='{}' fault_ep={} ep_type={}",
        svc.name,
        fault_ep,
        eneros_sel4_sys::safe::debug_cap_identify(fault_ep)
    );

    // 8c. G-R1（23 G-I 总出口修复）：root 侧 fault_ep 副本——mint 带 badge
    // （= 服务索引+1）到 root CSpace（新槽 `root_fault_ep`）。seL4 fault IPC
    // 接收方的 badge = 接收者持有的 endpoint cap badge（doFaultTransfer 把
    // 该 badge 写入 badge 寄存器）——root 侧必须持带 badge 的 fault_ep 副本，
    // run_ipc_loop 在此 poll 得 sender=badge 识别 faulting 服务。服务侧槽 1
    // 的 mint badge 不影响 root 收到的 badge（1368180 的 notification badge
    // 方案 + 服务侧 badge 均无法让 root 侧收到非零 sender，QEMU 实测 sender=0）。
    let root_fault_ep = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
    cnode_mint(
        ctx.root_cnode,
        root_fault_ep,
        64, // root cnode 的 levelBits = radix(12)+guard(52) = 64；
        // lookupSlotForCNodeOp 要求 depth ∈ [1, wordBits] 且 n_bits=64
        // 才能 guard 匹配（0 直接 RangeError，QEMU 实测）
        root_cnode,
        fault_ep,
        64,
        rights.all,
        ntfn_badge,
    )
    .map_err(SpawnError::Sel4)?;

    // 8b. K2.3：故障 notification——retype + 绑定 TCB（21-A 设计）。
    //
    // 23 G-I 总出口纠偏（相对 1368180）：badge mint 方案（服务 CSpace 槽 2 +
    // root 侧带 badge 副本）与 seL4 fault 通知机制不符——fault IPC 走 fault_ep
    // endpoint（run_ipc_loop 的 `handle.ntfn = fault_ep`），不走 notification
    // badge；1368180 因此从未系统级验证（bootctrl Crc 阻断期，ADR-033），
    // QEMU 实测 `unknown fault sender=0`。回退 21-A 的 notification 绑定
    // （保留 `tcb_bind_notification(tcb, ntfn)` 的槽号修复）。
    let ntfn = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
    retype_one(ctx, types.notification, OBJ_BITS_NOTIFICATION, ntfn)?;
    // SAFETY: tcb/ntfn 均为刚 retype 的有效 cap。bind 的 ntfn 参数是调用者
    // （root-task）CNode 中的 cap 槽——必须传 root 侧槽 `ntfn`（=939 等），
    // 不是服务 CSpace 槽 2。1368180 硬编码 2（root CSpace 槽 2 非 notification）
    // 导致 QEMU `BindNotification: Notification is invalid`（22 G-R1 遗留，
    // 从未系统级验证；23 G-I 总出口暴露并修复）。
    eneros_sel4_sys::safe::tcb_bind_notification(tcb, ntfn).map_err(SpawnError::Sel4)?;

    // 9. 寄存器：PC=entry_mapped, SP=stack_top, X0=0(ep cap 槽), X1=IPC buffer vaddr。
    //    布局知识由 `initial_registers` 单一持有（W2.1）；count=5 由该函数契约保证。
    let regs = initial_registers(entry, svc.stack_top, buffer_vaddr);
    // SAFETY: regs 指向有效 UserContext（栈上），count=5 写 pc/sp/spsr/x0/x1。
    tcb_write_registers(tcb, 0, 5, &regs).map_err(SpawnError::Sel4)?;

    // 10. 优先级（authority = root TCB cap）。
    // SAFETY: root_tcb 为 root-task 自身 TCB cap（有效且有权）。
    tcb_set_priority(tcb, ctx.root_tcb, SVC_PRIORITY).map_err(SpawnError::Sel4)?;

    Ok((tcb, ep, root_fault_ep))
}

/// 22 §2.W1.6：映射设备 MMIO 区域进 root-task VSpace（1:1）。
///
/// SP805 看门狗 @ `paddr`（QEMU 无设备时读写为 no-op 不 fault；真机有效）。
/// 用 device untyped retype frame → map 到同一物理地址（`arm_page_map`
/// 物理地址 = 虚拟地址的 1:1 语义由调用方保证）。
#[cfg(target_os = "none")]
pub fn map_device_mmio(ctx: &mut SpawnCtx, paddr: u64, size_bits: u64) -> Result<(), SpawnError> {
    let types = ObjectTypeTable::resolve();
    let rights = RightsTable::resolve();
    // 分配 frame 槽并 retype（device untyped 承载设备内存）。
    let slot = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
    // FIX-BUGS-0810 B2：按物理地址定位 device untyped（alloc_device_at）；
    // 平台未导出覆盖 paddr 的 device untyped → Err → main.rs 软件模式兜底
    // （"映射成功 ⇔ 正确物理页"，杜绝错页映射成功）。
    retype_device_one(ctx, types.arm_frame, size_bits, paddr, slot)?;
    // SAFETY: slot 为刚 retype 的 frame cap；root_vspace 已绑 ASID；
    // paddr 页对齐（调用方保证）；设备内存 map 用读写权限。
    arm_page_map(ctx.root_vspace, slot, paddr, rights.read | rights.write)
        .map_err(SpawnError::Sel4)?;
    Ok(())
}

/// retype 设备 frame（从 device untyped；22 §2.W1.6 SP805 等 MMIO）。
#[cfg(target_os = "none")]
fn retype_device_one(
    ctx: &mut SpawnCtx,
    object_type: u64,
    size_bits: u64,
    paddr: u64,
    slot: u64,
) -> Result<(), SpawnError> {
    // FIX-BUGS-0810 B2：物理地址定位（alloc_device_at），丢弃返回偏移。
    let (untyped, _byte_off) = ctx
        .untyped
        .alloc_device_at(paddr, size_bits)
        .ok_or(SpawnError::NoUntyped)?;
    // SAFETY: untyped 为覆盖 paddr 的 device untyped cap；slot 为 root CSpace
    // 空闲槽；node_offset = slot（BOOT-010：seL4 的 node_offset 是 dest cnode
    // 槽，不是 untyped 偏移——旧实现传 alloc_device 的 offset 导致 cap 写入
    // cnode[offset] 而非目标槽；FIX-BUGS-0810 B2 两件套之一）。
    eneros_sel4_sys::safe::retype(
        untyped,
        object_type,
        size_bits,
        1,
        ctx.root_cnode,
        slot,
        0,
        slot,
    )
    .map_err(SpawnError::Sel4)
}

/// retype 单个对象到 root CSpace 的 `slot` 槽。
#[cfg(target_os = "none")]
fn retype_one(
    ctx: &mut SpawnCtx,
    object_type: u64,
    size_bits: u64,
    slot: u64,
) -> Result<(), SpawnError> {
    // CapTable 的 seL4 实际大小 = 2^(size_bits+5)（SlotBits=5），与普通
    // 对象（2^size_bits）记账不同，须走 alloc_cap_table（21-A 遗留缺陷：
    // CNode 用 128 B 记账但 seL4 占 4 KiB → freeIndex 脱节，QEMU 实测
    // endpoint retype `Insufficient memory (1 * 16 bytes needed)`）。
    let (untyped, offset) = if object_type == ObjectTypeTable::resolve().captable {
        ctx.untyped
            .alloc_cap_table(size_bits)
            .ok_or(SpawnError::NoUntyped)?
    } else {
        ctx.untyped.alloc(size_bits).ok_or(SpawnError::NoUntyped)?
    };
    // SAFETY: untyped 由分配器保证足够 size_bits 对象；slot 由 CapAllocator
    // 保证在 root CSpace 容量内；dest_depth=0（一阶 cnode）。
    crate::println!(
        "[eneros-root-task] DBG retype_one untyped={} offset={} type={} slot={} untyped_cap_type={} cnode_type={}",
        untyped,
        offset,
        object_type,
        slot,
        eneros_sel4_sys::safe::debug_cap_identify(untyped),
        eneros_sel4_sys::safe::debug_cap_identify(ctx.root_cnode)
    );
    retype(
        untyped,
        object_type,
        size_bits,
        1,
        ctx.root_cnode,
        slot,
        0,
        // BOOT-010：seL4 的 node_offset 是 dest cnode 槽（不是 untyped 偏移）；
        // untyped 的物理偏移由内核 freeIndex 自动分配。21-A 把 untyped 偏移
        // 当 node_offset，导致 cap 写入 cnode[offset]（非目标槽）。
        slot,
    )
    .map_err(SpawnError::Sel4)
}

/// retype `count` 个同类型对象，返回它们的槽号。
#[cfg(target_os = "none")]
fn retype_objects(
    ctx: &mut SpawnCtx,
    types: &ObjectTypeTable,
    size_bits: u64,
    count: usize,
) -> Result<Vec<u64>, SpawnError> {
    let mut slots = Vec::with_capacity(count);
    for _ in 0..count {
        let slot = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
        retype_one(ctx, types.arm_page_table, size_bits, slot)?;
        slots.push(slot);
    }
    Ok(slots)
}

/// 方案 A 页内容拷贝：窗口映射 → 拷贝 → 卸载（ADR-0034）。
///
/// 把 bundle blob `[src_offset, src_offset+len)` 的 `len` 字节拷入 `frame`
/// 物理页。frame 映射到 root VSpace 临时窗口（固定 `WINDOW_BASE`），拷贝后
/// `arm_page_unmap` 解除（frame 只能映射一个地址；unmap 后窗口地址
/// 可被下个 frame 复用）。`len` 不得超过页大小（调用方按页分块）。
#[cfg(target_os = "none")]
fn copy_segment_via_window(
    ctx: &mut SpawnCtx,
    frame: u64,
    module: &crate::bundle::BundleModule,
    src_offset: u64,
    len: usize,
    rights: &RightsTable,
) -> Result<(), SpawnError> {
    use core::ptr::copy_nonoverlapping;
    debug_assert!(len <= PAGE_SIZE as usize, "单次拷贝不得超过一页");
    // SAFETY: WINDOW_BASE 为 root VSpace 内已建页表的窗口槽；frame 刚
    // retype 且未映射（unmap 后 cap 已解除，可重新映射）。
    arm_page_map(
        ctx.root_vspace,
        frame,
        WINDOW_BASE,
        rights.read | rights.write,
    )
    .map_err(SpawnError::Sel4)?;
    // SAFETY: 目标 WINDOW_BASE 指向刚映射的物理页（4 KiB）；源为 bundle
    // blob 的有效区间（elf-loader 已校验 offset+filesz 界内，len 由调用方
    // 限制在页内）。
    let src = module.blob.as_ptr().wrapping_add(src_offset as usize);
    unsafe {
        copy_nonoverlapping(src, WINDOW_BASE as *mut u8, len);
    }
    // SAFETY: frame 已映射（卸载前置条件满足）。
    arm_page_unmap(frame).map_err(SpawnError::Sel4)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::{
        InitBundle, INIT_BUNDLE_ENTRY_LEN, INIT_BUNDLE_HEADER_LEN, INIT_BUNDLE_MAGIC,
        INIT_BUNDLE_VERSION,
    };

    /// 最小 ELF64 PIE AArch64（字节布局与 elf-loader 测试 fixture 对齐）。
    fn minimal_exec_elf() -> Vec<u8> {
        let mut buf = vec![0u8; 64 + 56 + 8];
        // e_ident
        buf[..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
        buf[4] = 2; // ELFCLASS64
        buf[5] = 1; // ELFDATA2LSB
        buf[6] = 1; // EV_CURRENT
                    // e_type = ET_DYN (3)
        buf[16..18].copy_from_slice(&3u16.to_le_bytes());
        // e_machine = EM_AARCH64 (183)
        buf[18..20].copy_from_slice(&183u16.to_le_bytes());
        // e_entry = 0x1000
        buf[24..32].copy_from_slice(&0x1000u64.to_le_bytes());
        // e_phoff = 64
        buf[32..40].copy_from_slice(&64u64.to_le_bytes());
        // e_phentsize = 56
        buf[54..56].copy_from_slice(&56u16.to_le_bytes());
        // e_phnum = 1
        buf[56..58].copy_from_slice(&1u16.to_le_bytes());
        // PT_LOAD（标准 Elf64_Phdr：type|flags|offset|vaddr|paddr|filesz|memsz|align）
        let ph = 64;
        buf[ph..ph + 4].copy_from_slice(&1u32.to_le_bytes()); // p_type
        buf[ph + 4..ph + 8].copy_from_slice(&5u32.to_le_bytes()); // p_flags = R+X
        buf[ph + 8..ph + 16].copy_from_slice(&0u64.to_le_bytes()); // p_offset
        buf[ph + 16..ph + 24].copy_from_slice(&0x1000u64.to_le_bytes()); // p_vaddr
        buf[ph + 24..ph + 32].copy_from_slice(&0x1000u64.to_le_bytes()); // p_paddr
        buf[ph + 32..ph + 40].copy_from_slice(&8u64.to_le_bytes()); // p_filesz
        buf[ph + 40..ph + 48].copy_from_slice(&8u64.to_le_bytes()); // p_memsz
        buf[ph + 48..ph + 56].copy_from_slice(&0x1000u64.to_le_bytes()); // p_align
        buf[120..128].copy_from_slice(&[0xC0, 0x03, 0x5F, 0xD6, 0, 0, 0, 0]); // ret
        buf
    }

    /// 最小 R-X + RW- 双段 ELF。
    fn minimal_data_elf() -> Vec<u8> {
        let mut buf = vec![0u8; 64 + 56 * 2 + 8 + 8];
        buf[..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
        buf[4] = 2;
        buf[5] = 1;
        buf[6] = 1;
        buf[16..18].copy_from_slice(&3u16.to_le_bytes());
        buf[18..20].copy_from_slice(&183u16.to_le_bytes());
        buf[24..32].copy_from_slice(&0x1000u64.to_le_bytes());
        buf[32..40].copy_from_slice(&64u64.to_le_bytes());
        buf[54..56].copy_from_slice(&56u16.to_le_bytes());
        buf[56..58].copy_from_slice(&2u16.to_le_bytes()); // e_phnum = 2
        let ph1 = 64;
        buf[ph1..ph1 + 4].copy_from_slice(&1u32.to_le_bytes());
        buf[ph1 + 4..ph1 + 8].copy_from_slice(&5u32.to_le_bytes());
        buf[ph1 + 8..ph1 + 16].copy_from_slice(&0u64.to_le_bytes());
        buf[ph1 + 16..ph1 + 24].copy_from_slice(&0x1000u64.to_le_bytes());
        buf[ph1 + 24..ph1 + 32].copy_from_slice(&0x1000u64.to_le_bytes());
        buf[ph1 + 32..ph1 + 40].copy_from_slice(&8u64.to_le_bytes());
        buf[ph1 + 40..ph1 + 48].copy_from_slice(&8u64.to_le_bytes());
        buf[ph1 + 48..ph1 + 56].copy_from_slice(&0x1000u64.to_le_bytes());
        let ph2 = 64 + 56;
        let seg2_off = 64 + 56 * 2;
        buf[ph2..ph2 + 4].copy_from_slice(&1u32.to_le_bytes());
        buf[ph2 + 4..ph2 + 8].copy_from_slice(&6u32.to_le_bytes()); // R+W
        buf[ph2 + 8..ph2 + 16].copy_from_slice(&(seg2_off as u64).to_le_bytes());
        buf[ph2 + 16..ph2 + 24].copy_from_slice(&0x2000u64.to_le_bytes());
        buf[ph2 + 24..ph2 + 32].copy_from_slice(&0x2000u64.to_le_bytes());
        buf[ph2 + 32..ph2 + 40].copy_from_slice(&8u64.to_le_bytes());
        buf[ph2 + 40..ph2 + 48].copy_from_slice(&8u64.to_le_bytes());
        buf[ph2 + 48..ph2 + 56].copy_from_slice(&0x1000u64.to_le_bytes());
        buf[seg2_off..seg2_off + 8].copy_from_slice(&[0u8; 8]);
        buf
    }

    /// 构造 init bundle（root + n 个服务）。
    fn build_bundle(modules: &[(&str, u32, Vec<u8>)]) -> Vec<u8> {
        let table_len = INIT_BUNDLE_HEADER_LEN + modules.len() * INIT_BUNDLE_ENTRY_LEN;
        let payload_len: usize = modules.iter().map(|m| m.2.len()).sum();
        let total = table_len + payload_len;
        let mut bytes = vec![0u8; total];
        bytes[..8].copy_from_slice(INIT_BUNDLE_MAGIC);
        bytes[8..10].copy_from_slice(&INIT_BUNDLE_VERSION.to_le_bytes());
        bytes[10..12].copy_from_slice(&(INIT_BUNDLE_HEADER_LEN as u16).to_le_bytes());
        bytes[12..16].copy_from_slice(&(total as u32).to_le_bytes());
        bytes[24..28].copy_from_slice(&(modules.len() as u32).to_le_bytes());
        let mut cursor = table_len;
        for (i, (name, kind, blob)) in modules.iter().enumerate() {
            let entry = INIT_BUNDLE_HEADER_LEN + i * INIT_BUNDLE_ENTRY_LEN;
            bytes[entry..entry + 4].copy_from_slice(&kind.to_le_bytes());
            bytes[entry + 4..entry + 8].copy_from_slice(&0u32.to_le_bytes());
            bytes[entry + 8..entry + 16].copy_from_slice(&(cursor as u64).to_le_bytes());
            bytes[entry + 16..entry + 24].copy_from_slice(&(blob.len() as u64).to_le_bytes());
            bytes[entry + 24..entry + 24 + name.len()].copy_from_slice(name.as_bytes());
            bytes[cursor..cursor + blob.len()].copy_from_slice(blob);
            cursor += blob.len();
        }
        bytes
    }

    #[test]
    fn test_plan_spawn_root_plus_two_services() {
        let raw = build_bundle(&[
            (
                "root-task",
                crate::bundle::MODULE_KIND_ROOT,
                minimal_exec_elf(),
            ),
            (
                "svc-fault",
                crate::bundle::MODULE_KIND_SERVICE,
                minimal_data_elf(),
            ),
            (
                "svc-proc-ts",
                crate::bundle::MODULE_KIND_SERVICE,
                minimal_data_elf(),
            ),
        ]);
        let bundle = InitBundle::parse(&raw).expect("bundle parse");
        let plan = plan_spawn(&bundle, 0x4000_0000, 0x1000, 0x4000, 100).expect("plan");
        // 验收口径：root + ≥2 服务
        assert_eq!(plan.root.name, "root-task");
        assert_eq!(plan.service_count(), 2);
        assert_eq!(plan.services[0].name, "svc-fault");
        assert_eq!(plan.services[1].name, "svc-proc-ts");
        // 服务有独立 IPC EP；映射到各自 ELF 链接地址（seg.vaddr，0x200000
        // 区域），独立 VSpace 下虚拟地址可相同（stack_top 允许相等）。
        assert_eq!(plan.services[0].stack_top, plan.services[1].stack_top);
        assert_eq!(plan.services[0].ipc_ep, 101);
        assert_eq!(plan.services[1].ipc_ep, 102);
        // 页表 W^X：无可写可执行页（root 的 W^X 已由 plan_one 内部校验——
        // W4.3 后 SpawnRoot 不含 pages）
        for s in &plan.services {
            for p in &s.pages {
                assert!(!p.is_wx_violation(), "{} 页违反 W^X", s.name);
            }
        }
    }

    #[test]
    fn test_plan_spawn_too_few_services_fail_closed() {
        let raw = build_bundle(&[
            (
                "root-task",
                crate::bundle::MODULE_KIND_ROOT,
                minimal_exec_elf(),
            ),
            (
                "svc-fault",
                crate::bundle::MODULE_KIND_SERVICE,
                minimal_exec_elf(),
            ),
        ]);
        let bundle = InitBundle::parse(&raw).expect("bundle parse");
        assert_eq!(
            plan_spawn(&bundle, 0x4000_0000, 0x1000, 0x4000, 100),
            Err(SpawnError::TooFewServices),
            "服务不足 2 个必须 fail-closed"
        );
    }

    #[test]
    fn test_plan_spawn_no_root_fail_closed() {
        let raw = build_bundle(&[
            (
                "svc-fault",
                crate::bundle::MODULE_KIND_SERVICE,
                minimal_exec_elf(),
            ),
            (
                "svc-proc-ts",
                crate::bundle::MODULE_KIND_SERVICE,
                minimal_exec_elf(),
            ),
        ]);
        let bundle = InitBundle::parse(&raw).expect("bundle parse");
        assert_eq!(
            plan_spawn(&bundle, 0x4000_0000, 0x1000, 0x4000, 100),
            Err(SpawnError::NoRootTask)
        );
    }

    #[test]
    fn test_plan_spawn_no_services_fail_closed() {
        let raw = build_bundle(&[(
            "root-task",
            crate::bundle::MODULE_KIND_ROOT,
            minimal_exec_elf(),
        )]);
        let bundle = InitBundle::parse(&raw).expect("bundle parse");
        assert_eq!(
            plan_spawn(&bundle, 0x4000_0000, 0x1000, 0x4000, 100),
            Err(SpawnError::NoServices)
        );
    }

    #[test]
    fn test_tenant_enforce_spawn_tracks_usage() {
        // 蓝图/20 R2-3 验收：spawn 前/后 usage 差异真实累计；release 扣减。
        use eneros_tenant::manager::TenantManager;
        use eneros_tenant::{TenantEnforcer, TenantQuota, TenantStore};
        let mut store = TenantStore::new();
        let quota = TenantQuota {
            max_cpu_pct: 100.0,
            max_mem_kb: 8192,
            max_storage_kb: 65536,
            max_agents: 4,
            max_bandwidth_kbps: 1000,
        };
        let tenant = store.create("t0", quota).expect("create");
        let tid = tenant.tenant_id;
        let mut enf = TenantEnforcer::new(store);

        assert_eq!(enf.usage(tid).unwrap().mem_kb, 0, "spawn 前 usage 为零");
        enforce_spawn(&mut enf, tid, 500, 10.0).expect("spawn 记账");
        let u = enf.usage(tid).unwrap();
        assert_eq!(u.mem_kb, 500, "spawn 后 usage 真实累计");
        assert!((u.cpu_pct - 10.0).abs() < 1e-6);

        release_spawn(&mut enf, tid, 500, 10.0).expect("释放");
        assert_eq!(enf.usage(tid).unwrap().mem_kb, 0, "释放后扣减归零");
    }

    #[test]
    fn test_tenant_enforce_spawn_over_quota_rejected() {
        use eneros_tenant::manager::TenantManager;
        use eneros_tenant::{PlatformError, TenantEnforcer, TenantQuota, TenantStore};
        let mut store = TenantStore::new();
        let quota = TenantQuota {
            max_cpu_pct: 100.0,
            max_mem_kb: 1024,
            max_storage_kb: 65536,
            max_agents: 4,
            max_bandwidth_kbps: 1000,
        };
        let tenant = store.create("t1", quota).expect("create");
        let tid = tenant.tenant_id;
        let mut enf = TenantEnforcer::new(store);

        enforce_spawn(&mut enf, tid, 800, 10.0).expect("spawn 1");
        assert_eq!(
            enforce_spawn(&mut enf, tid, 800, 10.0),
            Err(PlatformError::QuotaExceeded),
            "超配必须拒绝且 usage 不增长"
        );
        assert_eq!(enf.usage(tid).unwrap().mem_kb, 800, "拒绝后 usage 不变");
    }

    #[test]
    fn test_plan_spawn_with_enforcement_rejects_over_quota() {
        // 蓝图/20 R2-3 验收：强制力接入 spawn 路径——超配时计划不落地。
        use eneros_tenant::manager::TenantManager;
        use eneros_tenant::{TenantEnforcer, TenantQuota, TenantStore};
        let mut store = TenantStore::new();
        let quota = TenantQuota {
            max_cpu_pct: 100.0,
            max_mem_kb: 512,
            max_storage_kb: 65536,
            max_agents: 4,
            max_bandwidth_kbps: 1000,
        };
        let tenant = store.create("t2", quota).expect("create");
        let tid = tenant.tenant_id;
        let mut enf = TenantEnforcer::new(store);

        let raw = build_bundle(&[
            (
                "root-task",
                crate::bundle::MODULE_KIND_ROOT,
                minimal_data_elf(),
            ),
            (
                "svc-fault",
                crate::bundle::MODULE_KIND_SERVICE,
                minimal_data_elf(),
            ),
            (
                "svc-proc-ts",
                crate::bundle::MODULE_KIND_SERVICE,
                minimal_data_elf(),
            ),
        ]);
        let bundle = InitBundle::parse(&raw).expect("bundle parse");
        let result = plan_spawn_with_enforcement(
            &mut enf,
            &bundle,
            0x4000_0000,
            4096,
            16384,
            100,
            tid,
            4096,
            50.0,
        );
        assert_eq!(
            result,
            Err(SpawnError::QuotaExceeded),
            "超配必须拒绝且计划不落地"
        );
        assert_eq!(enf.usage(tid).unwrap().mem_kb, 0, "拒绝后 usage 不增长");
    }

    #[test]
    fn test_page_entry_wx_semantics() {
        let exec = PageEntry {
            vaddr: 0x1000,
            phys: 0,
            size: 0x1000,
            read: true,
            write: false,
            exec: true,
        };
        assert!(!exec.is_wx_violation());
        let wx = PageEntry {
            vaddr: 0x1000,
            phys: 0,
            size: 0x1000,
            read: true,
            write: true,
            exec: true,
        };
        assert!(wx.is_wx_violation(), "W^X：可写可执行必须判定违规");
    }

    // ---- 21-A K1.2（ADR-0034）：untyped 分配器 / entry 平移 / 页权限 ----

    #[test]
    fn untyped_allocator_first_fit() {
        // 两个 untyped（1 MiB / 4 MiB），分配 4 KiB 对象应从第一个切起。
        let descs = vec![
            UntypedDesc {
                paddr: 0x1000,
                size_bits: 20, // 1 MiB = 256 个 4 KiB 对象
                is_device: false,
            },
            UntypedDesc {
                paddr: 0x1000_0000,
                size_bits: 22, // 4 MiB
                is_device: false,
            },
        ];
        let mut alloc = UntypedAllocator::new(30, descs);
        for i in 0..256 {
            let (cap, off) = alloc.alloc(12).expect("256 次分配必须成功");
            assert_eq!(cap, 30, "untyped cap 槽 = cap_base + 0");
            assert_eq!(off, i * 4096, "字节偏移逐次递增（freeIndex 语义）");
        }
        let (cap, _) = alloc.alloc(12).expect("第 257 次落到第二个 untyped");
        assert_eq!(cap, 31);
        let (cap, off) = alloc.alloc(13).expect("第二个 untyped 容纳 8 KiB 对象");
        assert_eq!(
            (cap, off),
            (31, 8192),
            "8 KiB 对象从 8 KiB 对齐处开始（seL4 alignUp 语义）"
        );
        assert_eq!(alloc.alloc(22), None, "无 4 MiB 以上空间 → None");
    }

    // ---- FIX-BUGS-0810 B2 防回归：alloc_device_at（物理定位，两件套之二）----

    /// device untyped 列表（SP805 区域 + UART 区域）。
    fn device_descs() -> Vec<UntypedDesc> {
        vec![
            UntypedDesc {
                paddr: 0x0905_0000, // SP805 WDT
                size_bits: 16, // 64 KiB
                is_device: true,
            },
            UntypedDesc {
                paddr: 0x0900_0000, // UART
                size_bits: 16,
                is_device: true,
            },
        ]
    }

    #[test]
    fn alloc_device_at_covers_phys() {
        let mut alloc = UntypedAllocator::new(30, device_descs());
        // 请求被 device untyped 覆盖的物理地址 → 返回该 untyped 与字节偏移。
        let (cap, off) = alloc
            .alloc_device_at(0x0905_0000, 12)
            .expect("SP805 区域必须被覆盖");
        assert_eq!(cap, 30, "cap 槽 = cap_base + SP805 untyped 索引");
        assert_eq!(off, 0, "区域首块字节偏移为 0");
        // 同一区域内再次分配（页内偏移推进）。
        let (cap2, off2) = alloc
            .alloc_device_at(0x0905_1000, 12)
            .expect("同区域次页");
        assert_eq!(cap2, 30);
        assert_eq!(off2, 0x1000);
    }

    #[test]
    fn alloc_device_at_uncovered_returns_none() {
        let mut alloc = UntypedAllocator::new(30, device_descs());
        // 请求未被任何 device untyped 覆盖的物理地址 → None（不得 first-fit
        // 兜底——R-B2a：错误物理页映射必须不可能）。
        assert_eq!(
            alloc.alloc_device_at(0x0800_0000, 12),
            None,
            "未覆盖 phys 必须 None（禁 first-fit 兜底）"
        );
        // 覆盖但大小不足 → None。
        assert_eq!(alloc.alloc_device_at(0x0905_0000, 20), None);
    }

    #[test]
    fn alloc_device_at_does_not_mix_non_device() {
        // 非 device untyped 即使物理覆盖也不得使用（普通 untyped 不能
        // retype device frame）。
        let descs = vec![UntypedDesc {
            paddr: 0x0905_0000,
            size_bits: 16,
            is_device: false,
        }];
        let mut alloc = UntypedAllocator::new(30, descs);
        assert_eq!(
            alloc.alloc_device_at(0x0905_0000, 12),
            None,
            "非 device untyped 不得用于设备分配"
        );
    }

    #[test]
    fn alloc_device_at_overlapping_region_still_bounded() {
        // 多 device untyped 重叠物理区域：请求落在第二个 untyped 覆盖内
        // 必须选第二个（cap 索引正确），不得错选。
        let mut alloc = UntypedAllocator::new(40, device_descs());
        let (cap, off) = alloc
            .alloc_device_at(0x0900_1000, 12)
            .expect("UART 区域覆盖");
        assert_eq!(cap, 41, "UART untyped = cap_base + 1");
        assert_eq!(off, 0x1000);
    }

    // ---- FIX-BUGS-0810 B4 防回归：alloc_cap_table 对齐与守卫 ----

    #[test]
    fn alloc_cap_table_aligns_accounting() {
        let descs = vec![UntypedDesc {
            paddr: 0x1000,
            size_bits: 20, // 1 MiB
            is_device: false,
        }];
        let mut alloc = UntypedAllocator::new(30, descs);
        // 先分配 16 B / 32 B 小对象（普通 alloc，alignUp 记账）。
        let (_, off16) = alloc.alloc(4).expect("16 B");
        assert_eq!(off16, 0);
        let (_, off32) = alloc.alloc(5).expect("32 B");
        assert_eq!(off32, 32, "16 B 后 32 B 对象从 32 B 对齐处开始（alignUp 语义）");
        // 再分配 CNode（size_bits=8 → 2^8 槽 × 32 B = 8 KiB）：
        // 旧行为：used=48 直接累加 → 48（与 seL4 freeIndex 对齐语义脱节）；
        // 新行为：alignUp(48, 8192) = 8192。
        let (_, off_cnode) = alloc.alloc_cap_table(8).expect("CNode");
        assert_eq!(
            off_cnode, 8192,
            "CNode 记账必须按对象大小对齐（alignUp(48, 8192)=8192，证伪旧直接累加）"
        );
    }

    #[test]
    fn alloc_cap_table_guards_oversize() {
        let descs = vec![UntypedDesc {
            paddr: 0x1000,
            size_bits: 63, // 合法超大 untyped（< 64，通过 desc 守卫）
            is_device: false,
        }];
        let mut alloc = UntypedAllocator::new(30, descs);
        // size_bits + 5 >= 64 → None（红线 6：防移位回绕；旧行为 release 下
        // 移位取模回绕得到错误 obj_size）。
        assert_eq!(alloc.alloc_cap_table(59), None);
        assert_eq!(alloc.alloc_cap_table(60), None);
        // 合理大小仍可分配。
        assert!(alloc.alloc_cap_table(8).is_some());
    }

    #[test]
    fn untyped_allocator_exhausted_returns_none() {
        // 单个 4 KiB untyped：第 2 次分配必须失败（K1.2 验收：不足 untyped
        // → fail-closed 路径）。
        let descs = vec![UntypedDesc {
            paddr: 0x1000,
            size_bits: 12, // 恰好 1 个 4 KiB 对象
            is_device: false,
        }];
        let mut alloc = UntypedAllocator::new(40, descs);
        assert!(alloc.alloc(12).is_some());
        assert_eq!(alloc.alloc(12), None, "untyped 耗尽必须返回 None");
    }

    #[test]
    fn untyped_allocator_skips_device_memory() {
        let descs = vec![
            UntypedDesc {
                paddr: 0xA000_0000,
                size_bits: 20,
                is_device: true, // 设备内存不可用于对象
            },
            UntypedDesc {
                paddr: 0x2000_0000,
                size_bits: 20,
                is_device: false,
            },
        ];
        let mut alloc = UntypedAllocator::new(50, descs);
        let (cap, off) = alloc.alloc(12).expect("必须跳过 device untyped");
        assert_eq!((cap, off), (51, 0));
    }

    #[test]
    fn entry_mapped_translates_into_plan_vspace() {
        let elf = LoadedElf::parse(&minimal_exec_elf()).expect("elf");
        // 段 vaddr 0x1000 → 计划页 vaddr 0x4000_0000；entry 0x1000 → 0x4000_0000。
        let pages = vec![PageEntry {
            vaddr: 0x4000_0000,
            phys: 0,
            size: 0x1000,
            read: true,
            write: false,
            exec: true,
        }];
        assert_eq!(entry_mapped(&pages, &elf), Ok(0x4000_0000));
    }

    #[test]
    fn entry_mapped_rejects_unaligned_segment() {
        // 段 vaddr 非页对齐（链接器异常产物）→ fail-closed；entry 同步到段内。
        let mut buf = minimal_exec_elf();
        let ph = 64;
        buf[ph + 16..ph + 24].copy_from_slice(&0x1080u64.to_le_bytes()); // p_vaddr = 0x1080
        buf[24..32].copy_from_slice(&0x1080u64.to_le_bytes()); // e_entry 在段内
        let elf = LoadedElf::parse(&buf).expect("elf");
        let pages = vec![PageEntry {
            vaddr: 0x4000_0000,
            phys: 0,
            size: 0x1000,
            read: true,
            write: false,
            exec: true,
        }];
        assert_eq!(entry_mapped(&pages, &elf), Err(SpawnError::PlanMismatch));
    }

    #[test]
    fn page_rights_maps_rw_flags() {
        let rights = eneros_sel4_sys::safe::RightsTable::resolve();
        assert_eq!(page_rights(true, false, &rights), rights.read);
        assert_eq!(page_rights(true, true, &rights), rights.read | rights.write);
        assert_eq!(page_rights(false, false, &rights), rights.none);
    }

    #[test]
    fn multi_page_segment_plans_and_maps_entry() {
        // review 阻塞项回归：段 > 4 KiB 时计划必须生成 size=2 页的 PageEntry，
        // entry 平移仍正确（spawn 端按页循环映射 + 分页拷贝）。
        let mut buf = minimal_exec_elf();
        // 段数据扩展到 2 页（filesz/memsz = 0x2000），缓冲区随之扩大。
        buf.resize(64 + 56 + 0x2000, 0);
        let ph = 64;
        buf[ph + 32..ph + 40].copy_from_slice(&0x2000u64.to_le_bytes()); // p_filesz
        buf[ph + 40..ph + 48].copy_from_slice(&0x2000u64.to_le_bytes()); // p_memsz
        let elf = LoadedElf::parse(&buf).expect("elf");
        let pages = vec![PageEntry {
            vaddr: 0x4000_0000,
            phys: 0,
            size: 0x2000, // 2 页
            read: true,
            write: false,
            exec: true,
        }];
        assert_eq!(entry_mapped(&pages, &elf), Ok(0x4000_0000));
        assert_eq!(pages[0].size / PAGE_SIZE, 2, "段按页对齐后跨 2 页");
    }
}
