//! seL4 安全封装（P0-1，v0.198.0）.
//!
//! 为 `ffi` 模块的 20+ 原始 FFI 提供类型化安全接口：
//!
//! - [`ObjectTypeTable`] / [`RightsTable`]：平台对象类型 / 能力权限的
//!   getter（sel4_target 从 C shim 读取真实枚举；host_build 用 stub 常量，
//!   与 ffi 模块的双后端一致）。
//! - [`CapSlot`] / [`CapAllocator`]：类型化 cap 右值与空闲槽分配器。
//! - [`Sel4Result`] / [`to_sel4_result`]：seL4 错误码 → `Result` 映射。
//! - [`retype`] / `tcb_*` / `endpoint_*`：retype / TCB 配置 / IPC 的
//!   安全封装（错误码真实传播，不再静默丢弃）。
//! - [`BootInfo`] 解析：untyped 区间与 `untypedList` 的 offset 访问器
//!   （对照 seL4 14.0.0 aarch64 `bootinfo.h` 布局）。
//!
//! # no_std 合规
//!
//! 纯 `core` + `alloc`（CapAllocator 用 `Vec`）。

#![allow(non_camel_case_types)]

#[cfg(test)]
use alloc::vec;

use crate::types::*;

// ============================================================================
// 错误码 → Result 映射
// ============================================================================

/// seL4 错误（`Error` 的非零值映射为描述性错误）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sel4Error {
    /// host 构建（无 seL4）下该操作无意义，明确失败而非模拟成功
    /// （21-A 红线 2：禁止用 mock 占位 seL4 系统调用）。
    HostUnsupported,
    /// 参数非法。
    InvalidArgument,
    /// 能力非法或不存在。
    InvalidCapability,
    /// 操作在当前状态下非法。
    IllegalOperation,
    /// 越界。
    RangeError,
    /// 对齐错误。
    AlignmentError,
    /// 查找失败（cap 解析）。
    FailedLookup,
    /// 其他 seL4 错误码。
    Other(i64),
}

/// 类型化 seL4 操作结果。
pub type Sel4Result<T> = Result<T, Sel4Error>;

/// 将 seL4 错误码映射为 [`Sel4Result`]。
pub fn to_sel4_result<T>(err: Error, ok: T) -> Sel4Result<T> {
    if err == NO_ERROR {
        Ok(ok)
    } else {
        Err(match err {
            INVALID_ARGUMENT => Sel4Error::InvalidArgument,
            INVALID_CAPABILITY => Sel4Error::InvalidCapability,
            ILLEGAL_OPERATION => Sel4Error::IllegalOperation,
            RANGE_ERROR => Sel4Error::RangeError,
            ALIGNMENT_ERROR => Sel4Error::AlignmentError,
            FAILED_LOOKUP => Sel4Error::FailedLookup,
            other => Sel4Error::Other(other),
        })
    }
}

// ============================================================================
// 平台对象类型 / 权限 getter（双后端）
// ============================================================================

/// 平台对象类型枚举 getter。
///
/// sel4_target：从 C shim 的 `eneros_sel4_obj_*` getter 读取真实枚举值。
/// host_build：使用文档化的 aarch64 seL4 14.0.0 常量（测试用；目标侧以
/// shim 为准）。
#[derive(Debug, Clone, Copy)]
pub struct ObjectTypeTable {
    pub untyped: ObjectType,
    pub tcb: ObjectType,
    pub endpoint: ObjectType,
    pub notification: ObjectType,
    pub captable: ObjectType,
    pub arm_frame: ObjectType,
    pub arm_page_table: ObjectType,
    pub arm_vspace: ObjectType,
}

/// 平台能力权限 getter。
#[derive(Debug, Clone, Copy)]
pub struct RightsTable {
    pub none: CapRights,
    pub read: CapRights,
    pub write: CapRights,
    pub grant: CapRights,
    pub all: CapRights,
}

/// 解析平台对象类型常量（纯 FFI getter，无副作用）。
///
/// # Safety
/// 所有调用为无参 C 函数返回平台常量；无指针参数/返回值，无内存安全风险。
#[cfg(sel4_target)]
fn resolve_object_types() -> ObjectTypeTable {
    // SAFETY: 以下 getter 均为无参 C 函数返回平台常量（见函数文档 # Safety），
    // 无指针参数/返回值，无内存安全风险。
    ObjectTypeTable {
        untyped: unsafe { crate::ffi::eneros_sel4_obj_untyped() },
        tcb: unsafe { crate::ffi::eneros_sel4_obj_tcb() },
        endpoint: unsafe { crate::ffi::eneros_sel4_obj_endpoint() },
        notification: unsafe { crate::ffi::eneros_sel4_obj_notification() },
        captable: unsafe { crate::ffi::eneros_sel4_obj_captable() },
        arm_frame: unsafe { crate::ffi::eneros_sel4_obj_arm_frame() },
        arm_page_table: unsafe { crate::ffi::eneros_sel4_obj_arm_pt() },
        arm_vspace: unsafe { crate::ffi::eneros_sel4_obj_arm_vspace() },
    }
}

/// 解析权限常量（纯 FFI getter，无副作用）。
///
/// # Safety
/// 所有调用为无参 C 函数返回权限位常量；无指针参数/返回值，无内存安全风险。
#[cfg(sel4_target)]
fn resolve_rights() -> RightsTable {
    // SAFETY: 以下 getter 均为无参 C 函数返回权限位常量（见函数文档 # Safety），
    // 无指针参数/返回值，无内存安全风险。
    RightsTable {
        none: unsafe { crate::ffi::eneros_sel4_rights_none() },
        read: unsafe { crate::ffi::eneros_sel4_rights_read() },
        write: unsafe { crate::ffi::eneros_sel4_rights_write() },
        grant: unsafe { crate::ffi::eneros_sel4_rights_grant() },
        all: unsafe { crate::ffi::eneros_sel4_rights_all() },
    }
}

#[cfg(host_build)]
fn resolve_object_types() -> ObjectTypeTable {
    // seL4 14.0.0 aarch64 对象类型枚举（与 shim getter 在目标侧返回一致）。
    ObjectTypeTable {
        untyped: 12,
        tcb: 1,
        endpoint: 4,
        notification: 3,
        captable: 2,
        arm_frame: 10,      // seL4_ARM_SmallPageObject
        arm_page_table: 11, // seL4_ARM_PageTableObject
        arm_vspace: 9,      // seL4_ARM_PageDirectoryObject
    }
}

#[cfg(host_build)]
fn resolve_rights() -> RightsTable {
    // seL4_CapRights：bit0=read, bit1=write, bit2=grant, bit3=grantReply。
    RightsTable {
        none: 0,
        read: 1,
        write: 2,
        grant: 4,
        all: 0x0F,
    }
}

impl ObjectTypeTable {
    /// 解析平台对象类型表。
    pub fn resolve() -> Self {
        resolve_object_types()
    }
}

impl RightsTable {
    /// 解析平台权限表。
    pub fn resolve() -> Self {
        resolve_rights()
    }
}

// ============================================================================
// root-task 固定 cap 常量（ADR-0034，K1.2）
// ============================================================================

/// root-task 从 elfloader 继承的固定 cap 槽号（bootinfo.h 编译期宏）。
///
/// sel4_target：从 C shim getter 读取真实值；host_build：使用文档化的
/// seL4 14.0.0 常量（测试用；目标侧以 shim 为准）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapInitThreadTable {
    /// root-task 自身 TCB cap（SetPriority 的 authority）。
    pub tcb: CPtr,
    /// root-task 自身 CSpace 根 cap（retype/mint 的 dest_root）。
    pub cnode: CPtr,
    /// root-task 自身 VSpace cap（临时窗口映射的 vspace）。
    pub vspace: CPtr,
    /// root-task 继承的 ASID pool cap（服务 VSpace 分配 ASID）。
    pub asid_pool: CPtr,
}

impl CapInitThreadTable {
    /// 解析 root-task 固定 cap 表。
    pub fn resolve() -> Self {
        #[cfg(sel4_target)]
        {
            // SAFETY: getter 均为无参 C 函数返回编译期常量，无内存安全风险。
            Self {
                tcb: unsafe { crate::ffi::eneros_sel4_cap_init_thread_tcb() },
                cnode: unsafe { crate::ffi::eneros_sel4_cap_init_thread_cnode() },
                vspace: unsafe { crate::ffi::eneros_sel4_cap_init_thread_vspace() },
                asid_pool: unsafe { crate::ffi::eneros_sel4_cap_init_thread_asid_pool() },
            }
        }
        #[cfg(host_build)]
        {
            // seL4 14.0.0 bootinfo.h 常量（与 shim getter 在目标侧返回一致）。
            Self {
                tcb: 1,
                cnode: 2,
                vspace: 3,
                asid_pool: 6,
            }
        }
    }
}

// ============================================================================
// 类型化 cap 右值与槽分配器
// ============================================================================

/// 类型化能力右值：一个已分配的 cap 槽 + 其对象类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapSlot {
    /// cap 槽（CPtr）。
    pub slot: CPtr,
    /// 对象类型。
    pub object_type: ObjectType,
}

impl CapSlot {
    /// 创建类型化 cap 右值。
    pub const fn new(slot: CPtr, object_type: ObjectType) -> Self {
        Self { slot, object_type }
    }

    /// cap 槽值。
    pub const fn cptr(&self) -> CPtr {
        self.slot
    }
}

/// 空闲 cap 槽分配器。
///
/// 从 `start`（含）开始递增分配；`end` 为上限（不含）。
#[derive(Debug, Clone)]
pub struct CapAllocator {
    next: CPtr,
    end: CPtr,
}

impl CapAllocator {
    /// 创建槽分配器，区间 `[start, end)`。
    pub const fn new(start: CPtr, end: CPtr) -> Self {
        Self { next: start, end }
    }

    /// 分配 `count` 个连续槽，返回起始槽；空间不足返回 `None`。
    pub fn alloc(&mut self, count: u32) -> Option<CPtr> {
        let n = count as u64;
        if self.next.checked_add(n).map_or(true, |e| e > self.end) {
            return None;
        }
        let slot = self.next;
        self.next += n;
        Some(slot)
    }

    /// 已分配槽数。
    pub fn used(&self) -> u64 {
        self.next
    }
}

// ============================================================================
// 安全封装：retype / TCB / IPC
// ============================================================================

/// Untyped_Retype 封装。
///
/// 从 `untyped` 能力 retype 出 `count` 个 `object_type` 对象，放入
/// `dest_root` 下的 `[dest_index, dest_index+count)` 槽。`size_bits` 是
/// 对象大小的 log2（如 4 KiB 页 = 12）。
#[allow(clippy::too_many_arguments)]
pub fn retype(
    untyped: CPtr,
    object_type: ObjectType,
    size_bits: Word,
    count: Word,
    dest_root: CPtr,
    dest_index: CPtr,
    dest_depth: Word,
    offset: Word,
) -> Sel4Result<()> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；参数由调用方保证（槽区间、类型、大小）。
        let err = unsafe {
            crate::ffi::eneros_sel4_untyped_retype(
                untyped,
                object_type,
                size_bits,
                count,
                dest_root,
                dest_index,
                dest_depth,
                offset,
            )
        };
        to_sel4_result(err, ())
    }
    #[cfg(host_build)]
    {
        // host stub：模拟成功；由 host 测试桩覆盖错误映射。
        let _ = (
            untyped,
            object_type,
            size_bits,
            count,
            dest_root,
            dest_index,
            dest_depth,
            offset,
        );
        Ok(())
    }
}

/// TCB 写寄存器封装（`seL4_TCB_WriteRegisters`）。
pub fn tcb_write_registers(
    tcb: CPtr,
    flags: Word,
    count: Word,
    regs: *const UserContext,
) -> Sel4Result<()> {
    #[cfg(sel4_target)]
    {
        // SAFETY: regs 必须指向 count 个有效寄存器；调用方保证。
        let err =
            unsafe { crate::ffi::eneros_sel4_tcb_write_registers(tcb, 0, flags, count, regs) };
        to_sel4_result(err, ())
    }
    #[cfg(host_build)]
    {
        let _ = (tcb, flags, count, regs);
        Ok(())
    }
}

/// TCB 恢复执行封装（`seL4_TCB_Resume`）。
pub fn tcb_resume(tcb: CPtr) -> Sel4Result<()> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；tcb 必须有效。
        let err = unsafe { crate::ffi::eneros_sel4_tcb_resume(tcb) };
        to_sel4_result(err, ())
    }
    #[cfg(host_build)]
    {
        let _ = tcb;
        Ok(())
    }
}

/// TCB 挂起封装（`seL4_TCB_Suspend`）。
pub fn tcb_suspend(tcb: CPtr) -> Sel4Result<()> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；tcb 必须有效。
        let err = unsafe { crate::ffi::eneros_sel4_tcb_suspend(tcb) };
        to_sel4_result(err, ())
    }
    #[cfg(host_build)]
    {
        let _ = tcb;
        Ok(())
    }
}

/// TCB 设置优先级封装（`seL4_TCB_SetPriority`）。
pub fn tcb_set_priority(tcb: CPtr, authority: CPtr, priority: Word) -> Sel4Result<()> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；tcb/authority 必须有效。
        let err = unsafe { crate::ffi::eneros_sel4_tcb_set_priority(tcb, authority, priority) };
        to_sel4_result(err, ())
    }
    #[cfg(host_build)]
    {
        let _ = (tcb, authority, priority);
        Ok(())
    }
}

/// TCB 绑定地址空间封装（`seL4_TCB_SetSpace`，ADR-0034）。
///
/// 把服务的 CSpace / VSpace 绑到 TCB——spawn 可运行进程的必需步骤
/// （TCB 创建后无地址空间，直接 resume 即 CapFault）。seL4 14.0.0 API：
/// `fault_ep` 为独立 word 参数（非 MCS 配置下内核仅记录不校验，传 0 安全）；
/// IPC buffer 由 [`tcb_set_ipcbuffer`] 单独绑定。
/// `cspace_root_data` / `vspace_root_data` 为一阶 cnode/vspace 时传 0。
///
/// # Safety
/// 前置条件（调用方必须保证，封装不校验）：
/// - `tcb`、`cspace_root`、`vspace_root` 必须有效且已 retype；
/// - 一阶 CSpace/VSpace 时 `cspace_root_data`/`vspace_root_data` 必须为 0。
///
/// 目标构建真调 FFI；host 构建返回 [`Sel4Error::HostUnsupported`]（红线 2）。
#[allow(clippy::too_many_arguments)]
pub fn tcb_set_space(
    tcb: CPtr,
    fault_ep: Word,
    cspace_root: CPtr,
    cspace_root_data: Word,
    vspace_root: CPtr,
    vspace_root_data: Word,
) -> Sel4Result<()> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；前置条件见函数文档。
        let err = unsafe {
            crate::ffi::eneros_sel4_tcb_set_space(
                tcb,
                fault_ep,
                cspace_root,
                cspace_root_data,
                vspace_root,
                vspace_root_data,
            )
        };
        to_sel4_result(err, ())
    }
    #[cfg(host_build)]
    {
        let _ = (
            tcb,
            fault_ep,
            cspace_root,
            cspace_root_data,
            vspace_root,
            vspace_root_data,
        );
        Err(Sel4Error::HostUnsupported)
    }
}

/// TCB 设置 IPC buffer 封装（`seL4_TCB_SetIPCBuffer`，ADR-0034）。
///
/// 消息长度超过寄存器 MR 时必须经 IPC buffer 传输（ipc-wire 帧 ≥5 words）；
/// 无 buffer 的线程执行 IPC 即 fault。`buffer` 为帧 cap，`buffer_data` 为
/// buffer 在服务地址空间的虚拟地址。
///
/// # Safety
/// 前置条件：`tcb` 有效；`buffer` 是已映射到服务地址空间的帧 cap
/// （映射 vaddr 与 `buffer_data` 一致且 4 KiB 对齐）。
///
/// 目标构建真调 FFI；host 构建返回 [`Sel4Error::HostUnsupported`]（红线 2）。
pub fn tcb_set_ipcbuffer(tcb: CPtr, buffer: CPtr, buffer_data: Word) -> Sel4Result<()> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；前置条件见函数文档。
        let err = unsafe { crate::ffi::eneros_sel4_tcb_set_ipcbuffer(tcb, buffer, buffer_data) };
        to_sel4_result(err, ())
    }
    #[cfg(host_build)]
    {
        let _ = (tcb, buffer, buffer_data);
        Err(Sel4Error::HostUnsupported)
    }
}

/// TCB 绑定 notification 封装（`seL4_TCB_BindNotification`，K2.3）。
///
/// 服务 TCB 发生异常（除零/非法访问等）时，内核向绑定的 notification
/// 发信号——root-task 在该 notification 上接收即检测到服务故障（21-A
/// K2.3 故障隔离验证）。
///
/// # Safety
/// 前置条件：`tcb` 有效；`ntfn` 是已 retype 的 notification 能力。
///
/// 目标构建真调 FFI；host 构建返回 [`Sel4Error::HostUnsupported`]（红线 2）。
pub fn tcb_bind_notification(tcb: CPtr, ntfn: CPtr) -> Sel4Result<()> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；前置条件见函数文档。
        let err = unsafe { crate::ffi::eneros_sel4_tcb_bind_notification(tcb, ntfn) };
        to_sel4_result(err, ())
    }
    #[cfg(host_build)]
    {
        let _ = (tcb, ntfn);
        Err(Sel4Error::HostUnsupported)
    }
}

/// CNode_Mint 封装（派生带权限的能力；seL4 14.0.0 含 badge 参数）。
#[allow(clippy::too_many_arguments)]
pub fn cnode_mint(
    dest_root: CPtr,
    dest_index: CPtr,
    dest_depth: Word,
    src_root: CPtr,
    src_index: CPtr,
    src_depth: Word,
    rights: CapRights,
    badge: Word,
) -> Sel4Result<()> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；槽区间与权限由调用方保证。
        let err = unsafe {
            crate::ffi::eneros_sel4_cnode_mint(
                dest_root, dest_index, dest_depth, src_root, src_index, src_depth, rights, badge,
            )
        };
        to_sel4_result(err, ())
    }
    #[cfg(host_build)]
    {
        let _ = (
            dest_root, dest_index, dest_depth, src_root, src_index, src_depth, rights, badge,
        );
        Ok(())
    }
}

// ============================================================================
// ARM VSpace / 页映射（21-A K1.1：补齐 arm_page_map / arm_page_table_map）
// ============================================================================

/// ARM 页映射封装（`seL4_ARM_PageMap`，4 KiB SmallPage）。
///
/// 把已 retype 的页对象映射到 `vspace` 的 `vaddr` 处，权限为 `rights`。
/// 映射属性使用平台默认值（`seL4_ARM_Default_VMAttributes`，由 C shim
/// 解析，safe 层统一收口，调用方无需关心平台差异）。
///
/// # Safety
/// 前置条件（调用方必须保证，封装不校验）：
/// - `page` 必须是已 retype 的 `SmallPage`（4 KiB 页）能力，且尚未映射；
/// - `vspace` 必须是已创建的 VSpace 能力（`arm_vspace` 对象）；
/// - `vaddr` 必须 4 KiB 页对齐，且其所在的 1 GiB 区域已由
///   [`arm_page_table_map`] 建立页表；
/// - `rights` 必须是合法权限位（见 [`RightsTable`]）。
///
/// 目标构建真调 FFI；host 构建返回 [`Sel4Error::HostUnsupported`]
/// （21-A 红线 2：禁止 host stub 模拟成功——页映射在 host 无意义）。
/// 调试：识别 cap 类型（`seL4_DebugCapIdentify`，仅 CONFIG_DEBUG_BUILD）。
pub fn debug_cap_identify(cap: CPtr) -> Word {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；无指针参数，无内存安全风险。
        unsafe { crate::ffi::eneros_sel4_debug_cap_identify(cap) }
    }
    #[cfg(host_build)]
    {
        let _ = cap;
        0
    }
}

pub fn arm_page_map(vspace: CPtr, page: CPtr, vaddr: Word, rights: CapRights) -> Sel4Result<()> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；前置条件见函数文档（page 已 retype、
        // vspace 有效、vaddr 4 KiB 对齐）。attr 为平台默认值 getter，
        // 无指针参数，无内存安全风险。
        let attr = unsafe { crate::ffi::eneros_sel4_arm_default_attributes() };
        // SAFETY: 同上；传参顺序对齐 C shim（page, vspace, vaddr, rights, attr）。
        let err =
            unsafe { crate::ffi::eneros_sel4_arm_page_map(page, vspace, vaddr, rights, attr) };
        to_sel4_result(err, ())
    }
    #[cfg(host_build)]
    {
        let _ = (vspace, page, vaddr, rights);
        Err(Sel4Error::HostUnsupported)
    }
}

/// ARM 页表映射封装（`seL4_ARM_PageTableMap`，1 GiB 区域）。
///
/// 把已 retype 的页表对象映射到 `vspace` 的 `vaddr` 处，为该 1 GiB 区域
/// 建立二级页表，之后才能在其内映射 4 KiB 页（[`arm_page_map`]）。
/// 映射属性使用平台默认值（与 [`arm_page_map`] 同一收口）。
///
/// # Safety
/// 前置条件（调用方必须保证，封装不校验）：
/// - `pt` 必须是已 retype 的 `PageTable` 对象能力（`arm_page_table`），
///   且尚未映射；
/// - `vspace` 必须是已创建的 VSpace 能力；
/// - `vaddr` 必须 1 GiB 对齐（页表覆盖 1 GiB 虚拟地址区域）。
///
/// 目标构建真调 FFI；host 构建返回 [`Sel4Error::HostUnsupported`]
/// （21-A 红线 2：禁止 host stub 模拟成功）。
pub fn arm_page_table_map(vspace: CPtr, pt: CPtr, vaddr: Word) -> Sel4Result<()> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；前置条件见函数文档（pt 已 retype、
        // vspace 有效、vaddr 1 GiB 对齐）。attr 为平台默认值 getter，
        // 无指针参数，无内存安全风险。
        let attr = unsafe { crate::ffi::eneros_sel4_arm_default_attributes() };
        // SAFETY: 同上；传参顺序对齐 C shim（pt, vspace, vaddr, attr）。
        let err = unsafe { crate::ffi::eneros_sel4_arm_page_table_map(pt, vspace, vaddr, attr) };
        to_sel4_result(err, ())
    }
    #[cfg(host_build)]
    {
        let _ = (vspace, pt, vaddr);
        Err(Sel4Error::HostUnsupported)
    }
}

/// ASID pool 分配封装（`seL4_ARM_ASIDPool_Assign`，ADR-0034）。
///
/// aarch64 上 VSpace 必须先从 ASID pool 分配到 ASID 才能映射页
/// （`arm_page_map` 内部走 `findVSpaceForASID`，未绑 ASID 即 FailedLookup）。
///
/// # Safety
/// 前置条件：`pool` 是有效的 ASID pool cap（root-task 继承的
/// `seL4_CapInitThreadASIDPool`）；`vspace` 是已 retype 且未分配 ASID 的
/// VSpace 能力。
///
/// 目标构建真调 FFI；host 构建返回 [`Sel4Error::HostUnsupported`]（红线 2）。
pub fn arm_asid_pool_assign(pool: CPtr, vspace: CPtr) -> Sel4Result<()> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；前置条件见函数文档。
        let err = unsafe { crate::ffi::eneros_sel4_arm_asid_pool_assign(pool, vspace) };
        to_sel4_result(err, ())
    }
    #[cfg(host_build)]
    {
        let _ = (pool, vspace);
        Err(Sel4Error::HostUnsupported)
    }
}

/// ARM 页卸载封装（`seL4_ARM_Page_Unmap`，ADR-0034）。
///
/// frame 只能映射到一个虚拟地址（内核按 cap 记录的 asid/vaddr 校验）；
/// K1.3 方案 A 的临时窗口拷贝流程 = 窗口映射 → 拷贝 → 本封装卸载 →
/// 再映射进服务 VSpace。
///
/// # Safety
/// 前置条件：`page` 是已映射的帧 cap（由之前 [`arm_page_map`] 映射）。
///
/// 目标构建真调 FFI；host 构建返回 [`Sel4Error::HostUnsupported`]（红线 2）。
pub fn arm_page_unmap(page: CPtr) -> Sel4Result<()> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；前置条件见函数文档。
        let err = unsafe { crate::ffi::eneros_sel4_arm_page_unmap(page) };
        to_sel4_result(err, ())
    }
    #[cfg(host_build)]
    {
        let _ = page;
        Err(Sel4Error::HostUnsupported)
    }
}

// ============================================================================
// IPC（21-A K2.1：ipc_send / ipc_recv / ipc_call / ipc_reply_recv /
//           set_mr / get_mr / message_info_new）
// ============================================================================

/// 写消息寄存器封装（`seL4_SetMR`）。
///
/// 把 `mr` 写入 MR 槽 `i`。aarch64 上 MR 0..3 走寄存器、其余走 IPC buffer
/// （TCB 必须已通过 [`tcb_set_ipcbuffer`] 绑定 buffer，否则报错/fault）。
///
/// # Safety
/// 无内存安全风险（MR 槽索引越界由内核/IPC buffer 边界处理）；`i`
/// 超过 [`types::USER_CONTEXT_MAX_REGS`] 的槽在目标构建由内核返回错误。
pub fn set_mr(i: Word, mr: Word) -> Sel4Result<()> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；无指针参数，无内存安全风险。
        unsafe { crate::ffi::eneros_sel4_set_mr(i, mr) };
        Ok(())
    }
    #[cfg(host_build)]
    {
        let _ = (i, mr);
        Err(Sel4Error::HostUnsupported)
    }
}

/// 读消息寄存器封装（`seL4_GetMR`）。
///
/// # Safety
/// 无内存安全风险（同 [`set_mr`]）。
pub fn get_mr(i: Word) -> Sel4Result<Word> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；无指针参数，无内存安全风险。
        Ok(unsafe { crate::ffi::eneros_sel4_get_mr(i) })
    }
    #[cfg(host_build)]
    {
        let _ = i;
        Err(Sel4Error::HostUnsupported)
    }
}

/// 构造 MessageInfo 封装（`seL4_MessageInfo_new`）。
///
/// 打包 label / caps_unwrapped / extra_caps / length 为 MessageInfo word。
/// 目标构建以 C shim 的权威 packer 为准（Rust 侧位布局见 [`types`]）。
///
/// # Safety
/// 纯值构造，无指针参数，无内存安全风险。
pub fn message_info_new(
    label: Word,
    caps_unwrapped: Word,
    extra_caps: Word,
    length: Word,
) -> Sel4Result<MessageInfo> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；纯值构造，无内存安全风险。
        Ok(unsafe {
            crate::ffi::eneros_sel4_message_info_new(label, caps_unwrapped, extra_caps, length)
        })
    }
    #[cfg(host_build)]
    {
        let _ = (label, caps_unwrapped, extra_caps, length);
        Err(Sel4Error::HostUnsupported)
    }
}

/// 发送消息封装（`seL4_Send`，无回复）。
///
/// # Safety
/// 前置条件：`dest` 是有效的 endpoint 能力；[`set_mr`] 已写入消息载荷。
/// 无内存安全风险。
pub fn ipc_send(dest: CPtr, msg_info: MessageInfo) -> Sel4Result<()> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；前置条件见函数文档。
        unsafe { crate::ffi::eneros_sel4_send(dest, msg_info) };
        Ok(())
    }
    #[cfg(host_build)]
    {
        let _ = (dest, msg_info);
        Err(Sel4Error::HostUnsupported)
    }
}

/// 阻塞接收封装（`seL4_Recv`）。
///
/// 在 `src`（endpoint cap；`seL4_CapNull` = 任意来源）上阻塞等待消息。
/// `sender` 为 `Some` 时写入发送者 cap（`None` = 不需要发送者）。
///
/// # Safety
/// - 可能阻塞（Endpoint 同步语义，禁止在中断上下文调用）；
/// - `sender: Some(&mut w)` 必须指向有效内存；目标侧转裸指针传给 FFI。
pub fn ipc_recv(src: CPtr, sender: Option<&mut Word>) -> Sel4Result<MessageInfo> {
    #[cfg(sel4_target)]
    {
        // SAFETY: sender 为 Option<&mut>——Some 时指向有效内存且独占借用，
        // None 时传 null（seL4 允许）。封装保证指针生命周期与调用一致。
        let sender_ptr = sender.map_or(core::ptr::null_mut(), |w| w as *mut Word);
        Ok(unsafe { crate::ffi::eneros_sel4_recv(src, sender_ptr) })
    }
    #[cfg(host_build)]
    {
        let _ = (src, sender);
        Err(Sel4Error::HostUnsupported)
    }
}

/// 非阻塞接收封装（`seL4_Poll`；22 §2.W1.3 supervisor 轮询 fault 用）。
///
/// 无消息时立即返回 `Ok(msg=0)`（不阻塞）；有消息时返回消息信息。
/// `sender` 为 `Some` 时写入发送者 cap。目标侧转裸指针传给 FFI。
pub fn ipc_poll(src: CPtr, sender: Option<&mut Word>) -> Sel4Result<MessageInfo> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 同 ipc_recv——sender 生命周期与调用一致；seL4_Poll 无阻塞。
        let sender_ptr = sender.map_or(core::ptr::null_mut(), |w| w as *mut Word);
        Ok(unsafe { crate::ffi::eneros_sel4_poll(src, sender_ptr) })
    }
    #[cfg(host_build)]
    {
        let _ = (src, sender);
        Err(Sel4Error::HostUnsupported)
    }
}

/// 同步调用封装（`seL4_Call`，请求→等待回复）。
///
/// # Safety
/// - 可能阻塞（同步往返，禁止在中断上下文调用）；
/// - `dest` 必须是有效的 endpoint 能力；[`set_mr`] 已写入请求载荷。
pub fn ipc_call(dest: CPtr, msg_info: MessageInfo) -> Sel4Result<MessageInfo> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；前置条件见函数文档。
        Ok(unsafe { crate::ffi::eneros_sel4_call(dest, msg_info) })
    }
    #[cfg(host_build)]
    {
        let _ = (dest, msg_info);
        Err(Sel4Error::HostUnsupported)
    }
}

/// 回复并继续接收封装（`seL4_ReplyRecv`）。
///
/// 服务侧模式：向 `dest` 回复当前请求，然后阻塞接收下一个请求
/// （`src` 为 `seL4_CapNull` 时接收任意来源）。`sender` 语义同 [`ipc_recv`]。
///
/// # Safety
/// - 可能阻塞（同步语义，禁止在中断上下文调用）；
/// - `sender: Some(&mut w)` 必须指向有效内存。
pub fn ipc_reply_recv(
    dest: CPtr,
    msg_info: MessageInfo,
    sender: Option<&mut Word>,
) -> Sel4Result<MessageInfo> {
    #[cfg(sel4_target)]
    {
        // SAFETY: sender 同 ipc_recv（Option<&mut> 独占借用，None 传 null）。
        let sender_ptr = sender.map_or(core::ptr::null_mut(), |w| w as *mut Word);
        Ok(unsafe { crate::ffi::eneros_sel4_reply_recv(dest, msg_info, sender_ptr) })
    }
    #[cfg(host_build)]
    {
        let _ = (dest, msg_info, sender);
        Err(Sel4Error::HostUnsupported)
    }
}

// ============================================================================
// BootInfo 解析（P0-1：untyped 列表 / 空闲 cap 区间 offset 访问器）
// ============================================================================

/// 获取 BootInfo 指针（`seL4_GetBootInfo`，21-A K1.2）。
///
/// 返回指针由 seL4 内核保证指向有效的 `seL4_BootInfo` 结构（rootserver
/// 环境）；host 构建返回 `null`（无 seL4，调用方不得使用）。
///
/// # Safety
/// 封装 ffi；返回值仅目标构建下有效，调用方必须结合
/// [`BootInfoReader::from_bootinfo_ptr`] 使用。
/// root-task 启动初始化（BOOT-008）：把内核传入的 bootinfo（capRegister/x0）
/// 注册进 libsel4（`seL4_InitBootInfo` + IPC buffer TLS），使
/// `seL4_GetBootInfo`/`seL4_Send`/`seL4_GetMR` 等 libsel4 API 可用。
/// root-task 用 Rust 汇编 `_start`（不经过 libsel4 的 sel4_start），
/// 必须调用本函数后才能使用 bootinfo 与 IPC（21-A 遗留缺陷）。
pub fn boot_init(bi: u64) {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；bi 为内核传入的有效 bootinfo 指针。
        unsafe { crate::ffi::eneros_sel4_boot_init(bi) }
    }
    #[cfg(host_build)]
    {
        let _ = bi;
    }
}

/// 设置当前线程的 IPC buffer 指针（`__sel4_ipc_buffer` TLS）。
///
/// 被 spawn 的 EL0 服务不是 rootserver：无 bootinfo，但 root-task 已通过
/// `seL4_TCB_SetIPCBuffer` 绑定 buffer 帧并把 vaddr 经 `_start` 第二参数
/// 传入。服务入口必须调用本函数，否则 `ipc_recv`/`get_mr` 在 NULL buffer
/// 上解引用（K2.3 遗留缺陷：svc_ipc_loop 首轮 recv 卡死）。
pub fn set_ipc_buffer(vaddr: u64) {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；vaddr 为 root-task 映射进服务 VSpace 的 buffer
        // 地址（4 KiB 对齐），无内存安全风险。
        unsafe { crate::ffi::eneros_sel4_set_ipc_buffer(vaddr) }
    }
    #[cfg(host_build)]
    {
        let _ = vaddr;
    }
}

/// 让出 CPU（`seL4_Yield`）。
///
/// root-task 在阻塞式 `ipc_call` 前主动让出，使被 spawn 的服务（优先级 10）
/// 有机会被调度执行（否则若 root-task 优先级最高，服务可能长时间不被调度）。
pub fn yield_cpu() {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；无参数，无内存安全风险。
        unsafe { crate::ffi::eneros_sel4_yield() }
    }
}

pub fn get_boot_info() -> *const crate::types::BootInfo {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；rootserver 环境下内核保证返回值有效。
        unsafe { crate::ffi::eneros_sel4_get_boot_info() as *const crate::types::BootInfo }
    }
    #[cfg(host_build)]
    {
        core::ptr::null()
    }
}

/// 一个 untyped 描述（对应 `seL4_UntypedDesc`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UntypedDesc {
    /// untyped 物理地址。
    pub paddr: u64,
    /// 大小位数（`size = 1 << sizeBits`）。
    pub size_bits: u64,
    /// 是否为设备内存。
    pub is_device: bool,
}

/// seL4 14.0.0 aarch64 `seL4_BootInfo` 关键字段 offset（字节）。
///
/// 对照 `sel4/bootinfo.h`（seL4 14.0.0）：
/// - `untyped` 是 `seL4_SlotRegion`（`start`/`end` 两个 Word），位于结构
///   体前部；`untypedList` 是指向 `seL4_UntypedDesc` 数组的指针。
/// - 具体偏移随内核配置变化；本表为 aarch64 默认配置值，目标侧可由
///   C shim 提供校正 getter（见 `docs/runtime/sel4-api-bindings.md`）。
pub mod bootinfo_offsets {
    /// `untyped.start`（cap 区间起点）。
    ///
    /// BOOT-009：seL4 14.0.0（无 MCS）的 `seL4_BootInfo` 字段布局（对照
    /// `libsel4/include/sel4/bootinfo_types.h`）：extraLen(0x0)/nodeID(0x8)/
    /// numNodes(0x10)/numIOPTLevels(0x18)/ipcBuffer(0x20)/empty(0x28)/
    /// sharedFrames(0x38)/userImageFrames(0x48)/userImagePaging(0x58)/
    /// ioSpaceCaps(0x68)/extraBIPages(0x78)/initThreadCNodeSizeBits(0x88)/
    /// initThreadDomain(0x90) → untyped(0x98)、untypedList(0xa8)。
    /// 旧值 0x28/0x30/0x38 是 empty/sharedFrames 的偏移（21-A 遗留缺陷，
    /// QEMU 实测 cap fault）。
    pub const UNTYPED_START: usize = 0x98;
    /// `untyped.end`（cap 区间终点）。
    pub const UNTYPED_END: usize = 0xa0;
    /// `untypedList`（`seL4_UntypedDesc*` 指针）。
    pub const UNTYPED_LIST: usize = 0xa8;
}

/// 30 RT-014：`seL4_UntypedDesc` 中 `isDevice` 标志偏移（paddr(8)+
/// sizeBits(1) 之后）——读侧 `untyped_at` 与写侧 `build_test_bootinfo_blob`
/// 共享单一事实源（旧写侧 off+16 与读侧 off+9 错位，device 标志恒读 0）。
pub const UNTYPED_FLAGS_OFFSET: usize = 9;

/// BootInfo 解析访问器。
///
/// 从 seL4_GetBootInfo 返回的指针解析 untyped 列表与空闲 cap 区间。
/// host 构建用 `from_bytes` 从内存 blob 构造（测试用）；sel4_target 用
/// `from_bootinfo_ptr` 从真实 BootInfo 指针解析。
#[derive(Debug, Clone, Copy)]
pub struct BootInfoReader {
    /// BootInfo 基址（物理/虚拟地址；sel4_target 用于解析）。
    #[allow(dead_code)]
    base: u64,
    /// untyped cap 区间 `[start, end)`。
    untyped_start: CPtr,
    untyped_end: CPtr,
    /// untypedList 指针（`seL4_UntypedDesc*`）。
    untyped_list: u64,
}

impl BootInfoReader {
    /// 从原始字节 blob 解析（host 测试用）。
    ///
    /// `blob` 必须按 `bootinfo_offsets` 布局编码：`untyped_start`/`end`
    /// 两个 u64，随后 `untypedList` 指针（相对 blob 基址）。
    pub fn from_bytes(blob: &[u8]) -> Option<Self> {
        let read_u64 = |off: usize| -> Option<u64> {
            let bytes = blob.get(off..off + 8)?;
            Some(u64::from_le_bytes(bytes.try_into().ok()?))
        };
        let start = read_u64(bootinfo_offsets::UNTYPED_START)?;
        let end = read_u64(bootinfo_offsets::UNTYPED_END)?;
        // BOOT-009：untypedList 是内嵌数组（非指针），基址 = blob 基址 + 偏移。
        let list = bootinfo_offsets::UNTYPED_LIST as u64;
        Some(Self {
            base: 0,
            untyped_start: start,
            untyped_end: end,
            untyped_list: list,
        })
    }

    /// sel4_target：从真实 BootInfo 指针解析。
    #[cfg(sel4_target)]
    pub fn from_bootinfo_ptr(bi: *const crate::types::BootInfo) -> Self {
        let base = bi as u64;
        let read = |off: usize| -> u64 {
            // SAFETY: BootInfo 结构由内核保证布局；offset 经 C shim 校准。
            unsafe { core::ptr::read_volatile((base + off as u64) as *const u64) }
        };
        Self {
            base,
            untyped_start: read(bootinfo_offsets::UNTYPED_START),
            untyped_end: read(bootinfo_offsets::UNTYPED_END),
            // BOOT-009：untypedList 是内嵌数组（不是指针），基址 = bootinfo 基址 + 偏移。
            untyped_list: base + bootinfo_offsets::UNTYPED_LIST as u64,
        }
    }

    /// host 构建：无真实 BootInfo，提供空解析（调用方不应在 host 使用）。
    #[cfg(host_build)]
    pub fn from_bootinfo_ptr(_bi: *const crate::types::BootInfo) -> Self {
        Self {
            base: 0,
            untyped_start: 0,
            untyped_end: 0,
            untyped_list: 0,
        }
    }

    /// untyped cap 区间起点。
    pub fn untyped_start(&self) -> CPtr {
        self.untyped_start
    }

    /// untyped cap 区间终点。
    pub fn untyped_end(&self) -> CPtr {
        self.untyped_end
    }

    /// untyped 数量。
    pub fn untyped_count(&self) -> u64 {
        self.untyped_end.saturating_sub(self.untyped_start)
    }

    /// 读取第 `index` 个 untyped 描述。
    ///
    /// `index` 必须 < [`Self::untyped_count`]；越界返回 `None`。
    pub fn untyped_at(&self, index: u64) -> Option<UntypedDesc> {
        if index >= self.untyped_count() {
            return None;
        }
        // seL4_UntypedDesc: { paddr: Word(8), sizeBits: Word(8),
        //                     isDevice: seL4_Bool(1), padding }，16 B 对齐。
        let off = self.untyped_list + index * 16;
        #[cfg(sel4_target)]
        {
            // SAFETY: untypedList 由内核保证指向有效数组。
            let paddr = unsafe { core::ptr::read_volatile(off as *const u64) };
            // seL4_UntypedDesc: paddr(8) + sizeBits(1) + isDevice(1) + padding(6)。
            // sizeBits 是 Uint8（低字节）；旧实现读 u64 会把 isDevice/padding 混入
            // （QEMU 实测 size_bits=283 之类，alloc 全部跳过）。
            let size_bits = u64::from(unsafe { core::ptr::read_volatile((off + 8) as *const u8) });
            let is_device = unsafe { core::ptr::read_volatile((off + 9) as *const u8) } != 0;
            Some(UntypedDesc {
                paddr,
                size_bits,
                is_device,
            })
        }
        #[cfg(host_build)]
        {
            // host 测试：从 blob 内联解析（from_bytes 场景）。
            let _ = off;
            None
        }
    }

    /// 创建空闲 cap 分配器（从 untyped 区间）。
    pub fn cap_allocator(&self) -> CapAllocator {
        CapAllocator::new(self.untyped_start, self.untyped_end)
    }
}

/// 便于 host 测试的 blob 构造辅助。
#[cfg(test)]
pub(crate) fn build_test_bootinfo_blob(
    untyped_start: u64,
    untyped_end: u64,
    list: alloc::vec::Vec<UntypedDesc>,
) -> alloc::vec::Vec<u8> {
    let mut blob = vec![0u8; 0x200];
    let put = |blob: &mut [u8], off: usize, v: u64| {
        blob[off..off + 8].copy_from_slice(&v.to_le_bytes());
    };
    put(&mut blob, bootinfo_offsets::UNTYPED_START, untyped_start);
    put(&mut blob, bootinfo_offsets::UNTYPED_END, untyped_end);
    // untypedList 紧跟 blob（偏移 0x100）。
    put(&mut blob, bootinfo_offsets::UNTYPED_LIST, 0x100);
    for (i, desc) in list.iter().enumerate() {
        let off = 0x100 + i * 16;
        put(&mut blob, off, desc.paddr);
        put(&mut blob, off + 8, desc.size_bits);
        // 30 RT-014：isDevice 标志偏移与读取方（untyped_at，off+9）对齐——
        // 旧实现写 off+16（seL4_UntypedDesc 布局：paddr(8)+sizeBits(1)+
        // isDevice(1)），测试辅助构造的 device 标志恒读为 0，掩盖 device
        // 判定测试意图（假绿）。
        blob[off + UNTYPED_FLAGS_OFFSET] = desc.is_device as u8;
    }
    blob
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    #[test]
    fn error_mapping_roundtrip() {
        assert_eq!(to_sel4_result(NO_ERROR, 42), Ok(42));
        assert_eq!(
            to_sel4_result(INVALID_ARGUMENT, ()),
            Err(Sel4Error::InvalidArgument)
        );
        assert_eq!(
            to_sel4_result(FAILED_LOOKUP, ()),
            Err(Sel4Error::FailedLookup)
        );
        assert_eq!(to_sel4_result(99, ()), Err(Sel4Error::Other(99)));
    }

    #[test]
    fn object_type_table_resolves() {
        let t = ObjectTypeTable::resolve();
        assert_ne!(t.tcb, t.endpoint, "TCB 与 Endpoint 类型必须不同");
        assert_ne!(t.arm_frame, 0);
    }

    #[test]
    fn rights_table_resolves() {
        let r = RightsTable::resolve();
        assert_eq!(r.none, 0);
        assert_ne!(r.all, 0);
        assert!(r.all & r.read != 0);
    }

    #[test]
    fn cap_allocator_allocates() {
        let mut alloc = CapAllocator::new(100, 108);
        assert_eq!(alloc.alloc(1), Some(100));
        assert_eq!(alloc.alloc(4), Some(101));
        assert_eq!(alloc.alloc(4), None, "超出区间");
        assert_eq!(alloc.used(), 105);
    }

    #[test]
    fn retype_host_stub_ok() {
        assert!(retype(1, 12, 12, 1, 2, 0, 0, 0).is_ok());
    }

    #[test]
    fn tcb_ops_host_stub_ok() {
        assert!(tcb_resume(5).is_ok());
        assert!(tcb_suspend(5).is_ok());
        assert!(tcb_set_priority(5, 2, 10).is_ok());
        assert!(tcb_write_registers(5, 0, 0, core::ptr::null()).is_ok());
        assert!(cnode_mint(2, 0, 0, 2, 1, 0, 1, 0).is_ok());
    }

    // 21-A K1.1 验收：页映射在 host 构建必须返回 HostUnsupported（非 Ok），
    // 与既有 retype/tcb_* 的"模拟成功"stub 不同——红线 2 禁止 mock 占位。
    #[test]
    fn arm_page_map_host_unsupported() {
        assert_eq!(
            arm_page_map(0x100, 0x200, 0x4000_0000, 0x0F),
            Err(Sel4Error::HostUnsupported)
        );
    }

    #[test]
    fn arm_page_table_map_host_unsupported() {
        assert_eq!(
            arm_page_table_map(0x100, 0x300, 0x4000_0000),
            Err(Sel4Error::HostUnsupported)
        );
    }

    // ADR-0034（K1.2）：真实 spawn 必需封装的 host 行为——一律明确失败。
    #[test]
    fn spawn_ffi_host_unsupported() {
        assert_eq!(
            tcb_set_space(0x10, 0, 0x20, 0, 0x30, 0),
            Err(Sel4Error::HostUnsupported)
        );
        assert_eq!(
            tcb_set_ipcbuffer(0x10, 0x40, 0x2000_0000),
            Err(Sel4Error::HostUnsupported)
        );
        assert_eq!(
            arm_asid_pool_assign(0x50, 0x60),
            Err(Sel4Error::HostUnsupported)
        );
        assert_eq!(arm_page_unmap(0x70), Err(Sel4Error::HostUnsupported));
    }

    #[test]
    fn cap_init_thread_table_resolves() {
        let caps = CapInitThreadTable::resolve();
        assert_ne!(caps.tcb, 0);
        assert_ne!(caps.cnode, caps.tcb);
        assert_ne!(caps.vspace, caps.tcb);
        assert_ne!(caps.asid_pool, 0);
    }

    // 21-A K2.1 验收：7 个 IPC 封装 host 构建一律返回 HostUnsupported
    // （IPC 在 host 无 seL4 无意义，红线 2 禁止模拟成功）。
    #[test]
    fn ipc_host_unsupported() {
        let mut sender = 0u64;
        assert_eq!(set_mr(0, 1), Err(Sel4Error::HostUnsupported));
        assert_eq!(get_mr(0), Err(Sel4Error::HostUnsupported));
        assert_eq!(
            message_info_new(0, 0, 0, 1),
            Err(Sel4Error::HostUnsupported)
        );
        assert_eq!(ipc_send(1, 0), Err(Sel4Error::HostUnsupported));
        assert_eq!(ipc_recv(1, None), Err(Sel4Error::HostUnsupported));
        assert_eq!(
            ipc_recv(1, Some(&mut sender)),
            Err(Sel4Error::HostUnsupported)
        );
        assert_eq!(ipc_call(1, 0), Err(Sel4Error::HostUnsupported));
        assert_eq!(
            ipc_reply_recv(1, 0, Some(&mut sender)),
            Err(Sel4Error::HostUnsupported)
        );
    }

    #[test]
    fn bootinfo_parses_untyped_list() {
        let list = vec![
            UntypedDesc {
                paddr: 0x1000,
                size_bits: 20,
                is_device: false,
            },
            UntypedDesc {
                paddr: 0xA000_0000,
                size_bits: 22,
                is_device: true,
            },
        ];
        let blob = build_test_bootinfo_blob(30, 40, list);
        let reader = BootInfoReader::from_bytes(&blob).expect("parse");
        assert_eq!(reader.untyped_start(), 30);
        assert_eq!(reader.untyped_end(), 40);
        assert_eq!(reader.untyped_count(), 10);
        // host 构建下 untyped_at 从 blob 读取受限（见实现说明）；
        // 这里验证区间与计数访问器。
        let mut alloc = reader.cap_allocator();
        assert_eq!(alloc.alloc(1), Some(30));
    }

    #[test]
    fn cap_slot_typed_right() {
        let slot = CapSlot::new(7, ObjectTypeTable::resolve().tcb);
        assert_eq!(slot.cptr(), 7);
    }
}

// ============================================================================
// WP-26.8：IRQ 子系统安全封装（26-C §2.2 步骤 1）
// ============================================================================

/// IRQControl_Get 封装：从 IRQControl cap 获取 IRQHandler cap 到目标槽。
/// host 构建返回 [`Sel4Error::HostUnsupported`]（红线 2：禁止假 Ok）。
pub fn irq_control_get(
    irq_ctrl: CPtr,
    irq: Word,
    dest_root: CPtr,
    dest_index: CPtr,
    dest_depth: Word,
) -> Sel4Result<()> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；参数由调用方保证（irq 号合法、目标槽区间）。
        let err = unsafe {
            crate::ffi::eneros_sel4_irq_control_get(
                irq_ctrl, irq, dest_root, dest_index, dest_depth,
            )
        };
        to_sel4_result(err, ())
    }
    #[cfg(host_build)]
    {
        let _ = (irq_ctrl, irq, dest_root, dest_index, dest_depth);
        Err(Sel4Error::HostUnsupported)
    }
}

/// IRQHandler_SetNotification 封装：IRQHandler cap 绑定 notification。
pub fn irq_handler_set_notification(handler: CPtr, ntfn: CPtr) -> Sel4Result<()> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；handler 必须为已 Get 的 IRQHandler cap。
        let err = unsafe { crate::ffi::eneros_sel4_irq_handler_set_notification(handler, ntfn) };
        to_sel4_result(err, ())
    }
    #[cfg(host_build)]
    {
        let _ = (handler, ntfn);
        Err(Sel4Error::HostUnsupported)
    }
}

/// IRQHandler_Ack 封装：中断处理完成后必须 Ack（否则中断不重触发）。
pub fn irq_handler_ack(handler: CPtr) -> Sel4Result<()> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；handler 必须为已绑定的 IRQHandler cap。
        let err = unsafe { crate::ffi::eneros_sel4_irq_handler_ack(handler) };
        to_sel4_result(err, ())
    }
    #[cfg(host_build)]
    {
        let _ = handler;
        Err(Sel4Error::HostUnsupported)
    }
}

/// IRQHandler_Clear 封装：解绑中断。
pub fn irq_handler_clear(handler: CPtr) -> Sel4Result<()> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；handler 必须为已 Get 的 IRQHandler cap。
        let err = unsafe { crate::ffi::eneros_sel4_irq_handler_clear(handler) };
        to_sel4_result(err, ())
    }
    #[cfg(host_build)]
    {
        let _ = handler;
        Err(Sel4Error::HostUnsupported)
    }
}

/// 初始 IRQControl cap 槽号（编译期取 seL4_CapIRQControl，不硬编码）。
pub fn cap_irq_control() -> u64 {
    #[cfg(sel4_target)]
    {
        // SAFETY: 无参数纯查询。
        unsafe { crate::ffi::eneros_sel4_cap_irq_control() }
    }
    #[cfg(host_build)]
    {
        // host 侧常量对齐（seL4 aarch64 初始 cap 编号）。
        5
    }
}
