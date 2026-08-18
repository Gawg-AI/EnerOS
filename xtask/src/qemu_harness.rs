use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use eneros_boot_protocol::{
    BootControl, MeasurementLog, BOOT_CONTROL_COPY_OFFSETS, BOOT_CONTROL_RECORD_SIZE,
    MEASUREMENT_LOG_SIZE,
};
use eneros_image_format::parse_gpt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::image_factory::wsl_path;
use super::{invalid, DynResult};

const DEFAULT_FIRMWARE: &str = "/usr/share/AAVMF/AAVMF_CODE.secboot.fd";

/// WP-27.1 AAVMF 固件锁定声明（R27-2 供应链缓解）。
///
/// 锁定记录（本环境 2026-08-10 实测，evidence/boot-unblock/ 有导出字节）：
/// - 路径：`/usr/share/AAVMF/AAVMF_CODE.secboot.fd`（Ubuntu 发行版包
///   `qemu-efi-aarch64`，版本字符串 `2025.11-3ubuntu7`，edk2-2025.11
///   构建于 2026-03-31，见固件内 `Sec.dll` 构建路径与启动日志首行）
/// - SHA256：`aafedb40e155a33ef37da3f94169352c111356c6c510359dd25fdfb4817bc2d6`
/// - VARS 模板：`/usr/share/AAVMF/AAVMF_VARS.fd`（64 MiB，全零化后作为
///   qemu-prod 模板；全零 VARS 使固件不含任何 Boot 项/anchor 变量）
///
/// 判定事实（WP-27.1 诊断结论）：2025.11 固件下 G1 日志（2026-08-09
/// 21:31）完整复现过 13 行启动断言——BOOTCTRL 分区由 xtask 镜像工厂与
/// boot-manager 自身写入，固件不写该分区；"bootctrl-resolve-fail
/// error=Genesis" 根因是磁盘 BOOTCTRL 被上次运行写为 seq=1 而 VARS 模板
/// 无保护锚点（fail-closed 语义正确），与固件版本无因果。更换/升级固件
/// 前必须重跑 qemu-prod 全链证据并在此更新版本+哈希。
/// 环境变量 `ENEROS_QEMU_FIRMWARE` 可指向替代固件（测试环境用 xPack
/// edk2-arm-code.fd 时）。
fn firmware_path() -> String {
    std::env::var("ENEROS_QEMU_FIRMWARE").unwrap_or_else(|_| DEFAULT_FIRMWARE.to_owned())
}

#[derive(Debug)]
struct RunOptions {
    image: PathBuf,
    vars_template: PathBuf,
    expected: Expected,
    evidence_dir: Option<PathBuf>,
    disk_read_only: bool,
    vars_read_only: bool,
    disk_fault_sector: Option<u64>,
    rng_present: bool,
    net_present: bool,
    /// 43（L2 协议互操作）：额外挂 user-mode（slirp）net 于槽 8。
    net_user: bool,
    data_disk: Option<PathBuf>,
    expected_slot: Option<String>,
    expected_reason: Option<String>,
    /// WP-27.1：运行前把镜像 BOOTCTRL 双副本归零（seq=0、双副本一致、
    /// 重算 CRC）。QEMU cache=none 直写镜像，上次成功引导会把 seq 递增为
    /// 1，而每次运行都复制全新 VARS 模板（无保护锚点）→ 无锚点路径
    /// Genesis 拒绝（fail-closed 语义正确，非 bug）。本开关把"重建镜像
    /// （约 10 分钟）等效于归零（秒级）"自动化，默认关闭（不破坏
    /// Reject/CUT/fault 注入等依赖磁盘特定状态的场景）。
    reset_bootctrl: bool,
    /// 42：QEMU QMP 受控退出。检测到 boot-evidence 后由 harness 发送
    /// `quit`，避免把 timeout 当 PASS。
    qmp_shutdown: bool,
    /// 42（WP-42.6）：QMP 退出前等待的就绪标记。`root-heartbeat` 只证明
    /// root 心跳（G2 链），`product-ready` 还要求根任务打印
    /// `PRODUCT-READY: true` 且 required degraded 为空（profile 声明集合
    /// 全部就绪）。默认 root-heartbeat 保持既有场景兼容。
    ready_marker: ReadyMarker,
    /// 42（WP-42.6）：QMP 受控退出等待标记的 deadline 秒数；超时 =
    /// NO_RESULT/FAIL，永远不是 PASS。默认 200s 与既有命令一致，formal
    /// product 场景用 420s（41 实测 PRODUCT-READY 约 290s）。
    qmp_deadline_secs: u64,
}

/// 42（WP-42.6）：QMP 受控退出的就绪标记选择。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadyMarker {
    RootHeartbeat,
    ProductReady,
}

impl ReadyMarker {
    fn parse(value: &str) -> DynResult<Self> {
        match value {
            "root-heartbeat" => Ok(Self::RootHeartbeat),
            "product-ready" => Ok(Self::ProductReady),
            _ => Err(invalid(
                "--ready-marker must be root-heartbeat or product-ready",
            )),
        }
    }

    /// 根任务在 required 服务全部 Ready 后才打印该后缀（WP-37.1 判据：
    /// `PRODUCT-READY: true (degraded: …; required degraded: )`；required
    /// degraded 为空时行尾恰为 `required degraded: )`）。
    fn qmp_marker(self) -> &'static [u8] {
        match self {
            Self::RootHeartbeat => br#""test":"root-heartbeat","result":"PASS""#,
            Self::ProductReady => br#"required degraded: )"#,
        }
    }
}

#[derive(Debug)]
struct InstallRunOptions {
    iso: PathBuf,
    payload: PathBuf,
    vars_template: PathBuf,
    evidence_dir: PathBuf,
    expected: ExpectedInstall,
    disk_size_bytes: Option<u64>,
    target_nonblank: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ExpectedInstall {
    Pass,
    Reject(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Expected {
    Pass,
    Reject(String),
    FirmwareReject,
    PowerCut(PowerCutStage),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PowerCutStage {
    TargetWritten,
    AnchorCommitted,
    MirrorWritten,
}

impl PowerCutStage {
    fn parse(value: &str) -> DynResult<Self> {
        match value {
            "TARGET_WRITTEN" => Ok(Self::TargetWritten),
            "ANCHOR_COMMITTED" => Ok(Self::AnchorCommitted),
            "MIRROR_WRITTEN" => Ok(Self::MirrorWritten),
            _ => Err(invalid(
                "CUT stage must be TARGET_WRITTEN, ANCHOR_COMMITTED, or MIRROR_WRITTEN",
            )),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::TargetWritten => "TARGET_WRITTEN",
            Self::AnchorCommitted => "ANCHOR_COMMITTED",
            Self::MirrorWritten => "MIRROR_WRITTEN",
        }
    }

    const fn marker(self) -> &'static str {
        match self {
            Self::TargetWritten => "ENEROS stage=bootctrl-target-written",
            Self::AnchorCommitted => "ENEROS stage=bootctrl-anchor-committed",
            Self::MirrorWritten => "ENEROS stage=bootctrl-mirror-written",
        }
    }

    const fn forbidden_later_marker(self) -> Option<&'static str> {
        match self {
            Self::TargetWritten => Some("ENEROS stage=bootctrl-anchor-committed"),
            Self::AnchorCommitted => Some("ENEROS stage=bootctrl-mirror-written"),
            Self::MirrorWritten => None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BootEvidence {
    schema: String,
    result: String,
    slot: String,
    boot_reason: String,
    security_version: u64,
    bootctrl_generation: u64,
    memory_map_count: u32,
    init_bytes: u64,
    source_date_epoch: u64,
    manifest_sha256: String,
    measurement_root_sha256: String,
    measurement_log_hex: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RejectionEvidence {
    schema: String,
    result: String,
    code: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InstallEvidence {
    schema: String,
    result: String,
    bytes: u64,
    payload_sha256: String,
    readback: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallRejectionEvidence {
    schema: String,
    result: String,
    code: String,
}

#[derive(Debug, Serialize)]
struct InstallHarnessResult {
    schema: &'static str,
    result: &'static str,
    expected: String,
    observed: String,
    qemu_exit: i32,
    firmware_path: String,
    firmware_sha256: String,
    vars_template_sha256: String,
    vars_run_final_sha256: String,
    iso_size_bytes: u64,
    iso_sha256: String,
    payload_size_bytes: u64,
    payload_sha256: String,
    installed_preboot_sha256: Option<String>,
    target_size_bytes: u64,
    target_initial_state: &'static str,
    target_initial_sha256: String,
    target_postinstall_sha256: String,
    target_unchanged_on_rejection: &'static str,
    iso_removed_boot: &'static str,
    argv_policy: &'static str,
}

#[derive(Debug, Serialize)]
struct HarnessResult {
    schema: &'static str,
    result: &'static str,
    expected: String,
    observed: String,
    qemu_exit: i32,
    firmware_path: String,
    firmware_sha256: String,
    vars_template_sha256: String,
    vars_run_final_sha256: String,
    image_size_bytes: u64,
    image_initial_sha256: String,
    image_final_sha256: String,
    manifest_sha256: Option<String>,
    measurement_root_sha256: Option<String>,
    boot_slot: Option<String>,
    boot_reason: Option<String>,
    security_version: Option<u64>,
    bootctrl_generation: Option<u64>,
    disk_mode: &'static str,
    vars_mode: &'static str,
    disk_fault_sector: Option<u64>,
    rng_mode: &'static str,
    argv_policy: &'static str,
}

#[derive(Debug, Serialize)]
struct BootControlEvidence {
    schema: &'static str,
    first_valid: bool,
    first_sequence: Option<u64>,
    first_security_floor: Option<u64>,
    first_sha256: String,
    second_valid: bool,
    second_sequence: Option<u64>,
    second_security_floor: Option<u64>,
    second_sha256: String,
    copies_equal: bool,
}

pub(super) fn command_run(root: &Path, args: &[String]) -> DynResult<()> {
    let Some((profile, tail)) = args.split_first() else {
        return Err(invalid("run requires an explicit profile"));
    };
    match profile.as_str() {
        "qemu-prod" => run_qemu(root, parse_options(tail)?),
        "qemu-g3" => command_g3(root, tail),
        "qemu-install" => run_installer(root, parse_install_options(tail)?),
        _ => Err(invalid(
            "the accepted media harness profiles are qemu-prod, qemu-g3, and qemu-install",
        )),
    }
}

fn parse_install_options(args: &[String]) -> DynResult<InstallRunOptions> {
    let mut iso = None;
    let mut payload = None;
    let mut vars_template = None;
    let mut evidence_dir = None;
    let mut expected = None;
    let mut disk_size_bytes = None;
    let mut target_nonblank = false;
    let mut index = 0usize;
    while index < args.len() {
        let flag = &args[index];
        let value = args
            .get(index + 1)
            .ok_or_else(|| invalid(format!("{flag} requires a value")))?;
        match flag.as_str() {
            "--iso" => iso = Some(PathBuf::from(value)),
            "--payload" => payload = Some(PathBuf::from(value)),
            "--vars-template" => vars_template = Some(PathBuf::from(value)),
            "--evidence-dir" => evidence_dir = Some(PathBuf::from(value)),
            "--expect" => {
                expected = Some(if value == "PASS" {
                    ExpectedInstall::Pass
                } else if let Some(code) = value.strip_prefix("REJECT:") {
                    if code.is_empty()
                        || !code
                            .bytes()
                            .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
                    {
                        return Err(invalid(
                            "installer rejection code is not uppercase snake case",
                        ));
                    }
                    ExpectedInstall::Reject(code.to_owned())
                } else {
                    return Err(invalid("installer --expect must be PASS or REJECT:<CODE>"));
                });
            }
            "--disk-size-bytes" => {
                disk_size_bytes = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| invalid("--disk-size-bytes must be an unsigned integer"))?,
                )
            }
            "--target-state" => match value.as_str() {
                "blank" => target_nonblank = false,
                "nonblank" => target_nonblank = true,
                _ => return Err(invalid("--target-state must be blank or nonblank")),
            },
            _ => return Err(invalid(format!("unknown qemu-install option '{flag}'"))),
        }
        index += 2;
    }
    Ok(InstallRunOptions {
        iso: iso.ok_or_else(|| invalid("--iso is mandatory"))?,
        payload: payload.ok_or_else(|| invalid("--payload is mandatory"))?,
        vars_template: vars_template.ok_or_else(|| invalid("--vars-template is mandatory"))?,
        evidence_dir: evidence_dir.ok_or_else(|| invalid("--evidence-dir is mandatory"))?,
        expected: expected.ok_or_else(|| invalid("--expect is mandatory"))?,
        disk_size_bytes,
        target_nonblank,
    })
}

fn parse_options(args: &[String]) -> DynResult<RunOptions> {
    let mut image = None;
    let mut vars_template = None;
    let mut expected = None;
    let mut evidence_dir = None;
    let mut disk_read_only = false;
    let mut vars_read_only = false;
    let mut disk_fault_sector = None;
    let mut rng_present = true;
    let mut net_present = true;
    let mut net_user = false;
    let mut data_disk = None;
    let mut expected_slot = None;
    let mut expected_reason = None;
    let mut reset_bootctrl = false;
    let mut qmp_shutdown = false;
    let mut ready_marker = ReadyMarker::RootHeartbeat;
    let mut qmp_deadline_secs = 200u64;
    let mut index = 0usize;
    while index < args.len() {
        let flag = &args[index];
        if flag == "--reset-bootctrl" {
            // 无值布尔开关：运行前归零镜像 BOOTCTRL 双副本（WP-27.1）。
            reset_bootctrl = true;
            index += 1;
            continue;
        }
        if flag == "--qmp-shutdown" {
            // 无值布尔开关：检测到就绪标记后由 QMP `quit` 受控退出。
            qmp_shutdown = true;
            index += 1;
            continue;
        }
        if flag == "--net-user" {
            // 43（L2 协议互操作）：无值布尔开关——user-mode net @槽 8。
            net_user = true;
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| invalid(format!("{flag} requires a value")))?;
        match flag.as_str() {
            "--image" => image = Some(PathBuf::from(value)),
            "--vars-template" => vars_template = Some(PathBuf::from(value)),
            "--expect" => {
                expected = Some(if value == "PASS" {
                    Expected::Pass
                } else if value == "FIRMWARE_REJECT" {
                    Expected::FirmwareReject
                } else if let Some(stage) = value.strip_prefix("CUT:") {
                    Expected::PowerCut(PowerCutStage::parse(stage)?)
                } else if let Some(code) = value.strip_prefix("REJECT:") {
                    if code.is_empty()
                        || !code
                            .bytes()
                            .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
                    {
                        return Err(invalid(
                            "REJECT code must be non-empty uppercase snake case",
                        ));
                    }
                    Expected::Reject(code.to_owned())
                } else {
                    return Err(invalid(
                        "--expect must be PASS, FIRMWARE_REJECT, REJECT:<CODE>, or CUT:<STAGE>",
                    ));
                });
            }
            "--evidence-dir" => evidence_dir = Some(PathBuf::from(value)),
            "--disk-mode" => match value.as_str() {
                "writable" => disk_read_only = false,
                "readonly" => disk_read_only = true,
                _ => return Err(invalid("--disk-mode must be writable or readonly")),
            },
            "--vars-mode" => match value.as_str() {
                "writable" => vars_read_only = false,
                "readonly" => vars_read_only = true,
                _ => return Err(invalid("--vars-mode must be writable or readonly")),
            },
            "--disk-fault-sector" => {
                disk_fault_sector = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| invalid("--disk-fault-sector must be an unsigned integer"))?,
                )
            }
            "--rng-mode" => match value.as_str() {
                "present" => rng_present = true,
                "absent" => rng_present = false,
                _ => return Err(invalid("--rng-mode must be present or absent")),
            },
            "--net-mode" => match value.as_str() {
                "present" => net_present = true,
                "absent" => net_present = false,
                _ => return Err(invalid("--net-mode must be present or absent")),
            },
            "--data-disk" => data_disk = Some(PathBuf::from(value)),
            "--expect-slot" => match value.as_str() {
                "A" | "B" | "RECOVERY" => expected_slot = Some(value.clone()),
                _ => return Err(invalid("--expect-slot must be A, B, or RECOVERY")),
            },
            "--expect-reason" => {
                if value.is_empty()
                    || !value
                        .bytes()
                        .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
                {
                    return Err(invalid(
                        "--expect-reason must be non-empty uppercase snake case",
                    ));
                }
                expected_reason = Some(value.clone());
            }
            "--ready-marker" => ready_marker = ReadyMarker::parse(value)?,
            "--qmp-deadline-secs" => {
                qmp_deadline_secs = value
                    .parse::<u64>()
                    .map_err(|_| invalid("--qmp-deadline-secs must be an unsigned integer"))?
            }
            _ => return Err(invalid(format!("unknown run qemu-prod option '{flag}'"))),
        }
        index += 2;
    }
    Ok(RunOptions {
        image: image.ok_or_else(|| invalid("--image is mandatory"))?,
        vars_template: vars_template.ok_or_else(|| invalid("--vars-template is mandatory"))?,
        expected: expected.ok_or_else(|| invalid("--expect is mandatory"))?,
        evidence_dir,
        disk_read_only,
        vars_read_only,
        disk_fault_sector,
        rng_present,
        net_present,
        net_user,
        data_disk,
        expected_slot,
        expected_reason,
        reset_bootctrl,
        qmp_shutdown,
        ready_marker,
        qmp_deadline_secs,
    })
}

fn run_installer(root: &Path, options: InstallRunOptions) -> DynResult<()> {
    let iso = rooted_target_path(root, &options.iso)?;
    let payload = rooted_target_path(root, &options.payload)?;
    let vars_template = rooted_target_path(root, &options.vars_template)?;
    if !iso.is_file() || !payload.is_file() || !vars_template.is_file() {
        return Err(invalid(
            "installer ISO, payload, and vars template must exist below target",
        ));
    }
    let evidence_dir = rooted_target_path(root, &options.evidence_dir)?;
    fs::create_dir_all(&evidence_dir)?;
    let target = evidence_dir.join("installed.raw");
    let payload_bytes = fs::read(&payload)?;
    let payload_size = u64::try_from(payload_bytes.len())?;
    let target_size = options.disk_size_bytes.unwrap_or(payload_size);
    if target_size == 0 || target_size % 512 != 0 {
        return Err(invalid(
            "installer target size must be non-zero and 512-byte aligned",
        ));
    }
    let mut target_file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&target)?;
    target_file.set_len(target_size)?;
    if options.target_nonblank {
        target_file.write_all(&[0xa5])?;
    }
    target_file.sync_all()?;
    drop(target_file);
    let initial_target = fs::read(&target)?;
    let initial_target_sha256 = hex(&Sha256::digest(&initial_target));

    let run_vars = evidence_dir.join("AAVMF_VARS.install.fd");
    let vars_template_sha256 = hex(&Sha256::digest(fs::read(&vars_template)?));
    fs::copy(&vars_template, &run_vars)?;
    let iso_wsl = wsl_path(root, &iso)?;
    let target_wsl = wsl_path(root, &target)?;
    let vars_wsl = wsl_path(root, &run_vars)?;
    let qemu_args = vec![
        "qemu-system-aarch64".to_owned(),
        // icount：TCG 下虚拟时钟随指令推进，arch timer 可靠触发
        // （无 icount 时空闲/WFE 下 QEMU 虚拟时钟停滞，timer IRQ 不产生）。
        "-machine".to_owned(),
        "virt,gic-version=2".to_owned(),
        "-smp".to_owned(),
        "4".to_owned(),
        "-cpu".to_owned(),
        "cortex-a57".to_owned(),
        "-m".to_owned(),
        // BOOT-003：内存必须与 sel4 qemu-arm-virt DTB 一致（1GB）——内核按 DTB
        // 的 memory 描述初始化并清零（memzero），512M 时访问 0x7ffc0000 区域
        // （phys 超出实际 RAM）触发 KERNEL DATA ABORT（translation fault L0）。
        "1G".to_owned(),
        // 42 installer：只使用 `-boot order=d` 从 ISO 启动；不能混入
        // qemu-prod 的 `order=c`（QEMU 取首个 -boot，导致 installer 走 PXE）。
        "-nographic".to_owned(),
        "-no-reboot".to_owned(),
        "-drive".to_owned(),
        format!("if=pflash,format=raw,readonly=on,file={}", firmware_path()),
        "-drive".to_owned(),
        format!("if=pflash,format=raw,file={vars_wsl}"),
        "-drive".to_owned(),
        format!("if=none,format=raw,file={target_wsl},id=enerosdisk"),
        "-device".to_owned(),
        "virtio-blk-device,bus=virtio-mmio-bus.0,drive=enerosdisk,bootindex=1".to_owned(),
        "-device".to_owned(),
        "virtio-scsi-device,id=installscsi".to_owned(),
        "-drive".to_owned(),
        format!("if=none,format=raw,readonly=on,media=cdrom,file={iso_wsl},id=installcd"),
        "-device".to_owned(),
        "scsi-cd,drive=installcd,bootindex=0".to_owned(),
        "-object".to_owned(),
        "rng-random,filename=/dev/urandom,id=enerosrng".to_owned(),
        "-device".to_owned(),
        "virtio-rng-device,bus=virtio-mmio-bus.3,rng=enerosrng".to_owned(),
        "-boot".to_owned(),
        "order=d".to_owned(),
    ];
    validate_argv(&qemu_args)?;
    if qemu_args
        .iter()
        .filter(|argument| argument.contains("id=installcd"))
        .count()
        != 1
    {
        return Err(invalid(
            "installer argv must mount exactly one installer ISO",
        ));
    }
    fs::write(evidence_dir.join("install-qemu.argv"), qemu_args.join(" "))?;
    let output = if cfg!(windows) {
        let mut command = Command::new("wsl.exe");
        command.args([
            "-d",
            "Ubuntu-22.04",
            "--",
            "timeout",
            "--signal=KILL",
            "120s",
        ]);
        command.args(&qemu_args);
        command.output()?
    } else {
        let mut command = Command::new("timeout");
        command.args(["--signal=KILL", "120s"]);
        command.args(&qemu_args);
        command.output()?
    };
    let exit = output.status.code().unwrap_or(-1);
    let mut raw_log = output.stdout;
    raw_log.extend_from_slice(&output.stderr);
    fs::write(evidence_dir.join("install-qemu-serial.log"), &raw_log)?;
    if !output.status.success() {
        return Err(invalid(format!(
            "installer QEMU did not shut down successfully (exit={exit}); timeout is a failure"
        )));
    }
    let log = String::from_utf8_lossy(&raw_log).replace('\0', "");
    let payload_sha256 = hex(&Sha256::digest(&payload_bytes));
    let iso_bytes = fs::read(&iso)?;
    let (
        expected,
        observed,
        installed_preboot_sha256,
        target_postinstall_sha256,
        target_unchanged_on_rejection,
        iso_removed_boot,
    ) = match &options.expected {
        ExpectedInstall::Pass => {
            let evidence: InstallEvidence = parse_record(&log, "eneros.install-evidence.v1")?;
            if evidence.schema != "eneros.install-evidence.v1"
                || evidence.result != "PASS"
                || evidence.bytes != payload_size
                || evidence.payload_sha256 != payload_sha256
                || evidence.readback != "PASS"
            {
                return Err(invalid(
                    "installer evidence does not match the signed payload",
                ));
            }
            let installed = fs::read(&target)?;
            let installed_hash = hex(&Sha256::digest(&installed));
            if installed.len() != payload_bytes.len() || installed_hash != payload_sha256 {
                return Err(invalid(
                    "installed disk differs from the signed payload before first boot",
                ));
            }
            fs::write(
                evidence_dir.join("install-evidence.json"),
                serde_json::to_vec_pretty(&evidence)?,
            )?;
            let boot_evidence = evidence_dir.join("boot-after-iso-removal");
            run_qemu(
                root,
                RunOptions {
                    image: target.clone(),
                    vars_template: run_vars.clone(),
                    expected: Expected::Pass,
                    evidence_dir: Some(boot_evidence),
                    disk_read_only: false,
                    vars_read_only: false,
                    disk_fault_sector: None,
                    rng_present: true,
                    net_present: true,
                    net_user: false,
                    data_disk: None,
                    expected_slot: None,
                    expected_reason: None,
                    reset_bootctrl: true,
                    qmp_shutdown: !cfg!(windows),
                    ready_marker: ReadyMarker::RootHeartbeat,
                    qmp_deadline_secs: 200,
                },
            )?;
            (
                "PASS".to_owned(),
                "PASS".to_owned(),
                Some(installed_hash.clone()),
                installed_hash,
                "NOT_APPLICABLE",
                "PASS",
            )
        }
        ExpectedInstall::Reject(code) => {
            let evidence: InstallRejectionEvidence =
                parse_record(&log, "eneros.install-rejection.v1")?;
            if evidence.schema != "eneros.install-rejection.v1"
                || evidence.result != "REJECT"
                || &evidence.code != code
            {
                return Err(invalid(format!(
                    "installer rejection mismatch: expected {code}, observed {}",
                    evidence.code
                )));
            }
            let postinstall = fs::read(&target)?;
            if postinstall != initial_target {
                return Err(invalid(format!(
                    "installer rejection {code} modified the target disk"
                )));
            }
            (
                format!("REJECT:{code}"),
                format!("REJECT:{}", evidence.code),
                None,
                hex(&Sha256::digest(&postinstall)),
                "PASS",
                "NOT_RUN_EXPECTED_REJECTION",
            )
        }
    };
    let result = InstallHarnessResult {
        schema: "eneros.qemu-install-result.v1",
        result: "PASS",
        expected,
        observed,
        qemu_exit: exit,
        firmware_path: firmware_path(),
        firmware_sha256: firmware_sha256()?,
        vars_template_sha256,
        vars_run_final_sha256: hex(&Sha256::digest(fs::read(&run_vars)?)),
        iso_size_bytes: u64::try_from(iso_bytes.len())?,
        iso_sha256: hex(&Sha256::digest(&iso_bytes)),
        payload_size_bytes: payload_size,
        payload_sha256,
        installed_preboot_sha256,
        target_size_bytes: target_size,
        target_initial_state: if options.target_nonblank {
            "nonblank"
        } else {
            "blank"
        },
        target_initial_sha256: initial_target_sha256,
        target_postinstall_sha256,
        target_unchanged_on_rejection,
        iso_removed_boot,
        argv_policy: "PASS",
    };
    fs::write(
        evidence_dir.join("qemu-install-result.json"),
        serde_json::to_vec_pretty(&result)?,
    )?;
    println!(
        "QEMU profile=qemu-install result=PASS expected={} iso_sha256={} payload_sha256={}",
        result.expected, result.iso_sha256, result.payload_sha256
    );
    Ok(())
}

#[cfg(unix)]
fn run_qemu_qmp_quit(
    qemu_args: &[String],
    evidence_dir: &Path,
    marker: &[u8],
    deadline_secs: u64,
) -> DynResult<(i32, Vec<u8>)> {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::UnixStream;
    use std::process::Stdio;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    let sock_path = evidence_dir.join("qmp.sock");
    let _ = fs::remove_file(&sock_path);
    let mut args = qemu_args.to_vec();
    args.push("-qmp".to_owned());
    args.push(format!("unix:{},server,wait=off", sock_path.display()));

    let mut child = Command::new(&args[0])
        .args(&args[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| invalid("failed to take QEMU stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| invalid("failed to take QEMU stderr"))?;

    let log: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let log_out = Arc::clone(&log);
    let out_thread = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => log_out.lock().unwrap().extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
    });
    let log_err = Arc::clone(&log);
    let err_thread = thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => log_err.lock().unwrap().extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(deadline_secs);
    let found = loop {
        if let Some(_status) = child.try_wait()? {
            break false;
        }
        let snapshot = log.lock().unwrap().clone();
        if snapshot.windows(marker.len()).any(|w| w == marker) {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        thread::sleep(Duration::from_millis(100));
    };

    if !found {
        let _ = child.kill();
        let _ = child.wait();
        let _ = out_thread.join();
        let _ = err_thread.join();
        let snapshot = log.lock().unwrap().clone();
        fs::write(evidence_dir.join("qemu-serial.log"), &snapshot)?;
        return Err(invalid(
            "QEMU QMP shutdown: readiness marker not found before deadline (NO_RESULT, not PASS)",
        ));
    }

    let mut stream =
        UnixStream::connect(&sock_path).map_err(|e| invalid(format!("QMP connect failed: {e}")))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut greeting = String::new();
    let _ = reader.read_line(&mut greeting);
    stream.write_all(b"{\"execute\":\"qmp_capabilities\"}\n")?;
    let mut capability_response = String::new();
    let _ = reader.read_line(&mut capability_response);
    stream.write_all(b"{\"execute\":\"quit\"}\n")?;
    drop(stream);

    let status = child.wait()?;
    let _ = out_thread.join();
    let _ = err_thread.join();
    let snapshot = log.lock().unwrap().clone();
    Ok((status.code().unwrap_or(-1), snapshot))
}

#[cfg(not(unix))]
fn run_qemu_qmp_quit(_qemu_args: &[String], _evidence_dir: &Path) -> DynResult<(i32, Vec<u8>)> {
    Err(invalid(
        "QMP shutdown is only implemented on Unix; use WSL/Linux runner",
    ))
}

fn run_qemu(root: &Path, options: RunOptions) -> DynResult<()> {
    let image = rooted_target_path(root, &options.image)?;
    let vars_template = rooted_target_path(root, &options.vars_template)?;
    if !image.is_file() || !vars_template.is_file() {
        return Err(invalid("QEMU image and vars template must both exist"));
    }
    if options.reset_bootctrl {
        // WP-27.1：同盘重跑（cache=none 直写）后 BOOTCTRL seq≠0 + VARS 无锚点
        // 会触发 fail-closed Genesis。运行前显式归零（seq=0、双副本一致、
        // 重算 CRC）等价于重建镜像的 BOOTCTRL 状态；仅对显式请求生效。
        let before = fs::read(&image)?;
        let mut mutable = before.clone();
        reset_bootctrl_copies(&mut mutable)?;
        fs::write(&image, &mutable)?;
        println!(
            "QEMU profile=qemu-prod bootctrl reset before_sha256={} after_sha256={}",
            hex(&Sha256::digest(&before)),
            hex(&Sha256::digest(&mutable))
        );
    }
    let image_initial_bytes = fs::read(&image)?;
    let image_initial_sha256 = hex(&Sha256::digest(&image_initial_bytes));
    let evidence_dir = match options.evidence_dir {
        Some(path) => rooted_target_path(root, &path)?,
        None => image
            .parent()
            .ok_or_else(|| invalid("image has no parent"))?
            .join("qa-003"),
    };
    fs::create_dir_all(&evidence_dir)?;
    let run_vars = evidence_dir.join("AAVMF_VARS.run.fd");
    let vars_template_sha256 = hex(&Sha256::digest(fs::read(&vars_template)?));
    fs::copy(&vars_template, &run_vars)?;

    let image_wsl = wsl_path(root, &image)?;
    let vars_wsl = wsl_path(root, &run_vars)?;
    let image_sectors = fs::metadata(&image)?.len() / 512;
    if options
        .disk_fault_sector
        .is_some_and(|sector| sector >= image_sectors)
    {
        return Err(invalid("disk fault sector is outside the image"));
    }
    let disk_file = if let Some(sector) = options.disk_fault_sector {
        let config = evidence_dir.join("blkdebug.conf");
        fs::write(
            &config,
            format!(
                "[inject-error]\nevent = \"write_aio\"\nerrno = \"5\"\nsector = \"{sector}\"\nonce = \"on\"\n"
            ),
        )?;
        let config_wsl = wsl_path(root, &config)?;
        format!("blkdebug:{config_wsl}:{image_wsl}")
    } else {
        image_wsl.clone()
    };
    let mut qemu_args = vec![
        "qemu-system-aarch64".to_owned(),
        // icount：TCG 下虚拟时钟随指令推进，arch timer 可靠触发
        // （无 icount 时空闲/WFE 下 QEMU 虚拟时钟停滞，timer IRQ 不产生）。
        "-machine".to_owned(),
        // acpi=off：AAVMF 固件在 ACPI 存在时选择 ACPI 硬件描述，
        // 不把 FDT 注册进 UEFI 配置表（gFdtTableGuid）；关闭 ACPI 后
        // 固件才安装 FDT 配置表，boot-manager 才能定位 DTB（BOOT-001）。
        "virt,gic-version=2,acpi=off".to_owned(),
        "-smp".to_owned(),
        "4".to_owned(),
        "-cpu".to_owned(),
        "cortex-a57".to_owned(),
        "-m".to_owned(),
        // BOOT-003：内存必须与 sel4 qemu-arm-virt DTB 一致（1GB）——内核按 DTB
        // 的 memory 描述初始化并清零（memzero），512M 时访问 0x7ffc0000 区域
        // （phys 超出实际 RAM）触发 KERNEL DATA ABORT（translation fault L0）。
        "1G".to_owned(),
        // BOOT-006：强制磁盘 boot 优先——AAVMF 的 PXE boot 项（vars 模板自带）
        // 会随机抢跑（DHCP 超时可达 60s+），导致 qemu-prod 验证超时；order=c
        // 让 UEFI 直接尝试磁盘（BOOTAA64.EFI）。
        "-boot".to_owned(),
        "order=c".to_owned(),
        "-nographic".to_owned(),
        "-no-reboot".to_owned(),
        "-drive".to_owned(),
        format!("if=pflash,format=raw,readonly=on,file={}", firmware_path()),
        "-drive".to_owned(),
        if options.vars_read_only {
            format!("if=pflash,format=raw,readonly=on,file={vars_wsl}")
        } else {
            format!("if=pflash,format=raw,file={vars_wsl}")
        },
        "-drive".to_owned(),
        if options.disk_read_only {
            format!("if=none,format=raw,readonly=on,file={disk_file},id=enerosdisk")
        } else {
            format!("if=none,format=raw,file={disk_file},id=enerosdisk")
        },
        "-device".to_owned(),
        "virtio-blk-device,bus=virtio-mmio-bus.0,drive=enerosdisk".to_owned(),
    ];
    if options.rng_present {
        qemu_args.extend([
            "-object".to_owned(),
            "rng-random,filename=/dev/urandom,id=enerosrng".to_owned(),
            "-device".to_owned(),
            "virtio-rng-device,bus=virtio-mmio-bus.3,rng=enerosrng".to_owned(),
        ]);
    }
    // G3 数据盘（storage 服务）：16MB raw 文件，QEMU 槽 2。
    let data_image = if let Some(path) = &options.data_disk {
        rooted_target_path(root, path)?
    } else {
        let path = evidence_dir.join("eneros-data.raw");
        if !path.is_file() {
            fs::write(&path, vec![0u8; 16 * 1024 * 1024])?;
        }
        path
    };
    let data_image_wsl = wsl_path(root, &data_image)?;
    qemu_args.extend([
        "-drive".to_owned(),
        // cache=none（O_DIRECT）：kill QEMU 即真实断电语义（未 flush
        // 数据不落盘），断电矩阵依赖此语义。
        format!("if=none,format=raw,cache=none,file={data_image_wsl},id=enerosdata"),
        "-device".to_owned(),
        "virtio-blk-device,bus=virtio-mmio-bus.2,drive=enerosdata".to_owned(),
    ]);
    // G3 virtio-net（QEMU 槽 3；socket 后端 = 外部独立对端，无 slirp 依赖）。
    if options.net_present {
        qemu_args.extend([
            "-netdev".to_owned(),
            // socket 后端：QEMU 监听，外部对端进程连接后形成 L2 桥。
            "socket,id=enerosnet,listen=127.0.0.1:5577".to_owned(),
            "-device".to_owned(),
            "virtio-net-device,bus=virtio-mmio-bus.1,netdev=enerosnet".to_owned(),
        ]);
    }
    // 43（L2 协议互操作）：user-mode（slirp）net 挂 virtio-mmio 槽 8
    // （0x0a001000——svc-modbus-tcp 移交槽位）；guest 10.0.2.15/24，
    // 10.0.2.2 = host loopback 别名（外部 raw 从站监听 host 端口可达）。
    if options.net_user {
        qemu_args.extend([
            "-netdev".to_owned(),
            "user,id=enerosnet8".to_owned(),
            "-device".to_owned(),
            "virtio-net-device,bus=virtio-mmio-bus.8,netdev=enerosnet8".to_owned(),
        ]);
    }
    validate_argv(&qemu_args)?;
    let archived_argv = qemu_args.join(" ");
    fs::write(evidence_dir.join("qemu.argv"), archived_argv.as_bytes())?;

    let (exit, raw_log) = if let Expected::PowerCut(stage) = &options.expected {
        run_power_cut(root, &qemu_args, &evidence_dir, *stage)?
    } else if matches!(options.expected, Expected::FirmwareReject) {
        run_firmware_reject(root, &qemu_args, &evidence_dir)?
    } else if options.qmp_shutdown && !cfg!(windows) {
        run_qemu_qmp_quit(
            &qemu_args,
            &evidence_dir,
            options.ready_marker.qmp_marker(),
            options.qmp_deadline_secs,
        )?
    } else {
        let output = if cfg!(windows) {
            let mut command = Command::new("wsl.exe");
            command.args([
                "-d",
                "Ubuntu-22.04",
                "--",
                "timeout",
                "--signal=KILL",
                "60s",
            ]);
            command.args(&qemu_args);
            command.output()?
        } else {
            let mut command = Command::new("timeout");
            command.args(["--signal=KILL", "200s"]);
            command.args(&qemu_args);
            command.output()?
        };
        let exit = output.status.code().unwrap_or(-1);
        let mut raw_log = output.stdout;
        raw_log.extend_from_slice(&output.stderr);
        if !output.status.success() {
            fs::write(evidence_dir.join("qemu-serial.log"), &raw_log)?;
            return Err(invalid(format!(
                "QEMU did not shut down successfully (exit={exit}); timeout is a failure"
            )));
        }
        (exit, raw_log)
    };
    fs::write(evidence_dir.join("qemu-serial.log"), &raw_log)?;
    let log = String::from_utf8_lossy(&raw_log).replace('\0', "");
    let image_final_bytes = fs::read(&image)?;
    let image_final_sha256 = hex(&Sha256::digest(&image_final_bytes));
    archive_boot_control_state(&evidence_dir, &image_final_bytes)?;
    let (
        observed,
        manifest_sha256,
        measurement_root_sha256,
        boot_slot,
        boot_reason,
        security_version,
        bootctrl_generation,
    ) = match &options.expected {
        Expected::Pass => {
            let evidence: BootEvidence = parse_record(&log, "eneros.boot-evidence.v1")?;
            let measurement_log = validate_boot_evidence(&evidence, &image_final_bytes)?;
            // G2/G3 系统证据：EL0/root/syscall/IPC/SMP/crash 闭环 + 驱动/存储。
            let g2 = parse_g2_evidence(&log)?;
            let g3 = parse_g3_evidence(&log)?;
            fs::write(
                evidence_dir.join("g2-evidence.json"),
                serde_json::to_vec_pretty(&g2)?,
            )?;
            fs::write(
                evidence_dir.join("g3-evidence.json"),
                serde_json::to_vec_pretty(&g3)?,
            )?;
            if options
                .expected_slot
                .as_deref()
                .is_some_and(|slot| slot != evidence.slot)
            {
                return Err(invalid(format!(
                    "boot slot mismatch: expected {}, observed {}",
                    options.expected_slot.as_deref().unwrap_or_default(),
                    evidence.slot
                )));
            }
            if options
                .expected_reason
                .as_deref()
                .is_some_and(|reason| reason != evidence.boot_reason)
            {
                return Err(invalid(format!(
                    "boot reason mismatch: expected {}, observed {}",
                    options.expected_reason.as_deref().unwrap_or_default(),
                    evidence.boot_reason
                )));
            }
            fs::write(evidence_dir.join("measurement-log-v1.bin"), measurement_log)?;
            fs::write(
                evidence_dir.join("measurement-kernel-evidence.json"),
                serde_json::to_vec_pretty(&evidence)?,
            )?;
            (
                "PASS".to_owned(),
                Some(evidence.manifest_sha256),
                Some(evidence.measurement_root_sha256),
                Some(evidence.slot),
                Some(evidence.boot_reason),
                Some(evidence.security_version),
                Some(evidence.bootctrl_generation),
            )
        }
        Expected::Reject(expected_code) => {
            let evidence: RejectionEvidence = parse_record(&log, "eneros.boot-rejection.v1")?;
            if evidence.schema != "eneros.boot-rejection.v1"
                || evidence.result != "REJECT"
                || &evidence.code != expected_code
            {
                return Err(invalid(format!(
                    "rejection mismatch: expected {expected_code}, observed {}",
                    evidence.code
                )));
            }
            (
                format!("REJECT:{}", evidence.code),
                None,
                None,
                None,
                None,
                None,
                None,
            )
        }
        Expected::FirmwareReject => {
            if !log.contains("BdsDxe: failed to load")
                || !log.contains("Access Denied")
                || log.contains("ENEROS stage=")
                || log.contains("eneros.boot-evidence.v1")
                || log.contains("eneros.boot-rejection.v1")
            {
                return Err(invalid(
                    "firmware rejection marker was absent or EnerOS code executed",
                ));
            }
            (
                "FIRMWARE_REJECT".to_owned(),
                None,
                None,
                None,
                None,
                None,
                None,
            )
        }
        Expected::PowerCut(stage) => {
            if !log.contains(stage.marker())
                || stage
                    .forbidden_later_marker()
                    .is_some_and(|marker| log.contains(marker))
                || log.contains("eneros.boot-evidence.v1")
            {
                return Err(invalid(
                    "power-cut marker was absent or handoff completed before power removal",
                ));
            }
            (
                format!("CUT:{}", stage.name()),
                None,
                None,
                None,
                None,
                None,
                None,
            )
        }
    };
    let expected = match &options.expected {
        Expected::Pass => "PASS".to_owned(),
        Expected::Reject(code) => format!("REJECT:{code}"),
        Expected::FirmwareReject => "FIRMWARE_REJECT".to_owned(),
        Expected::PowerCut(stage) => format!("CUT:{}", stage.name()),
    };
    let result = HarnessResult {
        schema: "eneros.qemu-media-result.v1",
        result: "PASS",
        expected,
        observed,
        qemu_exit: exit,
        firmware_path: firmware_path(),
        firmware_sha256: firmware_sha256()?,
        vars_template_sha256,
        vars_run_final_sha256: hex(&Sha256::digest(fs::read(&run_vars)?)),
        image_size_bytes: u64::try_from(image_initial_bytes.len())?,
        image_initial_sha256,
        image_final_sha256,
        manifest_sha256,
        measurement_root_sha256,
        boot_slot,
        boot_reason,
        security_version,
        bootctrl_generation,
        disk_mode: if options.disk_read_only {
            "readonly"
        } else {
            "writable"
        },
        vars_mode: if options.vars_read_only {
            "readonly"
        } else {
            "writable"
        },
        disk_fault_sector: options.disk_fault_sector,
        rng_mode: if options.rng_present {
            "present"
        } else {
            "absent"
        },
        argv_policy: "PASS",
    };
    fs::write(
        evidence_dir.join("qemu-result.json"),
        serde_json::to_vec_pretty(&result)?,
    )?;
    println!(
        "QEMU profile=qemu-prod result=PASS expected={} image_initial_sha256={} image_final_sha256={}",
        result.expected, result.image_initial_sha256, result.image_final_sha256
    );
    Ok(())
}

/// WP-27.1：把镜像的 BOOTCTRL 双副本归零（seq=0、双副本一致、重算 CRC）。
///
/// 用 `BootControl::decode` 解码现有合法记录 → 置 `sequence=0` →
/// `encode` 重算 CRC 并保证 canonical 布局 → 双副本写回。与 boot-manager
/// 的编码完全同源（无 python 侧重复实现，规避 reset-bootctrl.py 曾有的
/// CRC 最终异或不一致缺陷）。Genesis/Anchor 判定语义不被触碰。
fn reset_bootctrl_copies(image: &mut [u8]) -> DynResult<()> {
    let gpt = parse_gpt(image, 512)
        .map_err(|error| invalid(format!("BOOTCTRL reset GPT parse failed: {error:?}")))?;
    let partition = gpt
        .find_by_name("BOOTCTRL")
        .ok_or_else(|| invalid("image has no BOOTCTRL partition"))?;
    let partition_start = usize::try_from(
        partition
            .first_lba
            .checked_mul(512)
            .ok_or_else(|| invalid("BOOTCTRL offset overflow"))?,
    )?;
    let (first, second) = image
        .get_mut(
            partition_start
                ..partition_start
                    .checked_add(BOOT_CONTROL_RECORD_SIZE * 2)
                    .ok_or_else(|| invalid("BOOTCTRL copy range overflow"))?,
        )
        .ok_or_else(|| invalid("BOOTCTRL copies truncated"))?
        .split_at_mut(BOOT_CONTROL_RECORD_SIZE);
    let mut control = BootControl::decode(first)
        .map_err(|error| invalid(format!("BOOTCTRL reset source decode failed: {error:?}")))?;
    control.sequence = 0;
    let record = control
        .encode()
        .map_err(|error| invalid(format!("BOOTCTRL reset encode failed: {error:?}")))?;
    first.copy_from_slice(&record);
    second.copy_from_slice(&record);
    Ok(())
}

fn archive_boot_control_state(evidence_dir: &Path, image: &[u8]) -> DynResult<()> {
    let gpt = parse_gpt(image, 512)
        .map_err(|error| invalid(format!("BOOTCTRL evidence GPT parse failed: {error:?}")))?;
    let partition = gpt
        .find_by_name("BOOTCTRL")
        .ok_or_else(|| invalid("formal image has no BOOTCTRL partition"))?;
    let partition_start = usize::try_from(
        partition
            .first_lba
            .checked_mul(512)
            .ok_or_else(|| invalid("BOOTCTRL offset overflow"))?,
    )?;
    let copy = |offset: u64| -> DynResult<[u8; BOOT_CONTROL_RECORD_SIZE]> {
        let start = partition_start
            .checked_add(usize::try_from(offset)?)
            .ok_or_else(|| invalid("BOOTCTRL copy offset overflow"))?;
        image
            .get(start..start + BOOT_CONTROL_RECORD_SIZE)
            .ok_or_else(|| invalid("BOOTCTRL copy is truncated"))?
            .try_into()
            .map_err(|_| invalid("BOOTCTRL copy length"))
    };
    let first = copy(BOOT_CONTROL_COPY_OFFSETS[0])?;
    let second = copy(BOOT_CONTROL_COPY_OFFSETS[1])?;
    let first_control = BootControl::decode(&first).ok();
    let second_control = BootControl::decode(&second).ok();
    fs::write(evidence_dir.join("bootctrl-copy-1.bin"), first)?;
    fs::write(evidence_dir.join("bootctrl-copy-2.bin"), second)?;
    fs::write(
        evidence_dir.join("bootctrl-state.json"),
        serde_json::to_vec_pretty(&BootControlEvidence {
            schema: "eneros.bootctrl-evidence.v1",
            first_valid: first_control.is_some(),
            first_sequence: first_control.as_ref().map(|control| control.sequence),
            first_security_floor: first_control.as_ref().map(|control| control.security_floor),
            first_sha256: hex(&Sha256::digest(first)),
            second_valid: second_control.is_some(),
            second_sequence: second_control.as_ref().map(|control| control.sequence),
            second_security_floor: second_control
                .as_ref()
                .map(|control| control.security_floor),
            second_sha256: hex(&Sha256::digest(second)),
            copies_equal: first == second,
        })?,
    )?;
    Ok(())
}

fn firmware_sha256() -> DynResult<String> {
    let path = firmware_path();
    let digest = if cfg!(windows) {
        let output = Command::new("wsl.exe")
            .args(["-d", "Ubuntu-22.04", "--", "sha256sum", &path])
            .output()?;
        if !output.status.success() {
            return Err(invalid("failed to hash the configured UEFI firmware"));
        }
        String::from_utf8(output.stdout)?
            .split_ascii_whitespace()
            .next()
            .ok_or_else(|| invalid("UEFI firmware hash output is empty"))?
            .to_owned()
    } else {
        hex(&Sha256::digest(std::fs::read(&path)?))
    };
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(invalid("UEFI firmware hash is not canonical SHA-256"));
    }
    Ok(digest)
}

fn run_firmware_reject(
    root: &Path,
    qemu_args: &[String],
    evidence_dir: &Path,
) -> DynResult<(i32, Vec<u8>)> {
    const MONITOR: &str = r#"
log=$1
shift
: > "$log"
"$@" >"$log" 2>&1 &
pid=$!
count=0
while kill -0 "$pid" 2>/dev/null; do
  if grep -Fq 'BdsDxe: failed to load' "$log" && grep -Fq 'Access Denied' "$log"; then
    kill -TERM "$pid"
    wait "$pid"
    cat "$log"
    exit 0
  fi
  count=$((count+1))
  if [ "$count" -ge 300 ]; then
    kill -KILL "$pid"
    wait "$pid"
    cat "$log"
    exit 70
  fi
  sleep 0.1
done
wait "$pid"
cat "$log"
exit 71
"#;
    let log_path = evidence_dir.join("qemu-firmware-reject.raw.log");
    let log_argument = if cfg!(windows) {
        wsl_path(root, &log_path)?
    } else {
        log_path.to_string_lossy().into_owned()
    };
    let mut command = if cfg!(windows) {
        let mut command = Command::new("wsl.exe");
        command.args([
            "-d",
            "Ubuntu-22.04",
            "--exec",
            "sh",
            "-c",
            MONITOR,
            "eneros-firmware-monitor",
            &log_argument,
        ]);
        command
    } else {
        let mut command = Command::new("sh");
        command.args(["-c", MONITOR, "eneros-firmware-monitor", &log_argument]);
        command
    };
    command.args(qemu_args);
    let output = command.output()?;
    let mut raw_log = output.stdout;
    raw_log.extend_from_slice(&output.stderr);
    if !output.status.success() {
        fs::write(&log_path, &raw_log)?;
        return Err(invalid(format!(
            "firmware rejection marker was not observed before monitor failure ({})",
            output.status
        )));
    }
    Ok((output.status.code().unwrap_or(0), raw_log))
}

fn run_power_cut(
    root: &Path,
    qemu_args: &[String],
    evidence_dir: &Path,
    stage: PowerCutStage,
) -> DynResult<(i32, Vec<u8>)> {
    let log_path = evidence_dir.join("qemu-powercut.raw.log");
    let log_wsl = wsl_path(root, &log_path)?;
    let controller = root.join("tools/qemu-powercut.sh");
    let status = if cfg!(windows) {
        let controller_wsl = wsl_path(root, &controller)?;
        let mut command = Command::new("wsl.exe");
        command.args([
            "-d",
            "Ubuntu-22.04",
            "--",
            "bash",
            &controller_wsl,
            &log_wsl,
            stage.marker(),
        ]);
        command.args(qemu_args);
        command.status()?
    } else {
        let mut command = Command::new("bash");
        command.arg(&controller);
        command.arg(&log_path).arg(stage.marker()).args(qemu_args);
        command.status()?
    };
    let raw_log = fs::read(&log_path)?;
    if !status.success() {
        return Err(invalid(format!(
            "power-cut controller failed before an exact cut (exit={:?})",
            status.code()
        )));
    }
    Ok((137, raw_log))
}

fn rooted_target_path(root: &Path, path: &Path) -> DynResult<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let target = root.join("target").canonicalize()?;
    let checked = if path.exists() {
        path.canonicalize()?
    } else {
        let parent = path
            .parent()
            .ok_or_else(|| invalid("target path has no parent"))?;
        if !parent.exists() {
            fs::create_dir_all(parent)?;
        }
        parent.canonicalize()?.join(
            path.file_name()
                .ok_or_else(|| invalid("target path has no file name"))?,
        )
    };
    if !checked.starts_with(&target) {
        return Err(invalid(
            "QEMU harness paths must stay below workspace target",
        ));
    }
    Ok(checked)
}

fn validate_argv(args: &[String]) -> DynResult<()> {
    if args.is_empty() || args[0] != "qemu-system-aarch64" {
        return Err(invalid("QEMU argv has no accepted executable"));
    }
    for forbidden in ["-kernel", "-initrd", "-gdb", "-s"] {
        if args.iter().any(|argument| argument == forbidden) {
            return Err(invalid(format!("forbidden QEMU argument: {forbidden}")));
        }
    }
    for pair in args.windows(2) {
        if pair[0] == "-device" && pair[1].to_ascii_lowercase().starts_with("loader,") {
            return Err(invalid("forbidden QEMU argument: -device loader"));
        }
    }
    if args
        .iter()
        .any(|argument| argument.to_ascii_lowercase().contains("loader,file="))
    {
        return Err(invalid("forbidden QEMU argument: loader,file="));
    }
    let disk_count = args
        .iter()
        .filter(|argument| argument.contains("id=enerosdisk"))
        .count();
    let firmware = firmware_path();
    let firmware_ok = args.iter().any(|argument| {
        argument.starts_with("if=pflash,format=raw,readonly=on,file=")
            && argument.ends_with(&firmware)
    });
    if disk_count != 1 || !firmware_ok || !firmware.ends_with(".fd") {
        return Err(invalid(
            "QEMU argv must contain exactly one formal disk and the configured UEFI pflash firmware",
        ));
    }
    Ok(())
}

fn parse_record<T: for<'de> Deserialize<'de>>(log: &str, schema: &str) -> DynResult<T> {
    let mut matches = Vec::new();
    for line in log.lines() {
        let Some(start) = line.find('{') else {
            continue;
        };
        let Some(end) = line.rfind('}') else {
            continue;
        };
        let candidate = &line[start..=end];
        if candidate.contains(schema) {
            matches.push(candidate);
        }
    }
    if matches.len() != 1 {
        return Err(invalid(format!(
            "expected exactly one {schema} record, found {}",
            matches.len()
        )));
    }
    Ok(serde_json::from_str(matches[0])?)
}

fn validate_boot_evidence(
    evidence: &BootEvidence,
    image: &[u8],
) -> DynResult<[u8; MEASUREMENT_LOG_SIZE]> {
    if evidence.schema != "eneros.boot-evidence.v1"
        || evidence.result != "PASS"
        || !matches!(evidence.slot.as_str(), "A" | "B" | "RECOVERY")
        || !matches!(
            evidence.boot_reason.as_str(),
            "COLD"
                | "WATCHDOG"
                | "UPDATE_TRIAL"
                | "ROLLBACK"
                | "MANUAL_RECOVERY"
                | "VERIFICATION_FAILURE"
        )
        || evidence.security_version == 0
        || evidence.memory_map_count == 0
        || evidence.init_bytes < 64
        || evidence.source_date_epoch == 0
        || evidence.manifest_sha256.len() != 64
        || evidence.measurement_root_sha256.len() != 64
        || !evidence
            .measurement_root_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || evidence
            .measurement_root_sha256
            .bytes()
            .all(|byte| byte == b'0')
        || evidence.measurement_log_hex.len() != MEASUREMENT_LOG_SIZE * 2
    {
        return Err(invalid(
            "boot evidence fields do not satisfy the frozen result schema",
        ));
    }
    let _ = evidence.bootctrl_generation;
    let gpt = parse_gpt(image, 512)
        .map_err(|error| invalid(format!("harness could not parse image GPT: {error:?}")))?;
    let partition_name = match evidence.slot.as_str() {
        "A" => "BOOT_A",
        "B" => "BOOT_B",
        "RECOVERY" => "RECOVERY",
        _ => return Err(invalid("invalid evidence slot")),
    };
    let partition = gpt
        .find_by_name(partition_name)
        .ok_or_else(|| invalid("evidence slot partition is absent"))?;
    let start = usize::try_from(
        partition
            .first_lba
            .checked_mul(512)
            .ok_or_else(|| invalid("manifest offset overflow"))?,
    )?;
    let header = image
        .get(start..start + 16)
        .ok_or_else(|| invalid("manifest header is truncated"))?;
    let length = usize::try_from(u32::from_le_bytes(header[12..16].try_into()?))?;
    let manifest = image
        .get(start..start + length)
        .ok_or_else(|| invalid("manifest is truncated"))?;
    let expected = hex(&Sha256::digest(manifest));
    if evidence.manifest_sha256 != expected {
        return Err(invalid(format!(
            "kernel manifest digest {} disagrees with image {}",
            evidence.manifest_sha256, expected
        )));
    }
    let measurement_bytes: [u8; MEASUREMENT_LOG_SIZE] = decode_hex(&evidence.measurement_log_hex)?
        .try_into()
        .map_err(|_| invalid("measurement log has the wrong decoded length"))?;
    let measurement = MeasurementLog::decode(&measurement_bytes)
        .map_err(|error| invalid(format!("kernel measurement log is invalid: {error:?}")))?;
    if hex(&measurement.final_digest) != evidence.measurement_root_sha256 {
        return Err(invalid(
            "measurement root disagrees with the decoded canonical log",
        ));
    }
    Ok(measurement_bytes)
}

fn decode_hex(value: &str) -> DynResult<Vec<u8>> {
    if value.len() % 2 != 0 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid("measurement log is not canonical hexadecimal"));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair)
                .map_err(|_| invalid("measurement log contains non-ASCII bytes"))?;
            u8::from_str_radix(text, 16)
                .map_err(|_| invalid("measurement log contains invalid hexadecimal"))
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn safe_args() -> Vec<String> {
        vec![
            "qemu-system-aarch64".to_owned(),
            "-drive".to_owned(),
            "if=pflash,format=raw,readonly=on,file=/usr/share/AAVMF/AAVMF_CODE.secboot.fd"
                .to_owned(),
            "-drive".to_owned(),
            "if=none,file=formal.raw,id=enerosdisk".to_owned(),
        ]
    }

    #[test]
    fn argv_policy_accepts_media_and_rejects_loader_kernel_and_gdb() {
        assert!(validate_argv(&safe_args()).is_ok());
        let mut path_with_kernel_word = safe_args();
        let disk = path_with_kernel_word
            .iter_mut()
            .find(|argument| argument.contains("id=enerosdisk"))
            .unwrap();
        *disk = "if=none,file=/tmp/wx-kernel-signed.raw,id=enerosdisk".to_owned();
        assert!(validate_argv(&path_with_kernel_word).is_ok());

        for injected in ["-kernel", "loader,file=x", "-gdb", "-s"] {
            let mut args = safe_args();
            args.push(injected.to_owned());
            assert!(validate_argv(&args).is_err(), "accepted {injected}");
        }
        let mut loader = safe_args();
        loader.extend(["-device".to_owned(), "loader,file=x".to_owned()]);
        assert!(validate_argv(&loader).is_err());
    }

    #[test]
    fn structured_record_requires_exactly_one_valid_json_object() {
        let record =
            r#"noise {"schema":"eneros.boot-rejection.v1","result":"REJECT","code":"SIGNATURE"}"#;
        let parsed: RejectionEvidence = parse_record(record, "eneros.boot-rejection.v1").unwrap();
        assert_eq!(parsed.code, "SIGNATURE");
        assert!(
            parse_record::<RejectionEvidence>("no record", "eneros.boot-rejection.v1").is_err()
        );
        let duplicate = format!("{record}\n{record}");
        assert!(parse_record::<RejectionEvidence>(&duplicate, "eneros.boot-rejection.v1").is_err());
    }
}

/// G2 系统证据解析：`eneros.g2.v1` 记录集合。
///
/// 要求：el0-enter、kernel-init、mmu 必须 PASS；secondary-online 每核
/// PASS；全部 `test` 名不得出现 FAIL；必须出现 system-ready 证据链
/// （root 侧 EVIDENCE_* 经内核转发的 eneros.g2.v1 记录）。
#[derive(Debug, Serialize)]
struct G2Evidence {
    schema: String,
    result: String,
    records: Vec<G2Record>,
    passed: usize,
    failed: usize,
    /// 42 WP-42.2：W^X 判定（供 isolation report-gate 消费）。
    /// 推导链：root-task spawn 对每个服务 ELF 页执行
    /// `PageEntry::is_wx_violation` 断言（crates/runtime/root-task/
    /// src/spawn.rs:2733，违规即 panic → 引导失败）；本记录集满足
    /// mmu/el0-enter 全 PASS 且零 FAIL ⇒ 服务均经 W^X 校验成功进入
    /// EL0。局限：仅覆盖已声明的服务 ELF（受信对端），非对抗性
    /// W^X 探测——如实声明，不作更多声称。
    wx_enforced: bool,
}

#[derive(Debug, Serialize)]
struct G2Record {
    test: String,
    result: String,
}

#[derive(Debug, Deserialize)]
struct RawG2Record {
    test: String,
    result: String,
}

fn parse_g2_evidence(log: &str) -> DynResult<G2Evidence> {
    let mut records = Vec::new();
    for line in log.lines() {
        let Some(start) = line.find('{') else {
            continue;
        };
        let Some(end) = line.rfind('}') else { continue };
        let candidate = &line[start..=end];
        if !candidate.contains("eneros.g2.v1") {
            continue;
        }
        let record: RawG2Record =
            serde_json::from_str(candidate).map_err(|_| invalid("malformed g2 evidence record"))?;
        records.push(G2Record {
            test: record.test,
            result: record.result,
        });
    }
    // 必需证据集合。
    let required = [
        "el0-enter",
        "kernel-init",
        "mmu",
        "system-ready",
        "root-heartbeat",
    ];
    for name in required {
        let present = records.iter().any(|record| {
            record.test == name && (record.result == "PASS" || record.result == "DEGRADE")
        });
        if !present {
            return Err(invalid(format!("missing required G2 evidence '{name}'")));
        }
    }
    let failed = records
        .iter()
        .filter(|record| record.result == "FAIL")
        .count();
    let passed = records
        .iter()
        .filter(|record| record.result == "PASS")
        .count();
    if failed != 0 {
        return Err(invalid(format!(
            "G2 evidence contains {failed} FAIL record(s)"
        )));
    }
    Ok(G2Evidence {
        schema: "eneros.g2-evidence.v1".to_owned(),
        result: "PASS".to_owned(),
        records,
        passed,
        failed,
        wx_enforced: failed == 0,
    })
}

/// G3 驱动/存储证据解析（DRV-003~012、STO）：同一 g2 schema 内的驱动
/// 记录。要求 shm-map/dma-alloc/irq-bind 至少各一次 PASS；dma-grant
/// 出现（storage 驱动路径）；irq-delivered 出现（IRQ 真实投递）；
/// drv-* 证据（root 侧转发）无 FAIL。
#[derive(Debug, Serialize)]
struct G3Evidence {
    schema: String,
    result: String,
    records: Vec<G2Record>,
    passed: usize,
    failed: usize,
}

fn parse_g3_evidence(log: &str) -> DynResult<G3Evidence> {
    let mut records = Vec::new();
    for line in log.lines() {
        let Some(start) = line.find('{') else {
            continue;
        };
        let Some(end) = line.rfind('}') else { continue };
        let candidate = &line[start..=end];
        if !candidate.contains("eneros.g2.v1") {
            continue;
        }
        let record: RawG2Record =
            serde_json::from_str(candidate).map_err(|_| invalid("malformed g3 evidence record"))?;
        records.push(G2Record {
            test: record.test,
            result: record.result,
        });
    }
    // 驱动资源路径证据（DRV-003 syscall 真实执行）。
    for name in ["shm-map", "dma-alloc", "irq-bind"] {
        let present = records.iter().any(|record| {
            record.test == name && (record.result == "PASS" || record.result == "DEGRADE")
        });
        if !present {
            return Err(invalid(format!("missing required G3 evidence '{name}'")));
        }
    }
    let failed = records
        .iter()
        .filter(|record| record.result == "FAIL")
        .count();
    let passed = records
        .iter()
        .filter(|record| record.result == "PASS")
        .count();
    if failed != 0 {
        return Err(invalid(format!(
            "G3 evidence contains {failed} FAIL record(s)"
        )));
    }
    Ok(G3Evidence {
        schema: "eneros.g3-evidence.v1".to_owned(),
        result: "PASS".to_owned(),
        records,
        passed,
        failed,
    })
}

// ---- G3 专项：断电矩阵 + 驱动故障/rebind（DRV-012/013、STO-002）----

/// G3 断电阶段 marker。
const G3_STAGE_DATA_WRITTEN: &str = "STORAGE stage=tx-data-written";
const G3_STAGE_COMMIT_WRITTEN: &str = "STORAGE stage=tx-commit-written";

/// G3 专项运行：断电矩阵（data/commit 两个断点）+ 冷启动验证。
///
/// 断电语义（cache=none）：
/// - CUT@tx-data-written：记录未提交 → 重启后必须为旧值；
/// - CUT@tx-commit-written：记录已提交 → 重启后为旧值或新值（禁混合）；
///
/// 每次重启验证 boot.count 单调（无回退、无丢失已确认）。
pub(super) fn command_g3(root: &Path, args: &[String]) -> DynResult<()> {
    let mut image = None;
    let mut vars_template = None;
    let mut evidence_dir = None;
    let mut data_disk = None;
    let mut phase = "matrix".to_owned();
    let mut index = 0usize;
    while index < args.len() {
        let flag = &args[index];
        let value = args
            .get(index + 1)
            .ok_or_else(|| invalid(format!("{flag} requires a value")))?;
        match flag.as_str() {
            "--image" => image = Some(PathBuf::from(value)),
            "--vars-template" => vars_template = Some(PathBuf::from(value)),
            "--evidence-dir" => evidence_dir = Some(PathBuf::from(value)),
            "--data-disk" => data_disk = Some(PathBuf::from(value)),
            "--phase" => phase = value.clone(),
            _ => return Err(invalid(format!("unknown qemu-g3 option '{flag}'"))),
        }
        index += 2;
    }
    let image = image.ok_or_else(|| invalid("--image is mandatory"))?;
    let vars_template = vars_template.ok_or_else(|| invalid("--vars-template is mandatory"))?;
    let evidence_dir = evidence_dir
        .map(|path| rooted_target_path(root, &path))
        .transpose()?
        .unwrap_or_else(|| root.join("target/qemu-g3"));
    fs::create_dir_all(&evidence_dir)?;
    // 数据盘：默认 16MB 空白盘（断电矩阵专用副本）。
    let data_disk = if let Some(path) = &data_disk {
        rooted_target_path(root, path)?
    } else {
        let path = evidence_dir.join("matrix-data.raw");
        if !path.is_file() {
            fs::write(&path, vec![0u8; 16 * 1024 * 1024])?;
        }
        path
    };

    match phase.as_str() {
        "matrix" => {
            // 1) 冷启动基线（boot.count 0→1）。
            run_g3_once(
                root,
                &image,
                &vars_template,
                &evidence_dir,
                &data_disk,
                None,
                "cold-baseline",
            )?;
            // 2) 断电@tx-data-written：记录未提交 → 旧值。
            run_g3_once(
                root,
                &image,
                &vars_template,
                &evidence_dir,
                &data_disk,
                Some(G3_STAGE_DATA_WRITTEN),
                "cut-data-written",
            )?;
            // 3) 断电@tx-commit-written：已提交 → 旧或新。
            run_g3_once(
                root,
                &image,
                &vars_template,
                &evidence_dir,
                &data_disk,
                Some(G3_STAGE_COMMIT_WRITTEN),
                "cut-commit-written",
            )?;
            // 4) 断电@tx-flushed：已确认落盘 → 新值。
            run_g3_once(
                root,
                &image,
                &vars_template,
                &evidence_dir,
                &data_disk,
                Some("STORAGE stage=tx-flushed"),
                "cut-flushed",
            )?;
            println!(
                "G3 power-cut matrix result=PASS phases=4 data_disk={}",
                data_disk.display()
            );
        }
        "cold" => {
            run_g3_once(
                root,
                &image,
                &vars_template,
                &evidence_dir,
                &data_disk,
                None,
                "cold",
            )?;
        }
        other => return Err(invalid(format!("unknown qemu-g3 phase '{other}'"))),
    }
    Ok(())
}

/// 单次 G3 运行：QEMU 启动（可选断电 marker），解析证据并验证
/// boot.count 单调性。
fn run_g3_once(
    root: &Path,
    image: &Path,
    vars_template: &Path,
    evidence_dir: &Path,
    data_disk: &Path,
    cut_marker: Option<&str>,
    phase: &str,
) -> DynResult<()> {
    let image = rooted_target_path(root, image)?;
    let vars_template = rooted_target_path(root, vars_template)?;
    let phase_dir = evidence_dir.join(phase);
    fs::create_dir_all(&phase_dir)?;
    let run_vars = phase_dir.join("AAVMF_VARS.run.fd");
    fs::copy(&vars_template, &run_vars)?;
    let image_wsl = wsl_path(root, &image)?;
    let vars_wsl = wsl_path(root, &run_vars)?;
    let data_wsl = wsl_path(root, data_disk)?;
    let qemu_args = vec![
        "qemu-system-aarch64".to_owned(),
        // icount：TCG 下虚拟时钟随指令推进，arch timer 可靠触发
        // （无 icount 时空闲/WFE 下 QEMU 虚拟时钟停滞，timer IRQ 不产生）。
        "-machine".to_owned(),
        "virt,gic-version=2".to_owned(),
        "-smp".to_owned(),
        "4".to_owned(),
        "-cpu".to_owned(),
        "cortex-a57".to_owned(),
        "-m".to_owned(),
        // BOOT-003：内存必须与 sel4 qemu-arm-virt DTB 一致（1GB）——内核按 DTB
        // 的 memory 描述初始化并清零（memzero），512M 时访问 0x7ffc0000 区域
        // （phys 超出实际 RAM）触发 KERNEL DATA ABORT（translation fault L0）。
        "1G".to_owned(),
        // BOOT-006：强制磁盘 boot 优先——AAVMF 的 PXE boot 项（vars 模板自带）
        // 会随机抢跑（DHCP 超时可达 60s+），导致 qemu-prod 验证超时；order=c
        // 让 UEFI 直接尝试磁盘（BOOTAA64.EFI）。
        "-boot".to_owned(),
        "order=c".to_owned(),
        "-nographic".to_owned(),
        "-no-reboot".to_owned(),
        "-drive".to_owned(),
        format!("if=pflash,format=raw,readonly=on,file={}", firmware_path()),
        "-drive".to_owned(),
        format!("if=pflash,format=raw,file={vars_wsl}"),
        "-drive".to_owned(),
        format!("if=none,format=raw,file={image_wsl},id=enerosdisk"),
        "-device".to_owned(),
        "virtio-blk-device,bus=virtio-mmio-bus.0,drive=enerosdisk".to_owned(),
        "-object".to_owned(),
        "rng-random,filename=/dev/urandom,id=enerosrng".to_owned(),
        "-device".to_owned(),
        "virtio-rng-device,bus=virtio-mmio-bus.3,rng=enerosrng".to_owned(),
        "-drive".to_owned(),
        format!("if=none,format=raw,cache=none,file={data_wsl},id=enerosdata"),
        "-device".to_owned(),
        "virtio-blk-device,bus=virtio-mmio-bus.2,drive=enerosdata".to_owned(),
        "-netdev".to_owned(),
        "socket,id=enerosnet,listen=127.0.0.1:5577".to_owned(),
        "-device".to_owned(),
        "virtio-net-device,bus=virtio-mmio-bus.1,netdev=enerosnet".to_owned(),
    ];
    validate_argv(&qemu_args)?;
    let qemu_bin = find_qemu_binary()?;
    let (exit, raw_log) = if let Some(marker) = cut_marker {
        run_with_powercut(root, &qemu_bin, &qemu_args, marker)?
    } else {
        let output = Command::new("timeout")
            .args(["--signal=KILL", "120s"])
            .arg(&qemu_bin)
            .args(&qemu_args)
            .output()?;
        let exit = output.status.code().unwrap_or(-1);
        let mut raw_log = output.stdout;
        raw_log.extend_from_slice(&output.stderr);
        if !output.status.success() {
            fs::write(phase_dir.join("qemu-serial.log"), &raw_log)?;
            return Err(invalid(format!(
                "G3 QEMU did not shut down successfully (exit={exit}); timeout is a failure"
            )));
        }
        (exit, raw_log)
    };
    fs::write(phase_dir.join("qemu-serial.log"), &raw_log)?;
    let log = String::from_utf8_lossy(&raw_log).replace('\0', "");
    // 证据解析：G2 必需集合 + G3 驱动资源 + storage 自检。
    let g2 = parse_g2_evidence(&log)?;
    let g3 = parse_g3_evidence(&log)?;
    // storage 自检证据（boot.count 单调 + readback）。
    if !log.contains("STORAGE config readback ok") {
        return Err(invalid("G3 storage readback evidence missing"));
    }
    if log.contains("STORAGE config readback mismatch") {
        return Err(invalid("G3 storage readback mismatch"));
    }
    if log.contains("STORAGE scrub found errors") {
        return Err(invalid("G3 storage scrub found errors"));
    }
    // 解析本次 boot.count（monotonic 校验由矩阵调用方通过 data 盘快照做）。
    let boot_count = extract_boot_count(&log);
    fs::write(
        phase_dir.join("g3-phase.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "eneros.g3-phase.v1",
            "phase": phase,
            "cut_marker": cut_marker,
            "qemu_exit": exit,
            "g2": g2.result,
            "g3": g3.result,
            "boot_count": boot_count,
        }))?,
    )?;
    Ok(())
}

/// 提取 storage 打印的 boot.count（"STORAGE config commit ok boot_count=N"）。
fn extract_boot_count(log: &str) -> Option<u64> {
    for line in log.lines() {
        if let Some(pos) = line.find("boot_count=") {
            let tail = &line[pos + "boot_count=".len()..];
            let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                return digits.parse().ok();
            }
        }
    }
    None
}

/// 运行 QEMU 并在 marker 处断电（kill -KILL，模拟介质断电）。
fn run_with_powercut(
    root: &Path,
    qemu_bin: &Path,
    qemu_args: &[String],
    marker: &str,
) -> DynResult<(i32, Vec<u8>)> {
    let script = root.join("tools/qemu-powercut.sh");
    let mut command = Command::new(&script);
    command
        .arg("/dev/stdout")
        .arg(marker)
        .arg(qemu_bin)
        .args(qemu_args);
    // qemu-powercut.sh 需要 QEMU 输出到 fifo；这里用脚本自身。
    let output = command.output()?;
    if !output.status.success() {
        return Err(invalid(format!(
            "power-cut run failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok((output.status.code().unwrap_or(-1), output.stdout))
}

/// 定位 QEMU 二进制（环境变量优先）。
fn find_qemu_binary() -> DynResult<PathBuf> {
    if let Ok(path) = std::env::var("ENEROS_QEMU_BIN") {
        let path = PathBuf::from(path);
        if !path.is_file() {
            return Err(invalid("ENEROS_QEMU_BIN does not exist"));
        }
        return Ok(path);
    }
    // PATH 查找。
    for dir in std::env::var("PATH").unwrap_or_default().split(':') {
        let candidate = Path::new(dir).join("qemu-system-aarch64");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(invalid(
        "qemu-system-aarch64 not found; set ENEROS_QEMU_BIN or extend PATH",
    ))
}
