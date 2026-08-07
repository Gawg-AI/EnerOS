//! BootInfo 消费（KRN-002）：root-task 读取并校验 boot manager 传递的 BootInfo。
//!
//! 契约链闭合：boot-manager-uefi 构造 `BootInfo`（BOOT_INFO_MAGIC + ABI 版本 +
//! struct_size 等）→ seL4 elfloader 装载 rootserver → root-task 在 `_start`
//! 中 `consume_boot_info` 解码 + `validate()`，确认启动域契约真实消费
//! （08 §2「启动链断头路」：验签链尾接物不再是真空——boot_id、selected_slot、
//! init_bundle_phys/len 由此进入运行时）。

extern crate alloc;

use eneros_boot_protocol::boot_info::{BootInfo, BootInfoError};
#[cfg(test)]
use eneros_boot_protocol::boot_info::BootReason;

/// 从原始字节解码并校验 BootInfo（KRN-002 消费入口）。
///
/// - 长度不足 / 解码失败 → `BootInfoError::Length` 等（decode 内部）；
/// - 字段一致性（magic/版本/struct_size/flags/范围）→ `validate()`。
pub fn consume_boot_info(raw: &[u8]) -> Result<BootInfo, BootInfoError> {
    let bi = BootInfo::decode(raw)?;
    bi.validate()?;
    Ok(bi)
}

// ============================================================================
// Root Task 镜像物理页映射页表（蓝图/20 R1-2）
// ============================================================================

/// 物理页映射条目（Root Task 镜像装载：paddr → vaddr 页表计划）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysPageMap {
    /// 物理地址（页对齐）。
    pub paddr: u64,
    /// 映射到的虚拟地址。
    pub vaddr: u64,
    /// 映射大小（页对齐）。
    pub size: u64,
    /// 可写。
    pub writable: bool,
    /// 可执行（W^X：与 writable 互斥）。
    pub executable: bool,
}

impl PhysPageMap {
    /// W^X 校验：可执行页不得可写。
    pub fn is_wx_violation(&self) -> bool {
        self.executable && self.writable
    }
}

/// 生成物理页映射计划：把 `paddr` 起始的连续 `len` 字节映射到
/// `vaddr`，按 `page_size` 切页（蓝图/20 R1-2：boot.rs 扩展）。
///
/// - 非页对齐 / 零长 → `Err`（fail-closed）；
/// - `executable` 与 `writable` 同时为真 → `Err`（W^X 硬性拒绝）。
pub fn map_physical_pages(
    paddr: u64,
    vaddr: u64,
    len: u64,
    page_size: u64,
    writable: bool,
    executable: bool,
) -> Result<alloc::vec::Vec<PhysPageMap>, &'static str> {
    if page_size == 0 || paddr % page_size != 0 || vaddr % page_size != 0 {
        return Err("physical page mapping: address not page-aligned");
    }
    if len == 0 {
        return Err("physical page mapping: zero length");
    }
    if writable && executable {
        return Err("physical page mapping: W^X violation");
    }
    let pages = (len + page_size - 1) / page_size;
    let mut out = alloc::vec::Vec::with_capacity(pages as usize);
    for i in 0..pages {
        out.push(PhysPageMap {
            paddr: paddr + i * page_size,
            vaddr: vaddr + i * page_size,
            size: page_size,
            writable,
            executable,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_boot_protocol::boot_info::{BootInfo, BootSlot};

    #[test]
    fn test_map_physical_pages_basic() {
        let maps = map_physical_pages(0x4000_0000, 0x4000_0000, 0x3000, 0x1000, false, true)
            .expect("mapping");
        assert_eq!(maps.len(), 3);
        assert_eq!(maps[0].paddr, 0x4000_0000);
        assert_eq!(maps[2].vaddr, 0x4000_2000);
        assert!(maps[0].executable && !maps[0].writable);
        assert!(maps.iter().all(|m| !m.is_wx_violation()));
    }

    #[test]
    fn test_map_physical_pages_wx_rejected() {
        assert!(map_physical_pages(0x4000_0000, 0x4000_0000, 0x1000, 0x1000, true, true).is_err());
    }

    #[test]
    fn test_map_physical_pages_unaligned_rejected() {
        assert!(map_physical_pages(0x4000_0001, 0x4000_0000, 0x1000, 0x1000, false, true).is_err());
        assert!(map_physical_pages(0x4000_0000, 0x4000_0000, 0, 0x1000, false, true).is_err());
    }

    fn valid_boot_info() -> BootInfo {
        let mut bi = BootInfo::new(BootSlot::A, BootReason::Cold);
        bi.kernel_entry = 0x4000_0000; // boot manager 填充的真实内核入口（4KB 对齐）
        bi.seal(); // 填充 crc32（validate 要求非零且匹配）
        bi
    }

    #[test]
    fn test_krn002_consume_valid_boot_info() {
        let bi = valid_boot_info();
        let raw = bi.to_le_bytes();
        let parsed = consume_boot_info(&raw).expect("valid BootInfo must decode");
        assert_eq!(parsed.boot_id, bi.boot_id);
        assert_eq!(parsed.selected_slot, BootSlot::A);
        assert_eq!(parsed.init_bundle_phys, bi.init_bundle_phys);
    }

    #[test]
    fn test_krn002_bad_magic_rejected() {
        let bi = valid_boot_info();
        let mut raw = bi.to_le_bytes();
        raw[0] = b'X';
        let err = consume_boot_info(&raw).unwrap_err();
        assert_eq!(err, BootInfoError::Magic);
    }

    #[test]
    fn test_krn002_truncated_rejected() {
        let bi = valid_boot_info();
        let raw = bi.to_le_bytes();
        let err = consume_boot_info(&raw[..40]).unwrap_err();
        assert_eq!(err, BootInfoError::Length);
    }

    #[test]
    fn test_krn002_flags_unknown_rejected() {
        let bi = valid_boot_info();
        let mut raw = bi.to_le_bytes();
        // 置一个未知 flag 位（bits 5+ 未定义）
        let flags = u64::from_le_bytes(raw[16..24].try_into().unwrap()) | (1 << 5);
        raw[16..24].copy_from_slice(&flags.to_le_bytes());
        assert_eq!(consume_boot_info(&raw).unwrap_err(), BootInfoError::Flags);
    }
}
