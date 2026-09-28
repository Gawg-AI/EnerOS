//! 70.7（WP-70.7.3/70.7.5）：BootControl 事务内核——boot 面所有者
//!（svc-storage）侧的安装事务单源。
//!
//! 位置裁决定（REG-70.7 排查 run16-22 实弹）：事务最初放在 svc-ota 侧、
//! 经 BLK_BOOT_READ/WRITE 多次 IPC 回 svc-storage 搬运 bootctrl 记录——
//! 实弹中「同客户端第二次调用」稳定挂死于 seL4 Call（guard 首调恒通、
//! 后续调用恒挂）。修订 = 事务随 boot 后端下沉到 svc-storage：每事务
//! **单次 guard 式 IPC**（与首调同型），bootctrl 记录零 IPC 搬运。
//!
//! wire（BLK_BOOT_* 子码，handle_boot_cmd 分派；**字段偏移单源 =
//! [`crate::blk_wire`]，本表是叙述不是定义处**）：
//!
//! | 子码 | 请求 | 响应 |
//! |---|---|---|
//! | `BLK_BOOT_TRIAL` (0x09) | secver(8) + image_id(16) | seq(8) |
//! | `BLK_BOOT_CONFIRM` (0x0a) | slot_is_a(1) + secver(8) + image_id(16) | seq(8) |
//! | `BLK_BOOT_CANCEL` (0x0b) | — | seq(8) ‖ word 文本 |
//! | `BLK_BOOT_STATUS` (0x0c) | — | [`format_update_status`] 文本 |
//!
//! 拒绝 = ST_INVALID + payload 携 word 文本（`no-pending-trial` 等）。
//! 词形全集由 [`BootReject`] 枚举承载（蓝图 77 卷二 WP-77.2.3：调用方
//! 据枚举判错误阶段，字面逐字节维持 wire golden）。

use crate::block_cmd::InstallBackend;
use crate::boot_guard::BOOTCTRL_COPY_STRIDE_LBAS;

/// BootInstall 拒绝词全集（蓝图 77 卷二 WP-77.2.3 单源）。14 个词形与
/// 收编前的 `&'static str` 逐字节一致——wire（ST_INVALID + payload word）
/// 不变，只有**返回类型**从裸串变为具名枚举：
///
/// - 服务端（`block_cmd::boot_word_reject`）仍发送 `as_word()` 原字节；
/// - 客户端（agent-sdk `BlockIo`）仅对四个事务子码（TRIAL/CONFIRM/
///   CANCEL/STATUS）+ `ST_INVALID` + 完整已知词构造 `Rejected`；
///   未知词/空载/其他状态/坏信封一律不进本枚举（诊断/管理语义面，
///   越权分类 = `ERR:` 假成功面被打开，蓝图 §2.5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootReject {
    /// bootctrl 副本 0 读失败（`read_current`）。
    Read0,
    /// bootctrl 副本 1 读失败（`read_current`）。
    Read1,
    /// 双副本均不可解码（读面即拒）。
    BootctrlBothInvalid,
    /// 记录重编码失败（`commit`）。
    Encode,
    /// 首写副本失败（`commit`；positional——copy0/copy1 皆可能，只报阶段）。
    Write0,
    /// 次写副本失败（`commit`；**介质可能已部分更新**——copy0 可能已写入，
    /// 不得解释成"未写入"，不得自动重试）。
    Write1,
    /// flush 失败（`commit`；同上：双副本写已发生）。
    Flush,
    /// 守卫导出的非活动区间不是任一 BOOT 槽（安装面自相矛盾，fail-closed）。
    InstallRangeNotASlot,
    /// 运行身份缺失（确认面拒绝 + 写面关闭）。
    NoRunningIdentity,
    /// 确认槽 ≠ 本次实际运行槽。
    ConfirmSlotNotRunning,
    /// 确认 security_version ≠ 运行载荷身份。
    ConfirmSecverMismatch,
    /// 确认 image_id ≠ 运行载荷身份。
    ConfirmImageMismatch,
    /// 无 pending trial（幂等拒绝；已确认回退 = 新更新事务）。
    NoPendingTrial,
    /// 状态投影溢出（缓冲不足，fail-closed 不裁剪）。
    StatusOverflow,
}

impl BootReject {
    /// wire 词形（与收编前 `&'static str` 逐字节一致）。
    pub const fn as_word(self) -> &'static str {
        match self {
            BootReject::Read0 => "read0",
            BootReject::Read1 => "read1",
            BootReject::BootctrlBothInvalid => "bootctrl-both-invalid",
            BootReject::Encode => "encode",
            BootReject::Write0 => "write0",
            BootReject::Write1 => "write1",
            BootReject::Flush => "flush",
            BootReject::InstallRangeNotASlot => "install-range-not-a-slot",
            BootReject::NoRunningIdentity => "no-running-identity",
            BootReject::ConfirmSlotNotRunning => "confirm-slot-not-running",
            BootReject::ConfirmSecverMismatch => "confirm-secver-mismatch",
            BootReject::ConfirmImageMismatch => "confirm-image-mismatch",
            BootReject::NoPendingTrial => "no-pending-trial",
            BootReject::StatusOverflow => "status-overflow",
        }
    }

    /// 完整已知词 → 枚举（客户端分类唯一入口；非完整词形/未知词 → None）。
    pub fn from_word(word: &[u8]) -> Option<Self> {
        let w = core::str::from_utf8(word).ok()?;
        Some(match w {
            "read0" => BootReject::Read0,
            "read1" => BootReject::Read1,
            "bootctrl-both-invalid" => BootReject::BootctrlBothInvalid,
            "encode" => BootReject::Encode,
            "write0" => BootReject::Write0,
            "write1" => BootReject::Write1,
            "flush" => BootReject::Flush,
            "install-range-not-a-slot" => BootReject::InstallRangeNotASlot,
            "no-running-identity" => BootReject::NoRunningIdentity,
            "confirm-slot-not-running" => BootReject::ConfirmSlotNotRunning,
            "confirm-secver-mismatch" => BootReject::ConfirmSecverMismatch,
            "confirm-image-mismatch" => BootReject::ConfirmImageMismatch,
            "no-pending-trial" => BootReject::NoPendingTrial,
            "status-overflow" => BootReject::StatusOverflow,
            _ => return None,
        })
    }
}

extern crate alloc;
use eneros_boot_protocol::boot_control::BOOT_CONTROL_RECORD_SIZE;
use eneros_boot_protocol::BootControl;

const BC_RECORD: usize = BOOT_CONTROL_RECORD_SIZE;

/// 双副本间距的**单位恒等**编译期锁（蓝图 76 卷三 WP-76.3.2）：boot-protocol
/// 侧以**字节**偏移表达副本（`BOOT_CONTROL_COPY_OFFSETS` = [0, 4096]），
/// drv-svc 侧以**512B 扇区**表达（`BOOTCTRL_COPY_STRIDE_LBAS`）。两处单位
/// 分裂只允许在「同一算术」上成立——字面双源由本断言钉死，任一侧改单位即
/// 编译错。
const _: () = assert!(
    crate::boot_guard::BOOTCTRL_COPY_STRIDE_LBAS * 512
        == eneros_boot_protocol::boot_control::BOOT_CONTROL_COPY_OFFSETS[1]
);

/// 槽是否处于 **pending trial**（未确认、可参与启动）态。
///
/// 口径（70.7 复查 + 70.14 复查单源化）：`pending = 非 successful 且
/// priority≠0 且 tries≠0`；disabled / successful 槽均非 pending。
/// **不得**按优先级高低判（trial 写后 trial 槽优先级更高，高低判据会认反）。
fn is_pending_trial(s: eneros_boot_protocol::SlotState) -> bool {
    !s.successful && s.priority != 0 && s.tries_remaining != 0
}

/// 70.7 安装控制器（WP-70.7.3）：bootctrl 双副本事务。`face` =
/// boot 受限安装面（非活动槽 ∪ BOOTCTRL 写守卫在面内强制）。
pub struct BootInstall<'a> {
    face: &'a mut dyn InstallBackend,
}

impl<'a> BootInstall<'a> {
    pub fn new(face: &'a mut dyn InstallBackend) -> Self {
        BootInstall { face }
    }

    /// BOOTCTRL 分区起始 LBA（单源 = `boot_disk` 介质图；本文件生产侧唯一
    /// 取值点——事务写序与读面共用）。
    fn bootctrl_lba() -> u64 {
        eneros_ota_ab::boot_disk::lba_range_of(eneros_ota_ab::boot_disk::BootPartition::BootCtrl).0
    }

    /// 读当前 bootctrl（双副本高序列解析；任一副本有效即取该副本）。
    ///
    /// 第二返回值 `write_first` = **提交时应先写的副本序号**（0/1），它是
    /// [`Self::commit`] 的写序输入，事务侧**不得丢弃**。判据三条（不变量的
    /// 准确形态，比固件注释那句「高序列副本先写」更严——独立复查轮坐实，
    /// 登记 REG-76-M）：
    /// 1. **双副本均可解码** → 先写较高序列一侧（较低侧通常正是锚匹配副本，
    ///    中断时锚存留；平局取 copy0）；
    /// 2. **仅一侧可解码** → 先写**不可解码那一侧**。可解码侧是当前唯一可
    ///    引导事实，先覆它 = 任一中断点全盲（双副本皆不可解 → 固件侧
    ///    `bootctrl-both-invalid`；或锚不再匹配 → `AnchorMismatch` 引导失败）。
    ///    把「较高序列」直译成写序输入在本形态下会指向幸存副本本身——
    ///    蓝图 76 卷三 §1A.4 对同一形态的判定是「首写落无效副本、锚匹配
    ///    幸存，安全」，故本条以该判定为准；
    /// 3. 双副本均不可解码 → `Err("bootctrl-both-invalid")`（读面即拒）。
    pub fn read_current(&mut self) -> Result<(BootControl, usize), BootReject> {
        let bc_lba = Self::bootctrl_lba();
        // 单缓冲复用（栈峰值 -4KB；REG-70.7 排查：IPC 处理链栈深贴线，
        // 双 4KiB 记录缓冲 + 驱动 4KiB 块缓冲叠加曾致静默冻结形态）。
        // 70.14（REG-70.12-PROBE-BOOT-TRIAL-STALL 候选根因）：记录缓冲走**堆**——
        // 栈上 4 KiB 局部会在 boot 安装长链上叠加（实弹反汇编：`BootSlotBackend::write`
        // 的 prologue 即 `sub sp,#0x1000`），逼近 svc-storage 的 64 KiB 栈上限。
        let mut rec: alloc::boxed::Box<[u8; BC_RECORD]> = alloc::boxed::Box::new([0u8; BC_RECORD]);
        self.face
            .read_sectors(bc_lba, &mut rec[..])
            .map_err(|_| BootReject::Read0)?;
        let d0 = BootControl::decode(&rec[..]);
        self.face
            .read_sectors(bc_lba + BOOTCTRL_COPY_STRIDE_LBAS, &mut rec[..])
            .map_err(|_| BootReject::Read1)?;
        let d1 = BootControl::decode(&rec[..]);
        match (d0, d1) {
            (Ok(a), Ok(b)) => {
                if a.sequence >= b.sequence {
                    Ok((a, 0))
                } else {
                    Ok((b, 1))
                }
            }
            // 判据 2：幸存侧不可动，先写另一侧。
            (Ok(a), Err(_)) => Ok((a, 1)),
            (Err(_), Ok(b)) => Ok((b, 0)),
            _ => Err(BootReject::BootctrlBothInvalid),
        }
    }

    /// 双副本提交单源（WP-76.3.2）：编码 → **先写 `write_first` 副本** → 另一
    /// 副本 → flush。三事务（trial/confirm/cancel）的介质写半边全部经本函数——
    /// 写序分派在类型面上不再可表达为「按调用方记得的顺序」。
    ///
    /// 不变量 = 「任一中断点都保留锚定旧副本（固件侧 `resolve_boot_control`
    /// 精确匹配优先命中）」，其运行时形态由 [`Self::read_current`] 的三条判据
    /// 给出（双可解 → 高序列先写；单可解 → 损败侧先写）。固件侧注释那句
    /// 「高序列副本 → 低序列副本 → flush」是双可解情形的表述，不覆盖单侧
    /// 损败形态——该处曾按字面收编，独立复查轮证伪并改正（REG-76-M）。
    /// 实证锁 = 本文件测试区
    /// `confirm_and_cancel_write_high_seq_copy_first` /
    /// `power_loss_between_writes_preserves_anchor_match` /
    /// `commit_writes_undecodable_copy_first_when_one_side_is_sole_survivor`。
    ///
    /// 设计裁决（偏离体检报告原案、登记 REG-76-M）：报告建议 `BootctrlMedia`
    /// adapter 类型；本卷落为 `BootInstall` 私有方法——无第二消费者（三事务
    /// 都在本类型内）、介质访问已由 [`InstallBackend`] face 承载、新类型零增益。
    ///
    /// `tag` 仅用于 70.14 式排障插桩的词形前缀：`"BOOT-TRIAL"` 与收编前
    /// 逐字节一致；`"BOOT-CONFIRM"`/`"BOOT-CANCEL"` 为本卷新增对称插桩
    /// （成功半边此前零插桩，零门 grep 已盘点——见卷三 §1A.5）。
    /// 错误词形 `encode`/`write0`/`write1`/`flush` 是 positional 的：换序后
    /// 「首写=write0」的语义自洽，词形本身一字未改。
    fn commit(
        &mut self,
        write_first: usize,
        next: BootControl,
        tag: &'static str,
    ) -> Result<u64, BootReject> {
        // 单缓冲编码（栈预算：复用一棵 4 KiB 缓冲；`encode()` 按值返回会在
        // 本帧再叠一份 4 KiB 临时——70.7 目标侧 32 KiB 栈实弹 16 KiB 帧）。
        // 70.14（REG-70.12-PROBE-BOOT-TRIAL-STALL 候选根因）：记录缓冲走**堆**——
        // 栈上 4 KiB 局部会在 boot 安装长链上叠加（实弹反汇编：`BootSlotBackend::write`
        // 的 prologue 即 `sub sp,#0x1000`），逼近 svc-storage 的 64 KiB 栈上限。
        // （收编前三事务各抄一份同段注释——蓝图 76 卷三 §1A 数出的「4 段逐字
        // 重复」；现由本函数承载一份。）
        let bc_lba = Self::bootctrl_lba();
        // tag 只被 cfg(target_os="none") 的插桩消费——host 构建不编译那些行。
        let _ = tag;
        let mut rec: alloc::boxed::Box<[u8; BC_RECORD]> = alloc::boxed::Box::new([0u8; BC_RECORD]);
        next.encode_into(&mut rec).map_err(|_| BootReject::Encode)?;
        // 写序：先写 `write_first` 副本（判据三条见 [`Self::read_current`]），
        // 再写另一副本——掉电窗口任一时刻 resolve 仍握有锚定/可解码副本。
        let (first, second) = if write_first == 0 {
            (
                (bc_lba, &rec[..]),
                (bc_lba + BOOTCTRL_COPY_STRIDE_LBAS, &rec[..]),
            )
        } else {
            (
                (bc_lba + BOOTCTRL_COPY_STRIDE_LBAS, &rec[..]),
                (bc_lba, &rec[..]),
            )
        };
        // 70.14 插桩：事务三步各自可见（阻塞点定位：write0 / write1 / flush）。
        #[cfg(target_os = "none")]
        eneros_runtime::println!(
            "[svc-storage] {tag}: write0 lba={} len={} rec_lba={}",
            first.0,
            first.1.len(),
            bc_lba
        );
        self.face
            .write_sectors(first.0, first.1)
            .map_err(|_| BootReject::Write0)?;
        #[cfg(target_os = "none")]
        eneros_runtime::println!("[svc-storage] {tag}: write1 lba={}", second.0);
        self.face
            .write_sectors(second.0, second.1)
            .map_err(|_| BootReject::Write1)?;
        #[cfg(target_os = "none")]
        eneros_runtime::println!("[svc-storage] {tag}: flush");
        self.face.flush().map_err(|_| BootReject::Flush)?;
        #[cfg(target_os = "none")]
        eneros_runtime::println!("[svc-storage] {tag}: done seq={}", next.sequence);
        Ok(next.sequence)
    }

    /// 写 bootctrl trial 事务（WP-70.7.3）：目标 = **非活动槽** trial
    /// （优先级 15；tries=3；security_version = 目标值），序列 +1；双副本写
    /// + flush。**写序半边三事务单源 = [`Self::commit`]**；首写落点判据
    /// 三条（双可解→高序列侧；单侧可解→损败侧；双败即拒）单源在
    /// [`Self::read_current`]——REG-76-M 复查轮收紧后「高序列副本先写」
    /// 不再是普适表述，此处不复述写序（教义只留一处）。
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
    ) -> Result<u64, BootReject> {
        let (current, write_first) = self.read_current()?;
        // **trial 目标槽 = 守卫的非活动槽**（70.14 复查修复，单源 = 安装写面）。
        //
        // 为什么不能再用 `select()`：`select()` 是「下一引导槽」，首次 trial
        // 写成功后它就指向 trial 槽；而载荷真正被安装到的分区由
        // `BootWriteGuard`（init 期解析一次 = **本 boot 正在运行的槽的对侧**）
        // 决定——0x0F 候选身份读面读的也是它。两处若各判一次，同一 boot 内
        // 第二次 trial 就会把记录指向**正在运行**的槽：记录身份与载荷身份
        // 分叉（70.7-BOOT-TRIAL-BIND 修复面反向失效），安装写还会打到运行槽。
        // 现三处（安装写 / 候选身份读 / trial 记录）同源于 `install_ranges`。
        let ranges = self.face.install_ranges();
        let (inactive_start, inactive_lbas) = (ranges.inactive_start_lba, ranges.inactive_lbas);
        let (a_start, a_lbas) =
            eneros_ota_ab::boot_disk::lba_range_of(eneros_ota_ab::boot_disk::BootPartition::BootA);
        let (b_start, b_lbas) =
            eneros_ota_ab::boot_disk::lba_range_of(eneros_ota_ab::boot_disk::BootPartition::BootB);
        let active_is_a = if inactive_start == a_start && inactive_lbas == a_lbas {
            false
        } else if inactive_start == b_start && inactive_lbas == b_lbas {
            true
        } else {
            // 守卫导出的非活动区间不是任一 BOOT 槽 → 安装面自相矛盾，
            // 拒绝（fail-closed，不猜目标）。
            return Err(BootReject::InstallRangeNotASlot);
        };
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
        self.commit(write_first, next, "BOOT-TRIAL")
    }

    /// 启动确认（WP-70.7.4）：目标槽 successful=true（不再消耗 tries），
    /// **floor 推进**至已验证运行版本（确认 = 不可逆提交——eligible(floor)
    /// 收窄回退面；试启动失败回退链在确认前的 tries 耗尽路径不受影响），
    /// 序列 +1 双副本写 + flush。`slot_is_a` = 当前启动槽（IdentityCapsule
    /// 投影）；`security_version` = 已验证运行版本。
    ///
    /// **F70-02（S1）修复：确认基线 = 本次实际运行身份**（可信启动交接
    /// 数据，`InstallBackend::running_identity`），不是 `select()`。
    /// `select()` 是"下一引导槽"：A 最后一次试启被 `prepare_attempt`
    /// 持久扣至 tries=0 后，本次仍运行 A 而记录的 `select()` 已指向 B——
    /// 旧实现（与 select() 比对）在该形态下把**正在运行槽的健康确认**
    /// 拒为 `confirm-slot-not-selected`（S1 的确认半边）。现三项核对：
    ///
    /// 1. 运行身份缺失 ⇒ 拒绝（`no-running-identity`，§5.2.6 写面关闭）；
    /// 2. 确认槽 ≠ 实际运行槽 ⇒ 拒绝（`confirm-slot-not-running`）；
    /// 3. `security_version`/`image_id` ≠ 运行载荷身份（capsule 投影）
    ///    ⇒ 拒绝（`confirm-secver-mismatch` / `confirm-image-mismatch`，
    ///    §5.2.5：错身份、虚增安全水位的确认无介质状态推进）。
    ///
    /// 拒绝面零写入（记录逐字节不变、floor 不动）。正常最后一次试启的
    /// 正确确认必须成功（capsule 与请求同源 ⇒ 等值核对恒过）。
    pub fn confirm_running(
        &mut self,
        slot_is_a: bool,
        security_version: u64,
        image_id: [u8; 16],
    ) -> Result<u64, BootReject> {
        let ident = self
            .face
            .running_identity()
            .ok_or(BootReject::NoRunningIdentity)?;
        if ident.is_a != slot_is_a {
            return Err(BootReject::ConfirmSlotNotRunning);
        }
        if ident.security_version != security_version {
            return Err(BootReject::ConfirmSecverMismatch);
        }
        if ident.release_id != image_id {
            return Err(BootReject::ConfirmImageMismatch);
        }
        let (current, write_first) = self.read_current()?;
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
        // 写序半边单源 = [`Self::commit`]（收编前此处手写恒 copy0→copy1，
        // 并把 read_current 的写序输入丢弃为 `_`——WP-76.3.1 红臂即此）。
        self.commit(write_first, next, "BOOT-CONFIRM")
    }

    /// 取消未确认 trial（WP-70.7.5 mgmt `update-rollback` 后端）：trial 槽
    /// → disabled（新载荷留存但不启动），活跃槽优先级恢复（trial 写曾降
    /// 1——逆操作 `min(15, +1)`）；**floor 不动**（未确认 = 无不可逆提交，
    /// 活跃槽回退资格完整）。序列 +1 双副本写 + flush。已确认/无 trial →
    /// Err("no-pending-trial")——确认后回退 = 新更新事务（floor 之下不可
    /// 达，70.7.4 语义）。
    pub fn cancel_trial(&mut self) -> Result<u64, BootReject> {
        let (current, write_first) = self.read_current()?;
        // pending trial 判槽单源 = [`is_pending_trial`]（**非**按优先级高低——
        // trial 写后 trial 槽优先级更高，高低判据会认反）。
        let mut next = current; // BootControl: Copy
        match (
            is_pending_trial(current.slot_a),
            is_pending_trial(current.slot_b),
        ) {
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
            _ => return Err(BootReject::NoPendingTrial),
        }
        next.sequence = current.sequence + 1;
        // 写序半边单源 = [`Self::commit`]（收编前与 confirm 同型：手写恒
        // copy0→copy1 + 写序输入丢弃，WP-76.3.1 红臂即此）。
        self.commit(write_first, next, "BOOT-CANCEL")
    }
}

/// 70.7.5 `update-check` 状态投影（纯函数，host 锁格式）：单行文本，
/// svc-storage BLK_BOOT_STATUS 产出、root MGT2 0x06 中继、svc-mgmt
/// detail 直达站端。**零堆**：写入调用方定长缓冲（IPC 处理链上禁止
/// alloc——REG-70.7 run24-26 冻结面收窄后仅余堆/回复两嫌疑，此处消除
/// 堆嫌疑）。
pub fn format_update_status(c: &BootControl, buf: &mut [u8]) -> Result<usize, BootReject> {
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
        return Err(BootReject::StatusOverflow);
    }
    Ok(w.n)
}

/// 70.7 安装控制器宿主测试（mock boot 面；事务逻辑与介质布局域分离）。
#[cfg(test)]
mod boot_install_tests {
    use eneros_boot_protocol::boot_control::BOOT_CONTROL_RECORD_SIZE;
    use eneros_boot_protocol::{BootControl, BootSlot};

    use super::*;
    use crate::boot_guard::BOOTCTRL_START_LBA;

    /// mock boot 面：内存介质（BOOTCTRL 分区在真实 qemu-v1 lba 起）。
    struct MockFace {
        media: alloc::vec::Vec<u8>,
        writes: u32,
        /// **写序记录**（WP-76.3.1）：每笔 `(lba, 字节)` 按调用顺序入表——
        /// 掉电窗口推演的实证面（首写落哪个副本只能从介质写入次序读出）。
        log: core::cell::RefCell<alloc::vec::Vec<(u64, alloc::vec::Vec<u8>)>>,
        /// flush 失败注入（WP-76.3.2 变异咬合）：`commit` 的两笔写之后必须
        /// 落 flush——恒 Ok 的 mock 咬不住「删掉 flush」这一刀，本 latch 补牙。
        flush_fail: bool,
        /// 守卫**冻结**缓存（生产 = init 期解析一次、boot 内不变）。
        /// 不缓存就等于"守卫随介质漂移"，与生产语义不符（会掩盖
        /// trial 目标与安装面分叉）。
        guard: core::cell::RefCell<Option<crate::boot_guard::InstallRanges>>,
        /// F70-02：本次运行身份（生产 = 只读身份页 capsule；None ⇒ 写面
        /// 关闭 + 确认拒绝）。默认 = A 槽（factory 种子形态）。
        running: Option<crate::boot_guard::RunningIdentity>,
    }

    const LBA: usize = 512;

    /// 测试用 A 槽运行身份（与 factory 种子的 secver/id 对齐；可按测试覆写）。
    fn ident_a() -> crate::boot_guard::RunningIdentity {
        crate::boot_guard::RunningIdentity {
            is_a: true,
            security_version: 1,
            release_id: [9u8; 16],
        }
    }

    impl MockFace {
        fn new() -> Self {
            // 覆盖真实 BOOTCTRL lba（18432..18448，qemu-v1 布局）。
            MockFace {
                media: alloc::vec![0u8; 18480 * LBA],
                writes: 0,
                log: core::cell::RefCell::new(alloc::vec::Vec::new()),
                flush_fail: false,
                guard: core::cell::RefCell::new(None),
                running: Some(ident_a()),
            }
        }
        fn seed_bootctrl(&mut self, rec: &[u8; BOOT_CONTROL_RECORD_SIZE], copy: usize) {
            let off =
                ((BOOTCTRL_START_LBA + copy as u64 * BOOTCTRL_COPY_STRIDE_LBAS) as usize) * LBA;
            self.media[off..off + BOOT_CONTROL_RECORD_SIZE].copy_from_slice(rec);
        }
    }

    impl InstallBackend for MockFace {
        /// 70.14 复查修复：**夹具与生产同源**。旧实现硬编码
        /// `(16384, 16384, 0, 8)`——bootctrl 起始写成 0、槽起始写成 16384，
        /// 与 `boot_disk` 介质图（BootCtrl=18432 / BootA=20480 / BootB=36864，
        /// 各 8 MiB 槽）不符：夹具里的 `write_allowed` 判据与生产**不是同一
        /// 个判据**，安装写面的红测因此没有判别力（trial 目标槽改由
        /// `install_ranges` 单源导出后立即暴露）。
        fn install_ranges(&self) -> crate::boot_guard::InstallRanges {
            if let Some(cached) = *self.guard.borrow() {
                return cached;
            }
            let (bc_lba, _) = eneros_ota_ab::boot_disk::lba_range_of(
                eneros_ota_ab::boot_disk::BootPartition::BootCtrl,
            );
            let rec0 = bc_lba as usize * LBA;
            let rec1 = (bc_lba + BOOTCTRL_COPY_STRIDE_LBAS) as usize * LBA;
            let guard = crate::boot_guard::BootWriteGuard::resolve_with_running(
                &self.media[rec0..rec0 + BOOT_CONTROL_RECORD_SIZE],
                &self.media[rec1..rec1 + BOOT_CONTROL_RECORD_SIZE],
                self.running,
            )
            // 双副本均无效 → 非活动区间置空（`write_allowed` 恒拒 =
            // 生产 fail-closed 语义）。
            .unwrap_or(crate::boot_guard::BootWriteGuard {
                inactive_start_lba: 0,
                inactive_lbas: 0,
                active_is_a: false,
                active_security_version: 0,
                sequence: 0,
                running: None,
            });
            let out = guard.install_ranges();
            // 首次调用 = 本 boot 的 init；其后冻结（与生产一致）。
            *self.guard.borrow_mut() = Some(out);
            out
        }

        /// boot 面读取（512B 扇区 LBA 单位——内存介质直寻址；换算单源 =
        /// `boot_guard::sector_byte_offset`）。
        fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
            let off = crate::boot_guard::sector_byte_offset(lba).ok_or(())? as usize;
            let end = off + buf.len();
            if end > self.media.len() {
                return Err(());
            }
            buf.copy_from_slice(&self.media[off..end]);
            Ok(())
        }

        /// boot 面写入（512B 扇区 LBA 单位）。
        fn write_sectors(&mut self, lba: u64, data: &[u8]) -> Result<(), ()> {
            let off = crate::boot_guard::sector_byte_offset(lba).ok_or(())? as usize;
            let end = off + data.len();
            if end > self.media.len() {
                return Err(());
            }
            self.media[off..end].copy_from_slice(data);
            self.writes += 1;
            self.log
                .borrow_mut()
                .push((lba, alloc::vec::Vec::from(data)));
            Ok(())
        }

        /// mock flush：内存介质无写缓存，事务持久化承诺天然成立；
        /// `flush_fail` latch 例外（WP-76.3.2 变异咬合面）。
        fn flush(&mut self) -> Result<(), ()> {
            if self.flush_fail {
                return Err(());
            }
            Ok(())
        }

        /// F70-02：mock 的运行身份 = 构造时给定的身份页事实。
        fn running_identity(&self) -> Option<crate::boot_guard::RunningIdentity> {
            self.running
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
        // F70-02：确认对象 = **运行槽**（身份 A）。trial 先装 B（下一引导），
        // 本次仍运行 A——确认 A（运行载荷身份）→ A successful、floor 推进。
        let mut face = MockFace::new();
        face.seed_bootctrl(&factory_stable(), 0);
        face.seed_bootctrl(&factory_stable(), 1);
        // 运行载荷身份 = 本次镜像（secver=2、release=[3;16] 的 A 载荷）。
        face.running = Some(crate::boot_guard::RunningIdentity {
            is_a: true,
            security_version: 2,
            release_id: [3u8; 16],
        });
        let mut inst = BootInstall::new(&mut face);
        let seq0 = inst.write_bootctrl_trial(2, [3u8; 16]).expect("trial");
        let seq1 = inst.confirm_running(true, 2, [3u8; 16]).expect("confirm");
        assert_eq!(seq1, seq0 + 1);
        let (c, _) = inst.read_current().expect("read");
        assert!(c.slot_a.successful, "确认后 A = successful（运行槽）");
        assert_eq!(c.slot_a.security_version, 2);
        assert_eq!(c.security_floor, 2, "确认推进 floor（不可逆提交）");
        // trial 目标 B 保持 trial（下一引导 B）。
        assert_eq!(c.slot_b.tries_remaining, 3);
    }

    /// F70-02（S1 确认半边，红转绿）：A 最后一次试启（prepare_attempt 已把
    /// tries 1→0 持久、本次仍运行 A），盘上记录 `select()` 已指向 B。
    /// 旧实现按 `select()` 比对 → 运行槽 A 的健康确认被
    /// `confirm-slot-not-selected` 拒绝；现按运行身份核对必须**成功**，
    /// 且 successful 落盘、可重启恢复（下次选择仍按新记录收敛）。
    #[test]
    fn confirm_succeeds_on_last_trial_of_running_slot() {
        let id = [9u8; 16];
        let mut bc = BootControl::factory(id, 1, id);
        bc.slot_a = eneros_boot_protocol::SlotState::trial(15, 1, 1, id);
        bc.slot_b = eneros_boot_protocol::SlotState::successful(14, 1, id);
        let (booting, persisted) = bc.prepare_attempt().unwrap();
        assert_eq!(booting.slot, BootSlot::A);
        let rec = persisted.unwrap().encode().unwrap();
        let mut face = MockFace::new();
        face.seed_bootctrl(&rec, 0);
        face.seed_bootctrl(&rec, 1);
        let mut inst = BootInstall::new(&mut face);
        let seq = inst
            .confirm_running(true, 1, [9u8; 16])
            .expect("最后一次试启的运行槽确认必须成功");
        assert!(seq >= 2);
        let (c, _) = inst.read_current().expect("read");
        assert!(c.slot_a.successful, "A = successful（不再耗 tries）");
        assert_eq!(c.slot_a.tries_remaining, 0);
        assert_eq!(c.security_floor, 1);
        // 重启后选择：A successful 15 > B 14 → 仍 A（不误回退）。
        let sel = c.select().expect("select");
        assert_eq!(sel.slot, BootSlot::A);
        assert!(!sel.trial);
    }

    /// F70-02 B03：同一 boot 内记录前移（连续 trial / 下一选择变化）不改变
    /// 本次运行身份——确认与第二次 trial 的目标都不漂移。
    #[test]
    fn running_identity_does_not_drift_with_record_changes() {
        let mut face = MockFace::new();
        face.seed_bootctrl(&factory_stable(), 0);
        face.seed_bootctrl(&factory_stable(), 1);
        let mut inst = BootInstall::new(&mut face);
        inst.write_bootctrl_trial(2, [3u8; 16])
            .expect("第一次 trial → B");
        // 记录已前移（select() 现指 B trial）——运行身份仍是 A：
        // 确认 A 照常成功；第二次 trial 仍落 B（运行槽对侧）。
        inst.confirm_running(true, 1, [9u8; 16])
            .expect("确认运行槽 A");
        inst.write_bootctrl_trial(3, [4u8; 16])
            .expect("第二次 trial");
        let (c, _) = inst.read_current().expect("read");
        assert_eq!(
            c.slot_b.security_version, 3,
            "第二次 trial 仍落 B（运行槽对侧，不随 select() 漂移）"
        );
        assert!(c.slot_a.successful, "运行槽 A 已确认");
    }

    /// F70-02 B04：错槽 / 错载荷身份 / 错安全版本 / 虚增水位的确认一律
    /// 拒绝，且拒绝面零副作用（记录逐字节不变、floor 不动、写计数为 0）。
    #[test]
    fn confirm_rejects_wrong_slot_identity_or_secver_with_zero_side_effects() {
        // 错槽：运行 A，确认 B。
        let mut face = MockFace::new();
        face.seed_bootctrl(&factory_stable(), 0);
        face.seed_bootctrl(&factory_stable(), 1);
        let writes0 = face.writes;
        let before;
        {
            let mut inst = BootInstall::new(&mut face);
            before = inst.read_current().expect("read").0.encode().unwrap();
            // 错槽：运行 A，确认 B。
            assert_eq!(
                inst.confirm_running(false, 1, [9u8; 16]),
                Err(BootReject::ConfirmSlotNotRunning)
            );
            // 错安全版本（虚增水位：capsule secver=1，请求给 9）。
            assert_eq!(
                inst.confirm_running(true, 9, [9u8; 16]),
                Err(BootReject::ConfirmSecverMismatch)
            );
            // 错载荷身份（release_id 不符）。
            assert_eq!(
                inst.confirm_running(true, 1, [3u8; 16]),
                Err(BootReject::ConfirmImageMismatch)
            );
            let (c0, _) = inst.read_current().expect("read");
            assert_eq!(c0.security_floor, 1, "floor 不得被推高");
            assert_eq!(
                c0.encode().unwrap(),
                before,
                "拒绝面必须零写入（记录逐字节不变）"
            );
        }
        assert_eq!(face.writes, writes0, "拒绝面零介质写");
    }

    /// F70-02 B05：运行身份缺失（capsule 缺失/无效/Recovery）⇒ 确认与
    /// trial 都拒绝，写计数为 0（写面已由空区间关闭；cancel/状态诊断
    /// 保留——受控恢复通道）。
    #[test]
    fn missing_identity_rejects_confirm_and_trial() {
        let mut face = MockFace::new();
        face.running = None;
        face.seed_bootctrl(&factory_stable(), 0);
        face.seed_bootctrl(&factory_stable(), 1);
        let writes0 = face.writes;
        {
            let mut inst = BootInstall::new(&mut face);
            assert_eq!(
                inst.confirm_running(true, 1, [9u8; 16]),
                Err(BootReject::NoRunningIdentity)
            );
            assert_eq!(
                inst.write_bootctrl_trial(2, [3u8; 16]),
                Err(BootReject::InstallRangeNotASlot)
            );
            // 诊断/受控恢复保留：STATUS 可读。
            assert!(inst.read_current().is_ok());
        }
        assert_eq!(face.writes, writes0, "身份缺失 ⇒ 零介质写");
    }

    /// 70.14 复查红测（F70-02 基线重订）：**confirm 只接受本次实际运行的
    /// 槽**（运行身份），不是 wire 自述、也不是 `select()` 的下一选择。
    ///
    /// 缺陷形态（旧实现）：直接采信 wire 上的 `slot_is_a`——发一条与运行槽
    /// 相反的消息即把**非运行槽**置 successful 并把 `security_floor` 推到
    /// 任意 secver（`successful(priority.max(1), …)` 连 disabled（priority=0）
    /// 槽都会被提成 1）；下次引导选择被翻转、不可逆 floor 被单条消息推高。
    /// F70-02 后基线 = 可信运行身份：等值核对（槽/secver/release）全过才写。
    #[test]
    fn confirm_rejects_slot_that_is_not_running() {
        let mut face = MockFace::new();
        face.seed_bootctrl(&factory_stable(), 0);
        face.seed_bootctrl(&factory_stable(), 1);
        let mut inst = BootInstall::new(&mut face);
        // 稳定种子：运行身份 = A（guard 冻结）→ 确认 B 必须拒绝。
        let before = inst.read_current().expect("read").0.encode().unwrap();
        let err = inst
            .confirm_running(false, 1, [9u8; 16])
            .expect_err("confirm");
        assert_eq!(err, BootReject::ConfirmSlotNotRunning);
        let (c0, _) = inst.read_current().expect("read");
        assert_eq!(c0.security_floor, 1, "拒绝面零副作用（floor 不得被推高）");
        assert_eq!(
            c0.encode().unwrap(),
            before,
            "拒绝面必须零写入（记录逐字节不变）"
        );
        // 正例仍成立：运行槽 A 的正确确认成功。
        inst.confirm_running(true, 1, [9u8; 16])
            .expect("confirm 运行槽");
    }

    /// 70.14 复查红测：**trial 目标槽来自安装守卫（`install_ranges`）**，
    /// 而不是重新用 `select()` 推。
    ///
    /// 差别只在"同一 boot 内记录已变"时显现：守卫在 init 期解析一次（非活动
    /// = 本 boot 未在运行的槽），而 `select()` 是"下一引导槽"。载荷安装目标
    /// 与 0x0F 候选身份读面都按守卫口径取分区——若 trial 记录另按 `select()`
    /// 推，第二次 trial 会把记录指向**正在运行**的槽：记录身份 ≠ 载荷身份、
    /// 安装写打到运行槽。
    ///
    /// 本锁构造：先用"select()=A"的记录冻结守卫（非活动 = B），再把记录换成
    /// "select()=B"（模拟 boot 内记录已前移）→ trial 必须仍落 B（守卫口径），
    /// 而不是按 select() 的对侧落 A。
    #[test]
    fn trial_target_comes_from_the_install_guard_not_select() {
        let mut face = MockFace::new();
        let frozen = factory_stable(); // A=15 successful → select()=A ⇒ 守卫非活动 = B
        face.seed_bootctrl(&frozen, 0);
        face.seed_bootctrl(&frozen, 1);
        let inst = BootInstall::new(&mut face);
        // 冻结守卫（生产 = init 期）。
        let inactive_start = inst.face.install_ranges().inactive_start_lba;
        let (b_start, _) =
            eneros_ota_ab::boot_disk::lba_range_of(eneros_ota_ab::boot_disk::BootPartition::BootB);
        assert_eq!(inactive_start, b_start, "守卫非活动槽 = BOOT_B");
        // 释放 `inst` 对 face 的独占借用（守卫已缓存进 face）。
        let _ = inst;
        // 记录前移：改为 select() = B（B=15 successful > A=14）。
        let moved = factory_b_active();
        face.seed_bootctrl(&moved, 0);
        face.seed_bootctrl(&moved, 1);
        let mut inst = BootInstall::new(&mut face);
        let seq = inst.write_bootctrl_trial(2, [3u8; 16]).expect("trial");
        assert_eq!(seq, 2);
        let (c, _) = inst.read_current().expect("read");
        assert_eq!(
            c.slot_b.security_version, 2,
            "trial 必须落在守卫口径的非活动槽（B）——按 select() 推会落到 A（运行槽）"
        );
        assert_eq!(c.slot_b.tries_remaining, 3);
        assert_eq!(c.slot_a.priority, 13, "非目标槽（A=14）降 1 保留回退资格");
    }

    /// 70.7 复查回归锁（F70-02 基线重订）：trial 目标槽 = **运行槽的
    /// 对侧**（守卫口径），且新记录 `select()` 恰好指向 trial 槽（试启动
    /// 语义成立）。
    ///
    /// 缺陷形态（旧实现）：按 `slot_a.priority >= slot_b.priority` 自判
    /// "活跃槽"——平局/B 活跃等形态下与 `select()` 相反 → trial 记录落在
    /// 正在运行的槽上（试启动自我失效）。F70-02 后判据单源 = 运行身份
    /// （`select()` 只回答"下一引导"，A 最后一次试启后两口径分叉）。
    #[test]
    fn trial_targets_opposite_of_running_slot_across_seed_matrix() {
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
            inst.write_bootctrl_trial(2, [3u8; 16]).expect("trial");
            let after = inst.read_current().expect("read").0;
            let sel = after.select().expect("select");
            // 种子全是可引导态；落到 Recovery 即种子本身失效。
            assert_ne!(
                sel.slot,
                BootSlot::Recovery,
                "{name}: 种子不得落在 Recovery"
            );
            assert_eq!(
                sel.slot,
                BootSlot::B,
                "{name}: trial 必须落在运行槽（A）的对侧 B——与种子形态无关"
            );
            assert!(sel.trial, "{name}: 对新记录必须是试启动态");
            // 回退资格：原运行槽优先级保留 ≥1（tries 耗尽可回退）。
            assert!(after.slot_a.priority >= 1, "{name}: 运行槽保留回退资格");
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
        assert_eq!(inst.cancel_trial(), Err(BootReject::NoPendingTrial));
    }

    #[test]
    fn cancel_trial_rejects_confirmed_state() {
        // F70-02 基线：运行 B（身份 B）+ B 已确认 successful（无 pending
        // trial）→ 取消必须拒绝（floor 已推进，回退 = 新更新事务）。
        let mut face = MockFace::new();
        face.running = Some(crate::boot_guard::RunningIdentity {
            is_a: false,
            security_version: 2,
            release_id: [9u8; 16],
        });
        face.seed_bootctrl(&factory_b_active(), 0);
        face.seed_bootctrl(&factory_b_active(), 1);
        let mut inst = BootInstall::new(&mut face);
        inst.confirm_running(false, 2, [9u8; 16])
            .expect("confirm B");
        assert_eq!(inst.cancel_trial(), Err(BootReject::NoPendingTrial));
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
            Err(BootReject::StatusOverflow)
        );
    }

    // ==================== WP-76.3.1 · 写序对偶实证（体检 R17 候选 3） ====================

    /// 掉电推演种子对（本卷立卷时的一次性探针永久化）。
    ///
    /// 形态 = `copy0 = R1`（seq=1，**与受保护锚精确匹配**）+
    /// `copy1 = R2`（seq=2，A 降 1 / B = pending trial）——即「copy1 序列
    /// 高于 copy0」的运行期形态。§1A.4 可达性裁决：当前系统不可达（固件
    /// 事务在移交内核前必完成、运行期中断产生的高副本形态恒为 copy0 侧），
    /// 故本测是**防御性不变量锁**而非现行故障复现；任何未来路径造出该形态
    /// （固件写序调整、新增 copy1-first 写点、copy0 损败形态变化）时，
    /// 本锁决定行为。
    ///
    /// 返回 `(copy0 字节, copy1 字节, 由 copy0 精确导出的锚)`。
    fn anchor_split_seed() -> (
        [u8; BOOT_CONTROL_RECORD_SIZE],
        [u8; BOOT_CONTROL_RECORD_SIZE],
        eneros_boot_protocol::BootControlAnchor,
    ) {
        let id = [9u8; 16];
        let mut r1 = BootControl::factory(id, 1, id);
        r1.slot_a = eneros_boot_protocol::SlotState::successful(15, 1, id);
        r1.slot_b = eneros_boot_protocol::SlotState::successful(14, 1, id);
        r1.sequence = 1;
        let r1_bytes = r1.encode().expect("encode R1");
        let anchor =
            eneros_boot_protocol::BootControlAnchor::new(&r1, &r1_bytes).expect("anchor from R1");
        let mut r2 = r1;
        r2.slot_a.priority = 14;
        r2.slot_b = eneros_boot_protocol::SlotState::trial(15, 3, 2, [3u8; 16]);
        r2.sequence = 2;
        let r2_bytes = r2.encode().expect("encode R2");
        (r1_bytes, r2_bytes, anchor)
    }

    /// 三事务统一入口（写序对偶锁按名驱动，避免夹具三份手写）。
    fn run_transaction(name: &str, inst: &mut BootInstall<'_>) -> Result<u64, BootReject> {
        match name {
            "trial" => inst.write_bootctrl_trial(2, [3u8; 16]),
            "confirm" => inst.confirm_running(true, 1, [9u8; 16]),
            _ => inst.cancel_trial(),
        }
    }

    /// 掉电重放：以种子介质为起点，只重放事务的**前 `n` 笔写**（= 第
    /// `n+1` 笔之前掉电），返回重放后的双副本字节形。
    fn replay_writes(
        log: &[(u64, alloc::vec::Vec<u8>)],
        n: usize,
        seed0: &[u8; BOOT_CONTROL_RECORD_SIZE],
        seed1: &[u8; BOOT_CONTROL_RECORD_SIZE],
    ) -> (
        [u8; BOOT_CONTROL_RECORD_SIZE],
        [u8; BOOT_CONTROL_RECORD_SIZE],
    ) {
        let mut face = MockFace::new();
        face.seed_bootctrl(seed0, 0);
        face.seed_bootctrl(seed1, 1);
        for (lba, data) in log.iter().take(n) {
            face.write_sectors(*lba, data).expect("replay write");
        }
        let mut first = [0u8; BOOT_CONTROL_RECORD_SIZE];
        let mut second = [0u8; BOOT_CONTROL_RECORD_SIZE];
        face.read_sectors(BOOTCTRL_START_LBA, &mut first)
            .expect("replay read copy0");
        face.read_sectors(BOOTCTRL_START_LBA + BOOTCTRL_COPY_STRIDE_LBAS, &mut second)
            .expect("replay read copy1");
        (first, second)
    }

    /// 单臂写序检查。**返回违规描述而非 panic**——三臂必须各自跑完再汇总，
    /// 否则第一臂的红会短路遮蔽后两臂的形态（收编前实测即如此：`for` +
    /// `assert` 停在 confirm，cancel 臂从未被观察到）。
    fn write_order_violation(
        name: &str,
        r1: &[u8; BC_RECORD],
        r2: &[u8; BC_RECORD],
    ) -> Option<alloc::string::String> {
        let mut face = MockFace::new();
        face.seed_bootctrl(r1, 0);
        face.seed_bootctrl(r2, 1);
        {
            let mut inst = BootInstall::new(&mut face);
            run_transaction(name, &mut inst).expect(name);
        }
        let log = core::mem::take(&mut *face.log.borrow_mut());
        let hi = BOOTCTRL_START_LBA + BOOTCTRL_COPY_STRIDE_LBAS;
        if log.len() != 2 {
            return Some(alloc::format!(
                "{name}: 期望双副本恰两笔写，实得 {}",
                log.len()
            ));
        }
        if log[0].0 != hi || log[1].0 != BOOTCTRL_START_LBA {
            return Some(alloc::format!(
                "{name}: 写序违规 首写 lba={} 次写 lba={}（不变量：首写落高序列副本 {hi}）",
                log[0].0,
                log[1].0
            ));
        }
        None
    }

    /// WP-76.3.1 红测①（**现状：trial 臂绿、confirm/cancel 臂红**）：
    /// 三事务在「copy1 序列更高」形态下必须一律**高序列副本先写**。
    ///
    /// 不变量定义处 = 固件侧 `resolve_boot_control` 的采纳规则与其注释断言
    /// （「运行时写序：高序列副本 → 低序列副本 → flush」）。trial 满足；
    /// confirm/cancel 手写恒 copy0→copy1（`read_current` 的写序输入被
    /// `_` 丢弃）= 违反文档化不变量的潜伏防御缺口。红的是缺陷实证，
    /// WP-76.3.2（commit 单源）转绿——**禁止为合入而注释掉红臂**。
    #[test]
    fn confirm_and_cancel_write_high_seq_copy_first() {
        let (r1, r2, _) = anchor_split_seed();
        let mut violations: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
        for name in ["trial", "confirm", "cancel"] {
            violations.extend(write_order_violation(name, &r1, &r2));
        }
        assert!(
            violations.is_empty(),
            "高序列副本先写不变量被违反的臂：{violations:?}"
        );
    }

    /// 单臂掉电重放检查（同样返回描述而非 panic，三臂全跑）。
    fn power_loss_violation(
        name: &str,
        r1: &[u8; BC_RECORD],
        r2: &[u8; BC_RECORD],
        anchor: &eneros_boot_protocol::BootControlAnchor,
    ) -> Option<alloc::string::String> {
        let mut face = MockFace::new();
        face.seed_bootctrl(r1, 0);
        face.seed_bootctrl(r2, 1);
        {
            let mut inst = BootInstall::new(&mut face);
            run_transaction(name, &mut inst).expect(name);
        }
        let log = core::mem::take(&mut *face.log.borrow_mut());
        // 只重放首写 = 第二笔写之前掉电。
        let (first, second) = replay_writes(&log, 1, r1, r2);
        let resolved = eneros_boot_protocol::resolve_boot_control(&first, &second, Some(anchor));
        let resolved = match resolved {
            Ok(r) => r,
            Err(e) => {
                return Some(alloc::format!(
                    "{name}: 首写后掉电 ⇒ 固件侧解析失败 {e:?}（引导失败）"
                ))
            }
        };
        if resolved.control.sequence != 1 {
            return Some(alloc::format!(
                "{name}: 掉电未回退锚定旧记录（seq={} 期望 1）",
                resolved.control.sequence
            ));
        }
        if resolved.source != eneros_boot_protocol::BootControlCopy::First {
            return Some(alloc::format!("{name}: 锚匹配副本应为 copy0"));
        }
        if resolved.needs_anchor_initialization {
            return Some(alloc::format!("{name}: 精确匹配半边不得重锚"));
        }
        None
    }

    /// WP-76.3.1 红测②（与①对偶的**介质后果**）：两写之间掉电后，固件侧
    /// 真实 `resolve_boot_control` 必须仍精确匹配锚并回退旧记录（安全回旧态），
    /// 不得落 `AnchorMismatch`（= 引导失败，boot-manager-uefi 的
    /// `BootFailure::BootControl`）。
    #[test]
    fn power_loss_between_writes_preserves_anchor_match() {
        let (r1, r2, anchor) = anchor_split_seed();
        let mut violations: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
        for name in ["trial", "confirm", "cancel"] {
            violations.extend(power_loss_violation(name, &r1, &r2, &anchor));
        }
        assert!(
            violations.is_empty(),
            "掉电窗口丢失锚定副本的臂：{violations:?}"
        );
    }

    /// 独立复查轮坐实（2026-09-27，登记 REG-76-M）：**单侧可解码**形态下的
    /// 写序半边——前两测只覆盖「双副本均可解码」（平局/异序），本测补另一轴。
    ///
    /// 把「较高序列副本先写」直译成通用判据，在 {copy0 不可解码, copy1 有效}
    /// （及镜像）下会指向**唯一幸存副本**：先覆它，则任一中断点全盲——双副本
    /// 皆不可解（固件 `bootctrl-both-invalid`）或锚不再匹配（`AnchorMismatch`），
    /// 两条都通向引导失败。而蓝图 76 卷三 §1A.4 对同一形态的判定恰是「首写落
    /// 无效副本、copy1=锚匹配幸存，**安全**」⇒ 采该判定：单可解 ⇒ 先写损败侧
    /// （判据全文见 [`read_current`] 三条）。该形态真实可达：上一次事务的次写
    /// 被撕（torn write）即留下「一侧可解 / 一侧不可解」。
    #[test]
    fn commit_writes_undecodable_copy_first_when_one_side_is_sole_survivor() {
        let mut violations: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
        for good_copy in [0usize, 1usize] {
            let (survivor, anchor) = survivor_seed();
            let zeros = [0u8; BC_RECORD];
            let (seed0, seed1) = if good_copy == 0 {
                (&survivor, &zeros)
            } else {
                (&zeros, &survivor)
            };
            // 损败侧基址：好侧在 copy0 → 坏的是 copy1（+STRIDE），反之亦然。
            let broken_lba = BOOTCTRL_START_LBA
                + if good_copy == 0 {
                    BOOTCTRL_COPY_STRIDE_LBAS
                } else {
                    0
                };
            for name in ["trial", "confirm", "cancel"] {
                let mut face = MockFace::new();
                face.seed_bootctrl(seed0, 0);
                face.seed_bootctrl(seed1, 1);
                {
                    let mut inst = BootInstall::new(&mut face);
                    if let Err(e) = run_transaction(name, &mut inst) {
                        violations.push(alloc::format!("{name}/copy{good_copy}: 事务被拒 {:?}", e));
                        continue;
                    }
                }
                let log = core::mem::take(&mut *face.log.borrow_mut());
                if log.first().map(|w| w.0) != Some(broken_lba) {
                    violations.push(alloc::format!(
                        "{name}/copy{good_copy}: 首写落点 {:?} 应为损败侧 {broken_lba}（先覆唯一幸存副本 = 全盲）",
                        log.first().map(|w| w.0)
                    ));
                }
                let (first, second) = replay_writes(&log, 1, seed0, seed1);
                match eneros_boot_protocol::resolve_boot_control(&first, &second, Some(&anchor)) {
                    Ok(res) if res.control.sequence == 1 && !res.needs_anchor_initialization => {}
                    Ok(res) => violations.push(alloc::format!(
                        "{name}/copy{good_copy}: 掉电后未回退幸存记录（seq={} 重锚={:?}）",
                        res.control.sequence,
                        res.needs_anchor_initialization
                    )),
                    Err(e) => violations.push(alloc::format!(
                        "{name}/copy{good_copy}: 首写后掉电 ⇒ 固件侧解析失败 {e:?}（引导失败）"
                    )),
                }
            }
        }
        assert!(
            violations.is_empty(),
            "单侧幸存形态写序违规的臂：{violations:?}"
        );
    }

    /// 单侧幸存种子：仅一条记录（seq=1，A successful=运行身份、B pending
    /// trial ⇒ 三事务在此形态下都可跑），锚由该记录精确导出。
    fn survivor_seed() -> (
        [u8; BOOT_CONTROL_RECORD_SIZE],
        eneros_boot_protocol::BootControlAnchor,
    ) {
        let id = [9u8; 16];
        let mut c = BootControl::factory(id, 1, id);
        c.slot_a = eneros_boot_protocol::SlotState::successful(15, 1, id);
        c.slot_b = eneros_boot_protocol::SlotState::trial(15, 3, 2, [3u8; 16]);
        c.sequence = 1;
        let bytes = c.encode().expect("encode survivor");
        let anchor = eneros_boot_protocol::BootControlAnchor::new(&c, &bytes).expect("anchor");
        (bytes, anchor)
    }

    /// WP-76.3.2 变异咬合：`commit` 两笔写之后必须落 flush。恒 Ok 的 mock
    /// flush 咬不住「删掉 flush」这一刀，故给 `MockFace` 加 `flush_fail`
    /// latch——三事务一律以 `Flush` 失败上抛（不得静默成功、不得吞成
    /// 其它词形），且两笔介质写已发生（写序半边与 flush 半边各自可见）。
    #[test]
    fn commit_propagates_flush_failure() {
        for name in ["trial", "confirm", "cancel"] {
            let (r1, r2, _) = anchor_split_seed();
            let mut face = MockFace::new();
            face.seed_bootctrl(&r1, 0);
            face.seed_bootctrl(&r2, 1);
            face.flush_fail = true;
            {
                let mut inst = BootInstall::new(&mut face);
                assert_eq!(
                    run_transaction(name, &mut inst),
                    Err(BootReject::Flush),
                    "{name}: flush 失败必须以 Flush 上抛"
                );
            }
            let log = core::mem::take(&mut *face.log.borrow_mut());
            assert_eq!(log.len(), 2, "{name}: 双副本两笔写先于 flush");
        }
    }

    /// WP-77.2.3 词形逐字节 golden：14 个 [`BootReject`] 词与收编前
    /// `&'static str` 全集一致——wire（ST_INVALID + payload）由本表冻结，
    /// `from_word` 是 `as_word` 的完整逆（含未知词/截断词/带尾随字节的
    /// 词形一律 None——客户端 `Rejected` 分类只接受完整已知词）。
    #[test]
    fn boot_reject_words_are_byte_stable_golden() {
        const WORDS: [(&str, BootReject); 14] = [
            ("read0", BootReject::Read0),
            ("read1", BootReject::Read1),
            ("bootctrl-both-invalid", BootReject::BootctrlBothInvalid),
            ("encode", BootReject::Encode),
            ("write0", BootReject::Write0),
            ("write1", BootReject::Write1),
            ("flush", BootReject::Flush),
            ("install-range-not-a-slot", BootReject::InstallRangeNotASlot),
            ("no-running-identity", BootReject::NoRunningIdentity),
            (
                "confirm-slot-not-running",
                BootReject::ConfirmSlotNotRunning,
            ),
            ("confirm-secver-mismatch", BootReject::ConfirmSecverMismatch),
            ("confirm-image-mismatch", BootReject::ConfirmImageMismatch),
            ("no-pending-trial", BootReject::NoPendingTrial),
            ("status-overflow", BootReject::StatusOverflow),
        ];
        for (word, reject) in WORDS {
            assert_eq!(reject.as_word(), word, "{word}: as_word 漂移");
            assert_eq!(
                BootReject::from_word(word.as_bytes()),
                Some(reject),
                "{word}: from_word 必须还原枚举"
            );
            // 截断/带尾随字节 = 非完整已知词形，一律拒（客户端不得把
            // 「脏 payload」升格成具名拒绝）。
            assert_eq!(
                BootReject::from_word(&word.as_bytes()[..word.len() - 1]),
                None,
                "{word}: 截断词必须拒"
            );
            let mut trailing = alloc::vec::Vec::from(word.as_bytes());
            trailing.push(b' ');
            assert_eq!(
                BootReject::from_word(&trailing),
                None,
                "{word}: 尾随字节必须拒"
            );
        }
        // 未知词 + 空词 + 非 UTF-8 词形一律 None。
        assert_eq!(BootReject::from_word(b"no-such-word"), None);
        assert_eq!(BootReject::from_word(b""), None);
        assert_eq!(BootReject::from_word(&[0xff, 0xfe]), None);
    }
}
