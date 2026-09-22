//! 70.7（WP-70.7.2/70.7.3）：启动盘受限安装守卫。
//!
//! 职责：从启动盘 BOOTCTRL 分区的双副本记录解析记录态，结合**本次实际
//! 运行槽身份**（F70-02/S1：boot-manager → 身份 capsule → root 只读身份
//! 页）导出**可写范围** = 非运行 BOOT 槽 ∪ BOOTCTRL 分区（事务自更新）。
//! 读全域放行（BLK_BOOT_READ）；写越界 = ST_INVALID（fail-closed）。
//!
//! 单源：槽区间 = eneros-ota-ab::boot_disk（启动盘介质图，WP-70.7.1）；
//! 记录解析 = eneros-boot-protocol::BootControl（CRC/域检/canonical）。
//!
//! **运行身份 ≠ `select()`**（F70-02 复查修复）：`BootControl::select()`
//! 表示"依据当前记录的**下一次**启动选择"。boot-manager 在真正引导前经
//! `prepare_attempt` 持久扣减试启次数：A 的最后一次试启 1→0 后，本次仍
//! 引导 A，盘上记录的 `select()` 却已指向回退槽 B。旧守卫把下一引导选择
//! 当运行身份 → 运行槽 A 被开放为安装目标、A 的健康确认被
//! `confirm-slot-not-selected` 拒绝。现守卫的活跃槽/写范围/确认基线全部
//! 以**本次运行身份**（可信启动交接数据，root 只读身份页注入）为准，
//! `select()` 不再参与运行期判定。

use eneros_boot_protocol::boot_control::{BootControl, BOOT_CONTROL_RECORD_SIZE};
use eneros_ota_ab::boot_disk::{self, BootPartition};

/// BOOTCTRL 分区 LBA 基址（qemu-v1 布局；boot_disk 单源）。
pub const BOOTCTRL_START_LBA: u64 = boot_disk_lba_range().0;
/// BOOTCTRL 分区 LBA 数。
pub const BOOTCTRL_LBAS: u64 = boot_disk_lba_range().1;

const fn boot_disk_lba_range() -> (u64, u64) {
    eneros_ota_ab::boot_disk::lba_range_of(BootPartition::BootCtrl)
}

/// 本次运行的槽身份（F70-02 §5.2：可信启动交接数据）。
///
/// 来源链：boot-manager 验签后写 `IdentityCapsule`（版本+CRC）→ root
/// 拷入只读身份页映射给 svc-storage（`STORAGE_BOOT_IDENTITY_VADDR`）→
/// 守卫初始化时解析一次、本 boot 内冻结。字段 = 确认面核对的载荷身份
/// （§5.2.5：错槽、错身份、虚增安全水位的确认一律无介质状态推进）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunningIdentity {
    /// 本次实际运行的槽（true = A）。
    pub is_a: bool,
    /// 运行载荷的安全版本（capsule.security_version，非 floor）。
    pub security_version: u64,
    /// 运行载荷的 release_id（capsule.release_id = 已验签 manifest 投影）。
    pub release_id: [u8; 16],
}

/// 安装区间的具名承载（73 批A C2 核验收口）：boot A/B 安全面全部语义
/// 由字段名承担——替换裸四元组的位置契约后，换位/漏位不再可表示
///（位置解构 = 编译错）。
///
/// 单位：全部字段 = **512B 扇区 LBA / 扇区数**（`boot_disk` 同口径）。
///
/// wire（`BLK_BOOT_GUARD` 响应，ADR-0025 IPC wire ABI 面）：[`Self::pack_le`]
/// 产 40B = 4×u64 小端（offsets 0/8/16/24）+ 尾部 8B 保留恒零；
/// [`Self::unpack_le`] 读侧忽略保留字节。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstallRanges {
    /// 非运行 BOOT 槽（可写安装目标）起始 LBA；身份缺失时 = 0（写面
    /// 整体关闭——fail-closed，与 [`BootWriteGuard::inactive_start_lba`]
    /// 同语义）。
    pub inactive_start_lba: u64,
    /// 非运行槽 LBA 数（8 MiB / 512B = 16384）。
    pub inactive_lbas: u64,
    /// BOOTCTRL 事务分区起始 LBA（qemu-v1 = 18432；boot_disk 单源）。
    pub bootctrl_start_lba: u64,
    /// BOOTCTRL 事务分区 LBA 数（qemu-v1 = 2048）。
    pub bootctrl_lbas: u64,
}

impl InstallRanges {
    /// `BLK_BOOT_GUARD` 响应打包（40B：4×u64 小端 offsets 0/8/16/24，
    /// 尾部 8B 保留恒零）。单源——block_cmd.rs 打包与 svc-ota 解包共用
    /// 本契约（C1：与 2026-09-22 基线逐字节一致；golden 测试锁死）。
    pub fn pack_le(&self) -> [u8; 40] {
        let mut b = [0u8; 40];
        b[..8].copy_from_slice(&self.inactive_start_lba.to_le_bytes());
        b[8..16].copy_from_slice(&self.inactive_lbas.to_le_bytes());
        b[16..24].copy_from_slice(&self.bootctrl_start_lba.to_le_bytes());
        b[24..32].copy_from_slice(&self.bootctrl_lbas.to_le_bytes());
        b
    }

    /// `BLK_BOOT_GUARD` 响应解包（长度非 40 → None；尾部 8B 保留读侧
    /// 忽略——写侧恒零，两侧对称现状保持）。
    pub fn unpack_le(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 40 {
            return None;
        }
        Some(Self {
            inactive_start_lba: u64::from_le_bytes(bytes[..8].try_into().ok()?),
            inactive_lbas: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
            bootctrl_start_lba: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
            bootctrl_lbas: u64::from_le_bytes(bytes[24..32].try_into().ok()?),
        })
    }
}

/// 可写守卫（安装面状态；init 时解析一次——bootctrl 变更经 0x07 FLUSH
/// 后由下次 init/查询重读；**运行身份本 boot 内不变**，trial/BOOTCTRL
/// 改写/驱动重启都不能漂移）。
#[derive(Debug, Clone, Copy)]
pub struct BootWriteGuard {
    /// 非运行 BOOT 槽（可写安装目标）起始 LBA；**身份缺失时 = 0**（写面
    /// 整体关闭——fail-closed，不是"猜一个槽"）。
    pub inactive_start_lba: u64,
    /// 非运行槽 LBA 数（8 MiB / 512B = 16384）；身份缺失时 = 0。
    pub inactive_lbas: u64,
    /// 本次运行槽（诊断观测；身份缺失时无意义，恒 false）。
    pub active_is_a: bool,
    /// 运行槽在**盘上记录**里的 security_version（对账观测：记录与
    /// capsule 身份分叉在此可见）。
    pub active_security_version: u64,
    /// 活跃槽 bootctrl 序列号。
    pub sequence: u64,
    /// 本次运行身份（None ⇒ 安装写面与确认面全部关闭）。
    pub running: Option<RunningIdentity>,
}

impl BootWriteGuard {
    /// 从双副本记录 + **运行身份**解析守卫。
    ///
    /// - `Ok(g)`：记录至少一副本可解码；`g.running` 决定写面（None ⇒
    ///   非运行区间为空，`install_write_allowed` 恒拒）。
    /// - `Err`：双副本均无效/校验失败 → 写面整体禁用，读面不受限。
    ///
    /// 形参为**切片**——调用方可直接把双副本读入一棵缓冲的两半
    /// （REG-70.7 栈预算修复）。**不使用 `select()`**：活跃槽 = 运行
    /// 身份（§5.2.3：新 trial、BOOTCTRL 改写、下一槽优先级变化、驱动
    /// 重启均不能把当前槽改成安装目标）。
    pub fn resolve_with_running(
        first: &[u8],
        second: &[u8],
        running: Option<RunningIdentity>,
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
        let (inactive_start_lba, inactive_lbas, active_is_a, active_security_version) =
            match running {
                Some(r) => {
                    let (start, lbas) = if r.is_a {
                        boot_disk::lba_range_of(BootPartition::BootB)
                    } else {
                        boot_disk::lba_range_of(BootPartition::BootA)
                    };
                    let recorded = if r.is_a {
                        control.slot_a.security_version
                    } else {
                        control.slot_b.security_version
                    };
                    (start, lbas, r.is_a, recorded)
                }
                // 身份缺失/无效（含 Recovery 运行、旧 ABI 身份页）⇒ 写面
                // 关闭（§5.2.6）：非运行区间置空，读面/诊断不受限。
                None => (0, 0, false, 0),
            };
        Ok(BootWriteGuard {
            inactive_start_lba,
            inactive_lbas,
            active_is_a,
            active_security_version,
            sequence: control.sequence,
            running,
        })
    }

    /// 守卫态 → 安装区间投影（BOOTCTRL 侧由 boot_disk 单源常量补齐；
    /// `InstallBackend::install_ranges` 生产实现与守卫方法入口共用本
    /// 单源——73 批A 具名化前的三处手摆位收敛于此）。
    pub fn install_ranges(&self) -> InstallRanges {
        InstallRanges {
            inactive_start_lba: self.inactive_start_lba,
            inactive_lbas: self.inactive_lbas,
            bootctrl_start_lba: BOOTCTRL_START_LBA,
            bootctrl_lbas: BOOTCTRL_LBAS,
        }
    }

    /// 安装写越界判定：`[lba, lba+count)` 必须**完整落**在非运行槽或
    /// BOOTCTRL 分区之一（跨域/出界/运行槽 = 拒绝）。`count` = **512B
    /// 扇区数**（单源判定见 [`install_write_allowed`]）。身份缺失 ⇒ 非
    /// 运行区间为空，仅 BOOTCTRL 事务分区自身可写（事务语义不变）。
    pub fn install_write_allowed(&self, lba: u64, count: u64) -> bool {
        install_write_allowed(&self.install_ranges(), lba, count)
    }
}

/// 安装写范围判定的**单源**（扇区单位：`count` = 512B 扇区数）。
///
/// REG-70.7-BOOT-TRIAL-IO（run54 实弹）：判定式曾在 `handle_boot_cmd`
/// 与 `BootSlotBackend::write` 各抄一份，且把**字节**长度直接当扇区
/// 相加——4096B 的 BOOTCTRL 记录写（lba 18432）被算成 `18432+4096`
/// 越出 16 扇区的 BOOTCTRL 区间 → 安装事务恒 ST_UNAVAILABLE
///（`BOOT-TRIAL: rejected (io)`）。单位换算（字节 → 扇区上取整）只许
/// 发生在本模块的 [`crate::block_cmd::InstallBackend::write_allowed`]。
pub fn install_write_allowed(ranges: &InstallRanges, lba: u64, count: u64) -> bool {
    let InstallRanges {
        inactive_start_lba,
        inactive_lbas,
        bootctrl_start_lba,
        bootctrl_lbas,
    } = *ranges;
    if count == 0 {
        return false;
    }
    let end = lba.saturating_add(count);
    let in_inactive = lba >= inactive_start_lba && end <= inactive_start_lba + inactive_lbas;
    let in_bootctrl = lba >= bootctrl_start_lba && end <= bootctrl_start_lba + bootctrl_lbas;
    in_inactive || in_bootctrl
}

/// BOOTCTRL 双副本读取辅助：分区基址 + 副本偏移（0 / 4096）。
pub const BOOTCTRL_COPY_OFFSETS: [u64; 2] = [0, BOOT_CONTROL_RECORD_SIZE as u64];

/// 从只读身份页解析本次运行身份（F70-02 生产入口）。
///
/// 页 = root 在 spawn svc-storage 前写入的 `IdentityCapsule` 逐字节拷贝
/// （`STORAGE_BOOT_IDENTITY_VADDR`，只读映射、恒已映射——capsule 缺失时
/// root 写入全零页 ⇒ 解码失败 → None）。版本/长度/CRC 校验失败、或运行
/// 槽为 Recovery/未知（B05：不适用 Recovery 请求）→ None ⇒ 安装与确认
/// 写面整体关闭。
pub fn running_identity_from_page(vaddr: u64) -> Option<RunningIdentity> {
    use eneros_boot_protocol::{IdentityCapsule, IDENTITY_CAPSULE_SIZE};
    // SAFETY: 身份页由 root 在 spawn 前映射进本服务 VSpace（常驻、只读、
    // 恒存在——见 handover-abi STORAGE_BOOT_IDENTITY_VADDR 传递契约）；
    // 读 IDENTITY_CAPSULE_SIZE(136) 字节不越页（页 4096B）。
    let bytes = unsafe { core::slice::from_raw_parts(vaddr as *const u8, IDENTITY_CAPSULE_SIZE) };
    let capsule = IdentityCapsule::decode(bytes).ok()?;
    let is_a = match capsule.selected_slot {
        1 => true,
        2 => false,
        _ => return None,
    };
    Some(RunningIdentity {
        is_a,
        security_version: capsule.security_version,
        release_id: capsule.release_id,
    })
}

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

    fn ident(is_a: bool) -> Option<RunningIdentity> {
        Some(RunningIdentity {
            is_a,
            security_version: 1,
            release_id: [7u8; 16],
        })
    }

    #[test]
    fn guard_resolves_higher_sequence_and_ranges() {
        let lo = make_record(1, 15, 14);
        let hi = make_record(2, 15, 14);
        // 双副本取高序列。
        let g = BootWriteGuard::resolve_with_running(&hi, &lo, ident(true)).expect("resolve");
        assert_eq!(g.sequence, 2);
        assert!(g.active_is_a, "运行身份 = A");
        assert_eq!(
            g.inactive_start_lba,
            eneros_ota_ab::boot_disk::lba_range_of(BootPartition::BootB).0
        );
        assert_eq!(g.inactive_lbas, 16384);
        // 写非运行槽 = 允许；写运行槽 = 拒绝；写 BOOTCTRL = 允许。
        assert!(g.install_write_allowed(g.inactive_start_lba, 8));
        assert!(!g.install_write_allowed(2048, 8), "运行槽 A 基址");
        assert!(g.install_write_allowed(BOOTCTRL_START_LBA + 1, 4));
        assert!(!g.install_write_allowed(BOOTCTRL_START_LBA, 0));
        // 跨界拒绝。
        assert!(!g.install_write_allowed(g.inactive_start_lba + g.inactive_lbas - 4, 8));
    }

    /// F70-02 B01/B02（S1 红转绿）：A 最后一次试启（prepare_attempt 已把
    /// tries 1→0 持久、本次仍引导 A），盘上记录 `select()` 已指向 B——
    /// 守卫的写范围必须仍按**运行槽 A** 计算：A 拒写、B 可写。旧实现
    /// （`select()` 推活跃槽）在此形态下把运行槽 A 开放为安装目标。
    #[test]
    fn last_trial_attempt_keeps_running_slot_write_protected() {
        let mut bc = BootControl::factory([7u8; 16], 1, [7u8; 16]);
        bc.slot_a.tries_remaining = 1;
        let (booting, persisted) = bc.prepare_attempt().unwrap();
        assert_eq!(booting.slot, eneros_boot_protocol::BootSlot::A);
        let next = persisted.unwrap();
        // 持久化记录的 select() 已不再选 A（tries 耗尽）——这正是 S1 的
        // 介质形态。
        assert_ne!(
            next.select().map(|s| s.slot),
            Ok(eneros_boot_protocol::BootSlot::A)
        );
        let disk = next.encode().unwrap();
        let g = BootWriteGuard::resolve_with_running(&disk, &disk, ident(true)).expect("resolve");
        assert!(g.active_is_a, "运行身份 A 贯通（不被 select() 推翻）");
        let (a_lba, a_lbas) = boot_disk::lba_range_of(BootPartition::BootA);
        let (b_lba, _) = boot_disk::lba_range_of(BootPartition::BootB);
        assert!(
            !g.install_write_allowed(a_lba, 1),
            "运行槽 A 首扇区必须拒写"
        );
        assert!(
            !g.install_write_allowed(a_lba + a_lbas - 1, 1),
            "运行槽 A 末扇区必须拒写"
        );
        assert!(g.install_write_allowed(b_lba, 1), "非运行槽 B 可写");

        // B02 对称：B 最后一次试启、运行身份 B → B 拒写、A 可写。
        let id = [7u8; 16];
        let mut bc = BootControl::factory(id, 1, id);
        bc.slot_a = eneros_boot_protocol::SlotState::successful(14, 1, id);
        bc.slot_b = eneros_boot_protocol::SlotState::trial(15, 1, 1, id);
        let (booting, persisted) = bc.prepare_attempt().unwrap();
        assert_eq!(booting.slot, eneros_boot_protocol::BootSlot::B);
        let next = persisted.unwrap();
        assert_ne!(
            next.select().map(|s| s.slot),
            Ok(eneros_boot_protocol::BootSlot::B)
        );
        let disk = next.encode().unwrap();
        let g = BootWriteGuard::resolve_with_running(&disk, &disk, ident(false)).expect("resolve");
        assert!(!g.active_is_a);
        assert!(!g.install_write_allowed(b_lba, 1), "运行槽 B 拒写");
        assert!(g.install_write_allowed(a_lba, 1), "非运行槽 A 可写");
    }

    /// F70-02 B05：运行身份缺失/无效（capsule 缺失、Recovery 运行、旧
    /// ABI）⇒ 安装写面整体关闭（非 BOOTCTRL 区恒拒），BOOTCTRL 事务
    /// 分区维持原受限语义。
    #[test]
    fn missing_identity_closes_install_write_face() {
        let rec = make_record(2, 15, 14);
        let g = BootWriteGuard::resolve_with_running(&rec, &rec, None).expect("resolve");
        assert!(g.running.is_none());
        let (a_lba, _) = boot_disk::lba_range_of(BootPartition::BootA);
        let (b_lba, _) = boot_disk::lba_range_of(BootPartition::BootB);
        assert!(!g.install_write_allowed(a_lba, 1), "无身份 ⇒ A 拒写");
        assert!(!g.install_write_allowed(b_lba, 1), "无身份 ⇒ B 拒写");
        assert!(
            g.install_write_allowed(BOOTCTRL_START_LBA, 8),
            "BOOTCTRL 事务分区维持原语义"
        );
    }

    /// F70-02 B06：边界、跨区间、零长度、u64 溢出范围全部拒绝；非运行槽
    /// 合法边界（首/末扇区）可写。
    #[test]
    fn boundary_zero_and_overflow_ranges_rejected() {
        let rec = make_record(2, 15, 14);
        let g = BootWriteGuard::resolve_with_running(&rec, &rec, ident(true)).expect("resolve");
        let (start, lbas) = (g.inactive_start_lba, g.inactive_lbas);
        assert!(!g.install_write_allowed(start - 1, 1), "区间前一行");
        assert!(!g.install_write_allowed(start + lbas, 1), "区间后一行");
        assert!(!g.install_write_allowed(start, 0), "零长度");
        assert!(!g.install_write_allowed(start + lbas - 2, 4), "跨区间尾");
        assert!(!g.install_write_allowed(u64::MAX, u64::MAX), "溢出回绕");
        assert!(g.install_write_allowed(start, 1), "首扇区");
        assert!(g.install_write_allowed(start + lbas - 1, 1), "末扇区");
    }

    #[test]
    fn both_invalid_rejected() {
        let zero = [0u8; BOOT_CONTROL_RECORD_SIZE];
        assert!(BootWriteGuard::resolve_with_running(&zero, &zero, ident(true)).is_err());
    }

    #[test]
    fn bootctrl_partition_bounds_locked() {
        assert_eq!(BOOTCTRL_START_LBA, 18432);
        assert_eq!(BOOTCTRL_LBAS, 2048);
    }
}
