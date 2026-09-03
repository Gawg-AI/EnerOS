//! EnerOS Recovery Mode 最小化恢复内核 (workspace v0.192.0) — Phase 2 P2-K OTA 与 Recovery 第 3 版。
//!
//! 在主系统损坏无法启动时经串口/网络进入恢复模式，支持固件重刷（SM2 签名验证）、
//! 配置恢复出厂、系统诊断，提供"最后一道防线"避免现场设备变砖，为 v0.126.0 VPP
//! 出口提供系统可恢复性保障。
//!
//! # 交付物
//! - [`RecoveryCommand`] / [`RecoveryState`] / [`DiagResult`] / [`FlashProgress`] /
//!   [`RecoveryError`] — 恢复命令与状态数据模型
//! - [`parse_command`] / [`menu_text`] — 命令行解析与恢复菜单文本
//! - [`RecoveryMode`] — Recovery Mode 交互 trait（enter/execute/get_progress/get_diag/exit）
//! - [`RecoveryModeImpl`] — 组装 flash/diag/factory_reset 三模块的完整实现（Task 6）
//! - [`flash`] — FlashWriter 固件刷写（Task 3 实现）
//! - [`factory_reset`] — FactoryReset 恢复出厂/备份/恢复（Task 4 实现）
//! - [`diag`] — Diagnostician 系统诊断（Task 5 实现）
//!
//! # no_std 合规
//! `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`；非测试代码不使用
//! `std::*` / `panic!` / `todo!` / `unimplemented!` / `unwrap()` / `println!`；
//! 菜单与执行结果以 `alloc::string::String` 返回，输出通道由集成方决定。
//!
//! # 蓝图偏差声明（对应 spec D1~D7）
//! - **D1**：`println!` 直出菜单 → 菜单/结果以字符串返回（no_std 无 stdout；
//!   输出通道由集成方串口/网络决定）
//! - **D2**：execute 返回类型 `String`/`format!` → alloc::string::String /
//!   alloc::format!（no_std）
//! - **D3**：`receive_image(image_path)` 从文件路径读 → 流式 receive_chunk 字节
//!   接收（恢复环境无文件系统假设）；刷写目标为后端 BlockDevice（eneros-storage，
//!   与 v0.123.0 同口径）
//! - **D4**：`verify_signature` 简化 → 真实 SM3 增量哈希 + SM2 根公钥验签
//!   （eneros-crypto），构造注入设备根公钥
//! - **D5**：ListPartitions 硬编码 `"A: Active v0.124.0\nB: Failed v0.125.0"` →
//!   真实查询 eneros-ota-ab 分区表状态（v0.123.0 兼容回归）
//! - **D6**：crate 落位 `crates/recovery/` → `crates/security/recovery/`，
//!   包名 eneros-recovery
//! - **D7**：Diagnostician 硬件字段（temperature/network_ok/storage_ok）经探针
//!   注入而非直读硬件（可测试；默认探针恒 ok）
//! - **D8**：Flash 刷写参数化——`execute(Flash)` 不负责接收镜像：镜像字节由
//!   集成方经 `flash_writer_mut().receive_chunk` 流式喂入，`execute` 仅携带
//!   `[expected_hash_hex, signature_hex, slot_base]` 三参数完成验签 + 写盘
//!   （串口/网络接收循环属集成方职责，crate 不持有传输通道）

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod diag;
pub mod factory_reset;
pub mod flash;

/// 恢复命令：Recovery Mode 支持的七条交互命令。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryCommand {
    /// 刷写固件
    Flash,
    /// 恢复出厂配置
    FactoryReset,
    /// 系统诊断
    Diagnose,
    /// 列出分区
    ListPartitions,
    /// 备份配置
    Backup,
    /// 恢复配置
    Restore,
    /// 重启
    Reboot,
}

/// 恢复模式状态机六态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryState {
    /// 等待命令输入
    Waiting,
    /// 固件刷写中
    Flashing,
    /// 恢复出厂中
    Resetting,
    /// 系统诊断中
    Diagnosing,
    /// 操作完成
    Complete,
    /// 操作出错
    Error,
}

/// 系统诊断结果。
#[derive(Debug, Clone)]
pub struct DiagResult {
    /// 分区 A 健康状态
    pub partition_a_ok: bool,
    /// 分区 B 健康状态
    pub partition_b_ok: bool,
    /// 启动失败计数（双分区 boot_count 之和）
    pub boot_fail_count: u32,
    /// 最近一次错误信息（≤128 字节）
    pub last_error: heapless::String<128>,
    /// 存储健康状态
    pub storage_ok: bool,
    /// 网络健康状态
    pub network_ok: bool,
    /// 芯片温度（摄氏度）
    pub temperature: f32,
}

/// 固件刷写进度。
#[derive(Debug, Clone)]
pub struct FlashProgress {
    /// 镜像总字节数
    pub total_bytes: u64,
    /// 已刷写字节数
    pub flashed_bytes: u64,
    /// 当前刷写状态
    pub state: RecoveryState,
    /// 错误信息（无错误为 None，≤128 字节）
    pub error: Option<heapless::String<128>>,
}

/// 恢复模式错误类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryError {
    /// 刷写失败（offset 为失败位置的字节偏移）
    FlashFailed {
        /// 失败位置的字节偏移
        offset: u64,
    },
    /// 镜像签名无效
    SignatureInvalid,
    /// 存储访问错误
    StorageError,
    /// 网络错误
    NetworkError,
    /// 未知命令
    CommandUnknown,
    /// 分区损坏
    PartitionCorrupted,
    /// 配置备份失败
    ConfigBackupFailed,
    /// 操作超时
    Timeout,
}

/// 解析串口/网络输入的命令行文本。
///
/// 以空白字符切分：首个 token 为命令字面量，其余为参数。
/// 空输入或未知命令返回 `None`。
pub fn parse_command(input: &str) -> Option<(RecoveryCommand, alloc::vec::Vec<&str>)> {
    let mut parts = input.split_whitespace();
    let cmd = parts.next()?;
    let args: alloc::vec::Vec<&str> = parts.collect();
    match cmd {
        "flash" => Some((RecoveryCommand::Flash, args)),
        "reset" => Some((RecoveryCommand::FactoryReset, args)),
        "diag" => Some((RecoveryCommand::Diagnose, args)),
        "list" => Some((RecoveryCommand::ListPartitions, args)),
        "backup" => Some((RecoveryCommand::Backup, args)),
        "restore" => Some((RecoveryCommand::Restore, args)),
        "reboot" => Some((RecoveryCommand::Reboot, args)),
        _ => None,
    }
}

/// 生成恢复模式菜单文本。
///
/// 偏差 D1：no_std 环境无 stdout，菜单以字符串返回，由集成方经串口/网络输出。
pub fn menu_text() -> alloc::string::String {
    let mut s = alloc::string::String::new();
    s.push_str("=== EnerOS Recovery Mode ===\n");
    s.push_str("1. flash <image>  - 刷写固件\n");
    s.push_str("2. reset          - 恢复出厂配置\n");
    s.push_str("3. diag           - 系统诊断\n");
    s.push_str("4. list           - 列出分区\n");
    s.push_str("5. backup         - 备份配置\n");
    s.push_str("6. restore <file> - 恢复配置\n");
    s.push_str("7. reboot         - 重启\n");
    s.push_str("=============================\n");
    s
}

/// Recovery Mode 交互接口。
///
/// 偏差 D1/D2：菜单与执行结果以 `alloc::string::String` 返回，
/// 由集成方经串口/网络输出，实现内禁止 `println!`。
pub trait RecoveryMode {
    /// 进入恢复模式：状态置为 [`RecoveryState::Waiting`]。
    fn enter(&mut self) -> Result<(), RecoveryError>;

    /// 执行恢复命令，返回结果文本（由集成方输出）。
    fn execute(
        &mut self,
        cmd: RecoveryCommand,
        args: &[&str],
    ) -> Result<alloc::string::String, RecoveryError>;

    /// 查询当前刷写进度。
    fn get_progress(&self) -> &FlashProgress;

    /// 获取系统诊断结果。
    fn get_diag(&self) -> Result<DiagResult, RecoveryError>;

    /// 退出恢复模式。
    fn exit(&mut self) -> Result<(), RecoveryError>;
}

// ============================================================================
// RecoveryModeImpl：三模块组装的完整 Recovery Mode 实现（Task 6）
// ============================================================================

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

pub use diag::{DefaultProbe, Diagnostician, HealthProbe};
use eneros_crypto::Sm2PublicKey;
use eneros_storage::BlockDevice;
pub use factory_reset::{ConfigEntry, FactoryReset, CONFIG_CAPACITY};
pub use flash::FlashWriter;

/// 构造 ≤N 字节的 heapless 字符串：超长按字符边界截断（不 panic）。
fn hstr_truncate<const N: usize>(s: &str) -> heapless::String<N> {
    let mut out = heapless::String::<N>::new();
    for ch in s.chars() {
        if out.push(ch).is_err() {
            break;
        }
    }
    out
}

/// 格式化诊断结果为文本（D1：字符串返回，由集成方经串口/网络输出）。
fn format_diag(r: &DiagResult) -> String {
    format!(
        "partition A: {}\npartition B: {}\nboot_fail_count: {}\nstorage: {}\nnetwork: {}\ntemperature: {} C\nlast_error: {}",
        if r.partition_a_ok { "ok" } else { "not ok" },
        if r.partition_b_ok { "ok" } else { "not ok" },
        r.boot_fail_count,
        if r.storage_ok { "ok" } else { "not ok" },
        if r.network_ok { "ok" } else { "not ok" },
        r.temperature,
        r.last_error.as_str()
    )
}

/// Recovery Mode 完整实现：组装 [`FlashWriter`] / [`Diagnostician`] /
/// [`FactoryReset`] 三模块，实现 [`RecoveryMode`] 交互接口。
///
/// 泛型 `D` 为后端块设备（生产为 eMMC/NVMe 驱动，测试为 `MockBlockDevice`），
/// `P` 为健康探针（D7，默认 [`DefaultProbe`] 恒 ok 占位）。
///
/// 刷写协作约定（D8）：镜像字节由集成方经 [`Self::flash_writer_mut`] 流式
/// 喂入（`receive_chunk`），随后 `execute(Flash, [expected_hash_hex,
/// signature_hex, slot_base])` 一步完成 SM3/SM2 验签 + 分块写盘。
pub struct RecoveryModeImpl<D: BlockDevice, P: HealthProbe = DefaultProbe> {
    /// 恢复模式状态机
    state: RecoveryState,
    /// 刷写进度（[`RecoveryMode::get_progress`] 返回）
    progress: FlashProgress,
    /// 固件刷写器（流式接收 + SM3/SM2 验签 + 分块写盘）
    flash_writer: FlashWriter<D>,
    /// 系统诊断器（探针字段 + ota-ab 分区表真实查询，D5/D7）
    diagnostician: Diagnostician<P>,
    /// 恢复出厂 / 配置备份恢复
    factory_reset: FactoryReset,
    /// Backup 命令缓存的配置字节流（供 Restore 使用，D3：无文件系统假设）
    last_backup: Option<Vec<u8>>,
}

impl<D: BlockDevice> RecoveryModeImpl<D, DefaultProbe> {
    /// 常用构造：默认探针（恒 ok 占位，D7）。
    pub fn new(device: D, root_key: Sm2PublicKey) -> Self {
        Self::with_probe(device, root_key, DefaultProbe)
    }
}

impl<D: BlockDevice, P: HealthProbe> RecoveryModeImpl<D, P> {
    /// 完整构造：注入后端块设备、设备根公钥与自定义健康探针。
    pub fn with_probe(device: D, root_key: Sm2PublicKey, probe: P) -> Self {
        Self {
            state: RecoveryState::Waiting,
            progress: FlashProgress {
                total_bytes: 0,
                flashed_bytes: 0,
                state: RecoveryState::Waiting,
                error: None,
            },
            flash_writer: FlashWriter::new(device, root_key),
            diagnostician: Diagnostician::new(probe),
            factory_reset: FactoryReset::new(),
            last_backup: None,
        }
    }

    /// 集成方喂入镜像分块的入口（D8：`receive_chunk` 流式接收）。
    pub fn flash_writer_mut(&mut self) -> &mut FlashWriter<D> {
        &mut self.flash_writer
    }

    /// 集成方注入 ota-ab 分区表的入口（D5：`set_partition_table`）。
    pub fn diagnostician_mut(&mut self) -> &mut Diagnostician<P> {
        &mut self.diagnostician
    }

    /// 集成方操作用户配置的入口（set/get）。
    pub fn factory_reset_mut(&mut self) -> &mut FactoryReset {
        &mut self.factory_reset
    }

    /// 当前状态机状态。
    pub fn state(&self) -> RecoveryState {
        self.state
    }

    /// 记录执行错误：状态机与进度置 Error，错误描述写入 progress.error
    /// （≤128 字节按字符边界截断）。
    fn record_error(&mut self, stage: &str, err: &RecoveryError) {
        self.state = RecoveryState::Error;
        self.progress.state = RecoveryState::Error;
        self.progress.error = Some(hstr_truncate(&format!("{stage}: {err:?}")));
    }

    /// Flash 命令：验签 + 写盘（镜像字节已由集成方 `receive_chunk` 喂入，D8）。
    ///
    /// args 语义为 `[expected_hash_hex(64 字符), signature_hex(128 字符),
    /// slot_base(十进制块地址)]`；参数数量不足 / hex 非法 / slot_base 非
    /// 十进制均返回 [`RecoveryError::CommandUnknown`]（参数校验先于状态迁移）。
    fn execute_flash(&mut self, args: &[&str]) -> Result<String, RecoveryError> {
        if args.len() != 3 {
            return Err(RecoveryError::CommandUnknown);
        }
        let Some(expected_hash) = eneros_hex::decode_fixed::<32>(args[0]) else {
            return Err(RecoveryError::CommandUnknown);
        };
        let Some(signature) = eneros_hex::decode_fixed::<64>(args[1]) else {
            return Err(RecoveryError::CommandUnknown);
        };
        let Ok(slot_base) = args[2].parse::<u64>() else {
            return Err(RecoveryError::CommandUnknown);
        };
        self.state = RecoveryState::Flashing;
        self.progress.state = RecoveryState::Flashing;
        // 接收阶段由集成方驱动，入口同步已接收总字节数到进度
        self.progress.total_bytes = self.flash_writer.total_bytes();
        // 1) SM3 哈希常时比对 + SM2 根公钥验签（未通过不触碰设备）
        if let Err(e) = self
            .flash_writer
            .finish_and_verify(&expected_hash, &signature)
        {
            self.record_error("verify", &e);
            return Err(e);
        }
        // 2) 分块写盘（末块零填充）
        match self.flash_writer.write_to_device(slot_base) {
            Ok(written) => {
                self.progress.flashed_bytes = written;
                self.progress.state = RecoveryState::Complete;
                self.state = RecoveryState::Complete;
                Ok(String::from("Flash complete. Reboot to activate."))
            }
            Err(e) => {
                self.record_error("write", &e);
                Err(e)
            }
        }
    }

    /// ListPartitions 命令：真实查询注入的 ota-ab 分区表（D5）。
    fn list_partitions_text(&self) -> String {
        let Some(table) = self.diagnostician.partition_table() else {
            return String::from("partition table unavailable");
        };
        let mut out = String::new();
        for p in &table.partitions {
            out.push_str(&format!(
                "{:?}: {:?} version={} boot_count={}\n",
                p.slot,
                p.state,
                p.version.as_str(),
                p.boot_count
            ));
        }
        out
    }
}

impl<D: BlockDevice, P: HealthProbe> RecoveryMode for RecoveryModeImpl<D, P> {
    fn enter(&mut self) -> Result<(), RecoveryError> {
        self.state = RecoveryState::Waiting;
        // 进度复位：上次刷写的计数/错误不带入新会话
        self.progress = FlashProgress {
            total_bytes: 0,
            flashed_bytes: 0,
            state: RecoveryState::Waiting,
            error: None,
        };
        Ok(())
    }

    fn execute(&mut self, cmd: RecoveryCommand, args: &[&str]) -> Result<String, RecoveryError> {
        match cmd {
            RecoveryCommand::Flash => self.execute_flash(args),
            RecoveryCommand::FactoryReset => {
                self.state = RecoveryState::Resetting;
                self.factory_reset.clear_configs();
                self.factory_reset.write_defaults()?;
                self.state = RecoveryState::Complete;
                Ok(String::from("Factory reset complete."))
            }
            RecoveryCommand::Diagnose => {
                self.state = RecoveryState::Diagnosing;
                let result = self.diagnostician.run();
                self.state = RecoveryState::Waiting;
                Ok(format_diag(&result))
            }
            RecoveryCommand::ListPartitions => Ok(self.list_partitions_text()),
            RecoveryCommand::Backup => {
                let bytes = self.factory_reset.backup_configs();
                let n = bytes.len();
                self.last_backup = Some(bytes);
                Ok(format!("Backup complete. {n} bytes."))
            }
            RecoveryCommand::Restore => {
                // D3：恢复环境无文件系统假设，从 Backup 缓存还原
                let Some(backup) = self.last_backup.take() else {
                    return Err(RecoveryError::CommandUnknown);
                };
                let result = self.factory_reset.restore_configs(&backup);
                // 缓存保留：恢复失败多为流损坏，原缓存仍可供诊断/重试
                self.last_backup = Some(backup);
                result?;
                Ok(String::from("Restore complete."))
            }
            RecoveryCommand::Reboot => {
                // 实际重启由集成方执行（本 crate 不触碰复位寄存器）
                Ok(String::from("Rebooting..."))
            }
        }
    }

    fn get_progress(&self) -> &FlashProgress {
        &self.progress
    }

    fn get_diag(&self) -> Result<DiagResult, RecoveryError> {
        Ok(self.diagnostician.run())
    }

    fn exit(&mut self) -> Result<(), RecoveryError> {
        self.state = RecoveryState::Complete;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use eneros_crypto::{CsRng, Sm2KeyPair};
    use eneros_storage::MockBlockDevice;

    use super::*;

    /// CMD1：flash 带参解析。
    #[test]
    fn cmd1_parse_flash_with_arg() {
        let parsed = parse_command("flash image.bin");
        assert_eq!(parsed, Some((RecoveryCommand::Flash, vec!["image.bin"])));
    }

    /// CMD2：reset 无参解析。
    #[test]
    fn cmd2_parse_reset_no_arg() {
        let parsed = parse_command("reset");
        assert_eq!(parsed, Some((RecoveryCommand::FactoryReset, vec![])));
    }

    /// CMD3：restore 带参解析；多余空白不影响解析。
    #[test]
    fn cmd3_parse_restore_and_whitespace() {
        let parsed = parse_command("restore cfg.bak");
        assert_eq!(parsed, Some((RecoveryCommand::Restore, vec!["cfg.bak"])));

        let parsed_ws = parse_command("  diag  ");
        assert_eq!(parsed_ws, Some((RecoveryCommand::Diagnose, vec![])));
    }

    /// CMD4：未知命令与空输入返回 None。
    #[test]
    fn cmd4_parse_unknown_and_empty() {
        assert_eq!(parse_command("bogus"), None);
        assert_eq!(parse_command(""), None);
        assert_eq!(parse_command("   "), None);
    }

    /// MENU5：菜单文本含七命令关键字与标题。
    #[test]
    fn menu5_menu_text_contains_all_commands() {
        let menu = menu_text();
        assert!(menu.contains("EnerOS Recovery Mode"));
        for kw in [
            "flash", "reset", "diag", "list", "backup", "restore", "reboot",
        ] {
            assert!(menu.contains(kw), "菜单缺少命令关键字: {kw}");
        }
    }

    // ====================================================================
    // RecoveryModeImpl 组装内嵌测试（ASM1~ASM8）
    // ====================================================================

    use eneros_crypto::{sm2_sign, sm3_hash};
    use eneros_ota_ab::{Partition, PartitionSlot, PartitionState, PartitionTable};

    /// 构造测试环境：真实 SM2 密钥对 + RecoveryModeImpl（MockBlockDevice 后端）。
    fn setup(
        block_count: u64,
        block_size: usize,
    ) -> (RecoveryModeImpl<MockBlockDevice>, Sm2KeyPair, CsRng) {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败");
        let rec =
            RecoveryModeImpl::new(MockBlockDevice::new(block_count, block_size), kp.public_key);
        (rec, kp, rng)
    }

    /// 构造指定槽位/状态的分区。
    fn make_partition(
        slot: PartitionSlot,
        state: PartitionState,
        version: &str,
        boot_count: u32,
    ) -> Partition {
        Partition {
            slot,
            state,
            version: heapless::String::<32>::try_from(version).unwrap_or_default(),
            size: 0,
            hash: [0; 32],
            signature: [0; 64],
            boot_count,
            last_boot_ms: 0,
        }
    }

    /// ASM1：enter 置 Waiting 并复位进度（上次错误不带入新会话）。
    #[test]
    fn asm1_enter_resets_state_and_progress() {
        let (mut rec, _kp, _rng) = setup(64, 512);
        rec.enter().expect("enter 应成功");
        assert_eq!(rec.state(), RecoveryState::Waiting);
        let p = rec.get_progress();
        assert_eq!(p.total_bytes, 0);
        assert_eq!(p.flashed_bytes, 0);
        assert_eq!(p.state, RecoveryState::Waiting);
        assert!(p.error.is_none());
    }

    /// ASM2：Flash 参数校验——数量不足 / hex 非法 / slot_base 非十进制
    /// 均 CommandUnknown，且状态机不迁移（校验先于状态迁移）。
    #[test]
    fn asm2_flash_args_validation() {
        let (mut rec, _kp, _rng) = setup(64, 512);
        // 参数数量不足
        assert_eq!(
            rec.execute(RecoveryCommand::Flash, &[]),
            Err(RecoveryError::CommandUnknown)
        );
        assert_eq!(
            rec.execute(RecoveryCommand::Flash, &["aa", "bb"]),
            Err(RecoveryError::CommandUnknown)
        );
        // hex 长度非法（hash 应 64 字符）
        assert_eq!(
            rec.execute(RecoveryCommand::Flash, &["zz", &"0".repeat(128), "10"]),
            Err(RecoveryError::CommandUnknown)
        );
        // slot_base 非十进制
        assert_eq!(
            rec.execute(
                RecoveryCommand::Flash,
                &[&"0".repeat(64), &"0".repeat(128), "abc"]
            ),
            Err(RecoveryError::CommandUnknown)
        );
        assert_eq!(
            rec.state(),
            RecoveryState::Waiting,
            "参数校验失败不应迁移状态"
        );
    }

    /// ASM3：FactoryReset 命令——配置恢复出厂默认集，状态 Complete。
    #[test]
    fn asm3_factory_reset_command() {
        let (mut rec, _kp, _rng) = setup(64, 512);
        rec.factory_reset_mut().set("site_id", "A01");
        let out = rec
            .execute(RecoveryCommand::FactoryReset, &[])
            .expect("恢复出厂应成功");
        assert!(out.contains("Factory reset"));
        assert_eq!(rec.state(), RecoveryState::Complete);
        assert_eq!(
            rec.factory_reset_mut()
                .get("device_name")
                .map(|v| v.as_str()),
            Some("eneros-edge")
        );
        assert!(rec.factory_reset_mut().get("site_id").is_none());
    }

    /// ASM4：Diagnose 命令——返回格式化诊断文本，诊断后状态回到 Waiting。
    #[test]
    fn asm4_diagnose_command_text() {
        let (mut rec, _kp, _rng) = setup(64, 512);
        rec.diagnostician_mut().set_partition_table(PartitionTable {
            partitions: [
                make_partition(PartitionSlot::A, PartitionState::Active, "0.124.0", 1),
                make_partition(PartitionSlot::B, PartitionState::Failed, "0.125.0", 2),
            ],
            active_slot: PartitionSlot::A,
        });
        let out = rec
            .execute(RecoveryCommand::Diagnose, &[])
            .expect("诊断应成功");
        assert!(out.contains("partition A: ok"));
        assert!(out.contains("partition B: not ok"));
        assert!(out.contains("boot_fail_count: 3"));
        assert_eq!(
            rec.state(),
            RecoveryState::Waiting,
            "诊断完成应回到 Waiting"
        );
    }

    /// ASM5：ListPartitions 命令——无分区表返回不可用提示；有分区表输出真实状态（D5）。
    #[test]
    fn asm5_list_partitions_command() {
        let (mut rec, _kp, _rng) = setup(64, 512);
        let out = rec
            .execute(RecoveryCommand::ListPartitions, &[])
            .expect("list 应成功");
        assert!(out.contains("unavailable"));

        rec.diagnostician_mut().set_partition_table(PartitionTable {
            partitions: [
                make_partition(PartitionSlot::A, PartitionState::Active, "0.124.0", 1),
                make_partition(PartitionSlot::B, PartitionState::Failed, "0.125.0", 2),
            ],
            active_slot: PartitionSlot::A,
        });
        let out = rec
            .execute(RecoveryCommand::ListPartitions, &[])
            .expect("list 应成功");
        assert!(out.contains("A: Active version=0.124.0"));
        assert!(out.contains("B: Failed version=0.125.0"));
    }

    /// ASM6：Backup → FactoryReset → Restore 经 execute 全流程。
    #[test]
    fn asm6_backup_restore_via_execute() {
        let (mut rec, _kp, _rng) = setup(64, 512);
        rec.factory_reset_mut().set("site_id", "A01");
        rec.factory_reset_mut().set("owner", "ops");
        let out = rec
            .execute(RecoveryCommand::Backup, &[])
            .expect("备份应成功");
        assert!(out.contains("Backup complete"));

        rec.execute(RecoveryCommand::FactoryReset, &[])
            .expect("恢复出厂应成功");
        assert!(rec.factory_reset_mut().get("site_id").is_none());

        let out = rec
            .execute(RecoveryCommand::Restore, &[])
            .expect("恢复应成功");
        assert!(out.contains("Restore complete"));
        assert_eq!(
            rec.factory_reset_mut().get("site_id").map(|v| v.as_str()),
            Some("A01")
        );
        assert_eq!(
            rec.factory_reset_mut().get("owner").map(|v| v.as_str()),
            Some("ops")
        );
    }

    /// ASM7：无备份直接 Restore → CommandUnknown；Reboot 返回提示文本。
    #[test]
    fn asm7_restore_without_backup_and_reboot() {
        let (mut rec, _kp, _rng) = setup(64, 512);
        assert_eq!(
            rec.execute(RecoveryCommand::Restore, &[]),
            Err(RecoveryError::CommandUnknown)
        );
        let out = rec
            .execute(RecoveryCommand::Reboot, &[])
            .expect("reboot 应成功");
        assert!(out.contains("Rebooting"));
    }

    /// ASM8：刷写端到端（impl 层）——receive_chunk 喂入 + execute(Flash) 验签写盘，
    /// 状态/进度正确且设备回读一致；exit 置 Complete。
    #[test]
    fn asm8_flash_end_to_end_and_exit() {
        let (mut rec, kp, mut rng) = setup(64, 512);
        rec.enter().expect("enter 应成功");
        let image: Vec<u8> = (0..3000u32).map(|i| (i % 251) as u8).collect();
        // D8：集成方流式喂入镜像
        rec.flash_writer_mut().receive_chunk(&image[..1000]);
        rec.flash_writer_mut().receive_chunk(&image[1000..]);
        let hash = sm3_hash(&image);
        let sig = sm2_sign(&hash, &kp.private_key, &kp.public_key, &mut rng)
            .expect("签名失败")
            .to_bytes();
        let hash_hex = eneros_hex::encode_lower(&hash);
        let sig_hex = eneros_hex::encode_lower(&sig);
        let out = rec
            .execute(RecoveryCommand::Flash, &[&hash_hex, &sig_hex, "10"])
            .expect("刷写应成功");
        assert!(out.contains("Flash complete"));
        assert_eq!(rec.state(), RecoveryState::Complete);
        let p = rec.get_progress();
        assert_eq!(p.total_bytes, 3000);
        assert_eq!(p.flashed_bytes, 3000);
        assert_eq!(p.state, RecoveryState::Complete);
        assert!(p.error.is_none());

        // 设备回读逐字节一致
        let dev = rec.flash_writer_mut().device();
        let mut readback = Vec::new();
        let mut blk = [0u8; 512];
        for i in 0..6u64 {
            dev.read_block(10 + i, &mut blk).expect("读块失败");
            readback.extend_from_slice(&blk);
        }
        assert_eq!(&readback[..3000], &image[..]);

        rec.exit().expect("exit 应成功");
        assert_eq!(rec.state(), RecoveryState::Complete);
    }

    /// ASM9：验签失败路径——错误记录到 progress.error，状态 Error（不触碰设备）。
    #[test]
    fn asm9_flash_verify_failure_records_error() {
        let (mut rec, _kp, mut rng) = setup(64, 512);
        let image: Vec<u8> = (0..1024u32).map(|i| (i % 97) as u8).collect();
        rec.flash_writer_mut().receive_chunk(&image);
        // 非根私钥签名
        let other = Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败");
        let hash = sm3_hash(&image);
        let sig = sm2_sign(&hash, &other.private_key, &other.public_key, &mut rng)
            .expect("签名失败")
            .to_bytes();
        let hash_hex = eneros_hex::encode_lower(&hash);
        let sig_hex = eneros_hex::encode_lower(&sig);
        assert_eq!(
            rec.execute(RecoveryCommand::Flash, &[&hash_hex, &sig_hex, "10"]),
            Err(RecoveryError::SignatureInvalid)
        );
        assert_eq!(rec.state(), RecoveryState::Error);
        let p = rec.get_progress();
        assert_eq!(p.state, RecoveryState::Error);
        let err = p.error.as_ref().expect("错误应被记录");
        assert!(err.as_str().contains("verify"));
        // 设备零写入
        let dev = rec.flash_writer_mut().device();
        let mut blk = [0u8; 512];
        for i in 0..dev.block_count() {
            dev.read_block(i, &mut blk).expect("读块失败");
            assert!(blk.iter().all(|&b| b == 0), "块 {i} 不应被写入");
        }
    }
}
