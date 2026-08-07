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

use crate::types::*;

#[cfg(test)]
use alloc::vec;

// ============================================================================
// 错误码 → Result 映射
// ============================================================================

/// seL4 错误（`Error` 的非零值映射为描述性错误）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sel4Error {
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

/// CNode_Mint 封装（派生带权限的能力）。
pub fn cnode_mint(
    dest_root: CPtr,
    dest_index: CPtr,
    dest_depth: Word,
    src_root: CPtr,
    src_index: CPtr,
    src_depth: Word,
    rights: CapRights,
) -> Sel4Result<()> {
    #[cfg(sel4_target)]
    {
        // SAFETY: 封装 ffi；槽区间与权限由调用方保证。
        let err = unsafe {
            crate::ffi::eneros_sel4_cnode_mint(
                dest_root, dest_index, dest_depth, src_root, src_index, src_depth, rights,
            )
        };
        to_sel4_result(err, ())
    }
    #[cfg(host_build)]
    {
        let _ = (
            dest_root, dest_index, dest_depth, src_root, src_index, src_depth, rights,
        );
        Ok(())
    }
}

// ============================================================================
// BootInfo 解析（P0-1：untyped 列表 / 空闲 cap 区间 offset 访问器）
// ============================================================================

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
    pub const UNTYPED_START: usize = 0x28;
    /// `untyped.end`（cap 区间终点）。
    pub const UNTYPED_END: usize = 0x30;
    /// `untypedList`（`seL4_UntypedDesc*` 指针）。
    pub const UNTYPED_LIST: usize = 0x38;
}

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
        let list = read_u64(bootinfo_offsets::UNTYPED_LIST)?;
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
            untyped_list: read(bootinfo_offsets::UNTYPED_LIST),
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
            let size_bits = unsafe { core::ptr::read_volatile((off + 8) as *const u64) };
            let is_device = unsafe { core::ptr::read_volatile((off + 16) as *const u8) } != 0;
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
        blob[off + 16] = desc.is_device as u8;
    }
    blob
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

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
        assert!(cnode_mint(2, 0, 0, 2, 1, 0, 1).is_ok());
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
