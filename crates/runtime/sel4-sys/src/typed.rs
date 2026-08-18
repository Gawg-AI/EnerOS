//! WP-37.5：类型化 capability 与指针边界（蓝图 37 §5）。
//!
//! 目标不是重写所有 FFI，而是让 unsafe 局部化：capability 句柄带上
//! 对象类型标签（`CapType`），虚拟/物理地址与帧大小成为新类型，映射
//! 区域由显式 owner 管理。raw pointer/`types::CPtr`（裸 usize 历史
//! 别名）的转换只在审计过的 adapter 进行。
//!
//! 迁移说明：蓝图 `CPtr<T>` 因 `types::CPtr`（裸别名）被历史 caller
//! 大量占用，本模块以 `sel4_sys::typed::CPtr<T>` 提供蓝图类型；历史
//! 别名随 caller 逐个迁移后淘汰（WP-37.5 迁移台账，禁止业务 crate
//! 直接 `ffi::*`）。

use core::marker::PhantomData;
use core::num::NonZeroUsize;

/// WP-37.5：capability 对象类型标签（sealed——外部 crate 不能伪造新类型，
/// 保证 `CPtr<T>` 的 T 只能来自本模块声明的能力种类）。
pub trait CapType: sealed::Sealed + Copy + 'static {}

mod sealed {
    pub trait Sealed {}
}

macro_rules! declare_cap_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {}
        impl sealed::Sealed for $name {}
        impl CapType for $name {}
    };
}

declare_cap_type!(
    /// TCB 能力。
    TcbCap
);
declare_cap_type!(
    /// VSpace 能力。
    VSpaceCap
);
declare_cap_type!(
    /// CNode 能力。
    CNodeCap
);
declare_cap_type!(
    /// 页表能力。
    PageTableCap
);
declare_cap_type!(
    /// 帧（SmallPage）能力。
    FrameCap
);
declare_cap_type!(
    /// endpoint 能力。
    EndpointCap
);
declare_cap_type!(
    /// notification 能力。
    NotificationCap
);
declare_cap_type!(
    /// ASID pool 能力。
    AsidPoolCap
);

/// WP-37.5：类型化 capability 句柄（蓝图 `CPtr<T>`）——非零槽号 +
/// 对象类型标签。`T` 在编译期约束句柄种类（TCB 句柄不能当 VSpace 用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CPtr<T: CapType> {
    slot: NonZeroUsize,
    _marker: PhantomData<T>,
}

impl<T: CapType> CPtr<T> {
    /// 从槽号构造；**0 拒绝**（seL4 槽 0 语义特殊，禁止无标签零句柄
    /// 混用）。
    pub fn new(slot: usize) -> Option<Self> {
        Some(Self {
            slot: NonZeroUsize::new(slot)?,
            _marker: PhantomData,
        })
    }

    /// 槽号（只读；供审计过的 unsafe adapter 转原始 FFI 参数）。
    pub fn slot(&self) -> usize {
        self.slot.get()
    }

    /// 降级为原始槽号（**仅限 adapter 内**使用；业务代码不得依赖）。
    pub const fn into_raw(self) -> usize {
        self.slot.get()
    }
}

/// WP-37.5：用户虚拟地址（`UserVaddr`）——与裸 usize 区分，算术显式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct UserVaddr(pub usize);

impl UserVaddr {
    /// 页对齐检查（映射/访问前置条件）。
    pub fn is_page_aligned(&self, page_size: usize) -> bool {
        page_size.is_power_of_two() && self.0 & (page_size - 1) == 0
    }

    /// checked 加法（越界拒绝——防恶意地址算术回绕）。
    pub fn checked_add(&self, delta: usize) -> Option<Self> {
        self.0.checked_add(delta).map(Self)
    }

    /// 区间检查：`[self, self+len)` 不溢出。
    pub fn range_ok(&self, len: usize) -> bool {
        self.0.checked_add(len).is_some()
    }
}

/// WP-37.5：物理地址（`PhysAddr`）——device 映射/DMA 语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysAddr(pub u64);

/// WP-37.5：帧大小位（`FrameSizeBits`）——seL4 frame size bits 合法域
/// [12, 25]（4 KiB … 32 MiB；SmallPage=12）。越界构造拒绝（fail-closed）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSizeBits(u8);

impl FrameSizeBits {
    /// 合法帧大小位范围（seL4 ARM：SmallPage=12 … LargePage=25）。
    pub const MIN_BITS: u8 = 12;
    pub const MAX_BITS: u8 = 25;

    /// 构造；越界 → None（不静默截断）。
    pub fn new(bits: u8) -> Option<Self> {
        (Self::MIN_BITS..=Self::MAX_BITS)
            .contains(&bits)
            .then_some(Self(bits))
    }

    /// 帧字节大小（`1 << bits`，checked 保证 ≤ 32 MiB）。
    pub fn size_bytes(&self) -> usize {
        1usize << self.0
    }

    /// 原始位值。
    pub fn bits(&self) -> u8 {
        self.0
    }
}

/// WP-37.5：已映射区域（`MappedRegion`）——映射所有权的事实记录：
/// vaddr/大小/权限/帧大小。unmap 由显式调用（seL4 cap 语义复杂，
/// Drop 自动 unmap 登记为 target 接线项；本类型保证"映射状态可查询、
/// 权限不可静默漂移"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MappedRegion {
    vaddr: UserVaddr,
    size: usize,
    frame_bits: FrameSizeBits,
    /// 页权限位（与 root-task `VmPermissions` 的 read/write/execute 语义
    /// 一致；避免跨 crate 依赖，用独立位集）。
    read: bool,
    write: bool,
    execute: bool,
}

impl MappedRegion {
    /// 构造；vaddr 必须页对齐、size 必须帧大小整数倍（防半页映射漂移）。
    pub fn new(
        vaddr: UserVaddr,
        size: usize,
        frame_bits: FrameSizeBits,
        read: bool,
        write: bool,
        execute: bool,
    ) -> Option<Self> {
        let frame = frame_bits.size_bytes();
        if !vaddr.is_page_aligned(frame) || size == 0 || size % frame != 0 {
            return None;
        }
        if !vaddr.range_ok(size) {
            return None;
        }
        Some(Self {
            vaddr,
            size,
            frame_bits,
            read,
            write,
            execute,
        })
    }

    /// 区域起始虚拟地址。
    pub fn vaddr(&self) -> UserVaddr {
        self.vaddr
    }

    /// 区域大小。
    pub fn size(&self) -> usize {
        self.size
    }

    /// 覆盖检查：`vaddr ∈ [self.vaddr, self.vaddr+size)`。
    pub fn contains(&self, vaddr: UserVaddr) -> bool {
        vaddr.0 >= self.vaddr.0 && vaddr.0 < self.vaddr.0.saturating_add(self.size)
    }

    /// 权限（W^X 违规拒绝在构造期已由调用方保证；此处只读查询）。
    pub fn permissions(&self) -> (bool, bool, bool) {
        (self.read, self.write, self.execute)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_cptr_rejects_zero_and_preserves_slot() {
        assert!(CPtr::<TcbCap>::new(0).is_none(), "槽 0 必须拒绝");
        let c = CPtr::<TcbCap>::new(5).expect("non-zero");
        assert_eq!(c.slot(), 5);
        assert_eq!(c.into_raw(), 5);
        // 类型标签：TCB 句柄不能赋给 VSpace 变量（编译期约束）。
        let _v: CPtr<VSpaceCap> = CPtr::new(7).expect("vspace");
        assert_eq!(_v.slot(), 7);
    }

    #[test]
    fn user_vaddr_alignment_and_overflow() {
        let v = UserVaddr(0x4000);
        assert!(v.is_page_aligned(0x1000));
        assert!(!UserVaddr(0x4001).is_page_aligned(0x1000));
        assert_eq!(v.checked_add(0x10), Some(UserVaddr(0x4010)));
        assert!(UserVaddr(usize::MAX).checked_add(1).is_none(), "溢出拒绝");
        assert!(!UserVaddr(usize::MAX - 3).range_ok(8), "区间溢出拒绝");
    }

    #[test]
    fn frame_size_bits_bounds() {
        assert!(FrameSizeBits::new(11).is_none(), "低于 12 拒绝");
        assert!(FrameSizeBits::new(26).is_none(), "高于 25 拒绝");
        let f = FrameSizeBits::new(12).expect("4KiB");
        assert_eq!(f.size_bytes(), 0x1000);
        assert_eq!(f.bits(), 12);
        assert_eq!(
            FrameSizeBits::new(21).unwrap().size_bytes(),
            2 * 1024 * 1024
        );
    }

    #[test]
    fn mapped_region_validation() {
        let f = FrameSizeBits::new(12).unwrap();
        // 非页对齐 vaddr 拒绝。
        assert!(MappedRegion::new(UserVaddr(0x1001), 0x1000, f, true, false, false).is_none());
        // 非帧整数倍 size 拒绝。
        assert!(MappedRegion::new(UserVaddr(0x1000), 0x1500, f, true, false, false).is_none());
        // 溢出区间拒绝。
        assert!(
            MappedRegion::new(UserVaddr(usize::MAX - 0x800), 0x1000, f, true, false, false)
                .is_none()
        );
        // 合法区域：contains/权限查询。
        let m = MappedRegion::new(UserVaddr(0x1000), 0x2000, f, true, true, false).expect("region");
        assert_eq!(m.size(), 0x2000);
        assert!(m.contains(UserVaddr(0x1000)));
        assert!(m.contains(UserVaddr(0x2FFF)));
        assert!(!m.contains(UserVaddr(0x3000)));
        assert_eq!(m.permissions(), (true, true, false));
    }
}
