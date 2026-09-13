//! 70.7（WP-70.7.3/70.7.5）：BootControl 事务内核——boot 面所有者
//!（svc-storage）侧的安装事务单源。
//!
//! 位置裁决定（REG-70.7 排查 run16-22 实弹）：事务最初放在 svc-ota 侧、
//! 经 BLK_BOOT_READ/WRITE 多次 IPC 回 svc-storage 搬运 bootctrl 记录——
//! 实弹中「同客户端第二次调用」稳定挂死于 seL4 Call（guard 首调恒通、
//! 后续调用恒挂）。修订 = 事务随 boot 后端下沉到 svc-storage：每事务
//! **单次 guard 式 IPC**（与首调同型），bootctrl 记录零 IPC 搬运。
//!
//! wire（BLK_BOOT_* 子码，handle_boot_cmd 分派）：
//!
//! | 子码 | 请求 | 响应 |
//! |---|---|---|
//! | `BLK_BOOT_TRIAL` (0x09) | secver(8) + image_id(16) | seq(8) |
//! | `BLK_BOOT_CONFIRM` (0x0a) | slot_is_a(1) + secver(8) + image_id(16) | seq(8) |
//! | `BLK_BOOT_CANCEL` (0x0b) | — | seq(8) ‖ word 文本 |
//! | `BLK_BOOT_STATUS` (0x0c) | — | [`format_update_status`] 文本 |
//!
//! 拒绝 = ST_INVALID + payload 携 word 文本（`no-pending-trial` 等）。

use crate::block_cmd::InstallBackend;

extern crate alloc;
use eneros_boot_protocol::boot_control::BOOT_CONTROL_RECORD_SIZE;
use eneros_boot_protocol::BootControl;

const BC_RECORD: usize = BOOT_CONTROL_RECORD_SIZE;

/// 70.7 安装控制器（WP-70.7.3）：bootctrl 双副本事务。`face` =
/// boot 受限安装面（非活动槽 ∪ BOOTCTRL 写守卫在面内强制）。
pub struct BootInstall<'a> {
    face: &'a mut dyn InstallBackend,
}

impl<'a> BootInstall<'a> {
    pub fn new(face: &'a mut dyn InstallBackend) -> Self {
        BootInstall { face }
    }

    /// 读当前 bootctrl（双副本高序列解析；任一副本有效即取该副本）。
    pub fn read_current(&mut self) -> Result<(BootControl, usize), &'static str> {
        let (bc_lba, _) = eneros_ota_ab::boot_disk::lba_range_of(
            eneros_ota_ab::boot_disk::BootPartition::BootCtrl,
        );
        // 单缓冲复用（栈峰值 -4KB；REG-70.7 排查：IPC 处理链栈深贴线，
        // 双 4KiB 记录缓冲 + 驱动 4KiB 块缓冲叠加曾致静默冻结形态）。
        // 70.14（REG-70.12-PROBE-BOOT-TRIAL-STALL 候选根因）：记录缓冲走**堆**——
        // 栈上 4 KiB 局部会在 boot 安装长链上叠加（实弹反汇编：`BootSlotBackend::write`
        // 的 prologue 即 `sub sp,#0x1000`），逼近 svc-storage 的 64 KiB 栈上限。
        let mut rec: alloc::boxed::Box<[u8; BC_RECORD]> = alloc::boxed::Box::new([0u8; BC_RECORD]);
        self.face.read(bc_lba, &mut rec[..]).map_err(|_| "read0")?;
        let d0 = BootControl::decode(&rec[..]);
        self.face
            .read(bc_lba + 8, &mut rec[..])
            .map_err(|_| "read1")?;
        let d1 = BootControl::decode(&rec[..]);
        match (d0, d1) {
            (Ok(a), Ok(b)) => {
                if a.sequence >= b.sequence {
                    Ok((a, 0))
                } else {
                    Ok((b, 1))
                }
            }
            (Ok(a), Err(_)) => Ok((a, 0)),
            (Err(_), Ok(b)) => Ok((b, 1)),
            _ => Err("bootctrl-both-invalid"),
        }
    }

    /// 写 bootctrl trial 事务（WP-70.7.3）：目标 = **非活动槽** trial
    /// （优先级 15；tries=3；security_version = 目标值），序列 +1；双副本写
    /// （高序列副本先写，低序列后写——掉电窗口任一时刻 resolve 均取更高
    /// 有效序列）+ flush。
    /// `target_image_id`/`target_security_version` 来自 70.5 验签描述。
    /// 返回新序列号。
    ///
    /// **非活动槽判据单源 = `BootControl::select()`**（70.7 复查修正）：
    /// 旧版按 `slot_a.priority >= slot_b.priority` 自判，与
    /// `BootWriteGuard::resolve`（用 `select()`）**在平局/试启动态下相反**——
    /// trial 记录会落到**当前正在运行的槽**（下次引导仍选它，试启动语义
    /// 自我失效），而 svc-ota 的候选身份读面（0x0F）读的是守卫口径的
    /// 「非活动槽」，两侧指向不同物理分区 → manifest 身份与载荷不符。
    /// 现两处同源：select() 选出的是被启动槽，trial 恒写其对侧。
    pub fn write_bootctrl_trial(
        &mut self,
        target_security_version: u64,
        target_image_id: [u8; 16],
    ) -> Result<u64, &'static str> {
        let (current, high_copy) = self.read_current()?;
        // 活跃槽 = select() 的裁决（priority → successful 平局规则的权威
        // 实现）；Recovery/不可选 = 无「非活动槽」概念 → 拒绝（fail-closed：
        // 守卫也不放行任何 BOOT 槽写）。
        let active_is_a = match current.select().map_err(|_| "no-bootable-slot")?.slot {
            eneros_boot_protocol::BootSlot::A => true,
            eneros_boot_protocol::BootSlot::B => false,
            eneros_boot_protocol::BootSlot::Recovery => return Err("recovery-selected"),
        };
        let (bc_lba, _) = eneros_ota_ab::boot_disk::lba_range_of(
            eneros_ota_ab::boot_disk::BootPartition::BootCtrl,
        );
        // 下一控制块：非活动槽 = trial，**严格高于**活跃槽（平局规则
        // "successful 胜 trial" 会使平局永远回旧槽——trial 永不启动）。
        // 策略 = 降活跃 1（≥1 保留回退资格）+ 非活动 = 15 封顶。
        let mut next = current; // BootControl: Copy
        if active_is_a {
            next.slot_a.priority = next.slot_a.priority.saturating_sub(1).max(1);
            next.slot_b = eneros_boot_protocol::SlotState::trial(
                15,
                3,
                target_security_version,
                target_image_id,
            );
        } else {
            next.slot_b.priority = next.slot_b.priority.saturating_sub(1).max(1);
            next.slot_a = eneros_boot_protocol::SlotState::trial(
                15,
                3,
                target_security_version,
                target_image_id,
            );
        }
        next.sequence = current.sequence + 1;
        // 单缓冲编码（栈预算：复用一棵 4 KiB 缓冲；`encode()` 按值返回会在
        // 本帧再叠一份 4 KiB 临时——70.7 目标侧 32 KiB 栈实弹 16 KiB 帧）。
        // 70.14（REG-70.12-PROBE-BOOT-TRIAL-STALL 候选根因）：记录缓冲走**堆**——
        // 栈上 4 KiB 局部会在 boot 安装长链上叠加（实弹反汇编：`BootSlotBackend::write`
        // 的 prologue 即 `sub sp,#0x1000`），逼近 svc-storage 的 64 KiB 栈上限。
        let mut rec: alloc::boxed::Box<[u8; BC_RECORD]> = alloc::boxed::Box::new([0u8; BC_RECORD]);
        next.encode_into(&mut rec).map_err(|_| "encode")?;
        // 写序：高序列副本先写（掉电后 resolve 仍取新块）；再写低副本。
        let (first, second) = if high_copy == 0 {
            ((bc_lba, &rec[..]), (bc_lba + 8, &rec[..]))
        } else {
            ((bc_lba + 8, &rec[..]), (bc_lba, &rec[..]))
        };
        // 70.14 插桩：事务三步各自可见（阻塞点定位：write0 / write1 / flush）。
        #[cfg(target_os = "none")]
        eneros_runtime::println!(
            "[svc-storage] BOOT-TRIAL: write0 lba={} len={} rec_lba={}",
            first.0,
            first.1.len(),
            bc_lba
        );
        self.face.write(first.0, first.1).map_err(|_| "write0")?;
        #[cfg(target_os = "none")]
        eneros_runtime::println!("[svc-storage] BOOT-TRIAL: write1 lba={}", second.0);
        self.face.write(second.0, second.1).map_err(|_| "write1")?;
        #[cfg(target_os = "none")]
        eneros_runtime::println!("[svc-storage] BOOT-TRIAL: flush");
        self.face.flush().map_err(|_| "flush")?;
        #[cfg(target_os = "none")]
        eneros_runtime::println!("[svc-storage] BOOT-TRIAL: done seq={}", next.sequence);
        Ok(next.sequence)
    }

    /// 启动确认（WP-70.7.4）：目标槽 successful=true（不再消耗 tries），
    /// **floor 推进**至已验证运行版本（确认 = 不可逆提交——eligible(floor)
    /// 收窄回退面；试启动失败回退链在确认前的 tries 耗尽路径不受影响），
    /// 序列 +1 双副本写 + flush。`slot_is_a` = 当前启动槽（IdentityCapsule
    /// 投影）；`security_version` = 已验证运行版本。
    pub fn confirm_running(
        &mut self,
        slot_is_a: bool,
        security_version: u64,
        image_id: [u8; 16],
    ) -> Result<u64, &'static str> {
        let (current, _) = self.read_current()?;
        let mut next = current; // BootControl: Copy
        if slot_is_a {
            next.slot_a = eneros_boot_protocol::SlotState::successful(
                next.slot_a.priority.max(1),
                security_version,
                image_id,
            );
        } else {
            next.slot_b = eneros_boot_protocol::SlotState::successful(
                next.slot_b.priority.max(1),
                security_version,
                image_id,
            );
        }
        next.security_floor = next.security_floor.max(security_version);
        next.sequence = current.sequence + 1;
        // 单缓冲编码（栈预算：复用一棵 4 KiB 缓冲；`encode()` 按值返回会在
        // 本帧再叠一份 4 KiB 临时——70.7 目标侧 32 KiB 栈实弹 16 KiB 帧）。
        // 70.14（REG-70.12-PROBE-BOOT-TRIAL-STALL 候选根因）：记录缓冲走**堆**——
        // 栈上 4 KiB 局部会在 boot 安装长链上叠加（实弹反汇编：`BootSlotBackend::write`
        // 的 prologue 即 `sub sp,#0x1000`），逼近 svc-storage 的 64 KiB 栈上限。
        let mut rec: alloc::boxed::Box<[u8; BC_RECORD]> = alloc::boxed::Box::new([0u8; BC_RECORD]);
        next.encode_into(&mut rec).map_err(|_| "encode")?;
        let (bc_lba, _) = eneros_ota_ab::boot_disk::lba_range_of(
            eneros_ota_ab::boot_disk::BootPartition::BootCtrl,
        );
        self.face.write(bc_lba, &rec[..]).map_err(|_| "write0")?;
        self.face
            .write(bc_lba + 8, &rec[..])
            .map_err(|_| "write1")?;
        self.face.flush().map_err(|_| "flush")?;
        Ok(next.sequence)
    }

    /// 取消未确认 trial（WP-70.7.5 mgmt `update-rollback` 后端）：trial 槽
    /// → disabled（新载荷留存但不启动），活跃槽优先级恢复（trial 写曾降
    /// 1——逆操作 `min(15, +1)`）；**floor 不动**（未确认 = 无不可逆提交，
    /// 活跃槽回退资格完整）。序列 +1 双副本写 + flush。已确认/无 trial →
    /// Err("no-pending-trial")——确认后回退 = 新更新事务（floor 之下不可
    /// 达，70.7.4 语义）。
    pub fn cancel_trial(&mut self) -> Result<u64, &'static str> {
        let (current, _) = self.read_current()?;
        // pending trial 判槽（**非**按优先级高低——trial 写后 trial 槽优先
        // 级更高，高低判据会认反）：pending = 非 successful 且可参与启动
        //（priority≠0 / tries≠0）。disabled/successful 槽均非 pending。
        let pending = |s: eneros_boot_protocol::SlotState| {
            !s.successful && s.priority != 0 && s.tries_remaining != 0
        };
        let mut next = current; // BootControl: Copy
        match (pending(current.slot_a), pending(current.slot_b)) {
            (true, false) => {
                let t = current.slot_a;
                next.slot_a =
                    eneros_boot_protocol::SlotState::disabled(t.security_version, t.image_id);
                next.slot_b.priority = (next.slot_b.priority + 1).min(15);
            }
            (false, true) => {
                let t = current.slot_b;
                next.slot_b =
                    eneros_boot_protocol::SlotState::disabled(t.security_version, t.image_id);
                next.slot_a.priority = (next.slot_a.priority + 1).min(15);
            }
            // 双 pending（歧义）或无 pending → 拒绝（fail-closed）。
            _ => return Err("no-pending-trial"),
        }
        next.sequence = current.sequence + 1;
        // 单缓冲编码（栈预算：复用一棵 4 KiB 缓冲；`encode()` 按值返回会在
        // 本帧再叠一份 4 KiB 临时——70.7 目标侧 32 KiB 栈实弹 16 KiB 帧）。
        // 70.14（REG-70.12-PROBE-BOOT-TRIAL-STALL 候选根因）：记录缓冲走**堆**——
        // 栈上 4 KiB 局部会在 boot 安装长链上叠加（实弹反汇编：`BootSlotBackend::write`
        // 的 prologue 即 `sub sp,#0x1000`），逼近 svc-storage 的 64 KiB 栈上限。
        let mut rec: alloc::boxed::Box<[u8; BC_RECORD]> = alloc::boxed::Box::new([0u8; BC_RECORD]);
        next.encode_into(&mut rec).map_err(|_| "encode")?;
        let (bc_lba, _) = eneros_ota_ab::boot_disk::lba_range_of(
            eneros_ota_ab::boot_disk::BootPartition::BootCtrl,
        );
        self.face.write(bc_lba, &rec[..]).map_err(|_| "write0")?;
        self.face
            .write(bc_lba + 8, &rec[..])
            .map_err(|_| "write1")?;
        self.face.flush().map_err(|_| "flush")?;
        Ok(next.sequence)
    }
}

/// 70.7.5 `update-check` 状态投影（纯函数，host 锁格式）：单行文本，
/// svc-storage BLK_BOOT_STATUS 产出、root MGT2 0x06 中继、svc-mgmt
/// detail 直达站端。**零堆**：写入调用方定长缓冲（IPC 处理链上禁止
/// alloc——REG-70.7 run24-26 冻结面收窄后仅余堆/回复两嫌疑，此处消除
/// 堆嫌疑）。
pub fn format_update_status(c: &BootControl, buf: &mut [u8]) -> Result<usize, &'static str> {
    use core::fmt::Write as _;
    struct W<'a> {
        buf: &'a mut [u8],
        n: usize,
        overflow: bool,
    }
    impl core::fmt::Write for W<'_> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let b = s.as_bytes();
            if self.n + b.len() > self.buf.len() {
                self.overflow = true;
                return Err(core::fmt::Error);
            }
            self.buf[self.n..self.n + b.len()].copy_from_slice(b);
            self.n += b.len();
            Ok(())
        }
    }
    let slot = |w: &mut W, s: &eneros_boot_protocol::SlotState| {
        write!(
            w,
            "{}/{}/{}/{}",
            s.priority,
            if s.successful { 1 } else { 0 },
            s.tries_remaining,
            s.security_version
        )
    };
    let mut w = W {
        buf,
        n: 0,
        overflow: false,
    };
    let _ = write!(w, "seq={} floor={} A=", c.sequence, c.security_floor);
    let _ = slot(&mut w, &c.slot_a);
    let _ = write!(w, " B=");
    let _ = slot(&mut w, &c.slot_b);
    if w.overflow {
        return Err("status-overflow");
    }
    Ok(w.n)
}

/// 70.7 安装控制器宿主测试（mock boot 面；事务逻辑与介质布局域分离）。
#[cfg(test)]
mod boot_install_tests {
    use eneros_boot_protocol::boot_control::BOOT_CONTROL_RECORD_SIZE;
    use eneros_boot_protocol::{BootControl, BootSlot};

    use super::*;

    /// mock boot 面：内存介质（BOOTCTRL 分区在真实 qemu-v1 lba 起）。
    struct MockFace {
        media: alloc::vec::Vec<u8>,
        writes: u32,
    }

    const LBA: usize = 512;

    impl MockFace {
        fn new() -> Self {
            // 覆盖真实 BOOTCTRL lba（18432..18448，qemu-v1 布局）。
            MockFace {
                media: alloc::vec![0u8; 18480 * LBA],
                writes: 0,
            }
        }
        fn seed_bootctrl(&mut self, rec: &[u8; BOOT_CONTROL_RECORD_SIZE], copy: usize) {
            let off = (18432 + copy * 8) * LBA;
            self.media[off..off + BOOT_CONTROL_RECORD_SIZE].copy_from_slice(rec);
        }
    }

    impl crate::block_cmd::BlockBackend for MockFace {
        fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
            let off = lba as usize * LBA;
            let end = off + buf.len();
            if end > self.media.len() {
                return Err(());
            }
            buf.copy_from_slice(&self.media[off..end]);
            Ok(())
        }
        fn write(&mut self, lba: u64, data: &[u8]) -> Result<(), ()> {
            let off = lba as usize * LBA;
            let end = off + data.len();
            if end > self.media.len() {
                return Err(());
            }
            self.media[off..end].copy_from_slice(data);
            self.writes += 1;
            Ok(())
        }
        fn flush(&mut self) -> Result<(), ()> {
            Ok(())
        }
        fn health(&self) -> (u64, u32, u8) {
            (0, 512, 0)
        }
    }

    impl InstallBackend for MockFace {
        fn install_ranges(&self) -> (u64, u64, u64, u64) {
            (16384, 16384, 0, 8)
        }
    }

    fn factory_a_trial3() -> [u8; BOOT_CONTROL_RECORD_SIZE] {
        let id = [9u8; 16];
        let mut c = BootControl::factory(id, 1, id);
        c.slot_a = eneros_boot_protocol::SlotState::trial(15, 3, 1, id);
        c.sequence = 1;
        c.encode().unwrap()
    }

    /// 稳态种子（双 successful：A=15 / B=14）——cancel/status 判据域
    ///（factory_a_trial3 的 A=trial 会污染 pending 判槽）。
    fn factory_stable() -> [u8; BOOT_CONTROL_RECORD_SIZE] {
        let id = [9u8; 16];
        let mut c = BootControl::factory(id, 1, id);
        c.slot_a = eneros_boot_protocol::SlotState::successful(15, 1, id);
        c.slot_b = eneros_boot_protocol::SlotState::successful(14, 1, id);
        c.sequence = 1;
        c.encode().unwrap()
    }

    /// **B 活跃**稳态（双 successful：A=14 / B=15）——select() 取 B。
    /// 旧按优先级自判的实现此处恰好也对，但配合下面平局种子即露出反例。
    fn factory_b_active() -> [u8; BOOT_CONTROL_RECORD_SIZE] {
        let id = [9u8; 16];
        let mut c = BootControl::factory(id, 1, id);
        c.slot_a = eneros_boot_protocol::SlotState::successful(14, 1, id);
        c.slot_b = eneros_boot_protocol::SlotState::successful(15, 1, id);
        c.sequence = 1;
        c.encode().unwrap()
    }

    /// 平局种子（双 successful 同级：A=B=15）——select() 走「B successful
    /// 且 A 非」不成立 → **优先 A**；而 `priority >= priority` 的自判同样
    /// 得 A。真正露反例的是「B successful 且 A trial 且同级」：select() 取
    /// B（successful 胜 trial），自判却取 A。
    fn factory_tie_b_successful() -> [u8; BOOT_CONTROL_RECORD_SIZE] {
        let id = [9u8; 16];
        let mut c = BootControl::factory(id, 1, id);
        c.slot_a = eneros_boot_protocol::SlotState::trial(15, 3, 1, id);
        c.slot_b = eneros_boot_protocol::SlotState::successful(15, 1, id);
        c.sequence = 1;
        c.encode().unwrap()
    }

    #[test]
    fn trial_transaction_promotes_inactive_strictly_higher() {
        let mut face = MockFace::new();
        face.seed_bootctrl(&factory_a_trial3(), 0);
        face.seed_bootctrl(&factory_a_trial3(), 1);
        let mut inst = BootInstall::new(&mut face);
        let seq = inst.write_bootctrl_trial(2, [3u8; 16]).expect("trial");
        assert_eq!(seq, 2, "序列 +1");
        // 新控制块：B = trial(15, tries=3, secver=2) 严格高于 A（14）。
        let c = inst.read_current().expect("read").0;
        assert_eq!(c.slot_a.priority, 14, "活跃降 1（≥1 保留回退）");
        assert_eq!(c.slot_b.priority, 15);
        assert_eq!(c.slot_b.tries_remaining, 3);
        assert_eq!(c.slot_b.security_version, 2);
        assert_eq!(c.slot_b.image_id, [3u8; 16]);
        assert_eq!(c.security_floor, 1, "trial 不推进 floor（回退链完整）");
        // select → B（trial 严格高）。
        let sel = c.select().expect("select");
        assert_eq!(sel.slot, BootSlot::B);
        assert!(sel.trial);
    }

    #[test]
    fn confirm_marks_successful_and_sequence_advances() {
        let mut face = MockFace::new();
        face.seed_bootctrl(&factory_stable(), 0);
        face.seed_bootctrl(&factory_stable(), 1);
        let mut inst = BootInstall::new(&mut face);
        let seq0 = inst.write_bootctrl_trial(2, [3u8; 16]).expect("trial");
        let seq1 = inst.confirm_running(false, 2, [3u8; 16]).expect("confirm");
        assert_eq!(seq1, seq0 + 1);
        let (c, _) = inst.read_current().expect("read");
        assert!(c.slot_b.successful, "确认后 B = successful");
        assert_eq!(c.slot_b.tries_remaining, 0, "successful ⇒ tries 0");
        assert_eq!(c.security_floor, 2, "确认推进 floor（不可逆提交）");
        // 再次启动选择保持 B（successful 优先级 15 > A 14）。
        let sel = c.select().expect("select");
        assert_eq!(sel.slot, BootSlot::B);
        assert!(!sel.trial);
    }

    /// 70.7 复查回归锁：trial 目标槽 = **select() 的裁决对侧**。
    ///
    /// 缺陷形态（旧实现）：按 `slot_a.priority >= slot_b.priority` 自判活跃槽。
    /// 平局时自判 A、select() 也返回 A（successful 平局规则）——但**当活跃
    /// 槽是 B 且两槽优先级相等**时（B 确认后 A 降 1 再被 trial 写回 15、
    /// 或 cancel 恢复优先级后两槽同为 successful 15/14 的多轮事务），
    /// 自判与 select() 相反 → trial 记录落在**正在运行的槽**上（下次引导
    /// 仍选它 = 试启动自我失效），且 svc-ota 候选身份读面按守卫口径取的
    /// 是另一分区 → manifest 身份与载荷不符。
    ///
    /// 本锁断言：任意种子下，trial 写入的槽恒不等 select() 当前裁决槽，
    /// 且新记录 select() 恰好指向 trial 槽（试启动语义成立）。
    #[test]
    fn trial_targets_opposite_of_select_not_priority_order() {
        // 种子矩阵：A 活跃（15/14）、B 活跃（14/15）、**双 successful 平局
        // 但 B 优先**（15/15 → select 走 successful 规则取 A）、trial 态。
        type Seed = fn() -> [u8; BOOT_CONTROL_RECORD_SIZE];
        let seeds: [(&str, Seed); 4] = [
            ("A-active", factory_stable),
            ("B-active", factory_b_active),
            ("tie-b-successful", factory_tie_b_successful),
            ("A-trial-remain", factory_a_trial3),
        ];
        for (name, seed) in seeds {
            let mut face = MockFace::new();
            let rec = seed();
            face.seed_bootctrl(&rec, 0);
            face.seed_bootctrl(&rec, 1);
            let mut inst = BootInstall::new(&mut face);
            let before = inst
                .read_current()
                .expect("read")
                .0
                .select()
                .expect("select")
                .slot;
            inst.write_bootctrl_trial(2, [3u8; 16]).expect("trial");
            let after = inst.read_current().expect("read").0;
            let sel = after.select().expect("select");
            // 种子全是可引导态；落到 Recovery 即种子本身失效。
            assert_ne!(before, BootSlot::Recovery, "{name}: 种子不得落在 Recovery");
            let expected = match before {
                BootSlot::A => BootSlot::B,
                BootSlot::B => BootSlot::A,
                BootSlot::Recovery => BootSlot::Recovery,
            };
            assert_eq!(sel.slot, expected, "{name}: trial 必须落在 select() 对侧");
            assert!(sel.trial, "{name}: 对新记录必须是试启动态");
            // 回退资格：原活跃槽优先级保留 ≥1（tries 耗尽可回退）。
            let old = if expected == BootSlot::B {
                after.slot_a
            } else {
                after.slot_b
            };
            assert!(old.priority >= 1, "{name}: 原活跃槽保留回退资格");
        }
    }

    #[test]
    fn sequence_monotonic_across_repeated_transactions() {
        let mut face = MockFace::new();
        face.seed_bootctrl(&factory_a_trial3(), 0);
        face.seed_bootctrl(&factory_a_trial3(), 1);
        let mut inst = BootInstall::new(&mut face);
        let mut last = inst.write_bootctrl_trial(2, [3u8; 16]).unwrap();
        for _ in 0..3 {
            let s = inst.write_bootctrl_trial(3, [4u8; 16]).unwrap();
            assert_eq!(s, last + 1, "序列严格 +1");
            last = s;
        }
        assert_eq!(last, 5);
    }

    #[test]
    fn cancel_trial_disables_trial_and_restores_active() {
        let mut face = MockFace::new();
        face.seed_bootctrl(&factory_stable(), 0);
        face.seed_bootctrl(&factory_stable(), 1);
        let mut inst = BootInstall::new(&mut face);
        inst.write_bootctrl_trial(2, [3u8; 16]).expect("trial");
        let seq = inst.cancel_trial().expect("cancel");
        let (c, _) = inst.read_current().expect("read");
        assert_eq!(seq, c.sequence);
        assert_eq!(c.security_floor, 1, "取消不推进 floor");
        // B trial → disabled（payload 留存不启动）；A 恢复 15 successful。
        assert_eq!(c.slot_b.priority, 0, "trial 槽 disabled");
        assert!(!c.slot_b.successful);
        assert_eq!(c.slot_b.tries_remaining, 0);
        assert_eq!(c.slot_a.priority, 15, "活跃槽优先级恢复");
        assert!(c.slot_a.successful);
        // 再次取消 → no-pending-trial（幂等拒绝）。
        assert_eq!(inst.cancel_trial(), Err("no-pending-trial"));
    }

    #[test]
    fn cancel_trial_rejects_confirmed_state() {
        // trial → confirm（成功提交）之后取消必须拒绝（floor 已推进，
        // 回退 = 新更新事务）。
        let mut face = MockFace::new();
        face.seed_bootctrl(&factory_stable(), 0);
        face.seed_bootctrl(&factory_stable(), 1);
        let mut inst = BootInstall::new(&mut face);
        inst.write_bootctrl_trial(2, [3u8; 16]).expect("trial");
        inst.confirm_running(false, 2, [3u8; 16]).expect("confirm");
        assert_eq!(inst.cancel_trial(), Err("no-pending-trial"));
    }

    #[test]
    fn update_status_format_is_deterministic() {
        let mut face = MockFace::new();
        face.seed_bootctrl(&factory_stable(), 0);
        face.seed_bootctrl(&factory_stable(), 1);
        let mut inst = BootInstall::new(&mut face);
        inst.write_bootctrl_trial(2, [3u8; 16]).expect("trial");
        let (c, _) = inst.read_current().expect("read");
        let mut buf = [0u8; 64];
        let n = format_update_status(&c, &mut buf).expect("format");
        assert_eq!(&buf[..n], b"seq=2 floor=1 A=14/1/0/1 B=15/0/3/2");
        // 溢出拒绝（fail-closed，不静默裁剪）。
        assert_eq!(
            format_update_status(&c, &mut buf[..8]),
            Err("status-overflow")
        );
    }
}
