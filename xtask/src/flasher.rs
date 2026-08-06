use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use eneros_image_format::parse_gpt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{invalid, DynResult};

const INVENTORY_SCHEMA: &str = "eneros.flash-device-inventory.v1";
const TOKEN_SCHEMA: &str = "eneros.flash-authorization.v1";
const REPORT_SCHEMA: &str = "eneros.flash-write-report.v1";

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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    authorization_sha256: String,
}

#[derive(Debug, Serialize)]
struct FlashReport {
    schema: &'static str,
    result: &'static str,
    target_kind: String,
    target_path: String,
    target_stable_id: String,
    image_path: String,
    bytes_written: u64,
    expected_sha256: String,
    readback_sha256: String,
    authorization_sha256: String,
    flush: &'static str,
    readback: &'static str,
    fault_injection: Option<String>,
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
            "flash requires list, verify-inventory, lab-create, authorize-file, or write",
        ));
    };
    match mode.as_str() {
        "list" => command_list(root, tail),
        "verify-inventory" => command_verify_inventory(root, tail),
        "lab-create" => command_lab_create(root, tail),
        "authorize-file" => command_authorize_file(root, tail),
        "write" => command_write(root, tail),
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

fn command_authorize_file(root: &Path, args: &[String]) -> DynResult<()> {
    if args.len() != 6 {
        return Err(invalid(
            "flash authorize-file requires --image <raw> --target <lab-file> --output <token>",
        ));
    }
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
        authorization_sha256: String::new(),
    };
    token.authorization_sha256 = token_digest(&token)?;
    fs::write(&output, serde_json::to_vec_pretty(&token)?)?;
    println!("{}", serde_json::to_string_pretty(&token)?);
    Ok(())
}

fn command_write(root: &Path, args: &[String]) -> DynResult<()> {
    let image_arg = value_after(args, "--image")?;
    let token_arg = value_after(args, "--authorization")?;
    let output_arg = value_after(args, "--output")?;
    let fault = optional_value_after(args, "--fault")?;
    let expected_len = if fault.is_some() { 8 } else { 6 };
    if args.len() != expected_len {
        return Err(invalid(
            "flash write requires --image <raw> --authorization <token> --output <report> [--fault post-write-corruption]",
        ));
    }
    if fault.is_some_and(|value| value != "post-write-corruption") {
        return Err(invalid("unsupported flash fault injection"));
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
    let current = fs::read(&target_path)?;
    if u64::try_from(current.len())? != token.target_size_bytes
        || hex(&Sha256::digest(&current)) != token.target_initial_sha256
    {
        return Err(invalid("flash target changed after authorization"));
    }
    write_and_sync(&target_path, &image)?;
    if fault.is_some() {
        inject_post_write_corruption(&target_path, image.len())?;
    }
    let readback = fs::read(&target_path)?;
    let readback_digest = hex(&Sha256::digest(&readback));
    let pass = readback.len() == image.len() && readback_digest == token.image_sha256;
    let report = FlashReport {
        schema: REPORT_SCHEMA,
        result: if pass { "PASS" } else { "FAIL" },
        target_kind: token.target_kind.clone(),
        target_path: token.target_path.clone(),
        target_stable_id: token.target_stable_id.clone(),
        image_path: token.image_path.clone(),
        bytes_written: u64::try_from(image.len())?,
        expected_sha256: token.image_sha256.clone(),
        readback_sha256: readback_digest,
        authorization_sha256: token.authorization_sha256.clone(),
        flush: "PASS",
        readback: if pass { "PASS" } else { "FAIL" },
        fault_injection: fault.map(ToOwned::to_owned),
    };
    fs::write(&output, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !pass {
        return Err(invalid("flash readback digest differs from the image"));
    }
    Ok(())
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

fn verify_token(token: &AuthorizationToken) -> DynResult<()> {
    if token.schema != TOKEN_SCHEMA
        || token.target_kind != "file-lab"
        || token.target_size_bytes == 0
        || token.target_size_bytes != token.image_size_bytes
        || !matches!(token.logical_sector_size, 512 | 4096)
        || token.authorization_sha256 != token_digest(token)?
    {
        return Err(invalid("flash authorization token is invalid or mutated"));
    }
    Ok(())
}

fn token_digest(token: &AuthorizationToken) -> DynResult<String> {
    let mut unsigned = token.clone();
    unsigned.authorization_sha256.clear();
    Ok(hex(&Sha256::digest(serde_json::to_vec(&unsigned)?)))
}

fn validate_raw_image(image: &[u8]) -> DynResult<u32> {
    for sector_size in [512, 4096] {
        if parse_gpt(image, sector_size).is_ok() {
            return Ok(sector_size);
        }
    }
    Err(invalid("flash input is not a valid canonical GPT image"))
}

fn write_and_sync(path: &Path, bytes: &[u8]) -> DynResult<()> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
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
}
