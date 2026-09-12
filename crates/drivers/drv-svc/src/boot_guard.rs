//! 70.7（WP-70.7.2/70.7.3）：启动盘受限安装守卫。
//!
//! 职责：从启动盘 BOOTCTRL 分区的双副本记录解析**活跃槽**，导出
//! **可写范围** = 非活动 BOOT 槽 ∪ BOOTCTRL 分区（事务自更新）。
//! 读全域放行（BLK_BOOT_READ）；写越界 = ST_INVALID（fail-closed）。
//!
//! 单源：槽区间 = eneros-ota-ab::boot_disk（启动盘介质图，WP-70.7.1）；
//! 记录解析 = eneros-boot-protocol::BootControl（CRC/域检/canonical）。
//! 运行时无 protected anchor（UEFI vars 域）——双副本取 **序列号更高
//! 的有效副本** 投影活跃槽（标准双副本规则；anchor 一致性由
//! boot-manager 域维护，不属本守卫合同）。

use eneros_boot_protocol::boot_control::{BootControl, BOOT_CONTROL_RECORD_SIZE};
use eneros_ota_ab::boot_disk::{self, BootPartition};

/// BOOTCTRL 分区 LBA 基址（qemu-v1 布局；boot_disk 单源）。
pub const BOOTCTRL_START_LBA: u64 = boot_disk_lba_range().0;
/// BOOTCTRL 分区 LBA 数。
pub const BOOTCTRL_LBAS: u64 = boot_disk_lba_range().1;

const fn boot_disk_lba_range() -> (u64, u64) {
    eneros_ota_ab::boot_disk::lba_range_of(BootPartition::BootCtrl)
}

/// 可写守卫（安装面状态；init 时解析一次——bootctrl 变更经 0x07 FLUSH
/// 后由下次 init/查询重读）。
#[derive(Debug, Clone, Copy)]
pub struct BootWriteGuard {
    /// 非活动 BOOT 槽（可写安装目标）起始 LBA。
    pub inactive_start_lba: u64,
    /// 非活动槽 LBA 数（8 MiB / 512B = 16384）。
    pub inactive_lbas: u64,
    /// 活跃槽（禁写）。
    pub active_is_a: bool,
    /// 活跃槽 security_version（对账观测）。
    pub active_security_version: u64,
    /// 活跃槽 bootctrl 序列号。
    pub sequence: u64,
}

impl BootWriteGuard {
    /// 从双副本记录解析守卫（`Ok` = 解析成功；`Err` = 双副本均无效/
    /// 校验失败 → 写面整体禁用，读面不受限）。
    pub fn resolve(
        first: &[u8; BOOT_CONTROL_RECORD_SIZE],
        second: &[u8; BOOT_CONTROL_RECORD_SIZE],
    ) -> Result<Self, &'static str> {
        let a = BootControl::decode(first).ok();
        let b = BootControl::decode(second).ok();
        let control = match (a, b) {
            (Some(x), Some(y)) => {
                if x.sequence >= y.sequence {
                    x
                } else {
                    y
                }
            }
            (Some(x), None) => x,
            (None, Some(y)) => y,
            (None, None) => return Err("bootctrl-both-invalid"),
        };
        let selection = control.select().map_err(|_| "no-bootable-slot")?;
        let active_is_a = selection.slot == eneros_boot_protocol::BootSlot::A;
        let (inactive_start_lba, inactive_lbas) = if active_is_a {
            boot_disk::lba_range_of(BootPartition::BootB)
        } else {
            boot_disk::lba_range_of(BootPartition::BootA)
        };
        Ok(BootWriteGuard {
            inactive_start_lba,
            inactive_lbas,
            active_is_a,
            active_security_version: selection.security_version,
            sequence: control.sequence,
        })
    }

    /// 安装写越界判定：`[lba, lba+count)` 必须**完整落**在非活动槽或
    /// BOOTCTRL 分区之一（跨域/出界/活跃槽 = 拒绝）。
    pub fn install_write_allowed(&self, lba: u64, count: u64) -> bool {
        if count == 0 {
            return false;
        }
        let end = lba.saturating_add(count);
        let in_inactive =
            lba >= self.inactive_start_lba && end <= self.inactive_start_lba + self.inactive_lbas;
        let in_bootctrl = lba >= BOOTCTRL_START_LBA && end <= BOOTCTRL_START_LBA + BOOTCTRL_LBAS;
        in_inactive || in_bootctrl
    }
}

/// BOOTCTRL 双副本读取辅助：分区基址 + 副本偏移（0 / 4096）。
pub const BOOTCTRL_COPY_OFFSETS: [u64; 2] = [0, BOOT_CONTROL_RECORD_SIZE as u64];

#[cfg(test)]
mod tests {
    use super::*;

    /// BootControl 测试记录构造（boot-protocol 公厂 + 手改序列）。
    fn make_record(
        sequence: u64,
        a_priority: u8,
        b_priority: u8,
    ) -> [u8; BOOT_CONTROL_RECORD_SIZE] {
        let image_id = [7u8; 16];
        let mut ctl = BootControl::factory(image_id, 1, image_id);
        ctl.sequence = sequence;
        ctl.slot_a.priority = a_priority;
        ctl.slot_b.priority = b_priority;
        ctl.encode().expect("encode")
    }

    #[test]
    fn guard_resolves_higher_sequence_and_ranges() {
        let lo = make_record(1, 15, 14);
        let hi = make_record(2, 15, 14);
        // 双副本取高序列。
        let g = BootWriteGuard::resolve(&hi, &lo).expect("resolve");
        assert!(g.sequence >= 2);
        assert!(g.active_is_a, "A 优先级高 → 活跃 A");
        assert_eq!(
            g.inactive_start_lba,
            eneros_ota_ab::boot_disk::lba_range_of(BootPartition::BootB).0
        );
        assert_eq!(g.inactive_lbas, 16384);
        // 写非活动槽 = 允许；写活跃槽 = 拒绝；写 BOOTCTRL = 允许。
        assert!(g.install_write_allowed(g.inactive_start_lba, 8));
        assert!(!g.install_write_allowed(2048, 8), "活跃槽 A 基址");
        assert!(g.install_write_allowed(BOOTCTRL_START_LBA + 1, 4));
        assert!(!g.install_write_allowed(BOOTCTRL_START_LBA, 0));
        // 跨界拒绝。
        assert!(!g.install_write_allowed(g.inactive_start_lba + g.inactive_lbas - 4, 8));
    }

    #[test]
    fn both_invalid_rejected() {
        let zero = [0u8; BOOT_CONTROL_RECORD_SIZE];
        assert!(BootWriteGuard::resolve(&zero, &zero).is_err());
    }

    #[test]
    fn bootctrl_partition_bounds_locked() {
        assert_eq!(BOOTCTRL_START_LBA, 18432);
        assert_eq!(BOOTCTRL_LBAS, 2048);
    }
}
