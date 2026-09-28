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

use eneros_boot_protocol::{ENEROS_INIT_BUNDLE_PHYS, ENEROS_RESERVED_WINDOW_LEN};
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// 30 RT-006：PT_LOAD 段 vaddr 区间重叠/回绕（elf-loader
    /// validate_segments 判定，SpawnError 侧透传）。
    SegmentOverlap,
    /// 30 RT-007：单次窗口拷贝超过页大小（release 也生效——原
    /// debug_assert 在 release 剔除后越页写 WINDOW_BASE 窗口）。
    WindowOverflow,
    /// 租户强制力拒绝（超配/租户不存在，蓝图/20 R2-3）。
    QuotaExceeded,
    /// 虚拟地址空间越界。
    OutOfVaddrSpace,
    /// required 服务 ELF 解析失败（WP-37.1：坏 required 必阻断 product
    /// boot——旧实现坏 ELF continue 后只看服务数量 ≥2）。
    RequiredElfFailed { name: String },
    /// bundle 含未知 kind 模块（WP-37.1：kind 只在 {1,2,3}）。
    UnknownBundleKind,
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
    /// WP-37.4：ET_DYN 重定位镜像槽位越界（configure_service 帧拷贝源
    /// 校验；review blocking 修复——target-only 路径曾引用未定义变体）。
    RelocOutOfBounds,
    /// WP-37.4：ET_DYN staging 跨度超上限（MAX_ETDYN_STAGING，防恶意
    /// 稀疏段 OOM）。
    EtDynStagingTooLarge,
}

/// 页表条目（W^X 语义；WP-37.4：页模型——非对齐段按 floor 页基映射）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageEntry {
    /// 虚拟地址（**页对齐**页基；非对齐段 vaddr 映射到 floor 页）。
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
    /// WP-37.4（ET_EXEC 拷贝源）：本页首字节对应的 bundle blob 文件偏移
    /// （= seg.offset - io + k*PAGE；io = seg.vaddr - floor(seg.vaddr)）。
    /// ET_DYN 忽略（整页从重定位镜像拷）。
    pub src_file_offset: u64,
    /// WP-37.4（ET_EXEC 拷贝目标）：页内写入偏移（非对齐段首页 = io，
    /// 其余页 = 0）。ET_DYN 忽略。
    pub dst_offset: u64,
    /// WP-37.4（ET_EXEC 拷贝长度）：本页需拷贝的段字节数
    /// （≤ size - dst_offset；BSS 余量为 retype 清零）。
    pub copy_len: usize,
}

impl PageEntry {
    /// W^X 校验：可执行页不得可写（页级合并后）。
    pub fn is_wx_violation(&self) -> bool {
        self.exec && self.write
    }
}

/// WP-37.4：段 → 页计划（ET_EXEC/ET_DYN 共用）——非对齐段 vaddr
/// （lld 末段紧随前段 filesz 尾，PIC 代码生成后普遍出现）按 floor 页基
/// 映射；同页多段合并权限（页级 W+X 拒绝）；ET_EXEC 拷贝三要素
/// （src_file_offset/dst_offset/copy_len）逐页计算。
fn plan_pages(
    segments: &[eneros_elf_loader::LoadedSegment],
    page_size: u64,
) -> Result<Vec<PageEntry>, SpawnError> {
    let mut pages: Vec<PageEntry> = Vec::new();
    for seg in segments {
        let flags = seg.flags;
        if flags.is_wx_violation() {
            return Err(SpawnError::WxViolation);
        }
        // ELF 规范：p_offset ≡ p_vaddr (mod p_align)——非对齐段 io 与
        // offset 尾差同余，src 偏移恒页对齐（checked 防御）。
        let page_base = seg.vaddr & !(page_size - 1);
        let io = seg.vaddr - page_base;
        if seg.offset < io {
            return Err(SpawnError::PlanMismatch);
        }
        let seg_end = seg
            .vaddr
            .checked_add(seg.memsz)
            .ok_or(SpawnError::OutOfVaddrSpace)?;
        let pages_needed = align_up(seg_end, page_size) / page_size - page_base / page_size;
        for k in 0..pages_needed {
            let pv = page_base
                .checked_add(k * page_size)
                .ok_or(SpawnError::OutOfVaddrSpace)?;
            let dst_offset = if k == 0 { io } else { 0 };
            // 进入本页前段内已拷贝字节数（k=0 为 0；saturating 防 io>k*PAGE）。
            let seg_bytes_before = (k * page_size).saturating_sub(io);
            let copy_len = seg
                .filesz
                .saturating_sub(seg_bytes_before)
                .min(page_size - dst_offset);
            let src_file_offset = (seg.offset - io)
                .checked_add(k * page_size)
                .ok_or(SpawnError::OutOfVaddrSpace)?;
            // 同页多段：合并权限（页级粒度）；合并后 W+X → 拒绝。
            if let Some(existing) = pages.iter_mut().find(|p| p.vaddr == pv) {
                existing.read |= flags.read;
                existing.write |= flags.write;
                existing.exec |= flags.execute;
                if existing.is_wx_violation() {
                    return Err(SpawnError::WxViolation);
                }
                continue;
            }
            pages.push(PageEntry {
                vaddr: pv,
                phys: 0,
                size: page_size,
                read: flags.read,
                write: flags.write,
                exec: flags.execute,
                src_file_offset,
                dst_offset,
                copy_len: copy_len as usize,
            });
        }
    }
    pages.sort_by_key(|p| p.vaddr);
    Ok(pages)
}

/// WP-37.4（接线）：ET_DYN 服务的重定位后镜像（load_verified_elf 的
/// staging 输出）——帧拷贝数据源（重定位已应用）；`min_loaded` =
/// 装载后最低虚拟地址（staging[k] = 装载后地址 min_loaded+k 的字节）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelocatedImage {
    /// 重定位后平坦镜像（装载后虚拟地址空间）。
    pub bytes: Vec<u8>,
    /// 镜像首字节对应的装载后虚拟地址。
    pub min_loaded: u64,
}

/// 单个服务的 spawn 计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnedService {
    /// 服务名（bundle manifest）。
    pub name: String,
    /// 入口虚拟地址（ELF entry；ET_DYN = bias 平移后）。
    pub entry: u64,
    /// 栈顶虚拟地址（栈向下增长）。
    pub stack_top: u64,
    /// 栈大小。
    pub stack_size: u64,
    /// IPC 端点标识（root-task ↔ 服务的通信通道）。
    pub ipc_ep: u64,
    /// 页表条目（W^X 保证）。
    pub pages: Vec<PageEntry>,
    /// WP-37.4：ET_DYN 服务的重定位镜像（None = ET_EXEC，帧从 bundle blob
    /// 拷贝）。
    pub relocated: Option<RelocatedImage>,
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

/// 计划实际内存占用（KiB，向上取整）：Σ(ELF 段页 + 栈 + IPC buffer 页)。
/// WP-37.2：enforcement 预算来源——plan 已解析真实 ELF 段/栈大小，
/// 不猜测；manifest ResourceBudget 注入 wire 后替换调用方配额策略。
/// 注：root 镜像由 elfloader 加载（不经 root-task 记账），不在预算内。
pub fn plan_mem_kb(plan: &SpawnPlan, page_size: u64) -> usize {
    let mut total: u64 = 0;
    for svc in &plan.services {
        for p in &svc.pages {
            total = total.saturating_add(p.size);
        }
        total = total.saturating_add(svc.stack_size);
        // 每服务一个 IPC buffer 页（configure_service 映射，4 KiB）。
        total = total.saturating_add(page_size);
    }
    // KiB 向上取整（任何剩余页都按 1 KiB 记账，宁多勿漏）。
    usize::try_from(total.saturating_add(1023) / 1024).unwrap_or(usize::MAX)
}

/// 向上对齐到页大小。
pub const fn align_up(v: u64, align: u64) -> u64 {
    (v + align - 1) & !(align - 1)
}

/// WP-37.4：ET_DYN staging 镜像上限（8 MiB；防御恶意稀疏段 OOM——
/// bundle 本身受 BOOT-007 2 MiB 窗口约束，此为独立防御上限）。
pub const MAX_ETDYN_STAGING: u64 = 8 * 1024 * 1024;

/// WP-37.4：ET_DYN 服务实例间 bias 步长（双 bias 运行证据：同一 PIE
/// 两个实例分别装载到 `dyn_bias + i*STRIDE`，均正确运行）。
pub const ET_DYN_BIAS_STRIDE: u64 = 0x40000; // 256 KiB（覆盖 ~50 KiB 服务）

/// 为单个 ELF 生成 spawn 计划（页表 + 入口 + 栈 + EP）。
fn plan_one(
    name: &str,
    elf: &LoadedElf,
    _vspace_base: u64,
    page_size: u64,
    stack_size: u64,
    ipc_ep: u64,
) -> Result<SpawnedService, SpawnError> {
    if stack_size % page_size != 0 {
        // 栈大小必须页对齐（K1.2 按页映射栈，非对齐会少映射栈底部分）。
        return Err(SpawnError::PlanMismatch);
    }
    // 30 RT-006：段区间重叠/回绕校验（落点统一——复用 elf-loader
    // validate_segments 纯函数，禁止两处各写）。
    eneros_elf_loader::LoadedElf::validate_segments(elf.segments())
        .map_err(|_| SpawnError::SegmentOverlap)?;
    // WP-37.4：段 → 页计划（floor 页基 + 同页合并 + ET_EXEC 拷贝三要素；
    // 非对齐末段（lld 后段紧随前段 filesz 尾）不再拒绝）。
    let pages: Vec<PageEntry> = plan_pages(elf.segments(), page_size)?;
    // 栈区起点 = 页模型末端（各页 ceil 最大值；乱序段防护同旧语义）。
    let mut max_seg_end = 0u64;
    for seg in elf.segments() {
        let end = seg
            .vaddr
            .checked_add(seg.memsz)
            .ok_or(SpawnError::OutOfVaddrSpace)?;
        max_seg_end = max_seg_end.max(align_up(end, page_size));
    }
    let vaddr = max_seg_end;
    // WP-37.3（复盘修复）：栈底与 ELF 段之间保留一个 **guard 页**（未映射）——
    // 栈向下溢出访问 guard → 可归因 VM fault，不会落入（可能可执行的）
    // ELF 段。guard 页 [stack_base - page_size, stack_base) 不在 pages
    // 也不在栈映射范围（configure_service 从 stack_top 向下映射 stack_size
    // 字节，最低映射页 = stack_base），天然未映射。
    let stack_base = vaddr
        .checked_add(page_size)
        .ok_or(SpawnError::OutOfVaddrSpace)?;
    let stack_top = stack_base
        .checked_add(stack_size)
        .ok_or(SpawnError::OutOfVaddrSpace)?;
    Ok(SpawnedService {
        name: String::from(name),
        entry: elf.entry(),
        stack_top,
        stack_size,
        ipc_ep,
        pages,
        relocated: None, // ET_EXEC：帧从 bundle blob 拷贝（configure_service）
    })
}

/// WP-37.4（接线）：ET_DYN 服务计划——经统一 `load_verified_elf` 布局
/// （bias 平移 + staging 重定位），产出携带重定位镜像的 SpawnedService。
/// 约束：装载后段 vaddr 必须页对齐（映射约束，与 ET_EXEC 一致）；栈布局
/// 与 guard 语义同 [`plan_one`]。
fn plan_one_dyn(
    name: &str,
    blob: &[u8],
    policy: &eneros_elf_loader::ElfLoadPolicy,
    page_size: u64,
    stack_size: u64,
    ipc_ep: u64,
) -> Result<SpawnedService, SpawnError> {
    if stack_size % page_size != 0 {
        return Err(SpawnError::PlanMismatch);
    }
    // 预算 staging 大小：按装载跨度（bias + 段区间 min/max）。
    let elf = LoadedElf::parse(blob).map_err(|_| SpawnError::BadElf)?;
    let mut min_loaded = u64::MAX;
    let mut max_loaded_end = 0u64;
    for seg in elf.segments() {
        let start = policy
            .dyn_bias
            .checked_add(seg.vaddr)
            .ok_or(SpawnError::OutOfVaddrSpace)?;
        let end = start
            .checked_add(seg.memsz)
            .ok_or(SpawnError::OutOfVaddrSpace)?;
        min_loaded = min_loaded.min(start);
        max_loaded_end = max_loaded_end.max(end);
    }
    // staging 按页对齐跨度分配（review should-fix：configure_service 对
    // ET_DYN 整页拷贝——页尾 BSS 必须落在镜像内且已清零；load_verified_elf
    // 内部 staging.fill(0) 保证）。上限保护：跨度超 MAX_ETDYN_STAGING 拒绝
    // （防恶意稀疏段 OOM）。
    let raw_span = max_loaded_end - min_loaded;
    if raw_span > MAX_ETDYN_STAGING {
        return Err(SpawnError::EtDynStagingTooLarge);
    }
    let span = align_up(raw_span, page_size);
    let mut staging = alloc::vec![0u8; span as usize];
    let image =
        LoadedElf::load_verified_elf(blob, policy, &mut staging).map_err(|_| SpawnError::BadElf)?;
    // 段页（WP-37.4：页模型——floor 页基 + 同页合并；非对齐段不再拒绝；
    // 内容整页从 staging 拷，src/dst/copy_len 三要素对 ET_DYN 无意义）。
    let mut pages: Vec<PageEntry> = Vec::new();
    let mut max_seg_end = 0u64;
    for seg in &image.segments {
        let end = seg
            .vaddr
            .checked_add(seg.memsz)
            .ok_or(SpawnError::OutOfVaddrSpace)?;
        max_seg_end = max_seg_end.max(align_up(end, page_size));
        let page_base = seg.vaddr & !(page_size - 1);
        let pages_needed = align_up(end, page_size) / page_size - page_base / page_size;
        for k in 0..pages_needed {
            let pv = page_base
                .checked_add(k * page_size)
                .ok_or(SpawnError::OutOfVaddrSpace)?;
            if let Some(existing) = pages.iter_mut().find(|p| p.vaddr == pv) {
                existing.read |= seg.permissions.read;
                existing.write |= seg.permissions.write;
                existing.exec |= seg.permissions.execute;
                if existing.is_wx_violation() {
                    return Err(SpawnError::WxViolation);
                }
                continue;
            }
            pages.push(PageEntry {
                vaddr: pv,
                phys: 0,
                size: page_size,
                read: seg.permissions.read,
                write: seg.permissions.write,
                exec: seg.permissions.execute,
                src_file_offset: 0, // ET_DYN：staging 整页拷贝，源字段无意义
                dst_offset: 0,
                copy_len: 0,
            });
        }
    }
    pages.sort_by_key(|p| p.vaddr);
    // 栈 + guard（同 plan_one 语义）。
    let stack_base = max_seg_end
        .checked_add(page_size)
        .ok_or(SpawnError::OutOfVaddrSpace)?;
    let stack_top = stack_base
        .checked_add(stack_size)
        .ok_or(SpawnError::OutOfVaddrSpace)?;
    Ok(SpawnedService {
        name: String::from(name),
        entry: image.entry,
        stack_top,
        stack_size,
        ipc_ep,
        pages,
        relocated: Some(RelocatedImage {
            bytes: staging,
            min_loaded,
        }),
    })
}

/// 生产唯一 spawn 计划入口（蓝图 67 WP-67.5 死入口收敛：原
/// `plan_spawn` / `plan_spawn_dyn_aware` / `plan_spawn_with_enforcement`
/// 三死入口与降级循环双写已删——对拍测试
/// `bp67_parity_plan_spawn_vs_dyn_aware_on_et_exec` 证明两版对相同
/// ET_EXEC 输入产出结构级全等的 plan/Err 后收敛为本单实现）。
/// ET_DYN 服务经统一 `load_verified_elf`（bias + 重定位），ET_EXEC 服务
/// 走 [`plan_one`]（链接地址）。`dyn_bias` = ET_DYN 装载基址（页对齐；
/// 独立 VSpace 下可与其他服务重叠）。
/// 蓝图 40：按名栈覆盖（页）机制保留在 `overrides` 参数；原
/// `CONTROL_SERVICE_STACK_PAGES` 空表已随本卷删除（EnvelopeIngress 大
/// 缓冲改堆持有后不再需要覆盖；per-service 栈接线归 42）。
pub fn plan_spawn_dyn_aware_with_stack(
    bundle: &crate::bundle::InitBundle,
    vspace_base: u64,
    page_size: u64,
    default_stack_size: u64,
    ep_base: u64,
    dyn_bias: u64,
    overrides: &[(&str, u64)],
) -> Result<SpawnPlan, SpawnError> {
    plan_spawn_dyn_aware_impl(
        bundle,
        vspace_base,
        page_size,
        default_stack_size,
        ep_base,
        dyn_bias,
        overrides,
    )
}

fn plan_spawn_dyn_aware_impl(
    bundle: &crate::bundle::InitBundle,
    vspace_base: u64,
    page_size: u64,
    stack_size: u64,
    ep_base: u64,
    dyn_bias: u64,
    stack_overrides: &[(&str, u64)],
) -> Result<SpawnPlan, SpawnError> {
    // 1. Root Task 模块。
    let root_module = bundle
        .modules
        .iter()
        .find(|m| m.kind == crate::bundle::MODULE_KIND_ROOT)
        .ok_or(SpawnError::NoRootTask)?;
    let root_elf = LoadedElf::parse(root_module.blob).map_err(|_| SpawnError::BadElf)?;
    if !root_elf.has_executable() {
        return Err(SpawnError::BadElf);
    }
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

    // 2. EL0 服务（WP-37.1：required 集合必须全部成功；optional 降级）。
    // WP-37.4：ET_DYN 服务按实例递增 bias（`ET_DYN_BIAS_STRIDE` 步长，
    // 独立 VSpace 下地址可复用，递增仅为了双 bias 运行证据——同一 PIE
    // 两个实例不同装载基址都正确运行）。
    let mut dyn_policy = eneros_elf_loader::ElfLoadPolicy {
        allow_et_exec: true,
        dyn_bias,
    };
    let mut dyn_instance: u64 = 0;
    let mut services = Vec::new();
    let mut degraded = Vec::new();
    let mut next_vaddr = root.stack_top;
    let mut ep = ep_base + 1;
    for module in &bundle.modules {
        let kind = module.kind;
        if kind == crate::bundle::MODULE_KIND_ROOT {
            continue;
        }
        if kind != crate::bundle::MODULE_KIND_REQUIRED
            && kind != crate::bundle::MODULE_KIND_OPTIONAL
        {
            return Err(SpawnError::UnknownBundleKind);
        }
        // 蓝图 40：按名栈覆盖（控制服务大栈）。
        let module_stack = stack_overrides
            .iter()
            .find(|(name, _)| *name == module.name)
            .map(|(_, pages)| pages * page_size)
            .unwrap_or(stack_size);
        let result = match LoadedElf::parse(module.blob) {
            Ok(elf) if elf.is_et_dyn() => {
                // 每实例独立 bias（首个 = 调用方基址；后续按步长递增）。
                dyn_policy.dyn_bias = dyn_bias
                    .checked_add(
                        dyn_instance
                            .checked_mul(ET_DYN_BIAS_STRIDE)
                            .ok_or(SpawnError::OutOfVaddrSpace)?,
                    )
                    .ok_or(SpawnError::OutOfVaddrSpace)?;
                dyn_instance += 1;
                plan_one_dyn(
                    &module.name,
                    module.blob,
                    &dyn_policy,
                    page_size,
                    module_stack,
                    ep,
                )
            }
            Ok(elf) => plan_one(&module.name, &elf, next_vaddr, page_size, module_stack, ep),
            Err(_) => Err(SpawnError::BadElf),
        };
        match result {
            Ok(plan) => {
                next_vaddr = plan.stack_top;
                ep += 1;
                services.push(plan);
            }
            Err(e) => {
                if module.requirement() == crate::bundle::Requirement::Required {
                    return Err(if e == SpawnError::BadElf {
                        SpawnError::RequiredElfFailed {
                            name: module.name.clone(),
                        }
                    } else {
                        e
                    });
                }
                degraded.push(module.name.clone());
            }
        }
    }
    if services.is_empty() {
        return Err(SpawnError::NoServices);
    }
    if services.len() < 2 {
        return Err(SpawnError::TooFewServices);
    }
    if !degraded.is_empty() {
        crate::println!(
            "[eneros-root-task] WP-37.1 optional degraded: {} (capability unavailable)",
            degraded.join(",")
        );
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
    /// SPEC-B WP-B.1：已映射设备帧登记 `(paddr, size_bits, vaddr)`——同物理
    /// 页（如 2MiB 大区帧内子设备）二次请求直接去重复用，避免对同一 device
    /// untyped 重复 Retype 撞 freeIndex 落错页（QEMU virt 大区根因，实测
    /// pl011 取帧错位 → UART 翻译故障）。固定 8 槽（当前 ≤3 处使用）。
    pub dev_maps: [Option<(u64, u64, u64)>; 8],
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
        // BOOT-007 保留窗拉黑（2026-09-23 审计复核施工）：与保留窗重叠的
        // untyped 标记 blocked，不参与任何分配——但**必须保留在 descs 列表
        // 里**：分配器 cap 槽号 = cap_base + 列表索引，删除中间项会使后续
        // 所有 untyped 的 cap 槽号错位 → Retype 打到错误 cap（QEMU 实弹：
        // spawn FAILED Sel4(InvalidArgument)，首版冒烟即咬）。窗内写权不可
        // 由任何下游校验检出（模块 sha256 表与 blob 同窗自参照）。
        let mut blocked = Vec::new();
        let mut window_blacklisted: u32 = 0;
        for i in 0..reader.untyped_count() {
            if let Some(d) = reader.untyped_at(i) {
                let is_blocked = overlaps_reserved_window(d.paddr, d.size_bits);
                if is_blocked {
                    window_blacklisted += 1;
                }
                // 非证据性调试输出（untyped 枚举）：仅 debug 构建打印——
                // release 下 61 行串口输出拖慢 QEMU boot（200s 预算内
                // 12 服务 spawn 超时，37 实测）。拉黑项不打印（不代表可用）。
                if !is_blocked {
                    if cfg!(debug_assertions) {
                        crate::println!(
                            "[eneros-root-task] DBG untyped[{}] cap={} paddr=0x{:x} size_bits={} dev={}",
                            i,
                            reader.untyped_start() + i,
                            d.paddr,
                            d.size_bits,
                            d.is_device
                        );
                    } else if d.is_device {
                        // 46.5 诊断（release）：仅打印 device untyped（通常 ≤10 行），
                        // 定位 virtio-mmio 区域粒度 → Retype freeIndex 语义下设备帧
                        // 实际落点。
                        crate::println!(
                            "[eneros-root-task] DEV-UNTYPED[{}] paddr=0x{:x} size_bits={}",
                            i,
                            d.paddr,
                            d.size_bits
                        );
                    }
                }
                descs.push(d);
                blocked.push(is_blocked);
            }
        }
        // 保留窗拉黑证据行（release 也打印；单行，boot 预算不敏感）。
        crate::println!(
            "[eneros-root-task] BOOT-007 reserved-window untyped blacklisted: n={}",
            window_blacklisted
        );
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
            untyped: UntypedAllocator::with_blocked(reader.untyped_start(), descs, blocked),
            caps: slots,
            root_tcb: caps.tcb,
            root_cnode: caps.cnode,
            root_vspace: caps.vspace,
            asid_pool: caps.asid_pool,
            dev_maps: [None; 8],
        }
    }
}

/// root CSpace 容量上限。41（CV-41-1）：内核 `KernelRootCNodeSizeBits`
/// 12→13（4096→8192 槽）——34 个 EL0 服务 spawn 期 NoCapSlot（QEMU
/// 实测 root_slot=4089 耗尽）；**70.15（2026-09-14）**：13→14（8192→16384
/// 槽）——长窗映像使 root 槽位耗到 8191/8192 边缘，一处页界移动即
/// `spawn FAILED: NoCapSlot`（svc-mgmt 段，实测；`build-rust.sh` 的
/// `KernelRootCNodeSizeBits` 同步 14）。本常量必须与内核配置一致，否则
/// 分配器提前拒绝（fail-closed 挂起而非越界）。
pub const ROOT_CNODE_SLOTS: u64 = 1 << 14;

/// BOOT-007 保留窗重叠判定（2026-09-23 审计复核施工）。
///
/// seL4 上游 `create_untypeds`（kernel/boot.c）把 normal 内存区间之间的
/// 洞按 **device untyped** 发放：overlay 生效时保留窗
/// [0x42000000,0x44000000) 恰为 avail 两段之间的洞（70.6 日志
/// DEV-UNTYPED[21] paddr=0x42000000 size_bits=25 实证）；overlay 缺失时
/// 该窗混入 normal untyped（DMA bump 分配可确定性吃进）。两种形态都
/// 必须在分配入口拉黑：窗内写权（retype 帧→映射）不可由任何下游校验
/// 检出——模块 sha256 表与 blob 同窗自参照、DTB 仅 FDT magic+totalsize、
/// 身份 capsule 仅 CRC32。
#[cfg_attr(not(target_os = "none"), allow(dead_code))] // host 面仅测试使用
pub(crate) fn overlaps_reserved_window(paddr: u64, size_bits: u64) -> bool {
    let start = ENEROS_INIT_BUNDLE_PHYS;
    let end = ENEROS_INIT_BUNDLE_PHYS + ENEROS_RESERVED_WINDOW_LEN;
    let desc_end = paddr + (1u64 << size_bits);
    paddr < end && desc_end > start
}

/// 70.6（WP-70.6.3）：root 管理端点（root 侧 poll cap；sel4_spawn_services
/// 阶段 1.5 装配）。0 = 未装配（主循环跳过管理轮询）。
#[cfg(target_os = "none")]
pub static ROOT_MGMT_EP: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

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
///
/// `seL4_TCBBits`=11（2 KiB）：aarch64 默认——12（4 KiB）仅当
/// HARDWARE_DEBUG_API / ARM_HYP_ENABLE_VCPU_CP14 / HYP+SMP+BENCHMARK
/// 开启（本构建全关，见 top-rust kernel gen_config）。旧值 12 使记账每
/// TCB 超前内核 freeIndex 2 KiB（内核 getObjectSize 对 TCB 忽略用户
/// size_bits，固定按 11 消费）——34 服务累积 ~68 KiB 漂移，DMA 池
/// phys 记在真实帧上方 → virtio-blk 请求超时（41 第三轮根因，QEMU
/// monitor 实测 0x409ac000 空、desc/avail 未落位）。
pub const OBJ_BITS_TCB: u64 = 11; // 2 KiB（seL4_TCBBits）
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

/// WP-37.1 #8：就绪 notification 的服务 CSpace 槽号（0=ep，1=fault_ep，
/// 2=ready ntfn；服务 init 完成后 `seL4_Signal(2)` 上报就绪）。
pub const READY_NTFN_CSPACE_SLOT: u64 = 2;

/// 方案 A 临时窗口基址（root-task 自身 VSpace，1 GiB 区域；与 root 镜像
/// 0x4000_0000 区域不冲突；L0/L1 页表由 elfloader 提供，spawn 补 L2/L3）。
pub const WINDOW_BASE: u64 = 0x8000_0000;

/// 服务地址空间基址（main.rs 引用本常量——SPEC-R4 WP-R4.3 单源，
/// 历史"与 main.rs VSPACE_BASE 一致"注释互认双写已收编）。
///
/// 0x200000 = 服务 ELF 链接基址（aarch64-unknown-none 默认）：ET_EXEC
/// 固定地址代码的绝对引用（跳转/数据）指向链接地址，映射必须与链接一致，
/// 否则服务执行即 VMFAULT（QEMU 实测 type=1 pc=0x203554）。每个服务
/// 独立 VSpace，虚拟地址区间可与其他服务/root-task 相同（页表隔离）。
pub const SVC_VSPACE_BASE: u64 = 0x200000;

/// 服务静态优先级（seL4 静态优先级调度，低于 root-task）。
pub const SVC_PRIORITY: u64 = 10;

// ============================================================================
// 41（WP-41 存储句柄移交，CVR-39-02 剩余面）：svc-storage 设备移交常量
// ============================================================================

/// virtio-blk 数据盘 MMIO 槽（QEMU virt 槽 2：enerosdata 数据盘；41 QEMU
/// 实测 slot=2 blk bound）。物理地址 = VIRTIO_MMIO_REGION_BASE + slot*0x200。
pub const STORAGE_MMIO_SLOT: u64 = 2;
/// virtio-mmio 区域基址（QEMU virt 板级事实；单源 = handover-abi——
/// SPEC-R4 WP-R4.3，历史 spawn/driver_probe/entropy 三抄已收编）。
pub const STORAGE_VIRTIO_REGION_BASE: u64 = eneros_handover_abi::VIRTIO_MMIO_REGION_BASE;
/// virtio-mmio 槽步长（legacy 传输；单源 = handover-abi）。
pub const STORAGE_MMIO_SLOT_STRIDE: u64 = eneros_handover_abi::VIRTIO_MMIO_SLOT_STRIDE;
// SPEC-B/C（2026-08-28）：移交 vaddr 常量单一事实源 = abi/handover-abi
//（历史与 drv-svc blk_backend 同值不同名双源，注释互称"单一事实源"——
// eipc_grants 孪生病收尾，本卷治常量双源；Handover 结构/表驱动为 C.3 续作）。
// SPEC-R4 WP-R4.3：NET/UART 面同病同治——本文件不再本地定义任何移交
// vaddr/页数常量，NET_MMIO_VADDR/NET_DMA_PAGES/UART_MMIO_VADDR 一并自
// handover-abi re-export（区段不交冻结测试见本文件 1295 起）。
pub use eneros_handover_abi::{
    NET_DMA_PAGES, NET_MMIO_VADDR, STORAGE_DMA_PAGES, STORAGE_DMA_VADDR, STORAGE_MMIO_VADDR,
    UART_MMIO_VADDR,
};

/// svc-storage 设备移交句柄（spawn 时构造，服务 `_start` X3/X4/X5 接收）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageHandover {
    /// virtio-blk MMIO 页在服务 VSpace 的虚拟地址（X3）。
    pub mmio_vaddr: u64,
    /// DMA 池在服务 VSpace 的虚拟地址（X4）。
    pub dma_vaddr: u64,
    /// DMA 池物理基址（X5；Q_PFN/描述符 addr 写此值）。
    pub dma_phys: u64,
    /// 70.7（WP-70.7.2）：启动盘（slot 0）DMA 池 1:1 基址——**独立连续
    /// 分配**（与数据池无位置耦合），经 initial_registers **X7** 直传
    /// svc-storage（None → X7=0 → 服务侧诚实降级；X7 对 storage 空闲，
    /// UART 移交集互斥；X8=C-ABI 第 9 参不可达 run12/13 实弹）。禁止
    /// "+N 页" 派生：first-fit 落位不保证紧邻（同登记根因）。
    pub boot_dma_vaddr: Option<u64>,
}

/// 计算 svc-storage 的 MMIO 物理地址（QEMU virt 数据盘槽位）。
pub fn storage_mmio_phys() -> u64 {
    STORAGE_VIRTIO_REGION_BASE + STORAGE_MMIO_SLOT * STORAGE_MMIO_SLOT_STRIDE
}

/// 协议服务 VSpace 中 virtio-net MMIO 页的固定虚拟地址 = handover-abi
/// [`NET_MMIO_VADDR`]（SPEC-R4 WP-R4.3 收编，本文件 re-export；每服务
/// VSpace 独立——各服务可复用同一 vaddr 映射各自槽位物理页）。
///
/// 46（WP-46.2 资源占用单一事实源）→ 蓝图 62 卷 62.2（WP-62.2.1）：
/// 槽位表收编 [`crate::wiring::SERVICE_WIRING`] 单一事实源（心跳/栈覆盖/
/// 编排驱动同表），此处仅保留推导面。每协议独占一个 QEMU virtio-mmio 槽
/// （`0x0a000000 + slot*0x200`）；**两个协议服务不得同槽**——槽冲突即
/// 装配失败（wiring 表唯一性断言 + 本文件 mmio vaddr 单测双守卫）。
pub fn proto_net_slot(name: &str) -> Option<u64> {
    crate::wiring::proto_net_slot(name)
}

/// 46.2 资源占用装配校验（冲突即失败，见 `proto_net_slot`）。
#[cfg(test)]
mod proto_net_slot_tests {
    use super::*;

    #[test]
    fn all_protocol_services_map_to_distinct_slots() {
        // 7 个协议服务：modbus-rtu 无 NIC 槽（serial），其余 6 槽互异。
        let services = [
            "svc-modbus-rtu",
            "svc-modbus-tcp",
            "svc-iec104",
            "svc-mqtt",
            "svc-mms",
            "svc-goose",
            "svc-sv",
        ];
        let mut seen = alloc::collections::BTreeSet::new();
        let mut nets = 0;
        for name in services {
            match proto_net_slot(name) {
                None => assert_eq!(name, "svc-modbus-rtu", "仅 modbus-rtu 无槽"),
                Some(slot) => {
                    nets += 1;
                    assert!(
                        (1..=13).contains(&slot),
                        "槽 {slot} 越出 QEMU 配置范围（1..=13；goose=1 首 socket）"
                    );
                    assert!(
                        seen.insert(slot),
                        "槽位冲突：{name} 与已登记服务同槽 {slot} —— 装配失败"
                    );
                }
            }
        }
        assert_eq!(
            nets, 6,
            "6 个网络协议服务各占一槽（goose=1，含 modbus-tcp=8）"
        );
    }

    #[test]
    fn unknown_or_serial_service_gets_no_nic_slot() {
        assert_eq!(proto_net_slot("svc-telemetry"), None);
        assert_eq!(proto_net_slot("svc-modbus-rtu"), None);
    }

    #[test]
    fn net_mmio_vaddr_unique_across_all_slots() {
        // 46.5 二修（QEMU 实弹根因）：页 0 与页 1 是不同物理帧——若映射基址
        // 平置 NET_MMIO_VADDR + 页内偏移（slot*0x200&0xfff），slot1(goose) 与
        // slot9(iec104) 均得 vaddr=0x40020200，后映射遮蔽先者 → virtio RX 挂。
        // 本测试锁定：全部协议槽的 mmio_vaddr（= 区域基 + 全量槽偏移）互异。
        let services = [
            "svc-modbus-tcp",
            "svc-iec104",
            "svc-mqtt",
            "svc-mms",
            "svc-goose",
            "svc-sv",
        ];
        let page_mask = (1u64 << OBJ_BITS_SMALL_PAGE) - 1;
        let mut seen = alloc::collections::BTreeSet::new();
        for name in services {
            let slot = proto_net_slot(name).expect("net service slot");
            let mmio_phys = STORAGE_VIRTIO_REGION_BASE + slot * STORAGE_MMIO_SLOT_STRIDE;
            let mmio_off = mmio_phys - STORAGE_VIRTIO_REGION_BASE;
            let map_vaddr = NET_MMIO_VADDR + (mmio_off & !page_mask);
            let mmio_vaddr = NET_MMIO_VADDR + mmio_off;
            assert!(
                seen.insert(mmio_vaddr),
                "mmio_vaddr 冲突 0x{:x}（{name} 槽 {slot}）—— 跨页帧遮蔽，装配失败",
                mmio_vaddr
            );
            assert!(
                map_vaddr % (1u64 << OBJ_BITS_SMALL_PAGE) == 0,
                "映射基须 4K 对齐: 0x{:x}",
                map_vaddr
            );
        }
    }
}

/// 协议服务 virtio-net 设备移交句柄（46 WP-46.2 各协议共用；spawn 时
/// 构造，服务 `_start` X3/X4/X5 接收）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetHandover {
    /// virtio-net MMIO 页在服务 VSpace 的虚拟地址（X3）。
    pub mmio_vaddr: u64,
    /// DMA 池在服务 VSpace 的虚拟地址（X4；1:1 = 物理基址）。
    pub dma_vaddr: u64,
    /// DMA 池物理基址（X5）。
    pub dma_phys: u64,
}

// ============================================================================
// SPEC-SERIES-2026-08-27/D（WP-D.1）：svc-modbus-rtu UART 移交槽位
// ============================================================================

/// 协议服务 VSpace 中 PL011 MMIO 页的固定虚拟地址 = handover-abi
/// [`UART_MMIO_VADDR`]（SPEC-R4 WP-R4.3 收编，本文件 re-export；svc-modbus-rtu
/// 消费侧经 X7 参数接收，无本地副本）。区段 0x40030000..0x40031000 与
/// STORAGE_DMA_VADDR（0x40010000+9页）、NET_MMIO_VADDR 两页区
/// （0x40020000..0x40022000）互不重叠（单测锁定）。
///
/// SPEC-D WP-D.1：UART 设备移交句柄（spawn 时构造，服务 `_start` **X7**
/// 接收 vaddr；DAQ-1 裁决=寄存器通道——X7 空闲且 MMIO 映射移交与
/// storage/net 的 X3/X4/X5 寄存器先例同族；EIPC 授予式适合"端点调用授权"，
/// 不适合设备帧映射面）。⚠ 写入面预算见 [`INITIAL_REG_COUNT`]：count 按
/// seL4 aarch64 **规范序**计（PC/SP/SPSR + X0..X7 = 11），不等于 X 寄存器号
/// ——D 卷曾误把 count=10 读作"覆盖到 X9"，实际只到 X6，X7 被内核静默截断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UartHandover {
    /// PL011 MMIO 页在服务 VSpace 的虚拟地址（X7；恒为 [`UART_MMIO_VADDR`]）。
    pub mmio_vaddr: u64,
}

/// 服务 → 可移交非-console PL011 物理基址**允许清单**（board-fact 登记制，
/// fail-closed：未登记即不移交）。
///
/// - 登记项只写**非 console** 的 pl011 基址（uart0 = kernel console 永不登记：
///   重配波特率/复位 CR 会打断内核串口输出；移交 console 是事故不是功能）。
/// - 真机 bring-up = owner 在此登记板级非 console pl011 的 reg 基址
///   （C4 注脚：届时 C1 host 全链在板上复跑即为真机验收——程序本体零改码）。
/// - 扁平序号（行内展开序）= 冲突拒绝位图的 bit 号；同实例二次认领在
///   spawn 循环被拒（见 `uart_handover_slot` 单测的冲突语义锁定）。
const UART_HANDOVER_TABLE: &[(&str, &[u64])] = &[
    // ("svc-modbus-rtu", &[0x______]),  // ← 真机填空项（owner 登记）
];

/// 服务 → `(可移交 paddr, 表内扁平序号)`。表参数化供 host 单测注入演练
/// （生产入口恒读 [`UART_HANDOVER_TABLE`]）；每服务取该行首个登记 paddr。
fn uart_handover_slot_in(table: &[(&str, &[u64])], name: &str) -> Option<(u64, usize)> {
    let mut flat = 0usize;
    for (svc, paddrs) in table {
        for p in *paddrs {
            if *svc == name {
                return Some((*p, flat));
            }
            flat += 1;
        }
    }
    None
}

/// 生产入口：读允许清单解析服务的 UART 移交槽。
pub fn uart_handover_slot(name: &str) -> Option<(u64, usize)> {
    uart_handover_slot_in(UART_HANDOVER_TABLE, name)
}

/// SPEC-D WP-D.1 C1 装配校验（host）：UART 移交允许清单 + 占用位图纯逻辑。
#[cfg(test)]
mod proto_uart_slot_tests {
    #![allow(clippy::disallowed_macros)] // 测试断言 panic!（mgmt-cli 先例）
    use super::*;

    /// 允许清单**空表**语义锁定：空表下任何服务都不得获得 UART 移交
    /// （fail-closed：未登记即不移交）。
    #[test]
    fn empty_allowlist_hands_over_nothing() {
        let empty: &[(&str, &[u64])] = &[];
        for name in [
            "svc-modbus-rtu",
            "svc-modbus-tcp",
            "svc-telemetry",
            "svc-config",
        ] {
            assert_eq!(
                uart_handover_slot_in(empty, name),
                None,
                "{name} 在空表下不得有移交"
            );
        }
    }

    /// 允许清单**生产表**语义锁定：今日全空（F-S1 环境公理——QEMU virt
    /// 唯一 pl011 即 kernel console；非 console pl011 的实例化/移交路径
    /// 见执行卡登记，恢复时本断言随之改写）。
    #[test]
    fn production_allowlist_hands_over_nothing_today() {
        for name in [
            "svc-modbus-rtu",
            "svc-modbus-tcp",
            "svc-telemetry",
            "svc-config",
        ] {
            assert_eq!(
                uart_handover_slot(name),
                None,
                "{name} 在生产表下不得有移交"
            );
        }
    }

    /// 2026-08-28 六卷复验回归：写入面 count 必须覆盖 `initial_registers`
    /// 可能写的全部 X 寄存器（x0..x7）。aarch64 规范序 X 不连续（X7=索引
    /// 10）——SPEC-D 曾沿用 count=10，X7 被内核静默截断（QEMU 空表下
    /// X7 恒 0 恰好无症状，真机移交即断链）。本测试锁「X7 落在写入面内」；
    /// 编译期另有 const 断言双保险（见 [`INITIAL_REG_COUNT`]）。70.7 注：
    /// svc-storage 的 boot 池基址复用 X7（UART 集互斥），不新增寄存器——
    /// X8 为 C-ABI 第 9 参不可达（run12/13 实弹）。
    #[test]
    fn initial_reg_count_covers_every_handover_register() {
        for x in 0..=7usize {
            let idx = eneros_sel4_sys::types::reg_x_index(x)
                .unwrap_or_else(|| panic!("x{x} 必须在规范序表内"));
            assert!(
                idx < INITIAL_REG_COUNT,
                "写入面 count={INITIAL_REG_COUNT} 漏 x{x}（规范索引 {idx}）——寄存器移交静默截断"
            );
        }
        // 锚定病灶本尊：X7 规范索引=10（非 X 寄存器号线性映射）。
        assert_eq!(eneros_sel4_sys::types::reg_x_index(7), Some(10));
        assert_eq!(INITIAL_REG_COUNT, 11);
    }

    /// 登记制语义：行内多实例取首个；未登记服务 None；扁平序号按表展开
    /// 序分配（供占用位图索引）。
    #[test]
    fn registered_service_resolves_first_paddr_and_flat_index() {
        let table: &[(&str, &[u64])] = &[
            ("svc-a", &[0x0900_0000]),
            ("svc-b", &[0x0910_0000, 0x0920_0000]), // 行内多实例：取首个
        ];
        assert_eq!(
            uart_handover_slot_in(table, "svc-a"),
            Some((0x0900_0000, 0))
        );
        assert_eq!(
            uart_handover_slot_in(table, "svc-b"),
            Some((0x0910_0000, 1))
        );
        assert_eq!(uart_handover_slot_in(table, "svc-none"), None);
    }

    /// C1 冲突拒绝：同一实例（同一位图序号）第二次认领必须被拒；互异
    /// 实例各自成功；越界序号一律拒绝。目标构建 [`uart_claim`] 复用本
    /// 纯函数核，两侧判定等价。
    #[test]
    fn slot_claim_conflict_rejected_distinct_accepted() {
        let mut taken: u32 = 0;
        // 首次认领 idx=0 成功。
        let next = uart_claim_once(taken, 0).expect("首次认领必须成功");
        taken = next;
        // 同实例（idx=0）二次认领 → 拒绝。
        assert_eq!(uart_claim_once(taken, 0), None, "重复移交同一实例必须被拒");
        // 互异实例（idx=1）→ 成功且位图累积。
        let next = uart_claim_once(taken, 1).expect("第二实例认领必须成功");
        assert_eq!(next.count_ones(), 2, "两实例各占一");
        // 越界（≥32）→ 拒绝。
        assert_eq!(uart_claim_once(0, 32), None);
        assert_eq!(uart_claim_once(0, usize::MAX), None);
    }

    /// UART MMIO 页区与其余设备 VA 区两两不交（次生遮蔽防范——46.5
    /// 同页遮蔽根因的静态防线）。
    #[test]
    fn uart_mmio_vaddr_region_disjoint_from_other_device_regions() {
        let page = PAGE_SIZE;
        let ranges = [
            (
                "storage_dma",
                STORAGE_DMA_VADDR,
                STORAGE_DMA_PAGES as u64 * page,
            ),
            ("net_mmio", NET_MMIO_VADDR, 2 * page),
            ("uart_mmio", UART_MMIO_VADDR, page),
            (
                "storage_boot_identity",
                eneros_handover_abi::STORAGE_BOOT_IDENTITY_VADDR,
                page,
            ),
        ];
        for i in 0..ranges.len() {
            for j in (i + 1)..ranges.len() {
                let (ni, si, li) = ranges[i];
                let (nj, sj, lj) = ranges[j];
                assert!(
                    si + li <= sj || sj + lj <= si,
                    "设备 VA 区重叠: {ni}[{si:#x}+{li:#x}] 与 {nj}[{sj:#x}+{lj:#x}]"
                );
            }
        }
    }
}

/// spawn 循环内的实例占用位图（bit i = 表内扁平序号 i 已移交）。seL4
/// 启动期单线程顺序 spawn——load/store 位图足够；restart 不经此处（映射
/// 随服务地址空间持久，寄存器由 `initial_registers` 从 SpawnedHandle 重建）。
#[cfg(target_os = "none")]
static UART_CLAIMED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// 占位纯函数核：`idx` 空闲则返回置位后的新位图；已占用/越界（≥32）→
/// None。host 单测直接覆盖冲突语义；目标构建 [`uart_claim`] 以同一函数
/// 判定，保证两侧判定等价（单一事实源）。host 非 test 构建无运行时消费方
/// （uart_claim 仅目标构建存在）——dead 允许注记只对这一面生效。
#[cfg_attr(not(target_os = "none"), allow(dead_code))]
fn uart_claim_once(taken: u32, idx: usize) -> Option<u32> {
    if idx >= 32 {
        return None;
    }
    let bit = 1u32 << idx;
    (taken & bit == 0).then_some(taken | bit)
}

/// 尝试占住扁平序号 `idx`（已占用 → None，冲突 fail-closed）。
#[cfg(target_os = "none")]
fn uart_claim(idx: usize) -> Option<()> {
    use core::sync::atomic::Ordering;
    let cur = UART_CLAIMED.load(Ordering::Acquire);
    match uart_claim_once(cur, idx) {
        Some(next) => {
            UART_CLAIMED.store(next, Ordering::Release);
            Some(())
        }
        None => None,
    }
}

/// D 卷目标构建下尝试构造服务的 UART 移交句柄并映射 MMIO 页进其 VSpace。
/// 返回 None = 表中无登记 / 序号越限 / 同实例已被占用 / 映射失败（各分支
/// 打印如实原因行，不静默）。host 构建无 seL4 FFI → 恒 None（纯逻辑测试
/// 经 `uart_handover_slot_in` 覆盖）。
#[cfg(target_os = "none")]
fn try_uart_handover(
    svc_name: &str,
    ctx: &mut SpawnCtx,
    vspace: u64,
    types: &ObjectTypeTable,
    rights: &RightsTable,
) -> Option<UartHandover> {
    let (paddr, idx) = match uart_handover_slot(svc_name) {
        Some(v) => v,
        None => return None,
    };
    if uart_claim(idx).is_none() {
        crate::println!(
            "[eneros-root-task] UART-HANDOVER {svc_name}: CONFLICT idx={idx} 实例已移交 —— 拒绝（fail-closed）"
        );
        return None;
    }
    let frame = match ctx.caps.alloc(1) {
        Some(f) => f,
        None => {
            crate::println!("[eneros-root-task] UART-HANDOVER {svc_name}: NoCapSlot —— 不移交");
            return None;
        }
    };
    // SAFETY: frame 为刚分配的槽；retype_device_one 按 paddr 定位覆盖其上的
    // device untyped（FIX-BUGS-0810 B2），失败即 Err 不产生半成品映射。
    let mapped = retype_device_one(ctx, types.arm_frame, OBJ_BITS_SMALL_PAGE, paddr, frame)
        .and_then(|()| {
            let perm = VmPermissions::DEVICE_RW_NX;
            map_frame_with_pagetables(
                ctx,
                vspace,
                frame,
                UART_MMIO_VADDR,
                perm.cap_rights(rights),
                perm.vm_attributes(),
            )
        });
    match mapped {
        Ok(()) => {
            crate::println!(
                "[eneros-root-task] UART-HANDOVER {svc_name}: mmio phys=0x{paddr:x} -> vaddr=0x{UART_MMIO_VADDR:x}"
            );
            Some(UartHandover {
                mmio_vaddr: UART_MMIO_VADDR,
            })
        }
        Err(e) => {
            crate::println!(
                "[eneros-root-task] UART-HANDOVER {svc_name}: map FAILED {e:?} —— 不移交"
            );
            None
        }
    }
}

/// svc-storage DMA 帧的平台 RAM 上限（QEMU 1GB：物理 [0x40000000,
/// 0x80000000)——bootinfo untyped 从 0x40000000 起；真机由 DTB 驱动——42）。
pub const STORAGE_DMA_RAM_TOP: u64 = 0x8000_0000;

/// untyped 分配器：从 bootinfo untyped 列表按 size_bits first-fit 切对象。
///
/// 每个 untyped 维护已切对象数 `used[i]`；`alloc` 返回
/// `(untyped_cap, offset)`——`offset` 是 untyped 内对象序号（retype 的
/// `node_offset` 参数）。
#[derive(Debug, Clone)]
pub struct UntypedAllocator {
    /// untyped cap 区间起点（cap 槽 = cap_base + 索引）。
    cap_base: u64,
    /// **全部** untyped 描述（device/normal 都在，索引必须与 bootinfo
    /// untyped 列表一一对齐——cap 槽号 = cap_base + 索引；device/normal
    /// 的区分由各分配路径的 `is_device` 检查承担）。
    descs: Vec<UntypedDesc>,
    /// BOOT-007 保留窗拉黑位（与 descs 等长；true = 该 untyped 不参与
    /// 任何分配，但保留索引占位——2026-09-23 审计复核施工）。
    blocked: Vec<bool>,
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
        let blocked = alloc::vec![false; descs.len()];
        Self::with_blocked(cap_base, descs, blocked)
    }

    /// 带拉黑位构造：`blocked` 与 `descs` 等长（BOOT-007 保留窗拉黑；
    /// 见 [`overlaps_reserved_window`]）。
    pub(crate) fn with_blocked(cap_base: u64, descs: Vec<UntypedDesc>, blocked: Vec<bool>) -> Self {
        let used = alloc::vec![0u64; descs.len()];
        // release 下失配由 `is_blocked` 的越界缺省 true 兜底（fail-closed），
        // debug 下此断言尽早暴露构造方 bug。
        debug_assert_eq!(descs.len(), blocked.len(), "blocked 必须与 descs 等长");
        Self {
            cap_base,
            descs,
            blocked,
            used,
        }
    }

    /// BOOT-007 拉黑判定唯一实现。越界索引按已拉黑处理（fail-closed：
    /// blocked 与 descs 等长是构造期不变式，失配时宁可拒分配不可放行——
    /// 旧的 `unwrap_or(false)` 在 release 下越界即静默不拉黑，fail-open）。
    fn is_blocked(&self, i: usize) -> bool {
        self.blocked.get(i).copied().unwrap_or(true)
    }

    /// `alignUp(used[i], obj_size)`（seL4 freeIndex 先按对象大小对齐再递增）。
    ///
    /// 与 [`Self::fit`] 共用**红线 6 守卫**（`size_bits ≥ 64` 时
    /// `1u64 << size_bits` 在 debug 下 panic、release 下按 64 取模回绕）。
    /// 对齐半边单独可取，是因为 DMA 路径的 RAM-top 跳过判据必须在**提交
    /// 记账之前**看到落点（`phys = paddr + aligned`）——定义点仍唯一，
    /// 两处调用之间 `used[i]` 不变，故 [`Self::fit`] 重算结果与本函数恒等。
    fn align_up(&self, i: usize, obj_size: u64) -> Option<u64> {
        if self.descs[i].size_bits >= u64::BITS as u64 {
            return None; // 超 64 位的对象不存在；防移位溢出（红线 6）
        }
        self.used[i]
            .checked_add(obj_size - 1)
            .map(|v| v & !(obj_size - 1))
    }

    /// freeIndex 记账单源（seL4 `decodeUntypedInvocation` 对齐律；蓝图 76
    /// 卷三 WP-76.3.3）：capacity 计算（含红线 6 守卫）→ `alignUp(used,
    /// obj_size)` → `aligned + need ≤ capacity` → `used` 递增，返回对齐后
    /// 的字节偏移。
    ///
    /// 收编前三处（`alloc`/`alloc_contiguous_pages`/`alloc_cap_table`）各抄
    /// 一遍同型记账，其中 `alloc_contiguous_pages` **漏了红线 6 守卫**
    /// （体检 R17 候选 7 坐实；`alloc`/`alloc_cap_table` 的 `continue` 分支
    /// 在收编前由守卫兜住，故缺口只在 DMA 路径）。守卫进本函数后，「漏写
    /// 守卫」在结构上不再可表示。
    ///
    /// `obj_size` = 对齐粒度、`need` = 实际占用量——两参数分离是 contiguous
    /// 的本义（粒度 = 页、占用 = count × 页）；bump 型单对象调用给
    /// `need == obj_size`。
    ///
    /// **跳过判据留在调用方**（`is_blocked` / `is_device` / `size_bits`
    /// 充分性 / RAM-top / 物理覆盖 / CNode 槽位 bits）——它们是各分配点的
    /// 策略，不是记账模型。[`Self::alloc_device_at`] 同样不并入：它做的是
    /// 「标记已用」（`used = max(used, off+size)`，物理定位分配），不是
    /// 「切下一块」（蓝图 76 卷三 §8 登记不修项）。
    fn fit(&mut self, i: usize, obj_size: u64, need: u64) -> Option<u64> {
        let aligned = self.align_up(i, obj_size)?;
        let capacity = 1u64 << self.descs[i].size_bits;
        if aligned.checked_add(need).is_some_and(|end| end <= capacity) {
            self.used[i] = aligned + need;
            Some(aligned)
        } else {
            None
        }
    }

    /// 分配覆盖指定物理地址的 **device** 对象（FIX-BUGS-0810 B2 两件套之二）。
    ///
    /// [`alloc_device`]（first-fit）**不校验物理地址**——MMIO 映射若用它，
    /// 修好 node_offset 后会把"别的设备的物理页"映射到请求的虚拟地址
    /// （静默乱写，比软件模式更糟）。本函数按物理定位 device untyped
    /// （按物理覆盖检查：请求地址对齐到对象大小、落点须落在该 untyped
    /// 物理区间内；条件反转为 `is_device` 必须为真），
    /// 返回 `(untyped_cap, byte_offset)`；找不到覆盖 `phys` 的 device untyped
    /// 返回 `None`（调用方 fail-closed 走软件模式兜底）。
    ///
    /// 与 [`Self::fit`] 同属 freeIndex 记账面但**不同模**：本函数按物理定位
    /// 分配、把 `used` 抬到 `max(used, off+size)`（「标记已用」），fit 是
    /// bump 型「切下一块」。强行同槽 = 错抽象（蓝图 76 卷三 §8 登记不修项；
    /// 出现第二个「定位+标记」分配点时另抽 `mark_used_at`）。
    pub fn alloc_device_at(&mut self, phys: u64, size_bits: u64) -> Option<(u64, u64)> {
        for i in 0..self.descs.len() {
            let desc = self.descs[i];
            if self.is_blocked(i) {
                continue; // BOOT-007 保留窗拉黑（索引占位保留，见 with_blocked）
            }
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
        if size_bits >= u64::BITS as u64 {
            return None; // 请求侧对象不存在（循环不变量，故提到循环外）
        }
        for i in 0..self.descs.len() {
            let desc = self.descs[i];
            if self.is_blocked(i) {
                continue;
            }
            if desc.is_device || desc.size_bits < size_bits {
                continue;
            }
            let obj_size = 1u64 << size_bits;
            // 记账单源 = [`Self::fit`]（seL4 freeIndex：alignUp(used, obj) +
            // obj ≤ capacity；红线 6 守卫同处）。
            if let Some(aligned) = self.fit(i, obj_size, obj_size) {
                return Some((self.cap_base + i as u64, aligned));
            }
        }
        None
    }

    /// 分配 `count` 页**连续物理**帧（同一 untyped 内连续切；DMA 池要求
    /// 物理连续——virtio 队列 Q_PFN/描述符地址跨页，41 存储句柄移交）。
    ///
    /// 返回 `(untyped_cap, byte_offset, phys_base)`——`phys_base` =
    /// untyped 物理基址 + 对齐偏移（服务侧写 Q_PFN 用；cpu 视图由
    /// 调用方另行映射进服务 VSpace）。
    pub fn alloc_contiguous_pages(&mut self, count: usize) -> Option<(u64, u64, u64)> {
        let obj_size = 1u64 << OBJ_BITS_SMALL_PAGE;
        let need = (count as u64).checked_mul(obj_size)?;
        for i in 0..self.descs.len() {
            let desc = self.descs[i];
            if self.is_blocked(i) {
                continue;
            }
            if desc.is_device || desc.size_bits < OBJ_BITS_SMALL_PAGE {
                continue;
            }
            // 落点判据先于记账：RAM-top 跳过要在 `used` 递增之前看到 phys。
            // [`Self::align_up`] 与随后的 [`Self::fit`] 之间 `used[i]` 不变
            // （两者之间无任何写），故 fit 重算的对齐值与此恒等——对齐算术
            // 的唯一定义点在 align_up，记账的唯一定义点在 fit。
            // 收编前此处是**无守卫**的 `1u64 << desc.size_bits`（红线 6 缺口，
            // 体检 R17 候选 7）：守卫进 align_up/fit 后本函数与另两口同侧。
            let Some(aligned) = self.align_up(i, obj_size) else {
                continue; // size_bits ≥ 64 → 该 untyped 不可用，换下一个
            };
            let phys = desc.paddr.checked_add(aligned)?;
            // 41 存储句柄移交修复：DMA 帧必须落在平台 RAM 内（QEMU
            // 1GB = [0, 0x40000000)）——bootinfo 的 untyped 列表可能含
            // RAM 外区域（QEMU 实测 0x409ac000 超 1GB，virtio-blk 请求
            // 超时——QEMU 无法访问）。真机上限由 DTB memory 驱动（42）。
            if phys >= STORAGE_DMA_RAM_TOP {
                crate::println!(
                    "[eneros-root-task] DBG dma-alloc skip untyped paddr=0x{:x} size_bits={} used=0x{:x} phys=0x{:x} (>= RAM top)",
                    desc.paddr,
                    desc.size_bits,
                    self.used[i],
                    phys
                );
                continue;
            }
            if let Some(aligned) = self.fit(i, obj_size, need) {
                crate::println!(
                    "[eneros-root-task] DBG dma-alloc pick untyped paddr=0x{:x} size_bits={} aligned=0x{:x} phys=0x{:x}",
                    desc.paddr,
                    desc.size_bits,
                    aligned,
                    phys
                );
                return Some((self.cap_base + i as u64, aligned, phys));
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
        // FIX-BUGS-0810 B4：CNode 槽数 = 2^size_bits、每槽 2^5 字节，故
        // size_bits + 5 必须 < 64，否则移位按 64 取模回绕（release 得到错误
        // obj_size，debug 下 panic）——红线 6 的**请求侧**半边（desc 侧半边
        // 单源在 [`Self::align_up`]，随 fit 一同生效）。判据与循环变量无关，
        // 故提到循环外。
        if size_bits.checked_add(5).map_or(true, |b| b >= 64) {
            return None;
        }
        for i in 0..self.descs.len() {
            let desc = self.descs[i];
            if self.is_blocked(i) {
                continue;
            }
            if desc.is_device || desc.size_bits < size_bits {
                continue;
            }
            // CNode 实际大小：槽数 × 每槽字节 = 2^size_bits × 2^SlotBits。
            let obj_size = (1u64 << size_bits) << 5; // seL4_SlotBits = 5
                                                     // 记账单源 = [`Self::fit`]：与 [`Self::alloc`] 同一条 alignUp
                                                     // 律（收编前两处各抄一遍、靠注释声称「完全一致」——现由结构兑现，
                                                     // 分歧只剩 obj_size 推导：页对象 vs CNode 槽）。
            if let Some(aligned) = self.fit(i, obj_size, obj_size) {
                return Some((self.cap_base + i as u64, aligned));
            }
        }
        None
    }

    /// 已分配字节总数（host 测试统计用）。
    pub fn allocated(&self) -> u64 {
        self.used.iter().sum()
    }
}

/// 段权限 → seL4 CapRights（R/W；aarch64 无独立 X cap 位——执行由
/// VMAttributes 的 ExecuteNever 控制，见 [`VmPermissions`]）。
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

/// 页权限类型化（WP-37.3，蓝图 37 §3）：read/write/execute 三态 + 换算
/// seL4 CapRights 与 VMAttributes。唯一页权限事实——禁止 caller 直接
/// 拼 rights/attr（旧实现 exec 位在 `arm_page_map` 层丢失 → 数据页可执行）。
/// WP-37.3（复盘补全）：缓存策略（device 页必须 NonCacheable——
/// 蓝图：MMIO = RW- + XN + Device-nGnRE/平台正确属性）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachePolicy {
    /// 普通可缓存内存（text/rodata/data/stack/IPC）。
    Cacheable,
    /// 设备内存（MMIO；seL4 VMAttributes 不含 PageCacheable 位）。
    NonCacheable,
}

/// WP-37.3（复盘补全）：共享性（模型字段——记录映射意图；AArch64 seL4
/// 用户态 VMAttributes 无 shareability 位，由内核页表属性/装配层处理）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shareability {
    /// 非共享（单核私有）。
    NonShareable,
    /// 内部共享（同簇多核）。
    InnerShareable,
    /// 外部共享（跨簇）。
    OuterShareable,
}

/// 页权限类型化（WP-37.3，蓝图 37 §3）：read/write/execute/cache/shareability
/// 按固定规则换算 seL4 CapRights 与 VMAttributes。唯一页权限事实——禁止
/// caller 直接拼 rights/attr（旧实现 exec 位在 `arm_page_map` 层丢失 →
/// 数据页可执行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmPermissions {
    /// 读。
    pub read: bool,
    /// 写。
    pub write: bool,
    /// 执行（text；执行页不可写，W^X 由 [`Self::is_wx_violation`] 保证）。
    pub execute: bool,
    /// 缓存策略（device 页 NonCacheable）。
    pub cache: CachePolicy,
    /// 共享性（模型字段，装配层解释）。
    pub shareability: Shareability,
}

impl VmPermissions {
    /// ELF text 段：R-X，可缓存，内部共享。
    pub const RX: Self = Self {
        read: true,
        write: false,
        execute: true,
        cache: CachePolicy::Cacheable,
        shareability: Shareability::InnerShareable,
    };
    /// rodata：R-- + XN。
    pub const RO_NX: Self = Self {
        read: true,
        write: false,
        execute: false,
        cache: CachePolicy::Cacheable,
        shareability: Shareability::InnerShareable,
    };
    /// data/bss/heap/stack/IPC：RW- + XN。
    pub const RW_NX: Self = Self {
        read: true,
        write: true,
        execute: false,
        cache: CachePolicy::Cacheable,
        shareability: Shareability::InnerShareable,
    };
    /// MMIO：RW- + XN + NonCacheable（device-nGnRE 语义；shareability 非共享）。
    pub const DEVICE_RW_NX: Self = Self {
        read: true,
        write: true,
        execute: false,
        cache: CachePolicy::NonCacheable,
        shareability: Shareability::NonShareable,
    };

    /// 从 ELF 段 flags 构造（WP-37.3：段权限 → 类型化权限；普通内存默认
    /// 可缓存 + 内部共享）。
    pub fn from_segment_flags(flags: &eneros_elf_loader::SegmentFlags) -> Self {
        Self {
            read: flags.read,
            write: flags.write,
            execute: flags.execute,
            cache: CachePolicy::Cacheable,
            shareability: Shareability::InnerShareable,
        }
    }

    /// W^X 校验：可执行段不得可写（与 [`PageEntry::is_wx_violation`] 同语义）。
    pub fn is_wx_violation(&self) -> bool {
        self.execute && self.write
    }

    /// 换算 seL4 CapRights（aarch64 无独立 X 位）。
    pub fn cap_rights(&self, rights: &eneros_sel4_sys::safe::RightsTable) -> u64 {
        page_rights(self.read, self.write, rights)
    }

    /// 换算 seL4 ARM VMAttributes：非执行页必须带 ExecuteNever（P0-03）；
    /// NonCacheable（device）页不带 PageCacheable 位（蓝图 device-nGnRE）。
    pub fn vm_attributes(&self) -> eneros_sel4_sys::ArmVmAttributes {
        eneros_sel4_sys::safe::arm_vm_attributes_full(
            !self.execute,
            self.cache == CachePolicy::Cacheable,
        )
    }
}

/// 计算 entry 映射后的实际虚拟地址。
///
/// 页模型下解析 entry 实际地址（WP-37.4）。
///
/// 页映射保持虚拟地址语义：非对齐段映射在 floor 页基，entry 的页内偏移
/// 不变 → **entry 虚拟地址即实际地址**（旧实现 `seg_start + (entry -
/// seg.vaddr)` 在页对齐时恰好等于 entry，非对齐时错误且被拒绝）。ET_DYN
/// 的 entry 已由 load_verified_elf bias 平移。仅校验：entry 必须落在某个
/// 可执行页内（fail-closed）。
pub fn entry_mapped(plan_pages: &[PageEntry], elf: &LoadedElf) -> Result<u64, SpawnError> {
    let entry = elf.entry();
    if plan_pages
        .iter()
        .any(|p| p.exec && entry >= p.vaddr && entry < p.vaddr + p.size)
    {
        Ok(entry)
    } else {
        Err(SpawnError::PlanMismatch)
    }
}

// ============================================================================
// 蓝图 40 收尾：EIPC 端点授予表 + 心跳监控表（CVR-39-02 承接——root 向
// EL0 服务授予跨服务端点 capability）。
// ============================================================================

/// EIPC 端点授予表——WP-54.4 起本体下沉 agent-sdk（单一事实源），
/// 此处 re-export 保路径稳定。
pub use eneros_agent_sdk::EIPC_GRANTS;

/// 心跳监控服务面（WP-40.4 隔离面）：授予心跳 notification（服务
/// CSpace 槽 `HEARTBEAT_NTFN_SLOT`）+ root 侧 poll cap。蓝图 62 卷 62.2
/// （WP-62.2.1）：名单收编 [`crate::wiring::SERVICE_WIRING`] 单一事实源
/// （heartbeat 登记；基线名单由 wiring 表结构断言锁定），此处仅保留
/// 授予判定推导面。
pub fn heartbeat_slot_of(name: &str) -> u64 {
    if crate::wiring::is_heartbeat(name) {
        eneros_agent_sdk::HEARTBEAT_NTFN_SLOT
    } else {
        0
    }
}

/// 计算服务的 EIPC 授予（表序）：返回 (X3 掩码, [(槽位, 目标名)])。
/// WP-54.4：委托 agent-sdk 单一事实源——mint 侧与服务侧共享同一推导。
pub fn eipc_grants_for(name: &str) -> (u64, Vec<(u64, &str)>) {
    eneros_agent_sdk::eipc_grants(name)
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
    /// WP-37.1 #8：服务就绪 notification 的 root 侧带 badge 副本
    /// （readiness handshake poll 用；服务 Signal 后 poll 得 sender≠0）。
    pub ready_ntfn: u64,
    /// WP-37.2：服务能力位图（bundle flags 高 16 位——唯一授权来源；
    /// supervisor 据此填充 CapabilityGrantSet，未声明即拒绝）。
    pub capabilities: u32,
    /// WP-37.1：服务必需性（required 阻断 product-ready；optional 降级
    /// 只写 CapabilityUnavailable，不阻断——PRODUCT-READY 汇总判据）。
    pub requirement: crate::bundle::Requirement,
    /// 蓝图 40 收尾：EIPC 授予位掩码（X3）——bit i = 服务 CSpace 槽
    /// `EIPC_EP_BASE_SLOT + i` 持有目标服务 endpoint cap（mint 于
    /// 阶段 1.5，见 [`EIPC_GRANTS`]）。0 = 未授予任何端点。
    pub eipc_mask: u64,
    /// 蓝图 40 收尾：服务 CSpace cnode cap（阶段 1.5 跨服务 mint 目标）。
    pub cnode: u64,
    /// 蓝图 40 收尾：心跳 notification root 侧 poll cap（0 = 该服务
    /// 不参与心跳监控）。服务每处理一条消息 Signal 一次（服务 CSpace
    /// 槽 `HEARTBEAT_NTFN_SLOT`）；supervisor 以"有请求在途却无心跳"
    /// 判定 hang（WP-40.4 隔离面：无 fault 卡死也能 kill）。
    pub heartbeat_ntfn: u64,
    /// 41（WP-41 存储句柄移交）：svc-storage 设备移交句柄（X3/X4/X5
    /// 寄存器参数；restart 时与 spawn 同源重建——MMIO/DMA 映射在
    /// restart 期间保持，物理地址不变）。非 storage 服务为 None。
    pub storage_handover: Option<StorageHandover>,
    /// 43（L2 协议互操作面）：svc-modbus-tcp 的 virtio-net 移交句柄
    /// （X3/X4/X5；同 storage 机制）。非 modbus-tcp 服务为 None。
    pub net_handover: Option<NetHandover>,
    /// SPEC-D WP-D.1：svc-modbus-rtu 的 UART MMIO 移交句柄（X7）。非
    /// 登记服务为 None（今日空表恒 None；restart 同源重建寄存器）。
    pub uart_handover: Option<UartHandover>,
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
    let window_pt = retype_page_tables(ctx, &types, OBJ_BITS_PAGE_TABLE, 2)?;
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
    // 46.5：virtio-mmio 共享页框 cap（跨服务复用）。QEMU virt 该区域是单一
    // device untyped（0x0a000000，size_bits=25），Retype 无 untyped 内偏移，
    // freeIndex 逐页推进——每服务独立 retype 只会拿到"第 k 页"（页基槽号
    // =8k），与目标槽无关。页 0（slot0-7 含 storage/goose）与页 1（slot8-15
    // 含其余 5 协议）各只真实 retype 一次，同页服务经 CNode_Mint 派生帧 cap
    // 复用（见 configure_service 的共享帧逻辑）。
    let mut net_page0_frame: Option<u64> = None;
    let mut net_page1_frame: Option<u64> = None;
    // SPEC-B WP-B.1：整批 spawn 共享一次闸门读数（root 单线程；PL031 在
    // spawn 前已由 rtc_factory 注入）。restart 路径由 service.rs 每次新读。
    let wall_epoch = gated_wall_epoch("spawn");
    for (svc_idx, svc) in plan.services.iter().enumerate() {
        let module = bundle
            .modules
            .iter()
            .find(|m| m.name == svc.name)
            .ok_or(SpawnError::MissingBundleModule)?;
        let elf = LoadedElf::parse(&module.blob).map_err(|_| SpawnError::BadElf)?;
        // WP-37.4：ET_DYN 的 entry 已由 load_verified_elf bias 平移
        //（svc.entry = 装载后入口）；ET_EXEC 的 entry 是链接地址（页
        // 模型下页基 = floor(seg.vaddr)，entry 页内偏移不变 → 直接校验
        // 落页即可）。
        let entry = if svc.relocated.is_some() {
            svc.entry
        } else {
            entry_mapped(&svc.pages, &elf)?
        };
        // 蓝图 40 收尾：X3/X4 由静态授予表推导（eipc_grants_for 与
        // 阶段 1.5 mint 共享同一推导——单一事实源）。
        let (eipc_mask, _) = eipc_grants_for(&svc.name);
        let heartbeat_slot = heartbeat_slot_of(&svc.name);
        // D（Deepening-v1 §D.2）：会话构造**只此一处**，且必须命名字段——旧 14 参
        // 位置表里 fault_ep/ntfn_badge/eipc_mask/heartbeat_slot/wall_epoch 五个
        // 同型 u64 两两相邻（R-DE2），命名字面量让相邻错位在编译期不可表达。
        let configured = configure_service(
            ServiceSpawnSession {
                module,
                entry,
                ctx: &mut *ctx,
                types: &types,
                rights: &rights,
                fault_ep,
                ntfn_badge: svc_idx as u64 + 1, // fault_ep badge（0 保留；supervisor 识别服务用）
                eipc_mask,
                heartbeat_slot,
                wall_epoch,
                net_page0_frame: &mut net_page0_frame,
                net_page1_frame: &mut net_page1_frame,
            },
            svc,
            &elf,
        )?;
        tcb_slots.push(configured.tcb);
        handles.push(SpawnedHandle {
            name: svc.name.clone(),
            tcb: configured.tcb,
            ep: configured.ep,
            // K2.3 fault 检测：root 侧带 badge 的 fault_ep 副本（G-R1 修复，
            // 23 G-I 总出口）——fault IPC 走 fault_ep endpoint，接收方 badge
            // = 持有 cap 的 badge；root 在此 poll 得 sender=badge 识别服务
            //（1368180 的 notification badge 方案错位，QEMU 实测 sender=0）。
            ntfn: configured.root_fault_ep,
            // 22 §2.W1.3：重启寄存器参数（entry 由 ELF 解析；buffer 紧邻
            // 栈顶上方，与 configure_service 的 buffer_vaddr 计算一致）。
            entry,
            stack_top: svc.stack_top,
            buffer_vaddr: align_up(svc.stack_top, PAGE_SIZE),
            // WP-37.1 #8：就绪 notification root 侧 poll cap。
            ready_ntfn: configured.ready_ntfn_root,
            // WP-37.2：能力位图（module 已解析 flags 高 16 位）。
            capabilities: module.capabilities,
            // WP-37.1：必需性（kind → Requirement）。
            requirement: module.requirement(),
            eipc_mask,
            cnode: configured.cnode,
            heartbeat_ntfn: 0,
            storage_handover: configured.storage_handover,
            net_handover: configured.net_handover,
            uart_handover: configured.uart_handover,
        });
    }

    // 阶段 1.5（蓝图 40 收尾，CVR-39-02 承接）：跨服务 EIPC 端点授予 +
    // 心跳 notification。全部服务 ep 在阶段一已知——跨服务 mint 只能在
    // 全集就绪后执行（客户端在 resume 前拿到完整端点集）。
    {
        // SPEC-C 可观测性：授予表总边数随镜像落串口——运行时表与代码
        // 单源一致性直证（新增边缺席排查留给此行）。
        crate::println!(
            "[eneros-root-task] EIPC-GRANT-TABLE edges={} base_slot={}",
            eneros_agent_sdk::EIPC_GRANTS.len(),
            eneros_agent_sdk::EIPC_EP_BASE_SLOT,
        );
        // 70.6（WP-70.6.3）：root 管理端点——retype endpoint 对象，root 侧
        // 持 poll cap（主循环非阻塞轮询 + Poll 暗装 reply cap 处理后
        // Reply），并注入按名解析表：`("svc-mgmt","root-mgmt")` 授予边经
        // 下方标准 mint 循环照常发放（badge=客户端索引+1）。
        let root_mgmt_ep = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
        retype_one(ctx, types.endpoint, OBJ_BITS_ENDPOINT, root_mgmt_ep)?;
        ROOT_MGMT_EP.store(root_mgmt_ep, core::sync::atomic::Ordering::Release);
        crate::println!("[eneros-root-task] MGMT-EP: root management endpoint cap={root_mgmt_ep}");
        let mut eps: Vec<(alloc::string::String, u64)> =
            handles.iter().map(|h| (h.name.clone(), h.ep)).collect();
        eps.push((alloc::string::String::from("root-mgmt"), root_mgmt_ep));
        for (idx, handle) in handles.iter_mut().enumerate() {
            let (_, grants) = eipc_grants_for(&handle.name);
            for (slot, target) in &grants {
                let Some(&(_, target_ep)) = eps.iter().find(|(n, _)| n == target) else {
                    crate::println!(
                        "[eneros-root-task] EIPC-GRANT: {} -> {} SKIPPED (target not spawned)",
                        handle.name,
                        target
                    );
                    continue;
                };
                // SAFETY: handle.cnode 为该服务 CSpace（一阶）；槽位空闲
                //（阶段一仅占用 0/1/2）；target_ep 为有效 endpoint cap；
                // badge=客户端索引+1（服务端归因用）。
                cnode_mint(
                    handle.cnode,
                    *slot,
                    OBJ_BITS_CNODE,
                    ctx.root_cnode,
                    target_ep,
                    64,
                    rights.all,
                    idx as u64 + 1,
                )
                .map_err(SpawnError::Sel4)?;
                crate::println!(
                    "[eneros-root-task] EIPC-GRANT: {} -> {} slot={} badge={}",
                    handle.name,
                    target,
                    slot,
                    idx as u64 + 1
                );
            }
            // 心跳 notification：retype + mint 服务 CSpace 槽
            // HEARTBEAT_NTFN_SLOT + root 侧带 badge poll 副本（与
            // ready_ntfn 同模式；badge=服务索引+1）。
            if heartbeat_slot_of(&handle.name) != 0 {
                let hb = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
                retype_one(ctx, types.notification, OBJ_BITS_NOTIFICATION, hb)?;
                // SAFETY: 服务 CSpace 槽 63 空闲；rights 全权（Signal 用）。
                cnode_mint(
                    handle.cnode,
                    eneros_agent_sdk::HEARTBEAT_NTFN_SLOT,
                    OBJ_BITS_CNODE,
                    ctx.root_cnode,
                    hb,
                    64,
                    rights.all,
                    idx as u64 + 1,
                )
                .map_err(SpawnError::Sel4)?;
                let hb_root = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
                // SAFETY: root cnode 槽空闲；badge=服务索引+1（poll 识别）。
                cnode_mint(
                    ctx.root_cnode,
                    hb_root,
                    64,
                    ctx.root_cnode,
                    hb,
                    64,
                    rights.all,
                    idx as u64 + 1,
                )
                .map_err(SpawnError::Sel4)?;
                handle.heartbeat_ntfn = hb_root;
                crate::println!(
                    "[eneros-root-task] HEARTBEAT: {} slot={} badge={}",
                    handle.name,
                    eneros_agent_sdk::HEARTBEAT_NTFN_SLOT,
                    idx as u64 + 1
                );
            }
        }
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
/// seL4 aarch64 `UserContext` 布局：`pc(0) sp(1) spsr(2) x0(3) x1(4)…x7(10)`
/// （**规范序 X 不连续**，权威映射见 sel4-sys `types.rs` 顶注），写入面
/// count = [`INITIAL_REG_COUNT`]。这是 spawn（`configure_service`）与
/// restart（`restart_service`）共享的唯一构造点——寄存器布局知识不再分散在两处
/// 靠注释同步。
///
/// - `entry`：ELF 入口（PC）。
/// - `stack_top`：栈顶（SP）。
/// - `buffer_vaddr`：IPC buffer 虚拟地址（X1；X0 固定为 0 = 服务 CSpace 槽 0 的 ep cap）。
/// - `ready_ntfn_slot`：就绪 notification 的服务 CSpace 槽号（X2；WP-37.1 #8
///   readiness handshake——服务 init 完成后 `seL4_Signal(slot)` 上报就绪）。
/// - `eipc_mask`：EIPC 授予位掩码（X3；蓝图 40 收尾）——bit i = 槽
///   `EIPC_EP_BASE_SLOT + i` 持目标服务 endpoint cap（0 = 未授予）。
/// - `heartbeat_slot`：心跳 notification 槽（X4；0 = 未授予不心跳）。
///
/// SPEC-B WP-B.1：`wall_epoch` 经 time_gate 闸门的 Unix epoch 秒，统一走
/// **X6** 下发。X3/X4/X5 在 storage/net 设备移交路径已被占用（41/43/46），
/// 不可复用；X6 对全部服务空闲（规范索引 9，写入面 count 见
/// [`INITIAL_REG_COUNT`]）。0 = 无可信
/// 时间，服务侧显式降级（`TIME-EPOCH-MISSING`），绝不静默冒充 1970 基准。
///
/// SPEC-D WP-D.1：`uart` 移交走 **X7**（DAQ-1 裁决=寄存器通道；X7 对全部
/// 服务空闲——storage/net 与 uart 分属互斥服务集，X3-X5 三路分支不受影响；
/// UART 移交服务仍保留通用分支的 X3=EIPC 掩码 / X4=心跳槽，因其同时消费
/// svc-config 授予边拉取协议配置）。0 = 无串口柄 → 服务侧诚实降级壳。
///
/// 70.7（WP-70.7.2）：svc-storage 的 boot 池 1:1 基址走 **X7**（`Some(s)`
/// 臂内 `s.boot_dma_vaddr`，None → 0 = 启动后端不可用诚实降级；X7 对
/// storage 空闲——UART 移交集互斥）。禁止按 "数据池 + N 页" 派生——boot
/// 池独立连续分配（REG-70.7-BOOT-BACKEND-ZEROSIZE 根因：first-fit 落位
/// +9 页而非紧邻 +10 页，派生地址写 ghost 页 → svc-storage FAULT 无限
/// 重启）。亦不可用 X8：aarch64 C-ABI 第 9 参不经寄存器（run12/13 实弹）。
/// 初寄存器写入面 count（`tcb_write_registers` 第三参）：必须覆盖
/// [`initial_registers`] 触达的最高**规范索引**。aarch64 规范序 X 不连续
/// （X7 = `REG_X7` = 10，跟在 PC/SP/SPSR 之后），故 count = 11 而非 X 寄存器
/// 号。SPEC-D WP-D.1 曾沿用 SPEC-B 的 count=10：恰好漏 X7（内核对越界
/// count 静默截断，不报错——真机移交值恒 0，服务侧诚实降级掩盖了断链），
/// 2026-08-28 六卷复验修正。下方编译期断言锁不变式：规范序常量若变或
/// count 被回缩，构建即炸。
pub(crate) const INITIAL_REG_COUNT: usize = 11;

const _: () = assert!(
    INITIAL_REG_COUNT > eneros_sel4_sys::types::REG_X7,
    "INITIAL_REG_COUNT 必须覆盖 X7（REG_X7）——UART 移交与 70.7 boot 池基址经 X7",
);

#[cfg(target_os = "none")]
pub(crate) fn initial_registers(
    entry: u64,
    stack_top: u64,
    buffer_vaddr: u64,
    ready_ntfn_slot: u64,
    eipc_mask: u64,
    heartbeat_slot: u64,
    wall_epoch: u64,
    storage: Option<StorageHandover>,
    net: Option<NetHandover>,
    uart: Option<UartHandover>,
) -> UserContext {
    let mut regs = UserContext::zeroed();
    regs.set_pc(entry);
    regs.set_sp(stack_top);
    regs.set_x(0, 0); // ep cap 在服务 CSpace 槽 0
    regs.set_x(1, buffer_vaddr);
    regs.set_x(2, ready_ntfn_slot);
    match storage {
        // 41：svc-storage 设备移交——X3=MMIO vaddr, X4=DMA vaddr, X5=DMA phys；
        // 70.7：X7=boot 池 1:1 基址（None→0，服务侧诚实降级）。X7 对
        // svc-storage 空闲（UART 移交集与 storage 互斥）；**不可用 X8**——
        // aarch64 C-ABI 第 9 参不走寄存器，_start 形参读不到（run12/13
        // 实弹：X8 已写而服务侧恒 0）。
        Some(s) => {
            regs.set_x(3, s.mmio_vaddr);
            regs.set_x(4, s.dma_vaddr);
            regs.set_x(5, s.dma_phys);
            regs.set_x(7, s.boot_dma_vaddr.unwrap_or(0));
        }
        // 43：svc-modbus-tcp 设备移交（virtio-net）——同 X3/X4/X5 布局。
        None => match net {
            Some(n) => {
                regs.set_x(3, n.mmio_vaddr);
                regs.set_x(4, n.dma_vaddr);
                regs.set_x(5, n.dma_phys);
            }
            // 其他服务：X3=EIPC 掩码, X4=心跳槽, X5=0（未使用）。
            None => {
                regs.set_x(3, eipc_mask);
                regs.set_x(4, heartbeat_slot);
            }
        },
    }
    // SPEC-B WP-B.1：X6=墙钟 epoch 秒（全服务路径统一；不受上面三路分支影响）。
    regs.set_x(6, wall_epoch);
    // SPEC-D WP-D.1：X7=UART MMIO vaddr（0=无移交；与三路分支正交）。
    if let Some(u) = uart {
        regs.set_x(7, u.mmio_vaddr);
    }
    regs
}

/// SPEC-B WP-B.1：读 PL031 当前 Unix epoch 秒并过 [`time_gate::gate`] 合理性
/// 闸门，产出可下发服务的 X6 值 + 串口判定行。
///
/// - `Accept` → 原值，打印 `TIME-EPOCH=<secs>`；
/// - 无 RTC / Stale / Future / Missing 任一臂 → **0** 并打印
///   `TIME-EPOCH-MISSING ... verdict=...`——垃圾时间按缺失显式降级，
///   绝不静默放行。
///
/// spawn 主流程调一次、restart_service 每次重灌前新读一次（重启墙钟前进
/// 判据的数据面）。QEMU 窗口复现缺时臂：`-rtc base=<过去时刻>` 使 PL031
/// 真实读出过期秒值，经完整闸门路径落 MISSING（evidence/spec-2026-08-27-B/
/// run_window.sh；宿主时钟污染零触碰生产代码）。
#[cfg(target_os = "none")]
pub(crate) fn gated_wall_epoch(stage: &str) -> u64 {
    use eneros_config_bundle::schema::time_gate::{self, WallEpochVerdict};
    let raw = eneros_time::rtc_now_secs();
    let (x6, line) = match raw {
        None => (
            0u64,
            alloc::format!("TIME-EPOCH-MISSING stage={stage} source=no-rtc"),
        ),
        Some(secs) => match time_gate::gate(secs) {
            WallEpochVerdict::Accept => (secs, alloc::format!("TIME-EPOCH={secs} stage={stage}")),
            v => (
                0,
                alloc::format!("TIME-EPOCH-MISSING stage={stage} verdict={v:?} raw={secs}"),
            ),
        },
    };
    crate::println!("[eneros-root-task] {}", line);
    x6
}

/// D（Deepening-v1 §D.2）：一次 EL0 服务配置的**输入面**收口——取代旧
/// `configure_service` 的 14 参位置表。
///
/// 构造必须走**命名字段字面量**（全仓唯一构造点在 `sel4_spawn_services`
/// 阶段一循环内）：旧位置表里 `fault_ep / ntfn_badge / eipc_mask /
/// heartbeat_slot / wall_epoch` 五个同型 `u64` 两两相邻（R-DE2——错配一位
/// 即把心跳槽写进 X3、把 EIPC 掩码写进 X4，类型完全合法，编译期无从发现，
/// 只在真机上表现为服务收错柄）。命名字段让任何交换都必须各自写一次字段名
/// ⇒ 参数表内错位不可表达。刻意**不**引入 newtype 包装（§D.4 R-DE2 口径 +
/// YAGNI：包装只加噪声，命名字段已封死错位面）。
///
/// 字段序 = 旧形参序（去掉 `svc`/`elf`——二者留在 `configure_service`
/// 形参位），逐位可审。`net_page0_frame` / `net_page1_frame` 保持
/// `&'a mut Option<u64>`：46.5 共享帧登记是 **out-param**（整批 spawn 循环内
/// 跨服务读写——页 0/页 1 各只真实 retype 一次，同页后续服务 mint 复用），
/// 语义上不属于任何单次配置，故借位挂在会话上、不折进返回值。
#[cfg(target_os = "none")]
struct ServiceSpawnSession<'a> {
    /// bundle 模块（ELF 段页文件数据源；WP-37.2 能力位图/必需性同源于此）。
    /// `BundleModule<'a>` 自带 blob 借位（借用内核映射的 bundle 区，长度
    /// = root-task 全程）；`'a` 取会话借位即可——结构协变，长借位可读作短借位。
    module: &'a crate::bundle::BundleModule<'a>,
    /// ELF 入口（PC；ET_DYN 已由 load_verified_elf bias 平移）。
    entry: u64,
    /// K1.2 spawn 上下文（cap 分配器 + root 固定 cap + untyped 池）。
    ctx: &'a mut SpawnCtx,
    /// seL4 对象类型号表（启动期 resolve 一次）。
    types: &'a ObjectTypeTable,
    /// seL4 权限位表。
    rights: &'a RightsTable,
    /// root 侧 fault handler endpoint cap（服务 CSpace 槽 1 的 mint 源）。
    fault_ep: u64,
    /// fault/ready mint 的 badge（= 服务索引+1；0 保留）。
    ntfn_badge: u64,
    /// X3：EIPC 授予位掩码（0 = 未授予任何端点）。
    eipc_mask: u64,
    /// X4：心跳 notification 槽（0 = 不参与心跳监控）。
    heartbeat_slot: u64,
    /// X6：经 time_gate 闸门的墙钟 epoch 秒（整批 spawn 共享一次读数）。
    wall_epoch: u64,
    /// 46.5：virtio 区域页 0 共享帧 cap（out-param，见结构体文档）。
    net_page0_frame: &'a mut Option<u64>,
    /// 46.5：virtio 区域页 1 共享帧 cap（out-param，同上）。
    net_page1_frame: &'a mut Option<u64>,
}

/// D（Deepening-v1 §D.2）：一次服务配置的**产物**——取代旧 8 元组返回面
/// （R-DE3：散落各处的解构点位序耦合，自此收口于命名字段）。
///
/// 字段与旧元组**逐位对应、语义不变**：服务 TCB / 服务 endpoint / root 侧
/// fault handler ep（同一 endpoint 对象的 root 视角 cap；服务侧在 CSpace 槽 1
/// 持 mint 副本）/ 服务就绪 notification 的 root 侧带 badge 副本（WP-37.1 #8
/// readiness handshake：服务 init 完成后 Signal CSpace 槽 2，root 在此 poll
/// 判定）/ 服务 CSpace cnode（阶段 1.5 跨服务 mint 目标端点用）/ 三路设备
/// 移交句柄（含义与旧 `Option<…Handover>` 完全一致——非该种服务恒 None）。
#[cfg(target_os = "none")]
struct ConfiguredService {
    /// 服务 TCB cap（root CSpace 槽）。
    tcb: u64,
    /// 服务 endpoint cap（root CSpace 槽；root 侧发 IPC 用）。
    ep: u64,
    /// root 侧 fault handler ep 带 badge 副本（→ `SpawnedHandle.ntfn`）。
    root_fault_ep: u64,
    /// 就绪 notification 的 root 侧带 badge 副本（→ `SpawnedHandle.ready_ntfn`）。
    ready_ntfn_root: u64,
    /// 服务 CSpace cnode（阶段 1.5 跨服务 mint 目标）。
    cnode: u64,
    /// 41（WP-41）：svc-storage 设备移交句柄（X3/X4/X5 + 70.7 X7）。
    storage_handover: Option<StorageHandover>,
    /// 43/46：协议服务 virtio-net 移交句柄（X3/X4/X5）。
    net_handover: Option<NetHandover>,
    /// SPEC-D WP-D.1：UART MMIO 移交句柄（X7）。
    uart_handover: Option<UartHandover>,
}

/// R14-D：连续 DMA 池装配两趟纪律单源——先 `alloc_contiguous_pages`
/// 取连续物理 untyped（first-fit），再逐帧 cap alloc + retype，末逐页
/// 1:1 映射（41 virtio 超时根因的两趟纪律）。原 configure_service 内
/// storage/boot/net 三处 ~50 行同构拷贝（2026-09-24 收编）。
///
/// 失败按阶段区分由调用方保留各自串口词形（字节不变）：`Alloc` = 池
/// untyped 不足；`Build` = retype 或映射失败（收编前两态同走 map-FAILED
/// 打印——retype 失败原本也落 "map FAILED" 词形，此处保持同型）。
/// 失败路径的已建能力/映射不回收（与收编前一致）。
#[cfg(target_os = "none")]
enum DmaPoolFail {
    Alloc,
    Build,
}

#[cfg(target_os = "none")]
fn map_contiguous_dma_pool(
    ctx: &mut SpawnCtx,
    vspace: u64,
    types: &ObjectTypeTable,
    rights: &RightsTable,
    pages: usize,
) -> Result<u64, DmaPoolFail> {
    let (untyped, _byte_off, phys_base) = ctx
        .untyped
        .alloc_contiguous_pages(pages)
        .ok_or(DmaPoolFail::Alloc)?;
    let mut dma_frames: Vec<u64> = Vec::new();
    for _k in 0..pages {
        let frame = ctx.caps.alloc(1).ok_or(DmaPoolFail::Build)?;
        retype(
            untyped,
            types.arm_frame,
            OBJ_BITS_SMALL_PAGE,
            1,
            ctx.root_cnode,
            frame,
            0,
            frame,
        )
        .map_err(|_| DmaPoolFail::Build)?;
        dma_frames.push(frame);
    }
    // 1:1 映射（虚拟 = 物理）。
    let perm = VmPermissions::RW_NX;
    for (k, frame) in dma_frames.into_iter().enumerate() {
        map_frame_with_pagetables(
            ctx,
            vspace,
            frame,
            phys_base + (k as u64) * (1u64 << OBJ_BITS_SMALL_PAGE),
            perm.cap_rights(rights),
            perm.vm_attributes(),
        )
        .map_err(|_| DmaPoolFail::Build)?;
    }
    Ok(phys_base)
}

/// 配置单个服务（阶段一）：对象 retype / ASID / 页表 / 页映射与拷贝 /
/// CSpace mint / 寄存器 / 优先级。
///
/// # 返回值
/// [`ConfiguredService`]（D/§D.2 收口：旧 8 元组的命名字段形态，逐位对应）。
///
/// 会话字段在函数首行回落为与旧形参**同名**的局部绑定——函数体逐字迁移，
/// 本候选只动形参表与返回构造两点（不动任何语句、顺序、格式）。
#[cfg(target_os = "none")]
fn configure_service(
    session: ServiceSpawnSession<'_>,
    svc: &SpawnedService,
    elf: &LoadedElf,
) -> Result<ConfiguredService, SpawnError> {
    let ServiceSpawnSession {
        module,
        entry,
        ctx,
        types,
        rights,
        fault_ep,
        ntfn_badge,
        eipc_mask,
        heartbeat_slot,
        wall_epoch,
        net_page0_frame,
        net_page1_frame,
    } = session;
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
    let pt = retype_page_tables(ctx, types, OBJ_BITS_PAGE_TABLE, 3)?;
    for pt_cap in &pt {
        // SAFETY: pt_cap 为刚 retype 的页表；vspace 已绑 ASID；
        // SVC_VSPACE_BASE 1 GiB 对齐（各层级 lookup 槽 0）。
        arm_page_table_map(vspace, *pt_cap, SVC_VSPACE_BASE).map_err(SpawnError::Sel4)?;
    }

    // 6a. ELF 段页（WP-37.4：**页模型**——pages 已按 floor 页基规划，
    // 每页独立 frame 映射 + 拷贝；非对齐段首页经 dst_offset 写入，
    // 与段序无关）。
    // frame 槽无需跟踪：映射即用，服务侧不持有帧 cap。
    // WP-37.4：ET_DYN 服务（svc.relocated=Some）帧数据源 = 重定位镜像
    // （staging 平坦布局，min_loaded 基址）；ET_EXEC 从 bundle blob 按
    // 页的 src_file_offset 拷。
    let relocated = svc.relocated.as_ref();
    for page in &svc.pages {
        // WP-37.3：页权限 → 类型化 VmPermissions（read/write/execute），
        // 换算 rights + VMAttributes（执行页无 XN，数据页强制 ExecuteNever）。
        // 旧实现只传 rights，exec 位在 arm_page_map 层丢失（P0-03）。
        let perm = VmPermissions {
            read: page.read,
            write: page.write,
            execute: page.exec,
            cache: CachePolicy::Cacheable,
            shareability: Shareability::InnerShareable,
        };
        debug_assert!(!perm.is_wx_violation(), "W^X 已在计划层保证");
        let r = perm.cap_rights(rights);
        let attr = perm.vm_attributes();
        let frame = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
        retype_one(ctx, types.arm_frame, OBJ_BITS_SMALL_PAGE, frame)?;
        // 6c. 方案 A：本页内的文件数据经 root 临时窗口拷贝（ADR-0034）。
        // ET_DYN：整页从重定位镜像拷（review should-fix：镜像为页对齐
        // 跨度、BSS 已清零、重定位已应用——不依赖 phdr 顺序的段配对）；
        // ET_EXEC：从 blob 按页 src_file_offset 拷（非对齐首页写入页内
        // dst_offset 处——lld 末段 vaddr 紧随前段 filesz 尾，PIC 后普遍）。
        if let Some(reloc) = relocated {
            // staging 索引 = 页基 - min_loaded（平坦镜像；构造上
            // page.vaddr >= min_loaded，saturating 防御）。
            let slot = page.vaddr.saturating_sub(reloc.min_loaded) as usize;
            let slot_end = slot
                .checked_add(PAGE_SIZE as usize)
                .ok_or(SpawnError::RelocOutOfBounds)?;
            if slot_end > reloc.bytes.len() {
                return Err(SpawnError::RelocOutOfBounds);
            }
            copy_window_bytes(ctx, frame, &reloc.bytes[slot..slot_end], rights)?;
        } else if page.copy_len > 0 {
            copy_segment_via_window_at(
                ctx,
                frame,
                module,
                page.src_file_offset,
                page.dst_offset,
                page.copy_len,
                rights,
            )?;
        }
        // 6d. 映射进服务 VSpace（BSS 余量由 retype 清零保证）。
        // SAFETY: frame 已 retype 且（如有文件数据）已解除窗口映射；
        // vspace 已绑 ASID；page.vaddr 4 KiB 对齐（页模型保证）。
        crate::println!(
            "[eneros-root-task] DBG page_map vspace={} frame={} vaddr=0x{:x} svc='{}' frame_type={} attr=0x{:x}",
            vspace,
            frame,
            page.vaddr,
            svc.name,
            eneros_sel4_sys::safe::debug_cap_identify(frame),
            attr
        );
        arm_page_map(vspace, frame, page.vaddr, r, attr).map_err(SpawnError::Sel4)?;
    }

    // 栈页（stack_size 字节，R+W+NX；stack_top 向下增长）。
    // WP-37.3：栈页强制 ExecuteNever（旧实现无 XN——栈可执行）。
    let stack_perm = VmPermissions::RW_NX;
    let stack_attr = stack_perm.vm_attributes();
    let stack_pages = svc.stack_size / PAGE_SIZE;
    for k in 0..stack_pages {
        let frame = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
        retype_one(ctx, types.arm_frame, OBJ_BITS_SMALL_PAGE, frame)?;
        let vaddr = svc.stack_top - PAGE_SIZE * (k + 1);
        // SAFETY: frame 刚 retype（全零，无需拷贝）；vaddr 页对齐。
        crate::println!(
            "[eneros-root-task] DBG stack_map vspace={} frame={} vaddr=0x{:x} svc='{}' attr=0x{:x}",
            vspace,
            frame,
            vaddr,
            svc.name,
            stack_attr
        );
        arm_page_map(
            vspace,
            frame,
            vaddr,
            stack_perm.cap_rights(rights),
            stack_attr,
        )
        .map_err(SpawnError::Sel4)?;
    }

    // IPC buffer 页（栈顶之后，4 KiB 对齐；K2 消息 > 寄存器 MR 时必需）。
    // WP-37.3：IPC buffer 强制 ExecuteNever（旧实现无 XN）。
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
    arm_page_map(
        vspace,
        buffer,
        buffer_vaddr,
        VmPermissions::RW_NX.cap_rights(rights),
        VmPermissions::RW_NX.vm_attributes(),
    )
    .map_err(SpawnError::Sel4)?;

    // 41（WP-41 存储句柄移交，CVR-39-02 剩余面）：svc-storage 设备移交——
    // virtio-blk 数据盘 MMIO 页 + 9 页连续 DMA 帧映射进服务 VSpace 固定
    // 虚拟地址（STORAGE_MMIO_VADDR / STORAGE_DMA_VADDR；与 drv-svc
    // blk_backend.rs 常量一致——单一事实源）。其他服务无移交（None）。
    //
    // 失败策略：映射失败 → 打印并**继续 spawn**（storage 服务保持无后端
    // → EXECUTE 显式 ST_UNAVAILABLE，fail-closed 不伪造 I/O；设备缺失
    // 不阻断其他服务）。
    let mut storage_handover: Option<StorageHandover> = None;
    if svc.name == "svc-storage" {
        // F70-02（S1）：本次运行槽身份页——root 把可信身份 capsule
        //（版本+CRC，boot-manager 写入；qemu-test 闭包 = 显式合成身份）
        // 逐字节拷入 root 自有帧，以**只读**权限映射到
        // `STORAGE_BOOT_IDENTITY_VADDR`。capsule 缺失/无效 → 全零页
        //（服务侧解码失败 = "运行身份缺失" ⇒ 安装/确认写面关闭，
        // fail-closed——不猜槽、不从 OTA 请求自述取运行槽）。
        //
        // **必须成功映射**：服务侧无条件读该地址（缺页 = FAULT 循环），
        // 资源耗尽与 IPC buffer 同级（spawn 失败 ⇒ 服务不可用，诚实降级）。
        let identity_frame = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
        retype_one(ctx, types.arm_frame, OBJ_BITS_SMALL_PAGE, identity_frame)?;
        let mut identity_page = [0u8; eneros_boot_protocol::IDENTITY_CAPSULE_SIZE];
        // WP-77.2.2：注入面同样只信冻结裁决——Validated 才拷 capsule
        // 字节；Absent/Rejected 一律零页（写面由空区间关闭）。
        let identity_state = match crate::service::validated_identity() {
            Some(capsule) => {
                identity_page.copy_from_slice(&capsule.to_bytes());
                "capsule"
            }
            None => "absent-or-rejected(zeros)",
        };
        copy_window_bytes(ctx, identity_frame, &identity_page, rights)?;
        let identity_perm = VmPermissions::RO_NX;
        map_frame_with_pagetables(
            ctx,
            vspace,
            identity_frame,
            eneros_handover_abi::STORAGE_BOOT_IDENTITY_VADDR,
            identity_perm.cap_rights(rights),
            identity_perm.vm_attributes(),
        )?;
        crate::println!(
            "[eneros-root-task] BOOT-IDENTITY-PAGE: {} → svc-storage vaddr=0x{:x} (RO)",
            identity_state,
            eneros_handover_abi::STORAGE_BOOT_IDENTITY_VADDR
        );
        let mmio_phys = storage_mmio_phys();
        // a) MMIO 设备帧（1 页覆盖 0x200 槽寄存器区）。
        let mmio_frame = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
        // 按物理定位 device untyped（FIX-BUGS-0810 B2 两件套）；失败 → 降级。
        let mmio_ok = (|| -> Result<(), SpawnError> {
            retype_device_one(
                ctx,
                types.arm_frame,
                OBJ_BITS_SMALL_PAGE,
                mmio_phys,
                mmio_frame,
            )?;
            let perm = VmPermissions::DEVICE_RW_NX;
            map_frame_with_pagetables(
                ctx,
                vspace,
                mmio_frame,
                STORAGE_MMIO_VADDR,
                perm.cap_rights(rights),
                perm.vm_attributes(),
            )?;
            Ok(())
        })();
        match mmio_ok {
            Ok(()) => {
                crate::println!(
                    "[eneros-root-task] STORAGE-HANDOVER: mmio phys=0x{:x} -> vaddr=0x{:x}",
                    mmio_phys,
                    STORAGE_MMIO_VADDR
                );
                // 46.5：storage 的 MMIO 页 = virtio 区域页 0（含 slot0-7，
                // storage 槽 2 在页内偏移 0x400）。登记该页框 cap 供同页
                // 服务（svc-goose 槽 1）mint 复用——Retype freeIndex 语义下
                // 页 0 只能真实 retype 一次。
                *net_page0_frame = Some(mmio_frame);
            }
            Err(e) => {
                crate::println!(
                    "[eneros-root-task] STORAGE-HANDOVER: mmio map FAILED {:?} —— storage 无后端（显式 Unavailable）",
                    e
                );
            }
        }
        // b) DMA 池：数据盘 9 页 + 启动盘 9 页（70.7 WP-70.7.2）——
        //    各自独立连续分配（first-fit；物理位置互不要求）。启动池基址
        //    经 initial_registers **X7** 直传 svc-storage；失败 → 打印 +
        //    降级（无后端显式 Unavailable，不阻断其他服务）。
        // R14-D：数据池与启动池两趟纪律经 map_contiguous_dma_pool 单源。
        match map_contiguous_dma_pool(ctx, vspace, types, rights, STORAGE_DMA_PAGES) {
            Ok(phys_base) => {
                crate::println!(
                    "[eneros-root-task] STORAGE-HANDOVER: dma vaddr=0x{:x} phys=0x{:x} pages={} (1:1)",
                    phys_base,
                    phys_base,
                    STORAGE_DMA_PAGES
                );
                let mmio_vaddr = STORAGE_MMIO_VADDR + STORAGE_MMIO_SLOT * STORAGE_MMIO_SLOT_STRIDE;
                // 70.7：启动盘池独立分配（任意物理位置——X7 传基址）。
                let boot_dma_vaddr = match map_contiguous_dma_pool(
                    ctx,
                    vspace,
                    types,
                    rights,
                    eneros_handover_abi::SVC_BOOT_DMA_PAGES,
                ) {
                    Ok(boot_phys_base) => {
                        crate::println!(
                            "[eneros-root-task] BOOT-STORAGE-HANDOVER: dma vaddr=0x{boot_phys_base:x} (1:1) pages={}",
                            eneros_handover_abi::SVC_BOOT_DMA_PAGES
                        );
                        Some(boot_phys_base)
                    }
                    Err(DmaPoolFail::Alloc) => {
                        crate::println!(
                            "[eneros-root-task] BOOT-STORAGE-HANDOVER: alloc FAILED —— 启动后端不可用"
                        );
                        None
                    }
                    Err(DmaPoolFail::Build) => {
                        crate::println!(
                            "[eneros-root-task] BOOT-STORAGE-HANDOVER: map FAILED —— 启动后端不可用"
                        );
                        None
                    }
                };
                storage_handover = Some(StorageHandover {
                    mmio_vaddr,
                    dma_vaddr: phys_base, // 1:1（虚拟 = 物理）
                    dma_phys: phys_base,
                    boot_dma_vaddr,
                });
            }
            Err(DmaPoolFail::Build) => {
                crate::println!(
                    "[eneros-root-task] STORAGE-HANDOVER: dma map FAILED —— storage 无后端（显式 Unavailable）"
                );
            }
            Err(DmaPoolFail::Alloc) => {
                crate::println!(
                    "[eneros-root-task] STORAGE-HANDOVER: dma pool alloc FAILED —— storage 无后端（显式 Unavailable）"
                );
            }
        }
    }
    // 43（L2 协议互操作面）+ 46（WP-46.2 句柄移交 ×6）：协议服务设备
    // 移交——virtio-net 槽 `proto_net_slot`（QEMU `virtio-mmio-bus.<slot>`，
    // phys 0x0a000000 + slot*0x200）+ 9 页连续 DMA 帧。每协议独占一槽
    // （资源占用表 `proto_net_slot` 单测强制互异）。失败 → 打印 + 降级
    // （协议服务无传输 → capability None → EXECUTE ST_UNAVAILABLE，
    // fail-closed 不伪造协议 I/O）。
    let mut net_handover: Option<NetHandover> = None;
    // None 臂 = 无网络槽：对非协议服务（驱动/业务/运营）是**正常装配路径**
    // 而非降级（多数服务走此臂，不打印——串口词形冻结）。"该登记未登记"
    // 的表外静默缺省由 wiring 完备性门在 host 红测拦截：
    // wiring::tests::every_spawned_service_is_wired_or_exempted（63.2
    // WP-63.2.3）——每个被 spawn 的服务必须 ∈ SERVICE_WIRING ∪
    // EXPLICIT_UNWIRED（后者逐项理由），profile roster 直读双向断言。
    if let Some(net_slot) = proto_net_slot(&svc.name) {
        let mmio_phys = STORAGE_VIRTIO_REGION_BASE + net_slot * STORAGE_MMIO_SLOT_STRIDE;
        // a) MMIO 设备帧：页 0（slot0-7）与页 1（slot8-15）各只真实 retype
        // 一次，同页服务经 CNode_Mint 派生帧 cap 复用到各自 VSpace（46.5
        // 根因修复：QEMU virt 该区域是单一 device untyped，Retype 无 untyped
        // 内偏移、freeIndex 逐页推进——每服务独立 retype 必落在"第 k 页"而非
        // 目标槽页；旧实现下只有恰好 k==槽页 的 modbus/storage 侥幸工作。
        // 46.5 二修（QEMU 实弹）：寄存器基址必须**跨页唯一**——页 0 与页 1
        // 是不同物理帧，若都在 NET_MMIO_VADDR 平基址映射 + 页内槽偏移
        // （slot*0x200 & 0xfff），slot1(goose,页0) 与 slot9(iec104,页1) 均得
        // vaddr=0x40020200，后映射者遮蔽先者 → iec104 virtio RX 挂（QEMU
        // 实测两服务 mmio=0x40020200 重合）。修正：页 0 服务映射基
        // NET_MMIO_VADDR，页 1 服务映射基 NET_MMIO_VADDR+0x1000（两页 VA
        // 区 0x40020000..0x40022000）；mmio_vaddr = 基 + 页内槽偏移（全量
        // 区域偏移，唯一）。
        let page_mask = (1u64 << OBJ_BITS_SMALL_PAGE) - 1;
        let page_base = mmio_phys & !page_mask;
        let mmio_off = mmio_phys - STORAGE_VIRTIO_REGION_BASE; // 0..0x1e00，跨两页
                                                               // 该服务独享 VA 映射页基（页 0 → 0x40020000，页 1 → 0x40021000）。
        let map_vaddr = NET_MMIO_VADDR + (mmio_off & !page_mask);
        let mmio_frame = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
        let shared = if page_base == STORAGE_VIRTIO_REGION_BASE {
            net_page0_frame
        } else {
            net_page1_frame
        };
        let mmio_ok = match *shared {
            Some(src) => {
                // 同页后续服务：Mint 派生同页框 cap 再映射进本服务 VSpace
                //（root cnode depth=64，与 fault_mint 同约定；rights 用全权限）。
                cnode_mint(
                    ctx.root_cnode,
                    mmio_frame,
                    64,
                    ctx.root_cnode,
                    src,
                    64,
                    rights.all,
                    0,
                )
                .map_err(SpawnError::Sel4)
                .and_then(|()| {
                    let perm = VmPermissions::DEVICE_RW_NX;
                    map_frame_with_pagetables(
                        ctx,
                        vspace,
                        mmio_frame,
                        map_vaddr,
                        perm.cap_rights(rights),
                        perm.vm_attributes(),
                    )
                })
            }
            None => {
                // 该页首个服务：真实 retype（freeIndex 恰落在本页——storage
                // 页0→slot2、modbus 页1→slot8 为各自该页首个消费）并登记共享 cap。
                let r = (|| -> Result<(), SpawnError> {
                    retype_device_one(
                        ctx,
                        types.arm_frame,
                        OBJ_BITS_SMALL_PAGE,
                        mmio_phys,
                        mmio_frame,
                    )?;
                    let perm = VmPermissions::DEVICE_RW_NX;
                    map_frame_with_pagetables(
                        ctx,
                        vspace,
                        mmio_frame,
                        map_vaddr,
                        perm.cap_rights(rights),
                        perm.vm_attributes(),
                    )?;
                    Ok(())
                })();
                if r.is_ok() {
                    *shared = Some(mmio_frame);
                }
                r
            }
        };
        match mmio_ok {
            Ok(()) => {
                crate::println!(
                    "[eneros-root-task] NET-HANDOVER {}: mmio phys=0x{:x} -> vaddr=0x{:x} (slot {})",
                    svc.name,
                    mmio_phys,
                    map_vaddr,
                    net_slot
                );
            }
            Err(e) => {
                crate::println!(
                    "[eneros-root-task] NET-HANDOVER {}: mmio map FAILED {:?} —— 无传输（显式 Unavailable）",
                    svc.name, e
                );
            }
        }
        // b) DMA 池（9 页连续物理帧，1:1 映射；R14-D 两趟纪律单源）。
        match map_contiguous_dma_pool(ctx, vspace, types, rights, NET_DMA_PAGES) {
            Ok(phys_base) => {
                crate::println!(
                    "[eneros-root-task] NET-HANDOVER {}: dma vaddr=0x{:x} phys=0x{:x} pages={} (1:1)",
                    svc.name,
                    phys_base,
                    phys_base,
                    NET_DMA_PAGES
                );
                net_handover = Some(NetHandover {
                    // 46.5：寄存器基址 = 映射页基（NET_MMIO_VADDR / +0x1000）
                    // + **页内槽偏移**（全量区域偏移唯一；slot1 与 slot9 不再
                    // 同址）。同 storage 侧"帧基+槽偏移"约定。旧实现平基址 →
                    // 同页多槽服务读到的都是页内第一个设备（slot8）的寄存器。
                    mmio_vaddr: NET_MMIO_VADDR + mmio_off,
                    dma_vaddr: phys_base,
                    dma_phys: phys_base,
                });
            }
            Err(DmaPoolFail::Build) => {
                crate::println!(
                    "[eneros-root-task] NET-HANDOVER {}: dma map FAILED —— 无传输（显式 Unavailable）",
                    svc.name
                );
            }
            Err(DmaPoolFail::Alloc) => {
                crate::println!(
                    "[eneros-root-task] NET-HANDOVER {}: dma pool alloc FAILED —— 无传输（显式 Unavailable）",
                    svc.name
                );
            }
        }
    }

    // SPEC-D WP-D.1：svc-modbus-rtu UART MMIO 移交——允许清单登记制
    // （`uart_handover_slot`；今日空表=F-S1 QEMU 无第二串口的诚实路径）。
    // 寄存器通道 X7 承载 vaddr（DAQ-1 裁决见 UartHandover 文档）；服务侧
    // 句柄缺席 → 能力壳 ST_UNAVAILABLE/RTU-UNAVAILABLE（fail-closed 两态，
    // 无中间谎言态）。
    let mut uart_handover: Option<UartHandover> = None;
    if uart_handover_slot(&svc.name).is_some() {
        uart_handover = try_uart_handover(&svc.name, ctx, vspace, types, rights);
    }

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

    // 9. 寄存器：PC=entry_mapped, SP=stack_top, X0=0(ep cap 槽), X1=IPC buffer vaddr,
    //    X2=就绪 ntfn 槽（服务 CSpace 槽 2；WP-37.1 #8 readiness handshake），
    //    X3=EIPC 授予掩码（蓝图 40 收尾）, X4=心跳 ntfn 槽（0=未授予）；
    //    svc-storage 特例：X3=MMIO vaddr, X4=DMA vaddr, X5=DMA phys（41 设备
    //    移交，见 initial_registers 文档）。
    //    SPEC-B WP-B.1：X6=闸门后墙钟 epoch 秒。
    //    布局知识由 `initial_registers` 单一持有（W2.1）；count 由该函数契约保证。
    let regs = initial_registers(
        entry,
        svc.stack_top,
        buffer_vaddr,
        READY_NTFN_CSPACE_SLOT,
        eipc_mask,
        heartbeat_slot,
        wall_epoch,
        storage_handover,
        net_handover,
        uart_handover,
    );
    // SAFETY: regs 指向有效 UserContext（栈上），count=INITIAL_REG_COUNT 写
    // pc/sp/spsr/x0..x7（规范索引 0..10；X5 供 svc-storage/net 设备移交，
    // 其他服务 X5=0；X6=SPEC-B 墙钟 epoch 秒，全部服务统一；X7=SPEC-D
    // UART MMIO vaddr（UART 集服务）或 70.7 svc-storage boot 池基址
    //（storage 臂）——两集互斥）。
    tcb_write_registers(tcb, 0, INITIAL_REG_COUNT as u64, &regs).map_err(SpawnError::Sel4)?;

    // 10. 优先级（authority = root TCB cap）。
    // SAFETY: root_tcb 为 root-task 自身 TCB cap（有效且有权）。
    tcb_set_priority(tcb, ctx.root_tcb, SVC_PRIORITY).map_err(SpawnError::Sel4)?;

    // 11. WP-37.1 #8：服务就绪 notification——retype + 服务 CSpace 槽 2 mint
    //     （服务 init 完成后 seL4_Signal(2) 上报就绪）+ root 侧带 badge 副本
    //     （readiness poll 用；badge = 服务索引+1，与 fault_ep 同规，poll 得
    //     sender≠0 即有就绪信号）。不绑定 TCB（与 fault 路径正交：fault 走
    //     fault_ep endpoint，本 ntfn 只承载就绪信令，互不混淆）。
    let ready_ntfn = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
    retype_one(ctx, types.notification, OBJ_BITS_NOTIFICATION, ready_ntfn)?;
    // SAFETY: cnode 为服务 CSpace（一阶）；槽 2 空闲。badge=ntfn_badge
    // （服务索引+1，非零）——seL4 Signal 把**信令方 cap 的 badge** 写入
    // notification 数据字，root 侧 Poll 得 sender=该 badge（=服务索引+1）
    // 判定就绪；badge=0 时 sender=0，poll 判据失效（QEMU 三/四跑实测：
    // 服务 READY-SIGNALED 但 root 全部 Degraded）。
    cnode_mint(
        cnode,
        READY_NTFN_CSPACE_SLOT,
        OBJ_BITS_CNODE,
        root_cnode,
        ready_ntfn,
        64,
        rights.all,
        ntfn_badge,
    )
    .map_err(SpawnError::Sel4)?;
    // root 侧带 badge 副本（badge=ntfn_badge=服务索引+1；poll 得 sender 识别）。
    let ready_ntfn_root = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
    // SAFETY: root cnode 槽 ready_ntfn_root 空闲；badge=ntfn_badge（非零）。
    cnode_mint(
        ctx.root_cnode,
        ready_ntfn_root,
        64,
        root_cnode,
        ready_ntfn,
        64,
        rights.all,
        ntfn_badge,
    )
    .map_err(SpawnError::Sel4)?;
    crate::println!(
        "[eneros-root-task] DBG ready_ntfn svc='{}' root_slot={} badge={}",
        svc.name,
        ready_ntfn_root,
        ntfn_badge
    );

    Ok(ConfiguredService {
        tcb,
        ep,
        root_fault_ep,
        ready_ntfn_root,
        cnode,
        storage_handover,
        net_handover,
        uart_handover,
    })
}

/// 22 §2.W1.6：映射设备 MMIO 区域进 root-task VSpace（1:1）。
///
/// SP805 看门狗 @ `paddr`（QEMU 无设备时读写为 no-op 不 fault；真机有效）。
/// 用 device untyped retype frame → map 到同一物理地址（`arm_page_map`
/// 物理地址 = 虚拟地址的 1:1 语义由调用方保证）。
#[cfg(target_os = "none")]
pub fn map_device_mmio(ctx: &mut SpawnCtx, paddr: u64, size_bits: u64) -> Result<(), SpawnError> {
    // SPEC-B WP-B.1：同物理页去重——paddr 已落在某既有设备帧范围内 → 直接
    // 复用（vaddr 恒等=物理，不重 Retype）。大区帧先映射、子设备后请求的
    // 场景由本登记保证零冲突。
    for entry in &ctx.dev_maps {
        if let Some((p0, bits0, _)) = entry {
            let Some(end0) = p0.checked_add(1u64 << *bits0) else {
                continue;
            };
            if paddr >= *p0 && paddr < end0 {
                return Ok(());
            }
        }
    }
    let types = ObjectTypeTable::resolve();
    let rights = RightsTable::resolve();
    // 分配 frame 槽并 retype（device untyped 承载设备内存）。
    let slot = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
    // FIX-BUGS-0810 B2：按物理地址定位 device untyped（alloc_device_at）；
    // 平台未导出覆盖 paddr 的 device untyped → Err → main.rs 软件模式兜底
    // （"映射成功 ⇔ 正确物理页"，杜绝错页映射成功）。
    retype_device_one(ctx, types.arm_frame, size_bits, paddr, slot)?;
    // WP-27.3 D1（QEMU 实测 FailedLookup）：Page_Map 的 lookupPTSlot 需要
    // vaddr 所在层级的页表链——root vspace 的内核初始映射只覆盖 kernel
    // image 与 init bundle 区域（PGD 槽 0 → PUD 表），设备地址（如
    // 0x09000000）的 PUD/PD/PT 槽为空 → 帧无法插入（ptBitsLeft 与
    // frameSize 不匹配）。逐级补建页表（arm_page_table_map 自动挂到
    // lookupPTSlot 返回的缺失槽），直到 Page_Map 成功（最多 3 级）。
    // 非 FailedLookup 错误不重试（fail-closed）。
    let mut mapped = false;
    // WP-37.3：MMIO 页 RW + ExecuteNever（device 属性；cache 语义 38 接管）。
    let dev_perm = VmPermissions::DEVICE_RW_NX;
    let dev_attr = dev_perm.vm_attributes();
    for _level in 0..3 {
        let result = arm_page_map(
            ctx.root_vspace,
            slot,
            paddr,
            dev_perm.cap_rights(&rights),
            dev_attr,
        );
        match result {
            Ok(()) => {
                mapped = true;
                break;
            }
            Err(eneros_sel4_sys::safe::Sel4Error::FailedLookup) => {
                let pt_slot = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
                retype_one(ctx, types.arm_page_table, OBJ_BITS_PAGE_TABLE, pt_slot)?;
                arm_page_table_map(ctx.root_vspace, pt_slot, paddr).map_err(SpawnError::Sel4)?;
            }
            Err(e) => return Err(SpawnError::Sel4(e)),
        }
    }
    if !mapped {
        return Err(SpawnError::Sel4(
            eneros_sel4_sys::safe::Sel4Error::FailedLookup,
        ));
    }
    // SPEC-B WP-B.1：登记设备帧（首个空槽；满则丢弃——去重仅优化）。
    for slot in &mut ctx.dev_maps {
        if slot.is_none() {
            *slot = Some((paddr, size_bits, paddr));
            break;
        }
    }
    Ok(())
}

/// 映射一个已 retype 的 frame 到**任意 VSpace** 的指定虚拟地址，带
/// FailedLookup 逐级补建页表（服务 VSpace 只有 SVC_VSPACE_BASE 区域
/// 的 L1/L2/L3——设备虚拟地址（如 0x40000000）不在链上，须补建；
/// 41 存储句柄移交，逻辑同 [`map_device_mmio`] 但目标 vspace/vaddr
/// 参数化）。
#[cfg(target_os = "none")]
pub(crate) fn map_frame_with_pagetables(
    ctx: &mut SpawnCtx,
    vspace: u64,
    frame: u64,
    vaddr: u64,
    cap_rights: u64,
    attr: u64, // ArmVmAttributes（类型别名，私有——按值传 u64）
) -> Result<(), SpawnError> {
    let types = ObjectTypeTable::resolve();
    let mut mapped = false;
    for _level in 0..3 {
        let result = arm_page_map(vspace, frame, vaddr, cap_rights, attr);
        match result {
            Ok(()) => {
                mapped = true;
                break;
            }
            Err(eneros_sel4_sys::safe::Sel4Error::FailedLookup) => {
                let pt_slot = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
                retype_one(ctx, types.arm_page_table, OBJ_BITS_PAGE_TABLE, pt_slot)?;
                arm_page_table_map(vspace, pt_slot, vaddr).map_err(SpawnError::Sel4)?;
            }
            Err(e) => return Err(SpawnError::Sel4(e)),
        }
    }
    if !mapped {
        return Err(SpawnError::Sel4(
            eneros_sel4_sys::safe::Sel4Error::FailedLookup,
        ));
    }
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
    // FIX-BUGS-0810 B2：按物理地址定位 device untyped（alloc_device_at）。
    // 注意：seL4 Retype **没有 untyped 内 offset 参数**——node_offset 是
    // 目标 CNode 槽位，untyped 内位置由内核 freeIndex 自动推进
    // （libsel4 sel4.xml UntypedRetype 参数序实锤）。设备帧实际落在
    // untyped 的 freeIndex 处；alloc_device_at 只保证"untyped 覆盖 paddr"。
    // 46.5 曾把 slot 误当 untyped 偏移改坏（InvalidArgument/InvalidCapability），
    // 已回退——node_offset = slot（cap 放置槽）。
    let (untyped, _byte_off) = ctx
        .untyped
        .alloc_device_at(paddr, size_bits)
        .ok_or(SpawnError::NoUntyped)?;
    // SAFETY: untyped 为覆盖 paddr 的 device untyped cap；slot 为 root CSpace
    // 空闲槽；node_index/node_offset = slot（cap 写入目标槽）。
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
pub(crate) fn retype_one(
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
    // 非证据性调试输出（对象创建跟踪）：仅 debug 构建打印（release 下
    // ~200 行串口拖慢 QEMU boot，37 实测 200s 预算不足）。
    if cfg!(debug_assertions) {
        crate::println!(
            "[eneros-root-task] DBG retype_one untyped={} offset={} type={} slot={} untyped_cap_type={} cnode_type={}",
            untyped,
            offset,
            object_type,
            slot,
            eneros_sel4_sys::safe::debug_cap_identify(untyped),
            eneros_sel4_sys::safe::debug_cap_identify(ctx.root_cnode)
        );
    }
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
fn retype_page_tables(
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

/// 30 RT-007：单次窗口拷贝长度校验（release 构建也生效——旧
/// `debug_assert!(len <= PAGE_SIZE)` 在 release 剔除后越页写 WINDOW_BASE
/// 窗口）。纯函数，host 可直接测。
#[allow(dead_code)] // 44.2：窗口拷贝长度校验，当前路径未用（44.4 冻结登记）
pub(crate) fn window_copy_len_ok(len: usize) -> Result<(), SpawnError> {
    if len > PAGE_SIZE as usize {
        return Err(SpawnError::WindowOverflow);
    }
    Ok(())
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
    // WP-37.4：blob 切片路径复用窗口机制（源界由 elf-loader 保证）。
    let start = src_offset as usize;
    let end = start.checked_add(len).ok_or(SpawnError::SegmentOverlap)?;
    if end > module.blob.len() {
        return Err(SpawnError::SegmentOverlap);
    }
    copy_window_bytes(ctx, frame, &module.blob[start..end], rights)
}

/// WP-37.4：blob 切片拷入 frame 的**页内偏移**处（非对齐段首页——
/// lld 末段 vaddr 紧随前段 filesz 尾，页内偏移 = vaddr - floor(vaddr)）。
/// 窗口映射 → 偏移处拷贝 → 卸载；`len + dst_offset` 不得超过页大小。
#[cfg(target_os = "none")]
fn copy_segment_via_window_at(
    ctx: &mut SpawnCtx,
    frame: u64,
    module: &crate::bundle::BundleModule,
    src_offset: u64,
    dst_offset: u64,
    len: usize,
    rights: &RightsTable,
) -> Result<(), SpawnError> {
    let start = src_offset as usize;
    let end = start.checked_add(len).ok_or(SpawnError::SegmentOverlap)?;
    if end > module.blob.len() {
        return Err(SpawnError::SegmentOverlap);
    }
    // dst_offset + len 必须落在页内（页模型保证；防御性二次校验）。
    let dst_end = (dst_offset as usize)
        .checked_add(len)
        .ok_or(SpawnError::SegmentOverlap)?;
    if dst_end > PAGE_SIZE as usize {
        return Err(SpawnError::SegmentOverlap);
    }
    // SAFETY: 窗口映射 + 页内偏移写入——dst_offset < PAGE_SIZE（上文校验），
    // len ≤ PAGE_SIZE - dst_offset；frame 刚 retype 且未映射。
    use core::ptr::copy_nonoverlapping;
    arm_page_map(
        ctx.root_vspace,
        frame,
        WINDOW_BASE,
        rights.read | rights.write,
        VmPermissions::RW_NX.vm_attributes(),
    )
    .map_err(SpawnError::Sel4)?;
    unsafe {
        copy_nonoverlapping(
            module.blob.as_ptr().add(start),
            (WINDOW_BASE as *mut u8).add(dst_offset as usize),
            len,
        );
    }
    // SAFETY: frame 已映射（卸载前置条件满足）。
    arm_page_unmap(frame).map_err(SpawnError::Sel4)?;
    Ok(())
}

/// WP-37.4：经 root 私有窗口把内存切片拷入刚 retype 的 frame（窗口映射 →
/// 拷贝 → 卸载；`len` 不得超过页大小）。源为调用方切片（bundle blob 或
/// ET_DYN 重定位镜像），界内性由调用方保证、本函数二次校验长度。
#[cfg(target_os = "none")]
fn copy_window_bytes(
    ctx: &mut SpawnCtx,
    frame: u64,
    src: &[u8],
    rights: &RightsTable,
) -> Result<(), SpawnError> {
    use core::ptr::copy_nonoverlapping;
    // 30 RT-007：显式检查（fail-closed；纯函数 host 可测）。
    window_copy_len_ok(src.len())?;
    // SAFETY: WINDOW_BASE 为 root VSpace 内已建页表的窗口槽；frame 刚
    // retype 且未映射（unmap 后 cap 已解除，可重新映射）。
    // WP-37.3：窗口为 root 私有 staging，RW + ExecuteNever（禁止 W+X 暴露）。
    arm_page_map(
        ctx.root_vspace,
        frame,
        WINDOW_BASE,
        rights.read | rights.write,
        VmPermissions::RW_NX.vm_attributes(),
    )
    .map_err(SpawnError::Sel4)?;
    // SAFETY: 目标 WINDOW_BASE 指向刚映射的物理页（4 KiB）；源切片长度
    // 已由 window_copy_len_ok 限制在页内。
    unsafe {
        copy_nonoverlapping(src.as_ptr(), WINDOW_BASE as *mut u8, src.len());
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

    /// WP-37.4（页模型）：非对齐段（lld 末段紧随前段 filesz 尾的典型
    /// 布局）——plan_one 必须接受并产出 floor 页基 + 页内偏移拷贝三要素。
    #[test]
    fn test_plan_one_accepts_unaligned_last_segment() {
        // 合成 ELF：段1 R-X @ 0x1000（offset 0，filesz 8）；段2 RW-
        // @ 0x2238（非对齐；offset 0x1238 ≡ 0x238 (mod 0x1000)——ELF 规范
        // p_offset ≡ p_vaddr (mod p_align)；filesz 0x90、memsz 0x100）。
        let seg2_off = 0x1238usize;
        let len = 64 + 56 * 2 + seg2_off + 0x90;
        let mut buf = vec![0u8; len];
        buf[..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
        buf[4] = 2;
        buf[5] = 1;
        buf[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
        buf[18..20].copy_from_slice(&183u16.to_le_bytes());
        buf[24..32].copy_from_slice(&0x1000u64.to_le_bytes()); // entry
        buf[32..40].copy_from_slice(&64u64.to_le_bytes());
        buf[54..56].copy_from_slice(&56u16.to_le_bytes());
        buf[56..58].copy_from_slice(&2u16.to_le_bytes());
        let ph1 = 64;
        buf[ph1..ph1 + 4].copy_from_slice(&1u32.to_le_bytes());
        buf[ph1 + 4..ph1 + 8].copy_from_slice(&5u32.to_le_bytes()); // R+X
        buf[ph1 + 8..ph1 + 16].copy_from_slice(&0u64.to_le_bytes());
        buf[ph1 + 16..ph1 + 24].copy_from_slice(&0x1000u64.to_le_bytes());
        buf[ph1 + 24..ph1 + 32].copy_from_slice(&0x1000u64.to_le_bytes());
        buf[ph1 + 32..ph1 + 40].copy_from_slice(&8u64.to_le_bytes());
        buf[ph1 + 40..ph1 + 48].copy_from_slice(&8u64.to_le_bytes());
        buf[ph1 + 48..ph1 + 56].copy_from_slice(&0x1000u64.to_le_bytes());
        let ph2 = 64 + 56;
        buf[ph2..ph2 + 4].copy_from_slice(&1u32.to_le_bytes());
        buf[ph2 + 4..ph2 + 8].copy_from_slice(&6u32.to_le_bytes()); // R+W
        buf[ph2 + 8..ph2 + 16].copy_from_slice(&(seg2_off as u64).to_le_bytes());
        buf[ph2 + 16..ph2 + 24].copy_from_slice(&0x2238u64.to_le_bytes()); // vaddr 非对齐
        buf[ph2 + 24..ph2 + 32].copy_from_slice(&0x2238u64.to_le_bytes());
        buf[ph2 + 32..ph2 + 40].copy_from_slice(&0x90u64.to_le_bytes()); // filesz
        buf[ph2 + 40..ph2 + 48].copy_from_slice(&0x100u64.to_le_bytes()); // memsz
        buf[ph2 + 48..ph2 + 56].copy_from_slice(&0x1000u64.to_le_bytes());
        let elf = eneros_elf_loader::LoadedElf::parse(&buf).expect("parse");
        let plan =
            plan_one("svc-x", &elf, 0x4000_0000, 0x1000, 0x4000, 5).expect("非对齐末段必须可计划");
        // 页模型断言：段2 页基 = floor(0x2238) = 0x2000，页内偏移 0x238，
        // 源偏移 = seg.offset - io = 0x1238 - 0x238 = 0x1000，拷贝 0x90 字节。
        let p2 = plan
            .pages
            .iter()
            .find(|p| p.vaddr == 0x2000)
            .expect("非对齐段必须映射到 floor 页基");
        assert_eq!(p2.dst_offset, 0x238, "页内写入偏移 = io");
        assert_eq!(p2.src_file_offset, 0x1000, "源偏移 = seg.offset - io");
        assert_eq!(p2.copy_len, 0x90, "拷贝长度 = filesz");
        assert!(p2.write && !p2.exec, "RW- 段权限保持");
        // 栈仍置于所有段末端之后（无重叠）。
        let max_end = plan.pages.iter().map(|p| p.vaddr + p.size).max().unwrap();
        assert!(plan.stack_top - plan.stack_size >= max_end);
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
            bytes[entry + 4..entry + 8].copy_from_slice(&15u32.to_le_bytes()); // flags=Support（WP-37.1 已知 criticality 编码）
            bytes[entry + 8..entry + 16].copy_from_slice(&(cursor as u64).to_le_bytes());
            bytes[entry + 16..entry + 24].copy_from_slice(&(blob.len() as u64).to_le_bytes());
            bytes[entry + 24..entry + 24 + name.len()].copy_from_slice(name.as_bytes());
            bytes[cursor..cursor + blob.len()].copy_from_slice(blob);
            cursor += blob.len();
        }
        bytes
    }

    /// ET_EXEC 变体夹具（WP-37.4 前 plan_spawn 的原生语义域——对拍与
    /// 迁移用例用；e_type 补丁 = 2）。
    fn minimal_exec_elf_et_exec() -> Vec<u8> {
        let mut buf = minimal_exec_elf();
        buf[16..18].copy_from_slice(&2u16.to_le_bytes());
        buf
    }

    fn minimal_data_elf_et_exec() -> Vec<u8> {
        let mut buf = minimal_data_elf();
        buf[16..18].copy_from_slice(&2u16.to_le_bytes());
        buf
    }

    /// 蓝图 67 WP-67.5 收敛后单实现语义锚（原对拍测试的存留臂：删除前
    /// `plan_spawn` 与本入口对相同 ET_EXEC 输入产出结构级全等的 plan/Err
    /// ——`assert_eq!(legacy, modern)` 实测绿后才删旧入口；本用例保留
    /// with_stack 入口的 ET_EXEC 正例 + required fail-closed 语义锚）。
    #[test]
    fn bp67_dyn_aware_with_stack_et_exec_semantics() {
        // ① 正常：root + 2 required ET_EXEC 服务（ipc_ep 递增 / 页 W^X）。
        let raw = build_bundle(&[
            (
                "root-task",
                crate::bundle::MODULE_KIND_ROOT,
                minimal_exec_elf_et_exec(),
            ),
            (
                "svc-fault",
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_data_elf_et_exec(),
            ),
            (
                "svc-proc-ts",
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_data_elf_et_exec(),
            ),
        ]);
        let bundle = InitBundle::parse(&raw).expect("bundle parse");
        let plan = plan_spawn_dyn_aware_with_stack(
            &bundle,
            0x4000_0000,
            0x1000,
            0x4000,
            100,
            0x200000,
            &[],
        )
        .expect("plan");
        assert_eq!(plan.service_count(), 2);
        assert_eq!(plan.services[0].ipc_ep, 101);
        assert_eq!(plan.services[1].ipc_ep, 102);
        assert_eq!(
            plan.services[0].stack_top, plan.services[1].stack_top,
            "ET_EXEC 链接地址布局（独立 VSpace 地址可复用）"
        );

        // ② required 坏 ELF → fail-closed（同对拍 ②臂）。
        let raw_bad = build_bundle(&[
            (
                "root-task",
                crate::bundle::MODULE_KIND_ROOT,
                minimal_exec_elf_et_exec(),
            ),
            (
                "svc-fault",
                crate::bundle::MODULE_KIND_REQUIRED,
                vec![0xDE, 0xAD],
            ),
            (
                "svc-proc-ts",
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_data_elf_et_exec(),
            ),
        ]);
        let bundle_bad = InitBundle::parse(&raw_bad).expect("parse");
        assert_eq!(
            plan_spawn_dyn_aware_with_stack(
                &bundle_bad,
                0x4000_0000,
                0x1000,
                0x4000,
                100,
                0x200000,
                &[],
            ),
            Err(SpawnError::RequiredElfFailed {
                name: "svc-fault".to_string()
            })
        );
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
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_data_elf_et_exec(),
            ),
            (
                "svc-proc-ts",
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_data_elf_et_exec(),
            ),
        ]);
        let bundle = InitBundle::parse(&raw).expect("bundle parse");
        let plan = plan_spawn_dyn_aware_with_stack(
            &bundle,
            0x4000_0000,
            0x1000,
            0x4000,
            100,
            0x200000,
            &[],
        )
        .expect("plan");
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

    /// WP-37.3（复盘）：stack guard 页——栈底之下保留一个未映射页，
    /// 向下溢出访问 guard → 可归因 VM fault，不落入 ELF 段。
    #[test]
    fn stack_guard_page_is_unmapped_below_stack() {
        let raw = build_bundle(&[
            (
                "root-task",
                crate::bundle::MODULE_KIND_ROOT,
                minimal_exec_elf(),
            ),
            (
                "svc-fault",
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_data_elf_et_exec(),
            ),
            (
                "svc-proc-ts",
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_data_elf_et_exec(),
            ),
        ]);
        let bundle = InitBundle::parse(&raw).expect("bundle parse");
        let plan = plan_spawn_dyn_aware_with_stack(
            &bundle,
            0x4000_0000,
            0x1000,
            0x4000,
            100,
            0x200000,
            &[],
        )
        .expect("plan");
        let svc = &plan.services[0];
        // 栈区 [stack_top - stack_size, stack_top)；guard 页在栈底下一页
        // [stack_top - stack_size - page_size, stack_top - stack_size)。
        let stack_base = svc.stack_top - svc.stack_size;
        let guard_vaddr = stack_base - 0x1000;
        // guard 不在 ELF 段映射内（pages 的 vaddr 区间都不覆盖 guard）。
        for p in &svc.pages {
            let seg_end = p.vaddr.checked_add(p.size).expect("page end");
            assert!(
                guard_vaddr >= seg_end || guard_vaddr + 0x1000 <= p.vaddr,
                "guard 页必须未映射（不得落入段页区间）"
            );
        }
        // 栈与 guard 之间无重叠：最低栈映射页 = stack_base（向下映射
        // stack_size 字节），guard 在其下方一页。
        assert_eq!(stack_base, guard_vaddr + 0x1000);
        assert!(svc.stack_size == 0x4000);
    }
    /// 30 RT-006 判据：构造 vaddr 重叠的合成 ELF（两段同 vaddr）→
    /// plan_one 必须 Err(SegmentOverlap)（修复前无检查，页重叠后写覆盖）。
    #[test]
    fn test_plan_one_rejects_overlapping_segments() {
        // 用 minimal_exec_elf 的字节但手工构造重叠段：通过 validate_segments
        // 纯函数直接验证 plan_one 的校验来源（落点统一）。
        let seg = |vaddr: u64| eneros_elf_loader::LoadedSegment {
            vaddr,
            memsz: 0x1000,
            filesz: 0x1000,
            offset: 0,
            flags: eneros_elf_loader::SegmentFlags {
                read: true,
                write: false,
                execute: true,
            },
            align: 0x1000,
        };
        let segs = alloc::vec![seg(0x1000), seg(0x1000)];
        assert_eq!(
            eneros_elf_loader::LoadedElf::validate_segments(&segs),
            Err(eneros_elf_loader::ElfError::SegmentOverlap),
            "plan_one 的校验来源（validate_segments）必须拦重叠段"
        );
    }

    /// 30 RT-007 判据：copy_segment_via_window 的窗口长度检查（release
    /// 构建 debug_assert 被剔除——显式纯函数）。审查修复：旧测试用
    /// include_str 检查源码字符串（假测），现直接调用纯函数验证行为。
    #[test]
    fn test_window_copy_len_ok() {
        assert_eq!(window_copy_len_ok(PAGE_SIZE as usize), Ok(()));
        assert_eq!(
            window_copy_len_ok(PAGE_SIZE as usize + 1),
            Err(SpawnError::WindowOverflow)
        );
        assert_eq!(window_copy_len_ok(0), Ok(()));
    }

    /// 审查修复判据（蓝图 RT-006 规格补全）：乱序非重叠段（ELF 序最后
    /// 一段不是最高 vaddr）→ 栈起点低于前面段的末端 → 栈与段重叠必须
    /// 拒绝（旧实现只查段间重叠，栈区 [stack_top-stack_size, stack_top)
    /// 与段共享虚拟页，后写覆盖/执行损坏）。
    #[test]
    fn test_plan_one_rejects_stack_collision_with_out_of_order_segments() {
        let mut buf = minimal_data_elf(); // 段序 0x1000(R-X), 0x2000(RW-)
                                          // 交换两段 vaddr：先 0x2000 后 0x1000（非重叠但乱序）。
        let ph1 = 64;
        let ph2 = 64 + 56;
        buf[ph1 + 16..ph1 + 24].copy_from_slice(&0x2000u64.to_le_bytes());
        buf[ph1 + 24..ph1 + 32].copy_from_slice(&0x2000u64.to_le_bytes());
        buf[ph2 + 16..ph2 + 24].copy_from_slice(&0x1000u64.to_le_bytes());
        buf[ph2 + 24..ph2 + 32].copy_from_slice(&0x1000u64.to_le_bytes());
        // entry 必须落在可执行段（0x2000，R-X 段）。
        buf[24..32].copy_from_slice(&0x2000u64.to_le_bytes());
        let elf = eneros_elf_loader::LoadedElf::parse(&buf).expect("parse");
        // 前提：乱序非重叠段必须通过 validate_segments（否则测试前提
        // 不成立——检查的是栈碰撞而非段重叠）。
        assert!(
            eneros_elf_loader::LoadedElf::validate_segments(elf.segments()).is_ok(),
            "测试前提：乱序非重叠段不得触发段重叠"
        );
        // WP-37.4（页模型）：栈区置于所有段末端最大值之后——乱序段也
        // 不会与栈重叠（旧实现按段序末端的顺序布局会重叠 → 拒绝；新
        // 模型按 max_seg_end 布局，不变量由构造保证）。
        let plan = plan_one("svc-x", &elf, 0x4000_0000, 0x1000, 0x4000, 5)
            .expect("乱序非重叠段必须可计划（栈置于 max_seg_end 后）");
        let max_seg_end = plan.pages.iter().map(|p| p.vaddr + p.size).max().unwrap();
        assert!(
            plan.stack_top - plan.stack_size >= max_seg_end,
            "栈底必须不低于任何段末端（无重叠）"
        );
        // 升序（正常）段序不拒绝。
        let mut ok = minimal_data_elf();
        ok[24..32].copy_from_slice(&0x1000u64.to_le_bytes());
        let elf_ok = eneros_elf_loader::LoadedElf::parse(&ok).expect("parse");
        assert!(
            plan_one("svc-x", &elf_ok, 0x4000_0000, 0x1000, 0x4000, 5).is_ok(),
            "升序段序必须照常通过"
        );
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
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_exec_elf(),
            ),
        ]);
        let bundle = InitBundle::parse(&raw).expect("bundle parse");
        assert_eq!(
            plan_spawn_dyn_aware_with_stack(
                &bundle,
                0x4000_0000,
                0x1000,
                0x4000,
                100,
                0x200000,
                &[]
            ),
            Err(SpawnError::TooFewServices),
            "服务不足 2 个必须 fail-closed"
        );
    }

    /// WP-37.1 #6：required 服务 ELF 损坏必须阻断 product boot
    /// （旧实现坏 ELF continue 后只看服务数量 ≥2——P0/RT-05）。
    #[test]
    fn test_plan_spawn_required_bad_elf_fails_closed() {
        let raw = build_bundle(&[
            (
                "root-task",
                crate::bundle::MODULE_KIND_ROOT,
                minimal_exec_elf(),
            ),
            (
                "svc-fault",
                crate::bundle::MODULE_KIND_REQUIRED,
                vec![0xFF, 0xEE, 0xDD], // 坏 ELF
            ),
            (
                "svc-proc-ts",
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_exec_elf(),
            ),
        ]);
        let bundle = InitBundle::parse(&raw).expect("bundle parse");
        assert_eq!(
            plan_spawn_dyn_aware_with_stack(
                &bundle,
                0x4000_0000,
                0x1000,
                0x4000,
                100,
                0x200000,
                &[]
            ),
            Err(SpawnError::RequiredElfFailed {
                name: "svc-fault".to_string()
            }),
            "坏 required 服务必须阻断（旧实现静默 continue）"
        );
    }

    /// WP-37.1：optional 服务 ELF 损坏只降级不阻断（required 集合仍全量
    /// 成功；plan 只含 required 服务，数量校验 ≥2 照常）。
    #[test]
    fn test_plan_spawn_optional_bad_elf_degrades() {
        let raw = build_bundle(&[
            (
                "root-task",
                crate::bundle::MODULE_KIND_ROOT,
                minimal_exec_elf(),
            ),
            (
                "svc-fault",
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_exec_elf(),
            ),
            (
                "svc-proc-ts",
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_exec_elf(),
            ),
            (
                "svc-drv-opt",
                crate::bundle::MODULE_KIND_OPTIONAL,
                vec![0xFF, 0xEE, 0xDD], // 坏 ELF（optional）
            ),
        ]);
        let bundle = InitBundle::parse(&raw).expect("bundle parse");
        let plan = plan_spawn_dyn_aware_with_stack(
            &bundle,
            0x4000_0000,
            0x1000,
            0x4000,
            100,
            0x200000,
            &[],
        )
        .expect("plan");
        assert_eq!(
            plan.service_count(),
            2,
            "optional 失败不进 plan，required 全量保留"
        );
        assert!(plan.services.iter().all(|s| s.name != "svc-drv-opt"));
    }

    /// WP-37.1：未知 kind 防御性失败（bundle 解析器已拒绝；plan 层不静默）。
    #[test]
    fn test_plan_spawn_unknown_kind_fails_closed() {
        // 绕过 bundle parse 的 kind 校验：直接构造 InitBundle 手工对象。
        let blob0 = minimal_exec_elf();
        let blob1 = minimal_exec_elf();
        let bundle = crate::bundle::InitBundle {
            total_len: 0,
            source_date_epoch: 0,
            payload_sha256: [0u8; 32],
            modules: alloc::vec![
                crate::bundle::BundleModule {
                    kind: crate::bundle::MODULE_KIND_ROOT,
                    flags: 31,
                    capabilities: 0,
                    name: "root-task".to_string(),
                    blob: &blob0,
                    sha256: [0u8; 32],
                },
                crate::bundle::BundleModule {
                    kind: 42, // 未知 kind（解析器已拒绝；plan 防御性失败）
                    flags: 15,
                    capabilities: 0,
                    name: "svc-x".to_string(),
                    blob: &blob1,
                    sha256: [0u8; 32],
                },
            ],
        };
        assert_eq!(
            plan_spawn_dyn_aware_with_stack(
                &bundle,
                0x4000_0000,
                0x1000,
                0x4000,
                100,
                0x200000,
                &[]
            ),
            Err(SpawnError::UnknownBundleKind)
        );
    }

    #[test]
    fn test_plan_spawn_no_root_fail_closed() {
        let raw = build_bundle(&[
            (
                "svc-fault",
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_exec_elf(),
            ),
            (
                "svc-proc-ts",
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_exec_elf(),
            ),
        ]);
        let bundle = InitBundle::parse(&raw).expect("bundle parse");
        assert_eq!(
            plan_spawn_dyn_aware_with_stack(
                &bundle,
                0x4000_0000,
                0x1000,
                0x4000,
                100,
                0x200000,
                &[]
            ),
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
            plan_spawn_dyn_aware_with_stack(
                &bundle,
                0x4000_0000,
                0x1000,
                0x4000,
                100,
                0x200000,
                &[]
            ),
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
    fn test_page_entry_wx_semantics() {
        let exec = PageEntry {
            vaddr: 0x1000,
            phys: 0,
            size: 0x1000,
            read: true,
            write: false,
            exec: true,
            src_file_offset: 0,
            dst_offset: 0,
            copy_len: 0,
        };
        assert!(!exec.is_wx_violation());
        let wx = PageEntry {
            vaddr: 0x1000,
            phys: 0,
            size: 0x1000,
            read: true,
            write: true,
            exec: true,
            src_file_offset: 0,
            dst_offset: 0,
            copy_len: 0,
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
                size_bits: 16,      // 64 KiB
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
    fn reserved_window_blocked_keeps_cap_index_alignment() {
        // 复现 2026-09-23 首版拉黑实弹坑：从 descs 删除保留窗 untyped 使
        // 后续槽号错位 → Retype 打到错误 cap（spawn FAILED
        // Sel4(InvalidArgument)）。blocked 位必须同时满足：窗内分配被拒 +
        // cap_base+原索引 保持对齐（拉黑项占位不删除）。
        let descs = vec![
            UntypedDesc {
                paddr: 0x0a00_0000,
                size_bits: 25,
                is_device: true,
            },
            UntypedDesc {
                paddr: 0x4200_0000, // 保留窗（70.6 实测形状）
                size_bits: 25,
                is_device: true,
            },
            UntypedDesc {
                paddr: 0x4400_0000,
                size_bits: 25,
                is_device: false,
            },
        ];
        let blocked: Vec<bool> = descs
            .iter()
            .map(|d| super::overlaps_reserved_window(d.paddr, d.size_bits))
            .collect();
        assert_eq!(blocked, vec![false, true, false]);
        let mut alloc = UntypedAllocator::with_blocked(100, descs, blocked);
        // 窗内地址的 device 分配请求必须被拒（即便对应 cap 存在）
        assert!(
            alloc.alloc_device_at(0x4210_0000, 12).is_none(),
            "保留窗内 device 分配必须被拉黑"
        );
        // normal 分配越过索引 0（device）与索引 1（blocked）落在索引 2
        // → cap = cap_base + 原索引（对齐铁律）
        let (cap, _) = alloc.alloc(12).expect("索引 2 normal untyped 可分配");
        assert_eq!(cap, 102, "cap = cap_base + 原索引（拉黑项占位不删除）");
    }

    #[test]
    fn blocked_length_mismatch_fails_closed() {
        // R15-F（蓝图74-01 WP-74.1.3）：blocked 短于 descs 的病态构造
        // （等长不变式被破坏）必须 fail-closed。两面设计：
        // - release：debug_assert 退场，is_blocked 越界缺省 true 兜底——
        //   任何 alloc* 一律 None（宁可拒分配不可放行，BOOT-007 纪律）；
        // - debug：with_blocked 构造期断言响亮拦截（测试 profile 为
        //   unwind，catch_unwind 可验证 panic 面存在）。
        // 夹具：唯一可分配候选全部落在越界索引上——修复前（越界当未拉黑）
        // 四个分配口都会成功分配（红），修复后一律 None（绿）。
        let descs = vec![
            UntypedDesc {
                paddr: 0x3000_0000,
                size_bits: 25,
                is_device: false, // blocked[0]=true（界内 true）
            },
            UntypedDesc {
                paddr: 0x0a00_0000,
                size_bits: 25,
                is_device: true, // 越界索引 1：device 分配唯一候选
            },
            UntypedDesc {
                paddr: 0x3800_0000,
                size_bits: 25,
                is_device: false, // 越界索引 2：normal/DMA/CNode 唯一候选
            },
        ];
        let blocked = vec![true]; // 短于 descs.len()=3

        #[cfg(debug_assertions)]
        {
            let result = std::panic::catch_unwind(|| {
                UntypedAllocator::with_blocked(100, descs.clone(), blocked.clone())
            });
            assert!(
                result.is_err(),
                "debug 下等长失配必须被构造期断言拦截（fail-closed 响亮面）"
            );
        }
        #[cfg(not(debug_assertions))]
        {
            let mut alloc = UntypedAllocator::with_blocked(100, descs.clone(), blocked.clone());
            assert!(alloc.alloc(12).is_none(), "越界缺省 true 须拒 normal 分配");
            assert!(
                alloc.alloc_device_at(0x0a05_0000, 12).is_none(),
                "越界缺省 true 须拒 device 分配"
            );
            assert!(
                alloc.alloc_contiguous_pages(1).is_none(),
                "越界缺省 true 须拒 DMA 连续页分配"
            );
            assert!(
                alloc.alloc_cap_table(0).is_none(),
                "越界缺省 true 须拒 CNode 分配"
            );
            assert_eq!(alloc.allocated(), 0, "病态构造下不得有任何记账字节");
        }
    }

    #[test]
    fn reserved_window_blacklist_shapes() {
        use super::overlaps_reserved_window;
        // overlay 生效形态：32 MiB device untyped 恰铺满保留窗
        // （70.6 日志 DEV-UNTYPED[21] 实测形状）
        assert!(overlaps_reserved_window(0x4200_0000, 25));
        // overlay 缺失形态：normal 大块连续 untyped 覆盖整窗
        assert!(overlaps_reserved_window(0x4000_0000, 30));
        // 部分搭界：起点在窗前、末端探入窗内
        assert!(overlaps_reserved_window(0x41f0_0000, 21));
        // 窗内小对象（capsule/DTB 区被碎切时的任意子段）
        assert!(overlaps_reserved_window(0x427f_f000, 12));
        // 不误伤：DMA 池常用 normal untyped（0x40800000/8MiB，窗外）
        assert!(!overlaps_reserved_window(0x4080_0000, 23));
        // 不误伤：virtio-mmio 设备区（0x0a000000/32MiB）
        assert!(!overlaps_reserved_window(0x0a00_0000, 25));
        // 不误伤：窗上方 normal 区（0x44000000 起，紧贴右沿）
        assert!(!overlaps_reserved_window(0x4400_0000, 20));
        // 边界语义（exclusive）：末端恰为窗起点 / 起点恰为窗末端
        assert!(!overlaps_reserved_window(0x41f0_0000, 20));
        assert!(!overlaps_reserved_window(0x4400_0000, 25));
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
        let (cap2, off2) = alloc.alloc_device_at(0x0905_1000, 12).expect("同区域次页");
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
        assert_eq!(
            off32, 32,
            "16 B 后 32 B 对象从 32 B 对齐处开始（alignUp 语义）"
        );
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

    // ---- WP-76.3.3（蓝图 76 卷三）：freeIndex 记账单源 = align_up/fit ----

    /// 守卫负例（**收编前 contiguous 臂红**）：`size_bits ≥ 64` 的病态 desc
    /// 不得让任何 bump 型分配口 panic 或按 64 取模回绕出错值。收编前
    /// `alloc_contiguous_pages` 的 `1u64 << desc.size_bits` 是唯一无守卫的
    /// 一处（debug 移位 panic / release capacity=1 错值）；另两口的既有
    /// 守卫随之吸收进 fit，本测同时是「守卫吸收」的回归锁。
    #[test]
    fn fit_guard_rejects_oversized_untyped_descs() {
        for bits in [64u64, 65, 128] {
            let descs = vec![UntypedDesc {
                paddr: 0x4100_0000,
                size_bits: bits,
                is_device: false,
            }];
            let mut alloc = UntypedAllocator::new(30, descs);
            assert_eq!(alloc.alloc(12), None, "bits={bits}: alloc 必须 None");
            assert_eq!(
                alloc.alloc_contiguous_pages(4),
                None,
                "bits={bits}: 缺口臂必须 None（不得 panic/错值）"
            );
            assert_eq!(
                alloc.alloc_cap_table(8),
                None,
                "bits={bits}: cap_table 必须 None"
            );
            assert_eq!(alloc.allocated(), 0, "bits={bits}: 拒绝面零记账");
        }
    }

    /// 守卫必须是「跳过该 untyped」而非「中止整轮分配」——否则一处病态
    /// bootinfo 描述会把后面合法 untyped 一并饿死。
    #[test]
    fn fit_guard_skips_oversized_desc_and_continues() {
        let descs = vec![
            UntypedDesc {
                paddr: 0x4100_0000,
                size_bits: 64, // 病态：守卫项
                is_device: false,
            },
            UntypedDesc {
                paddr: 0x4200_0000,
                size_bits: 20, // 1 MiB：唯一可分配候选
                is_device: false,
            },
        ];
        let mut alloc = UntypedAllocator::new(30, descs);
        assert_eq!(alloc.alloc(12), Some((31, 0)), "守卫跳过索引 0 → 索引 1");
        assert_eq!(
            alloc.alloc_contiguous_pages(2),
            Some((31, 4096, 0x4200_1000)),
            "contiguous 同样必须继续扫（used=4096 → aligned 4096）"
        );
        // used=12288 → CNode 粒度 8192 → alignUp=16384（不是 12288 直接累加）。
        assert_eq!(
            alloc.alloc_cap_table(8),
            Some((31, 16384)),
            "cap_table 同侧"
        );
    }

    /// 对齐矩阵：`used` 不在 obj 粒度边界上时，三 bump 口必须一律 alignUp
    /// （记账单源迁移零漂移的行为锁）+ capacity 恰好/差一双侧界。
    #[test]
    fn fit_alignment_matrix_across_three_bump_sites() {
        let mk = |bits: u64| {
            vec![UntypedDesc {
                paddr: 0x4100_0000,
                size_bits: bits,
                is_device: false,
            }]
        };
        // 造 used=4097：一页 4 KiB（used=4096）+ 一 1 B 对象（alignUp 粒度 1）。
        let warm = |a: &mut UntypedAllocator| {
            assert_eq!(a.alloc(12), Some((30, 0)));
            assert_eq!(a.alloc(0), Some((30, 4096)), "1 B 对象落 4096");
        };
        let mut a = UntypedAllocator::new(30, mk(20));
        warm(&mut a);
        assert_eq!(
            a.alloc(12),
            Some((30, 8192)),
            "used=4097 → alignUp(4097,4096)=8192（非简单累加的 4097）"
        );
        let mut b = UntypedAllocator::new(30, mk(20));
        warm(&mut b);
        assert_eq!(
            b.alloc_contiguous_pages(1),
            Some((30, 8192, 0x4100_2000)),
            "DMA 页粒度同样 alignUp 上取整"
        );
        let mut c = UntypedAllocator::new(30, mk(20));
        warm(&mut c);
        // CNode size_bits=12 → 4096 槽 × 32 B = 128 KiB 粒度（≠ 页粒度）。
        assert_eq!(
            c.alloc_cap_table(12),
            Some((30, 131072)),
            "CNode 按自身粒度对齐（alignUp(4097,131072)=131072）"
        );
        // capacity 界：恰好铺满 → 再差一字节也不给。
        let mut d = UntypedAllocator::new(30, mk(12));
        assert_eq!(d.alloc_contiguous_pages(1), Some((30, 0, 0x4100_0000)));
        assert_eq!(d.alloc(0), None, "capacity 已满 ⇒ 1 B 也拒");
        let mut e = UntypedAllocator::new(30, mk(13));
        assert_eq!(
            e.alloc_contiguous_pages(4),
            None,
            "16 KiB 需求 > 8 KiB capacity"
        );
    }

    #[test]
    fn entry_mapped_translates_into_plan_vspace() {
        let elf = LoadedElf::parse(&minimal_exec_elf()).expect("elf");
        // WP-37.4（页模型）：页映射保持虚拟地址语义——entry 落在可执行
        // 页内即返回 entry 本身（非对齐段 entry 页内偏移不变）。
        let pages = vec![PageEntry {
            vaddr: 0x1000, // minimal_exec_elf 的段 vaddr（floor 页基）
            phys: 0,
            size: 0x1000,
            read: true,
            write: false,
            exec: true,
            src_file_offset: 0,
            dst_offset: 0,
            copy_len: 0,
        }];
        assert_eq!(entry_mapped(&pages, &elf), Ok(0x1000));
        // 负例：entry 不在任何可执行页内（旧"翻译"语义的 fixture——
        // 页在 0x4000_0000 而 entry 在 0x1000）→ fail-closed。
        let wrong = vec![PageEntry {
            vaddr: 0x4000_0000,
            phys: 0,
            size: 0x1000,
            read: true,
            write: false,
            exec: true,
            src_file_offset: 0,
            dst_offset: 0,
            copy_len: 0,
        }];
        assert_eq!(entry_mapped(&wrong, &elf), Err(SpawnError::PlanMismatch));
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
            src_file_offset: 0,
            dst_offset: 0,
            copy_len: 0,
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

    /// WP-37.3：VmPermissions 类型化换算（P0-03 判据——数据/栈/IPC/MMIO
    /// 页必须带 ExecuteNever，text 页不设 XN）。
    #[test]
    fn vm_permissions_derive_rights_and_attributes() {
        use eneros_sel4_sys::safe::{arm_vm_attributes, arm_vm_attributes_full};
        let rights = eneros_sel4_sys::safe::RightsTable::resolve();
        // text：R-X → rights=read，attr 无 ExecuteNever（可执行）。
        assert_eq!(VmPermissions::RX.cap_rights(&rights), rights.read);
        assert_eq!(VmPermissions::RX.vm_attributes(), arm_vm_attributes(false));
        assert!(!VmPermissions::RX.is_wx_violation());
        // rodata：R-- + XN。
        assert_eq!(
            VmPermissions::RO_NX.vm_attributes(),
            arm_vm_attributes(true)
        );
        // data/stack/IPC：RW- + XN（可缓存）。
        assert_eq!(
            VmPermissions::RW_NX.cap_rights(&rights),
            rights.read | rights.write
        );
        assert_eq!(
            VmPermissions::RW_NX.vm_attributes(),
            arm_vm_attributes(true)
        );
        // WP-37.3（复盘补全）：DEVICE 页 = XN + NonCacheable（不含
        // PageCacheable 位，device-nGnRE 语义）——与普通 RW_NX 不同。
        assert_eq!(
            VmPermissions::DEVICE_RW_NX.vm_attributes(),
            arm_vm_attributes_full(true, false)
        );
        assert_ne!(
            VmPermissions::DEVICE_RW_NX.vm_attributes(),
            VmPermissions::RW_NX.vm_attributes(),
            "device 页必须无 PageCacheable 位"
        );
        assert_eq!(VmPermissions::DEVICE_RW_NX.cache, CachePolicy::NonCacheable);
        assert_eq!(
            VmPermissions::DEVICE_RW_NX.shareability,
            Shareability::NonShareable
        );
        // 可执行 + 可写组合必须被标记为 W^X 违规（计划层拒绝）。
        let wx = VmPermissions {
            read: true,
            write: true,
            execute: true,
            cache: CachePolicy::Cacheable,
            shareability: Shareability::InnerShareable,
        };
        assert!(wx.is_wx_violation());
        // from_segment_flags 保留 ELF 段三态 + 普通内存默认（可缓存/内共享）。
        let rx = VmPermissions::from_segment_flags(&eneros_elf_loader::SegmentFlags {
            read: true,
            write: false,
            execute: true,
        });
        assert_eq!(rx, VmPermissions::RX);
    }

    /// WP-37.3（复盘补全）：cacheable 位换算（device 无 PageCacheable）。
    #[test]
    fn arm_vm_attributes_cacheable_bit_mapping() {
        use eneros_sel4_sys::safe::arm_vm_attributes_full;
        // 可缓存 + XN = Default(0x03) | XN(0x04) = 0x07。
        assert_eq!(arm_vm_attributes_full(true, true), 0x07);
        // device（非缓存）+ XN = Parity(0x02) | XN(0x04) = 0x06。
        assert_eq!(arm_vm_attributes_full(true, false), 0x06);
        // 可缓存、可执行 = 0x03。
        assert_eq!(arm_vm_attributes_full(false, true), 0x03);
    }

    /// WP-37.1（review should-fix）：optional 服务 plan_one 失败必须降级
    /// 而非阻断整个 boot；required 的 plan_one 失败仍传播中止。
    /// 用"parse 通过但 plan_one 失败"的 ELF（段 vaddr 非页对齐 → PlanMismatch）
    /// 构造可达路径（W+X 在 parse 层即被拦截，到不了 plan_one）。
    #[test]
    fn test_plan_spawn_optional_plan_one_error_degrades() {
        // 段 vaddr=0x1001（非页对齐）→ parse 通过（parse 不校验对齐），
        // plan_one Err(PlanMismatch)。entry 同步改 0x1001（落在段内）。
        // e_type 补丁 = 2（蓝图 67 WP-67.5 迁移注记：plan_one 错误分支属
        // ET_EXEC 路径——收敛后 ET_DYN 走 plan_one_dyn，非对齐由页模型
        // floor 吸收，不触发 PlanMismatch）。
        let mut misaligned = minimal_exec_elf();
        misaligned[16..18].copy_from_slice(&2u16.to_le_bytes());
        misaligned[24..32].copy_from_slice(&0x1001u64.to_le_bytes()); // e_entry
        misaligned[64 + 16..64 + 24].copy_from_slice(&0x1001u64.to_le_bytes()); // p_vaddr
        misaligned[64 + 24..64 + 32].copy_from_slice(&0x1001u64.to_le_bytes()); // p_paddr
        let raw = build_bundle(&[
            (
                "root-task",
                crate::bundle::MODULE_KIND_ROOT,
                minimal_exec_elf(),
            ),
            (
                "svc-fault",
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_exec_elf(),
            ),
            (
                "svc-proc-ts",
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_exec_elf(),
            ),
            (
                "svc-drv-opt",
                crate::bundle::MODULE_KIND_OPTIONAL,
                misaligned.clone(), // plan_one Err(PlanMismatch)
            ),
        ]);
        let bundle = InitBundle::parse(&raw).expect("bundle parse");
        let plan = plan_spawn_dyn_aware_with_stack(
            &bundle,
            0x4000_0000,
            0x1000,
            0x4000,
            100,
            0x200000,
            &[],
        )
        .expect("plan");
        assert_eq!(plan.service_count(), 2, "optional 的 plan_one 失败必须降级");
        assert!(plan.services.iter().all(|s| s.name != "svc-drv-opt"));

        // required 的 plan_one 失败（同样非页对齐）→ 中止 boot。
        let raw_req = build_bundle(&[
            (
                "root-task",
                crate::bundle::MODULE_KIND_ROOT,
                minimal_exec_elf(),
            ),
            ("svc-fault", crate::bundle::MODULE_KIND_REQUIRED, misaligned),
            (
                "svc-proc-ts",
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_exec_elf(),
            ),
        ]);
        let bundle_req = InitBundle::parse(&raw_req).expect("bundle parse");
        assert_eq!(
            plan_spawn_dyn_aware_with_stack(
                &bundle_req,
                0x4000_0000,
                0x1000,
                0x4000,
                100,
                0x200000,
                &[]
            ),
            Err(SpawnError::PlanMismatch),
            "required 的 plan_one 错误必须阻断"
        );
    }

    /// WP-37.4（接线）：plan_spawn_dyn_aware——ET_DYN 服务经统一 loader
    /// 布局（bias 平移 + relocated 镜像）；ET_EXEC 服务走 plan_one。
    #[test]
    fn test_plan_spawn_dyn_aware_routes_et_dyn_to_loader() {
        // minimal_exec_elf 是 ET_DYN（e_type=3）——全部服务走 loader。
        let raw = build_bundle(&[
            (
                "root-task",
                crate::bundle::MODULE_KIND_ROOT,
                minimal_exec_elf(),
            ),
            (
                "svc-fault",
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_exec_elf(),
            ),
            (
                "svc-proc-ts",
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_exec_elf(),
            ),
        ]);
        let bundle = InitBundle::parse(&raw).expect("bundle parse");
        let plan =
            plan_spawn_dyn_aware_with_stack(&bundle, 0x200000, 0x1000, 0x4000, 100, 0x200000, &[])
                .expect("dyn-aware plan");
        assert_eq!(plan.service_count(), 2);
        // 双 bias（WP-37.4）：首个实例 bias=0x200000，第二个实例按
        // ET_DYN_BIAS_STRIDE 递增——同一 PIE 两个不同装载基址。
        for (i, svc) in plan.services.iter().enumerate() {
            // ET_DYN：携带重定位镜像；entry = bias + 链接 entry(0x1000)。
            assert!(svc.relocated.is_some(), "ET_DYN 必须携带 relocated 镜像");
            let bias = 0x200000 + (i as u64) * ET_DYN_BIAS_STRIDE;
            assert_eq!(svc.entry, bias + 0x1000, "entry 必须按实例 bias 平移");
            // 装载后段 vaddr = bias + 链接 vaddr。
            assert!(svc.pages.iter().any(|p| p.vaddr == bias + 0x1000));
            // staging 镜像含段内容（min_loaded = bias + 段 vaddr；root-task
            // minimal_exec_elf 的段 p_offset=0 → 内容 = ELF magic 0x7F）。
            let reloc = svc.relocated.as_ref().unwrap();
            let slot = (bias + 0x1000 - reloc.min_loaded) as usize;
            assert_eq!(reloc.bytes[slot], 0x7F, "段内容应存在于镜像");
        }
        assert_ne!(
            plan.services[0].entry, plan.services[1].entry,
            "双 bias 实例 entry 必须不同（重定位目标不同）"
        );
        // required 语义保持：坏 ET_DYN ELF → RequiredElfFailed。
        let bad = build_bundle(&[
            (
                "root-task",
                crate::bundle::MODULE_KIND_ROOT,
                minimal_exec_elf(),
            ),
            (
                "svc-fault",
                crate::bundle::MODULE_KIND_REQUIRED,
                vec![0xFF, 0xEE, 0xDD],
            ),
            (
                "svc-proc-ts",
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_exec_elf(),
            ),
        ]);
        let bundle_bad = InitBundle::parse(&bad).expect("parse bad");
        assert_eq!(
            plan_spawn_dyn_aware_with_stack(
                &bundle_bad,
                0x200000,
                0x1000,
                0x4000,
                100,
                0x200000,
                &[]
            ),
            Err(SpawnError::RequiredElfFailed {
                name: "svc-fault".to_string()
            })
        );
    }

    /// WP-37.4（review should-fix）：ET_EXEC 服务经 plan_spawn_dyn_aware
    /// 走 plan_one 路径（relocated=None，链接地址布局）——ET_EXEC 回归。
    #[test]
    fn test_plan_spawn_dyn_aware_et_exec_uses_plan_one() {
        let mut exec_elf = minimal_exec_elf();
        exec_elf[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
        let raw = build_bundle(&[
            (
                "root-task",
                crate::bundle::MODULE_KIND_ROOT,
                exec_elf.clone(),
            ),
            (
                "svc-fault",
                crate::bundle::MODULE_KIND_REQUIRED,
                exec_elf.clone(),
            ),
            ("svc-proc-ts", crate::bundle::MODULE_KIND_REQUIRED, exec_elf),
        ]);
        let bundle = InitBundle::parse(&raw).expect("bundle parse");
        let plan =
            plan_spawn_dyn_aware_with_stack(&bundle, 0x200000, 0x1000, 0x4000, 100, 0x200000, &[])
                .expect("dyn-aware plan with ET_EXEC");
        assert_eq!(plan.service_count(), 2);
        for svc in &plan.services {
            assert!(svc.relocated.is_none(), "ET_EXEC 不携带 relocated 镜像");
            assert_eq!(svc.entry, 0x1000, "ET_EXEC entry 保持链接地址");
            // 链接地址布局（vaddr=0x1000，非 bias 平移）。
            assert!(svc.pages.iter().any(|p| p.vaddr == 0x1000));
        }
    }

    /// WP-37.2：enforcement 预算 = 计划实际占用（KiB 向上取整，不猜测）。
    #[test]
    fn plan_mem_kb_counts_pages_and_stack() {
        // 2 服务 ×（ELF 段页 + 栈 16KiB）。
        let raw = build_bundle(&[
            (
                "root-task",
                crate::bundle::MODULE_KIND_ROOT,
                minimal_exec_elf(),
            ),
            (
                "svc-fault",
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_data_elf_et_exec(),
            ),
            (
                "svc-proc-ts",
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_data_elf_et_exec(),
            ),
        ]);
        let bundle = InitBundle::parse(&raw).expect("bundle parse");
        let plan = plan_spawn_dyn_aware_with_stack(
            &bundle,
            0x4000_0000,
            0x1000,
            0x4000,
            100,
            0x200000,
            &[],
        )
        .expect("plan");
        // 每服务：ELF 段页 + 栈 16KiB + IPC buffer 页 4KiB（review nit 修复）。
        let per_svc: u64 = plan.services[0].pages.iter().map(|p| p.size).sum::<u64>()
            + plan.services[0].stack_size
            + 0x1000;
        let expect_kb = ((per_svc * 2) as usize).div_ceil(1024);
        assert_eq!(plan_mem_kb(&plan, 0x1000), expect_kb);
        // 空 plan（无服务）→ 0。
        let empty = SpawnPlan {
            root: SpawnRoot {
                name: "root-task".to_string(),
                stack_top: 0,
            },
            services: alloc::vec![],
        };
        assert_eq!(plan_mem_kb(&empty, 0x1000), 0);
    }

    /// WP-37.2：enforcement 主路径——plan 后 enforce 记账，release 扣减；
    /// 超预算真实拒绝（非空操作）。
    #[test]
    fn enforcement_after_plan_tracks_and_releases() {
        use eneros_tenant::manager::TenantManager;
        use eneros_tenant::{TenantEnforcer, TenantQuota, TenantStore};
        let mut store = TenantStore::new();
        let quota = TenantQuota {
            max_cpu_pct: 100.0,
            max_mem_kb: 4096,
            max_storage_kb: 65536,
            max_agents: 64,
            max_bandwidth_kbps: 1000,
        };
        store.create("system", quota).expect("system tenant");
        let mut enf = TenantEnforcer::new(store);

        let raw = build_bundle(&[
            (
                "root-task",
                crate::bundle::MODULE_KIND_ROOT,
                minimal_exec_elf(),
            ),
            (
                "svc-fault",
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_data_elf(),
            ),
            (
                "svc-proc-ts",
                crate::bundle::MODULE_KIND_REQUIRED,
                minimal_data_elf(),
            ),
        ]);
        let bundle = InitBundle::parse(&raw).expect("bundle parse");
        let plan = plan_spawn_dyn_aware_with_stack(
            &bundle,
            0x4000_0000,
            0x1000,
            0x4000,
            100,
            0x200000,
            &[],
        )
        .expect("plan");
        let mem_kb = plan_mem_kb(&plan, 0x1000);
        assert!(mem_kb > 0);

        // 系统租户 id = create 返回的 tenant_id（store 自增，首个=1）。
        enforce_spawn(&mut enf, 1, mem_kb, 100.0).expect("enforce 记账");
        let u = enf.usage(1).unwrap();
        assert_eq!(u.mem_kb, mem_kb, "记账 = 计划实际占用");
        assert_eq!(u.agents, 1);

        // spawn 失败 → release 扣减归零（main.rs Err 路径语义）。
        release_spawn(&mut enf, 1, mem_kb, 100.0).expect("release");
        assert_eq!(enf.usage(1).unwrap().mem_kb, 0);
        assert_eq!(enf.usage(1).unwrap().agents, 0);

        // 超预算必须拒绝（enforcement 真实生效，非空操作）。
        enforce_spawn(&mut enf, 1, 4096, 100.0).expect("4096 恰为上限");
        assert_eq!(
            enforce_spawn(&mut enf, 1, 1, 100.0),
            Err(eneros_tenant::PlatformError::QuotaExceeded),
            "超过 max_mem_kb 必须拒绝"
        );
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
            vaddr: 0x1000,
            phys: 0,
            size: 0x2000, // 2 页
            read: true,
            write: false,
            exec: true,
            src_file_offset: 0,
            dst_offset: 0,
            copy_len: 0,
        }];
        // entry 0x1000 落在可执行页内 → 返回 entry 本身（页模型）。
        assert_eq!(entry_mapped(&pages, &elf), Ok(0x1000));
        assert_eq!(pages[0].size / PAGE_SIZE, 2, "段按页对齐后跨 2 页");
    }

    /// D（Deepening-v1 §D.2）结构守卫：服务配置面必须是**命名字段会话**。
    ///
    /// 旧 `configure_service` 的 14 参位置表把 `fault_ep / ntfn_badge /
    /// eipc_mask / heartbeat_slot / wall_epoch` 五个同型 `u64` 排成相邻位
    ///（R-DE2：交换任意一对都类型合法、编译期不拦，只在真机上表现为服务收
    /// 错柄），返回面是 8 元组（R-DE3：解构点位序耦合）。二者已收口为
    /// `ServiceSpawnSession` / `ConfiguredService`。本测试锁新形态并反锁
    /// 旧形态回归（token 经 `concat!` 拼接防守卫字面量自匹配——service.rs
    /// `root_wire_faces_live_in_modules` 先例）。被测面是
    /// `#[cfg(target_os = "none")]` 的 `ServiceSpawnSession` /
    /// `ConfiguredService`，host 侧只能锁其源码形态；类型正确性由交叉编译
    /// （`cargo build -p eneros-root-task --target aarch64-unknown-none`）承担。
    #[test]
    fn service_configuration_face_is_named_field_session() {
        let src = include_str!("spawn.rs");
        // a) 输入面：宽位置表不得回归——全文件不得再需要位置参豁免。
        assert!(
            !src.contains(concat!("too_many_", "arguments")),
            "spawn.rs 再现宽位置参数表（R-DE2 位面复活）"
        );
        assert!(
            src.contains(concat!("session: Service", "SpawnSession")),
            "configure_service 形参必须收口为 ServiceSpawnSession"
        );
        // b) 五相邻 u64 中唯一由表达式算出的一位（badge = 服务索引+1）必须
        //    显式命名接线——旧形态它就躺在位置表第 9 位上。
        assert!(
            src.contains(concat!("ntfn_badge: svc_", "idx as u64 + 1")),
            "ntfn_badge 必须按名接线，不得回落到位置表序位"
        );
        // c) 两个 out-param（46.5 共享帧）仍以**命名字段**挂借位——位置表下
        //    它们是第 13/14 位，交换即两页帧 cap 互串。
        assert!(
            src.contains(concat!("net_page0_frame: &mut net_", "page0_frame")),
            "net_page0_frame 必须按名挂 &mut（out-param 借位不得回落位置表）"
        );
        // d) 返回面：命名字段结构体（旧 8 元组解构点自此消失）。
        assert!(
            src.contains(concat!("-> Result<Configured", "Service, SpawnError>")),
            "返回面必须是 ConfiguredService（R-DE3）"
        );
    }
}
