use std::fs;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use eneros_boot_protocol::manifest::{PERMISSION_EXECUTE, PERMISSION_READ, PERMISSION_WRITE};
use eneros_boot_protocol::{
    BootControl, ComponentRole, ManifestSlot, ReleaseManifest, BOOT_CONTROL_COPY_OFFSETS,
    BOOT_CONTROL_FLAG_MANUAL_RECOVERY, BOOT_CONTROL_RECORD_SIZE, MANIFEST_HEADER_LEN,
};
use eneros_image_format::parse_gpt;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{invalid, DynResult};

#[derive(Debug, Clone, Copy)]
enum ManifestMutation {
    SignatureBitFlip,
    SecurityVersionOne,
    WrongPlatform,
    WxKernel,
    KernelLoadOutside,
}

impl ManifestMutation {
    fn parse(value: &str) -> DynResult<Self> {
        match value {
            "signature-bitflip" => Ok(Self::SignatureBitFlip),
            "security-version-one" => Ok(Self::SecurityVersionOne),
            "wrong-platform" => Ok(Self::WrongPlatform),
            "wx-kernel" => Ok(Self::WxKernel),
            "kernel-load-outside" => Ok(Self::KernelLoadOutside),
            _ => Err(invalid(format!(
                "unknown manifest mutation '{value}'; expected signature-bitflip, security-version-one, wrong-platform, wx-kernel, or kernel-load-outside"
            ))),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::SignatureBitFlip => "signature-bitflip",
            Self::SecurityVersionOne => "security-version-one",
            Self::WrongPlatform => "wrong-platform",
            Self::WxKernel => "wx-kernel",
            Self::KernelLoadOutside => "kernel-load-outside",
        }
    }
}

#[derive(Debug)]
struct Options {
    mutation: ManifestMutation,
    image: PathBuf,
    output: PathBuf,
    signing_key: PathBuf,
    slot: ManifestSlot,
}

#[derive(Serialize)]
struct MutationRecord {
    schema: &'static str,
    mutation: &'static str,
    slot: &'static str,
    source_sha256: String,
    output_sha256: String,
    manifest_offset: u64,
    manifest_length: usize,
    signing_key_id: String,
}

#[derive(Serialize)]
struct BootControlMutationRecord {
    schema: &'static str,
    mutation: &'static str,
    source_sha256: String,
    output_sha256: String,
    sequence: u64,
    security_floor: u64,
    copies_equal: bool,
}

#[derive(Debug, Clone, Copy)]
enum InstallerMutation {
    PayloadBitFlip,
    ManifestSignatureBitFlip,
}

impl InstallerMutation {
    fn parse(value: &str) -> DynResult<Self> {
        match value {
            "payload-bitflip" => Ok(Self::PayloadBitFlip),
            "manifest-signature-bitflip" => Ok(Self::ManifestSignatureBitFlip),
            _ => Err(invalid(
                "fault installer mutation must be payload-bitflip or manifest-signature-bitflip",
            )),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::PayloadBitFlip => "payload-bitflip",
            Self::ManifestSignatureBitFlip => "manifest-signature-bitflip",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct IsoEntry {
    offset: usize,
    length: usize,
}

#[derive(Serialize)]
struct InstallerMutationRecord {
    schema: &'static str,
    mutation: &'static str,
    source_sha256: String,
    output_sha256: String,
    iso_file: &'static str,
    byte_offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
enum ArtifactMutation {
    GptBitFlip,
    EspApplicationBitFlip,
    ComponentBitFlip,
    BootControlBitFlip,
    RecoveryEmptyBitFlip,
}

impl ArtifactMutation {
    fn parse(value: &str) -> DynResult<Self> {
        match value {
            "gpt-bitflip" => Ok(Self::GptBitFlip),
            "esp-application-bitflip" => Ok(Self::EspApplicationBitFlip),
            "component-bitflip" => Ok(Self::ComponentBitFlip),
            "bootctrl-bitflip" => Ok(Self::BootControlBitFlip),
            "recovery-empty-bitflip" => Ok(Self::RecoveryEmptyBitFlip),
            _ => Err(invalid(
                "fault artifact mutation must be gpt-bitflip, esp-application-bitflip, component-bitflip, bootctrl-bitflip, or recovery-empty-bitflip",
            )),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::GptBitFlip => "gpt-bitflip",
            Self::EspApplicationBitFlip => "esp-application-bitflip",
            Self::ComponentBitFlip => "component-bitflip",
            Self::BootControlBitFlip => "bootctrl-bitflip",
            Self::RecoveryEmptyBitFlip => "recovery-empty-bitflip",
        }
    }
}

#[derive(Serialize)]
struct ArtifactMutationRecord {
    schema: &'static str,
    mutation: &'static str,
    source_sha256: String,
    output_sha256: String,
    byte_offset: usize,
}

pub(super) fn command_fault(root: &Path, args: &[String]) -> DynResult<()> {
    let Some((kind, tail)) = args.split_first() else {
        return Err(invalid("fault requires a kind"));
    };
    match kind.as_str() {
        "manifest" => mutate_manifest(root, parse_options(tail)?),
        "bootctrl" => mutate_boot_control(root, tail),
        "installer" => mutate_installer(root, tail),
        "artifact" => mutate_artifact(root, tail),
        _ => Err(invalid(
            "fault kind must be manifest, bootctrl, installer, or artifact",
        )),
    }
}

fn mutate_artifact(root: &Path, args: &[String]) -> DynResult<()> {
    let mutation = args
        .first()
        .ok_or_else(|| invalid("fault artifact requires a mutation"))
        .and_then(|value| ArtifactMutation::parse(value))?;
    let mut image_path = None;
    let mut output_path = None;
    let mut component_partition = "BOOT_A";
    let mut slot_supplied = false;
    let mut index = 1usize;
    while index < args.len() {
        let flag = &args[index];
        let value = args
            .get(index + 1)
            .ok_or_else(|| invalid(format!("{flag} requires a value")))?;
        match flag.as_str() {
            "--image" => image_path = Some(PathBuf::from(value)),
            "--output" => output_path = Some(PathBuf::from(value)),
            "--slot" => {
                component_partition = match value.as_str() {
                    "A" => "BOOT_A",
                    "B" => "BOOT_B",
                    "RECOVERY" => "RECOVERY",
                    _ => return Err(invalid("--slot must be A, B, or RECOVERY")),
                };
                slot_supplied = true;
            }
            _ => return Err(invalid(format!("unknown fault artifact option '{flag}'"))),
        }
        index += 2;
    }
    if slot_supplied && mutation != ArtifactMutation::ComponentBitFlip {
        return Err(invalid(
            "--slot is accepted only for the component-bitflip mutation",
        ));
    }
    let source = existing_target_file(
        root,
        &image_path.ok_or_else(|| invalid("--image is mandatory"))?,
    )?;
    let output = fault_output(
        root,
        &output_path.ok_or_else(|| invalid("--output is mandatory"))?,
    )?;
    if source == output {
        return Err(invalid("fault output must not overwrite its source image"));
    }
    let mut image = fs::read(&source)?;
    let source_sha256 = hex(&Sha256::digest(&image));
    let gpt = parse_gpt(&image, 512)
        .map_err(|error| invalid(format!("fault source GPT is invalid: {error:?}")))?;
    let partition_start = |name: &str| -> DynResult<usize> {
        let partition = gpt
            .find_by_name(name)
            .ok_or_else(|| invalid(format!("fault source has no {name}")))?;
        usize::try_from(
            partition
                .first_lba
                .checked_mul(u64::from(gpt.sector_size))
                .ok_or_else(|| invalid("fault partition offset overflow"))?,
        )
        .map_err(Into::into)
    };
    let byte_offset = match mutation {
        ArtifactMutation::GptBitFlip => 512usize
            .checked_add(56)
            .ok_or_else(|| invalid("GPT mutation offset overflow"))?,
        ArtifactMutation::BootControlBitFlip => partition_start("BOOTCTRL")?
            .checked_add(64)
            .ok_or_else(|| invalid("BOOTCTRL mutation offset overflow"))?,
        ArtifactMutation::RecoveryEmptyBitFlip => partition_start("DATA")?,
        ArtifactMutation::ComponentBitFlip => {
            let start = partition_start(component_partition)?;
            let header = image
                .get(start..start + MANIFEST_HEADER_LEN)
                .ok_or_else(|| {
                    invalid(format!(
                        "{component_partition} manifest header is truncated"
                    ))
                })?;
            let length = usize::try_from(u32::from_le_bytes(header[12..16].try_into()?))?;
            let manifest =
                ReleaseManifest::decode(image.get(start..start + length).ok_or_else(|| {
                    invalid(format!("{component_partition} manifest is truncated"))
                })?)
                .map_err(|error| {
                    invalid(format!("{component_partition} manifest decode: {error:?}"))
                })?;
            let kernel = manifest
                .components
                .iter()
                .find(|component| component.role == ComponentRole::Kernel)
                .ok_or_else(|| invalid(format!("{component_partition} has no kernel component")))?;
            if kernel.media_length < 14 {
                return Err(invalid(format!(
                    "{component_partition} kernel is too small for mutation"
                )));
            }
            start
                .checked_add(usize::try_from(kernel.media_offset)?)
                .and_then(|offset| offset.checked_add(13))
                .ok_or_else(|| invalid("component mutation offset overflow"))?
        }
        ArtifactMutation::EspApplicationBitFlip => {
            let start = partition_start("PLATFORM_BOOT")?;
            let partition = gpt
                .find_by_name("PLATFORM_BOOT")
                .ok_or_else(|| invalid("fault source has no PLATFORM_BOOT"))?;
            let end = usize::try_from(
                partition
                    .last_lba
                    .checked_add(1)
                    .and_then(|lba| lba.checked_mul(u64::from(gpt.sector_size)))
                    .ok_or_else(|| invalid("ESP end offset overflow"))?,
            )?;
            let esp = &image[start..end];
            let entry = esp
                .windows(11)
                .position(|window| window == b"BOOTAA64EFI")
                .ok_or_else(|| invalid("ESP has no BOOTAA64.EFI directory entry"))?;
            let cluster = usize::from(u16::from_le_bytes(
                esp.get(entry + 26..entry + 28)
                    .ok_or_else(|| invalid("ESP file cluster is truncated"))?
                    .try_into()?,
            ));
            let bytes_per_sector = usize::from(u16::from_le_bytes(esp[11..13].try_into()?));
            let sectors_per_cluster = usize::from(esp[13]);
            let reserved = usize::from(u16::from_le_bytes(esp[14..16].try_into()?));
            let fats = usize::from(esp[16]);
            let root_entries = usize::from(u16::from_le_bytes(esp[17..19].try_into()?));
            let sectors_per_fat = usize::from(u16::from_le_bytes(esp[22..24].try_into()?));
            let root_sectors = (root_entries * 32).div_ceil(bytes_per_sector);
            let data_start = reserved
                .checked_add(
                    fats.checked_mul(sectors_per_fat)
                        .ok_or_else(|| invalid("ESP FAT geometry overflow"))?,
                )
                .and_then(|sector| sector.checked_add(root_sectors))
                .ok_or_else(|| invalid("ESP data offset overflow"))?;
            if bytes_per_sector != 512 || sectors_per_cluster == 0 || cluster < 2 {
                return Err(invalid("ESP FAT geometry is invalid"));
            }
            start
                .checked_add(
                    data_start
                        .checked_add(
                            (cluster - 2)
                                .checked_mul(sectors_per_cluster)
                                .ok_or_else(|| invalid("ESP cluster offset overflow"))?,
                        )
                        .and_then(|sector| sector.checked_mul(bytes_per_sector))
                        .ok_or_else(|| invalid("ESP application offset overflow"))?,
                )
                .and_then(|offset| offset.checked_add(256))
                .ok_or_else(|| invalid("ESP application mutation offset overflow"))?
        }
    };
    let byte = image
        .get_mut(byte_offset)
        .ok_or_else(|| invalid("artifact mutation points outside the image"))?;
    *byte ^= 1;
    let output_sha256 = hex(&Sha256::digest(&image));
    let temporary = output.with_extension("raw.candidate");
    fs::write(&temporary, &image)?;
    fs::rename(temporary, &output)?;
    fs::write(
        output.with_extension("mutation.json"),
        serde_json::to_vec_pretty(&ArtifactMutationRecord {
            schema: "eneros.artifact-mutation.v1",
            mutation: mutation.name(),
            source_sha256,
            output_sha256: output_sha256.clone(),
            byte_offset,
        })?,
    )?;
    println!(
        "FAULT mutation={} result=PASS output={} sha256={output_sha256}",
        mutation.name(),
        output.display()
    );
    Ok(())
}

fn mutate_installer(root: &Path, args: &[String]) -> DynResult<()> {
    let mutation = args
        .first()
        .ok_or_else(|| invalid("fault installer requires a mutation"))
        .and_then(|value| InstallerMutation::parse(value))?;
    let mut iso_path = None;
    let mut output_path = None;
    let mut index = 1usize;
    while index < args.len() {
        let flag = &args[index];
        let value = args
            .get(index + 1)
            .ok_or_else(|| invalid(format!("{flag} requires a value")))?;
        match flag.as_str() {
            "--iso" => iso_path = Some(PathBuf::from(value)),
            "--output" => output_path = Some(PathBuf::from(value)),
            _ => return Err(invalid(format!("unknown fault installer option '{flag}'"))),
        }
        index += 2;
    }
    let source = existing_target_file(
        root,
        &iso_path.ok_or_else(|| invalid("--iso is mandatory"))?,
    )?;
    let output = fault_output(
        root,
        &output_path.ok_or_else(|| invalid("--output is mandatory"))?,
    )?;
    if source == output {
        return Err(invalid("fault output must not overwrite its source ISO"));
    }
    let mut iso = fs::read(&source)?;
    let source_sha256 = hex(&Sha256::digest(&iso));
    let eneros = find_iso_entry(&iso, iso_root(&iso)?, b"ENEROS", true)?;
    let (name, entry, relative_offset) = match mutation {
        InstallerMutation::PayloadBitFlip => {
            let entry = find_iso_entry(&iso, eneros, b"PAYLOAD.RAW", false)?;
            if entry.length < 4097 {
                return Err(invalid("installer payload is too small for the mutation"));
            }
            ("ENEROS/PAYLOAD.RAW", entry, 4096usize)
        }
        InstallerMutation::ManifestSignatureBitFlip => {
            let entry = find_iso_entry(&iso, eneros, b"INSTALL.MAN", false)?;
            if entry.length != eneros_boot_protocol::INSTALL_MANIFEST_SIZE {
                return Err(invalid("installer manifest length is not canonical"));
            }
            (
                "ENEROS/INSTALL.MAN",
                entry,
                eneros_boot_protocol::INSTALL_MANIFEST_SIGNED_SIZE,
            )
        }
    };
    let byte_offset = entry
        .offset
        .checked_add(relative_offset)
        .ok_or_else(|| invalid("installer mutation offset overflow"))?;
    let byte = iso
        .get_mut(byte_offset)
        .ok_or_else(|| invalid("installer mutation points outside the ISO"))?;
    *byte ^= 1;
    let output_sha256 = hex(&Sha256::digest(&iso));
    let temporary = output.with_extension("iso.candidate");
    fs::write(&temporary, &iso)?;
    fs::rename(&temporary, &output)?;
    fs::write(
        output.with_extension("mutation.json"),
        serde_json::to_vec_pretty(&InstallerMutationRecord {
            schema: "eneros.installer-mutation.v1",
            mutation: mutation.name(),
            source_sha256,
            output_sha256: output_sha256.clone(),
            iso_file: name,
            byte_offset,
        })?,
    )?;
    println!(
        "FAULT mutation={} result=PASS output={} sha256={output_sha256}",
        mutation.name(),
        output.display()
    );
    Ok(())
}

fn iso_root(iso: &[u8]) -> DynResult<IsoEntry> {
    const SECTOR: usize = 2048;
    let pvd = iso
        .get(16 * SECTOR..17 * SECTOR)
        .ok_or_else(|| invalid("installer ISO has no primary volume descriptor"))?;
    if pvd[0] != 1 || pvd[1..6] != *b"CD001" || pvd[6] != 1 {
        return Err(invalid(
            "installer ISO primary volume descriptor is invalid",
        ));
    }
    parse_iso_record(iso, &pvd[156..])
}

fn find_iso_entry(
    iso: &[u8],
    directory: IsoEntry,
    expected: &[u8],
    expected_directory: bool,
) -> DynResult<IsoEntry> {
    const SECTOR: usize = 2048;
    if directory.length == 0 || directory.length > 64 * 1024 {
        return Err(invalid("installer ISO directory length is invalid"));
    }
    let bytes = iso
        .get(directory.offset..directory.offset + directory.length)
        .ok_or_else(|| invalid("installer ISO directory is truncated"))?;
    let mut offset = 0usize;
    while offset < bytes.len() {
        let record_length = usize::from(bytes[offset]);
        if record_length == 0 {
            offset = offset
                .checked_add(SECTOR)
                .map(|next| next / SECTOR * SECTOR)
                .ok_or_else(|| invalid("installer ISO directory offset overflow"))?;
            continue;
        }
        let record = bytes
            .get(offset..offset + record_length)
            .ok_or_else(|| invalid("installer ISO directory record is truncated"))?;
        let name_length = usize::from(
            *record
                .get(32)
                .ok_or_else(|| invalid("installer ISO directory name is absent"))?,
        );
        let mut name = record
            .get(33..33 + name_length)
            .ok_or_else(|| invalid("installer ISO directory name is truncated"))?;
        if let Some(version) = name.iter().position(|byte| *byte == b';') {
            name = &name[..version];
        }
        let is_directory = record.get(25).is_some_and(|flags| flags & 0x02 != 0);
        if name == expected && is_directory == expected_directory {
            return parse_iso_record(iso, record);
        }
        offset = offset
            .checked_add(record_length)
            .ok_or_else(|| invalid("installer ISO directory record overflow"))?;
    }
    Err(invalid(format!(
        "installer ISO entry '{}' is absent",
        String::from_utf8_lossy(expected)
    )))
}

fn parse_iso_record(iso: &[u8], record: &[u8]) -> DynResult<IsoEntry> {
    const SECTOR: usize = 2048;
    let record_length = usize::from(
        *record
            .first()
            .ok_or_else(|| invalid("installer ISO directory record is absent"))?,
    );
    if record_length < 34 || record.len() < record_length {
        return Err(invalid("installer ISO directory record is invalid"));
    }
    let extent = usize::try_from(u32::from_le_bytes(record[2..6].try_into()?))?;
    let length = usize::try_from(u32::from_le_bytes(record[10..14].try_into()?))?;
    let offset = extent
        .checked_mul(SECTOR)
        .ok_or_else(|| invalid("installer ISO extent overflow"))?;
    let end = offset
        .checked_add(length)
        .ok_or_else(|| invalid("installer ISO file end overflow"))?;
    if length == 0 || end > iso.len() {
        return Err(invalid("installer ISO file extent is outside the artifact"));
    }
    Ok(IsoEntry { offset, length })
}

fn mutate_boot_control(root: &Path, args: &[String]) -> DynResult<()> {
    if args.first().map(String::as_str) != Some("manual-recovery") {
        return Err(invalid(
            "fault bootctrl requires the manual-recovery mutation",
        ));
    }
    let mut image_path = None;
    let mut output_path = None;
    let mut index = 1usize;
    while index < args.len() {
        let flag = &args[index];
        let value = args
            .get(index + 1)
            .ok_or_else(|| invalid(format!("{flag} requires a value")))?;
        match flag.as_str() {
            "--image" => image_path = Some(PathBuf::from(value)),
            "--output" => output_path = Some(PathBuf::from(value)),
            _ => return Err(invalid(format!("unknown fault bootctrl option '{flag}'"))),
        }
        index += 2;
    }
    let source = existing_target_file(
        root,
        &image_path.ok_or_else(|| invalid("--image is mandatory"))?,
    )?;
    let output = fault_output(
        root,
        &output_path.ok_or_else(|| invalid("--output is mandatory"))?,
    )?;
    if source == output {
        return Err(invalid("fault output must not overwrite its source image"));
    }
    let mut image = fs::read(&source)?;
    let source_sha256 = hex(&Sha256::digest(&image));
    let gpt = parse_gpt(&image, 512)
        .map_err(|error| invalid(format!("fault source GPT is invalid: {error:?}")))?;
    let partition = gpt
        .find_by_name("BOOTCTRL")
        .ok_or_else(|| invalid("fault source has no BOOTCTRL"))?;
    let partition_start = usize::try_from(
        partition
            .first_lba
            .checked_mul(u64::from(gpt.sector_size))
            .ok_or_else(|| invalid("BOOTCTRL offset overflow"))?,
    )?;
    let read_copy = |image: &[u8], offset: u64| -> DynResult<[u8; BOOT_CONTROL_RECORD_SIZE]> {
        let start = partition_start
            .checked_add(usize::try_from(offset)?)
            .ok_or_else(|| invalid("BOOTCTRL copy offset overflow"))?;
        image
            .get(start..start + BOOT_CONTROL_RECORD_SIZE)
            .ok_or_else(|| invalid("BOOTCTRL copy is truncated"))?
            .try_into()
            .map_err(|_| invalid("BOOTCTRL copy length"))
    };
    let first = read_copy(&image, BOOT_CONTROL_COPY_OFFSETS[0])?;
    let second = read_copy(&image, BOOT_CONTROL_COPY_OFFSETS[1])?;
    if first != second {
        return Err(invalid(
            "manual recovery mutation requires equal genesis BOOTCTRL copies",
        ));
    }
    let mut control = BootControl::decode(&first)
        .map_err(|error| invalid(format!("BOOTCTRL decode failed: {error:?}")))?;
    control.flags |= BOOT_CONTROL_FLAG_MANUAL_RECOVERY;
    let encoded = control
        .encode()
        .map_err(|error| invalid(format!("manual BOOTCTRL encode failed: {error:?}")))?;
    for offset in BOOT_CONTROL_COPY_OFFSETS {
        let start = partition_start
            .checked_add(usize::try_from(offset)?)
            .ok_or_else(|| invalid("BOOTCTRL copy offset overflow"))?;
        image[start..start + BOOT_CONTROL_RECORD_SIZE].copy_from_slice(&encoded);
    }
    let output_sha256 = hex(&Sha256::digest(&image));
    let temporary = output.with_extension("raw.candidate");
    fs::write(&temporary, &image)?;
    fs::rename(&temporary, &output)?;
    fs::write(
        output.with_extension("mutation.json"),
        serde_json::to_vec_pretty(&BootControlMutationRecord {
            schema: "eneros.bootctrl-mutation.v1",
            mutation: "manual-recovery",
            source_sha256,
            output_sha256: output_sha256.clone(),
            sequence: control.sequence,
            security_floor: control.security_floor,
            copies_equal: true,
        })?,
    )?;
    println!(
        "FAULT mutation=manual-recovery result=PASS output={} sha256={output_sha256}",
        output.display()
    );
    Ok(())
}

fn parse_options(args: &[String]) -> DynResult<Options> {
    let Some(mutation) = args.first() else {
        return Err(invalid("fault manifest requires a mutation"));
    };
    let mut image = None;
    let mut output = None;
    let mut signing_key = None;
    let mut slot = ManifestSlot::A;
    let mut index = 1usize;
    while index < args.len() {
        let flag = &args[index];
        let value = args
            .get(index + 1)
            .ok_or_else(|| invalid(format!("{flag} requires a value")))?;
        match flag.as_str() {
            "--image" => image = Some(PathBuf::from(value)),
            "--output" => output = Some(PathBuf::from(value)),
            "--signing-key" => signing_key = Some(PathBuf::from(value)),
            "--slot" => {
                slot = match value.as_str() {
                    "A" => ManifestSlot::A,
                    "B" => ManifestSlot::B,
                    "RECOVERY" => ManifestSlot::Recovery,
                    _ => return Err(invalid("--slot must be A, B, or RECOVERY")),
                }
            }
            _ => return Err(invalid(format!("unknown fault manifest option '{flag}'"))),
        }
        index += 2;
    }
    Ok(Options {
        mutation: ManifestMutation::parse(mutation)?,
        image: image.ok_or_else(|| invalid("--image is mandatory"))?,
        output: output.ok_or_else(|| invalid("--output is mandatory"))?,
        signing_key: signing_key.ok_or_else(|| invalid("--signing-key is mandatory"))?,
        slot,
    })
}

fn mutate_manifest(root: &Path, options: Options) -> DynResult<()> {
    let source = existing_target_file(root, &options.image)?;
    let output = fault_output(root, &options.output)?;
    if source == output {
        return Err(invalid("fault output must not overwrite its source image"));
    }
    let key_bytes = fs::read(&options.signing_key)?;
    let seed: [u8; 32] = key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| invalid("fault signing key must be an external raw 32-byte Ed25519 seed"))?;
    let signing_key = SigningKey::from_bytes(&seed);
    let signing_key_id = first_16(Sha256::digest(signing_key.verifying_key().to_bytes()));

    let mut image = fs::read(&source)?;
    let source_sha256 = hex(&Sha256::digest(&image));
    let gpt = parse_gpt(&image, 512)
        .map_err(|error| invalid(format!("fault source GPT is invalid: {error:?}")))?;
    let (partition_name, expected_slot) = match options.slot {
        ManifestSlot::A => ("BOOT_A", ManifestSlot::A),
        ManifestSlot::B => ("BOOT_B", ManifestSlot::B),
        ManifestSlot::Recovery => ("RECOVERY", ManifestSlot::Recovery),
        _ => return Err(invalid("fault manifest slot is not bootable")),
    };
    let partition = gpt
        .find_by_name(partition_name)
        .ok_or_else(|| invalid(format!("fault source has no {partition_name}")))?;
    let manifest_offset = partition
        .first_lba
        .checked_mul(u64::from(gpt.sector_size))
        .ok_or_else(|| invalid("manifest offset overflow"))?;
    let start = usize::try_from(manifest_offset)?;
    let header = image
        .get(start..start + MANIFEST_HEADER_LEN)
        .ok_or_else(|| invalid("manifest header is truncated"))?;
    let length = usize::try_from(u32::from_le_bytes(header[12..16].try_into()?))?;
    let end = start
        .checked_add(length)
        .ok_or_else(|| invalid("manifest end overflow"))?;
    let encoded = image
        .get(start..end)
        .ok_or_else(|| invalid("manifest is truncated"))?;
    let mut manifest = ReleaseManifest::decode(encoded)
        .map_err(|error| invalid(format!("source manifest is invalid: {error:?}")))?;
    if manifest.header.key_id != signing_key_id {
        return Err(invalid(
            "external signing key does not match the source manifest key id",
        ));
    }
    if manifest
        .components
        .iter()
        .any(|component| component.slot != expected_slot)
    {
        return Err(invalid(
            "manifest components do not match the requested slot",
        ));
    }

    let mutated = match options.mutation {
        ManifestMutation::SignatureBitFlip => {
            let mut raw = encoded.to_vec();
            let last = raw
                .last_mut()
                .ok_or_else(|| invalid("manifest signature is absent"))?;
            *last ^= 1;
            raw
        }
        ManifestMutation::SecurityVersionOne => {
            manifest.header.security_version = 1;
            for component in &mut manifest.components {
                component.security_version = 1;
            }
            sign_canonical(manifest, &signing_key)?
        }
        ManifestMutation::WrongPlatform => {
            manifest.header.platform_id[0] ^= 1;
            sign_canonical(manifest, &signing_key)?
        }
        ManifestMutation::KernelLoadOutside => {
            let kernel = manifest
                .components
                .iter_mut()
                .find(|component| component.role == ComponentRole::Kernel)
                .ok_or_else(|| invalid("manifest has no kernel component"))?;
            kernel.load_address = 0x4000_0000;
            kernel.entry_address = kernel.load_address;
            sign_canonical(manifest, &signing_key)?
        }
        ManifestMutation::WxKernel => {
            let mut raw = encoded.to_vec();
            let kernel_index = manifest
                .components
                .iter()
                .position(|component| component.role == ComponentRole::Kernel)
                .ok_or_else(|| invalid("manifest has no kernel component"))?;
            let permission_offset = MANIFEST_HEADER_LEN
                .checked_add(
                    kernel_index
                        .checked_mul(128)
                        .ok_or_else(|| invalid("component offset overflow"))?,
                )
                .and_then(|offset| offset.checked_add(3))
                .ok_or_else(|| invalid("permission offset overflow"))?;
            raw[permission_offset] = PERMISSION_READ | PERMISSION_WRITE | PERMISSION_EXECUTE;
            let signed_len = manifest
                .signed_len()
                .map_err(|error| invalid(format!("manifest signed length: {error:?}")))?;
            resign_raw(&mut raw, signed_len, &signing_key)?;
            raw
        }
    };
    if mutated.len() != length {
        return Err(invalid("mutation changed the manifest allocation length"));
    }
    image[start..end].copy_from_slice(&mutated);
    let output_sha256 = hex(&Sha256::digest(&image));
    let temporary = output.with_extension("raw.candidate");
    fs::write(&temporary, &image)?;
    fs::rename(&temporary, &output)?;

    let record = MutationRecord {
        schema: "eneros.boot-mutation.v1",
        mutation: options.mutation.name(),
        slot: partition_name,
        source_sha256,
        output_sha256: output_sha256.clone(),
        manifest_offset,
        manifest_length: length,
        signing_key_id: hex(&signing_key_id),
    };
    fs::write(
        output.with_extension("mutation.json"),
        serde_json::to_vec_pretty(&record)?,
    )?;
    println!(
        "FAULT mutation={} result=PASS output={} sha256={output_sha256}",
        options.mutation.name(),
        output.display()
    );
    Ok(())
}

fn sign_canonical(mut manifest: ReleaseManifest, signing_key: &SigningKey) -> DynResult<Vec<u8>> {
    manifest.signature = [0; 64];
    let signed = manifest
        .encode_unsigned()
        .map_err(|error| invalid(format!("mutated manifest is not canonical: {error:?}")))?;
    manifest.signature = signing_key.sign(&signed).to_bytes();
    manifest
        .encode_signed()
        .map_err(|error| invalid(format!("signed mutated manifest is invalid: {error:?}")))
}

fn resign_raw(raw: &mut [u8], signed_len: usize, signing_key: &SigningKey) -> DynResult<()> {
    let signature_start = signed_len
        .checked_add(8)
        .ok_or_else(|| invalid("signature offset overflow"))?;
    let signature_end = signature_start
        .checked_add(64)
        .ok_or_else(|| invalid("signature end overflow"))?;
    if signature_end != raw.len() {
        return Err(invalid("raw manifest trailer length is not canonical"));
    }
    let signature = signing_key.sign(&raw[..signed_len]);
    raw[signature_start..signature_end].copy_from_slice(&signature.to_bytes());
    VerifyingKey::from_bytes(&signing_key.verifying_key().to_bytes())?
        .verify(&raw[..signed_len], &signature)?;
    Ok(())
}

fn existing_target_file(root: &Path, path: &Path) -> DynResult<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let path = path.canonicalize()?;
    let target = root.join("target").canonicalize()?;
    if !path.starts_with(&target) || !path.is_file() {
        return Err(invalid(
            "fault source must be an existing file below target",
        ));
    }
    Ok(path)
}

fn fault_output(root: &Path, path: &Path) -> DynResult<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let parent = path
        .parent()
        .ok_or_else(|| invalid("fault output has no parent"))?;
    fs::create_dir_all(parent)?;
    let parent = parent.canonicalize()?;
    let allowed = root.join("target/eneros/faults").canonicalize()?;
    if !parent.starts_with(&allowed) {
        return Err(invalid("fault output must stay below target/eneros/faults"));
    }
    Ok(parent.join(
        path.file_name()
            .ok_or_else(|| invalid("fault output has no file name"))?,
    ))
}

fn first_16(digest: impl AsRef<[u8]>) -> [u8; 16] {
    let mut output = [0u8; 16];
    output.copy_from_slice(&digest.as_ref()[..16]);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_names_are_closed() {
        assert!(ManifestMutation::parse("wrong-platform").is_ok());
        assert!(ManifestMutation::parse("signature-bitflip").is_ok());
        assert!(ManifestMutation::parse("security-version-one").is_ok());
        assert!(ManifestMutation::parse("wx-kernel").is_ok());
        assert!(ManifestMutation::parse("kernel-load-outside").is_ok());
        assert!(ManifestMutation::parse("unsigned").is_err());
        assert!(InstallerMutation::parse("payload-bitflip").is_ok());
        assert!(InstallerMutation::parse("manifest-signature-bitflip").is_ok());
        assert!(InstallerMutation::parse("unsigned").is_err());
        assert!(ArtifactMutation::parse("gpt-bitflip").is_ok());
        assert!(ArtifactMutation::parse("esp-application-bitflip").is_ok());
        assert!(ArtifactMutation::parse("component-bitflip").is_ok());
        assert!(ArtifactMutation::parse("bootctrl-bitflip").is_ok());
        assert!(ArtifactMutation::parse("recovery-empty-bitflip").is_ok());
        assert!(ArtifactMutation::parse("sidecar-bitflip").is_err());
    }
}
