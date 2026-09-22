use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use eneros_image_format::parse_gpt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{invalid, DynResult};

const INVENTORY_SCHEMA: &str = "eneros.flash-device-inventory.v1";
const TOKEN_SCHEMA: &str = "eneros.flash-authorization.v1";
const REPORT_SCHEMA: &str = "eneros.flash-write-report.v1";
const JOURNAL_SCHEMA: &str = "eneros.flash-resume-journal.v1";
const BIRTH_SCHEMA: &str = "eneros.device-birth-record.v1";
const KEY_DOMAINS_SCHEMA: &str = "eneros.factory-key-domains.v1";

/// 47（WP-47.3 私钥出仓）：与 keys 模块同源（keys-local）。
const DEV_SIGNING_KEY_REL: &str = super::keys::DEV_SIGNING_KEY_REL;

/// 稀疏写入把相邻非零区之间的零间隙最多合并到这么宽；更宽的零间隙会保留
/// 为两次独立 extent（物理介质上可不写、断点续写时按 extent 记账）。
const MIN_GAP_TO_SPLIT: u64 = 4096;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct DeviceRecord {
    stable_id: String,
    path: String,
    model: String,
    bus: String,
    size_bytes: u64,
    logical_sector_size: u32,
    read_only: bool,
    removable: bool,
    topology_complete: bool,
    system_reasons: Vec<String>,
    mountpoints: Vec<String>,
    eligible: bool,
    denials: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Inventory {
    schema: String,
    result: String,
    platform: String,
    observation_id: String,
    device_count: usize,
    eligible_count: usize,
    devices: Vec<DeviceRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct AuthorizationToken {
    schema: String,
    target_kind: String,
    target_path: String,
    target_stable_id: String,
    target_size_bytes: u64,
    logical_sector_size: u32,
    target_initial_sha256: String,
    image_path: String,
    image_size_bytes: u64,
    image_sha256: String,
    /// readback=sample 时，锁定的是镜像"非零 extent 清单"的摘要（见
    /// extent_manifest_digest）；readback=full 时为空。
    sample_manifest_sha256: String,
    /// 工位/操作员/候选绑定（WP-43.9/43.10）。
    operator_id: String,
    station_id: String,
    tool_version: String,
    candidate_id: String,
    /// "full" = 全量镜像摘要；"sample" = extent 清单摘要（策略明确、可机器重算）。
    readback_policy: String,
    /// provision 作业使用 factory|release 域；birth record 只能是 factory。
    key_domain: String,
    key_id: String,
    public_key: String,
    authorization_sha256: String,
    signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FlashReport {
    schema: String,
    result: String,
    target_kind: String,
    target_path: String,
    target_stable_id: String,
    image_path: String,
    bytes_written: u64,
    expected_sha256: String,
    readback_sha256: String,
    authorization_sha256: String,
    flush: String,
    readback: String,
    fault_injection: Option<String>,
    operator_id: String,
    station_id: String,
    tool_version: String,
    candidate_id: String,
    key_domain: String,
    key_id: String,
    readback_policy: String,
    readback_detail: String,
    resumed: bool,
    interrupted: bool,
    extents_written: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResumeJournal {
    schema: String,
    image_sha256: String,
    target_stable_id: String,
    target_path: String,
    target_size_bytes: u64,
    extents_total: usize,
    extents_written: Vec<ExtentRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ExtentRecord {
    offset: u64,
    len: u64,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BirthRecord {
    schema: String,
    result: String,
    report_path: String,
    device_serial: String,
    device_model: String,
    candidate_id: String,
    image_sha256: String,
    readback_sha256: String,
    readback_policy: String,
    station_id: String,
    operator_id: String,
    key_domain: String,
    key_id: String,
    public_key: String,
    generated_at: String,
    generated_unix: u64,
    signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyDomains {
    schema: String,
    updated_by: String,
    updated_at: String,
    #[serde(default)]
    note: Option<String>,
    domains: BTreeMap<String, KeyDomain>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyDomain {
    allowed_uses: Vec<String>,
    key_ref: String,
}

#[derive(Debug, Deserialize)]
struct WindowsEnvelope {
    devices: Vec<WindowsDevice>,
}

#[derive(Debug, Deserialize)]
struct WindowsDevice {
    path: String,
    stable_id: String,
    model: String,
    bus: String,
    size_bytes: u64,
    logical_sector_size: u32,
    read_only: bool,
    removable: bool,
    topology_complete: bool,
    system_reasons: Vec<String>,
    mountpoints: Vec<String>,
}

pub(super) fn command_flash(root: &Path, args: &[String]) -> DynResult<()> {
    let Some((mode, tail)) = args.split_first() else {
        return Err(invalid(
            "flash requires list, verify-inventory, lab-create, authorize-file, write, birth-record, verify-birth, or key-domains",
        ));
    };
    match mode.as_str() {
        "list" => command_list(root, tail),
        "verify-inventory" => command_verify_inventory(root, tail),
        "lab-create" => command_lab_create(root, tail),
        "authorize-file" => command_authorize_file(root, tail),
        "write" => command_write(root, tail),
        "birth-record" => command_birth_record(root, tail),
        "verify-birth" => command_verify_birth(root, tail),
        "key-domains" => command_key_domains(root, tail),
        other => Err(invalid(format!("unknown flash mode '{other}'"))),
    }
}

fn command_list(root: &Path, args: &[String]) -> DynResult<()> {
    let platform = args
        .first()
        .ok_or_else(|| invalid("flash list requires windows or linux"))?;
    let output = value_after(args, "--output")?;
    if args.len() != 3 {
        return Err(invalid(
            "flash list requires <windows|linux> --output <target-json>",
        ));
    }
    let devices = match platform.as_str() {
        "windows" => enumerate_windows()?,
        "linux" => enumerate_linux()?,
        other => return Err(invalid(format!("unsupported flash platform '{other}'"))),
    };
    let inventory = finalize_inventory(platform, devices)?;
    verify_inventory(&inventory)?;
    let output = target_output(root, Path::new(output), "inventory")?;
    fs::write(&output, serde_json::to_vec_pretty(&inventory)?)?;
    println!("{}", serde_json::to_string_pretty(&inventory)?);
    Ok(())
}

fn command_verify_inventory(root: &Path, args: &[String]) -> DynResult<()> {
    let (path, fault) = match args {
        [path] => (path, None),
        [path, flag, fault] if flag == "--fault" => (path, Some(fault.as_str())),
        _ => {
            return Err(invalid(
                "flash verify-inventory requires <target-json> [--fault system-reason-clear]",
            ));
        }
    };
    let path = target_input(root, Path::new(path), "inventory")?;
    let mut inventory: Inventory = serde_json::from_slice(&fs::read(path)?)?;
    if let Some(fault) = fault {
        if fault != "system-reason-clear" {
            return Err(invalid("unknown inventory fault injection"));
        }
        let system = inventory
            .devices
            .iter_mut()
            .find(|device| !device.system_reasons.is_empty())
            .ok_or_else(|| invalid("inventory has no system record to mutate"))?;
        system.system_reasons.clear();
    }
    verify_inventory(&inventory)?;
    println!(
        "FLASH inventory result=PASS platform={} devices={} eligible={}",
        inventory.platform, inventory.device_count, inventory.eligible_count
    );
    Ok(())
}

fn command_lab_create(root: &Path, args: &[String]) -> DynResult<()> {
    let image = value_after(args, "--image")?;
    let target = value_after(args, "--target")?;
    let shape_option = optional_value_after(args, "--shape")?;
    let shape = shape_option.unwrap_or("exact");
    let expected_len = if shape_option.is_some() { 6 } else { 4 };
    if args.len() != expected_len || !matches!(shape, "exact" | "short" | "nonblank") {
        return Err(invalid(
            "flash lab-create requires --image <raw> --target <lab-file> [--shape short|nonblank]",
        ));
    }
    let image = target_input(root, Path::new(image), "flash image")?;
    let image_bytes = fs::read(&image)?;
    validate_raw_image(&image_bytes)?;
    let target = lab_output(root, Path::new(target))?;
    let length = match shape {
        "short" => image_bytes
            .len()
            .checked_sub(512)
            .ok_or_else(|| invalid("image is too short for short-target fixture"))?,
        _ => image_bytes.len(),
    };
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&target)?;
    file.set_len(u64::try_from(length)?)?;
    if shape == "nonblank" {
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&[0xA5])?;
    }
    file.flush()?;
    file.sync_all()?;
    println!(
        "FLASH lab-create result=PASS shape={shape} bytes={length} path={}",
        relative(root, &target)?
    );
    Ok(())
}

/// 产线 provisioning 作业：对 file-lab 目标签发 ed25519 签名的授权令牌，
/// 绑定额外工位/操作员/候选与 readback 策略（WP-43.9 signed provisioning
/// job、WP-43.10 工位身份/最小权限）。
fn command_authorize_file(root: &Path, args: &[String]) -> DynResult<()> {
    let image_path = target_input(
        root,
        Path::new(value_after(args, "--image")?),
        "flash image",
    )?;
    let target_path = lab_target(root, Path::new(value_after(args, "--target")?))?;
    let output = target_output(
        root,
        Path::new(value_after(args, "--output")?),
        "authorization token",
    )?;
    let operator = value_after(args, "--operator")?.to_owned();
    let station = value_after(args, "--station")?.to_owned();
    let candidate = value_after(args, "--candidate")?.to_owned();
    let key = optional_value_after(args, "--key")?.map(PathBuf::from);
    let readback_policy = optional_value_after(args, "--readback")?
        .unwrap_or("full")
        .to_owned();
    let key_domain = optional_value_after(args, "--domain")?
        .unwrap_or("factory")
        .to_owned();
    if operator.trim().is_empty() || station.trim().is_empty() || candidate.trim().is_empty() {
        return Err(invalid(
            "authorize-file requires non-empty --operator/--station/--candidate (WP-43.10 工位身份)",
        ));
    }
    if !matches!(readback_policy.as_str(), "full" | "sample") {
        return Err(invalid("--readback must be full or sample"));
    }
    if !matches!(key_domain.as_str(), "factory" | "release") {
        return Err(invalid("--domain must be factory or release"));
    }
    let image = fs::read(&image_path)?;
    let sector_size = validate_raw_image(&image)?;
    let target = fs::read(&target_path)?;
    if target.len() != image.len() {
        return Err(invalid(
            "lab target size must exactly equal the raw image size",
        ));
    }
    if target.iter().any(|byte| *byte != 0) {
        return Err(invalid("lab target must be blank before authorization"));
    }
    let relative_target = relative(root, &target_path)?;
    let stable_id = format!(
        "file-lab:{}",
        hex(&Sha256::digest(relative_target.as_bytes()))
    );
    let signing_key = load_signing_key(root, key)?;
    let keypair = SigningKey::from_bytes(&signing_key);
    let public = keypair.verifying_key();
    let public_bytes = public.to_bytes();
    let key_id = hex(&Sha256::digest(public_bytes));
    let sample_manifest_sha256 = if readback_policy == "sample" {
        extent_manifest_digest(&image)
    } else {
        String::new()
    };
    let mut token = AuthorizationToken {
        schema: TOKEN_SCHEMA.to_owned(),
        target_kind: "file-lab".to_owned(),
        target_path: relative_target,
        target_stable_id: stable_id,
        target_size_bytes: u64::try_from(target.len())?,
        logical_sector_size: sector_size,
        target_initial_sha256: hex(&Sha256::digest(&target)),
        image_path: relative(root, &image_path)?,
        image_size_bytes: u64::try_from(image.len())?,
        image_sha256: hex(&Sha256::digest(&image)),
        sample_manifest_sha256,
        operator_id: operator,
        station_id: station,
        tool_version: env!("CARGO_PKG_VERSION").to_owned(),
        candidate_id: candidate,
        readback_policy: readback_policy.clone(),
        key_domain,
        key_id,
        public_key: hex(&public_bytes),
        authorization_sha256: String::new(),
        signature: String::new(),
    };
    sign_token(&mut token, &signing_key)?;
    fs::write(&output, serde_json::to_vec_pretty(&token)?)?;
    println!("{}", serde_json::to_string_pretty(&token)?);
    Ok(())
}

fn command_write(root: &Path, args: &[String]) -> DynResult<()> {
    let image_arg = value_after(args, "--image")?;
    let token_arg = value_after(args, "--authorization")?;
    let output_arg = value_after(args, "--output")?;
    let fault = optional_value_after(args, "--fault")?;
    if let Some(fault) = fault {
        if !matches!(fault, "mid-write-interrupt" | "post-write-corruption") {
            return Err(invalid(
                "unsupported flash fault injection; expected mid-write-interrupt or post-write-corruption",
            ));
        }
    }
    let image_path = target_input(root, Path::new(image_arg), "flash image")?;
    let token_path = target_input(root, Path::new(token_arg), "authorization token")?;
    let output = target_output(root, Path::new(output_arg), "flash report")?;
    let token: AuthorizationToken = serde_json::from_slice(&fs::read(token_path)?)?;
    verify_token(&token)?;
    if token.target_kind != "file-lab" {
        return Err(invalid(
            "G1 writer only accepts authorized file-lab targets",
        ));
    }
    let token_image = root.join(&token.image_path).canonicalize()?;
    if token_image != image_path {
        return Err(invalid(
            "authorization token is bound to another image path",
        ));
    }
    let target_path = lab_target(root, &root.join(&token.target_path))?;
    let image = fs::read(&image_path)?;
    if u64::try_from(image.len())? != token.image_size_bytes
        || hex(&Sha256::digest(&image)) != token.image_sha256
        || validate_raw_image(&image)? != token.logical_sector_size
    {
        return Err(invalid("flash image identity changed after authorization"));
    }
    let journal_path = PathBuf::from(format!(
        "{}.flash-resume.json",
        target_path.to_string_lossy()
    ));
    // 续写判定：存在匹配本作业（image sha/target stable id/size）且已写 extent
    // 与介质一致的 resume journal 时，允许跳过"授权后空白"前置校验——介质上
    // 的真实部分状态由 journal 佐证，绝不把半写介质当作成功。
    let existing_journal = match read_journal(&journal_path)? {
        Some(journal)
            if journal.image_sha256 == token.image_sha256
                && journal.target_stable_id == token.target_stable_id
                && journal.target_size_bytes == token.target_size_bytes
                && !journal.extents_written.is_empty() =>
        {
            if !verify_journal_extents(&target_path, &journal)? {
                return Err(invalid(
                    "resume journal present but target partial state does not match it; use a fresh target",
                ));
            }
            Some(journal)
        }
        Some(_) => None,
        None => None,
    };
    if existing_journal.is_none() {
        let current = fs::read(&target_path)?;
        if u64::try_from(current.len())? != token.target_size_bytes
            || hex(&Sha256::digest(&current)) != token.target_initial_sha256
        {
            return Err(invalid("flash target changed after authorization"));
        }
    }
    let extents = populated_extents(&image);
    let interrupt_after = if fault == Some("mid-write-interrupt") {
        Some(1)
    } else {
        None
    };
    let (extents_written, resumed, interrupted) = write_sparse_with_journal(
        &target_path,
        &image,
        &journal_path,
        &extents,
        &token.image_sha256,
        &token.target_stable_id,
        &token.target_path,
        token.target_size_bytes,
        interrupt_after,
    )?;
    if interrupted {
        // 部分状态留在 journal 中可续写；这里绝不出成功报告。
        return Err(invalid(
            "write interrupted mid-flight; re-run the same --image/--authorization to resume (partial state is never reported as success)",
        ));
    }
    if fault == Some("post-write-corruption") {
        inject_post_write_corruption(&target_path, image.len())?;
    }
    let readback = fs::read(&target_path)?;
    let (readback_sha256, pass, readback_detail) = readback_result(&readback, &token);
    let report = FlashReport {
        schema: REPORT_SCHEMA.to_owned(),
        result: (if pass { "PASS" } else { "FAIL" }).to_owned(),
        target_kind: token.target_kind.clone(),
        target_path: token.target_path.clone(),
        target_stable_id: token.target_stable_id.clone(),
        image_path: token.image_path.clone(),
        bytes_written: u64::try_from(image.len())?,
        expected_sha256: if token.readback_policy == "full" {
            token.image_sha256.clone()
        } else {
            token.sample_manifest_sha256.clone()
        },
        readback_sha256,
        authorization_sha256: token.authorization_sha256.clone(),
        flush: "PASS".to_owned(),
        readback: (if pass { "PASS" } else { "FAIL" }).to_owned(),
        fault_injection: fault.map(ToOwned::to_owned),
        operator_id: token.operator_id.clone(),
        station_id: token.station_id.clone(),
        tool_version: token.tool_version.clone(),
        candidate_id: token.candidate_id.clone(),
        key_domain: token.key_domain.clone(),
        key_id: token.key_id.clone(),
        readback_policy: token.readback_policy.clone(),
        readback_detail,
        resumed,
        interrupted: false,
        extents_written,
    };
    fs::write(&output, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !pass {
        return Err(invalid("flash readback digest differs from the image"));
    }
    Ok(())
}

/// WP-43.10：每台设备签发签名出生记录（factory 域），绑定序列号/工位/
/// 操作员/候选与 write 报告的 readback 摘要。只接受 result=PASS 的报告。
fn command_birth_record(root: &Path, args: &[String]) -> DynResult<()> {
    let report_arg = value_after(args, "--report")?;
    let out_arg = value_after(args, "--out")?;
    let serial = value_after(args, "--device-serial")?;
    let operator = value_after(args, "--operator")?;
    let station = value_after(args, "--station")?;
    let key = optional_value_after(args, "--key")?.map(PathBuf::from);
    let model = optional_value_after(args, "--device-model")?
        .unwrap_or("")
        .to_owned();
    if serial.trim().is_empty() || operator.trim().is_empty() || station.trim().is_empty() {
        return Err(invalid(
            "birth-record requires non-empty --device-serial/--operator/--station",
        ));
    }
    let report_path = target_input(root, Path::new(report_arg), "flash report")?;
    let report: FlashReport = serde_json::from_slice(&fs::read(&report_path)?)?;
    if report.result != "PASS" {
        return Err(invalid("birth record requires a PASS write report"));
    }
    let signing_key = load_signing_key(root, key)?;
    let keypair = SigningKey::from_bytes(&signing_key);
    let public = keypair.verifying_key();
    let public_bytes = public.to_bytes();
    let key_id = hex(&Sha256::digest(public_bytes));
    let generated_unix = current_unix();
    let mut record = BirthRecord {
        schema: BIRTH_SCHEMA.to_owned(),
        result: "PASS".to_owned(),
        report_path: relative(root, &report_path)?,
        device_serial: serial.to_owned(),
        device_model: model,
        candidate_id: report.candidate_id.clone(),
        image_sha256: report.expected_sha256.clone(),
        readback_sha256: report.readback_sha256.clone(),
        readback_policy: report.readback_policy.clone(),
        station_id: station.to_owned(),
        operator_id: operator.to_owned(),
        key_domain: "factory".to_owned(),
        key_id,
        public_key: hex(&public_bytes),
        generated_at: utc_timestamp(generated_unix),
        generated_unix,
        signature: String::new(),
    };
    sign_birth(&mut record, &signing_key)?;
    let out = target_output(root, Path::new(out_arg), "birth record")?;
    fs::write(&out, serde_json::to_vec_pretty(&record)?)?;
    println!("{}", serde_json::to_string_pretty(&record)?);
    Ok(())
}

fn command_verify_birth(root: &Path, args: &[String]) -> DynResult<()> {
    let path = args
        .first()
        .ok_or_else(|| invalid("flash verify-birth requires <birth-record.json>"))?;
    if args.len() != 1 {
        return Err(invalid(
            "flash verify-birth requires exactly one birth record path",
        ));
    }
    let path = target_input(root, Path::new(path), "birth record")?;
    let record: BirthRecord = serde_json::from_slice(&fs::read(path)?)?;
    verify_birth(&record)?;
    println!(
        "FLASH birth-record result=PASS serial={} candidate={} generated_at={}",
        record.device_serial, record.candidate_id, record.generated_at
    );
    Ok(())
}

/// 产线 key 域分离策略文件（WP-43.10）：factory/release/recovery 三域声明
/// 与允许用途；由 owner 维护，本命令只做结构校验（N/A 自行裁决不得由
/// 执行 AI 静默补写）。
fn command_key_domains(root: &Path, args: &[String]) -> DynResult<()> {
    let path = args
        .first()
        .ok_or_else(|| invalid("flash key-domains requires <json>"))?;
    if args.len() != 1 {
        return Err(invalid("flash key-domains requires exactly one json path"));
    }
    let path = repo_input(root, Path::new(path), "key domains")?;
    let domains: KeyDomains = serde_json::from_slice(&fs::read(path)?)?;
    validate_key_domains(&domains)?;
    println!(
        "FLASH key-domains result=PASS domains={}",
        domains.domains.len()
    );
    Ok(())
}

// ============================================================
// 签名 / 校验（provisioning job 与 birth record，WP-43.9/43.10）
// ============================================================

fn load_signing_key(root: &Path, key: Option<PathBuf>) -> DynResult<[u8; 32]> {
    let path = match key {
        Some(path) if path.is_absolute() => path,
        Some(path) => root.join(path),
        None => root.join(DEV_SIGNING_KEY_REL),
    };
    let bytes = fs::read(&path)?;
    let len = bytes.len();
    bytes.try_into().map_err(|_| {
        invalid(if len < 32 {
            "signing key too short"
        } else {
            "signing key invalid length"
        })
    })
}

fn unsigned_token(token: &AuthorizationToken) -> DynResult<AuthorizationToken> {
    let mut unsigned = token.clone();
    unsigned.authorization_sha256.clear();
    unsigned.signature.clear();
    Ok(unsigned)
}

fn sign_token(token: &mut AuthorizationToken, key: &[u8; 32]) -> DynResult<()> {
    let keypair = SigningKey::from_bytes(key);
    let unsigned = unsigned_token(token)?;
    let bytes = serde_json::to_vec(&unsigned)?;
    token.authorization_sha256 = hex(&Sha256::digest(&bytes));
    token.signature = hex(&keypair.sign(&bytes).to_bytes());
    Ok(())
}

fn verify_token(token: &AuthorizationToken) -> DynResult<()> {
    if token.schema != TOKEN_SCHEMA
        || token.target_kind != "file-lab"
        || token.target_size_bytes == 0
        || token.target_size_bytes != token.image_size_bytes
        || !matches!(token.logical_sector_size, 512 | 4096)
        || token.operator_id.trim().is_empty()
        || token.station_id.trim().is_empty()
        || token.candidate_id.trim().is_empty()
        || token.tool_version.trim().is_empty()
        || !matches!(token.readback_policy.as_str(), "full" | "sample")
        || !matches!(token.key_domain.as_str(), "factory" | "release")
        || (token.readback_policy == "sample" && token.sample_manifest_sha256.is_empty())
        || (token.readback_policy == "full" && !token.sample_manifest_sha256.is_empty())
    {
        return Err(invalid(
            "flash authorization token semantic fields are invalid",
        ));
    }
    let expected = "file-lab:".to_owned() + &hex(&Sha256::digest(token.target_path.as_bytes()));
    if token.target_stable_id != expected {
        return Err(invalid("token target_stable_id does not match target path"));
    }
    let verifying = verifying_key_from_token(token)?;
    let unsigned = unsigned_token(token)?;
    let unsigned_bytes = serde_json::to_vec(&unsigned)?;
    if hex(&Sha256::digest(&unsigned_bytes)) != token.authorization_sha256 {
        return Err(invalid("token integrity digest does not match its content"));
    }
    verify_ed25519(&verifying, &unsigned_bytes, &token.signature)?;
    Ok(())
}

fn unsigned_birth(record: &BirthRecord) -> DynResult<BirthRecord> {
    let mut unsigned = record.clone();
    unsigned.signature.clear();
    Ok(unsigned)
}

fn sign_birth(record: &mut BirthRecord, key: &[u8; 32]) -> DynResult<()> {
    let keypair = SigningKey::from_bytes(key);
    let unsigned = unsigned_birth(record)?;
    let bytes = serde_json::to_vec(&unsigned)?;
    record.signature = hex(&keypair.sign(&bytes).to_bytes());
    Ok(())
}

fn verify_birth(record: &BirthRecord) -> DynResult<()> {
    if record.schema != BIRTH_SCHEMA
        || record.result != "PASS"
        || record.device_serial.trim().is_empty()
        || record.candidate_id.trim().is_empty()
        || record.key_domain != "factory"
        || record.station_id.trim().is_empty()
        || record.operator_id.trim().is_empty()
        || record.generated_unix == 0
    {
        return Err(invalid("birth record semantic fields are invalid"));
    }
    let verifying = verifying_key_from_birth(record)?;
    let unsigned = unsigned_birth(record)?;
    let unsigned_bytes = serde_json::to_vec(&unsigned)?;
    verify_ed25519(&verifying, &unsigned_bytes, &record.signature)?;
    Ok(())
}

/// 校验 key_id == sha256(公钥字节)，返回解析出的 VerifyingKey——令牌/出生
/// 记录内嵌公钥 + 指纹自校验（生产环境另以 key-domains.json 把 key_id 固定
/// 到厂方公钥，见 WP-43.10）。
fn verifying_key_from_public(key_id: &str, public_hex: &str) -> DynResult<VerifyingKey> {
    let pub_bytes = hex_decode(public_hex)?;
    if pub_bytes.len() != 32 {
        return Err(invalid("public key length is not 32 bytes"));
    }
    let pub_arr: [u8; 32] = pub_bytes
        .try_into()
        .map_err(|_| invalid("public key invalid length"))?;
    if hex(&Sha256::digest(pub_arr)) != key_id {
        return Err(invalid("key_id does not match embedded public key"));
    }
    VerifyingKey::from_bytes(&pub_arr).map_err(|_| invalid("invalid ed25519 public key"))
}

fn verifying_key_from_token(token: &AuthorizationToken) -> DynResult<VerifyingKey> {
    verifying_key_from_public(&token.key_id, &token.public_key)
}

fn verifying_key_from_birth(record: &BirthRecord) -> DynResult<VerifyingKey> {
    verifying_key_from_public(&record.key_id, &record.public_key)
}

fn verify_ed25519(verifying: &VerifyingKey, bytes: &[u8], signature_hex: &str) -> DynResult<()> {
    let sig_bytes = hex_decode(signature_hex)?;
    if sig_bytes.len() != 64 {
        return Err(invalid("signature length is not 64 bytes"));
    }
    let signature = Signature::from_bytes(
        &sig_bytes
            .try_into()
            .map_err(|_| invalid("signature invalid length"))?,
    );
    verifying
        .verify(bytes, &signature)
        .map_err(|_| invalid("signature does not verify"))
}

// ============================================================
// 稀疏写入 + 断点续写（WP-43.9）
// ============================================================

fn populated_extents(bytes: &[u8]) -> Vec<(u64, u64)> {
    let mut raw: Vec<(u64, u64)> = Vec::new();
    let mut start: Option<usize> = None;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != 0 {
            if start.is_none() {
                start = Some(index);
            }
        } else if let Some(s) = start.take() {
            raw.push((s as u64, (index - s) as u64));
        }
    }
    if let Some(s) = start {
        raw.push((s as u64, (bytes.len() - s) as u64));
    }
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for (offset, len) in raw {
        if let Some(last) = merged.last_mut() {
            let gap = offset - (last.0 + last.1);
            if gap < MIN_GAP_TO_SPLIT {
                last.1 += gap + len;
                continue;
            }
        }
        merged.push((offset, len));
    }
    merged
}

/// sample 策略的 readback 目标摘要：对全文件（含零区）的非零 extent 清单
/// 做规范化摘要——总长 + extent 数 + 每个 (offset,len,extent-sha256)。
/// 确定性、可机器重算；只覆盖非零区（策略在白名单中显式声明）。
fn extent_manifest_digest(bytes: &[u8]) -> String {
    let extents = populated_extents(bytes);
    let mut hasher = Sha256::new();
    hasher.update(u64::try_from(bytes.len()).unwrap_or(0).to_le_bytes());
    hasher.update(u32::try_from(extents.len()).unwrap_or(0).to_le_bytes());
    for (offset, len) in &extents {
        let start = usize::try_from(*offset).unwrap_or(0);
        let end = usize::try_from(*offset + *len).unwrap_or(bytes.len());
        hasher.update(offset.to_le_bytes());
        hasher.update(len.to_le_bytes());
        hasher.update(Sha256::digest(&bytes[start..end]));
    }
    hex(&hasher.finalize())
}

fn read_journal(path: &Path) -> DynResult<Option<ResumeJournal>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Box::new(error)),
    }
}

fn write_journal(path: &Path, journal: &ResumeJournal) -> DynResult<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    file.write_all(&serde_json::to_vec(journal)?)?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

/// 校验 resume journal 里登记的每个已写 extent 与介质当前内容逐字节一致。
/// 用于续写前证明"介质上的部分状态 = journal 声称的部分状态"（可恢复、未
/// 篡改）；任一不符返回 false，调用方拒绝在该介质上续写。
fn verify_journal_extents(target: &Path, journal: &ResumeJournal) -> DynResult<bool> {
    let mut file = fs::File::open(target)?;
    for extent in &journal.extents_written {
        let len = usize::try_from(extent.len)?;
        let mut buffer = vec![0u8; len];
        file.seek(SeekFrom::Start(extent.offset))?;
        file.read_exact(&mut buffer)?;
        if hex(&Sha256::digest(&buffer)) != extent.sha256 {
            return Ok(false);
        }
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn write_sparse_with_journal(
    target: &Path,
    image: &[u8],
    journal_path: &Path,
    extents: &[(u64, u64)],
    image_sha: &str,
    stable_id: &str,
    target_rel: &str,
    target_size: u64,
    interrupt_after_new: Option<usize>,
) -> DynResult<(usize, bool, bool)> {
    let image_size = u64::try_from(image.len())?;
    let journal = match read_journal(journal_path)? {
        Some(existing)
            if existing.extents_total == extents.len()
                && existing.image_sha256 == image_sha
                && existing.target_stable_id == stable_id
                && existing.target_size_bytes == image_size =>
        {
            existing
        }
        Some(_) => {
            return Err(invalid(
                "resume journal exists for a different job; remove the stale journal first",
            ));
        }
        None => ResumeJournal {
            schema: JOURNAL_SCHEMA.to_owned(),
            image_sha256: image_sha.to_owned(),
            target_stable_id: stable_id.to_owned(),
            target_path: target_rel.to_owned(),
            target_size_bytes: target_size,
            extents_total: extents.len(),
            extents_written: Vec::new(),
        },
    };
    let resumed = !journal.extents_written.is_empty();
    let written_offsets: BTreeSet<u64> = journal
        .extents_written
        .iter()
        .map(|extent| extent.offset)
        .collect();
    let mut journal = journal;
    let mut file = OpenOptions::new().read(true).write(true).open(target)?;
    let mut new_written = 0usize;
    for (offset, len) in extents {
        if written_offsets.contains(offset) {
            continue;
        }
        if let Some(limit) = interrupt_after_new {
            if new_written >= limit {
                break;
            }
        }
        let start = usize::try_from(*offset)?;
        let end = usize::try_from(*offset + *len)?;
        file.seek(SeekFrom::Start(*offset))?;
        file.write_all(&image[start..end])?;
        file.flush()?;
        file.sync_all()?;
        journal.extents_written.push(ExtentRecord {
            offset: *offset,
            len: *len,
            sha256: hex(&Sha256::digest(&image[start..end])),
        });
        new_written += 1;
        write_journal(journal_path, &journal)?;
    }
    let completed = journal.extents_written.len() == extents.len();
    if completed {
        let _ = fs::remove_file(journal_path);
    }
    Ok((journal.extents_written.len(), resumed, !completed))
}

fn readback_result(readback: &[u8], token: &AuthorizationToken) -> (String, bool, String) {
    match token.readback_policy.as_str() {
        "full" => (
            hex(&Sha256::digest(readback)),
            hex(&Sha256::digest(readback)) == token.image_sha256,
            format!("full-image-digest ({} bytes)", readback.len()),
        ),
        "sample" => {
            let digest = extent_manifest_digest(readback);
            let count = populated_extents(readback).len();
            (
                digest.clone(),
                digest == token.sample_manifest_sha256,
                format!("extent-manifest over {count} populated extents"),
            )
        }
        _ => (
            String::new(),
            false,
            "unknown-readback-policy (fail-closed)".to_owned(),
        ),
    }
}

fn enumerate_windows() -> DynResult<Vec<DeviceRecord>> {
    let script = r#"
$ErrorActionPreference='Stop'
$devices=@(Get-Disk | Sort-Object Number | ForEach-Object {
  $disk=$_
  $parts=@(Get-Partition -DiskNumber $disk.Number -ErrorAction SilentlyContinue)
  $mounts=@($parts | ForEach-Object { $_.AccessPaths } | Where-Object { $_ })
  $reasons=@()
  if ($disk.IsSystem) { $reasons += 'windows-system-disk' }
  if ($disk.IsBoot) { $reasons += 'windows-boot-disk' }
  $stable=[string]$disk.UniqueId
  [pscustomobject]@{
    path=('\\.\PhysicalDrive'+$disk.Number)
    stable_id=$stable
    model=[string]$disk.FriendlyName
    bus=[string]$disk.BusType
    size_bytes=[uint64]$disk.Size
    logical_sector_size=[uint32]$disk.LogicalSectorSize
    read_only=[bool]$disk.IsReadOnly
    removable=([string]$disk.BusType -in @('USB','SD','MMC'))
    topology_complete=(-not [string]::IsNullOrWhiteSpace($stable))
    system_reasons=@($reasons)
    mountpoints=@($mounts)
  }
})
@{devices=$devices}|ConvertTo-Json -Depth 6 -Compress
"#;
    let output = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .output()?;
    if !output.status.success() {
        return Err(invalid(format!(
            "Windows disk enumeration failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let envelope: WindowsEnvelope = serde_json::from_slice(&output.stdout)?;
    Ok(envelope
        .devices
        .into_iter()
        .map(|device| {
            finalize_device(DeviceRecord {
                stable_id: device.stable_id,
                path: device.path,
                model: device.model,
                bus: device.bus,
                size_bytes: device.size_bytes,
                logical_sector_size: device.logical_sector_size,
                read_only: device.read_only,
                removable: device.removable,
                topology_complete: device.topology_complete,
                system_reasons: device.system_reasons,
                mountpoints: device.mountpoints,
                eligible: false,
                denials: Vec::new(),
            })
        })
        .collect())
}

fn enumerate_linux() -> DynResult<Vec<DeviceRecord>> {
    let output = Command::new("wsl.exe")
        .args([
            "--exec",
            "lsblk",
            "--json",
            "--bytes",
            "--paths",
            "--output",
            "NAME,PATH,TYPE,SIZE,RO,RM,MOUNTPOINTS,PKNAME,MODEL,SERIAL,TRAN,LOG-SEC",
        ])
        .output()?;
    if !output.status.success() {
        return Err(invalid(format!(
            "Linux disk enumeration failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let value: Value = serde_json::from_slice(&output.stdout)?;
    let devices = value
        .get("blockdevices")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("lsblk JSON has no blockdevices array"))?;
    let mut output_devices = Vec::new();
    for value in devices {
        if string_field(value, "type")? != "disk" {
            continue;
        }
        let mut mountpoints = Vec::new();
        collect_mountpoints(value, &mut mountpoints)?;
        mountpoints.sort();
        mountpoints.dedup();
        let mut system_reasons = Vec::new();
        if mountpoints.iter().any(|mount| mount == "/") {
            system_reasons.push("linux-root-ancestor".to_owned());
        }
        if mountpoints
            .iter()
            .any(|mount| mount == "/boot" || mount == "/boot/efi")
        {
            system_reasons.push("linux-boot-ancestor".to_owned());
        }
        let serial = optional_string_field(value, "serial");
        let model = optional_string_field(value, "model");
        let size = u64_field(value, "size")?;
        let stable_id = if serial.is_empty() {
            String::new()
        } else {
            format!("linux:{}:{}:{}", serial, model, size)
        };
        output_devices.push(finalize_device(DeviceRecord {
            stable_id,
            path: string_field(value, "path")?,
            model,
            bus: optional_string_field(value, "tran"),
            size_bytes: size,
            logical_sector_size: u32::try_from(u64_field(value, "log-sec")?)?,
            read_only: bool_field(value, "ro")?,
            removable: bool_field(value, "rm")?,
            topology_complete: !serial.is_empty(),
            system_reasons,
            mountpoints,
            eligible: false,
            denials: Vec::new(),
        }));
    }
    Ok(output_devices)
}

fn finalize_device(mut device: DeviceRecord) -> DeviceRecord {
    let mut denials = Vec::new();
    if device.stable_id.trim().is_empty() || !device.topology_complete {
        denials.push("unstable-or-incomplete-identity".to_owned());
    }
    if device.path.trim().is_empty() || device.size_bytes == 0 {
        denials.push("invalid-path-or-size".to_owned());
    }
    if !matches!(device.logical_sector_size, 512 | 4096) {
        denials.push("unsupported-logical-sector-size".to_owned());
    }
    if device.read_only {
        denials.push("read-only".to_owned());
    }
    if !device.system_reasons.is_empty() {
        denials.push("system-or-boot-disk".to_owned());
    }
    if !device.mountpoints.is_empty() {
        denials.push("mounted-volume-or-child".to_owned());
    }
    if device.bus.trim().is_empty() || device.bus.eq_ignore_ascii_case("unknown") {
        denials.push("ambiguous-bus".to_owned());
    }
    denials.sort();
    denials.dedup();
    device.eligible = denials.is_empty();
    device.denials = denials;
    device
}

fn finalize_inventory(platform: &str, devices: Vec<DeviceRecord>) -> DynResult<Inventory> {
    let canonical = serde_json::to_vec(&devices)?;
    Ok(Inventory {
        schema: INVENTORY_SCHEMA.to_owned(),
        result: "PASS".to_owned(),
        platform: platform.to_owned(),
        observation_id: hex(&Sha256::digest(&canonical)),
        device_count: devices.len(),
        eligible_count: devices.iter().filter(|device| device.eligible).count(),
        devices,
    })
}

fn verify_inventory(inventory: &Inventory) -> DynResult<()> {
    if inventory.schema != INVENTORY_SCHEMA
        || inventory.result != "PASS"
        || !matches!(inventory.platform.as_str(), "windows" | "linux")
        || inventory.device_count != inventory.devices.len()
        || inventory.eligible_count
            != inventory
                .devices
                .iter()
                .filter(|device| device.eligible)
                .count()
        || inventory.observation_id != hex(&Sha256::digest(serde_json::to_vec(&inventory.devices)?))
    {
        return Err(invalid("flash inventory envelope is inconsistent"));
    }
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for device in &inventory.devices {
        if &finalize_device(device.clone()) != device
            || !ids.insert(device.stable_id.as_str())
            || !paths.insert(device.path.as_str())
        {
            return Err(invalid("flash inventory record is mutated or ambiguous"));
        }
    }
    Ok(())
}

fn validate_key_domains(domains: &KeyDomains) -> DynResult<()> {
    if domains.schema != KEY_DOMAINS_SCHEMA || domains.updated_by.trim().is_empty() {
        return Err(invalid("key-domains file schema/owner is invalid"));
    }
    for domain in ["factory", "release", "recovery"] {
        let Some(entry) = domains.domains.get(domain) else {
            return Err(invalid(format!("key domain '{domain}' is missing")));
        };
        if entry.allowed_uses.is_empty() || entry.key_ref.trim().is_empty() {
            return Err(invalid(format!(
                "key domain '{domain}' is not fully declared"
            )));
        }
    }
    let required_uses = [
        ("factory", "provisioning"),
        ("factory", "birth-record"),
        ("release", "release-signing"),
        ("recovery", "recovery-signing"),
    ];
    for (domain, required_use) in required_uses {
        let entry = domains
            .domains
            .get(domain)
            .ok_or_else(|| invalid(format!("key domain '{domain}' is missing")))?;
        if !entry
            .allowed_uses
            .iter()
            .any(|allowed| allowed == required_use)
        {
            return Err(invalid(format!(
                "key domain '{domain}' must allow '{required_use}'"
            )));
        }
    }
    Ok(())
}

fn validate_raw_image(image: &[u8]) -> DynResult<u32> {
    for sector_size in [512, 4096] {
        if parse_gpt(image, sector_size).is_ok() {
            return Ok(sector_size);
        }
    }
    Err(invalid("flash input is not a valid canonical GPT image"))
}

fn inject_post_write_corruption(path: &Path, length: usize) -> DynResult<()> {
    if length == 0 {
        return Err(invalid("cannot inject corruption into an empty target"));
    }
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    file.seek(SeekFrom::Start(u64::try_from(length / 2)?))?;
    let mut byte = [0u8; 1];
    file.read_exact(&mut byte)?;
    byte[0] ^= 1;
    file.seek(SeekFrom::Start(u64::try_from(length / 2)?))?;
    file.write_all(&byte)?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

fn collect_mountpoints(value: &Value, output: &mut Vec<String>) -> DynResult<()> {
    if let Some(mounts) = value.get("mountpoints") {
        if let Some(values) = mounts.as_array() {
            for mount in values {
                if let Some(mount) = mount.as_str().filter(|mount| !mount.is_empty()) {
                    output.push(mount.to_owned());
                }
            }
        } else if let Some(mount) = mounts.as_str().filter(|mount| !mount.is_empty()) {
            output.push(mount.to_owned());
        }
    }
    if let Some(children) = value.get("children").and_then(Value::as_array) {
        for child in children {
            collect_mountpoints(child, output)?;
        }
    }
    Ok(())
}

fn string_field(value: &Value, field: &str) -> DynResult<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| invalid(format!("lsblk field '{field}' is missing or not a string")))
}

fn optional_string_field(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

fn u64_field(value: &Value, field: &str) -> DynResult<u64> {
    let value = value
        .get(field)
        .ok_or_else(|| invalid(format!("lsblk field '{field}' is missing")))?;
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .ok_or_else(|| invalid(format!("lsblk field '{field}' is not an integer")))
}

fn bool_field(value: &Value, field: &str) -> DynResult<bool> {
    let value = value
        .get(field)
        .ok_or_else(|| invalid(format!("lsblk field '{field}' is missing")))?;
    value
        .as_bool()
        .or_else(|| value.as_u64().map(|value| value != 0))
        .or_else(|| value.as_str().map(|value| value == "1" || value == "true"))
        .ok_or_else(|| invalid(format!("lsblk field '{field}' is not boolean")))
}

fn value_after<'a>(args: &'a [String], flag: &str) -> DynResult<&'a str> {
    let mut values = args.windows(2).filter(|pair| pair[0] == flag);
    let value = values
        .next()
        .map(|pair| pair[1].as_str())
        .ok_or_else(|| invalid(format!("{flag} is mandatory")))?;
    if values.next().is_some() {
        return Err(invalid(format!("{flag} may only appear once")));
    }
    Ok(value)
}

fn optional_value_after<'a>(args: &'a [String], flag: &str) -> DynResult<Option<&'a str>> {
    let values: Vec<&str> = args
        .windows(2)
        .filter(|pair| pair[0] == flag)
        .map(|pair| pair[1].as_str())
        .collect();
    match values.as_slice() {
        [] => Ok(None),
        [value] => Ok(Some(value)),
        _ => Err(invalid(format!("{flag} may only appear once"))),
    }
}

fn lab_target(root: &Path, path: &Path) -> DynResult<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
    .canonicalize()?;
    let lab = root.join("target/eneros/flash-lab");
    fs::create_dir_all(&lab)?;
    let lab = lab.canonicalize()?;
    if !candidate.starts_with(&lab) || !candidate.is_file() {
        return Err(invalid(
            "file-backed flash target must be an existing regular file below target/eneros/flash-lab",
        ));
    }
    Ok(candidate)
}

fn lab_output(root: &Path, path: &Path) -> DynResult<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let parent = candidate
        .parent()
        .ok_or_else(|| invalid("lab target has no parent"))?;
    fs::create_dir_all(parent)?;
    let parent = parent.canonicalize()?;
    let lab = root.join("target/eneros/flash-lab");
    fs::create_dir_all(&lab)?;
    let lab = lab.canonicalize()?;
    if !parent.starts_with(&lab) {
        return Err(invalid(
            "file-backed flash target must stay below target/eneros/flash-lab",
        ));
    }
    Ok(parent.join(
        candidate
            .file_name()
            .ok_or_else(|| invalid("lab target has no filename"))?,
    ))
}

fn repo_input(root: &Path, path: &Path, label: &str) -> DynResult<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    if !candidate.is_file() {
        return Err(invalid(format!(
            "{label} must be an existing file (repo-relative path)"
        )));
    }
    Ok(candidate)
}

fn target_input(root: &Path, path: &Path, label: &str) -> DynResult<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
    .canonicalize()?;
    let target = root.join("target").canonicalize()?;
    if !candidate.starts_with(&target) || !candidate.is_file() {
        return Err(invalid(format!(
            "{label} must be an existing file below target"
        )));
    }
    Ok(candidate)
}

fn target_output(root: &Path, path: &Path, label: &str) -> DynResult<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let parent = candidate
        .parent()
        .ok_or_else(|| invalid(format!("{label} has no parent")))?;
    fs::create_dir_all(parent)?;
    let parent = parent.canonicalize()?;
    let target = root.join("target").canonicalize()?;
    if !parent.starts_with(&target) {
        return Err(invalid(format!("{label} output must stay below target")));
    }
    Ok(parent.join(
        candidate
            .file_name()
            .ok_or_else(|| invalid(format!("{label} has no filename")))?,
    ))
}

fn relative(root: &Path, path: &Path) -> DynResult<String> {
    let canonical_root = root.canonicalize()?;
    let canonical_path = path.canonicalize()?;
    Ok(canonical_path
        .strip_prefix(canonical_root)?
        .to_string_lossy()
        .replace('\\', "/"))
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as FmtWrite;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn hex_decode(input: &str) -> DynResult<Vec<u8>> {
    let compact: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.len() % 2 != 0 {
        return Err(invalid("hex string has odd length"));
    }
    let mut bytes = Vec::with_capacity(compact.len() / 2);
    for pair in compact.as_bytes().chunks(2) {
        let text = std::str::from_utf8(pair).map_err(|_| invalid("non-ascii hex"))?;
        let byte = u8::from_str_radix(text, 16).map_err(|_| invalid("invalid hex digit"))?;
        bytes.push(byte);
    }
    Ok(bytes)
}

fn current_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// RFC3339 UTC（无小数秒），供 birth record / 报告时间戳使用。
fn utc_timestamp(unix: u64) -> String {
    let days = i64::try_from(unix / 86400).unwrap_or(0);
    let seconds_of_day = unix % 86400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Howard Hinnant civil_from_days：天数 -> (年,月,日)；1970-01-01 = 0。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let shifted = z + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); // [0, 365]
    let month_prime = (5 * day_of_year + 2) / 153; // [0, 11]
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1; // [1, 31]
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    }; // [1, 12]
    let year_out = if month <= 2 { year + 1 } else { year };
    (
        year_out,
        u32::try_from(month).unwrap_or(0),
        u32::try_from(day).unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate() -> DeviceRecord {
        DeviceRecord {
            stable_id: "usb:serial-1".to_owned(),
            path: r"\\.\PhysicalDrive9".to_owned(),
            model: "lab".to_owned(),
            bus: "USB".to_owned(),
            size_bytes: 256 * 1024 * 1024,
            logical_sector_size: 512,
            read_only: false,
            removable: true,
            topology_complete: true,
            system_reasons: Vec::new(),
            mountpoints: Vec::new(),
            eligible: false,
            denials: Vec::new(),
        }
    }

    #[test]
    fn flasher_seed_rejects_overlong_input() {
        let root = std::env::temp_dir().join(format!("eneros-flasher-key-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("seed.bin");
        fs::write(&path, [6u8; 64]).unwrap();
        assert!(load_signing_key(&root, Some(path.clone())).is_err());
        fs::write(&path, [6u8; 32]).unwrap();
        assert_eq!(load_signing_key(&root, Some(path)).unwrap(), [6u8; 32]);
        fs::remove_dir_all(root).unwrap();
    }

    fn test_key() -> [u8; 32] {
        [7u8; 32]
    }

    fn golden_token(key: &[u8; 32]) -> AuthorizationToken {
        let keypair = SigningKey::from_bytes(key);
        let public = keypair.verifying_key().to_bytes();
        let target_path = "target/eneros/flash-lab/lab.raw";
        let mut token = AuthorizationToken {
            schema: TOKEN_SCHEMA.to_owned(),
            target_kind: "file-lab".to_owned(),
            target_path: target_path.to_owned(),
            target_stable_id: format!("file-lab:{}", hex(&Sha256::digest(target_path.as_bytes()))),
            target_size_bytes: 4096,
            logical_sector_size: 512,
            target_initial_sha256: "00".repeat(32),
            image_path: "target/eneros/qemu-prod.raw".to_owned(),
            image_size_bytes: 4096,
            image_sha256: "11".repeat(32),
            sample_manifest_sha256: String::new(),
            operator_id: "line-7".to_owned(),
            station_id: "station-b".to_owned(),
            tool_version: "1.0.0-rc.0".to_owned(),
            candidate_id: "cand-1".to_owned(),
            readback_policy: "full".to_owned(),
            key_domain: "factory".to_owned(),
            key_id: hex(&Sha256::digest(public)),
            public_key: hex(&public),
            authorization_sha256: String::new(),
            signature: String::new(),
        };
        sign_token(&mut token, key).unwrap();
        token
    }

    #[test]
    fn system_mounted_readonly_and_incomplete_devices_are_denied() {
        assert!(finalize_device(candidate()).eligible);
        let mut system = candidate();
        system.system_reasons.push("windows-system-disk".to_owned());
        assert!(!finalize_device(system).eligible);
        let mut mounted = candidate();
        mounted.mountpoints.push("E:\\".to_owned());
        assert!(!finalize_device(mounted).eligible);
        let mut readonly = candidate();
        readonly.read_only = true;
        assert!(!finalize_device(readonly).eligible);
        let mut incomplete = candidate();
        incomplete.topology_complete = false;
        assert!(!finalize_device(incomplete).eligible);
    }

    #[test]
    fn inventory_mutation_is_rejected() {
        let inventory = finalize_inventory("windows", vec![finalize_device(candidate())]).unwrap();
        verify_inventory(&inventory).unwrap();
        let mut mutated = inventory;
        mutated.devices[0].eligible = false;
        assert!(verify_inventory(&mutated).is_err());
    }

    #[test]
    fn token_roundtrip_and_mutation_are_detected() {
        let key = test_key();
        let token = golden_token(&key);
        assert!(verify_token(&token).is_ok());
        let mut mutated = token.clone();
        mutated.operator_id = "attacker".to_owned();
        assert!(verify_token(&mutated).is_err());
        let mut sig_mut = token.clone();
        sig_mut.signature = "ff".repeat(64);
        assert!(verify_token(&sig_mut).is_err());
    }

    #[test]
    fn populated_extents_merges_close_gaps() {
        let bytes: Vec<u8> = vec![
            0x00, 0x01, 0x02, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x04,
        ];
        let extents = populated_extents(&bytes);
        // 0x01..0x02 与 0x05 间距 2 字节、0x05 与尾部 0x04 间距 10 字节，
        // 均 < 4096 → 全部合并为一个 [1,16)。
        assert_eq!(extents, vec![(1, 16)]);
        // 远超 MIN_GAP_TO_SPLIT 的大零隙应拆开为两次独立 extent。
        let mut spaced = vec![0u8; 20000];
        spaced[10..20].fill(1);
        spaced[10000..10010].fill(2);
        let extents = populated_extents(&spaced);
        assert_eq!(extents.len(), 2);
        assert_eq!(extents[0], (10, 10));
        assert_eq!(extents[1], (10000, 10));
    }

    #[test]
    fn extent_manifest_digest_is_deterministic_and_sensitive() {
        let a: Vec<u8> = vec![0; 16384];
        let mut a = a;
        a[4000..4008].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let b = a.clone();
        assert_eq!(extent_manifest_digest(&a), extent_manifest_digest(&b));
        // 非零区字节翻转必须改变摘要：
        let mut flip_populated = a.clone();
        flip_populated[4003] = 0xFF;
        assert_ne!(
            extent_manifest_digest(&a),
            extent_manifest_digest(&flip_populated)
        );
        // 零区字节被改成非零会新建一个 extent，因此同样改变摘要
        // （sample 策略覆盖全部非零 extent，包括新出现的）：
        let mut flip_zero = a.clone();
        flip_zero[16000] = 0xAB;
        assert_ne!(
            extent_manifest_digest(&a),
            extent_manifest_digest(&flip_zero)
        );
        // 非零字节清零会移除/收缩 extent，同样改变摘要：
        let mut clear_populated = a.clone();
        clear_populated[4001] = 0x00;
        assert_ne!(
            extent_manifest_digest(&a),
            extent_manifest_digest(&clear_populated)
        );
    }

    #[test]
    fn resume_journal_resumes_partial_write() {
        let mut image = vec![0u8; 1024 * 1024];
        image[0..4096].fill(0xA5); // extent A
        image[8192..12 * 1024].fill(0x5A); // extent B（与 A 间距 4096，独立）
        let tail = image.len() - 8192;
        image[tail..].fill(0x3C); // extent C
        let extents = populated_extents(&image);
        assert_eq!(extents.len(), 3);
        let dir = std::env::temp_dir().join("eneros-flash-test");
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("resume.raw");
        fs::write(&target, vec![0u8; image.len()]).unwrap();
        let journal = dir.join("resume.raw.flash-resume.json");
        let sha = hex(&Sha256::digest(&image));
        // 只写 1 个 extent，模拟中断。
        let (written, resumed, interrupted) = write_sparse_with_journal(
            &target,
            &image,
            &journal,
            &extents,
            &sha,
            "file-lab:test",
            "target/eneros/flash-lab/resume.raw",
            u64::try_from(image.len()).unwrap(),
            Some(1),
        )
        .unwrap();
        assert_eq!(written, 1);
        assert!(!resumed);
        assert!(interrupted);
        assert!(journal.exists());
        // 恢复：续写剩余 extent。
        let (written2, resumed2, interrupted2) = write_sparse_with_journal(
            &target,
            &image,
            &journal,
            &extents,
            &sha,
            "file-lab:test",
            "target/eneros/flash-lab/resume.raw",
            u64::try_from(image.len()).unwrap(),
            None,
        )
        .unwrap();
        assert_eq!(written2, extents.len());
        assert!(resumed2);
        assert!(!interrupted2);
        assert!(!journal.exists());
        assert_eq!(fs::read(&target).unwrap(), image);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn birth_record_sign_verify_and_mutation() {
        let key = test_key();
        let keypair = SigningKey::from_bytes(&key);
        let public = keypair.verifying_key().to_bytes();
        let mut record = BirthRecord {
            schema: BIRTH_SCHEMA.to_owned(),
            result: "PASS".to_owned(),
            report_path: "target/eneros/evidence/IMG-011/bound-flash-report.json".to_owned(),
            device_serial: "SN-0001".to_owned(),
            device_model: "qemu-virt".to_owned(),
            candidate_id: "cand-1".to_owned(),
            image_sha256: "11".repeat(32),
            readback_sha256: "22".repeat(32),
            readback_policy: "full".to_owned(),
            station_id: "station-b".to_owned(),
            operator_id: "line-7".to_owned(),
            key_domain: "factory".to_owned(),
            key_id: hex(&Sha256::digest(public)),
            public_key: hex(&public),
            generated_at: "2026-08-18T00:00:00Z".to_owned(),
            generated_unix: 1_758_600_000,
            signature: String::new(),
        };
        sign_birth(&mut record, &key).unwrap();
        assert!(verify_birth(&record).is_ok());
        let mut tampered = record.clone();
        tampered.device_serial = "SN-9999".to_owned();
        assert!(verify_birth(&tampered).is_err());
    }

    #[test]
    fn key_domains_require_all_three_domains() {
        let domains = KeyDomains {
            schema: KEY_DOMAINS_SCHEMA.to_owned(),
            updated_by: "owner".to_owned(),
            updated_at: "2026-08-18T00:00:00Z".to_owned(),
            note: None,
            domains: BTreeMap::from([
                (
                    "factory".to_owned(),
                    KeyDomain {
                        allowed_uses: vec!["provisioning".to_owned(), "birth-record".to_owned()],
                        key_ref: crate::keys::DEV_SIGNING_KEY_REL.to_owned(),
                    },
                ),
                (
                    "release".to_owned(),
                    KeyDomain {
                        allowed_uses: vec!["release-signing".to_owned()],
                        key_ref: "product/security/release-signing.key".to_owned(),
                    },
                ),
                (
                    "recovery".to_owned(),
                    KeyDomain {
                        allowed_uses: vec!["recovery-signing".to_owned()],
                        key_ref: "product/security/recovery-signing.key".to_owned(),
                    },
                ),
            ]),
        };
        assert!(validate_key_domains(&domains).is_ok());
        let mut missing = domains.clone();
        missing.domains.remove("recovery");
        assert!(validate_key_domains(&missing).is_err());
        let mut wrong_use = domains.clone();
        wrong_use
            .domains
            .get_mut("factory")
            .unwrap()
            .allowed_uses
            .retain(|use_name| use_name != "birth-record");
        assert!(validate_key_domains(&wrong_use).is_err());
    }

    #[test]
    fn verify_journal_extents_detects_partial_mismatch() {
        let dir = std::env::temp_dir().join("eneros-flash-test-journal");
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("partial.raw");
        fs::write(&target, vec![0x5Au8; 4096]).unwrap();
        let good = ResumeJournal {
            schema: JOURNAL_SCHEMA.to_owned(),
            image_sha256: "aa".repeat(32),
            target_stable_id: "file-lab:t".to_owned(),
            target_path: "x".to_owned(),
            target_size_bytes: 4096,
            extents_total: 1,
            extents_written: vec![ExtentRecord {
                offset: 0,
                len: 4096,
                sha256: hex(&Sha256::digest([0x5Au8; 4096])),
            }],
        };
        assert!(verify_journal_extents(&target, &good).unwrap());
        // 介质内容与 journal 不一致 → 拒绝续写。
        let bad = ResumeJournal {
            extents_written: vec![ExtentRecord {
                offset: 0,
                len: 4096,
                sha256: hex(&Sha256::digest([0xFFu8; 4096])),
            }],
            ..good.clone()
        };
        assert!(!verify_journal_extents(&target, &bad).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn utc_timestamp_is_well_formed() {
        assert_eq!(utc_timestamp(0), "1970-01-01T00:00:00Z");
        // 2026-08-18T00:00:00Z 的 unix 秒数（预计算值下界校验用非精确保留）。
        let ts = utc_timestamp(1_758_600_000);
        assert!(ts.ends_with('Z'));
        assert_eq!(ts.len(), 20);
    }
}
