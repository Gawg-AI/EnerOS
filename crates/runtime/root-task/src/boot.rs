//! BootInfo 消费（KRN-002）：root-task 读取并校验 boot manager 传递的 BootInfo。
//!
//! 契约链闭合：boot-manager-uefi 构造 `BootInfo`（BOOT_INFO_MAGIC + ABI 版本 +
//! struct_size 等）→ seL4 elfloader 装载 rootserver → root-task 在 `_start`
//! 中 `consume_boot_info` 解码 + `validate()`，确认启动域契约真实消费
//! （08 §2「启动链断头路」：验签链尾接物不再是真空——boot_id、selected_slot、
//! init_bundle_phys/len 由此进入运行时）。

extern crate alloc;

#[cfg(test)]
use eneros_boot_protocol::boot_info::BootReason;
use eneros_boot_protocol::boot_info::{BootInfo, BootInfoError};

/// 从原始字节解码并校验 BootInfo（KRN-002 消费入口）。
///
/// - 长度不足 / 解码失败 → `BootInfoError::Length` 等（decode 内部）；
/// - 字段一致性（magic/版本/struct_size/flags/范围）→ `validate()`。
pub fn consume_boot_info(raw: &[u8]) -> Result<BootInfo, BootInfoError> {
    let bi = BootInfo::decode(raw)?;
    bi.validate()?;
    Ok(bi)
}

// W4.3（蓝图23）：删除了 `PhysPageMap`/`map_physical_pages`——零生产调用方，
// 其页对齐校验 + W^X 拒绝逻辑已由 `spawn.rs` 的 `PageEntry::is_wx_violation` +
// `plan_one` 等价覆盖（deletion test 通过，运行时零变化）。

#[cfg(test)]
mod tests {
    use eneros_boot_protocol::boot_info::{BootInfo, BootSlot};

    use super::*;

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
