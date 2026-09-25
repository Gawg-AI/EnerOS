#![no_main]
#![no_std]

extern crate alloc;

use alloc::vec;

use eneros_boot_protocol::{InstallManifest, INSTALL_MANIFEST_SIZE};
use eneros_installer_uefi::{
    find_iso_payload, iso_read_exact, verify_install_manifest, InstallFailure, IsoFile, IsoIo,
    IsoMedia, TrustAnchor, ISO_SECTOR_SIZE,
};
use sha2::{Digest, Sha256};
use uefi::boot::{self, ScopedProtocol, SearchType};
use uefi::proto::media::block::BlockIO;
use uefi::runtime::{self, ResetType};
use uefi::{entry, print, println, Status};

include!(concat!(env!("OUT_DIR"), "/installer_trust.rs"));

const IO_CHUNK_SIZE: usize = 1024 * 1024;

/// UEFI `BlockIO` → [`IsoMedia`] adapter（bin 侧读管道；lib 解析逻辑
/// 经 trait 与 UEFI 解耦——WP-74.3.2）.
struct BlockIoMedia<'a> {
    io: &'a mut ScopedProtocol<BlockIO>,
}

impl IsoMedia for BlockIoMedia<'_> {
    fn read_exact(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IsoIo> {
        let media_id = self.io.media().media_id();
        self.io
            .read_blocks(media_id, lba, buf)
            .map_err(|_| IsoIo::Read)
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
        let (iso_media, last_lba) = {
            let media = source.media();
            (
                media.is_media_present()
                    && media.is_read_only()
                    && media.block_size() as usize == ISO_SECTOR_SIZE,
                media.last_block(),
            )
        };
        if !iso_media {
            continue;
        }
        // ISO 读相（定位/验签/源摘要）：adapter 持 source 可变借用，
        // 块结束释放后写相重建 adapter（写盘读源同管道）。
        let (manifest, payload_file) = {
            let mut media_adapter = BlockIoMedia { io: &mut source };
            let Some((manifest_file, payload_file)) =
                find_iso_payload(&mut media_adapter, last_lba)?
            else {
                continue;
            };
            let mut encoded = [0u8; INSTALL_MANIFEST_SIZE];
            iso_read_exact(&mut media_adapter, manifest_file, 0, &mut encoded)?;
            if manifest_file.length != INSTALL_MANIFEST_SIZE as u64 {
                return Err(InstallFailure::ManifestFormat);
            }
            // WP-74.3.1：验签唯一入口 = BootSignatureProvider（lib 层）；
            // 锚由 build.rs 生成的信任常量构造（生成链不动）。
            let anchor = TrustAnchor {
                public_key: TRUSTED_PUBLIC_KEY,
                key_id: TRUSTED_KEY_ID,
                platform_id: PLATFORM_ID,
                minimum_security_version: MINIMUM_SECURITY_VERSION,
            };
            let manifest = verify_install_manifest(&encoded, &anchor)?;
            if payload_file.length != manifest.payload_length {
                return Err(InstallFailure::PayloadDigest);
            }
            let source_digest = hash_iso_file(&mut media_adapter, payload_file)?;
            if source_digest != manifest.payload_digest {
                return Err(InstallFailure::PayloadDigest);
            }
            (manifest, payload_file)
        };
        let mut target = select_blank_target(&manifest)?;
        let mut media_adapter = BlockIoMedia { io: &mut source };
        write_payload(&mut media_adapter, payload_file, &manifest, &mut target)?;
        let readback_digest = hash_target(&manifest, &mut target)?;
        if readback_digest != manifest.payload_digest {
            return Err(InstallFailure::TargetReadback);
        }
        return Ok((manifest.payload_length, readback_digest));
    }
    Err(InstallFailure::SourceNotFound)
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

fn write_payload<M: IsoMedia + ?Sized>(
    source: &mut M,
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

fn hash_iso_file<M: IsoMedia + ?Sized>(
    source: &mut M,
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
