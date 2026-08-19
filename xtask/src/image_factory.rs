use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use eneros_boot_protocol::manifest::{
    COMPONENT_FLAG_REQUIRED, PERMISSION_EXECUTE, PERMISSION_READ,
};
use eneros_boot_protocol::{
    BootControl, Component, ComponentRole, InstallManifest, ManifestHeader, ManifestSlot,
    ProductVersion, ReleaseManifest, SlotState, BOOT_CONTROL_COPY_OFFSETS,
    BOOT_CONTROL_FLAG_MANUAL_RECOVERY, BOOT_CONTROL_RECORD_SIZE,
};
use eneros_image_format::{parse_gpt, write_gpt, Gpt, Guid, PartitionSpec};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{
    audit_layout, audit_profile, ensure_audit_pass, ensure_layout_audit_pass, invalid,
    load_profile, read_json, DynResult, ImageLayout, ProductProfile, ProfileClass,
};

const KERNEL_LOAD_ADDRESS: u64 = 0x4100_0000;
const INIT_LOAD_ADDRESS: u64 = 0x4200_0000;
const MANIFEST_RESERVED_BYTES: usize = 64 * 1024;
const ESP_TYPE_GUID: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";
const UEFI_VARIABLE_OWNER_GUID: &str = "454e4552-4f53-4001-8000-000000000db0";
const DEFAULT_UEFI_VARS_TEMPLATE: &str = "/usr/share/AAVMF/AAVMF_VARS.fd";

#[derive(Debug)]
struct RawOptions {
    profile: String,
    signing_key: PathBuf,
    uefi_key: Option<String>,
    uefi_cert: Option<String>,
    uefi_application: Option<PathBuf>,
    output: Option<PathBuf>,
}

#[derive(Debug)]
struct IsoOptions {
    profile: String,
    raw_image: PathBuf,
    signing_key: PathBuf,
    uefi_key: Option<String>,
    uefi_cert: String,
    installer_application: Option<PathBuf>,
    output: Option<PathBuf>,
}

#[derive(Debug)]
struct RecoveryOptions {
    profile: String,
    source_image: PathBuf,
    release_public_key: PathBuf,
    output: Option<PathBuf>,
}

#[derive(Debug)]
struct VarsOptions {
    template: String,
    uefi_cert: String,
    output: PathBuf,
}

#[derive(Debug, Serialize)]
struct VarsTemplateReport {
    schema: &'static str,
    result: &'static str,
    owner_guid: &'static str,
    input_template: String,
    output_size_bytes: u64,
    output_sha256: String,
    secure_boot: &'static str,
    enrolled_databases: [&'static str; 3],
}

#[derive(Debug, Serialize)]
struct RecoveryImageReport {
    schema: &'static str,
    profile: String,
    source_sha256: String,
    output_sha256: String,
    size_bytes: u64,
    release_id_hex: String,
    release_key_id_hex: String,
    platform_id_hex: String,
    security_floor: u64,
    bootctrl_copies_equal: bool,
    manual_recovery: bool,
    disabled_slots: Vec<String>,
    zeroed_partitions: Vec<String>,
    preserved_partitions: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ImageMap {
    schema: &'static str,
    profile: String,
    source_date_epoch: u64,
    disk_size_bytes: u64,
    sector_size: u32,
    disk_guid: String,
    platform_id_hex: String,
    release_key_id_hex: String,
    release_public_key_hex: String,
    release_id_hex: String,
    partitions: Vec<ImageMapPartition>,
    components: Vec<ImageMapComponent>,
}

#[derive(Debug, Serialize)]
struct ImageMapPartition {
    role: String,
    type_guid: String,
    unique_guid: String,
    first_lba: u64,
    last_lba: u64,
    size_bytes: u64,
    sha256: String,
}

#[derive(Debug, Serialize)]
struct ImageMapComponent {
    slot: String,
    role: String,
    partition: String,
    media_offset: u64,
    media_length: u64,
    load_address: u64,
    memory_length: u64,
    entry_address: u64,
    sha256: String,
}

#[derive(Debug, Serialize)]
struct ArtifactHashes {
    schema: &'static str,
    artifacts: BTreeMap<String, ArtifactHash>,
}

#[derive(Debug, Serialize)]
struct ArtifactHash {
    size_bytes: u64,
    sha256: String,
}

#[derive(Debug, Serialize)]
struct BuildIdentity {
    schema: &'static str,
    profile: String,
    workspace_version: &'static str,
    source_commit: String,
    source_date_epoch: u64,
    cargo_lock_sha256: String,
    build_id_hex: String,
}

struct PackedKernel {
    bytes: Vec<u8>,
    entry: u64,
    memory_length: u64,
}

struct PartitionPlacement {
    role: String,
    type_guid: Guid,
    type_guid_text: String,
    unique_guid: Guid,
    unique_guid_text: String,
    first_lba: u64,
    last_lba: u64,
    attributes: u64,
}

pub(super) fn command_image(root: &Path, args: &[String]) -> DynResult<()> {
    let Some((mode, tail)) = args.split_first() else {
        return Err(invalid("image requires a mode"));
    };
    match mode.as_str() {
        "raw" => build_raw(root, parse_raw_options(tail)?),
        "iso" => build_iso(root, parse_iso_options(tail)?),
        "recovery" => build_recovery(root, parse_recovery_options(tail)?),
        "vars" => build_vars_template(root, parse_vars_options(tail)?),
        "update" => crate::update_package::command_image_update(root, tail),
        "qemu-prod" => build_qemu_prod(root, tail),
        other => Err(invalid(format!(
            "image mode '{other}' is not implemented; accepted now: raw, iso, recovery, vars, update, qemu-prod"
        ))),
    }
}

/// `image qemu-prod`：qemu-prod release profile 的快捷目标。
///
/// 等价于 `image raw qemu-prod`（raw-gpt 产物），密钥从 CI 环境变量
/// `ENEROS_IMAGE_SIGNING_KEY` / `ENEROS_IMAGE_UEFI_KEY` / `ENEROS_IMAGE_UEFI_CERT`
/// 或仓库内 dev/test 密钥读取（G1 release-candidate 证据产物，非生产签名）。
/// 可选 `--output` 覆盖默认输出路径。
fn build_qemu_prod(root: &Path, args: &[String]) -> DynResult<()> {
    let mut output = None;
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--output" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| invalid("--output requires a value"))?;
                output = Some(PathBuf::from(value));
                index += 2;
            }
            other => {
                return Err(invalid(format!(
                    "unknown image qemu-prod option '{other}'; accepted: --output <path>"
                )));
            }
        }
    }
    let signing_key = env_key_file(
        root,
        "ENEROS_IMAGE_SIGNING_KEY",
        Path::new("product/security/dev-signing.key"),
    )?;
    let uefi_key = env_value_or("ENEROS_IMAGE_UEFI_KEY", "product/security/uefi-test.key")?;
    let uefi_cert = env_value_or("ENEROS_IMAGE_UEFI_CERT", "product/security/uefi-test.pem")?;
    build_raw(
        root,
        RawOptions {
            profile: "qemu-prod".to_owned(),
            signing_key,
            uefi_key: Some(uefi_key),
            uefi_cert: Some(uefi_cert),
            uefi_application: None,
            output,
        },
    )
}

/// 从环境变量或仓库默认路径解析 32 字节 Ed25519 seed 文件。
fn env_key_file(root: &Path, env_name: &str, default: &Path) -> DynResult<PathBuf> {
    match std::env::var(env_name) {
        Ok(value) => Ok(root.join(value)),
        Err(_) => {
            let path = root.join(default);
            if !path.is_file() {
                return Err(invalid(format!(
                    "signing key missing: set {env_name} or provide {}",
                    default.display()
                )));
            }
            Ok(path)
        }
    }
}

/// 从环境变量或仓库默认路径解析 UEFI 签名材料路径字符串。
fn env_value_or(env_name: &str, default: &str) -> DynResult<String> {
    Ok(std::env::var(env_name).unwrap_or_else(|_| default.to_owned()))
}

fn parse_vars_options(args: &[String]) -> DynResult<VarsOptions> {
    let mut template = DEFAULT_UEFI_VARS_TEMPLATE.to_owned();
    let mut uefi_cert = None;
    let mut output = None;
    let mut index = 0usize;
    while index < args.len() {
        let flag = &args[index];
        let value = args
            .get(index + 1)
            .ok_or_else(|| invalid(format!("{flag} requires a value")))?;
        match flag.as_str() {
            "--template" => template = value.clone(),
            "--uefi-cert" => uefi_cert = Some(value.clone()),
            "--output" => output = Some(PathBuf::from(value)),
            _ => return Err(invalid(format!("unknown image vars option '{flag}'"))),
        }
        index += 2;
    }
    Ok(VarsOptions {
        template,
        uefi_cert: uefi_cert.ok_or_else(|| invalid("--uefi-cert is mandatory"))?,
        output: output.ok_or_else(|| invalid("--output is mandatory"))?,
    })
}

fn build_vars_template(root: &Path, options: VarsOptions) -> DynResult<()> {
    Guid::parse(UEFI_VARIABLE_OWNER_GUID)
        .map_err(|error| invalid(format!("UEFI variable owner GUID: {error:?}")))?;
    let output = target_output(root, &options.output, "UEFI vars template")?;
    let candidate = output.with_extension("fd.candidate");
    if candidate.exists() {
        fs::remove_file(&candidate)?;
    }
    let candidate_tool = wsl_path(root, &candidate)?;
    let template_tool = wsl_path(root, Path::new(&options.template))?;
    let uefi_cert_tool = wsl_path(root, Path::new(&options.uefi_cert))?;
    let mut command = if cfg!(windows) {
        let mut command = Command::new("wsl.exe");
        command.args(["-d", "Ubuntu-22.04", "--", "virt-fw-vars"]);
        command
    } else {
        Command::new("virt-fw-vars")
    };
    command.args([
        "-i",
        &template_tool,
        "--set-pk",
        UEFI_VARIABLE_OWNER_GUID,
        &uefi_cert_tool,
        "--add-kek",
        UEFI_VARIABLE_OWNER_GUID,
        &uefi_cert_tool,
        "--add-db",
        UEFI_VARIABLE_OWNER_GUID,
        &uefi_cert_tool,
        "--sb",
        "-o",
        &candidate_tool,
    ]);
    run(&mut command)?;

    let metadata = fs::metadata(&candidate)?;
    if metadata.len() < 1024 * 1024 || metadata.len() % 4096 != 0 {
        return Err(invalid(
            "generated UEFI vars template has a non-canonical size",
        ));
    }
    let mut verify = if cfg!(windows) {
        let mut command = Command::new("wsl.exe");
        command.args(["-d", "Ubuntu-22.04", "--", "virt-fw-vars"]);
        command
    } else {
        Command::new("virt-fw-vars")
    };
    verify.args(["-i", &candidate_tool, "--print"]);
    run(&mut verify)?;

    if output.exists() {
        let previous = output.with_extension("fd.previous");
        if previous.exists() {
            fs::remove_file(&previous)?;
        }
        fs::rename(&output, previous)?;
    }
    fs::rename(&candidate, &output)?;
    let bytes = fs::read(&output)?;
    let report = VarsTemplateReport {
        schema: "eneros.uefi-vars-template.v1",
        result: "PASS",
        owner_guid: UEFI_VARIABLE_OWNER_GUID,
        input_template: options.template,
        output_size_bytes: u64::try_from(bytes.len())?,
        output_sha256: hex(&Sha256::digest(&bytes)),
        secure_boot: "ENABLED",
        enrolled_databases: ["PK", "KEK", "db"],
    };
    fs::write(
        output.with_extension("fd.report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!(
        "IMAGE vars result=PASS path={} bytes={} sha256={}",
        output.display(),
        report.output_size_bytes,
        report.output_sha256
    );
    Ok(())
}

fn parse_recovery_options(args: &[String]) -> DynResult<RecoveryOptions> {
    let Some(profile) = args.first() else {
        return Err(invalid("image recovery requires an explicit profile"));
    };
    let mut source_image = None;
    let mut release_public_key = None;
    let mut output = None;
    let mut index = 1usize;
    while index < args.len() {
        let flag = &args[index];
        let value = args
            .get(index + 1)
            .ok_or_else(|| invalid(format!("{flag} requires a value")))?;
        match flag.as_str() {
            "--source" => source_image = Some(PathBuf::from(value)),
            "--release-public-key" => release_public_key = Some(PathBuf::from(value)),
            "--output" => output = Some(PathBuf::from(value)),
            _ => return Err(invalid(format!("unknown image recovery option '{flag}'"))),
        }
        index += 2;
    }
    Ok(RecoveryOptions {
        profile: profile.clone(),
        source_image: source_image.ok_or_else(|| invalid("--source is mandatory"))?,
        release_public_key: release_public_key
            .ok_or_else(|| invalid("--release-public-key is mandatory"))?,
        output,
    })
}

fn parse_iso_options(args: &[String]) -> DynResult<IsoOptions> {
    let Some(profile) = args.first() else {
        return Err(invalid("image iso requires an explicit profile"));
    };
    let mut raw_image = None;
    let mut signing_key = None;
    let mut uefi_key = None;
    let mut uefi_cert = None;
    let mut installer_application = None;
    let mut output = None;
    let mut index = 1usize;
    while index < args.len() {
        let flag = &args[index];
        let value = args
            .get(index + 1)
            .ok_or_else(|| invalid(format!("{flag} requires a value")))?;
        match flag.as_str() {
            "--raw" => raw_image = Some(PathBuf::from(value)),
            "--signing-key" => signing_key = Some(PathBuf::from(value)),
            "--uefi-key" => uefi_key = Some(value.clone()),
            "--uefi-cert" => uefi_cert = Some(value.clone()),
            "--installer-application" => installer_application = Some(PathBuf::from(value)),
            "--output" => output = Some(PathBuf::from(value)),
            _ => return Err(invalid(format!("unknown image iso option '{flag}'"))),
        }
        index += 2;
    }
    if installer_application.is_some() == uefi_key.is_some() {
        return Err(invalid(
            "exactly one of --uefi-key or --installer-application is required",
        ));
    }
    Ok(IsoOptions {
        profile: profile.clone(),
        raw_image: raw_image.ok_or_else(|| invalid("--raw is mandatory"))?,
        signing_key: signing_key.ok_or_else(|| invalid("--signing-key is mandatory"))?,
        uefi_key,
        uefi_cert: uefi_cert.ok_or_else(|| invalid("--uefi-cert is mandatory"))?,
        installer_application,
        output,
    })
}

fn parse_raw_options(args: &[String]) -> DynResult<RawOptions> {
    let Some(profile) = args.first() else {
        return Err(invalid("image raw requires an explicit profile"));
    };
    let mut signing_key = None;
    let mut uefi_key = None;
    let mut uefi_cert = None;
    let mut uefi_application = None;
    let mut output = None;
    let mut index = 1usize;
    while index < args.len() {
        let flag = &args[index];
        let value = args
            .get(index + 1)
            .ok_or_else(|| invalid(format!("{flag} requires a value")))?;
        match flag.as_str() {
            "--signing-key" => signing_key = Some(PathBuf::from(value)),
            "--uefi-key" => uefi_key = Some(value.clone()),
            "--uefi-cert" => uefi_cert = Some(value.clone()),
            "--uefi-application" => uefi_application = Some(PathBuf::from(value)),
            "--output" => output = Some(PathBuf::from(value)),
            _ => return Err(invalid(format!("unknown image raw option '{flag}'"))),
        }
        index += 2;
    }
    Ok(RawOptions {
        profile: profile.clone(),
        signing_key: signing_key.ok_or_else(|| invalid("--signing-key is mandatory"))?,
        uefi_key,
        uefi_cert,
        uefi_application,
        output,
    })
}

fn build_recovery(root: &Path, options: RecoveryOptions) -> DynResult<()> {
    let profile = load_profile(root, &options.profile)?;
    if profile.class != ProfileClass::Recovery
        || !profile
            .artifacts
            .iter()
            .any(|artifact| artifact == "recovery-image")
    {
        return Err(invalid(format!(
            "profile '{}' is not a recovery-image profile",
            profile.id
        )));
    }
    let profile_audit = audit_profile(root, &profile.id, None)?;
    ensure_audit_pass(&profile_audit)?;
    let layout_id = profile
        .image_layout
        .as_deref()
        .ok_or_else(|| invalid("recovery profile has no image_layout"))?;
    let layout_audit = audit_layout(root, layout_id)?;
    ensure_layout_audit_pass(&layout_audit)?;
    let layout: ImageLayout = read_json(
        &root
            .join("product/image-layouts")
            .join(format!("{layout_id}.json")),
    )?;

    let source = target_artifact(root, &options.source_image)?;
    let key_path = target_artifact(root, &options.release_public_key)?;
    let key_bytes = fs::read(&key_path)?;
    let public_key: [u8; 32] = key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| invalid("release public key must be exactly 32 raw Ed25519 bytes"))?;
    let verifying_key = VerifyingKey::from_bytes(&public_key)?;
    let key_id = first_16(Sha256::digest(public_key));
    let platform_id = first_16(Sha256::digest(profile.platform.as_bytes()));
    let minimum_security_version = u64::from(profile.security.minimum_security_version);

    let source_bytes = fs::read(&source)?;
    if u64::try_from(source_bytes.len())? != layout.disk_size_bytes {
        return Err(invalid(
            "recovery source size differs from the audited layout",
        ));
    }
    let gpt = parse_gpt(&source_bytes, layout.sector_size)
        .map_err(|error| invalid(format!("recovery source GPT is invalid: {error:?}")))?;
    verify_gpt_layout(&gpt, &layout)?;
    let manifest_a = verify_boot_partition(
        &source_bytes,
        &gpt,
        "BOOT_A",
        ManifestSlot::A,
        &verifying_key,
        key_id,
        platform_id,
        minimum_security_version,
    )?;
    let manifest_b = verify_boot_partition(
        &source_bytes,
        &gpt,
        "BOOT_B",
        ManifestSlot::B,
        &verifying_key,
        key_id,
        platform_id,
        minimum_security_version,
    )?;
    let manifest_recovery = verify_boot_partition(
        &source_bytes,
        &gpt,
        "RECOVERY",
        ManifestSlot::Recovery,
        &verifying_key,
        key_id,
        platform_id,
        minimum_security_version,
    )?;
    if manifest_a.header.release_id != manifest_b.header.release_id
        || manifest_a.header.release_id != manifest_recovery.header.release_id
    {
        return Err(invalid(
            "recovery source A/B/Recovery manifests have different release identities",
        ));
    }

    let bootctrl = gpt
        .find_by_name("BOOTCTRL")
        .ok_or_else(|| invalid("recovery source has no BOOTCTRL partition"))?;
    let (bootctrl_start, _) = partition_byte_range(bootctrl, gpt.sector_size)?;
    let read_copy = |image: &[u8], offset: u64| -> DynResult<[u8; BOOT_CONTROL_RECORD_SIZE]> {
        let start = bootctrl_start
            .checked_add(usize::try_from(offset)?)
            .ok_or_else(|| invalid("BOOTCTRL recovery copy offset overflow"))?;
        image
            .get(start..start + BOOT_CONTROL_RECORD_SIZE)
            .ok_or_else(|| invalid("BOOTCTRL recovery copy is truncated"))?
            .try_into()
            .map_err(|_| invalid("BOOTCTRL recovery copy length changed"))
    };
    let first = read_copy(&source_bytes, BOOT_CONTROL_COPY_OFFSETS[0])?;
    let second = read_copy(&source_bytes, BOOT_CONTROL_COPY_OFFSETS[1])?;
    if first != second {
        return Err(invalid(
            "recovery image requires equal canonical BOOTCTRL source copies",
        ));
    }
    let mut control = BootControl::decode(&first)
        .map_err(|error| invalid(format!("recovery source BOOTCTRL: {error:?}")))?;
    if control.sequence != 0
        || control.recovery_image_id != manifest_recovery.header.release_id
        || control.security_floor < minimum_security_version
        || control.recovery_security_version < control.security_floor
    {
        return Err(invalid(
            "recovery source BOOTCTRL identity, generation, or security floor is invalid",
        ));
    }

    let mut image = source_bytes.clone();
    let zeroed_roles = [
        "BOOT_A", "BOOT_B", "SYS_A", "SYS_B", "CONFIG", "DATA", "LOGS", "MODELS",
    ];
    for role in zeroed_roles {
        let partition = gpt
            .find_by_name(role)
            .ok_or_else(|| invalid(format!("recovery source has no {role} partition")))?;
        let (start, end) = partition_byte_range(partition, gpt.sector_size)?;
        image[start..end].fill(0);
    }
    control.flags |= BOOT_CONTROL_FLAG_MANUAL_RECOVERY;
    control.slot_a = SlotState::disabled(control.slot_a.security_version, control.slot_a.image_id);
    control.slot_b = SlotState::disabled(control.slot_b.security_version, control.slot_b.image_id);
    let encoded_control = control
        .encode()
        .map_err(|error| invalid(format!("recovery BOOTCTRL encode: {error:?}")))?;
    for offset in BOOT_CONTROL_COPY_OFFSETS {
        let start = bootctrl_start
            .checked_add(usize::try_from(offset)?)
            .ok_or_else(|| invalid("BOOTCTRL recovery write offset overflow"))?;
        image[start..start + BOOT_CONTROL_RECORD_SIZE].copy_from_slice(&encoded_control);
    }

    let parsed = parse_gpt(&image, layout.sector_size)
        .map_err(|error| invalid(format!("generated recovery GPT is invalid: {error:?}")))?;
    if parsed != gpt {
        return Err(invalid("recovery generation changed the GPT"));
    }
    for role in zeroed_roles {
        let partition = parsed
            .find_by_name(role)
            .ok_or_else(|| invalid(format!("generated recovery has no {role}")))?;
        let (start, end) = partition_byte_range(partition, parsed.sector_size)?;
        if image[start..end].iter().any(|byte| *byte != 0) {
            return Err(invalid(format!(
                "generated recovery partition {role} is not empty"
            )));
        }
    }
    let recovery_after = verify_boot_partition(
        &image,
        &parsed,
        "RECOVERY",
        ManifestSlot::Recovery,
        &verifying_key,
        key_id,
        platform_id,
        minimum_security_version,
    )?;
    if recovery_after != manifest_recovery {
        return Err(invalid("recovery generation changed the signed payload"));
    }
    let first_after = read_copy(&image, BOOT_CONTROL_COPY_OFFSETS[0])?;
    let second_after = read_copy(&image, BOOT_CONTROL_COPY_OFFSETS[1])?;
    if first_after != encoded_control || second_after != encoded_control {
        return Err(invalid("generated recovery BOOTCTRL copies differ"));
    }

    let directory = root.join("target/eneros/artifacts").join(&profile.id);
    fs::create_dir_all(&directory)?;
    let output = target_output(
        root,
        &options
            .output
            .unwrap_or_else(|| directory.join("eneros-qemu-prod-recovery.raw")),
        "recovery image",
    )?;
    write_candidate(&output, &image)?;
    let source_sha256 = hex(&Sha256::digest(&source_bytes));
    let output_sha256 = hex(&Sha256::digest(&image));
    let report = RecoveryImageReport {
        schema: "eneros.recovery-image-report.v1",
        profile: profile.id.clone(),
        source_sha256,
        output_sha256: output_sha256.clone(),
        size_bytes: u64::try_from(image.len())?,
        release_id_hex: hex(&manifest_recovery.header.release_id),
        release_key_id_hex: hex(&key_id),
        platform_id_hex: hex(&platform_id),
        security_floor: control.security_floor,
        bootctrl_copies_equal: true,
        manual_recovery: true,
        disabled_slots: vec!["A".to_owned(), "B".to_owned()],
        zeroed_partitions: zeroed_roles.iter().map(|role| (*role).to_owned()).collect(),
        preserved_partitions: vec![
            "PLATFORM_BOOT".to_owned(),
            "BOOTCTRL".to_owned(),
            "RECOVERY".to_owned(),
        ],
    };
    fs::write(
        directory.join("recovery-image-report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    fs::write(
        directory.join("recovery-artifact-sha256.json"),
        serde_json::to_vec_pretty(&ArtifactHashes {
            schema: "eneros.artifact-hashes.v1",
            artifacts: BTreeMap::from([(
                output
                    .file_name()
                    .ok_or_else(|| invalid("recovery artifact has no file name"))?
                    .to_string_lossy()
                    .into_owned(),
                ArtifactHash {
                    size_bytes: u64::try_from(image.len())?,
                    sha256: output_sha256.clone(),
                },
            )]),
        })?,
    )?;
    println!(
        "RECOVERY profile={} result=PASS path={} bytes={} sha256={output_sha256}",
        profile.id,
        output.display(),
        image.len()
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn verify_boot_partition(
    image: &[u8],
    gpt: &Gpt,
    partition_name: &str,
    expected_slot: ManifestSlot,
    verifying_key: &VerifyingKey,
    key_id: [u8; 16],
    platform_id: [u8; 16],
    minimum_security_version: u64,
) -> DynResult<ReleaseManifest> {
    let partition = gpt
        .find_by_name(partition_name)
        .ok_or_else(|| invalid(format!("image has no {partition_name} partition")))?;
    let (partition_start, partition_end) = partition_byte_range(partition, gpt.sector_size)?;
    let encoded = extract_gpt_manifest_bytes(image, gpt, partition_name)?;
    let manifest = ReleaseManifest::decode(&encoded)
        .map_err(|error| invalid(format!("{partition_name} manifest decode: {error:?}")))?;
    if manifest.header.platform_id != platform_id
        || manifest.header.key_id != key_id
        || manifest.header.security_version < minimum_security_version
        || manifest.components.iter().any(|component| {
            component.slot != expected_slot
                || component.security_version != manifest.header.security_version
        })
    {
        return Err(invalid(format!(
            "{partition_name} manifest identity, slot, or security version mismatch"
        )));
    }
    let signed_len = manifest
        .signed_len()
        .map_err(|error| invalid(format!("{partition_name} signed length: {error:?}")))?;
    verifying_key
        .verify_strict(
            &encoded[..signed_len],
            &Signature::from_bytes(&manifest.signature),
        )
        .map_err(|_| invalid(format!("{partition_name} manifest signature is invalid")))?;
    for component in &manifest.components {
        let start = partition_start
            .checked_add(usize::try_from(component.media_offset)?)
            .ok_or_else(|| invalid(format!("{partition_name} component offset overflow")))?;
        let end = start
            .checked_add(usize::try_from(component.media_length)?)
            .ok_or_else(|| invalid(format!("{partition_name} component end overflow")))?;
        if end > partition_end || hex(&Sha256::digest(&image[start..end])) != hex(&component.digest)
        {
            return Err(invalid(format!(
                "{partition_name} component {:?} digest or bounds mismatch",
                component.role
            )));
        }
    }
    Ok(manifest)
}

fn verify_gpt_layout(gpt: &Gpt, layout: &ImageLayout) -> DynResult<()> {
    let expected_guid =
        Guid::parse(&layout.disk_guid).map_err(|error| invalid(format!("disk GUID: {error:?}")))?;
    if gpt.disk_guid != expected_guid
        || gpt.sector_size != layout.sector_size
        || gpt.total_lbas != layout.disk_size_bytes / u64::from(layout.sector_size)
        || usize::from(gpt.partition_count) != layout.partitions.len()
    {
        return Err(invalid(
            "GPT geometry differs from the audited image layout",
        ));
    }
    for expected in &layout.partitions {
        let partition = gpt
            .find_by_name(&expected.role)
            .ok_or_else(|| invalid(format!("GPT has no {} partition", expected.role)))?;
        let expected_type = Guid::parse(&expected.type_guid)
            .map_err(|error| invalid(format!("partition GUID: {error:?}")))?;
        let size = partition
            .last_lba
            .checked_sub(partition.first_lba)
            .and_then(|sectors| sectors.checked_add(1))
            .and_then(|sectors| sectors.checked_mul(u64::from(gpt.sector_size)))
            .ok_or_else(|| invalid("GPT partition size overflow"))?;
        if partition.type_guid != expected_type
            || partition.attributes != expected.attributes
            || size != expected.size_bytes
        {
            return Err(invalid(format!(
                "GPT partition {} differs from the audited layout",
                expected.role
            )));
        }
    }
    Ok(())
}

fn partition_byte_range(
    partition: &eneros_image_format::ParsedPartition,
    sector_size: u32,
) -> DynResult<(usize, usize)> {
    let start = byte_offset(partition.first_lba, sector_size)?;
    let end = byte_offset(
        partition
            .last_lba
            .checked_add(1)
            .ok_or_else(|| invalid("partition last LBA overflow"))?,
        sector_size,
    )?;
    Ok((start, end))
}

fn build_iso(root: &Path, options: IsoOptions) -> DynResult<()> {
    let profile = load_profile(root, &options.profile)?;
    if !profile
        .artifacts
        .iter()
        .any(|artifact| artifact == "installer-iso")
    {
        return Err(invalid(format!(
            "profile '{}' does not declare installer-iso",
            profile.id
        )));
    }
    ensure_audit_pass(&audit_profile(root, &profile.id, None)?)?;
    let layout_id = profile
        .image_layout
        .as_deref()
        .ok_or_else(|| invalid("installer profile has no image layout"))?;
    ensure_layout_audit_pass(&audit_layout(root, layout_id)?)?;
    let layout: ImageLayout = read_json(
        &root
            .join("product/image-layouts")
            .join(format!("{layout_id}.json")),
    )?;
    let source_date_epoch = required_epoch()?;
    let raw_path = target_artifact(root, &options.raw_image)?;
    let raw = fs::read(&raw_path)?;
    if u64::try_from(raw.len())? != layout.disk_size_bytes {
        return Err(invalid(
            "installer payload size disagrees with audited layout",
        ));
    }
    let gpt = parse_gpt(&raw, layout.sector_size)
        .map_err(|error| invalid(format!("installer payload GPT is invalid: {error:?}")))?;
    let expected_guid =
        Guid::parse(&layout.disk_guid).map_err(|error| invalid(format!("disk GUID: {error:?}")))?;
    if gpt.disk_guid != expected_guid || gpt.sector_size != layout.sector_size {
        return Err(invalid("installer payload has the wrong disk identity"));
    }

    let key_bytes = fs::read(&options.signing_key)?;
    let secret: [u8; 32] = key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| invalid("release signing key must be an external raw 32-byte seed"))?;
    let signing_key = SigningKey::from_bytes(&secret);
    let public_key = signing_key.verifying_key().to_bytes();
    let key_id = first_16(Sha256::digest(public_key));
    let platform_id = first_16(Sha256::digest(profile.platform.as_bytes()));
    let release_manifest = extract_gpt_manifest(&raw, &gpt, "BOOT_A")?;
    if release_manifest.header.key_id != key_id
        || release_manifest.header.platform_id != platform_id
        || release_manifest.header.security_version
            < u64::from(profile.security.minimum_security_version)
    {
        return Err(invalid(
            "installer payload manifest does not match the selected release trust",
        ));
    }
    let manifest_bytes = extract_gpt_manifest_bytes(&raw, &gpt, "BOOT_A")?;
    let signed = ReleaseManifest::signed_region(&manifest_bytes)
        .map_err(|error| invalid(format!("payload manifest signed region: {error:?}")))?;
    let signature = ed25519_dalek::Signature::from_bytes(&release_manifest.signature);
    signing_key
        .verifying_key()
        .verify_strict(signed, &signature)
        .map_err(|_| invalid("installer payload release manifest signature is invalid"))?;

    let payload_digest: [u8; 32] = Sha256::digest(&raw).into();
    let unsigned_install = InstallManifest::new(
        platform_id,
        key_id,
        release_manifest.header.security_version,
        source_date_epoch,
        u64::try_from(raw.len())?,
        layout.sector_size,
        gpt.disk_guid.0,
        release_manifest.header.release_id,
        payload_digest,
    );
    let install_signature = signing_key
        .sign(
            &unsigned_install
                .encode_unsigned()
                .map_err(|error| invalid(format!("install manifest encode: {error:?}")))?,
        )
        .to_bytes();
    let encoded_install = unsigned_install
        .with_signature(install_signature)
        .encode_signed()
        .map_err(|error| invalid(format!("signed install manifest encode: {error:?}")))?;

    let directory = root.join("target/eneros/artifacts").join(&profile.id);
    let installer_directory = directory.join("installer");
    fs::create_dir_all(&installer_directory)?;
    let unsigned_installer = build_uefi_installer(
        root,
        &public_key,
        &key_id,
        &platform_id,
        u64::from(profile.security.minimum_security_version),
    )?;
    let installer = if let Some(signed) = options.installer_application.as_deref() {
        verify_pre_signed_uefi(
            root,
            &unsigned_installer,
            signed,
            &options.uefi_cert,
            &installer_directory,
        )?
    } else {
        sign_uefi_application(
            root,
            &unsigned_installer,
            options
                .uefi_key
                .as_deref()
                .ok_or_else(|| invalid("installer UEFI key is absent"))?,
            &options.uefi_cert,
            &installer_directory,
        )?
    };
    fs::write(
        installer_directory.join("install-manifest-v1.bin"),
        encoded_install,
    )?;
    let boot_image = build_fat16(8 * 1024 * 1024, 0, &installer)?;

    let staging = installer_directory.join("iso-root");
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(staging.join("EFI/BOOT"))?;
    fs::create_dir_all(staging.join("ENEROS"))?;
    fs::write(staging.join("EFI/BOOT/efiboot.img"), boot_image)?;
    fs::write(staging.join("ENEROS/INSTALL.MAN"), encoded_install)?;
    fs::copy(&raw_path, staging.join("ENEROS/PAYLOAD.RAW"))?;

    let output = target_output(
        root,
        &options
            .output
            .unwrap_or_else(|| directory.join(format!("eneros-{}-installer.iso", profile.id))),
        "installer ISO",
    )?;
    let timestamp = iso_timestamp(source_date_epoch)?;
    let staging_wsl = wsl_path(root, &staging)?;
    let output_wsl = wsl_path(root, &output)?;
    let mut xorriso = if cfg!(windows) {
        let mut command = Command::new("wsl.exe");
        command.args(["-d", "Ubuntu-22.04", "--", "xorriso"]);
        command
    } else {
        Command::new("xorriso")
    };
    xorriso.args([
        "-as",
        "mkisofs",
        "-iso-level",
        "3",
        "-full-iso9660-filenames",
        "-volid",
        "ENEROS_INSTALL",
        "--efi-boot",
        "EFI/BOOT/efiboot.img",
        "-isohybrid-gpt-basdat",
        &format!("--modification-date={timestamp}"),
        "--set_all_file_dates",
        &timestamp,
        "-output",
        &output_wsl,
        &staging_wsl,
    ]);
    run(&mut xorriso)?;
    let iso = fs::read(&output)?;
    if iso.len() <= raw.len() || iso.len() % 2048 != 0 {
        return Err(invalid("xorriso produced an invalid installer ISO size"));
    }
    fs::write(
        installer_directory.join("installer-artifact-sha256.json"),
        serde_json::to_vec_pretty(&ArtifactHashes {
            schema: "eneros.artifact-hashes.v1",
            artifacts: BTreeMap::from([
                (
                    output
                        .file_name()
                        .ok_or_else(|| invalid("installer ISO has no file name"))?
                        .to_string_lossy()
                        .into_owned(),
                    ArtifactHash {
                        size_bytes: u64::try_from(iso.len())?,
                        sha256: hex(&Sha256::digest(&iso)),
                    },
                ),
                (
                    "install-manifest-v1.bin".to_owned(),
                    ArtifactHash {
                        size_bytes: encoded_install.len() as u64,
                        sha256: hex(&Sha256::digest(encoded_install)),
                    },
                ),
            ]),
        })?,
    )?;
    println!(
        "ISO profile={} result=PASS path={} bytes={} sha256={} payload_sha256={}",
        profile.id,
        output.display(),
        iso.len(),
        hex(&Sha256::digest(&iso)),
        hex(&payload_digest)
    );
    Ok(())
}

fn build_raw(root: &Path, options: RawOptions) -> DynResult<()> {
    let profile = load_profile(root, &options.profile)?;
    if !profile
        .artifacts
        .iter()
        .any(|artifact| artifact == "raw-gpt")
    {
        return Err(invalid(format!(
            "profile '{}' does not declare raw-gpt",
            profile.id
        )));
    }
    let profile_audit = audit_profile(root, &profile.id, None)?;
    ensure_audit_pass(&profile_audit)?;
    let layout_id = profile
        .image_layout
        .as_deref()
        .ok_or_else(|| invalid("raw image profile has no image_layout"))?;
    let layout_audit = audit_layout(root, layout_id)?;
    ensure_layout_audit_pass(&layout_audit)?;
    let layout: ImageLayout = read_json(
        &root
            .join("product/image-layouts")
            .join(format!("{layout_id}.json")),
    )?;
    let source_date_epoch = required_epoch()?;
    let key_bytes = fs::read(&options.signing_key)?;
    if key_bytes.len() != 32 {
        return Err(invalid(
            "release signing key must be an external raw 32-byte Ed25519 seed",
        ));
    }
    let secret: [u8; 32] = key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| invalid("release signing key length changed while reading"))?;
    let signing_key = SigningKey::from_bytes(&secret);
    let public_key = signing_key.verifying_key().to_bytes();
    let key_id = first_16(Sha256::digest(public_key));
    let platform_id = first_16(Sha256::digest(profile.platform.as_bytes()));

    let directory = root.join("target/eneros/artifacts").join(&profile.id);
    fs::create_dir_all(&directory)?;
    let unsigned_efi = build_uefi_manager(root, &public_key, &key_id, &platform_id)?;
    let efi_application = if profile.class.is_release() {
        let uefi_cert = options
            .uefi_cert
            .as_deref()
            .ok_or_else(|| invalid("release raw image requires --uefi-cert"))?;
        if let Some(signed) = options.uefi_application.as_deref() {
            if options.uefi_key.is_some() {
                return Err(invalid(
                    "--uefi-application and --uefi-key are mutually exclusive",
                ));
            }
            verify_pre_signed_uefi(root, &unsigned_efi, signed, uefi_cert, &directory)?
        } else {
            let uefi_key = options.uefi_key.as_deref().ok_or_else(|| {
                invalid("release raw image requires --uefi-key or --uefi-application")
            })?;
            sign_uefi_application(root, &unsigned_efi, uefi_key, uefi_cert, &directory)?
        }
    } else {
        let bytes = fs::read(&unsigned_efi)?;
        fs::write(directory.join("BOOTAA64.EFI"), &bytes)?;
        bytes
    };
    let kernel = build_and_pack_kernel(root, source_date_epoch)?;
    let (init_bundle, mut bound_manifest) = build_init_bundle(root, source_date_epoch, &profile)?;
    // 回填 kernel digest（外部 seL4 elfloader 的真实 hash），写最终绑定 manifest。
    bound_manifest.bind_digests(
        &BTreeMap::new(),
        &crate::product_deriver::hex(&Sha256::digest(&kernel.bytes)),
    )?;
    let manifest_dir = crate::product_manifest::manifest_dir(root, &profile.id);
    fs::create_dir_all(&manifest_dir)?;
    fs::write(
        manifest_dir.join("product-manifest.normalized.json"),
        bound_manifest.normalized_json()?,
    )?;
    let release_id = release_identity(
        &profile,
        source_date_epoch,
        &efi_application,
        &kernel.bytes,
        &init_bundle,
    );
    let placements = place_partitions(&layout)?;
    let mut image = vec![0u8; usize::try_from(layout.disk_size_bytes)?];
    let specs = placements
        .iter()
        .map(|placement| PartitionSpec {
            type_guid: placement.type_guid,
            unique_guid: placement.unique_guid,
            first_lba: placement.first_lba,
            last_lba: placement.last_lba,
            attributes: placement.attributes,
            name: placement.role.as_str(),
        })
        .collect::<Vec<_>>();
    write_gpt(
        &mut image,
        layout.sector_size,
        Guid::parse(&layout.disk_guid).map_err(|error| invalid(format!("disk GUID: {error:?}")))?,
        &specs,
    )
    .map_err(|error| invalid(format!("GPT writer rejected audited layout: {error:?}")))?;

    let mut components = Vec::new();
    let boot_control = BootControl::factory(
        release_id,
        u64::from(profile.security.minimum_security_version),
        release_id,
    )
    .encode()
    .map_err(|error| invalid(format!("initial BOOTCTRL is invalid: {error:?}")))?;
    for placement in &placements {
        let start = byte_offset(placement.first_lba, layout.sector_size)?;
        let end = byte_offset(
            placement
                .last_lba
                .checked_add(1)
                .ok_or_else(|| invalid("partition LBA overflow"))?,
            layout.sector_size,
        )?;
        let partition = &mut image[start..end];
        match placement.role.as_str() {
            "PLATFORM_BOOT" => {
                if placement.type_guid_text != ESP_TYPE_GUID {
                    return Err(invalid("PLATFORM_BOOT must use the standard ESP type GUID"));
                }
                let hidden = u32::try_from(placement.first_lba)?;
                let fat = build_fat16(partition.len(), hidden, &efi_application)?;
                partition.copy_from_slice(&fat);
            }
            "BOOT_A" => {
                components.extend(write_boot_partition(
                    partition,
                    "BOOT_A",
                    ManifestSlot::A,
                    &profile,
                    source_date_epoch,
                    &signing_key,
                    platform_id,
                    key_id,
                    release_id,
                    &kernel,
                    &init_bundle,
                )?);
            }
            "BOOTCTRL" => {
                for offset in BOOT_CONTROL_COPY_OFFSETS {
                    let start = usize::try_from(offset)?;
                    let end = start
                        .checked_add(BOOT_CONTROL_RECORD_SIZE)
                        .ok_or_else(|| invalid("BOOTCTRL copy end overflow"))?;
                    let destination = partition.get_mut(start..end).ok_or_else(|| {
                        invalid("BOOTCTRL partition is too small for dual copies")
                    })?;
                    destination.copy_from_slice(&boot_control);
                }
            }
            "BOOT_B" => {
                components.extend(write_boot_partition(
                    partition,
                    "BOOT_B",
                    ManifestSlot::B,
                    &profile,
                    source_date_epoch,
                    &signing_key,
                    platform_id,
                    key_id,
                    release_id,
                    &kernel,
                    &init_bundle,
                )?);
            }
            "RECOVERY" => {
                components.extend(write_boot_partition(
                    partition,
                    "RECOVERY",
                    ManifestSlot::Recovery,
                    &profile,
                    source_date_epoch,
                    &signing_key,
                    platform_id,
                    key_id,
                    release_id,
                    &kernel,
                    &init_bundle,
                )?);
            }
            _ => {}
        }
    }

    let parsed = parse_gpt(&image, layout.sector_size)
        .map_err(|error| invalid(format!("self-verification of raw GPT failed: {error:?}")))?;
    if usize::from(parsed.partition_count) != placements.len() {
        return Err(invalid("self-verification partition count mismatch"));
    }
    let mut map_partitions = Vec::new();
    for placement in &placements {
        let start = byte_offset(placement.first_lba, layout.sector_size)?;
        let end = byte_offset(placement.last_lba + 1, layout.sector_size)?;
        map_partitions.push(ImageMapPartition {
            role: placement.role.clone(),
            type_guid: placement.type_guid_text.clone(),
            unique_guid: placement.unique_guid_text.clone(),
            first_lba: placement.first_lba,
            last_lba: placement.last_lba,
            size_bytes: u64::try_from(end - start)?,
            sha256: hex(&Sha256::digest(&image[start..end])),
        });
    }
    let map = ImageMap {
        schema: "eneros.image-map.v1",
        profile: profile.id.clone(),
        source_date_epoch,
        disk_size_bytes: layout.disk_size_bytes,
        sector_size: layout.sector_size,
        disk_guid: layout.disk_guid.clone(),
        platform_id_hex: hex(&platform_id),
        release_key_id_hex: hex(&key_id),
        release_public_key_hex: hex(&public_key),
        release_id_hex: hex(&release_id),
        partitions: map_partitions,
        components,
    };
    let output = target_output(
        root,
        &options
            .output
            .unwrap_or_else(|| directory.join(format!("eneros-{}.raw", profile.id))),
        "raw image",
    )?;
    write_candidate(&output, &image)?;
    let map_path = directory.join("image-map.json");
    fs::write(&map_path, serde_json::to_vec_pretty(&map)?)?;
    let manifest_a = extract_manifest(&image, &placements, "BOOT_A", layout.sector_size)?;
    let manifest_b = extract_manifest(&image, &placements, "BOOT_B", layout.sector_size)?;
    let manifest_recovery = extract_manifest(&image, &placements, "RECOVERY", layout.sector_size)?;
    fs::write(directory.join("release-manifest-A.bin"), manifest_a)?;
    fs::write(directory.join("release-manifest-B.bin"), manifest_b)?;
    fs::write(
        directory.join("release-manifest-Recovery.bin"),
        manifest_recovery,
    )?;
    fs::write(directory.join("eneros-init.bundle"), &init_bundle)?;
    fs::write(directory.join("eneros-product-kernel.bin"), &kernel.bytes)?;
    fs::write(directory.join("boot-control-initial.bin"), boot_control)?;
    fs::write(directory.join("release-public-key.bin"), public_key)?;
    let build_identity_path = directory.join("build-identity.json");
    fs::write(
        &build_identity_path,
        serde_json::to_vec_pretty(&BuildIdentity {
            schema: "eneros.build-identity.v1",
            profile: profile.id.clone(),
            workspace_version: env!("CARGO_PKG_VERSION"),
            source_commit: source_commit(root)?,
            source_date_epoch,
            cargo_lock_sha256: hex(&Sha256::digest(fs::read(root.join("Cargo.lock"))?)),
            build_id_hex: hex(&release_id),
        })?,
    )?;

    let artifact_paths = vec![
        output.clone(),
        map_path,
        directory.join("release-manifest-A.bin"),
        directory.join("release-manifest-B.bin"),
        directory.join("release-manifest-Recovery.bin"),
        directory.join("eneros-init.bundle"),
        directory.join("eneros-product-kernel.bin"),
        directory.join("boot-control-initial.bin"),
        directory.join("release-public-key.bin"),
        build_identity_path,
        directory.join("BOOTAA64.EFI"),
    ];
    let mut artifacts = BTreeMap::new();
    for path in &artifact_paths {
        let bytes = fs::read(path)?;
        artifacts.insert(
            path.file_name()
                .ok_or_else(|| invalid("artifact has no file name"))?
                .to_string_lossy()
                .into_owned(),
            ArtifactHash {
                size_bytes: u64::try_from(bytes.len())?,
                sha256: hex(&Sha256::digest(&bytes)),
            },
        );
    }
    fs::write(
        directory.join("artifact-sha256.json"),
        serde_json::to_vec_pretty(&ArtifactHashes {
            schema: "eneros.artifact-hashes.v1",
            artifacts,
        })?,
    )?;
    println!(
        "IMAGE profile={} result=PASS path={} bytes={} sha256={}",
        profile.id,
        output.display(),
        image.len(),
        hex(&Sha256::digest(&image))
    );
    Ok(())
}

fn required_epoch() -> DynResult<u64> {
    let text = env::var("SOURCE_DATE_EPOCH")
        .map_err(|_| invalid("SOURCE_DATE_EPOCH is mandatory for image generation"))?;
    let value = text
        .parse::<u64>()
        .map_err(|_| invalid("SOURCE_DATE_EPOCH must be an unsigned integer"))?;
    if value == 0 {
        return Err(invalid("SOURCE_DATE_EPOCH must be non-zero"));
    }
    Ok(value)
}

fn source_commit(root: &Path) -> DynResult<String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "HEAD"])
        .output()?;
    if !output.status.success() {
        return Err(invalid("git rev-parse HEAD failed for build identity"));
    }
    let commit = String::from_utf8(output.stdout)?.trim().to_owned();
    if commit.len() != 40
        || !commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(invalid("source commit is not a canonical lowercase SHA-1"));
    }
    Ok(commit)
}

fn iso_timestamp(source_date_epoch: u64) -> DynResult<String> {
    let mut command = if cfg!(windows) {
        let mut command = Command::new("wsl.exe");
        command.args(["-d", "Ubuntu-22.04", "--", "date"]);
        command
    } else {
        Command::new("date")
    };
    let epoch = format!("@{source_date_epoch}");
    command.args(["-u", "-d", &epoch, "+%Y%m%d%H%M%S00"]);
    let output = command.output()?;
    if !output.status.success() {
        return Err(invalid("failed to derive deterministic ISO timestamp"));
    }
    let timestamp = String::from_utf8(output.stdout)?.trim().to_owned();
    if timestamp.len() != 16 || !timestamp.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid("deterministic ISO timestamp has the wrong format"));
    }
    Ok(timestamp)
}

fn target_artifact(root: &Path, path: &Path) -> DynResult<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let candidate = candidate.canonicalize()?;
    let target = root.join("target").canonicalize()?;
    if !candidate.starts_with(&target) || !candidate.is_file() {
        return Err(invalid(
            "image factory input must be an existing file below target",
        ));
    }
    Ok(candidate)
}

fn target_output(root: &Path, path: &Path, kind: &str) -> DynResult<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let parent = candidate
        .parent()
        .ok_or_else(|| invalid(format!("{kind} output has no parent")))?;
    fs::create_dir_all(parent)?;
    let parent = parent.canonicalize()?;
    let target = root.join("target").canonicalize()?;
    if !parent.starts_with(&target) {
        return Err(invalid(format!("{kind} output must stay below target")));
    }
    Ok(parent.join(
        candidate
            .file_name()
            .ok_or_else(|| invalid(format!("{kind} output has no file name")))?,
    ))
}

pub(super) fn extract_gpt_manifest_bytes(
    image: &[u8],
    gpt: &Gpt,
    name: &str,
) -> DynResult<Vec<u8>> {
    let partition = gpt
        .find_by_name(name)
        .ok_or_else(|| invalid(format!("raw image has no {name} partition")))?;
    let start = usize::try_from(
        partition
            .first_lba
            .checked_mul(u64::from(gpt.sector_size))
            .ok_or_else(|| invalid("manifest offset overflow"))?,
    )?;
    let header = image
        .get(start..start + 16)
        .ok_or_else(|| invalid("release manifest header is truncated"))?;
    let length = usize::try_from(u32::from_le_bytes(header[12..16].try_into()?))?;
    image
        .get(start..start + length)
        .map(ToOwned::to_owned)
        .ok_or_else(|| invalid("release manifest is truncated"))
}

fn extract_gpt_manifest(image: &[u8], gpt: &Gpt, name: &str) -> DynResult<ReleaseManifest> {
    let encoded = extract_gpt_manifest_bytes(image, gpt, name)?;
    ReleaseManifest::decode(&encoded)
        .map_err(|error| invalid(format!("release manifest decode failed: {error:?}")))
}

fn build_uefi_manager(
    root: &Path,
    public_key: &[u8; 32],
    key_id: &[u8; 16],
    platform_id: &[u8; 16],
) -> DynResult<PathBuf> {
    let mut command = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()));
    command
        .current_dir(root)
        .env("ENEROS_RELEASE_PUBLIC_KEY_HEX", hex(public_key))
        .env("ENEROS_RELEASE_KEY_ID_HEX", hex(key_id))
        .env("ENEROS_PLATFORM_ID_HEX", hex(platform_id))
        .args([
            "build",
            "--locked",
            "--release",
            "-Z",
            "build-std=core,alloc",
            "-Z",
            "build-std-features=compiler-builtins-mem",
            "--target",
            "aarch64-unknown-uefi",
            "-p",
            "eneros-boot-manager-uefi",
            "--bin",
            "boot-manager-uefi",
            "--no-default-features",
            "--features",
            "uefi-app",
        ]);
    run(&mut command)?;
    let path = root.join("target/aarch64-unknown-uefi/release/boot-manager-uefi.efi");
    if !path.is_file() {
        return Err(invalid(format!(
            "UEFI build succeeded without expected artifact {}",
            path.display()
        )));
    }
    Ok(path)
}

fn build_uefi_installer(
    root: &Path,
    public_key: &[u8; 32],
    key_id: &[u8; 16],
    platform_id: &[u8; 16],
    minimum_security_version: u64,
) -> DynResult<PathBuf> {
    let mut command = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()));
    command
        .current_dir(root)
        .env("ENEROS_RELEASE_PUBLIC_KEY_HEX", hex(public_key))
        .env("ENEROS_RELEASE_KEY_ID_HEX", hex(key_id))
        .env("ENEROS_PLATFORM_ID_HEX", hex(platform_id))
        .env(
            "ENEROS_MINIMUM_SECURITY_VERSION",
            minimum_security_version.to_string(),
        )
        .args([
            "build",
            "--locked",
            "--release",
            "-Z",
            "build-std=core,alloc",
            "-Z",
            "build-std-features=compiler-builtins-mem",
            "--target",
            "aarch64-unknown-uefi",
            "-p",
            "eneros-installer-uefi",
            "--bin",
            "installer-uefi",
            "--no-default-features",
            "--features",
            "uefi-app",
        ]);
    run(&mut command)?;
    let path = root.join("target/aarch64-unknown-uefi/release/installer-uefi.efi");
    if !path.is_file() {
        return Err(invalid(format!(
            "installer UEFI build succeeded without {}",
            path.display()
        )));
    }
    Ok(path)
}

fn sign_uefi_application(
    root: &Path,
    unsigned: &Path,
    key: &str,
    cert: &str,
    directory: &Path,
) -> DynResult<Vec<u8>> {
    let output = directory.join("BOOTAA64.EFI");
    if cfg!(windows) {
        let unsigned_wsl = wsl_path(root, unsigned)?;
        let output_wsl = wsl_path(root, &output)?;
        let key_wsl = wsl_path(root, Path::new(key))?;
        let cert_wsl = wsl_path(root, Path::new(cert))?;
        let mut command = Command::new("wsl.exe");
        command.args([
            "-d",
            "Ubuntu-22.04",
            "--",
            "sbsign",
            "--key",
            &key_wsl,
            "--cert",
            &cert_wsl,
            "--output",
            &output_wsl,
            &unsigned_wsl,
        ]);
        run(&mut command)?;
        let mut verify = Command::new("wsl.exe");
        verify.args([
            "-d",
            "Ubuntu-22.04",
            "--",
            "sbverify",
            "--cert",
            &cert_wsl,
            &output_wsl,
        ]);
        run(&mut verify)?;
    } else {
        let mut command = Command::new("sbsign");
        command.args(["--key", key, "--cert", cert, "--output"]);
        command.arg(&output).arg(unsigned);
        run(&mut command)?;
        let mut verify = Command::new("sbverify");
        verify.args(["--cert", cert]).arg(&output);
        run(&mut verify)?;
    }
    let signed = fs::read(&output)?;
    if signed == fs::read(unsigned)? {
        return Err(invalid("UEFI signer returned an unchanged application"));
    }
    Ok(signed)
}

fn verify_pre_signed_uefi(
    root: &Path,
    unsigned: &Path,
    signed: &Path,
    cert: &str,
    directory: &Path,
) -> DynResult<Vec<u8>> {
    if !signed.is_file() {
        return Err(invalid(format!(
            "pre-signed UEFI application does not exist: {}",
            signed.display()
        )));
    }
    let destination = directory.join("BOOTAA64.EFI");
    let stripped = directory.join("BOOTAA64.stripped.efi");
    if cfg!(windows) {
        let signed_wsl = wsl_path(root, signed)?;
        let unsigned_wsl = wsl_path(root, unsigned)?;
        let destination_wsl = wsl_path(root, &destination)?;
        let stripped_wsl = wsl_path(root, &stripped)?;
        let mut verify = Command::new("wsl.exe");
        verify.args([
            "-d",
            "Ubuntu-22.04",
            "--",
            "sbverify",
            "--cert",
            cert,
            &signed_wsl,
        ]);
        run(&mut verify)?;
        let mut copy = Command::new("wsl.exe");
        copy.args(["-d", "Ubuntu-22.04", "--", "cp", &signed_wsl, &stripped_wsl]);
        run(&mut copy)?;
        let mut remove = Command::new("wsl.exe");
        remove.args([
            "-d",
            "Ubuntu-22.04",
            "--",
            "sbattach",
            "--remove",
            &stripped_wsl,
        ]);
        run(&mut remove)?;
        let mut compare = Command::new("wsl.exe");
        compare.args([
            "-d",
            "Ubuntu-22.04",
            "--",
            "cmp",
            "--silent",
            &stripped_wsl,
            &unsigned_wsl,
        ]);
        run(&mut compare)?;
        let mut install = Command::new("wsl.exe");
        install.args([
            "-d",
            "Ubuntu-22.04",
            "--",
            "cp",
            &signed_wsl,
            &destination_wsl,
        ]);
        run(&mut install)?;
    } else {
        let mut verify = Command::new("sbverify");
        verify.args(["--cert", cert]).arg(signed);
        run(&mut verify)?;
        fs::copy(signed, &stripped)?;
        let mut remove = Command::new("sbattach");
        remove.args(["--remove"]).arg(&stripped);
        run(&mut remove)?;
        if fs::read(&stripped)? != fs::read(unsigned)? {
            return Err(invalid(
                "pre-signed UEFI application does not contain the current boot manager",
            ));
        }
        fs::copy(signed, &destination)?;
    }
    if stripped.exists() {
        fs::remove_file(&stripped)?;
    }
    Ok(fs::read(destination)?)
}

pub(super) fn wsl_path(root: &Path, path: &Path) -> DynResult<String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let canonical = if absolute.exists() {
        absolute.canonicalize()?
    } else {
        let parent = absolute
            .parent()
            .ok_or_else(|| invalid("Windows path has no parent"))?
            .canonicalize()?;
        parent.join(
            absolute
                .file_name()
                .ok_or_else(|| invalid("Windows path has no file name"))?,
        )
    };
    let text = canonical.to_string_lossy();
    if !cfg!(windows) {
        return Ok(text.into_owned());
    }
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    let raw = text.as_bytes();
    if raw.len() < 3 || raw[1] != b':' || !raw[0].is_ascii_alphabetic() {
        return Err(invalid(format!(
            "cannot map non-drive Windows path into WSL: {text}"
        )));
    }
    let drive = char::from(raw[0].to_ascii_lowercase());
    let tail = text[2..].trim_start_matches(['\\', '/']).replace('\\', "/");
    Ok(format!("/mnt/{drive}/{tail}"))
}

fn build_and_pack_kernel(root: &Path, _source_date_epoch: u64) -> DynResult<PackedKernel> {
    // 自研内核（eneros-microkernel）已删除（ADR-0023：内核基座迁移 seL4）。
    // seL4 elfloader 镜像由 third_party/sel4-build.sh 构建；UEFI 启动链整合
    // （boot manager 加载 elfloader EFI）为遗留工作，本函数 fail-closed。
    let sel4_elf = root.join("target/sel4/elfloader-arm-qemu-arm-virt");
    if sel4_elf.is_file() {
        let elf = fs::read(&sel4_elf)?;
        return pack_elf64(&elf);
    }
    Err(invalid(
        "自研内核已迁移 seL4（ADR-0023）：请先运行 third_party/sel4-build.sh \
         并设置 ELFLOADER_IMAGE=elf 产物到 target/sel4/；UEFI 链整合为遗留工作",
    ))
}

fn pack_elf64(elf: &[u8]) -> DynResult<PackedKernel> {
    if elf.len() < 64
        || &elf[..4] != b"\x7fELF"
        || elf[4] != 2
        || elf[5] != 1
        || elf[6] != 1
        || get_u16(elf, 16)? != 2
        || get_u16(elf, 18)? != 183
    {
        return Err(invalid(
            "kernel is not canonical little-endian AArch64 ELF64 ET_EXEC",
        ));
    }
    let entry = get_u64(elf, 24)?;
    let phoff = usize::try_from(get_u64(elf, 32)?)?;
    let phentsize = usize::from(get_u16(elf, 54)?);
    let phnum = usize::from(get_u16(elf, 56)?);
    if phentsize != 56 || phnum == 0 || phnum > 64 {
        return Err(invalid("kernel ELF program-header geometry is invalid"));
    }
    let table_end = phoff
        .checked_add(
            phentsize
                .checked_mul(phnum)
                .ok_or_else(|| invalid("ELF phdr overflow"))?,
        )
        .ok_or_else(|| invalid("ELF phdr overflow"))?;
    if table_end > elf.len() {
        return Err(invalid("kernel ELF program-header table is truncated"));
    }
    let mut segments = Vec::new();
    let mut maximum = KERNEL_LOAD_ADDRESS;
    let mut entry_executable = false;
    for index in 0..phnum {
        let offset = phoff + index * phentsize;
        if get_u32(elf, offset)? != 1 {
            continue;
        }
        let flags = get_u32(elf, offset + 4)?;
        let file_offset = usize::try_from(get_u64(elf, offset + 8)?)?;
        let virtual_address = get_u64(elf, offset + 16)?;
        let physical_address = get_u64(elf, offset + 24)?;
        let file_size = usize::try_from(get_u64(elf, offset + 32)?)?;
        let memory_size = usize::try_from(get_u64(elf, offset + 40)?)?;
        let alignment = get_u64(elf, offset + 48)?;
        if virtual_address != physical_address
            || file_size > memory_size
            || memory_size == 0
            || alignment == 0
            || !alignment.is_power_of_two()
            || flags & 0x3 == 0x3
        {
            return Err(invalid("kernel ELF has unsafe LOAD segment semantics"));
        }
        let file_end = file_offset
            .checked_add(file_size)
            .ok_or_else(|| invalid("kernel ELF file range overflow"))?;
        let memory_end = virtual_address
            .checked_add(u64::try_from(memory_size)?)
            .ok_or_else(|| invalid("kernel ELF memory range overflow"))?;
        if file_end > elf.len()
            || virtual_address < KERNEL_LOAD_ADDRESS
            || memory_end > KERNEL_LOAD_ADDRESS + 8 * 1024 * 1024
        {
            return Err(invalid(
                "kernel ELF LOAD segment exceeds the frozen load window",
            ));
        }
        if flags & 1 != 0 && entry >= virtual_address && entry < memory_end {
            entry_executable = true;
        }
        for (existing_start, existing_end, _, _, _) in &segments {
            if virtual_address < *existing_end && *existing_start < memory_end {
                return Err(invalid("kernel ELF LOAD segments overlap"));
            }
        }
        maximum = maximum.max(memory_end);
        segments.push((virtual_address, memory_end, file_offset, file_size, flags));
    }
    if segments.is_empty() || !entry_executable || entry < KERNEL_LOAD_ADDRESS {
        return Err(invalid(
            "kernel ELF entry is not inside an executable LOAD segment",
        ));
    }
    segments.sort_by_key(|segment| segment.0);
    let memory_length = maximum
        .checked_sub(KERNEL_LOAD_ADDRESS)
        .ok_or_else(|| invalid("kernel ELF load base underflow"))?;
    let mut bytes = vec![0u8; usize::try_from(memory_length)?];
    for (address, _, file_offset, file_size, _) in segments {
        let destination = usize::try_from(address - KERNEL_LOAD_ADDRESS)?;
        bytes[destination..destination + file_size]
            .copy_from_slice(&elf[file_offset..file_offset + file_size]);
    }
    Ok(PackedKernel {
        bytes,
        entry,
        memory_length,
    })
}

fn build_init_bundle(
    root: &Path,
    source_date_epoch: u64,
    profile: &ProductProfile,
) -> DynResult<(Vec<u8>, crate::product_manifest::ProductManifest)> {
    // 蓝图 36 WP-36.4：init bundle 条目只从规范化 ProductManifest 派生，
    // 禁止 image factory 维护独立硬编码服务列表。
    let mut manifest = crate::product_manifest::normalize_profile(root, profile)?;
    let bundle = crate::product_deriver::init_bundle(&manifest)
        .map_err(|e| invalid(format!("init bundle derive failed: {e}")))?;
    let mut modules: Vec<(u32, u32, String, Vec<u8>)> = Vec::new();
    let mut service_digests: BTreeMap<String, String> = BTreeMap::new();
    for entry in &bundle.entries {
        if entry.external {
            return Err(invalid(format!(
                "init bundle entry must be a cargo-built service: {}",
                entry.name
            )));
        }
        build_user_elf(root, &entry.package, &entry.bin, &profile.id)?;
        let target_dir = if entry.target.is_empty() {
            "aarch64-unknown-none".to_owned()
        } else {
            entry.target.clone()
        };
        let elf_path = root
            .join("target")
            .join(&target_dir)
            .join("release")
            .join(&entry.bin);
        let elf = fs::read(&elf_path)
            .map_err(|e| invalid(format!("bundle elf missing {}: {e}", elf_path.display())))?;
        service_digests.insert(
            entry.name.clone(),
            crate::product_deriver::hex(&Sha256::digest(&elf)),
        );
        modules.push((entry.kind, entry.flags, entry.name.clone(), elf));
    }
    if modules.is_empty() {
        return Err(invalid(
            "init bundle is empty (manifest declares no bundled service)",
        ));
    }
    let modules_ref: Vec<(u32, u32, &str, &[u8])> = modules
        .iter()
        .map(|(kind, flags, name, blob)| (*kind, *flags, name.as_str(), blob.as_slice()))
        .collect();
    let bytes = compose_init_bundle(source_date_epoch, &modules_ref)?;
    // 回填服务 digest（kernel digest 由调用方回填）——先绑定再反解析，
    // 使 verify_media_bundle 的 digest 交叉比对（WP-36.4 “与 Binary BOM
    // 一一比对”）真实生效（36 收尾复验修复：原顺序在绑定前反解析，
    // digest 比对被静默跳过）。
    manifest.bind_digests(&service_digests, "")?;
    // 反解析 Gate（WP-36.4）：构建后立即反解析，bundle 与 manifest 一一比对，
    // 多余二进制同样失败（防止 hidden payload）。
    let violations = crate::product_deriver::verify_media_bundle(&manifest, &bytes)?;
    if !violations.is_empty() {
        return Err(invalid(format!(
            "REVERSE-PARSE-GATE FAIL after compose: {}",
            violations.join("; ")
        )));
    }
    let manifest_dir = crate::product_manifest::manifest_dir(root, &manifest.profile_id);
    fs::create_dir_all(&manifest_dir)?;
    fs::write(manifest_dir.join("init-bundle.bin"), &bytes)?;
    fs::write(
        manifest_dir.join("product-manifest.normalized.json"),
        manifest.normalized_json()?,
    )?;
    Ok((bytes, manifest))
}

/// 构建用户 ELF（aarch64-unknown-none release）。
fn build_user_elf(root: &Path, package: &str, bin: &str, profile_id: &str) -> DynResult<()> {
    let mut command = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()));
    command.current_dir(root).args([
        "build",
        "--locked",
        "--release",
        "-Z",
        "build-std=core,alloc",
        "-Z",
        "build-std-features=compiler-builtins-mem",
        "--target",
        "aarch64-unknown-none",
        "-p",
        package,
        "--bin",
        bin,
    ]);
    // BOOT-013：sel4-sys 的 shim/libsel4 头必须与运行内核同源（top-rust，
    // label 偏移一致）。默认 build.rs 用 third_party/sel4-build/top（旧 C
    // root-task 构建，invocation label 不同）——用户态 ELF 与 top-rust
    // 内核 label 错位导致 IPC 等系统调用行为异常（QEMU 实测服务不响应）。
    // 显式指定 SEL4_BUILD_DIR=top-rust，与 sel4-build 产物（内核/elfloader）
    // 保持一致。
    command.env(
        "SEL4_BUILD_DIR",
        root.join("third_party/sel4-build/top-rust"),
    );
    command.env("CROSS_COMPILE", "aarch64-none-elf-");
    // WP-37.4（CROSS_VERSION_CHANGE_REQUEST：37 需要，36 owner 文件最小
    // 特判）：svc-pie 是静态 PIE（ET_DYN）——PIC 代码生成仅对该 crate
    // 生效（--config 覆盖 config.toml 的同 key rustflags；QEMU 判别实验
    // 证实全局 PIC 会使 ET_EXEC 服务在 seL4 EL0 故障——PIC+EXEC 组合
    // 不可用，PIC+PIE 正常）。其余服务保持非 PIC 编译。
    if package == "eneros-svc-pie" {
        command.args([
            "--config",
            "target.aarch64-unknown-none.rustflags=[\"-C\",\"relocation-model=pic\"]",
        ]);
    }
    // 蓝图 40 收尾（CROSS_VERSION_CHANGE_REQUEST：EIPC 生产通道 target
    // 构建）：控制服务（Sel4EipcChannel/ChunkedClient）与 root-task
    // （Sel4IpcClient 驱动控制链）在 target 构建启用 sel4 feature；
    // host 构建/测试不启用（sel4-sys 不进 host 依赖图）。
    if package == "eneros-control-services" {
        command.args(["--features", "sel4"]);
    }
    // 45.5（遥测通路）：协议服务侧发送端（Sel4IpcClient + sample_wire）——
    // EL0 target 构建启用 protocol-services sel4 feature（host 不启用）。
    if package == "eneros-protocol-services" {
        command.args(["--features", "sel4"]);
    }
    // 41（WP-41.3 落盘）：业务服务（svc-ota 等）经 EIPC 调 svc-storage
    // 真落盘——Sel4IpcClient（agent-sdk ipc_sel4）需 sel4 feature。
    if package == "eneros-business-services" {
        command.args(["--features", "sel4"]);
    }
    if package == "eneros-root-task" {
        // 45.5（合成源降级）：qemu-test 介质启 `qemu-test-synth`（合成遥测
        // 仅测试面）；qemu-prod/board-prod 等 release 装配零合成路径——
        // closure gate 的 `DENY_FEATURE_PATTERNS + qemu-test-synth` 强制该
        // feature 不出现在 release 闭包。
        if profile_id == "qemu-test" {
            command.args(["--features", "sel4-eipc,qemu-test-synth"]);
        } else {
            command.args(["--features", "sel4-eipc"]);
        }
    }
    run(&mut command)
}

/// 组装 init bundle v2（SVC-002 schema）。
fn compose_init_bundle(
    source_date_epoch: u64,
    modules: &[(u32, u32, &str, &[u8])],
) -> DynResult<Vec<u8>> {
    let header_len = 64usize;
    let entry_len = 96usize;
    let table_len = header_len
        .checked_add(
            entry_len
                .checked_mul(modules.len())
                .ok_or_else(|| invalid("bundle table overflow"))?,
        )
        .ok_or_else(|| invalid("bundle table overflow"))?;
    let mut offset = table_len;
    for (_, _, _, blob) in modules {
        offset = offset
            .checked_add(blob.len())
            .ok_or_else(|| invalid("bundle payload overflow"))?;
    }
    let total = usize::try_from(u32::try_from(offset)?)?;
    let mut bytes = vec![0u8; total];
    bytes[..8].copy_from_slice(b"ENRINIT2");
    bytes[8..10].copy_from_slice(&2u16.to_le_bytes()); // WP-37.2 wire v3：flags 高 16 位携带能力位图
    bytes[10..12].copy_from_slice(&u16::try_from(header_len)?.to_le_bytes());
    bytes[12..16].copy_from_slice(&u32::try_from(total)?.to_le_bytes());
    bytes[16..24].copy_from_slice(&source_date_epoch.to_le_bytes());
    bytes[24..28].copy_from_slice(&u32::try_from(modules.len())?.to_le_bytes());
    let mut payload = Vec::new();
    let mut cursor = table_len;
    for (index, (kind, flags, name, blob)) in modules.iter().enumerate() {
        let entry_offset = header_len + index * entry_len;
        bytes[entry_offset..entry_offset + 4].copy_from_slice(&kind.to_le_bytes());
        bytes[entry_offset + 4..entry_offset + 8].copy_from_slice(&flags.to_le_bytes());
        bytes[entry_offset + 8..entry_offset + 16]
            .copy_from_slice(&u64::try_from(cursor)?.to_le_bytes());
        bytes[entry_offset + 16..entry_offset + 24]
            .copy_from_slice(&u64::try_from(blob.len())?.to_le_bytes());
        let name_bytes = name.as_bytes();
        if name_bytes.len() > 32 {
            return Err(invalid("module name exceeds 32 bytes"));
        }
        bytes[entry_offset + 24..entry_offset + 24 + name_bytes.len()].copy_from_slice(name_bytes);
        let digest: [u8; 32] = Sha256::digest(blob).into();
        bytes[entry_offset + 56..entry_offset + 88].copy_from_slice(&digest);
        bytes[cursor..cursor + blob.len()].copy_from_slice(blob);
        payload.extend_from_slice(blob);
        cursor += blob.len();
    }
    let payload_digest: [u8; 32] = Sha256::digest(&payload).into();
    bytes[32..64].copy_from_slice(&payload_digest);
    Ok(bytes)
}

#[allow(clippy::too_many_arguments)]
fn write_boot_partition(
    partition: &mut [u8],
    partition_name: &str,
    slot: ManifestSlot,
    profile: &ProductProfile,
    source_date_epoch: u64,
    signing_key: &SigningKey,
    platform_id: [u8; 16],
    key_id: [u8; 16],
    release_id: [u8; 16],
    kernel: &PackedKernel,
    init_bundle: &[u8],
) -> DynResult<Vec<ImageMapComponent>> {
    let kernel_offset = MANIFEST_RESERVED_BYTES;
    let init_offset = align_up(
        kernel_offset
            .checked_add(kernel.bytes.len())
            .ok_or_else(|| invalid("boot partition offset overflow"))?,
        4096,
    )?;
    let end = init_offset
        .checked_add(init_bundle.len())
        .ok_or_else(|| invalid("boot partition end overflow"))?;
    if end > partition.len() {
        return Err(invalid(format!(
            "{partition_name} exceeds its audited size budget"
        )));
    }
    let security_version = u64::from(profile.security.minimum_security_version.max(1));
    let kernel_digest: [u8; 32] = Sha256::digest(&kernel.bytes).into();
    let init_digest: [u8; 32] = Sha256::digest(init_bundle).into();
    let version = ProductVersion {
        major: 0,
        minor: 192,
        patch: 0,
    };
    let components = vec![
        Component {
            role: ComponentRole::Kernel,
            slot,
            permissions: PERMISSION_READ | PERMISSION_EXECUTE,
            flags: COMPONENT_FLAG_REQUIRED,
            media_offset: u64::try_from(kernel_offset)?,
            media_length: u64::try_from(kernel.bytes.len())?,
            load_address: KERNEL_LOAD_ADDRESS,
            memory_length: kernel.memory_length,
            entry_address: kernel.entry,
            security_version,
            alignment: 4096,
            digest: kernel_digest,
            content_id: first_16(kernel_digest),
            version,
        },
        Component {
            role: ComponentRole::InitBundle,
            slot,
            permissions: PERMISSION_READ,
            flags: COMPONENT_FLAG_REQUIRED,
            media_offset: u64::try_from(init_offset)?,
            media_length: u64::try_from(init_bundle.len())?,
            load_address: INIT_LOAD_ADDRESS,
            memory_length: u64::try_from(init_bundle.len())?,
            entry_address: 0,
            security_version,
            alignment: 4096,
            digest: init_digest,
            content_id: first_16(init_digest),
            version,
        },
    ];
    let unsigned_manifest = ReleaseManifest::new(
        ManifestHeader {
            flags: 0,
            platform_id,
            product_version: version,
            security_version,
            source_date_epoch,
            key_id,
            release_id,
        },
        components.clone(),
    );
    let unsigned = unsigned_manifest
        .encode_unsigned()
        .map_err(|error| invalid(format!("manifest encode: {error:?}")))?;
    let signature = signing_key.sign(&unsigned).to_bytes();
    let encoded = unsigned_manifest
        .with_signature(signature)
        .encode_signed()
        .map_err(|error| invalid(format!("signed manifest encode: {error:?}")))?;
    if encoded.len() > MANIFEST_RESERVED_BYTES {
        return Err(invalid("canonical manifest exceeds reserved boot area"));
    }
    partition[..encoded.len()].copy_from_slice(&encoded);
    partition[kernel_offset..kernel_offset + kernel.bytes.len()].copy_from_slice(&kernel.bytes);
    partition[init_offset..init_offset + init_bundle.len()].copy_from_slice(init_bundle);
    Ok(components
        .into_iter()
        .map(|component| ImageMapComponent {
            slot: match slot {
                ManifestSlot::A => "A",
                ManifestSlot::B => "B",
                ManifestSlot::Recovery => "RECOVERY",
                ManifestSlot::None => "NONE",
            }
            .to_owned(),
            role: format!("{:?}", component.role),
            partition: partition_name.to_owned(),
            media_offset: component.media_offset,
            media_length: component.media_length,
            load_address: component.load_address,
            memory_length: component.memory_length,
            entry_address: component.entry_address,
            sha256: hex(&component.digest),
        })
        .collect())
}

fn place_partitions(layout: &ImageLayout) -> DynResult<Vec<PartitionPlacement>> {
    let sector = u64::from(layout.sector_size);
    let alignment_lbas = layout.alignment_bytes / sector;
    let mut next_lba = alignment_lbas;
    let mut placements = Vec::new();
    for (index, partition) in layout.partitions.iter().enumerate() {
        next_lba = align_up_u64(next_lba, alignment_lbas)?;
        let lbas = partition.size_bytes / sector;
        let last_lba = next_lba
            .checked_add(lbas)
            .and_then(|value| value.checked_sub(1))
            .ok_or_else(|| invalid("partition placement overflow"))?;
        let unique_bytes =
            Sha256::digest([layout.disk_guid.as_bytes(), b"/", partition.role.as_bytes()].concat());
        let mut unique = first_16(unique_bytes);
        unique[6] = (unique[6] & 0x0f) | 0x40;
        unique[8] = (unique[8] & 0x3f) | 0x80;
        let unique_text = guid_text(&unique);
        placements.push(PartitionPlacement {
            role: partition.role.clone(),
            type_guid: Guid::parse(&partition.type_guid)
                .map_err(|error| invalid(format!("partition type GUID: {error:?}")))?,
            type_guid_text: partition.type_guid.clone(),
            unique_guid: Guid::parse(&unique_text)
                .map_err(|error| invalid(format!("derived unique GUID: {error:?}")))?,
            unique_guid_text: unique_text,
            first_lba: next_lba,
            last_lba,
            attributes: partition.attributes,
        });
        next_lba = last_lba
            .checked_add(1)
            .ok_or_else(|| invalid(format!("partition {index} placement overflow")))?;
    }
    let total_lbas = layout.disk_size_bytes / sector;
    if placements
        .last()
        .map_or(true, |placement| placement.last_lba >= total_lbas - 34)
    {
        return Err(invalid("partition placement collides with backup GPT"));
    }
    Ok(placements)
}

fn build_fat16(size: usize, hidden_sectors: u32, efi: &[u8]) -> DynResult<Vec<u8>> {
    const SECTOR: usize = 512;
    const RESERVED: usize = 1;
    const FATS: usize = 2;
    const ROOT_ENTRIES: usize = 512;
    const ROOT_SECTORS: usize = ROOT_ENTRIES * 32 / SECTOR;
    if size % SECTOR != 0 || size < 4 * 1024 * 1024 {
        return Err(invalid(
            "ESP must be a sector-aligned FAT16 volume of at least 4 MiB",
        ));
    }
    let total_sectors = size / SECTOR;
    if total_sectors > usize::from(u16::MAX) {
        return Err(invalid("minimal ESP exceeds FAT16 total-sector field"));
    }
    let mut fat_sectors = 1usize;
    loop {
        let data_sectors = total_sectors
            .checked_sub(RESERVED + FATS * fat_sectors + ROOT_SECTORS)
            .ok_or_else(|| invalid("FAT16 geometry underflow"))?;
        let required = (data_sectors + 2)
            .checked_mul(2)
            .and_then(|value| value.checked_add(SECTOR - 1))
            .map(|value| value / SECTOR)
            .ok_or_else(|| invalid("FAT16 geometry overflow"))?;
        if required == fat_sectors {
            break;
        }
        fat_sectors = required;
    }
    let data_start = RESERVED + FATS * fat_sectors + ROOT_SECTORS;
    let cluster_count = total_sectors - data_start;
    if !(4085..65525).contains(&cluster_count) {
        return Err(invalid("ESP geometry does not classify as FAT16"));
    }
    let file_clusters = efi.len().div_ceil(SECTOR);
    if file_clusters == 0 || 4 + file_clusters > cluster_count + 2 {
        return Err(invalid("BOOTAA64.EFI does not fit in the ESP"));
    }
    let mut volume = vec![0u8; size];
    let boot = &mut volume[..SECTOR];
    boot[..3].copy_from_slice(&[0xeb, 0x3c, 0x90]);
    boot[3..11].copy_from_slice(b"ENEROS  ");
    put_u16(boot, 11, SECTOR as u16)?;
    boot[13] = 1;
    put_u16(boot, 14, RESERVED as u16)?;
    boot[16] = FATS as u8;
    put_u16(boot, 17, ROOT_ENTRIES as u16)?;
    put_u16(boot, 19, total_sectors as u16)?;
    boot[21] = 0xf8;
    put_u16(boot, 22, u16::try_from(fat_sectors)?)?;
    put_u16(boot, 24, 63)?;
    put_u16(boot, 26, 255)?;
    put_u32(boot, 28, hidden_sectors)?;
    boot[36] = 0x80;
    boot[38] = 0x29;
    let volume_id: [u8; 4] = Sha256::digest(efi)[..4]
        .try_into()
        .map_err(|_| invalid("volume id derivation"))?;
    boot[39..43].copy_from_slice(&volume_id);
    boot[43..54].copy_from_slice(b"ENEROSBOOT ");
    boot[54..62].copy_from_slice(b"FAT16   ");
    boot[510..512].copy_from_slice(&[0x55, 0xaa]);

    let mut fat = vec![0u8; fat_sectors * SECTOR];
    set_fat16(&mut fat, 0, 0xfff8)?;
    set_fat16(&mut fat, 1, 0xffff)?;
    set_fat16(&mut fat, 2, 0xffff)?;
    set_fat16(&mut fat, 3, 0xffff)?;
    for index in 0..file_clusters {
        let cluster = 4 + index;
        let next = if index + 1 == file_clusters {
            0xffff
        } else {
            u16::try_from(cluster + 1)?
        };
        set_fat16(&mut fat, cluster, next)?;
    }
    for copy in 0..FATS {
        let start = (RESERVED + copy * fat_sectors) * SECTOR;
        volume[start..start + fat.len()].copy_from_slice(&fat);
    }
    let root_start = (RESERVED + FATS * fat_sectors) * SECTOR;
    write_directory_entry(
        &mut volume[root_start..root_start + 32],
        b"EFI        ",
        0x10,
        2,
        0,
    )?;
    let cluster_offset = |cluster: usize| -> DynResult<usize> {
        data_start
            .checked_add(
                cluster
                    .checked_sub(2)
                    .ok_or_else(|| invalid("bad FAT cluster"))?,
            )
            .and_then(|sector| sector.checked_mul(SECTOR))
            .ok_or_else(|| invalid("FAT cluster offset overflow"))
    };
    let efi_dir = cluster_offset(2)?;
    write_directory_entry(
        &mut volume[efi_dir..efi_dir + 32],
        b".          ",
        0x10,
        2,
        0,
    )?;
    write_directory_entry(
        &mut volume[efi_dir + 32..efi_dir + 64],
        b"..         ",
        0x10,
        0,
        0,
    )?;
    write_directory_entry(
        &mut volume[efi_dir + 64..efi_dir + 96],
        b"BOOT       ",
        0x10,
        3,
        0,
    )?;
    let boot_dir = cluster_offset(3)?;
    write_directory_entry(
        &mut volume[boot_dir..boot_dir + 32],
        b".          ",
        0x10,
        3,
        0,
    )?;
    write_directory_entry(
        &mut volume[boot_dir + 32..boot_dir + 64],
        b"..         ",
        0x10,
        2,
        0,
    )?;
    write_directory_entry(
        &mut volume[boot_dir + 64..boot_dir + 96],
        b"BOOTAA64EFI",
        0x20,
        4,
        u32::try_from(efi.len())?,
    )?;
    let file_start = cluster_offset(4)?;
    volume[file_start..file_start + efi.len()].copy_from_slice(efi);
    Ok(volume)
}

fn write_directory_entry(
    entry: &mut [u8],
    name: &[u8; 11],
    attributes: u8,
    cluster: u16,
    size: u32,
) -> DynResult<()> {
    if entry.len() != 32 || name.contains(&0) {
        return Err(invalid("invalid FAT directory entry"));
    }
    entry.fill(0);
    entry[..11].copy_from_slice(name);
    entry[11] = attributes;
    put_u16(entry, 26, cluster)?;
    put_u32(entry, 28, size)?;
    Ok(())
}

fn set_fat16(fat: &mut [u8], cluster: usize, value: u16) -> DynResult<()> {
    let offset = cluster
        .checked_mul(2)
        .ok_or_else(|| invalid("FAT entry offset overflow"))?;
    put_u16(fat, offset, value)
}

fn release_identity(
    profile: &ProductProfile,
    epoch: u64,
    efi: &[u8],
    kernel: &[u8],
    init: &[u8],
) -> [u8; 16] {
    let mut hash = Sha256::new();
    hash.update(b"EnerOS/release-identity/v1");
    hash.update(profile.id.as_bytes());
    hash.update(epoch.to_le_bytes());
    hash.update(Sha256::digest(efi));
    hash.update(Sha256::digest(kernel));
    hash.update(Sha256::digest(init));
    first_16(hash.finalize())
}

fn extract_manifest<'a>(
    image: &'a [u8],
    placements: &[PartitionPlacement],
    role: &str,
    sector_size: u32,
) -> DynResult<&'a [u8]> {
    let placement = placements
        .iter()
        .find(|placement| placement.role == role)
        .ok_or_else(|| invalid(format!("missing {role} placement")))?;
    let start = byte_offset(placement.first_lba, sector_size)?;
    let header = image
        .get(start..start + 16)
        .ok_or_else(|| invalid("manifest header outside image"))?;
    let length = usize::try_from(u32::from_le_bytes(header[12..16].try_into()?))?;
    image
        .get(start..start + length)
        .ok_or_else(|| invalid("manifest outside image"))
}

fn write_candidate(path: &Path, bytes: &[u8]) -> DynResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("raw image path has no parent"))?;
    fs::create_dir_all(parent)?;
    let candidate = path.with_extension("raw.candidate");
    fs::write(&candidate, bytes)?;
    if fs::metadata(&candidate)?.len() != u64::try_from(bytes.len())? {
        return Err(invalid("candidate raw image short write"));
    }
    if path.exists() {
        let previous = path.with_extension("raw.previous");
        if previous.exists() {
            fs::remove_file(&previous)?;
        }
        fs::rename(path, &previous)?;
    }
    fs::rename(candidate, path)?;
    Ok(())
}

fn run(command: &mut Command) -> DynResult<()> {
    eprintln!("+ {:?}", command);
    let status = command.status()?;
    if !status.success() {
        return Err(invalid(format!("child command failed with {status}")));
    }
    Ok(())
}

fn first_16(bytes: impl AsRef<[u8]>) -> [u8; 16] {
    let mut output = [0u8; 16];
    output.copy_from_slice(&bytes.as_ref()[..16]);
    output
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn guid_text(bytes: &[u8; 16]) -> String {
    format!(
        "{}-{}-{}-{}-{}",
        hex(&bytes[0..4]),
        hex(&bytes[4..6]),
        hex(&bytes[6..8]),
        hex(&bytes[8..10]),
        hex(&bytes[10..16])
    )
}

fn align_up(value: usize, alignment: usize) -> DynResult<usize> {
    if !alignment.is_power_of_two() {
        return Err(invalid("alignment is not a power of two"));
    }
    value
        .checked_add(alignment - 1)
        .map(|value| value & !(alignment - 1))
        .ok_or_else(|| invalid("alignment overflow"))
}

fn align_up_u64(value: u64, alignment: u64) -> DynResult<u64> {
    if !alignment.is_power_of_two() {
        return Err(invalid("alignment is not a power of two"));
    }
    value
        .checked_add(alignment - 1)
        .map(|value| value & !(alignment - 1))
        .ok_or_else(|| invalid("alignment overflow"))
}

fn byte_offset(lba: u64, sector_size: u32) -> DynResult<usize> {
    usize::try_from(
        lba.checked_mul(u64::from(sector_size))
            .ok_or_else(|| invalid("byte offset overflow"))?,
    )
    .map_err(Into::into)
}

fn get_u16(bytes: &[u8], offset: usize) -> DynResult<u16> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or_else(|| invalid("truncated u16"))?
            .try_into()?,
    ))
}

fn get_u32(bytes: &[u8], offset: usize) -> DynResult<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or_else(|| invalid("truncated u32"))?
            .try_into()?,
    ))
}

fn get_u64(bytes: &[u8], offset: usize) -> DynResult<u64> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or_else(|| invalid("truncated u64"))?
            .try_into()?,
    ))
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) -> DynResult<()> {
    bytes
        .get_mut(offset..offset + 2)
        .ok_or_else(|| invalid("u16 write outside buffer"))?
        .copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) -> DynResult<()> {
    bytes
        .get_mut(offset..offset + 4)
        .ok_or_else(|| invalid("u32 write outside buffer"))?
        .copy_from_slice(&value.to_le_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fat16_is_deterministic_and_contains_fallback_path() {
        let app = vec![0x5au8; 70_000];
        let first = build_fat16(8 * 1024 * 1024, 2048, &app).unwrap();
        let second = build_fat16(8 * 1024 * 1024, 2048, &app).unwrap();
        assert_eq!(first, second);
        assert_eq!(&first[54..62], b"FAT16   ");
        assert_eq!(&first[510..512], &[0x55, 0xaa]);
        assert!(first.windows(11).any(|window| window == b"BOOTAA64EFI"));
        assert!(first.windows(app.len()).any(|window| window == app));
    }

    #[test]
    fn fat16_rejects_bad_geometry_and_oversized_app() {
        assert!(build_fat16(2 * 1024 * 1024, 2048, &[1]).is_err());
        assert!(build_fat16(8 * 1024 * 1024, 2048, &vec![0; 9 * 1024 * 1024]).is_err());
    }

    #[test]
    fn elf_parser_rejects_truncation_and_wrong_machine() {
        assert!(pack_elf64(&[0u8; 63]).is_err());
        let mut header = vec![0u8; 64];
        header[..4].copy_from_slice(b"\x7fELF");
        header[4] = 2;
        header[5] = 1;
        header[6] = 1;
        header[16..18].copy_from_slice(&2u16.to_le_bytes());
        header[18..20].copy_from_slice(&62u16.to_le_bytes());
        assert!(pack_elf64(&header).is_err());
    }

    #[test]
    fn init_bundle_is_self_describing_and_deterministic() {
        let blobs = [b"blob-a".to_vec(), b"blob-b".to_vec()];
        let modules: &[(u32, u32, &str, &[u8])] =
            &[(1, 31, "root", &blobs[0]), (2, 20, "svc", &blobs[1])];
        let first = compose_init_bundle(1_700_000_000, modules).unwrap();
        let second = compose_init_bundle(1_700_000_000, modules).unwrap();
        assert_eq!(first, second);
        assert_eq!(&first[..8], b"ENRINIT2");
        assert_eq!(
            u32::from_le_bytes(first[12..16].try_into().unwrap()) as usize,
            first.len()
        );
        assert_eq!(
            &first[32..64],
            Sha256::digest(&first[64 + 2 * 96..]).as_slice()
        );
    }
}
