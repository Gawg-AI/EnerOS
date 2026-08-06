use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use ed25519_dalek::{Signature, VerifyingKey};
use eneros_boot_manager_uefi::{
    declared_manifest_len, verify_component, verify_manifest, TrustPolicy,
};
use eneros_boot_protocol::{
    BootControl, InstallManifest, ManifestSlot, BOOT_CONTROL_COPY_OFFSETS,
    BOOT_CONTROL_FLAG_MANUAL_RECOVERY, BOOT_CONTROL_RECORD_SIZE, INSTALL_MANIFEST_SIGNED_SIZE,
    INSTALL_MANIFEST_SIZE,
};
use eneros_image_format::{parse_gpt, Gpt, Guid, ParsedPartition};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::image_factory::wsl_path;
use super::{
    audit_layout, audit_profile, ensure_audit_pass, ensure_layout_audit_pass, invalid,
    load_profile, read_json, DynResult, ImageLayout, ProductProfile, ProfileClass,
};

const ISO_SECTOR_SIZE: usize = 2048;
const ESP_TYPE_GUID: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InspectionKind {
    Raw,
    Recovery,
    Installer,
}

impl InspectionKind {
    fn parse(value: &str) -> DynResult<Self> {
        match value {
            "raw" => Ok(Self::Raw),
            "recovery" => Ok(Self::Recovery),
            "installer" => Ok(Self::Installer),
            _ => Err(invalid("inspect kind must be raw, recovery, or installer")),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Recovery => "recovery",
            Self::Installer => "installer",
        }
    }
}

#[derive(Debug)]
struct InspectOptions {
    kind: InspectionKind,
    artifact: PathBuf,
    profile: String,
    release_public_key: PathBuf,
    uefi_cert: String,
    output: PathBuf,
}

#[derive(Debug, Serialize)]
struct ArtifactInspectionReport {
    schema: &'static str,
    result: &'static str,
    kind: &'static str,
    profile: String,
    artifact: String,
    size_bytes: u64,
    sha256: String,
    raw: Option<RawInspection>,
    installer: Option<InstallerInspection>,
}

#[derive(Debug, Serialize)]
struct RawInspection {
    layout: String,
    disk_guid: String,
    partition_count: u8,
    gpt: &'static str,
    bootctrl: &'static str,
    bootctrl_sequence: u64,
    bootctrl_security_floor: u64,
    bootctrl_copies_equal: bool,
    manual_recovery: bool,
    disabled_slots: Vec<String>,
    zeroed_partitions: Vec<String>,
    verified_manifest_slots: Vec<String>,
    release_id_hex: String,
    security_version: u64,
    component_count: usize,
    component_digests: &'static str,
    boot_application_sha256: String,
    boot_application_secure_boot_signature: &'static str,
}

#[derive(Debug, Serialize)]
struct InstallerInspection {
    iso9660: &'static str,
    el_torito: &'static str,
    volume_id: String,
    install_manifest: &'static str,
    install_manifest_signature: &'static str,
    payload_size_bytes: u64,
    payload_sha256: String,
    payload_matches_manifest: bool,
    installer_application_sha256: String,
    installer_secure_boot_signature: &'static str,
    embedded_raw: RawInspection,
}

#[derive(Debug, Clone, Copy)]
struct IsoEntry {
    offset: usize,
    length: usize,
    directory: bool,
}

#[derive(Debug, Clone, Copy)]
struct FatGeometry {
    bytes_per_sector: usize,
    sectors_per_cluster: usize,
    fat_start: usize,
    fat_length: usize,
    root_start: usize,
    root_length: usize,
    data_start_sector: usize,
    cluster_count: usize,
}

#[derive(Debug, Clone, Copy)]
struct FatEntry {
    cluster: u16,
    size: usize,
    directory: bool,
}

pub(super) fn command_inspect(root: &Path, args: &[String]) -> DynResult<()> {
    if let Some((mode, tail)) = args.split_first() {
        if mode == "update" {
            return crate::update_package::command_inspect_update(root, tail);
        }
    }
    let options = parse_options(args)?;
    inspect(root, options)
}

fn parse_options(args: &[String]) -> DynResult<InspectOptions> {
    let kind = args
        .first()
        .ok_or_else(|| invalid("inspect requires raw, recovery, or installer"))
        .and_then(|value| InspectionKind::parse(value))?;
    let mut artifact = None;
    let mut profile = None;
    let mut release_public_key = None;
    let mut uefi_cert = None;
    let mut output = None;
    let mut index = 1usize;
    while index < args.len() {
        let flag = &args[index];
        let value = args
            .get(index + 1)
            .ok_or_else(|| invalid(format!("{flag} requires a value")))?;
        match flag.as_str() {
            "--artifact" => artifact = Some(PathBuf::from(value)),
            "--profile" => profile = Some(value.clone()),
            "--release-public-key" => release_public_key = Some(PathBuf::from(value)),
            "--uefi-cert" => uefi_cert = Some(value.clone()),
            "--output" => output = Some(PathBuf::from(value)),
            _ => return Err(invalid(format!("unknown inspect option '{flag}'"))),
        }
        index += 2;
    }
    Ok(InspectOptions {
        kind,
        artifact: artifact.ok_or_else(|| invalid("--artifact is mandatory"))?,
        profile: profile.ok_or_else(|| invalid("--profile is mandatory"))?,
        release_public_key: release_public_key
            .ok_or_else(|| invalid("--release-public-key is mandatory"))?,
        uefi_cert: uefi_cert.ok_or_else(|| invalid("--uefi-cert is mandatory"))?,
        output: output.ok_or_else(|| invalid("--output is mandatory"))?,
    })
}

fn inspect(root: &Path, options: InspectOptions) -> DynResult<()> {
    let artifact = existing_target_file(root, &options.artifact, "artifact")?;
    let public_key_path =
        existing_target_file(root, &options.release_public_key, "release public key")?;
    let output = target_output(root, &options.output)?;
    if output.exists() {
        fs::remove_file(&output)?;
    }
    let profile = load_profile(root, &options.profile)?;
    let audit = audit_profile(root, &profile.id, None)?;
    ensure_audit_pass(&audit)?;
    validate_kind_profile(options.kind, &profile)?;
    let key_bytes = fs::read(public_key_path)?;
    crate::trust::enforce_release_public_key(root, &key_bytes)?;
    crate::trust::enforce_uefi_certificate(root, &options.uefi_cert)?;
    let public_key: [u8; 32] = key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| invalid("release public key must contain exactly 32 raw bytes"))?;
    VerifyingKey::from_bytes(&public_key)?;
    let artifact_bytes = fs::read(&artifact)?;
    let artifact_sha256 = hex(&Sha256::digest(&artifact_bytes));
    let extracted_prefix = output.with_extension("");

    let (raw, installer) = match options.kind {
        InspectionKind::Raw | InspectionKind::Recovery => (
            Some(inspect_raw_bytes(
                root,
                &artifact_bytes,
                &profile,
                public_key,
                &options.uefi_cert,
                options.kind,
                &extracted_prefix,
            )?),
            None,
        ),
        InspectionKind::Installer => {
            let result = inspect_installer(
                root,
                &artifact_bytes,
                &profile,
                public_key,
                &options.uefi_cert,
                &extracted_prefix,
            )?;
            (None, Some(result))
        }
    };
    let report = ArtifactInspectionReport {
        schema: "eneros.artifact-inspection.v1",
        result: "PASS",
        kind: options.kind.name(),
        profile: profile.id,
        artifact: artifact.to_string_lossy().into_owned(),
        size_bytes: u64::try_from(artifact_bytes.len())?,
        sha256: artifact_sha256.clone(),
        raw,
        installer,
    };
    let candidate = output.with_extension("json.candidate");
    fs::write(&candidate, serde_json::to_vec_pretty(&report)?)?;
    fs::rename(candidate, &output)?;
    println!(
        "INSPECT kind={} result=PASS artifact={} sha256={artifact_sha256} report={}",
        options.kind.name(),
        artifact.display(),
        output.display()
    );
    Ok(())
}

fn validate_kind_profile(kind: InspectionKind, profile: &ProductProfile) -> DynResult<()> {
    match (kind, profile.class) {
        (InspectionKind::Raw | InspectionKind::Installer, ProfileClass::Release)
        | (InspectionKind::Recovery, ProfileClass::Recovery) => Ok(()),
        _ => Err(invalid(format!(
            "inspect kind {} is incompatible with profile class {}",
            kind.name(),
            profile.class.policy_name()
        ))),
    }
}

fn inspect_raw_bytes(
    root: &Path,
    image: &[u8],
    profile: &ProductProfile,
    public_key: [u8; 32],
    uefi_cert: &str,
    kind: InspectionKind,
    extracted_prefix: &Path,
) -> DynResult<RawInspection> {
    let layout_id = profile
        .image_layout
        .as_deref()
        .ok_or_else(|| invalid("inspection profile has no image_layout"))?;
    let layout_audit = audit_layout(root, layout_id)?;
    ensure_layout_audit_pass(&layout_audit)?;
    let layout: ImageLayout = read_json(
        &root
            .join("product/image-layouts")
            .join(format!("{layout_id}.json")),
    )?;
    if u64::try_from(image.len())? != layout.disk_size_bytes {
        return Err(invalid(
            "raw artifact length differs from the audited layout",
        ));
    }
    let gpt = parse_gpt(image, layout.sector_size)
        .map_err(|error| invalid(format!("artifact GPT verification: {error:?}")))?;
    verify_layout(&gpt, &layout)?;
    let platform_id = first_16(Sha256::digest(profile.platform.as_bytes()));
    let key_id = first_16(Sha256::digest(public_key));

    let esp = gpt
        .find_by_name("PLATFORM_BOOT")
        .ok_or_else(|| invalid("artifact has no PLATFORM_BOOT partition"))?;
    if esp.type_guid
        != Guid::parse(ESP_TYPE_GUID).map_err(|error| invalid(format!("ESP GUID: {error:?}")))?
    {
        return Err(invalid("PLATFORM_BOOT is not an EFI System Partition"));
    }
    let (esp_start, esp_end) = partition_range(esp, gpt.sector_size)?;
    let boot_app = fat16_extract_boot_application(&image[esp_start..esp_end], esp.first_lba)?;
    let boot_app_path = suffix_path(extracted_prefix, "boot-manager.efi")?;
    verify_uefi_signature(root, &boot_app, uefi_cert, &boot_app_path)?;
    let boot_application_sha256 = hex(&Sha256::digest(&boot_app));

    let bootctrl = gpt
        .find_by_name("BOOTCTRL")
        .ok_or_else(|| invalid("artifact has no BOOTCTRL partition"))?;
    let (bootctrl_start, _) = partition_range(bootctrl, gpt.sector_size)?;
    let read_copy = |offset: u64| -> DynResult<[u8; BOOT_CONTROL_RECORD_SIZE]> {
        let start = bootctrl_start
            .checked_add(usize::try_from(offset)?)
            .ok_or_else(|| invalid("BOOTCTRL copy offset overflow"))?;
        image
            .get(start..start + BOOT_CONTROL_RECORD_SIZE)
            .ok_or_else(|| invalid("BOOTCTRL copy is truncated"))?
            .try_into()
            .map_err(|_| invalid("BOOTCTRL copy length mismatch"))
    };
    let first = read_copy(BOOT_CONTROL_COPY_OFFSETS[0])?;
    let second = read_copy(BOOT_CONTROL_COPY_OFFSETS[1])?;
    if first != second {
        return Err(invalid("canonical artifact BOOTCTRL copies differ"));
    }
    let control = BootControl::decode(&first)
        .map_err(|error| invalid(format!("BOOTCTRL verification: {error:?}")))?;
    if control.sequence != 0
        || control.security_floor < u64::from(profile.security.minimum_security_version)
    {
        return Err(invalid(
            "canonical artifact BOOTCTRL generation or security floor is invalid",
        ));
    }

    let (slot_specs, disabled_slots, zeroed_partitions, manual_recovery) = match kind {
        InspectionKind::Raw | InspectionKind::Installer => {
            if control.flags != 0
                || control.slot_a.priority == 0
                || control.slot_a.tries_remaining == 0
                || control.slot_a.successful
                || control.slot_b.priority == 0
                || !control.slot_b.successful
            {
                return Err(invalid("raw artifact BOOTCTRL A/B policy is not canonical"));
            }
            (
                vec![
                    (
                        "BOOT_A",
                        ManifestSlot::A,
                        control.slot_a.security_version,
                        control.slot_a.image_id,
                    ),
                    (
                        "BOOT_B",
                        ManifestSlot::B,
                        control.slot_b.security_version,
                        control.slot_b.image_id,
                    ),
                    (
                        "RECOVERY",
                        ManifestSlot::Recovery,
                        control.recovery_security_version,
                        control.recovery_image_id,
                    ),
                ],
                Vec::new(),
                Vec::new(),
                false,
            )
        }
        InspectionKind::Recovery => {
            if control.flags != BOOT_CONTROL_FLAG_MANUAL_RECOVERY
                || control.slot_a.priority != 0
                || control.slot_b.priority != 0
                || control.slot_a.successful
                || control.slot_b.successful
                || control.slot_a.tries_remaining != 0
                || control.slot_b.tries_remaining != 0
            {
                return Err(invalid(
                    "recovery artifact does not force manual recovery with disabled A/B slots",
                ));
            }
            let zeroed = vec![
                "BOOT_A".to_owned(),
                "BOOT_B".to_owned(),
                "SYS_A".to_owned(),
                "SYS_B".to_owned(),
                "CONFIG".to_owned(),
                "DATA".to_owned(),
                "LOGS".to_owned(),
                "MODELS".to_owned(),
            ];
            for role in &zeroed {
                let partition = gpt
                    .find_by_name(role)
                    .ok_or_else(|| invalid(format!("recovery artifact has no {role}")))?;
                let (start, end) = partition_range(partition, gpt.sector_size)?;
                if image[start..end].iter().any(|byte| *byte != 0) {
                    return Err(invalid(format!(
                        "recovery artifact partition {role} is not empty"
                    )));
                }
            }
            (
                vec![(
                    "RECOVERY",
                    ManifestSlot::Recovery,
                    control.recovery_security_version,
                    control.recovery_image_id,
                )],
                vec!["A".to_owned(), "B".to_owned()],
                zeroed,
                true,
            )
        }
    };

    let mut verified_manifest_slots = Vec::new();
    let mut release_id = None;
    let mut security_version = None;
    let mut component_count = 0usize;
    for (partition_name, slot, expected_security_version, expected_release_id) in slot_specs {
        let partition = gpt
            .find_by_name(partition_name)
            .ok_or_else(|| invalid(format!("artifact has no {partition_name}")))?;
        let (partition_start, partition_end) = partition_range(partition, gpt.sector_size)?;
        let header = image
            .get(partition_start..partition_start + eneros_boot_protocol::MANIFEST_HEADER_LEN)
            .ok_or_else(|| invalid(format!("{partition_name} manifest header is truncated")))?;
        let manifest_len = declared_manifest_len(header)
            .map_err(|error| invalid(format!("{partition_name} manifest length: {error:?}")))?;
        let encoded = image
            .get(partition_start..partition_start + manifest_len)
            .ok_or_else(|| invalid(format!("{partition_name} manifest is truncated")))?;
        let manifest = verify_manifest(
            encoded,
            &TrustPolicy {
                platform_id,
                key_id,
                public_key,
                minimum_security_version: u64::from(profile.security.minimum_security_version),
                expected_security_version,
                expected_release_id,
                selected_slot: slot,
                partition_length: u64::try_from(partition_end - partition_start)?,
            },
        )
        .map_err(|error| invalid(format!("{partition_name} trust policy: {error:?}")))?;
        if release_id.is_some_and(|id| id != manifest.header.release_id)
            || security_version.is_some_and(|version| version != manifest.header.security_version)
        {
            return Err(invalid(
                "artifact manifests disagree on release identity or security version",
            ));
        }
        release_id = Some(manifest.header.release_id);
        security_version = Some(manifest.header.security_version);
        for component in &manifest.components {
            let start = partition_start
                .checked_add(usize::try_from(component.media_offset)?)
                .ok_or_else(|| invalid("component media offset overflow"))?;
            let end = start
                .checked_add(usize::try_from(component.media_length)?)
                .ok_or_else(|| invalid("component media end overflow"))?;
            let payload = image
                .get(start..end)
                .filter(|_| end <= partition_end)
                .ok_or_else(|| invalid("component payload is outside its GPT partition"))?;
            verify_component(component, payload).map_err(|error| {
                invalid(format!(
                    "{partition_name} component {:?}: {error:?}",
                    component.role
                ))
            })?;
        }
        component_count += manifest.components.len();
        verified_manifest_slots.push(match slot {
            ManifestSlot::A => "A".to_owned(),
            ManifestSlot::B => "B".to_owned(),
            ManifestSlot::Recovery => "RECOVERY".to_owned(),
            ManifestSlot::None => return Err(invalid("non-bootable manifest slot")),
        });
    }
    let release_id = release_id.ok_or_else(|| invalid("artifact has no verified manifest"))?;
    if release_id != control.recovery_image_id
        || (kind != InspectionKind::Recovery
            && (release_id != control.slot_a.image_id || release_id != control.slot_b.image_id))
    {
        return Err(invalid(
            "BOOTCTRL image identities do not match the verified manifests",
        ));
    }
    Ok(RawInspection {
        layout: layout.id,
        disk_guid: guid_hex(gpt.disk_guid),
        partition_count: gpt.partition_count,
        gpt: "PASS",
        bootctrl: "PASS",
        bootctrl_sequence: control.sequence,
        bootctrl_security_floor: control.security_floor,
        bootctrl_copies_equal: true,
        manual_recovery,
        disabled_slots,
        zeroed_partitions,
        verified_manifest_slots,
        release_id_hex: hex(&release_id),
        security_version: security_version.unwrap_or(0),
        component_count,
        component_digests: "PASS",
        boot_application_sha256,
        boot_application_secure_boot_signature: "PASS",
    })
}

fn inspect_installer(
    root: &Path,
    iso: &[u8],
    profile: &ProductProfile,
    public_key: [u8; 32],
    uefi_cert: &str,
    extracted_prefix: &Path,
) -> DynResult<InstallerInspection> {
    if iso.len() % ISO_SECTOR_SIZE != 0 {
        return Err(invalid("installer ISO length is not sector aligned"));
    }
    let (root_entry, volume_id) = iso_primary_volume(iso)?;
    if volume_id != "ENEROS_INSTALL" {
        return Err(invalid("installer ISO volume id is not ENEROS_INSTALL"));
    }
    let efi = iso_find(iso, root_entry, b"EFI", true)?;
    let efi_boot = iso_find(iso, efi, b"BOOT", true)?;
    let boot_image_entry = iso_find(iso, efi_boot, b"EFIBOOT.IMG", false)?;
    verify_el_torito(iso, boot_image_entry)?;
    let boot_image = iso_slice(iso, boot_image_entry)?;
    let installer_app = fat16_extract_boot_application(boot_image, 0)?;
    let installer_path = suffix_path(extracted_prefix, "installer.efi")?;
    verify_uefi_signature(root, &installer_app, uefi_cert, &installer_path)?;

    let eneros = iso_find(iso, root_entry, b"ENEROS", true)?;
    let manifest_entry = iso_find(iso, eneros, b"INSTALL.MAN", false)?;
    let payload_entry = iso_find(iso, eneros, b"PAYLOAD.RAW", false)?;
    if manifest_entry.length != INSTALL_MANIFEST_SIZE {
        return Err(invalid("installer ISO manifest length is not canonical"));
    }
    let encoded_manifest = iso_slice(iso, manifest_entry)?;
    let install_manifest = InstallManifest::decode(encoded_manifest)
        .map_err(|error| invalid(format!("install manifest decode: {error:?}")))?;
    let platform_id = first_16(Sha256::digest(profile.platform.as_bytes()));
    let key_id = first_16(Sha256::digest(public_key));
    if install_manifest.platform_id != platform_id
        || install_manifest.key_id != key_id
        || install_manifest.security_version < u64::from(profile.security.minimum_security_version)
    {
        return Err(invalid(
            "install manifest platform, key, or security floor mismatch",
        ));
    }
    VerifyingKey::from_bytes(&public_key)?
        .verify_strict(
            &encoded_manifest[..INSTALL_MANIFEST_SIGNED_SIZE],
            &Signature::from_bytes(&install_manifest.signature),
        )
        .map_err(|_| invalid("install manifest signature is invalid"))?;
    let payload = iso_slice(iso, payload_entry)?;
    let payload_sha256 = hex(&Sha256::digest(payload));
    if u64::try_from(payload.len())? != install_manifest.payload_length
        || Sha256::digest(payload).as_slice() != install_manifest.payload_digest
    {
        return Err(invalid(
            "installer payload length or digest differs from the signed install manifest",
        ));
    }
    let embedded_prefix = suffix_path(extracted_prefix, "payload")?;
    let embedded_raw = inspect_raw_bytes(
        root,
        payload,
        profile,
        public_key,
        uefi_cert,
        InspectionKind::Raw,
        &embedded_prefix,
    )?;
    let payload_gpt = parse_gpt(payload, install_manifest.sector_size)
        .map_err(|error| invalid(format!("installer payload GPT: {error:?}")))?;
    if payload_gpt.disk_guid.0 != install_manifest.disk_guid
        || embedded_raw.release_id_hex != hex(&install_manifest.release_id)
        || embedded_raw.security_version != install_manifest.security_version
    {
        return Err(invalid(
            "install manifest identity is not bound to the embedded raw artifact",
        ));
    }
    Ok(InstallerInspection {
        iso9660: "PASS",
        el_torito: "PASS",
        volume_id,
        install_manifest: "PASS",
        install_manifest_signature: "PASS",
        payload_size_bytes: u64::try_from(payload.len())?,
        payload_sha256,
        payload_matches_manifest: true,
        installer_application_sha256: hex(&Sha256::digest(&installer_app)),
        installer_secure_boot_signature: "PASS",
        embedded_raw,
    })
}

fn verify_layout(gpt: &Gpt, layout: &ImageLayout) -> DynResult<()> {
    let expected_guid =
        Guid::parse(&layout.disk_guid).map_err(|error| invalid(format!("disk GUID: {error:?}")))?;
    if gpt.disk_guid != expected_guid
        || gpt.sector_size != layout.sector_size
        || gpt.total_lbas != layout.disk_size_bytes / u64::from(layout.sector_size)
        || usize::from(gpt.partition_count) != layout.partitions.len()
    {
        return Err(invalid("GPT geometry differs from the audited layout"));
    }
    let mut seen_guids = BTreeSet::new();
    for expected in &layout.partitions {
        let partition = gpt
            .find_by_name(&expected.role)
            .ok_or_else(|| invalid(format!("GPT has no {} partition", expected.role)))?;
        let expected_type = Guid::parse(&expected.type_guid)
            .map_err(|error| invalid(format!("partition type GUID: {error:?}")))?;
        let size = partition
            .last_lba
            .checked_sub(partition.first_lba)
            .and_then(|sectors| sectors.checked_add(1))
            .and_then(|sectors| sectors.checked_mul(u64::from(gpt.sector_size)))
            .ok_or_else(|| invalid("partition size overflow"))?;
        if partition.type_guid != expected_type
            || partition.attributes != expected.attributes
            || size != expected.size_bytes
            || !seen_guids.insert(partition.unique_guid.0)
        {
            return Err(invalid(format!(
                "GPT partition {} differs from the audited layout",
                expected.role
            )));
        }
    }
    Ok(())
}

fn partition_range(partition: &ParsedPartition, sector_size: u32) -> DynResult<(usize, usize)> {
    let sector = u64::from(sector_size);
    let start = usize::try_from(
        partition
            .first_lba
            .checked_mul(sector)
            .ok_or_else(|| invalid("partition offset overflow"))?,
    )?;
    let end = usize::try_from(
        partition
            .last_lba
            .checked_add(1)
            .and_then(|lba| lba.checked_mul(sector))
            .ok_or_else(|| invalid("partition end overflow"))?,
    )?;
    Ok((start, end))
}

fn fat16_extract_boot_application(volume: &[u8], expected_hidden: u64) -> DynResult<Vec<u8>> {
    let geometry = parse_fat16(volume, expected_hidden)?;
    let root = &volume[geometry.root_start..geometry.root_start + geometry.root_length];
    let efi = find_fat_entry(root, b"EFI        ", true)?;
    let efi_dir = read_fat_chain(volume, geometry, efi.cluster, None)?;
    let boot = find_fat_entry(&efi_dir, b"BOOT       ", true)?;
    let boot_dir = read_fat_chain(volume, geometry, boot.cluster, None)?;
    let application = find_fat_entry(&boot_dir, b"BOOTAA64EFI", false)?;
    if application.size == 0 {
        return Err(invalid("ESP BOOTAA64.EFI is empty"));
    }
    read_fat_chain(
        volume,
        geometry,
        application.cluster,
        Some(application.size),
    )
}

fn parse_fat16(volume: &[u8], expected_hidden: u64) -> DynResult<FatGeometry> {
    if volume.len() < 4 * 1024 * 1024 || volume.len() % 512 != 0 {
        return Err(invalid("ESP is not a sector-aligned FAT16-sized volume"));
    }
    let boot = &volume[..512];
    let bytes_per_sector = usize::from(le16(boot, 11)?);
    let sectors_per_cluster = usize::from(boot[13]);
    let reserved = usize::from(le16(boot, 14)?);
    let fats = usize::from(boot[16]);
    let root_entries = usize::from(le16(boot, 17)?);
    let total16 = usize::from(le16(boot, 19)?);
    let total32 = usize::try_from(le32(boot, 32)?)?;
    let total_sectors = if total16 != 0 { total16 } else { total32 };
    let sectors_per_fat = usize::from(le16(boot, 22)?);
    let hidden = u64::from(le32(boot, 28)?);
    if bytes_per_sector != 512
        || sectors_per_cluster == 0
        || !sectors_per_cluster.is_power_of_two()
        || reserved == 0
        || fats != 2
        || root_entries == 0
        || sectors_per_fat == 0
        || hidden != expected_hidden
        || total_sectors.checked_mul(bytes_per_sector) != Some(volume.len())
        || boot[54..62] != *b"FAT16   "
        || boot[510..512] != [0x55, 0xaa]
    {
        return Err(invalid("ESP FAT16 boot sector or geometry is invalid"));
    }
    let root_sectors = (root_entries * 32).div_ceil(bytes_per_sector);
    let fat_start = reserved
        .checked_mul(bytes_per_sector)
        .ok_or_else(|| invalid("FAT offset overflow"))?;
    let fat_length = sectors_per_fat
        .checked_mul(bytes_per_sector)
        .ok_or_else(|| invalid("FAT length overflow"))?;
    let root_start_sector = reserved
        .checked_add(
            fats.checked_mul(sectors_per_fat)
                .ok_or_else(|| invalid("FAT copies overflow"))?,
        )
        .ok_or_else(|| invalid("FAT root offset overflow"))?;
    let data_start_sector = root_start_sector
        .checked_add(root_sectors)
        .ok_or_else(|| invalid("FAT data offset overflow"))?;
    let cluster_count = total_sectors
        .checked_sub(data_start_sector)
        .ok_or_else(|| invalid("FAT data geometry underflow"))?
        / sectors_per_cluster;
    if !(4085..65525).contains(&cluster_count) {
        return Err(invalid("ESP cluster count is not FAT16"));
    }
    let second_fat = fat_start
        .checked_add(fat_length)
        .ok_or_else(|| invalid("second FAT offset overflow"))?;
    if volume.get(fat_start..fat_start + fat_length)
        != volume.get(second_fat..second_fat + fat_length)
    {
        return Err(invalid("ESP FAT16 copies differ"));
    }
    let root_start = root_start_sector
        .checked_mul(bytes_per_sector)
        .ok_or_else(|| invalid("FAT root byte offset overflow"))?;
    let root_length = root_sectors
        .checked_mul(bytes_per_sector)
        .ok_or_else(|| invalid("FAT root byte length overflow"))?;
    if root_start
        .checked_add(root_length)
        .map_or(true, |end| end > volume.len())
    {
        return Err(invalid("ESP FAT16 root directory is outside the volume"));
    }
    Ok(FatGeometry {
        bytes_per_sector,
        sectors_per_cluster,
        fat_start,
        fat_length,
        root_start,
        root_length,
        data_start_sector,
        cluster_count,
    })
}

fn find_fat_entry(
    directory: &[u8],
    expected: &[u8; 11],
    is_directory: bool,
) -> DynResult<FatEntry> {
    let mut found = None;
    for entry in directory.chunks_exact(32) {
        if entry[0] == 0 {
            break;
        }
        if entry[0] == 0xe5 || entry[11] == 0x0f || entry[..11] != expected[..] {
            continue;
        }
        let directory_flag = entry[11] & 0x10 != 0;
        if directory_flag != is_directory || le16(entry, 20)? != 0 {
            return Err(invalid("ESP FAT16 directory entry type is invalid"));
        }
        if found.is_some() {
            return Err(invalid("ESP FAT16 contains a duplicate boot path entry"));
        }
        found = Some(FatEntry {
            cluster: le16(entry, 26)?,
            size: usize::try_from(le32(entry, 28)?)?,
            directory: directory_flag,
        });
    }
    let found = found.ok_or_else(|| {
        invalid(format!(
            "ESP FAT16 entry '{}' is absent",
            String::from_utf8_lossy(expected)
        ))
    })?;
    if found.directory != is_directory || found.cluster < 2 {
        return Err(invalid("ESP FAT16 boot path cluster is invalid"));
    }
    Ok(found)
}

fn read_fat_chain(
    volume: &[u8],
    geometry: FatGeometry,
    first_cluster: u16,
    expected_size: Option<usize>,
) -> DynResult<Vec<u8>> {
    let cluster_bytes = geometry
        .sectors_per_cluster
        .checked_mul(geometry.bytes_per_sector)
        .ok_or_else(|| invalid("FAT cluster size overflow"))?;
    let mut seen = BTreeSet::new();
    let mut output = Vec::new();
    let mut cluster = usize::from(first_cluster);
    loop {
        if cluster < 2
            || cluster >= geometry.cluster_count + 2
            || !seen.insert(cluster)
            || seen.len() > geometry.cluster_count
        {
            return Err(invalid("ESP FAT16 cluster chain is invalid or cyclic"));
        }
        let sector = geometry
            .data_start_sector
            .checked_add(
                (cluster - 2)
                    .checked_mul(geometry.sectors_per_cluster)
                    .ok_or_else(|| invalid("FAT cluster sector overflow"))?,
            )
            .ok_or_else(|| invalid("FAT data sector overflow"))?;
        let start = sector
            .checked_mul(geometry.bytes_per_sector)
            .ok_or_else(|| invalid("FAT cluster offset overflow"))?;
        output.extend_from_slice(
            volume
                .get(start..start + cluster_bytes)
                .ok_or_else(|| invalid("FAT cluster is outside the volume"))?,
        );
        let fat_offset = geometry
            .fat_start
            .checked_add(
                cluster
                    .checked_mul(2)
                    .ok_or_else(|| invalid("FAT entry offset overflow"))?,
            )
            .ok_or_else(|| invalid("FAT entry absolute offset overflow"))?;
        if fat_offset + 2 > geometry.fat_start + geometry.fat_length {
            return Err(invalid("FAT chain entry is outside the FAT"));
        }
        let next = usize::from(le16(volume, fat_offset)?);
        if next >= 0xfff8 {
            break;
        }
        if !(2..0xfff0).contains(&next) || next == 0xfff7 {
            return Err(invalid(
                "FAT chain contains a free, reserved, or bad cluster",
            ));
        }
        cluster = next;
    }
    if let Some(size) = expected_size {
        let required_clusters = size.div_ceil(cluster_bytes);
        if seen.len() != required_clusters || size > output.len() {
            return Err(invalid("FAT file size does not match its cluster chain"));
        }
        output.truncate(size);
    }
    Ok(output)
}

fn iso_primary_volume(iso: &[u8]) -> DynResult<(IsoEntry, String)> {
    let pvd = iso
        .get(16 * ISO_SECTOR_SIZE..17 * ISO_SECTOR_SIZE)
        .ok_or_else(|| invalid("installer ISO has no primary volume descriptor"))?;
    if pvd[0] != 1 || pvd[1..6] != *b"CD001" || pvd[6] != 1 {
        return Err(invalid(
            "installer ISO primary volume descriptor is invalid",
        ));
    }
    let sectors_le = usize::try_from(le32(pvd, 80)?)?;
    let sectors_be = usize::try_from(u32::from_be_bytes(pvd[84..88].try_into()?))?;
    if sectors_le != sectors_be || sectors_le.checked_mul(ISO_SECTOR_SIZE) != Some(iso.len()) {
        return Err(invalid("installer ISO volume space size is inconsistent"));
    }
    let volume_id = String::from_utf8(pvd[40..72].to_vec())?
        .trim_end()
        .to_owned();
    Ok((parse_iso_record(iso, &pvd[156..])?, volume_id))
}

fn iso_find(iso: &[u8], directory: IsoEntry, expected: &[u8], is_dir: bool) -> DynResult<IsoEntry> {
    if !directory.directory || directory.length == 0 || directory.length > 128 * 1024 {
        return Err(invalid("installer ISO directory bounds are invalid"));
    }
    let bytes = iso_slice(iso, directory)?;
    let mut offset = 0usize;
    let mut found = None;
    while offset < bytes.len() {
        let record_length = usize::from(bytes[offset]);
        if record_length == 0 {
            offset = offset
                .checked_add(ISO_SECTOR_SIZE)
                .map(|next| next / ISO_SECTOR_SIZE * ISO_SECTOR_SIZE)
                .ok_or_else(|| invalid("ISO directory offset overflow"))?;
            continue;
        }
        let record = bytes
            .get(offset..offset + record_length)
            .ok_or_else(|| invalid("ISO directory record is truncated"))?;
        let name_length = usize::from(
            *record
                .get(32)
                .ok_or_else(|| invalid("ISO directory name length is absent"))?,
        );
        let mut name = record
            .get(33..33 + name_length)
            .ok_or_else(|| invalid("ISO directory name is truncated"))?;
        if let Some(version) = name.iter().position(|byte| *byte == b';') {
            name = &name[..version];
        }
        let entry = parse_iso_record(iso, record)?;
        if name == expected && entry.directory == is_dir {
            if found.is_some() {
                return Err(invalid("installer ISO contains a duplicate required entry"));
            }
            found = Some(entry);
        }
        offset = offset
            .checked_add(record_length)
            .ok_or_else(|| invalid("ISO directory record offset overflow"))?;
    }
    found.ok_or_else(|| {
        invalid(format!(
            "installer ISO entry '{}' is absent",
            String::from_utf8_lossy(expected)
        ))
    })
}

fn parse_iso_record(iso: &[u8], record: &[u8]) -> DynResult<IsoEntry> {
    let record_length = usize::from(
        *record
            .first()
            .ok_or_else(|| invalid("ISO directory record is absent"))?,
    );
    if record_length < 34 || record.len() < record_length {
        return Err(invalid("ISO directory record length is invalid"));
    }
    let extent_le = usize::try_from(le32(record, 2)?)?;
    let extent_be = usize::try_from(u32::from_be_bytes(record[6..10].try_into()?))?;
    let length_le = usize::try_from(le32(record, 10)?)?;
    let length_be = usize::try_from(u32::from_be_bytes(record[14..18].try_into()?))?;
    if extent_le != extent_be || length_le != length_be || length_le == 0 {
        return Err(invalid("ISO directory record endian fields disagree"));
    }
    let offset = extent_le
        .checked_mul(ISO_SECTOR_SIZE)
        .ok_or_else(|| invalid("ISO extent offset overflow"))?;
    if offset
        .checked_add(length_le)
        .map_or(true, |end| end > iso.len())
    {
        return Err(invalid("ISO extent is outside the artifact"));
    }
    Ok(IsoEntry {
        offset,
        length: length_le,
        directory: record[25] & 0x02 != 0,
    })
}

fn iso_slice(iso: &[u8], entry: IsoEntry) -> DynResult<&[u8]> {
    iso.get(entry.offset..entry.offset + entry.length)
        .ok_or_else(|| invalid("ISO entry is outside the artifact"))
}

fn verify_el_torito(iso: &[u8], boot_image: IsoEntry) -> DynResult<()> {
    let mut catalog_lba = None;
    let mut saw_terminator = false;
    for sector in 16usize..32 {
        let descriptor = iso
            .get(sector * ISO_SECTOR_SIZE..(sector + 1) * ISO_SECTOR_SIZE)
            .ok_or_else(|| invalid("ISO volume descriptor set is truncated"))?;
        if descriptor[1..6] != *b"CD001" || descriptor[6] != 1 {
            return Err(invalid("ISO volume descriptor set is invalid"));
        }
        if descriptor[0] == 0
            && descriptor[7..39].starts_with(b"EL TORITO SPECIFICATION")
            && catalog_lba
                .replace(usize::try_from(le32(descriptor, 71)?)?)
                .is_some()
        {
            return Err(invalid("ISO contains multiple El Torito boot catalogs"));
        }
        if descriptor[0] == 255 {
            saw_terminator = true;
            break;
        }
    }
    let catalog_lba = catalog_lba.ok_or_else(|| invalid("ISO has no El Torito descriptor"))?;
    if !saw_terminator {
        return Err(invalid("ISO volume descriptor terminator is absent"));
    }
    let catalog = iso
        .get(catalog_lba * ISO_SECTOR_SIZE..(catalog_lba + 1) * ISO_SECTOR_SIZE)
        .ok_or_else(|| invalid("El Torito boot catalog is outside the ISO"))?;
    let checksum = catalog[..32].chunks_exact(2).fold(0u16, |sum, word| {
        sum.wrapping_add(u16::from_le_bytes([word[0], word[1]]))
    });
    if catalog[0] != 1
        || catalog[1] != 0xef
        || catalog[30..32] != [0x55, 0xaa]
        || checksum != 0
        || catalog[32] != 0x88
        || catalog[33] != 0
    {
        return Err(invalid(
            "El Torito EFI validation or initial entry is invalid",
        ));
    }
    let sectors_512 = usize::from(le16(catalog, 38)?);
    let boot_lba = usize::try_from(le32(catalog, 40)?)?;
    if sectors_512
        .checked_mul(512)
        .filter(|length| *length == boot_image.length)
        .is_none()
        || boot_lba
            .checked_mul(ISO_SECTOR_SIZE)
            .filter(|offset| *offset == boot_image.offset)
            .is_none()
    {
        return Err(invalid(
            "El Torito catalog is not bound to EFI/BOOT/EFIBOOT.IMG",
        ));
    }
    Ok(())
}

fn verify_uefi_signature(
    root: &Path,
    application: &[u8],
    cert: &str,
    extracted: &Path,
) -> DynResult<()> {
    if application.len() < 1024 || application[..2] != *b"MZ" {
        return Err(invalid("extracted UEFI application is not a PE/COFF image"));
    }
    let parent = extracted
        .parent()
        .ok_or_else(|| invalid("extracted UEFI evidence has no parent"))?;
    fs::create_dir_all(parent)?;
    let parent = parent.canonicalize()?;
    let target = root.join("target").canonicalize()?;
    if !parent.starts_with(target) {
        return Err(invalid("extracted UEFI evidence must stay below target"));
    }
    let extracted = parent.join(
        extracted
            .file_name()
            .ok_or_else(|| invalid("extracted UEFI evidence has no file name"))?,
    );
    fs::write(&extracted, application)?;
    let mut command = if cfg!(windows) {
        let extracted_wsl = wsl_path(root, &extracted)?;
        let mut command = Command::new("wsl.exe");
        command.args([
            "-d",
            "Ubuntu-22.04",
            "--",
            "sbverify",
            "--cert",
            cert,
            &extracted_wsl,
        ]);
        command
    } else {
        let mut command = Command::new("sbverify");
        command.args(["--cert", cert]).arg(&extracted);
        command
    };
    let output = command.output()?;
    if !output.status.success() {
        return Err(invalid(format!(
            "UEFI Secure Boot signature verification failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

fn suffix_path(prefix: &Path, suffix: &str) -> DynResult<PathBuf> {
    let parent = prefix
        .parent()
        .ok_or_else(|| invalid("inspection output prefix has no parent"))?;
    let stem = prefix
        .file_name()
        .ok_or_else(|| invalid("inspection output prefix has no file name"))?
        .to_string_lossy();
    Ok(parent.join(format!("{stem}.{suffix}")))
}

fn existing_target_file(root: &Path, path: &Path, kind: &str) -> DynResult<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let candidate = candidate.canonicalize()?;
    let target = root.join("target").canonicalize()?;
    if !candidate.starts_with(target) || !candidate.is_file() {
        return Err(invalid(format!(
            "inspection {kind} must be an existing file below target"
        )));
    }
    Ok(candidate)
}

fn target_output(root: &Path, path: &Path) -> DynResult<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let parent = candidate
        .parent()
        .ok_or_else(|| invalid("inspection report has no parent"))?;
    fs::create_dir_all(parent)?;
    let parent = parent.canonicalize()?;
    let target = root.join("target").canonicalize()?;
    if !parent.starts_with(target) {
        return Err(invalid("inspection report must stay below target"));
    }
    Ok(parent.join(
        candidate
            .file_name()
            .ok_or_else(|| invalid("inspection report has no file name"))?,
    ))
}

fn le16(bytes: &[u8], offset: usize) -> DynResult<u16> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or_else(|| invalid("little-endian u16 is truncated"))?
            .try_into()?,
    ))
}

fn le32(bytes: &[u8], offset: usize) -> DynResult<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or_else(|| invalid("little-endian u32 is truncated"))?
            .try_into()?,
    ))
}

fn first_16(digest: impl AsRef<[u8]>) -> [u8; 16] {
    let mut output = [0u8; 16];
    output.copy_from_slice(&digest.as_ref()[..16]);
    output
}

fn guid_hex(guid: Guid) -> String {
    hex(&guid.0)
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

    #[test]
    fn inspection_kinds_are_closed() {
        assert_eq!(InspectionKind::parse("raw").unwrap(), InspectionKind::Raw);
        assert_eq!(
            InspectionKind::parse("recovery").unwrap(),
            InspectionKind::Recovery
        );
        assert_eq!(
            InspectionKind::parse("installer").unwrap(),
            InspectionKind::Installer
        );
        assert!(InspectionKind::parse("sidecar").is_err());
    }

    #[test]
    fn truncated_integer_reads_fail_closed() {
        assert!(le16(&[0], 0).is_err());
        assert!(le32(&[0, 1, 2], 0).is_err());
    }
}
