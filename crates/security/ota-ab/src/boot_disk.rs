//! 70.7（WP-70.7.1）：**产品启动盘**介质图——GPT 布局的类型化裁决面。
//!
//! 与 [`crate::ab_partition::DataRegion`]（16 MiB **数据盘**，svc-storage
//! 域）分治：本图描述 128 MiB **启动盘**（GPT 产品盘，UEFI/boot-manager
//! 域；运行时经 70.7 受限安装命令面才可写非活动槽）。
//!
//! 布局单一事实源 = `product/image-layouts/qemu-v1.json`
//! （sector 512B、alignment 1 MiB、disk 128 MiB）；本模块常量由
//! `place_partitions` 算法（xtask image_factory）推导并锁测试——
//! JSON 漂移即红（`qemu_v1_json_lock` 宿主测试直读 JSON 对拍）。
//!
//! 更新裁决（DEC-70-04）：更新载荷 = **非活动 BOOT 槽整分区**
//! （BOOT_A/B，8 MiB 各一；ReleaseManifest + 组件），SYS_A/B 本阶段
//! 只读（内嵌束随 BOOT 槽引导，SYS 槽角色归 70.12 SKU 冻结后裁决）；
//! BOOTCTRL 双副本（2×4096B 记录 + 64B anchor 余量）由
//! boot-protocol::BootControlTransaction 事务维护。

/// 分区字节区间（半开 [start, end)；512B 扇区 × lba）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootDiskRange {
    pub start_byte: u64,
    pub len_bytes: u64,
}

impl BootDiskRange {
    pub const fn new(start_byte: u64, len_bytes: u64) -> Self {
        BootDiskRange {
            start_byte,
            len_bytes,
        }
    }
    pub const fn end_byte(&self) -> u64 {
        self.start_byte + self.len_bytes()
    }
    pub const fn len_bytes(&self) -> u64 {
        self.len_bytes
    }
}

/// 扇区→字节（512B；与 ab_partition OTA_PROD_BLOCK_SIZE 同口径）。
pub const fn lba(l: u64) -> u64 {
    l * 512
}

/// 启动盘分区角色（布局序 = place_partitions 排布序）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootPartition {
    PlatformBoot,
    BootCtrl,
    BootA,
    BootB,
    SysA,
    SysB,
    Recovery,
    Config,
    Data,
    Logs,
    Models,
}

/// 分区容量（字节；qemu-v1.json 逐项）。
pub const PARTITION_SIZE_BYTES: &[(BootPartition, u64)] = &[
    (BootPartition::PlatformBoot, 8 * 1024 * 1024),
    (BootPartition::BootCtrl, 1024 * 1024),
    (BootPartition::BootA, 8 * 1024 * 1024),
    (BootPartition::BootB, 8 * 1024 * 1024),
    (BootPartition::SysA, 12 * 1024 * 1024),
    (BootPartition::SysB, 12 * 1024 * 1024),
    (BootPartition::Recovery, 16 * 1024 * 1024),
    (BootPartition::Config, 4 * 1024 * 1024),
    (BootPartition::Data, 16 * 1024 * 1024),
    (BootPartition::Logs, 8 * 1024 * 1024),
    (BootPartition::Models, 16 * 1024 * 1024),
];

/// 启动盘容量（128 MiB）。
pub const BOOT_DISK_BYTES: u64 = 128 * 1024 * 1024;
/// GPT 对齐（1 MiB）。
pub const ALIGN_BYTES: u64 = 1024 * 1024;
/// GPT 保留区（主头+分区表 34 扇区；备份 GPT 对称）。
pub const GPT_RESERVED_TAIL_BYTES: u64 = 34 * 512;

/// 按排布序推导全部分区区间（与 place_partitions 同算法：
/// 首分区 = 对齐基址；逐项对齐推进）。
pub const fn placements() -> [(BootPartition, BootDiskRange); 11] {
    let mut out = [
        (BootPartition::PlatformBoot, BootDiskRange::new(0, 0)),
        (BootPartition::BootCtrl, BootDiskRange::new(0, 0)),
        (BootPartition::BootA, BootDiskRange::new(0, 0)),
        (BootPartition::BootB, BootDiskRange::new(0, 0)),
        (BootPartition::SysA, BootDiskRange::new(0, 0)),
        (BootPartition::SysB, BootDiskRange::new(0, 0)),
        (BootPartition::Recovery, BootDiskRange::new(0, 0)),
        (BootPartition::Config, BootDiskRange::new(0, 0)),
        (BootPartition::Data, BootDiskRange::new(0, 0)),
        (BootPartition::Logs, BootDiskRange::new(0, 0)),
        (BootPartition::Models, BootDiskRange::new(0, 0)),
    ];
    let mut next = ALIGN_BYTES; // 首分区 = 1 MiB 对齐基址（GPT 头/表之后）
    let mut i = 0;
    while i < PARTITION_SIZE_BYTES.len() {
        let (role, size) = PARTITION_SIZE_BYTES[i];
        // 对齐向上取整（1 MiB 粒度）。
        let aligned = next.div_ceil(ALIGN_BYTES) * ALIGN_BYTES;
        out[i] = (role, BootDiskRange::new(aligned, size));
        next = aligned + size;
        i += 1;
    }
    out
}

/// 按角色取 LBA 区间（(start_lba, lbas)；512B 扇区）。
pub const fn lba_range_of(role: BootPartition) -> (u64, u64) {
    let (b, l) = match role {
        BootPartition::PlatformBoot => (lba(2048), 8 * 1024 * 1024),
        BootPartition::BootCtrl => (lba(18432), 1024 * 1024),
        BootPartition::BootA => (lba(20480), 8 * 1024 * 1024),
        BootPartition::BootB => (lba(36864), 8 * 1024 * 1024),
        BootPartition::SysA => (lba(53248), 12 * 1024 * 1024),
        BootPartition::SysB => (lba(77824), 12 * 1024 * 1024),
        BootPartition::Recovery => (lba(102400), 16 * 1024 * 1024),
        BootPartition::Config => (lba(135168), 4 * 1024 * 1024),
        BootPartition::Data => (lba(143360), 16 * 1024 * 1024),
        BootPartition::Logs => (lba(176128), 8 * 1024 * 1024),
        BootPartition::Models => (lba(192512), 16 * 1024 * 1024),
    };
    (b / 512, l / 512)
}

/// 按角色取区间。
pub fn range_of(role: BootPartition) -> BootDiskRange {
    placements()
        .iter()
        .find(|(r, _)| *r == role)
        .map(|(_, range)| range)
        .copied()
        .expect("布局表全角色覆盖")
}

/// 活跃槽标识（bootctrl select 投影；运行时由 boot-manager/BootControl
/// 事务给出）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveSlot {
    A,
    B,
}

/// 更新目标槽（DEC-70-04：非活动 BOOT 槽整分区）。
pub fn inactive_boot_slot_range(active: ActiveSlot) -> BootDiskRange {
    match active {
        ActiveSlot::A => range_of(BootPartition::BootB),
        ActiveSlot::B => range_of(BootPartition::BootA),
    }
}

/// 介质图机器守卫（WP-70.7.1）：全区间两两不交 + 界内 + GPT 保留不侵犯
/// + BOOTCTRL 容量 ≥ 双副本记录。`Err(冲突描述)` = 布局不可用（阻断）。
pub fn verify_boot_disk_map() -> Result<(), &'static str> {
    let table = placements();
    // 界内 + GPT 保留。
    for (role, range) in &table {
        if range.start_byte < ALIGN_BYTES {
            return Err("below-gpt-reserved");
        }
        if range.end_byte() > BOOT_DISK_BYTES - GPT_RESERVED_TAIL_BYTES {
            return Err("beyond-disk");
        }
        let _ = role;
    }
    // 两两不交。
    for i in 0..table.len() {
        for j in (i + 1)..table.len() {
            let (_, a) = table[i];
            let (_, b) = table[j];
            if a.start_byte < b.end_byte() && b.start_byte < a.end_byte() {
                return Err("partition-overlap");
            }
        }
    }
    // BOOTCTRL ≥ 双副本记录（2×4096）+ anchor 余量。
    let bc = range_of(BootPartition::BootCtrl);
    if bc.len_bytes() < 2 * 4096 + 64 {
        return Err("bootctrl-too-small");
    }
    // 更新槽与活跃槽互斥（BOOT_A/B 等容量）。
    let (a, b) = (
        range_of(BootPartition::BootA),
        range_of(BootPartition::BootB),
    );
    if a.len_bytes() != b.len_bytes() {
        return Err("boot-slot-asymmetric");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全域图守卫（WP-70.7.1 机器检查）。
    #[test]
    fn boot_disk_map_disjoint_and_bounded() {
        verify_boot_disk_map().expect("boot disk map");
    }

    /// 坐标锁：qemu-v1.json 布局（place_partitions 算法推导）。
    #[test]
    fn qemu_v1_coordinates_locked() {
        let cases = [
            (BootPartition::PlatformBoot, lba(2048), 8 * 1024 * 1024),
            (BootPartition::BootCtrl, lba(18432), 1024 * 1024),
            (BootPartition::BootA, lba(20480), 8 * 1024 * 1024),
            (BootPartition::BootB, lba(36864), 8 * 1024 * 1024),
            (BootPartition::SysA, lba(53248), 12 * 1024 * 1024),
            (BootPartition::SysB, lba(77824), 12 * 1024 * 1024),
            (BootPartition::Recovery, lba(102400), 16 * 1024 * 1024),
            (BootPartition::Config, lba(135168), 4 * 1024 * 1024),
            (BootPartition::Data, lba(143360), 16 * 1024 * 1024),
            (BootPartition::Logs, lba(176128), 8 * 1024 * 1024),
            (BootPartition::Models, lba(192512), 16 * 1024 * 1024),
        ];
        for (role, start, len) in cases {
            let r = range_of(role);
            assert_eq!(r.start_byte, start, "{role:?} 基址漂移");
            assert_eq!(r.len_bytes(), len, "{role:?} 长度漂移");
        }
    }

    /// 更新槽互斥：活跃 A → 写 B；活跃 B → 写 A；RECOVERY/活跃 BOOT 槽
    /// 永不在更新范围。
    #[test]
    fn inactive_slot_resolution_mutually_exclusive() {
        let a = range_of(BootPartition::BootA);
        let b = range_of(BootPartition::BootB);
        let for_a_active = inactive_boot_slot_range(ActiveSlot::A);
        assert_eq!(for_a_active, b);
        let for_b_active = inactive_boot_slot_range(ActiveSlot::B);
        assert_eq!(for_b_active, a);
        assert_ne!(a, b);
        // 恢复分区与更新范围不交。
        let rec = range_of(BootPartition::Recovery);
        assert!(rec.start_byte >= b.end_byte() || b.start_byte >= rec.end_byte());
    }

    /// 数据盘/启动盘分治：两盘坐标空间独立（数据盘 16 MiB ≠ 启动盘
    /// DATA 分区 16 MiB 的同名陷阱——类型化枚举防混用）。
    #[test]
    fn data_disk_and_boot_disk_are_disjoint_namespaces() {
        // 数据盘全域 16 MiB（ab_partition::DATA_DISK_BLOCKS）。
        assert_eq!(
            crate::ab_partition::DATA_DISK_BLOCKS * 512,
            16 * 1024 * 1024
        );
        // 启动盘 DATA 分区同为 16 MiB 但属不同物理盘——本测试钉住
        // "同名不同盘"事实，防坐标混写（WP-70.7.1 裁决注记）。
        assert_eq!(range_of(BootPartition::Data).len_bytes(), 16 * 1024 * 1024);
    }
}
