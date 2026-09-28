//! BootInfo 消费（KRN-002）：root-task 读取并校验 boot manager 传递的 BootInfo。
//!
//! 契约链（蓝图 77 卷二 WP-77.2.2 起为接线事实）：boot-manager-uefi 构造
//! 并 seal `BootInfo`（ABI 1.1，含 `security_floor`）→ elfloader（0003 补丁）
//! 验证后经版本化 mailbox 信封发布 → 内核（0004 补丁）快照并作为具名
//! extra bootinfo chunk 交给初始线程 → `service::identity_freeze` 从 extra
//! 区取出并 `consume_boot_info` 解码 + validate，与 capsule 页六字段比较
//! 后冻结 Validated/Absent/Rejected（REG-70.5-CAPSULE-STALENESS 根治面）。
//! 本模块的 `consume_boot_info` 是该链的解码入口（host 测试 + 目标冻结点
//! 共用）；bundle 物理地址解析（`resolve_bundle_phys`）仍走 BOOT-007 契约
//! 窗口——见证当前只承载身份面，不改变 bundle 取数路径。

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

/// WP-37.6：bundle 物理地址解析错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundlePhysError {
    /// BootInfo 未提供 bundle 地址且无可用契约窗口（当前不可达——
    /// BOOT-007 契约窗口恒存在；保留变体供未来策略收紧时使用）。
    NoBootInfoFallback,
}

/// WP-37.6：bundle 物理地址解析——BootInfo 值优先（`phys != 0`）；
/// 零值（未提供）时使用内核 **BOOT-007 契约窗口** 常量
/// [`eneros_boot_protocol::boot_info::ENEROS_INIT_BUNDLE_PHYS`]。
///
/// 该常量不是"猜测"：当前正式链中它是三方契约——seL4 内核补丁
/// `0001-init-bundle-mapping.patch` 把 `[ENEROS_INIT_BUNDLE_PHYS, +2MiB)`
/// 映射进初始线程 VSpace；boot manager 把 init bundle 装载到同一地址；
/// DTB 位置（`dtb_window_for`）与 EnerOS BootInfo 的 `init_bundle_phys`
/// 都从它推导。消费仍必须经过验证：magic 探测 + 模块 sha256
/// （`InitBundle::parse_verified`），验证失败 fail-closed 挂起——
/// 蓝图"禁止固定地址 fallback 进入 release"针对的是无验证盲猜，
/// 本路径的固定地址经内核契约 + 完整摘要验证。
///
/// elfloader 把 EnerOS BootInfo 指针传入 root-task 后（RECON-37 §5 登记
/// CROSS_VERSION_CHANGE_REQUEST），本函数的 `phys != 0` 分支接管，
/// 契约窗口退为兜底。
pub fn resolve_bundle_phys(boot_info_bundle_phys: u64) -> Result<u64, BundlePhysError> {
    if boot_info_bundle_phys != 0 {
        return Ok(boot_info_bundle_phys);
    }
    Ok(eneros_boot_protocol::boot_info::ENEROS_INIT_BUNDLE_PHYS)
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

    /// WP-37.6：bundle 物理地址解析——BootInfo 值优先；零值回落 BOOT-007
    /// 内核契约窗口常量（正式链 ABI；消费仍须 magic + 摘要验证）。
    #[test]
    fn test_resolve_bundle_phys_priority_and_contract_window() {
        // BootInfo 值优先（elfloader 通道建立后的真实语义）。
        assert_eq!(resolve_bundle_phys(0x4201_0000).unwrap(), 0x4201_0000);
        // 零值 → BOOT-007 契约窗口常量（内核 boot.c 映射 + boot manager
        // 装载地址；不是无验证猜测）。
        assert_eq!(
            resolve_bundle_phys(0).unwrap(),
            eneros_boot_protocol::boot_info::ENEROS_INIT_BUNDLE_PHYS
        );
    }
}
