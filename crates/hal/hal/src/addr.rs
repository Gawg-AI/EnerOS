//! 38（WP-38.1）：地址强类型——CpuVaddr/PhysAddr/BusAddr/DeviceAddr/Iova。
//!
//! # 不变量（蓝图 38 WP-38.1）
//! - VA、PA、IOVA/device address **永远是不同类型**，禁止 `From<u64>` 随意
//!   互转；只能经验证的 translator/mapping 显式转换；
//! - 所有转换带 checked 语义（回绕/越界 → None），不允许裸 `u64` 在层间
//!   传地址（35-01 §6.2：`DmaAllocator` 是设备地址唯一事实）；
//! - no_std：仅 core。

use core::fmt;

/// CPU 虚拟地址（root/服务 VSpace 内）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CpuVaddr(u64);

/// 物理地址（SoC 物理地址空间）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhysAddr(u64);

/// 总线地址（FDT `ranges` 翻译前/后的总线空间；parent bus address）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BusAddr(u64);

/// 设备可见地址（无 IOMMU 时 = 物理地址；有 IOMMU 时 = IOVA 映射结果）。
/// 设备 DMA 描述的 `addr` 字段只能写此类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceAddr(u64);

/// IOMMU I/O 虚拟地址（I/OVA；仅 IOMMU 域内有效）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Iova(u64);

macro_rules! addr_newtype {
    ($ty:ident) => {
        impl $ty {
            /// 构造（调用方保证语义正确；值本身不含空间信息）。
            pub const fn new(v: u64) -> Self {
                Self(v)
            }
            /// 读取原始值（仅跨 FFI/日志边界；层间传递禁止裸值）。
            pub const fn as_u64(self) -> u64 {
                self.0
            }
            /// 页对齐检查（`size` 页粒度，供映射/分配器使用）。
            pub fn is_aligned_to(self, align: u64) -> bool {
                align != 0 && self.0 % align == 0
            }
            /// checked 加法（回绕 → None）。
            pub const fn checked_add(self, v: u64) -> Option<Self> {
                match self.0.checked_add(v) {
                    Some(x) => Some(Self(x)),
                    None => None,
                }
            }
            /// checked 减法（下溢 → None）。
            pub const fn checked_sub(self, v: u64) -> Option<Self> {
                match self.0.checked_sub(v) {
                    Some(x) => Some(Self(x)),
                    None => None,
                }
            }
        }
        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}(0x{:x})", stringify!($ty), self.0)
            }
        }
    };
}

addr_newtype!(CpuVaddr);
addr_newtype!(PhysAddr);
addr_newtype!(BusAddr);
addr_newtype!(DeviceAddr);
addr_newtype!(Iova);

impl CpuVaddr {
    /// 平台已验证的 1:1 映射窗口（BOOT-007 等）内物理 → 虚拟。
    ///
    /// 这是**唯一**允许 `PhysAddr → CpuVaddr` 的转换：必须由平台显式声明
    /// 该窗口（`PlatformInventory::identity_window` 验证过），否则 None。
    pub const fn from_identity_window(
        pa: PhysAddr,
        win_base: PhysAddr,
        win_size: u64,
    ) -> Option<Self> {
        let off = match pa.0.checked_sub(win_base.0) {
            Some(o) => o,
            None => return None,
        };
        if off >= win_size {
            return None;
        }
        Some(Self(pa.0)) // 1:1 窗口：虚拟 == 物理
    }
}

impl PhysAddr {
    /// 与另一物理地址范围是否重叠（内存/reserved 校验用）。
    pub const fn range_overlaps(self, size: u64, other: PhysAddr, other_size: u64) -> bool {
        let self_end = match self.0.checked_add(size) {
            Some(e) => e,
            None => return true, // 自身回绕视为与一切重叠（fail-closed）
        };
        let other_end = match other.0.checked_add(other_size) {
            Some(e) => e,
            None => return true,
        };
        self.0 < other_end && other.0 < self_end
    }
}

impl DeviceAddr {
    /// 无 IOMMU 时由已验证映射声明的 device address（= 物理地址语义）。
    pub const fn from_phys(pa: PhysAddr) -> Self {
        Self(pa.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_distinct_types() {
        // 类型系统：不同地址类型不能互赋（编译期保证）。
        let pa = PhysAddr::new(0x1000);
        let va = CpuVaddr::new(0x1000);
        let ba = BusAddr::new(0x1000);
        let da = DeviceAddr::new(0x1000);
        let iova = Iova::new(0x1000);
        assert_eq!(pa.as_u64(), 0x1000);
        assert_eq!(va.as_u64(), 0x1000);
        assert_eq!(ba.as_u64(), 0x1000);
        assert_eq!(da.as_u64(), 0x1000);
        assert_eq!(iova.as_u64(), 0x1000);
    }

    #[test]
    fn identity_window_translation() {
        // 样例窗口参数（2 MiB 仅为本测试的 from_identity_window 样例输入，
        // 非 BOOT-007 实窗主张——映射窗已 8 MiB，见 boot-protocol 常量）。
        let win = PhysAddr::new(0x4200_0000);
        let inside = PhysAddr::new(0x4200_1000);
        let va = CpuVaddr::from_identity_window(inside, win, 0x20_0000);
        assert_eq!(va, Some(CpuVaddr::new(0x4200_1000)), "1:1 窗口内物理==虚拟");
        // 窗外 → None（fail-closed，不猜映射）。
        let outside = PhysAddr::new(0x4000_0000);
        assert_eq!(
            CpuVaddr::from_identity_window(outside, win, 0x20_0000),
            None
        );
        // 窗口边界（含起点不含终点）。
        assert!(CpuVaddr::from_identity_window(win, win, 0x20_0000).is_some());
        assert_eq!(
            CpuVaddr::from_identity_window(PhysAddr::new(0x4220_0000), win, 0x20_0000),
            None
        );
    }

    #[test]
    fn checked_ops_and_overlap() {
        let a = PhysAddr::new(0x1000);
        assert_eq!(a.checked_add(0x100), Some(PhysAddr::new(0x1100)));
        assert_eq!(a.checked_add(u64::MAX), None, "回绕必须拒绝");
        assert_eq!(a.checked_sub(0x2000), None, "下溢必须拒绝");
        // 范围重叠（含端点语义：end 开区间）。
        assert!(PhysAddr::new(0x1000).range_overlaps(0x100, PhysAddr::new(0x10f0), 0x100));
        assert!(!PhysAddr::new(0x1000).range_overlaps(0x100, PhysAddr::new(0x1100), 0x100));
        assert!(PhysAddr::new(0x1000).range_overlaps(0x100, PhysAddr::new(0x1000), 0x10));
    }

    #[test]
    fn alignment_check() {
        assert!(PhysAddr::new(0x1000).is_aligned_to(0x1000));
        assert!(!PhysAddr::new(0x1001).is_aligned_to(0x1000));
        assert!(!PhysAddr::new(0x1000).is_aligned_to(0), "0 对齐无效");
    }
}
