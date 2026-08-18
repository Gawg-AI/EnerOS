#![no_main]
#![no_std]

extern crate alloc;

use alloc::vec;

use ed25519_dalek::{Signature, VerifyingKey};
use eneros_boot_protocol::{InstallManifest, INSTALL_MANIFEST_SIZE};
use sha2::{Digest, Sha256};
use uefi::boot::{self, ScopedProtocol, SearchType};
use uefi::proto::media::block::BlockIO;
use uefi::runtime::{self, ResetType};
use uefi::{entry, print, println, Status};

include!(concat!(env!("OUT_DIR"), "/installer_trust.rs"));

const IO_CHUNK_SIZE: usize = 1024 * 1024;
const ISO_SECTOR_SIZE: usize = 2048;
const ISO_MAX_DIRECTORY_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy)]
struct IsoFile {
    first_lba: u64,
    length: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallFailure {
    SourceNotFound,
    SourceOpen,
    SourceRead,
    ManifestFormat,
    Platform,
    KeyId,
    Rollback,
    Signature,
    PayloadDigest,
    TargetMissing,
    TargetAmbiguous,
    TargetGeometry,
    TargetNotBlank,
    TargetWrite,
    TargetReadback,
}

impl InstallFailure {
    const fn code(self) -> &'static str {
        match self {
            Self::SourceNotFound => "SOURCE_NOT_FOUND",
            Self::SourceOpen => "SOURCE_OPEN",
            Self::SourceRead => "SOURCE_READ",
            Self::ManifestFormat => "INSTALL_MANIFEST",
            Self::Platform => "PLATFORM",
            Self::KeyId => "KEY_ID",
            Self::Rollback => "ROLLBACK",
            Self::Signature => "SIGNATURE",
            Self::PayloadDigest => "PAYLOAD_DIGEST",
            Self::TargetMissing => "TARGET_MISSING",
            Self::TargetAmbiguous => "TARGET_AMBIGUOUS",
            Self::TargetGeometry => "TARGET_GEOMETRY",
            Self::TargetNotBlank => "TARGET_NOT_BLANK",
            Self::TargetWrite => "TARGET_WRITE",
            Self::TargetReadback => "TARGET_READBACK",
        }
    }
}

#[entry]
fn main() -> Status {
    if uefi::helpers::init().is_err() {
        return Status::ABORTED;
    }
    println!("ENEROS stage=installer-entry secure=required");
    match install() {
        Ok((bytes, digest)) => {
            print_install_success(bytes, &digest);
            runtime::reset(ResetType::SHUTDOWN, Status::SUCCESS, None)
        }
        Err(error) => {
            println!(
                "{{\"schema\":\"eneros.install-rejection.v1\",\"result\":\"REJECT\",\"code\":\"{}\"}}",
                error.code()
            );
            runtime::reset(ResetType::SHUTDOWN, Status::SECURITY_VIOLATION, None)
        }
    }
}

fn install() -> Result<(u64, [u8; 32]), InstallFailure> {
    let handles = boot::locate_handle_buffer(SearchType::from_proto::<BlockIO>())
        .map_err(|_| InstallFailure::SourceNotFound)?;
    for &handle in handles.iter() {
        let Ok(mut source) = boot::open_protocol_exclusive::<BlockIO>(handle) else {
            continue;
        };
        let media = source.media();
        if !media.is_media_present()
            || !media.is_read_only()
            || media.block_size() as usize != ISO_SECTOR_SIZE
        {
            continue;
        }
        let Some((manifest_file, payload_file)) = find_iso_payload(&mut source)? else {
            continue;
        };
        let mut encoded = [0u8; INSTALL_MANIFEST_SIZE];
        iso_read_exact(&mut source, manifest_file, 0, &mut encoded)?;
        if manifest_file.length != INSTALL_MANIFEST_SIZE as u64 {
            return Err(InstallFailure::ManifestFormat);
        }
        let manifest = verify_install_manifest(&encoded)?;
        if payload_file.length != manifest.payload_length {
            return Err(InstallFailure::PayloadDigest);
        }
        let source_digest = hash_iso_file(&mut source, payload_file)?;
        if source_digest != manifest.payload_digest {
            return Err(InstallFailure::PayloadDigest);
        }
        let mut target = select_blank_target(&manifest)?;
        write_payload(&mut source, payload_file, &manifest, &mut target)?;
        let readback_digest = hash_target(&manifest, &mut target)?;
        if readback_digest != manifest.payload_digest {
            return Err(InstallFailure::TargetReadback);
        }
        return Ok((manifest.payload_length, readback_digest));
    }
    Err(InstallFailure::SourceNotFound)
}

fn find_iso_payload(
    source: &mut ScopedProtocol<BlockIO>,
) -> Result<Option<(IsoFile, IsoFile)>, InstallFailure> {
    let media_id = source.media().media_id();
    let mut descriptor = [0u8; ISO_SECTOR_SIZE];
    source
        .read_blocks(media_id, 16, &mut descriptor)
        .map_err(|_| InstallFailure::SourceRead)?;
    if descriptor[0] != 1 || descriptor[1..6] != *b"CD001" || descriptor[6] != 1 {
        return Ok(None);
    }
    let root = parse_iso_record(&descriptor[156..], source.media().last_block())?
        .ok_or(InstallFailure::SourceOpen)?;
    let eneros =
        find_iso_entry(source, root, b"ENEROS", true)?.ok_or(InstallFailure::SourceNotFound)?;
    let manifest = find_iso_entry(source, eneros, b"INSTALL.MAN", false)?
        .ok_or(InstallFailure::SourceNotFound)?;
    let payload = find_iso_entry(source, eneros, b"PAYLOAD.RAW", false)?
        .ok_or(InstallFailure::SourceNotFound)?;
    Ok(Some((manifest, payload)))
}

fn find_iso_entry(
    source: &mut ScopedProtocol<BlockIO>,
    directory: IsoFile,
    expected: &[u8],
    expected_directory: bool,
) -> Result<Option<IsoFile>, InstallFailure> {
    let length = usize::try_from(directory.length).map_err(|_| InstallFailure::SourceOpen)?;
    if length == 0 || length > ISO_MAX_DIRECTORY_SIZE {
        return Err(InstallFailure::SourceOpen);
    }
    let mut bytes = vec![0u8; length];
    iso_read_exact(source, directory, 0, &mut bytes)?;
    let last_lba = source.media().last_block();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let record_length = usize::from(bytes[offset]);
        if record_length == 0 {
            offset = offset
                .checked_add(ISO_SECTOR_SIZE)
                .map(|next| next / ISO_SECTOR_SIZE * ISO_SECTOR_SIZE)
                .ok_or(InstallFailure::SourceOpen)?;
            continue;
        }
        let record = bytes
            .get(offset..offset + record_length)
            .ok_or(InstallFailure::SourceOpen)?;
        let file = parse_iso_record(record, last_lba)?.ok_or(InstallFailure::SourceOpen)?;
        let name_length = usize::from(*record.get(32).ok_or(InstallFailure::SourceOpen)?);
        let mut name = record
            .get(33..33 + name_length)
            .ok_or(InstallFailure::SourceOpen)?;
        if let Some(index) = name.iter().position(|byte| *byte == b';') {
            name = &name[..index];
        }
        let is_directory = record.get(25).is_some_and(|flags| flags & 0x02 != 0);
        if name == expected && is_directory == expected_directory {
            return Ok(Some(file));
        }
        offset = offset
            .checked_add(record_length)
            .ok_or(InstallFailure::SourceOpen)?;
    }
    Ok(None)
}

fn parse_iso_record(record: &[u8], last_lba: u64) -> Result<Option<IsoFile>, InstallFailure> {
    let length = usize::from(*record.first().ok_or(InstallFailure::SourceOpen)?);
    if length == 0 {
        return Ok(None);
    }
    if length < 34 || record.len() < length {
        return Err(InstallFailure::SourceOpen);
    }
    let first_lba = u64::from(u32::from_le_bytes(
        record[2..6]
            .try_into()
            .map_err(|_| InstallFailure::SourceOpen)?,
    ));
    let file_length = u64::from(u32::from_le_bytes(
        record[10..14]
            .try_into()
            .map_err(|_| InstallFailure::SourceOpen)?,
    ));
    let blocks = file_length
        .checked_add(ISO_SECTOR_SIZE as u64 - 1)
        .map(|value| value / ISO_SECTOR_SIZE as u64)
        .ok_or(InstallFailure::SourceOpen)?;
    if file_length == 0
        || first_lba
            .checked_add(blocks)
            .filter(|end| *end <= last_lba + 1)
            .is_none()
    {
        return Err(InstallFailure::SourceOpen);
    }
    Ok(Some(IsoFile {
        first_lba,
        length: file_length,
    }))
}

fn verify_install_manifest(encoded: &[u8]) -> Result<InstallManifest, InstallFailure> {
    let manifest = InstallManifest::decode(encoded).map_err(|_| InstallFailure::ManifestFormat)?;
    if manifest.platform_id != PLATFORM_ID {
        return Err(InstallFailure::Platform);
    }
    if manifest.key_id != TRUSTED_KEY_ID {
        return Err(InstallFailure::KeyId);
    }
    if manifest.security_version < MINIMUM_SECURITY_VERSION {
        return Err(InstallFailure::Rollback);
    }
    let key =
        VerifyingKey::from_bytes(&TRUSTED_PUBLIC_KEY).map_err(|_| InstallFailure::Signature)?;
    let signature = Signature::from_bytes(&manifest.signature);
    key.verify_strict(
        &encoded[..eneros_boot_protocol::INSTALL_MANIFEST_SIGNED_SIZE],
        &signature,
    )
    .map_err(|_| InstallFailure::Signature)?;
    Ok(manifest)
}

fn select_blank_target(
    manifest: &InstallManifest,
) -> Result<ScopedProtocol<BlockIO>, InstallFailure> {
    let handles = boot::locate_handle_buffer(SearchType::from_proto::<BlockIO>())
        .map_err(|_| InstallFailure::TargetMissing)?;
    let mut target = None;
    for &handle in handles.iter() {
        let Ok(mut protocol) = boot::open_protocol_exclusive::<BlockIO>(handle) else {
            continue;
        };
        let media = protocol.media();
        if !media.is_media_present()
            || media.is_logical_partition()
            || media.is_read_only()
            || media.block_size() != manifest.sector_size
        {
            continue;
        }
        let total_bytes = media
            .last_block()
            .checked_add(1)
            .and_then(|blocks| blocks.checked_mul(u64::from(media.block_size())))
            .ok_or(InstallFailure::TargetGeometry)?;
        if total_bytes != manifest.payload_length {
            continue;
        }
        if target.is_some() {
            return Err(InstallFailure::TargetAmbiguous);
        }
        ensure_blank(&mut protocol)?;
        target = Some(protocol);
    }
    target.ok_or(InstallFailure::TargetMissing)
}

/// 39 WP-39.4（P1-07 修复）：**全域**空白判定——替代旧"首尾 LBA 空白"
/// 检查。分层扫描全部 LBA：任何非零内容、protective MBR、GPT 签名或
/// 已知文件系统/日志 superblock → 拒绝覆盖（默认拒绝；不存在无授权的
/// `--force`——provisioning 授权属 39 WP-39.3 的显式 capability）。
fn ensure_blank(target: &mut ScopedProtocol<BlockIO>) -> Result<(), InstallFailure> {
    let media = target.media();
    let media_id = media.media_id();
    let total_blocks = media
        .last_block()
        .checked_add(1)
        .ok_or(InstallFailure::TargetGeometry)?;
    let sector = usize::try_from(media.block_size()).map_err(|_| InstallFailure::TargetGeometry)?;
    if sector == 0 {
        return Err(InstallFailure::TargetGeometry);
    }
    let chunk_sectors = (IO_CHUNK_SIZE / sector) as u64;
    if chunk_sectors == 0 {
        return Err(InstallFailure::TargetGeometry);
    }
    // 分块缓存：顺序扫描时每块只读一次（1 MiB，与 write_payload 同级）。
    let mut chunk_cache: Option<(u64, alloc::vec::Vec<u8>)> = None;
    let mut chunk_reads = 0u64;
    let probe = eneros_image_format::surface::probe_surface(
        total_blocks,
        sector,
        chunk_sectors,
        &mut |lba, sector_buf| {
            let chunk_start = lba / chunk_sectors * chunk_sectors;
            let cache_miss = chunk_cache
                .as_ref()
                .map(|(start, _)| *start != chunk_start)
                .unwrap_or(true);
            if cache_miss {
                let mut buf = alloc::vec![0u8; IO_CHUNK_SIZE];
                target
                    .read_blocks(media_id, chunk_start, &mut buf)
                    .map_err(|_| eneros_image_format::surface::SurfaceError::Io)?;
                chunk_reads += 1;
                chunk_cache = Some((chunk_start, buf));
            }
            let (start, data) = chunk_cache
                .as_ref()
                .ok_or(eneros_image_format::surface::SurfaceError::Io)?;
            let off = ((lba - start) * sector as u64) as usize;
            sector_buf.copy_from_slice(&data[off..off + sector]);
            Ok(())
        },
    )
    .map_err(|_| InstallFailure::TargetReadback)?;
    // 裁决：默认拒绝任何未知非零内容/已知布局（fail-closed）。
    if probe.first_nonzero_lba.is_some()
        || probe.protective_mbr
        || probe.gpt_signature
        || !probe.fs_signatures.is_empty()
    {
        return Err(InstallFailure::TargetNotBlank);
    }
    let _ = chunk_reads;
    Ok(())
}

fn write_payload(
    source: &mut ScopedProtocol<BlockIO>,
    payload: IsoFile,
    manifest: &InstallManifest,
    target: &mut ScopedProtocol<BlockIO>,
) -> Result<(), InstallFailure> {
    let sector =
        usize::try_from(manifest.sector_size).map_err(|_| InstallFailure::TargetGeometry)?;
    let chunk_size = IO_CHUNK_SIZE / sector * sector;
    if chunk_size == 0 {
        return Err(InstallFailure::TargetGeometry);
    }
    let mut buffer = vec![0u8; chunk_size];
    let mut remaining = manifest.payload_length;
    let mut lba = 0u64;
    let media_id = target.media().media_id();
    let mut written_hash = Sha256::new();
    let mut source_offset = 0u64;
    while remaining != 0 {
        let length = usize::try_from(remaining.min(chunk_size as u64))
            .map_err(|_| InstallFailure::SourceRead)?;
        iso_read_exact(source, payload, source_offset, &mut buffer[..length])?;
        written_hash.update(&buffer[..length]);
        target
            .write_blocks(media_id, lba, &buffer[..length])
            .map_err(|_| InstallFailure::TargetWrite)?;
        remaining -= length as u64;
        source_offset = source_offset
            .checked_add(length as u64)
            .ok_or(InstallFailure::SourceRead)?;
        lba = lba
            .checked_add((length / sector) as u64)
            .ok_or(InstallFailure::TargetGeometry)?;
    }
    target
        .flush_blocks()
        .map_err(|_| InstallFailure::TargetWrite)?;
    let digest: [u8; 32] = written_hash.finalize().into();
    if digest != manifest.payload_digest {
        return Err(InstallFailure::PayloadDigest);
    }
    Ok(())
}

fn hash_target(
    manifest: &InstallManifest,
    target: &mut ScopedProtocol<BlockIO>,
) -> Result<[u8; 32], InstallFailure> {
    let sector =
        usize::try_from(manifest.sector_size).map_err(|_| InstallFailure::TargetGeometry)?;
    let chunk_size = IO_CHUNK_SIZE / sector * sector;
    let mut buffer = vec![0u8; chunk_size];
    let media_id = target.media().media_id();
    let mut hash = Sha256::new();
    let mut remaining = manifest.payload_length;
    let mut lba = 0u64;
    while remaining != 0 {
        let length = usize::try_from(remaining.min(chunk_size as u64))
            .map_err(|_| InstallFailure::TargetReadback)?;
        target
            .read_blocks(media_id, lba, &mut buffer[..length])
            .map_err(|_| InstallFailure::TargetReadback)?;
        hash.update(&buffer[..length]);
        remaining -= length as u64;
        lba = lba
            .checked_add((length / sector) as u64)
            .ok_or(InstallFailure::TargetGeometry)?;
    }
    Ok(hash.finalize().into())
}

fn hash_iso_file(
    source: &mut ScopedProtocol<BlockIO>,
    file: IsoFile,
) -> Result<[u8; 32], InstallFailure> {
    let mut buffer = vec![0u8; IO_CHUNK_SIZE];
    let mut remaining = file.length;
    let mut offset = 0u64;
    let mut hash = Sha256::new();
    while remaining != 0 {
        let size = usize::try_from(remaining.min(IO_CHUNK_SIZE as u64))
            .map_err(|_| InstallFailure::SourceRead)?;
        iso_read_exact(source, file, offset, &mut buffer[..size])?;
        hash.update(&buffer[..size]);
        remaining -= size as u64;
        offset = offset
            .checked_add(size as u64)
            .ok_or(InstallFailure::SourceRead)?;
    }
    Ok(hash.finalize().into())
}

fn iso_read_exact(
    source: &mut ScopedProtocol<BlockIO>,
    file: IsoFile,
    offset: u64,
    mut output: &mut [u8],
) -> Result<(), InstallFailure> {
    let requested = u64::try_from(output.len()).map_err(|_| InstallFailure::SourceRead)?;
    if offset
        .checked_add(requested)
        .filter(|end| *end <= file.length)
        .is_none()
    {
        return Err(InstallFailure::SourceRead);
    }
    let media_id = source.media().media_id();
    let mut absolute = file
        .first_lba
        .checked_mul(ISO_SECTOR_SIZE as u64)
        .and_then(|start| start.checked_add(offset))
        .ok_or(InstallFailure::SourceRead)?;
    let mut scratch = [0u8; ISO_SECTOR_SIZE];
    while !output.is_empty() {
        let within = usize::try_from(absolute % ISO_SECTOR_SIZE as u64)
            .map_err(|_| InstallFailure::SourceRead)?;
        if within == 0 && output.len() >= ISO_SECTOR_SIZE {
            let direct = output.len() / ISO_SECTOR_SIZE * ISO_SECTOR_SIZE;
            let (head, tail) = output.split_at_mut(direct);
            source
                .read_blocks(media_id, absolute / ISO_SECTOR_SIZE as u64, head)
                .map_err(|_| InstallFailure::SourceRead)?;
            absolute = absolute
                .checked_add(direct as u64)
                .ok_or(InstallFailure::SourceRead)?;
            output = tail;
        } else {
            source
                .read_blocks(media_id, absolute / ISO_SECTOR_SIZE as u64, &mut scratch)
                .map_err(|_| InstallFailure::SourceRead)?;
            let count = output.len().min(ISO_SECTOR_SIZE - within);
            output[..count].copy_from_slice(&scratch[within..within + count]);
            absolute = absolute
                .checked_add(count as u64)
                .ok_or(InstallFailure::SourceRead)?;
            output = &mut output[count..];
        }
    }
    Ok(())
}

fn print_install_success(bytes: u64, digest: &[u8; 32]) {
    print!(
        "{{\"schema\":\"eneros.install-evidence.v1\",\"result\":\"PASS\",\"bytes\":{},\"payload_sha256\":\"",
        bytes
    );
    for byte in digest {
        print!("{byte:02x}");
    }
    println!("\",\"readback\":\"PASS\"}}");
}
